// SPDX-License-Identifier: MPL-2.0
//! Capture-frame mechanics: the pixel half of `vitrin_view.capture_frame`
//! (P1.3.6, the observe half of artifact A1).
//!
//! One capture = one software copy of the realm view into a **fresh, sealed
//! memfd**, handed to the requesting agent connection via `SCM_RIGHTS`.
//! Capture is ALWAYS a copy: agents never see a live buffer, and no agent
//! ever holds a reference into compositor memory (PRD Doc 2 §2/§9). The
//! wire contract implemented here is `vitrin_view.frame_ready` exactly as
//! `protocol/vitrin-v0.xml` pins it: xrgb8888, row-major, origin top-left,
//! little-endian, `stride == width * 4`, padding byte `0xFF`, memfd size
//! exactly `stride * height`, sealed `SHRINK|GROW|WRITE|SEAL` before
//! sending, `flags == 0`.
//!
//! # Capture timing: latest completed frame, never render-on-demand
//!
//! The decision P1.3.6 settled. A capture reads the compositor's
//! **retained, last-completed framebuffer** (the headless backend's virtual
//! output image, [`crate::backend::headless`]); it never triggers a
//! composite of its own. Rationale:
//!
//! - **It is the IDL's semantic.** `capture_frame` promises "the realm
//!   view's most recently composited content as of when the server
//!   processes this request" and nothing fresher. Render-on-demand would
//!   manufacture a frame *newer* than anything the compositor completed —
//!   content no human-visible output ever showed — which is a different
//!   observable than the spec defines.
//! - **Determinism for golden tests.** A capture is a pure read of
//!   completed compositor output: N captures of an idle scene are
//!   byte-identical, and the M1.1 golden is exact (the headless backend
//!   composites the test pattern once, so every capture equals the
//!   pattern's xrgb8888 conversion, byte for byte). Render-on-demand would
//!   couple capture bytes to renderer state at request-arrival time — racy
//!   against damage and frame-callback pacing once client surfaces animate
//!   (P1.3.4) — turning goldens flaky by construction.
//! - **No torn frames.** A completed frame is internally consistent;
//!   rendering at request time could observe a half-updated scene.
//! - **Cost isolation.** Compositing cost stays on the compositor's own
//!   damage/frame cadence and can never be driven by agent request rate;
//!   the per-grant rate limit (the chokepoint's token bucket, P1.4.4) then
//!   bounds only the copy cost of capture itself.
//!
//! # Mechanics only — enforcement lives at the chokepoint (P1.4.4)
//!
//! This module deliberately contains **no authority code and no wire
//! emission**: P1.3.6's `CaptureGate` seam and its `CaptureService` were
//! replaced — not wrapped — by the enforcement chokepoint
//! ([`crate::enforcement::Chokepoint::enforce_use`]), exactly as the seam's
//! own docs promised. Every `capture_frame` is admitted or refused *there*
//! (connection → principal → grant → verbs → constraints), and only an
//! admitted capture reaches [`render_frame`], which converts pixels and
//! seals the memfd — it cannot refuse, cannot allow, and cannot speak on
//! the wire. The chokepoint's single-path test pins that `render_frame` has
//! no caller outside the chokepoint (and this module's own mechanics
//! tests): there is no legacy capture entry left to bypass enforcement
//! through.
//!
//! # The observation digest rides the copy path (P1.4.5, B1)
//!
//! [`render_frame`] returns the frame's [`ObservationDigest`] alongside the
//! event. This is the only point in the core that holds the exact bytes
//! delivered to the agent without re-reading the sealed memfd, so it is
//! where the flight recorder's B1 digest is computed — unconditionally, for
//! every capture, never sampled (see [`crate::recorder`] for the decision
//! and its measured cost). Digesting here also keeps the recorder out of
//! both this module and the chokepoint: capture produces the fact, the
//! chokepoint hands it back in its [`UseOutcome`], and the connection
//! server records it.
//!
//! [`UseOutcome`]: crate::enforcement::UseOutcome

use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::OwnedFd;

use rustix::fs::{MemfdFlags, SealFlags};
use vitrin_protocol::generated::vitrin_view::{events::FrameReady, Format, FrameFlags};

use crate::recorder::ObservationDigest;

/// Bytes per pixel of the version-1 capture format (`xrgb8888`); the IDL
/// pins `stride == width * 4` exactly.
const BYTES_PER_PIXEL: usize = 4;

/// The name after `memfd:` in `/proc/self/fd` — diagnostic only, never
/// namespace-relevant (memfds have no filesystem presence).
const MEMFD_NAME: &str = "vitrin-capture-frame";

/// A borrowed realm view to capture: the compositor's latest **completed**
/// frame, as tightly packed RGBA8888 (the headless backend's readback
/// layout — bytes R,G,B,A per pixel, rows top-down, no padding). See the
/// module docs for why this is always retained output, never a fresh
/// render.
pub(crate) struct RealmViewFrame<'a> {
    /// `width * height * 4` bytes of RGBA8888, rows top-down.
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
}

/// Why [`render_frame`] could not produce a frame. Every case is a
/// server-side condition (never the agent's fault) that the chokepoint
/// voices as the recoverable refusal `internal` — fail closed, never a
/// torn connection and never a contract-violating frame.
#[derive(Debug)]
pub(crate) enum CaptureError {
    /// The readback buffer disagrees with the claimed dimensions — a
    /// corrupted readback, not a dead realm (an agent must not conclude
    /// the shim crashed from a readback bug, so this is `internal`, never
    /// `no_surface`).
    MisSizedReadback { got: usize, expected: usize },
    /// The view has a zero dimension. Defense in depth: the chokepoint
    /// refuses `no_surface` before calling [`render_frame`], so reaching
    /// this arm is a core bug surfacing typed (`internal`) — `frame_ready`
    /// dimensions are always nonzero and this module will not fabricate an
    /// empty frame.
    DegenerateView { width: u32, height: u32 },
    /// memfd creation, write, or sealing failed.
    Memfd(io::Error),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::MisSizedReadback { got, expected } => {
                write!(f, "capture readback length {got}, expected {expected}")
            }
            CaptureError::DegenerateView { width, height } => {
                write!(f, "degenerate {width}x{height} view reached render_frame")
            }
            CaptureError::Memfd(err) => write!(f, "capture memfd failed: {err}"),
        }
    }
}

/// Produce one wire-ready `frame_ready` payload from the latest completed
/// realm view: convert the readback to xrgb8888, digest the result, and
/// seal the copy into a fresh memfd. Pure mechanics — no authority, no wire
/// I/O; the returned event (and the fd ownership it carries) is the
/// enforcement chokepoint's to send, and the server-side copy of the fd
/// drops when the event does.
///
/// The second return value is the B1 observation digest over **exactly**
/// the bytes the memfd carries — computed before sealing, from the same
/// buffer that is written, so the digest and the delivered frame cannot
/// disagree (module docs).
pub(crate) fn render_frame(
    view: &RealmViewFrame<'_>,
) -> Result<(FrameReady, ObservationDigest), CaptureError> {
    if view.width == 0 || view.height == 0 {
        return Err(CaptureError::DegenerateView {
            width: view.width,
            height: view.height,
        });
    }
    let expected = view.width as usize * view.height as usize * BYTES_PER_PIXEL;
    if view.rgba.len() != expected {
        return Err(CaptureError::MisSizedReadback {
            got: view.rgba.len(),
            expected,
        });
    }
    let pixels = rgba_to_xrgb8888(view.rgba);
    let digest = ObservationDigest::of(&pixels);
    let fd = sealed_frame_memfd(&pixels).map_err(CaptureError::Memfd)?;
    Ok((
        FrameReady {
            fd,
            format: Format::Xrgb8888,
            width: view.width,
            height: view.height,
            // Pinned by the IDL: stride == width * 4 exactly in version 1.
            stride: view.width * BYTES_PER_PIXEL as u32,
            // Always 0 in version 1; any set bit would be a server protocol
            // violation.
            flags: FrameFlags::default(),
        },
        digest,
    ))
}

/// Convert tightly packed RGBA8888 (readback layout: bytes R,G,B,A) to the
/// wire's little-endian xrgb8888 (bytes B,G,R,X). The padding byte is
/// forced to `0xFF` exactly — the IDL mandates it regardless of source
/// alpha, so identical content yields identical buffer bytes and
/// observation digests stay deterministic.
fn rgba_to_xrgb8888(rgba: &[u8]) -> Vec<u8> {
    debug_assert!(rgba.len().is_multiple_of(BYTES_PER_PIXEL));
    let mut out = vec![0u8; rgba.len()];
    for (src, dst) in rgba
        .chunks_exact(BYTES_PER_PIXEL)
        .zip(out.chunks_exact_mut(BYTES_PER_PIXEL))
    {
        dst[0] = src[2]; // B
        dst[1] = src[1]; // G
        dst[2] = src[0]; // R
        dst[3] = 0xff; // X: pinned 0xFF by the IDL, never source alpha
    }
    out
}

/// Create a **fresh** memfd holding exactly `frame` and seal it
/// `SHRINK|GROW|WRITE|SEAL` — the wire contract's mechanical
/// always-a-copy proof: a write-sealed fd is provably not a live buffer,
/// and its bytes are immutable for the fd's lifetime. `MFD_CLOEXEC` keeps
/// the fd out of any concurrently spawned realm (P1.5.2 fork/exec); the
/// write happens through `write(2)`, never a writable mapping, so the
/// write seal cannot fail with `EBUSY`.
///
/// `pub(crate)` for exactly one further caller: the `consent-injector`
/// build's [`crate::backend::headless::HeadlessView::consent_occlusion_window`]
/// (issue #138), which exports the consent card's own footprint of the
/// human-visible framebuffer. Sharing this rather than growing a second
/// exporter keeps the invariant worth having — **there is exactly one way
/// pixels leave this process, and it is a sealed copy** — true of the
/// instrumented build as well as the shipping one.
pub(crate) fn sealed_frame_memfd(frame: &[u8]) -> io::Result<OwnedFd> {
    let fd = rustix::fs::memfd_create(MEMFD_NAME, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;
    let mut file = File::from(fd);
    file.write_all(frame)?;
    rustix::fs::fcntl_add_seals(
        &file,
        SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE | SealFlags::SEAL,
    )?;
    Ok(OwnedFd::from(file))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsFd, OwnedFd};
    use std::os::unix::fs::FileExt;
    use std::sync::{Mutex, MutexGuard};

    use vitrin_golden::{compare, write_artifacts, Frame, Policy};
    use vitrin_ipc::{Connection, Message};
    use vitrin_protocol::generated::vitrin_view::{events::FrameReady, Format};

    use super::*;
    use crate::test_pattern;

    /// Representative dimensions of the checked-in image golden: small enough
    /// to commit (96*60*4 = ~23 KB) yet large enough to show all four corner
    /// markers, the color bars and the checkerboard, so a regression is
    /// legible in `diff.png`.
    const GOLDEN_W: u32 = 96;
    const GOLDEN_H: u32 = 60;

    /// Repo-root path of the shared image golden the harness asserts on.
    /// `CARGO_MANIFEST_DIR` is `crates/vitrin-core`; the golden lives under
    /// the workspace-wide `tests/golden/` dir (plan §2 layout).
    const IMAGE_GOLDEN_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/golden/headless_test_pattern_96x60.xrgb"
    );

    /// Serializes the remaining *shared-process* fd tests, so their
    /// socketpairs/memfds cannot race one another on the harness's thread
    /// pool. Shared with the headless backend's event-loop test for the
    /// same reason.
    ///
    /// The two `/proc/self/fd` **baseline** tests
    /// ([`fd_count_returns_to_baseline`],
    /// [`render_frame_fails_typed_never_fabricating_a_frame`]) deliberately
    /// do *not* rely on this lock: their measured quantity is the
    /// process-global open-fd count, which a *non-measuring* test on the
    /// pool can perturb whether or not it holds `FD_QUIESCE` — and
    /// `fd_lock` coverage across the ~14 fd-minting modules drifts silently
    /// (`consent/grab.rs` already dropped it). Those two are instead
    /// process-isolated via [`run_fd_isolated`], which re-execs the test
    /// binary so the body is the sole fd mutator in its process. See
    /// [`open_fd_count`] for the guard that keeps that invariant from
    /// silently rotting.
    pub(crate) static FD_QUIESCE: Mutex<()> = Mutex::new(());

    /// Lock [`FD_QUIESCE`], surviving a poisoned mutex (a failed test must
    /// not cascade into every later fd test failing on `unwrap`).
    pub(crate) fn fd_lock() -> MutexGuard<'static, ()> {
        FD_QUIESCE.lock().unwrap_or_else(|e| e.into_inner())
    }

    const VIEW_ID: u32 = 7;

    /// Env sentinel distinguishing the outer (harness-pool) invocation of a
    /// baseline test from the re-exec'd child that actually measures fds.
    /// Set only by [`run_fd_isolated`], so it is a reliable "am I the lone
    /// measuring process?" signal.
    const FD_ISOLATED_SENTINEL: &str = "VITRIN_FD_BASELINE_ISOLATED";

    /// Marker the isolated child prints once the real body has passed. Its
    /// presence in the child's stdout is what closes libtest's "a filter
    /// matching zero tests still exits 0" hole: an exit code of 0 alone
    /// would pass even if a typo in `test_path` selected no test at all.
    const FD_ISOLATED_MARKER: &str = "VITRIN_FD_BASELINE_RAN";

    /// Number of open fds in this process, via `/proc/self/fd`. The
    /// `read_dir` handle itself appears in the listing, but identically in
    /// every measurement, so equality comparisons are exact.
    ///
    /// The `debug_assert` is the regression guard: this reads a
    /// **process-global** quantity, so it is only meaningful inside
    /// [`run_fd_isolated`]'s re-exec'd child, where this test is the sole fd
    /// mutator. Any future fd-leak test must call `open_fd_count` to take a
    /// baseline; the moment it does so on the shared harness pool (sentinel
    /// unset) it trips here in debug/CI, forcing the author to route through
    /// `run_fd_isolated` rather than silently reintroducing the #74/#80
    /// race. Because `run_fd_isolated` is the only setter of the sentinel,
    /// the isolation cannot be bypassed by accident.
    pub(crate) fn open_fd_count() -> usize {
        debug_assert!(
            std::env::var_os(FD_ISOLATED_SENTINEL).is_some(),
            "open_fd_count() measures the process-global /proc/self/fd and \
             must run only inside run_fd_isolated's re-exec'd child; a \
             baseline read on the shared harness pool is the #74/#80 race"
        );
        std::fs::read_dir("/proc/self/fd")
            .expect("/proc/self/fd is readable on Linux")
            .count()
    }

    /// Run `body` in a private process where it is the *only* test
    /// executing, so its `/proc/self/fd` baseline can be perturbed by
    /// nothing but its own render/socketpair/drop.
    ///
    /// The quantity these tests measure — the process-global open-fd count
    /// — is owned by whatever else runs in the process, and the fd-minting
    /// tests are spread across ~14 modules whose `fd_lock` coverage drifts
    /// silently. Serializing on `fd_lock` cannot fix it: the perturbation
    /// comes from *non-measuring* tests, and no guard at the measurement
    /// site can catch a future minter that forgets the lock — nor fds
    /// minted directly by std/rustix/calloop. Isolation removes the race by
    /// construction (#74 task 2 / #80 option 1): in the child there is no
    /// concurrent test, no library-spawned backend thread (winit/Smithay
    /// runs only from `main`, never a `#[test]`) and no async runtime, so
    /// between the baseline snapshot and every assertion the only code that
    /// can open or close an fd is the body itself.
    ///
    /// On the outer invocation (sentinel unset) this re-execs the test
    /// binary with `--exact <test_path> --test-threads 1 --nocapture` and
    /// the sentinel set, then requires the child both to exit 0 *and* to
    /// have printed [`FD_ISOLATED_MARKER`] (proof the named test actually
    /// ran). On the isolated invocation (sentinel set) it runs `body` for
    /// real — this is `exec`, not `fork`, so there is no async-signal-safety
    /// hazard, and no other test is selected, so no recursion occurs.
    pub(crate) fn run_fd_isolated(test_path: &str, body: impl FnOnce()) {
        if std::env::var_os(FD_ISOLATED_SENTINEL).is_some() {
            body();
            // Printed only on the true measurement path, after the body's
            // own assertions have passed, so its presence in the child's
            // stdout means "this exact test ran to completion", not
            // "libtest matched nothing and exited 0".
            println!("{FD_ISOLATED_MARKER}");
            return;
        }

        let exe = std::env::current_exe().expect("the test binary has a path");
        let output = std::process::Command::new(exe)
            .args([
                "--exact",
                "--nocapture",
                "--quiet",
                "--test-threads",
                "1",
                test_path,
            ])
            .env(FD_ISOLATED_SENTINEL, "1")
            .output()
            .expect("re-exec of the test binary must spawn");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "isolated fd test `{test_path}` failed in its own process\n\
             --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
        assert!(
            stdout.contains(FD_ISOLATED_MARKER),
            "isolated fd test `{test_path}` exited 0 but never ran (marker \
             absent) — check the test path spelling\n--- stdout ---\n{stdout}"
        );
    }

    /// Render one frame of `rgba` and ship it over a socketpair exactly as
    /// the chokepoint does (bytes + `SCM_RIGHTS` fd), returning the
    /// client-side received message — the mechanics half of the old
    /// end-to-end serve, kept so the memfd/seal assertions still cover the
    /// transported artifact.
    fn ship_one(
        server: &mut Connection,
        client: &mut Connection,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Message {
        let (frame, digest) = render_frame(&RealmViewFrame {
            rgba,
            width,
            height,
        })
        .expect("render_frame must succeed on a well-formed view");
        // B1's core promise, asserted at the mechanics layer: the digest is
        // over exactly the bytes that go into the memfd.
        assert_eq!(digest, ObservationDigest::of(&rgba_to_xrgb8888(rgba)));
        server
            .send_message(&frame.encode(VIEW_ID), Some(frame.fd.as_fd()))
            .expect("send must succeed on a fresh pair");
        // `frame` (and the server's only copy of the memfd) drops here.
        client
            .recv_message()
            .expect("client receive must succeed")
            .expect("a frame must be waiting")
    }

    /// Decode a received `frame_ready`, asserting it arrived on the view
    /// object.
    fn decode_frame(msg: Message) -> FrameReady {
        let (object_id, frame) =
            FrameReady::decode(&msg.bytes, msg.fd).expect("frame_ready must decode");
        assert_eq!(object_id, VIEW_ID, "frame_ready is an event on the view");
        frame
    }

    /// Read the whole memfd back through `pread` (offset-independent: the
    /// server's writes advanced the shared file offset).
    fn read_memfd(fd: OwnedFd, len: usize) -> Vec<u8> {
        let file = File::from(fd);
        let mut buf = vec![0u8; len];
        file.read_exact_at(&mut buf, 0)
            .expect("memfd contents must be readable at offset 0");
        buf
    }

    /// M1.1 golden: a rendered capture is byte-for-byte the xrgb8888
    /// conversion of the synthetic test pattern (per-pixel tolerance
    /// zero), generated in-process — never a committed image file (plan
    /// risk R7: no image codec in the core, not even as a dev-dependency).
    /// Chained with `headless::tests::capture_is_identity_of_test_pattern`
    /// (the retained framebuffer IS the pattern, byte-exact), this is the
    /// "capture matches the realm view's content" acceptance for both the
    /// headless golden and the nested window, which composites the same
    /// pattern.
    #[test]
    fn served_frame_is_the_synthetic_pattern_golden() {
        let _fd = fd_lock();
        const W: u32 = 320;
        const H: u32 = 200;
        let (mut server, mut client) = Connection::pair().unwrap();

        // What the compositor retains: the composited pattern (the
        // headless golden proves compositing is an identity on it).
        let rgba = test_pattern::render(W, H);
        let frame = decode_frame(ship_one(&mut server, &mut client, &rgba, W, H));

        // The frame_ready contract, argument by argument.
        assert_eq!(frame.format, Format::Xrgb8888);
        assert_eq!(frame.width, W);
        assert_eq!(frame.height, H);
        assert_eq!(frame.stride, W * 4, "stride == width * 4 exactly");
        assert_eq!(frame.flags.bits(), 0, "flags are always 0 in version 1");

        // Memfd contract: size exactly stride * height (64-bit math), all
        // four seals present before it reached us.
        let stat = rustix::fs::fstat(&frame.fd).unwrap();
        assert_eq!(
            stat.st_size as u64,
            u64::from(frame.stride) * u64::from(frame.height)
        );
        let seals = rustix::fs::fcntl_get_seals(&frame.fd).unwrap();
        for seal in [
            SealFlags::SHRINK,
            SealFlags::GROW,
            SealFlags::WRITE,
            SealFlags::SEAL,
        ] {
            assert!(seals.contains(seal), "missing seal {seal:?}");
        }

        // The golden itself: exact bytes, X channel 0xFF throughout.
        let len = (frame.stride * frame.height) as usize;
        let pixels = read_memfd(frame.fd, len);
        assert_eq!(pixels, rgba_to_xrgb8888(&test_pattern::render(W, H)));
        assert!(pixels.iter().skip(3).step_by(4).all(|&x| x == 0xff));
    }

    /// Cross-language golden pin (P1.8.2, issue #41): the committed
    /// raw-bytes file `sdk/python/tests/golden/test_pattern_64x40.xrgb`
    /// IS `rgba_to_xrgb8888(test_pattern::render(64, 40))` — the exact
    /// wire bytes a capture of the 64x40 pattern delivers. Rust CI pins
    /// the file with this test; the Python SDK suite consumes it (its
    /// mock core serves these bytes as the frame memfd, and `.to_png()`
    /// must decode back to them), so the two implementations can only
    /// drift loudly — and no image codec ever enters the core (plan risk
    /// R7): the shared golden stays raw bytes, PNG encoding lives in the
    /// SDK alone.
    ///
    /// Regeneration — only when the pattern or the swizzle deliberately
    /// changes — goes through the single documented flow
    /// (`tests/golden/README.md`):
    ///
    /// ```sh
    /// cargo xtask bless --filter sdk_capture_golden
    /// ```
    ///
    /// which drives this test with `VITRIN_REGEN_GOLDEN=1`. Then commit the
    /// rewritten file together with the change that motivated it (the Python
    /// capture tests keep consuming it).
    #[test]
    fn sdk_capture_golden_file_pins_the_wire_bytes() {
        let _fd = fd_lock();
        const W: u32 = 64;
        const H: u32 = 40;
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/python/tests/golden/test_pattern_64x40.xrgb"
        );
        let expected = rgba_to_xrgb8888(&test_pattern::render(W, H));
        if std::env::var_os("VITRIN_REGEN_GOLDEN").is_some() {
            std::fs::write(path, &expected).expect("golden regeneration must be writable");
        }
        let committed = std::fs::read(path)
            .expect("committed SDK golden exists (regenerate: VITRIN_REGEN_GOLDEN=1)");
        assert_eq!(
            committed, expected,
            "sdk/python/tests/golden/test_pattern_64x40.xrgb no longer matches \
             rgba_to_xrgb8888(test_pattern::render(64, 40)); if the pattern or \
             swizzle changed deliberately, regenerate with VITRIN_REGEN_GOLDEN=1 \
             and update the Python capture tests"
        );
    }

    /// The flagship P1.9.2 consumer: a checked-in headless test-pattern
    /// **image** golden, compared through the shared `vitrin-golden` harness
    /// under an [`Exact`](Policy::Exact) policy — the exactness acceptance
    /// criterion, proven end to end through the same `compare` the harness
    /// offers every future pixel test. The committed file is the xrgb8888
    /// wire conversion of the synthetic pattern (the exact bytes a capture
    /// delivers), stored raw so no image codec is needed to read it back
    /// (plan risk R7); the harness's own PNG encoder is exercised only when a
    /// comparison fails, by [`write_artifacts`].
    ///
    /// Regeneration goes through the single documented flow — see
    /// `tests/golden/README.md`:
    ///
    /// ```sh
    /// cargo xtask bless --filter headless_test_pattern_image_golden
    /// ```
    ///
    /// which drives this test with `VITRIN_REGEN_GOLDEN=1`.
    #[test]
    fn headless_test_pattern_image_golden() {
        let actual = rgba_to_xrgb8888(&test_pattern::render(GOLDEN_W, GOLDEN_H));
        if std::env::var_os("VITRIN_REGEN_GOLDEN").is_some() {
            std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden"))
                .expect("golden directory");
            std::fs::write(IMAGE_GOLDEN_PATH, &actual)
                .expect("golden regeneration must be writable");
        }
        let expected = std::fs::read(IMAGE_GOLDEN_PATH)
            .expect("committed image golden exists (regenerate: cargo xtask bless)");

        let report = compare(
            Frame::xrgb(GOLDEN_W, GOLDEN_H, &actual),
            Frame::xrgb(GOLDEN_W, GOLDEN_H, &expected),
            Policy::Exact,
        );
        assert!(
            report.passed,
            "tests/golden/headless_test_pattern_96x60.xrgb no longer matches the synthetic \
             test pattern ({}); if the pattern changed deliberately, regenerate with \
             `cargo xtask bless --filter headless_test_pattern_image_golden` and review the diff",
            report.summary()
        );
        assert_eq!(
            report.max_channel_diff, 0,
            "an exact golden must be byte-identical"
        );
    }

    /// The acceptance criterion made executable: a **one-pixel** change to the
    /// scene fails the affected golden, and the failure leaves the
    /// actual/expected/diff artifacts behind for debugging. Guards the harness
    /// against ever silently passing on a real regression — the exact hazard
    /// P1.9.2 exists to close.
    #[test]
    fn a_one_pixel_change_fails_the_image_golden_and_writes_artifacts() {
        // Derive the reference straight from the generator, not the committed
        // file: identical bytes to the golden, but with no dependency on (and
        // no read race against) the regenerating test above.
        let golden = rgba_to_xrgb8888(&test_pattern::render(GOLDEN_W, GOLDEN_H));

        // Mutate exactly one channel of exactly one pixel of the captured
        // frame — the smallest possible scene change.
        let mut mutated = golden.clone();
        let target = (GOLDEN_H / 2 * GOLDEN_W + GOLDEN_W / 2) as usize * 4;
        mutated[target] ^= 0xff;

        let dir =
            std::env::temp_dir().join(format!("vitrin-golden-onepixel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let actual = Frame::xrgb(GOLDEN_W, GOLDEN_H, &mutated);
        let expected = Frame::xrgb(GOLDEN_W, GOLDEN_H, &golden);
        let report = compare(actual, expected, Policy::Exact);
        assert!(
            !report.passed,
            "a one-pixel change must fail the exact golden, but the harness passed it"
        );
        assert!(
            report.comparable,
            "the frames are the same size — the failure is content, not shape"
        );
        assert!(report.max_channel_diff > 0);
        assert_eq!(
            report.bad_fraction,
            1.0 / (GOLDEN_W as f64 * GOLDEN_H as f64),
            "exactly one pixel differs"
        );

        let artifacts =
            write_artifacts(&dir, actual, expected).expect("failure artifacts must be writable");
        for path in [&artifacts.actual, &artifacts.expected, &artifacts.diff] {
            let bytes = std::fs::read(path).expect("each artifact was written");
            assert!(!bytes.is_empty(), "{} is non-empty", path.display());
            assert_eq!(
                &bytes[..8],
                &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'],
                "{} is a PNG",
                path.display()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every capture yields a *fresh* memfd: two frames held concurrently
    /// are distinct open files (different inodes), and the sealed bytes of
    /// the first cannot be disturbed by anything that happens after it was
    /// delivered.
    #[test]
    fn every_capture_yields_a_fresh_memfd() {
        let _fd = fd_lock();
        const W: u32 = 64;
        const H: u32 = 48;
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(W, H);

        let first = decode_frame(ship_one(&mut server, &mut client, &rgba, W, H));
        let second = decode_frame(ship_one(&mut server, &mut client, &rgba, W, H));

        let a = rustix::fs::fstat(&first.fd).unwrap();
        let b = rustix::fs::fstat(&second.fd).unwrap();
        assert!(
            (a.st_dev, a.st_ino) != (b.st_dev, b.st_ino),
            "two captures must never share an open file"
        );
    }

    /// The write seal is the mechanical always-a-copy proof: the receiver
    /// provably cannot mutate or truncate the frame, so it can never be a
    /// window into live compositor memory.
    #[test]
    fn delivered_memfd_is_immutable() {
        let _fd = fd_lock();
        const W: u32 = 32;
        const H: u32 = 16;
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(W, H);
        let frame = decode_frame(ship_one(&mut server, &mut client, &rgba, W, H));

        let mut file = File::from(frame.fd);
        assert!(file.write_all(b"x").is_err(), "write seal must hold");
        assert!(
            rustix::fs::ftruncate(&file, 0).is_err(),
            "shrink seal must hold"
        );
        assert!(
            rustix::fs::ftruncate(&file, 1 << 20).is_err(),
            "grow seal must hold"
        );
    }

    /// The acceptance criterion verbatim: after N render/ship/close cycles
    /// the process's open-fd count is back at baseline — no fd is reused
    /// across requests and none leaks (server side drops its copy after
    /// sending, receiver side on frame drop).
    #[test]
    fn fd_count_returns_to_baseline() {
        run_fd_isolated("capture::tests::fd_count_returns_to_baseline", || {
            const W: u32 = 64;
            const H: u32 = 48;
            let (mut server, mut client) = Connection::pair().unwrap();
            let rgba = test_pattern::render(W, H);

            let baseline = open_fd_count();
            for _ in 0..8 {
                let frame = decode_frame(ship_one(&mut server, &mut client, &rgba, W, H));
                assert!(
                    open_fd_count() > baseline,
                    "a delivered frame holds exactly the fresh fd"
                );
                drop(frame);
                assert_eq!(
                    open_fd_count(),
                    baseline,
                    "fd count must return to baseline"
                );
            }
            assert_eq!(open_fd_count(), baseline);
        });
    }

    /// The typed mechanics failures: a mis-sized readback and a degenerate
    /// view are errors — never a fabricated frame, never an fd — which the
    /// chokepoint voices as `internal` (the readback case must never
    /// masquerade as a dead realm; the degenerate case is its no_surface
    /// backstop, unreachable when the chokepoint did its job).
    #[test]
    fn render_frame_fails_typed_never_fabricating_a_frame() {
        run_fd_isolated(
            "capture::tests::render_frame_fails_typed_never_fabricating_a_frame",
            || {
                let rgba = test_pattern::render(16, 16);
                let baseline = open_fd_count();

                let mis_sized = render_frame(&RealmViewFrame {
                    rgba: &rgba,
                    width: 32, // claims more pixels than the buffer holds
                    height: 32,
                });
                assert!(matches!(
                    mis_sized,
                    Err(CaptureError::MisSizedReadback {
                        got,
                        expected
                    }) if got == rgba.len() && expected == 32 * 32 * 4
                ));

                let degenerate = render_frame(&RealmViewFrame {
                    rgba: &[],
                    width: 0,
                    height: 0,
                });
                assert!(matches!(
                    degenerate,
                    Err(CaptureError::DegenerateView {
                        width: 0,
                        height: 0
                    })
                ));
                assert_eq!(open_fd_count(), baseline, "failures must not create fds");
            },
        );
    }

    /// The swizzle: RGBA bytes → little-endian xrgb8888 bytes (B,G,R,X),
    /// X forced to 0xFF even when the source alpha is not opaque.
    #[test]
    fn rgba_to_xrgb8888_swizzles_and_pins_padding() {
        assert_eq!(
            rgba_to_xrgb8888(&[0x11, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0x00]),
            [0x33, 0x22, 0x11, 0xff, 0xcc, 0xbb, 0xaa, 0xff]
        );
        assert!(rgba_to_xrgb8888(&[]).is_empty());
    }
}
