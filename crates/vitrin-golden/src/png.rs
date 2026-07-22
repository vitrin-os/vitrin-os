//! A minimal, hand-rolled PNG encoder — just enough to write the harness's
//! debugging artifacts (`actual.png`, `expected.png`, `diff.png`).
//!
//! Scope is deliberately tiny: 8-bit truecolor (RGB, no alpha), no filtering
//! (every scanline uses filter type 0), and DEFLATE **stored** blocks only
//! (BTYPE 00 — no Huffman, no LZ77). Stored blocks make a valid zlib stream
//! that every PNG decoder accepts while keeping the encoder to arithmetic a
//! reviewer can check by eye; the artifacts are for human eyes on a failing
//! test, not for size. This is why the crate needs no `png`/`flate2`
//! dependency (plan risk R7).

use std::fs;
use std::io;
use std::path::Path;

/// The 8-byte PNG signature.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// Encode `rgb` (tightly packed, `width * height * 3` bytes, rows top-down)
/// as an 8-bit truecolor PNG and write it to `path`.
pub fn write_rgb(path: &Path, width: u32, height: u32, rgb: &[u8]) -> io::Result<()> {
    debug_assert_eq!(rgb.len(), width as usize * height as usize * 3);
    fs::write(path, encode_rgb(width, height, rgb))
}

/// Encode `rgb` as a complete PNG byte stream.
fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&SIGNATURE);

    // IHDR: dimensions, bit depth 8, color type 2 (truecolor RGB), the three
    // trailing zeros = deflate compression, adaptive filtering, no interlace.
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT: a zlib stream wrapping the filtered scanlines.
    let raw = filtered_scanlines(width, height, rgb);
    write_chunk(&mut out, b"IDAT", &zlib_stored(&raw));

    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Prefix every row with a filter-type byte of 0 ("None"): the raw bytes a
/// PNG decoder reconstructs the image from.
fn filtered_scanlines(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let row = width as usize * 3;
    let mut out = Vec::with_capacity(height as usize * (row + 1));
    for y in 0..height as usize {
        out.push(0);
        out.extend_from_slice(&rgb[y * row..y * row + row]);
    }
    out
}

/// Wrap `data` in a zlib stream built entirely from DEFLATE stored blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // zlib header: CMF=0x78 (deflate, 32 KiB window), FLG=0x01 (fastest, no
    // preset dictionary) — the two together are a multiple of 31, as the
    // spec's check requires.
    let mut out = vec![0x78, 0x01];

    // Stored blocks: each carries at most 0xFFFF payload bytes. Emit at least
    // one block even for empty input so BFINAL is always set exactly once.
    let mut pos = 0usize;
    loop {
        let len = (data.len() - pos).min(0xFFFF);
        let is_final = pos + len >= data.len();
        out.push(u8::from(is_final)); // BFINAL bit; BTYPE 00 (stored)
        let len16 = len as u16;
        out.extend_from_slice(&len16.to_le_bytes());
        out.extend_from_slice(&(!len16).to_le_bytes());
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
        if is_final {
            break;
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Append a length-prefixed, CRC-suffixed PNG chunk.
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// Bitwise CRC-32 (the PNG/zlib polynomial), no lookup table — the artifacts
/// are small, so per-byte bit twiddling is plenty and keeps this auditable.
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u32::from(b);
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(self) -> u32 {
        !self.0
    }
}

/// Adler-32 checksum of `data` (the zlib trailer).
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_well_formed_png_container() {
        // 2x1 RGB image: one red pixel, one green pixel.
        let png = encode_rgb(2, 1, &[0xff, 0x00, 0x00, 0x00, 0xff, 0x00]);
        assert_eq!(&png[..8], &SIGNATURE, "PNG signature");
        assert!(
            contains(&png, b"IHDR") && contains(&png, b"IDAT") && contains(&png, b"IEND"),
            "the three chunks are present in order"
        );
        // IEND is the last chunk: 4 length + 4 tag + 0 data + 4 crc.
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn adler32_matches_the_known_vector() {
        // zlib's canonical example: Adler-32 of "Wikipedia" is 0x11E60398.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn crc32_matches_the_known_vector() {
        // CRC-32 of the ASCII string "123456789" is 0xCBF43926.
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
    }

    #[test]
    fn empty_input_still_yields_one_final_stored_block() {
        let z = zlib_stored(&[]);
        // header(2) + block header(1) + LEN(2) + NLEN(2) + adler(4).
        assert_eq!(z.len(), 11);
        assert_eq!(z[2] & 1, 1, "the single block is marked final");
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
