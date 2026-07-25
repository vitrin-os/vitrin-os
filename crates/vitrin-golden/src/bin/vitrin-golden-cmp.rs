// SPDX-License-Identifier: Apache-2.0
//! `vitrin-golden-cmp` — compare two raw framebuffer files with the shared
//! [`vitrin_golden`] harness, from any language that can spawn a process.
//!
//! The P1.8.5 real-app fidelity gate (issue #107) lives in the Python
//! integration suite: an agent captures a real app's frame through the full
//! grant → enforcement → `render_frame` → memfd → wire → SDK-decode path, and
//! the core writes the same composited readback straight to a file via
//! `vitrind --capture-dump` — the **core-internal capture**, taken before any
//! of that transport runs. Proving "the capture path adds no distortion"
//! against a real app means comparing the two by SSIM. The SSIM lives here, in
//! Rust, and the codebase refuses to keep a second copy of a definition that
//! could drift (plan risk R7, and the golden README's one-definition rule), so
//! rather than re-port SSIM into the dependency-free Python suite this tiny
//! binary lets that suite call the *real* harness across the process boundary.
//!
//! ```text
//! vitrin-golden-cmp ACTUAL EXPECTED WxH --policy POLICY
//!                   [--actual-format xrgb|rgba] [--expected-format xrgb|rgba]
//!                   [--artifacts DIR]
//! ```
//!
//! `ACTUAL`/`EXPECTED` are files of tightly packed `width*height*4` bytes,
//! rows top-down. `POLICY` is one of `exact`, `ssim:MIN` (e.g. `ssim:0.98`),
//! or `tol:MAXDIFF,MAXFRAC` (e.g. `tol:2,0.01`). Formats default to the
//! P1.8.5 layout — the agent frame is `xrgb` (the wire format), the core dump
//! is `rgba` (the raw readback) — and either may be overridden.
//!
//! The two frames are normalised to a common RGBA representation before the
//! compare (the harness rejects a format mismatch, and normalising here is
//! what lets an XRGB wire frame be checked against an RGBA readback — a swap so
//! trivial a bug in the *core's* own `rgba_to_xrgb8888` would surface as a
//! disagreement rather than hide behind a shared conversion). The alpha/padding
//! plane is canonicalised to `0xFF` on both sides so per-pixel policies judge
//! colour alone; SSIM reads luma and ignores it either way.
//!
//! Exit code: `0` if the comparison passed its policy, `1` if it did not, `2`
//! on a usage or I/O error. The [`Report`] summary is printed on stdout so a
//! failing gate logs a precise reason. `--artifacts DIR` drops
//! `actual.png`/`expected.png`/`diff.png` for a visual break-down.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vitrin_golden::{compare, write_artifacts, Frame, PixelFormat, Policy};

/// Bytes per pixel of every format this tool speaks.
const BPP: usize = 4;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::from(0),
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            eprintln!("vitrin-golden-cmp: {err}");
            ExitCode::from(2)
        }
    }
}

/// Parse the command line, run the comparison, print the report, and return
/// whether it passed. Any usage or I/O problem is an `Err`, distinct from a
/// well-formed comparison that simply did not pass.
fn run() -> Result<bool, String> {
    let args = Args::parse(std::env::args().skip(1))?;

    let actual = read_frame(&args.actual, args.actual_format, args.width, args.height)?;
    let expected = read_frame(
        &args.expected,
        args.expected_format,
        args.width,
        args.height,
    )?;

    // Both sides are canonical RGBA now, so `compare` never short-circuits on a
    // format mismatch and the frames it sees differ only in colour.
    let a = Frame::rgba(args.width, args.height, &actual);
    let e = Frame::rgba(args.width, args.height, &expected);
    let report = compare(a, e, args.policy);

    println!("{}", report.summary());

    if !report.passed {
        if let Some(dir) = &args.artifacts {
            match write_artifacts(dir, a, e) {
                Ok(written) => eprintln!(
                    "vitrin-golden-cmp: wrote break-down artifacts: {}, {}, {}",
                    written.actual.display(),
                    written.expected.display(),
                    written.diff.display()
                ),
                Err(err) => eprintln!("vitrin-golden-cmp: could not write artifacts: {err}"),
            }
        }
    }

    Ok(report.passed)
}

/// Read a raw framebuffer file and normalise it to canonical RGBA
/// (`R, G, B, 0xFF`).
///
/// A length that is not exactly `width*height*4` is a hard error, not a
/// silent truncation: a dump of the wrong size means the run under test
/// produced the wrong frame, and that must fail loudly.
fn read_frame(
    path: &Path,
    format: PixelFormat,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let expected_len = width as usize * height as usize * BPP;
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    if bytes.len() != expected_len {
        return Err(format!(
            "{} is {} bytes, expected {} ({}x{} x {} bpp): wrong-size frame",
            path.display(),
            bytes.len(),
            expected_len,
            width,
            height,
            BPP,
        ));
    }
    Ok(to_canonical_rgba(&bytes, format))
}

/// Rewrite tightly packed pixels of `format` as canonical RGBA
/// (`R, G, B, 0xFF`). RGBA passes its colour bytes through and pins alpha;
/// XRGB (little-endian `B, G, R, X`) swaps blue and red into place. The alpha
/// plane is pinned on both so per-pixel policies compare colour, not the
/// padding byte an XRGB frame carries as `0xFF` and a readback carries as its
/// own alpha.
fn to_canonical_rgba(bytes: &[u8], format: PixelFormat) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    match format {
        PixelFormat::Rgba8888 => {
            for (dst, src) in out.chunks_exact_mut(BPP).zip(bytes.chunks_exact(BPP)) {
                dst[0] = src[0];
                dst[1] = src[1];
                dst[2] = src[2];
                dst[3] = 0xFF;
            }
        }
        PixelFormat::Xrgb8888 => {
            for (dst, src) in out.chunks_exact_mut(BPP).zip(bytes.chunks_exact(BPP)) {
                dst[0] = src[2]; // R  <- xrgb byte 2
                dst[1] = src[1]; // G  <- xrgb byte 1
                dst[2] = src[0]; // B  <- xrgb byte 0
                dst[3] = 0xFF;
            }
        }
    }
    out
}

/// The parsed command line.
struct Args {
    actual: PathBuf,
    expected: PathBuf,
    width: u32,
    height: u32,
    actual_format: PixelFormat,
    expected_format: PixelFormat,
    policy: Policy,
    artifacts: Option<PathBuf>,
}

impl Args {
    /// Hand-rolled parsing, matching the workspace's no-arg-crate posture
    /// (`vitrind`'s own parser, plan risk R7). Three positionals — `ACTUAL`,
    /// `EXPECTED`, `WxH` — then flags in any order.
    fn parse<I: Iterator<Item = String>>(iter: I) -> Result<Args, String> {
        let mut positionals: Vec<String> = Vec::new();
        // Defaults are the P1.8.5 fidelity layout: the agent's wire frame is
        // XRGB, the core's `--capture-dump` readback is RGBA.
        let mut actual_format = PixelFormat::Xrgb8888;
        let mut expected_format = PixelFormat::Rgba8888;
        let mut policy: Option<Policy> = None;
        let mut artifacts: Option<PathBuf> = None;

        let mut iter = iter;
        while let Some(arg) = iter.next() {
            let take = |iter: &mut I, flag: &str| -> Result<String, String> {
                iter.next()
                    .ok_or_else(|| format!("`{flag}` requires a value"))
            };
            match arg.as_str() {
                "--actual-format" => {
                    actual_format = parse_format(&take(&mut iter, "--actual-format")?)?
                }
                "--expected-format" => {
                    expected_format = parse_format(&take(&mut iter, "--expected-format")?)?
                }
                "--policy" => policy = Some(parse_policy(&take(&mut iter, "--policy")?)?),
                "--artifacts" => artifacts = Some(PathBuf::from(take(&mut iter, "--artifacts")?)),
                "-h" | "--help" => return Err(USAGE.to_string()),
                other if other.starts_with("--") => {
                    return Err(format!("unknown flag `{other}`\n{USAGE}"))
                }
                _ => positionals.push(arg),
            }
        }

        if positionals.len() != 3 {
            return Err(format!(
                "expected 3 positional arguments (ACTUAL EXPECTED WxH), got {}\n{USAGE}",
                positionals.len()
            ));
        }
        let (width, height) = parse_size(&positionals[2])?;
        let policy = policy.ok_or_else(|| format!("`--policy` is required\n{USAGE}"))?;

        Ok(Args {
            actual: PathBuf::from(&positionals[0]),
            expected: PathBuf::from(&positionals[1]),
            width,
            height,
            actual_format,
            expected_format,
            policy,
            artifacts,
        })
    }
}

fn parse_format(value: &str) -> Result<PixelFormat, String> {
    match value {
        "rgba" | "rgba8888" => Ok(PixelFormat::Rgba8888),
        "xrgb" | "xrgb8888" => Ok(PixelFormat::Xrgb8888),
        other => Err(format!(
            "unknown pixel format `{other}` (expected `rgba` or `xrgb`)"
        )),
    }
}

/// `WxH` into `(width, height)`, both non-zero.
fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let (w, h) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("size `{value}` is not `WxH` (e.g. `640x480`)"))?;
    let width: u32 = w.parse().map_err(|_| format!("bad width in `{value}`"))?;
    let height: u32 = h.parse().map_err(|_| format!("bad height in `{value}`"))?;
    if width == 0 || height == 0 {
        return Err(format!("size `{value}` has a zero dimension"));
    }
    Ok((width, height))
}

/// `exact` | `ssim:MIN` | `tol:MAXDIFF,MAXFRAC` into a [`Policy`].
fn parse_policy(value: &str) -> Result<Policy, String> {
    if value == "exact" {
        return Ok(Policy::Exact);
    }
    if let Some(min) = value.strip_prefix("ssim:") {
        let min: f64 = min
            .parse()
            .map_err(|_| format!("bad SSIM threshold in `{value}` (e.g. `ssim:0.98`)"))?;
        if !(0.0..=1.0).contains(&min) {
            return Err(format!("SSIM threshold `{min}` is outside 0.0..=1.0"));
        }
        return Ok(Policy::Ssim { min });
    }
    if let Some(rest) = value.strip_prefix("tol:") {
        let (diff, frac) = rest.split_once(',').ok_or_else(|| {
            format!("`tol:` needs `MAXDIFF,MAXFRAC` (e.g. `tol:2,0.01`), got `{value}`")
        })?;
        let max_channel_diff: u8 = diff
            .parse()
            .map_err(|_| format!("bad max-channel-diff in `{value}`"))?;
        let max_bad_fraction: f64 = frac
            .parse()
            .map_err(|_| format!("bad max-bad-fraction in `{value}`"))?;
        if !(0.0..=1.0).contains(&max_bad_fraction) {
            return Err(format!(
                "max-bad-fraction `{max_bad_fraction}` is outside 0.0..=1.0"
            ));
        }
        return Ok(Policy::Tolerance {
            max_channel_diff,
            max_bad_fraction,
        });
    }
    Err(format!(
        "unknown policy `{value}` (expected `exact`, `ssim:MIN`, or `tol:MAXDIFF,MAXFRAC`)"
    ))
}

const USAGE: &str = "\
usage: vitrin-golden-cmp ACTUAL EXPECTED WxH --policy POLICY
                         [--actual-format xrgb|rgba] [--expected-format xrgb|rgba]
                         [--artifacts DIR]

  ACTUAL, EXPECTED   files of tightly packed width*height*4 bytes, rows top-down
  WxH                frame dimensions, e.g. 640x480
  POLICY             exact | ssim:MIN | tol:MAXDIFF,MAXFRAC
  --actual-format    format of ACTUAL   (default: xrgb, the wire frame)
  --expected-format  format of EXPECTED (default: rgba, the --capture-dump readback)
  --artifacts DIR    on failure, write actual/expected/diff PNGs under DIR

exit: 0 passed, 1 did not pass, 2 usage/IO error";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrgb_and_rgba_of_one_colour_normalise_to_the_same_bytes() {
        // The load-bearing conversion: an XRGB wire frame and an RGBA readback
        // of the same blue must land on identical canonical RGBA, so the
        // fidelity SSIM compares colour and not byte order. Blue is
        // `B,G,R,X = ff,00,00,ff` on the wire and `R,G,B,A = 00,00,ff,ff` as a
        // readback.
        let xrgb = [0xffu8, 0x00, 0x00, 0xff];
        let rgba = [0x00u8, 0x00, 0xff, 0xff];
        assert_eq!(
            to_canonical_rgba(&xrgb, PixelFormat::Xrgb8888),
            to_canonical_rgba(&rgba, PixelFormat::Rgba8888),
        );
        // And the canonical form is the RGBA one, with alpha pinned opaque.
        assert_eq!(
            to_canonical_rgba(&xrgb, PixelFormat::Xrgb8888),
            vec![0, 0, 0xff, 0xff]
        );
    }

    #[test]
    fn alpha_plane_is_pinned_so_per_pixel_policies_ignore_padding() {
        // An XRGB `X` byte and a readback `A` byte are unrelated padding; both
        // canonicalise to 0xFF so `exact`/`tol` judge colour alone.
        let opaque = to_canonical_rgba(&[1, 2, 3, 0x00], PixelFormat::Rgba8888);
        assert_eq!(opaque[3], 0xFF);
    }

    #[test]
    fn policy_spellings_parse() {
        assert!(matches!(parse_policy("exact"), Ok(Policy::Exact)));
        assert!(
            matches!(parse_policy("ssim:0.98"), Ok(Policy::Ssim { min }) if (min - 0.98).abs() < 1e-9)
        );
        assert!(matches!(
            parse_policy("tol:2,0.01"),
            Ok(Policy::Tolerance {
                max_channel_diff: 2,
                ..
            })
        ));
        assert!(parse_policy("ssim:1.5").is_err());
        assert!(parse_policy("nonsense").is_err());
    }

    #[test]
    fn size_parses_and_rejects_zero() {
        assert_eq!(parse_size("640x480").unwrap(), (640, 480));
        assert_eq!(parse_size("4X4").unwrap(), (4, 4));
        assert!(parse_size("640").is_err());
        assert!(parse_size("0x10").is_err());
    }
}
