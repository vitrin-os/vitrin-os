// SPDX-License-Identifier: Apache-2.0
//! Golden-frame comparison harness (P1.9.2).
//!
//! Shared pixel-assertion infrastructure: compare a freshly captured frame
//! against a checked-in reference image and produce a [`Report`]. Three
//! policies span the range from "must be identical" to "must merely look the
//! same":
//!
//! - [`Policy::Exact`] — byte-for-byte equality. What the headless
//!   test-pattern golden and the wire-format goldens use: deterministic
//!   integer renders that must not move a single pixel.
//! - [`Policy::Tolerance`] — allow a bounded per-channel difference on a
//!   bounded fraction of pixels. The knob for content that is deterministic
//!   in shape but not to the last least-significant bit.
//! - [`Policy::Ssim`] — structural similarity, the fallback where exactness
//!   is unreasonable (GPU output vs. a pixman software render: the same
//!   scene, different anti-aliasing).
//!
//! When a comparison fails, [`write_artifacts`] drops `actual.png`,
//! `expected.png` and an amplified `diff.png` heatmap next to each other so
//! the break is legible at a glance.
//!
//! Goldens are only ever regenerated through the single documented
//! entrypoint `cargo xtask bless` — see `tests/golden/README.md`.
//!
//! The crate has **no external dependencies**: the SSIM arithmetic is
//! hand-rolled here and the PNG encoder is hand-rolled in-tree
//! ([`vitrin_png`]), keeping every image codec out of the tree exactly as plan
//! risk R7 requires.
//!
//! The encoder used to live here as `src/png.rs`. WS-E.2.4 (issue #216) needed
//! the same arithmetic inside the trusted core, which may not take an image
//! codec in any dependency class — so the file moved to its own zero-dependency
//! crate and both consumers share one reviewed copy rather than two that drift.

use vitrin_png as png;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Bytes per pixel — every frame this harness handles is tightly packed
/// 4-byte pixels (RGBA8888 or XRGB8888), the two layouts the rest of the
/// codebase already speaks (`test_pattern` renders RGBA; the capture wire
/// format is XRGB).
const BYTES_PER_PIXEL: usize = 4;

/// How the four bytes of each pixel are ordered.
///
/// The harness never invents a pixel format: these are exactly the two the
/// core already uses — `test_pattern::render` emits [`Rgba8888`], and
/// `vitrin_view.frame_ready` delivers [`Xrgb8888`].
///
/// [`Rgba8888`]: PixelFormat::Rgba8888
/// [`Xrgb8888`]: PixelFormat::Xrgb8888
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Bytes `R, G, B, A` (the test-pattern / headless readback layout).
    Rgba8888,
    /// Bytes `B, G, R, X` little-endian (the capture wire layout; the fourth
    /// byte is padding, pinned `0xFF`).
    Xrgb8888,
}

impl PixelFormat {
    /// Byte offsets of the red, green and blue channels within a pixel.
    fn rgb_offsets(self) -> (usize, usize, usize) {
        match self {
            PixelFormat::Rgba8888 => (0, 1, 2),
            PixelFormat::Xrgb8888 => (2, 1, 0),
        }
    }
}

/// A borrowed view over one tightly packed 4-byte-per-pixel image: the unit
/// both sides of a comparison are expressed in.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Byte order of each pixel.
    pub format: PixelFormat,
    /// `width * height * 4` tightly packed bytes, rows top-down.
    pub bytes: &'a [u8],
}

impl<'a> Frame<'a> {
    /// A frame of an explicit [`PixelFormat`].
    pub fn new(width: u32, height: u32, format: PixelFormat, bytes: &'a [u8]) -> Self {
        Frame {
            width,
            height,
            format,
            bytes,
        }
    }

    /// An RGBA8888 frame (bytes `R, G, B, A`) — the `test_pattern::render`
    /// and headless-readback layout.
    pub fn rgba(width: u32, height: u32, bytes: &'a [u8]) -> Self {
        Frame::new(width, height, PixelFormat::Rgba8888, bytes)
    }

    /// An XRGB8888 frame (little-endian bytes `B, G, R, X`) — the
    /// `vitrin_view.frame_ready` wire layout.
    pub fn xrgb(width: u32, height: u32, bytes: &'a [u8]) -> Self {
        Frame::new(width, height, PixelFormat::Xrgb8888, bytes)
    }

    /// The byte length a well-formed frame of these dimensions must have.
    fn expected_len(&self) -> usize {
        self.width as usize * self.height as usize * BYTES_PER_PIXEL
    }

    /// Whether `bytes` actually holds `width * height` pixels.
    fn is_well_formed(&self) -> bool {
        self.bytes.len() == self.expected_len()
    }

    /// The four raw bytes of pixel `i` (row-major).
    fn pixel(&self, i: usize) -> &[u8] {
        &self.bytes[i * BYTES_PER_PIXEL..i * BYTES_PER_PIXEL + BYTES_PER_PIXEL]
    }

    /// The `(r, g, b)` channels of pixel `i`, resolved through the format.
    fn rgb(&self, i: usize) -> (u8, u8, u8) {
        let p = self.pixel(i);
        let (r, g, b) = self.format.rgb_offsets();
        (p[r], p[g], p[b])
    }

    /// Integer Rec.601 luma of pixel `i`, as an `f64` in `0.0..=255.0` — the
    /// grayscale SSIM operates on. Format-resolved, so RGBA and XRGB views of
    /// the same image score identically.
    fn luma(&self, i: usize) -> f64 {
        let (r, g, b) = self.rgb(i);
        f64::from(u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000.0
    }
}

/// The rule a comparison is judged against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Policy {
    /// Byte-for-byte identical. Passes iff every channel of every pixel
    /// matches exactly.
    Exact,
    /// Allow small per-channel differences on a bounded share of pixels. A
    /// pixel is "bad" if any of its channels differs by more than
    /// `max_channel_diff`; the comparison passes iff the bad fraction is at
    /// most `max_bad_fraction`.
    Tolerance {
        /// Largest per-channel absolute difference a pixel may have and still
        /// count as good.
        max_channel_diff: u8,
        /// Largest fraction of bad pixels the whole frame may carry (`0.0` =
        /// none allowed, `1.0` = always passes).
        max_bad_fraction: f64,
    },
    /// Structural similarity. Passes iff the mean SSIM over the frame is at
    /// least `min` (`1.0` = identical). The fallback for GPU-vs-software AA.
    Ssim {
        /// Minimum acceptable mean SSIM, in `0.0..=1.0`.
        min: f64,
    },
}

/// The outcome of a [`compare`]. Carries the verdict and every statistic the
/// three policies key on, so a caller can log a precise diagnosis regardless
/// of which policy it chose.
#[derive(Debug, Clone)]
pub struct Report {
    /// Whether the comparison satisfied its policy.
    pub passed: bool,
    /// Whether the two frames were even comparable (same dimensions, same
    /// format, both well-formed). When `false`, every other statistic is a
    /// worst-case placeholder and `passed` is `false`.
    pub comparable: bool,
    /// Width of the compared frames (the actual frame's, on a mismatch).
    pub width: u32,
    /// Height of the compared frames (the actual frame's, on a mismatch).
    pub height: u32,
    /// Largest per-channel absolute difference found anywhere.
    pub max_channel_diff: u8,
    /// Fraction of pixels that differ beyond the policy's tolerance (for
    /// [`Policy::Exact`], any differing pixel counts).
    pub bad_fraction: f64,
    /// Mean SSIM score, computed only for [`Policy::Ssim`].
    pub ssim: Option<f64>,
}

impl Report {
    /// A one-line human summary for a failing-test message.
    pub fn summary(&self) -> String {
        if !self.comparable {
            return format!(
                "frames are not comparable (dimensions/format mismatch); actual is {}x{}",
                self.width, self.height
            );
        }
        let verdict = if self.passed { "pass" } else { "FAIL" };
        match self.ssim {
            Some(ssim) => format!(
                "{verdict}: ssim {ssim:.6}, max channel diff {}, bad fraction {:.6}",
                self.max_channel_diff, self.bad_fraction
            ),
            None => format!(
                "{verdict}: max channel diff {}, bad fraction {:.6}",
                self.max_channel_diff, self.bad_fraction
            ),
        }
    }
}

/// Compare `actual` against `expected` under `policy`.
///
/// A dimension, format, or length mismatch short-circuits to a non-passing,
/// non-comparable [`Report`] rather than panicking: a golden that changed
/// size is a failure to surface, not a crash.
pub fn compare(actual: Frame, expected: Frame, policy: Policy) -> Report {
    if actual.width != expected.width
        || actual.height != expected.height
        || actual.format != expected.format
        || !actual.is_well_formed()
        || !expected.is_well_formed()
    {
        return Report {
            passed: false,
            comparable: false,
            width: actual.width,
            height: actual.height,
            max_channel_diff: u8::MAX,
            bad_fraction: 1.0,
            ssim: matches!(policy, Policy::Ssim { .. }).then_some(0.0),
        };
    }

    let total = actual.width as usize * actual.height as usize;
    let tolerance = match policy {
        Policy::Tolerance {
            max_channel_diff, ..
        } => max_channel_diff,
        _ => 0,
    };

    let mut max_channel_diff = 0u8;
    let mut bad_pixels = 0usize;
    for i in 0..total {
        let mut pixel_diff = 0u8;
        for (a, e) in actual.pixel(i).iter().zip(expected.pixel(i)) {
            pixel_diff = pixel_diff.max(a.abs_diff(*e));
        }
        max_channel_diff = max_channel_diff.max(pixel_diff);
        if pixel_diff > tolerance {
            bad_pixels += 1;
        }
    }
    let bad_fraction = if total == 0 {
        0.0
    } else {
        bad_pixels as f64 / total as f64
    };

    let ssim = matches!(policy, Policy::Ssim { .. }).then(|| mean_ssim(&actual, &expected));
    let passed = match policy {
        Policy::Exact => max_channel_diff == 0,
        Policy::Tolerance {
            max_bad_fraction, ..
        } => bad_fraction <= max_bad_fraction,
        // `ssim` is `Some` here by construction of the `matches!` above.
        Policy::Ssim { min } => ssim.unwrap_or(0.0) >= min,
    };

    Report {
        passed,
        comparable: true,
        width: actual.width,
        height: actual.height,
        max_channel_diff,
        bad_fraction,
        ssim,
    }
}

/// Mean SSIM over non-overlapping 8x8 windows of the two frames' luma
/// planes. Standard SSIM constants (`C1 = (0.01*255)^2`, `C2 =
/// (0.03*255)^2`); edge windows shrink to what fits. Returns `1.0` for an
/// empty frame (nothing to disagree about).
fn mean_ssim(a: &Frame, b: &Frame) -> f64 {
    // (0.01 * 255)^2 and (0.03 * 255)^2, precomputed (`powi` is not const).
    const C1: f64 = 6.5025;
    const C2: f64 = 58.5225;
    const WIN: usize = 8;

    let w = a.width as usize;
    let h = a.height as usize;
    if w == 0 || h == 0 {
        return 1.0;
    }

    let la: Vec<f64> = (0..w * h).map(|i| a.luma(i)).collect();
    let lb: Vec<f64> = (0..w * h).map(|i| b.luma(i)).collect();

    let mut acc = 0.0;
    let mut windows = 0usize;
    let mut y0 = 0;
    while y0 < h {
        let mut x0 = 0;
        while x0 < w {
            let bw = WIN.min(w - x0);
            let bh = WIN.min(h - y0);
            let n = (bw * bh) as f64;

            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for y in y0..y0 + bh {
                for x in x0..x0 + bw {
                    let va = la[y * w + x];
                    let vb = lb[y * w + x];
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let mu_a = sa / n;
            let mu_b = sb / n;
            let var_a = (saa / n - mu_a * mu_a).max(0.0);
            let var_b = (sbb / n - mu_b * mu_b).max(0.0);
            let cov = sab / n - mu_a * mu_b;

            let numerator = (2.0 * mu_a * mu_b + C1) * (2.0 * cov + C2);
            let denominator = (mu_a * mu_a + mu_b * mu_b + C1) * (var_a + var_b + C2);
            acc += numerator / denominator;
            windows += 1;

            x0 += WIN;
        }
        y0 += WIN;
    }

    acc / windows as f64
}

/// The three artifact paths [`write_artifacts`] produced.
#[derive(Debug, Clone)]
pub struct Artifacts {
    /// The captured frame, as written.
    pub actual: PathBuf,
    /// The reference frame.
    pub expected: PathBuf,
    /// An amplified per-pixel absolute-difference heatmap.
    pub diff: PathBuf,
}

/// How much the raw per-channel difference is scaled in `diff.png` before
/// clamping, so a small-but-real mismatch is not invisibly dark.
const DIFF_AMPLIFY: u32 = 8;

/// Write `actual.png`, `expected.png` and `diff.png` into `dir` (created if
/// absent) and return their paths. The diff is a heatmap: black where the
/// two frames agree, brightening toward the channels that disagree.
///
/// Robust to a dimension mismatch — the diff is sized to `actual`, and any
/// pixel with no counterpart in `expected` is painted full red — so it can
/// be called on exactly the failing comparisons that most need debugging.
pub fn write_artifacts(dir: &Path, actual: Frame, expected: Frame) -> io::Result<Artifacts> {
    fs::create_dir_all(dir)?;
    let paths = Artifacts {
        actual: dir.join("actual.png"),
        expected: dir.join("expected.png"),
        diff: dir.join("diff.png"),
    };

    png::write_rgb(&paths.actual, actual.width, actual.height, &to_rgb(&actual))?;
    png::write_rgb(
        &paths.expected,
        expected.width,
        expected.height,
        &to_rgb(&expected),
    )?;
    let heatmap = diff_heatmap(&actual, &expected);
    png::write_rgb(&paths.diff, actual.width, actual.height, &heatmap)?;

    Ok(paths)
}

/// Flatten a frame to tightly packed RGB (dropping the fourth byte, ordering
/// by the frame's format) for the PNG encoder.
fn to_rgb(frame: &Frame) -> Vec<u8> {
    let total = frame.width as usize * frame.height as usize;
    let mut out = Vec::with_capacity(total * 3);
    for i in 0..total {
        let (r, g, b) = frame.rgb(i);
        out.extend_from_slice(&[r, g, b]);
    }
    out
}

/// Build the amplified difference heatmap over `actual`'s dimensions.
fn diff_heatmap(actual: &Frame, expected: &Frame) -> Vec<u8> {
    let total = actual.width as usize * actual.height as usize;
    let expected_pixels = expected.width as usize * expected.height as usize;
    let mut out = Vec::with_capacity(total * 3);
    for i in 0..total {
        if i < expected_pixels {
            let (ar, ag, ab) = actual.rgb(i);
            let (er, eg, eb) = expected.rgb(i);
            out.push(amplify(ar.abs_diff(er)));
            out.push(amplify(ag.abs_diff(eg)));
            out.push(amplify(ab.abs_diff(eb)));
        } else {
            // No counterpart pixel in `expected`: flag it unmistakably.
            out.extend_from_slice(&[0xff, 0x00, 0x00]);
        }
    }
    out
}

/// Scale a raw channel difference for visibility, saturating at 255.
fn amplify(diff: u8) -> u8 {
    (u32::from(diff) * DIFF_AMPLIFY).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny solid-color RGBA frame's backing bytes.
    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.repeat(width as usize * height as usize)
    }

    #[test]
    fn exact_passes_on_identical_frames() {
        let a = solid(8, 8, [0x10, 0x20, 0x30, 0xff]);
        let report = compare(Frame::rgba(8, 8, &a), Frame::rgba(8, 8, &a), Policy::Exact);
        assert!(report.passed);
        assert!(report.comparable);
        assert_eq!(report.max_channel_diff, 0);
        assert_eq!(report.bad_fraction, 0.0);
    }

    #[test]
    fn exact_fails_on_a_single_channel_difference() {
        let a = solid(8, 8, [0x10, 0x20, 0x30, 0xff]);
        let mut b = a.clone();
        b[0] = 0x11; // one channel of one pixel
        let report = compare(Frame::rgba(8, 8, &a), Frame::rgba(8, 8, &b), Policy::Exact);
        assert!(!report.passed);
        assert_eq!(report.max_channel_diff, 1);
        assert_eq!(report.bad_fraction, 1.0 / 64.0);
    }

    #[test]
    fn tolerance_forgives_small_but_not_large_differences() {
        let a = solid(8, 8, [0x80, 0x80, 0x80, 0xff]);
        let mut b = a.clone();
        // Nudge every pixel's red by 3.
        for px in b.chunks_exact_mut(4) {
            px[0] = 0x83;
        }
        let lax = compare(
            Frame::rgba(8, 8, &a),
            Frame::rgba(8, 8, &b),
            Policy::Tolerance {
                max_channel_diff: 4,
                max_bad_fraction: 0.0,
            },
        );
        assert!(lax.passed, "diff of 3 is within a tolerance of 4");
        assert_eq!(lax.max_channel_diff, 3);

        let strict = compare(
            Frame::rgba(8, 8, &a),
            Frame::rgba(8, 8, &b),
            Policy::Tolerance {
                max_channel_diff: 2,
                max_bad_fraction: 0.1,
            },
        );
        assert!(!strict.passed, "every pixel is bad, exceeding 10%");
        assert_eq!(strict.bad_fraction, 1.0);
    }

    #[test]
    fn ssim_is_one_for_identical_and_below_one_when_it_differs() {
        // A little structure so variance is nonzero.
        let mut a = vec![0u8; 16 * 16 * 4];
        for (i, px) in a.chunks_exact_mut(4).enumerate() {
            let v = (i * 7 % 251) as u8;
            px.copy_from_slice(&[v, v, v, 0xff]);
        }
        let same = compare(
            Frame::rgba(16, 16, &a),
            Frame::rgba(16, 16, &a),
            Policy::Ssim { min: 0.99 },
        );
        assert!(same.passed);
        assert!((same.ssim.unwrap() - 1.0).abs() < 1e-9);

        let mut b = a.clone();
        for px in b.chunks_exact_mut(4) {
            px[0] = px[0].wrapping_add(60);
            px[1] = px[1].wrapping_add(60);
            px[2] = px[2].wrapping_add(60);
        }
        let diff = compare(
            Frame::rgba(16, 16, &a),
            Frame::rgba(16, 16, &b),
            Policy::Ssim { min: 0.99 },
        );
        assert!(diff.ssim.unwrap() < 1.0);
        assert!(!diff.passed);
    }

    #[test]
    fn ssim_tolerates_a_uniform_low_bit_dither() {
        // The reason SSIM is the fallback: a scene that is structurally the
        // same but off by a hair everywhere scores near 1.0.
        let mut a = vec![0u8; 16 * 16 * 4];
        for (i, px) in a.chunks_exact_mut(4).enumerate() {
            let v = ((i * 13) % 200 + 20) as u8;
            px.copy_from_slice(&[v, v, v, 0xff]);
        }
        let mut b = a.clone();
        for (i, px) in b.chunks_exact_mut(4).enumerate() {
            let jitter = if i % 2 == 0 { 1 } else { u8::MAX }; // +1 / -1
            px[0] = px[0].wrapping_add(jitter);
            px[1] = px[1].wrapping_add(jitter);
            px[2] = px[2].wrapping_add(jitter);
        }
        let report = compare(
            Frame::rgba(16, 16, &a),
            Frame::rgba(16, 16, &b),
            Policy::Ssim { min: 0.95 },
        );
        assert!(report.passed, "ssim was {:?}", report.ssim);
        assert!(!report.passed || report.max_channel_diff >= 1);
    }

    #[test]
    fn a_dimension_mismatch_is_a_non_comparable_failure() {
        let a = solid(8, 8, [1, 2, 3, 0xff]);
        let b = solid(8, 9, [1, 2, 3, 0xff]);
        let report = compare(Frame::rgba(8, 8, &a), Frame::rgba(8, 9, &b), Policy::Exact);
        assert!(!report.passed);
        assert!(!report.comparable);
        assert!(report.summary().contains("not comparable"));
    }

    #[test]
    fn rgba_and_xrgb_of_the_same_image_score_identically_under_ssim() {
        // Same picture, two byte layouts: build XRGB from the RGBA.
        let mut rgba = vec![0u8; 8 * 8 * 4];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i * 3) as u8, (i * 5) as u8, (i * 7) as u8, 0xff]);
        }
        let xrgb: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], 0xff])
            .collect();
        let r_report = mean_ssim(&Frame::rgba(8, 8, &rgba), &Frame::rgba(8, 8, &rgba));
        let x_report = mean_ssim(&Frame::xrgb(8, 8, &xrgb), &Frame::xrgb(8, 8, &xrgb));
        assert!((r_report - x_report).abs() < 1e-12);
    }

    #[test]
    fn write_artifacts_emits_three_pngs() {
        let dir = std::env::temp_dir().join(format!("vitrin-golden-art-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let a = solid(4, 4, [0x00, 0x00, 0x00, 0xff]);
        let mut b = a.clone();
        b[0] = 0xff;
        let paths = write_artifacts(&dir, Frame::rgba(4, 4, &a), Frame::rgba(4, 4, &b))
            .expect("artifacts must write");
        for p in [&paths.actual, &paths.expected, &paths.diff] {
            let bytes = fs::read(p).expect("artifact readable");
            assert_eq!(
                &bytes[..8],
                &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
