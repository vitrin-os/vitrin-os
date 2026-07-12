//! Hand-written wire-format primitives shared by every generated message's
//! `encode`/`decode`. This is runtime support code, not generated output: the
//! byte-level rules here (header shape, string padding, bounds checks) are
//! fixed by `docs/protocol/00-conventions.md` section 2 and do not vary per
//! interface or message, so the generator emits calls into this module
//! instead of re-emitting the same bit-twiddling in every generated file.
//!
//! Nothing in this module performs I/O: it only reads from and writes to
//! in-memory `&[u8]` / `Vec<u8>` buffers. Getting bytes on and off a real
//! socket (including the out-of-band `SCM_RIGHTS` fd transfer) is a different
//! crate's job.

use crate::error::DecodeError;

/// Size of the fixed frame header: `object_id (u32)`, `size (u16)`,
/// `opcode (u8)`, `fd_count (u8)`.
pub const HEADER_LEN: usize = 8;

/// The 8-byte frame header, little-endian throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub object_id: u32,
    /// Whole frame including this header. Frames above 65535 bytes are a
    /// fatal "oversized" condition at the transport layer; that policy lives
    /// outside this crate, which only encodes/decodes the field.
    pub size: u16,
    pub opcode: u8,
    /// Always 0 or 1 in v0 (at most one fd per message).
    pub fd_count: u8,
}

impl FrameHeader {
    /// Decode the header from the front of `bytes`. `bytes` may be longer
    /// than just the header; only the first 8 bytes are consumed.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Truncated {
                needed: HEADER_LEN,
                available: bytes.len(),
            });
        }
        let object_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let size = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let opcode = bytes[6];
        let fd_count = bytes[7];
        Ok(FrameHeader {
            object_id,
            size,
            opcode,
            fd_count,
        })
    }

    /// Append the header to `out`, with a placeholder `0` for `size` --
    /// callers must follow up with [`patch_size`] once the whole frame
    /// (header + body) has been written.
    pub fn encode_with_placeholder_size(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.object_id.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.push(self.opcode);
        out.push(self.fd_count);
    }
}

/// Overwrite the `size` field of an already-encoded frame with `out.len()`,
/// once the whole frame (header + argument payload) has been appended to
/// `out`. Every message in this IDL is well within the 65535-byte limit by
/// construction (the largest, `vitrin_handshake.hello`, tops out under 35 KiB
/// even at every string argument's maximum declared length) -- *provided*
/// every string argument was itself written within its own documented bound,
/// which [`write_string`] now enforces unconditionally. This is a real
/// `assert!`, not `debug_assert!`: it is cheap (one comparison per encoded
/// frame) and it is the last line of defense against a `u16` truncation that
/// would otherwise silently corrupt the `size` field instead of failing
/// loudly -- see `write_string`'s doc comment for the primary defense.
pub fn patch_size(out: &mut [u8]) {
    assert!(
        out.len() <= u16::MAX as usize,
        "encoded frame of {} bytes exceeds the 65535-byte wire limit",
        out.len()
    );
    let size = out.len() as u16;
    out[4..6].copy_from_slice(&size.to_le_bytes());
}

pub fn write_uint(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn write_int(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn read_uint(bytes: &[u8], pos: &mut usize) -> Result<u32, DecodeError> {
    let end = *pos + 4;
    if end > bytes.len() {
        return Err(DecodeError::Truncated {
            needed: end,
            available: bytes.len(),
        });
    }
    let v = u32::from_le_bytes(bytes[*pos..end].try_into().unwrap());
    *pos = end;
    Ok(v)
}

pub fn read_int(bytes: &[u8], pos: &mut usize) -> Result<i32, DecodeError> {
    read_uint(bytes, pos).map(|v| v as i32)
}

/// Encode a string argument: `u32` byte length, the UTF-8 bytes themselves
/// (no NUL terminator), zero-padded to the next 4-byte boundary. The length
/// prefix counts only the UTF-8 bytes, never the padding.
///
/// `max_bytes` is the argument's own documented `(max N bytes)` bound (the
/// same bound [`read_string`] enforces on decode). This function panics if
/// `s` exceeds it, on purpose: encoding only ever runs over values the
/// caller (ultimately the trusted core) constructed itself, so an
/// over-length string here is a caller bug, not untrusted input -- there is
/// no peer to send a recoverable error to. Enforcing the bound with a hard
/// panic, rather than silently writing the oversized bytes and leaving
/// `patch_size` to truncate the total frame length into its `u16` field, is
/// the fix for exactly that failure mode: without this check, a string
/// argument that overran its bound could silently wrap the frame's `size`
/// field (`out.len() as u16` in `patch_size`), producing a frame whose
/// header claims a length far shorter than its real byte count -- corrupt
/// on the wire with no error raised anywhere. Panicking here, at the
/// specific field that violated its bound, is strictly earlier and more
/// diagnosable than the previous `debug_assert!` in `patch_size` (which was
/// also compiled out entirely in release builds).
#[track_caller]
pub fn write_string(out: &mut Vec<u8>, s: &str, max_bytes: u32) {
    let bytes = s.as_bytes();
    assert!(
        bytes.len() as u64 <= max_bytes as u64,
        "string argument of {} bytes exceeds its documented maximum of {max_bytes} bytes",
        bytes.len()
    );
    write_uint(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
    let padding = pad_len(bytes.len());
    out.extend(std::iter::repeat_n(0u8, padding));
}

/// Decode a string argument, enforcing `max_bytes` (the arg's documented
/// `(max N bytes)` bound), UTF-8 validity, and the no-embedded-NUL rule.
/// Padding bytes are consumed but their content is not itself validated.
pub fn read_string(bytes: &[u8], pos: &mut usize, max_bytes: u32) -> Result<String, DecodeError> {
    let len = read_uint(bytes, pos)?;
    if len > max_bytes {
        return Err(DecodeError::StringTooLong {
            max: max_bytes,
            actual: len,
        });
    }
    let len = len as usize;
    let end = *pos + len;
    if end > bytes.len() {
        return Err(DecodeError::Truncated {
            needed: end,
            available: bytes.len(),
        });
    }
    let raw = &bytes[*pos..end];
    if raw.contains(&0u8) {
        return Err(DecodeError::EmbeddedNul);
    }
    let s = std::str::from_utf8(raw)
        .map_err(|_| DecodeError::InvalidUtf8)?
        .to_string();
    *pos = end;

    let padding = pad_len(len);
    let padded_end = *pos + padding;
    if padded_end > bytes.len() {
        return Err(DecodeError::Truncated {
            needed: padded_end,
            available: bytes.len(),
        });
    }
    *pos = padded_end;

    Ok(s)
}

/// Bytes of zero padding needed to bring `len` up to the next 4-byte boundary.
fn pad_len(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_round_trip_and_padding() {
        for s in ["", "a", "ab", "abc", "abcd", "hello, world", "héllo"] {
            let mut out = Vec::new();
            write_string(&mut out, s, 1024);
            assert_eq!(
                out.len() % 4,
                0,
                "encoded string buffer must be 4-byte aligned"
            );
            let mut pos = 0;
            let decoded = read_string(&out, &mut pos, 1024).unwrap();
            assert_eq!(decoded, s);
            assert_eq!(pos, out.len());
        }
    }

    #[test]
    fn string_over_bound_rejected() {
        let mut out = Vec::new();
        write_string(&mut out, "hello", 1024);
        let mut pos = 0;
        assert_eq!(
            read_string(&out, &mut pos, 3),
            Err(DecodeError::StringTooLong { max: 3, actual: 5 })
        );
    }

    #[test]
    #[should_panic(expected = "exceeds its documented maximum")]
    fn write_string_over_bound_panics_instead_of_corrupting_the_frame() {
        // Regression test: write_string used to have no bound check at all,
        // so an over-length string would silently pass through patch_size's
        // `out.len() as u16` cast and wrap the frame's size field instead of
        // failing. This must now panic before a single byte is written,
        // never produce a frame with a wrong size field.
        let mut out = Vec::new();
        write_string(&mut out, "hello", 3);
    }

    #[test]
    fn embedded_nul_rejected() {
        let mut out = Vec::new();
        write_uint(&mut out, 3);
        out.extend_from_slice(b"a\0b");
        let mut pos = 0;
        assert_eq!(
            read_string(&out, &mut pos, 1024),
            Err(DecodeError::EmbeddedNul)
        );
    }

    #[test]
    fn invalid_utf8_rejected() {
        let mut out = Vec::new();
        write_uint(&mut out, 2);
        out.extend_from_slice(&[0xff, 0xfe]);
        let mut pos = 0;
        assert_eq!(
            read_string(&out, &mut pos, 1024),
            Err(DecodeError::InvalidUtf8)
        );
    }

    #[test]
    fn header_round_trip() {
        let mut out = Vec::new();
        FrameHeader {
            object_id: 42,
            size: 0,
            opcode: 3,
            fd_count: 1,
        }
        .encode_with_placeholder_size(&mut out);
        out.extend_from_slice(&[9, 9, 9, 9]);
        patch_size(&mut out);
        let header = FrameHeader::decode(&out).unwrap();
        assert_eq!(header.object_id, 42);
        assert_eq!(header.size, 12);
        assert_eq!(header.opcode, 3);
        assert_eq!(header.fd_count, 1);
    }
}
