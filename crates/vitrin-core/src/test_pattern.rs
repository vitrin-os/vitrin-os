// SPDX-License-Identifier: MPL-2.0
//! The deterministic test pattern the skeleton renders.
//!
//! [`render`] is a pure function of the dimensions — integer math only, no
//! time, no randomness — because the P1.3.2 headless capture and the
//! P1.3.6 capture-service goldens assert on these exact pixels.
//!
//! Layout (recognizable at a glance, diagnostic under a diff):
//! - top ~70 %: eight vertical color bars (SMPTE-style order);
//! - bottom ~30 %: a 16 px checkerboard (scaling/filtering artifacts show
//!   immediately);
//! - corner markers in *distinct* colors — red TL, green TR, blue BL,
//!   white BR — so any flip or rotation anywhere in the pipeline is visible
//!   and machine-checkable.

/// Bytes per pixel: tightly packed RGBA8888 (imported as DRM fourcc
/// `ABGR8888`, i.e. bytes R,G,B,A on little-endian).
pub const BYTES_PER_PIXEL: usize = 4;

/// Fraction of the height (in percent) taken by the color bars.
const BARS_HEIGHT_PCT: u32 = 70;
/// Checkerboard cell edge in pixels.
const CHECKER_CELL: u32 = 16;
/// Corner-marker edge in pixels (clamped for tiny windows).
const MARKER: u32 = 24;

/// The eight color bars, left to right (RGBA).
const BARS: [[u8; 4]; 8] = [
    [0xff, 0xff, 0xff, 0xff], // white
    [0xff, 0xff, 0x00, 0xff], // yellow
    [0x00, 0xff, 0xff, 0xff], // cyan
    [0x00, 0xff, 0x00, 0xff], // green
    [0xff, 0x00, 0xff, 0xff], // magenta
    [0xff, 0x00, 0x00, 0xff], // red
    [0x00, 0x00, 0xff, 0xff], // blue
    [0x20, 0x20, 0x20, 0xff], // near-black
];

/// Checkerboard colors (RGBA).
const CHECKER_LIGHT: [u8; 4] = [0xaa, 0xaa, 0xaa, 0xff];
const CHECKER_DARK: [u8; 4] = [0x33, 0x33, 0x33, 0xff];

/// Corner markers (RGBA): top-left, top-right, bottom-left, bottom-right.
pub const MARKER_TOP_LEFT: [u8; 4] = [0xff, 0x00, 0x00, 0xff]; // red
pub const MARKER_TOP_RIGHT: [u8; 4] = [0x00, 0xff, 0x00, 0xff]; // green
pub const MARKER_BOTTOM_LEFT: [u8; 4] = [0x00, 0x00, 0xff, 0xff]; // blue
pub const MARKER_BOTTOM_RIGHT: [u8; 4] = [0xff, 0xff, 0xff, 0xff]; // white

/// Render the test pattern as a tightly packed RGBA8888 buffer of
/// `width * height * 4` bytes, rows top-down. Zero-sized dimensions yield
/// an empty buffer.
pub fn render(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
    for y in 0..height {
        for x in 0..width {
            let rgba = pixel(x, y, width, height);
            let offset = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
            pixels[offset..offset + BYTES_PER_PIXEL].copy_from_slice(&rgba);
        }
    }
    pixels
}

/// The pattern color at `(x, y)` for a `width x height` buffer.
fn pixel(x: u32, y: u32, width: u32, height: u32) -> [u8; 4] {
    // Corner markers paint over everything else.
    let marker = MARKER.min(width / 4).min(height / 4).max(1);
    let left = x < marker;
    let right = x >= width - marker;
    let top = y < marker;
    let bottom = y >= height - marker;
    match (left, right, top, bottom) {
        (true, _, true, _) => return MARKER_TOP_LEFT,
        (_, true, true, _) => return MARKER_TOP_RIGHT,
        (true, _, _, true) => return MARKER_BOTTOM_LEFT,
        (_, true, _, true) => return MARKER_BOTTOM_RIGHT,
        _ => {}
    }

    let bars_height = height * BARS_HEIGHT_PCT / 100;
    if y < bars_height {
        // `x < width` keeps the index strictly below BARS.len().
        let bar = (x as u64 * BARS.len() as u64 / width as u64) as usize;
        BARS[bar]
    } else if (x / CHECKER_CELL + y / CHECKER_CELL).is_multiple_of(2) {
        CHECKER_LIGHT
    } else {
        CHECKER_DARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RGBA quadruple at (x, y).
    fn px(buf: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
        buf[offset..offset + BYTES_PER_PIXEL].try_into().unwrap()
    }

    #[test]
    fn buffer_has_exact_dimensions() {
        assert_eq!(render(1280, 800).len(), 1280 * 800 * BYTES_PER_PIXEL);
        assert_eq!(render(1, 1).len(), BYTES_PER_PIXEL);
    }

    #[test]
    fn zero_sized_dimensions_yield_empty_buffers() {
        assert!(render(0, 600).is_empty());
        assert!(render(800, 0).is_empty());
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(render(640, 480), render(640, 480));
    }

    #[test]
    fn corner_markers_encode_orientation() {
        let (w, h) = (1280, 800);
        let buf = render(w, h);
        assert_eq!(px(&buf, w, 0, 0), MARKER_TOP_LEFT);
        assert_eq!(px(&buf, w, w - 1, 0), MARKER_TOP_RIGHT);
        assert_eq!(px(&buf, w, 0, h - 1), MARKER_BOTTOM_LEFT);
        assert_eq!(px(&buf, w, w - 1, h - 1), MARKER_BOTTOM_RIGHT);
        // All four are distinct, so no flip/rotation maps the pattern
        // onto itself.
        let corners = [
            MARKER_TOP_LEFT,
            MARKER_TOP_RIGHT,
            MARKER_BOTTOM_LEFT,
            MARKER_BOTTOM_RIGHT,
        ];
        for (i, a) in corners.iter().enumerate() {
            for b in &corners[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn color_bars_run_left_to_right() {
        let (w, h) = (800, 600);
        let buf = render(w, h);
        let y = h * BARS_HEIGHT_PCT / 200; // vertically centered in the bars
        for (i, expected) in BARS.iter().enumerate() {
            // Horizontal center of bar i.
            let x = (w * (2 * i as u32 + 1)) / (2 * BARS.len() as u32);
            assert_eq!(px(&buf, w, x, y), *expected, "bar {i}");
        }
    }

    #[test]
    fn checkerboard_alternates() {
        let (w, h) = (800, 600);
        let buf = render(w, h);
        let y = h - CHECKER_CELL / 2; // inside the bottom band
        let x = w / 2;
        let a = px(&buf, w, x, y);
        let b = px(&buf, w, x + CHECKER_CELL, y);
        assert_ne!(a, b);
        assert_eq!(px(&buf, w, x + 2 * CHECKER_CELL, y), a);
    }

    #[test]
    fn every_pixel_is_opaque() {
        let buf = render(97, 41); // odd sizes exercise the clamped marker
        for rgba in buf.chunks_exact(BYTES_PER_PIXEL) {
            assert_eq!(rgba[3], 0xff);
        }
    }
}
