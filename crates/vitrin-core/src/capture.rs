//! The capture service: the server side of `vitrin_view.capture_frame`
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
//! The decision this task settles. A capture reads the compositor's
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
//!   the per-grant rate limit (hook below, mechanics P1.4.4) then bounds
//!   only the copy cost of capture itself.
//!
//! # The enforcement seam (P1.4.4 hook)
//!
//! [`CaptureGate`] is where the **single enforcement chokepoint** plugs in.
//! [`CaptureService::serve`] consults the gate exactly once per
//! `capture_frame`, before any pixel is copied; there is no other
//! authority-checking path to a frame. This module ships only the seam —
//! no policy: the M1.1 walking skeleton installs [`AutoApprove`] (the
//! auto-approved-grant posture of milestone M1.1), and P1.4.4 replaces it
//! with the real connection → principal → grant → verbs → constraints check
//! plus the per-grant `max_event_rate` token bucket.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};

use rustix::fs::{MemfdFlags, SealFlags};
use vitrin_ipc::{Connection, TransportError};
use vitrin_protocol::generated::vitrin_grant::{self, Refusal, Verb};
use vitrin_protocol::generated::vitrin_view::{events::FrameReady, Format, FrameFlags};

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

/// The wire identities a capture answers on: `frame_ready` is an event on
/// the `vitrin_view` facet, `refused` on its co-minted `vitrin_grant`
/// handle. Both are connection-scoped object ids (sender-constrained
/// handles — an id is only ever honored on the connection that minted it).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureIds {
    /// The `vitrin_view` object the `capture_frame` arrived on.
    pub view_id: u32,
    /// The grant handle co-minted with that view (`vitrin_grant.refused`
    /// is the failure terminal).
    pub grant_id: u32,
}

/// A use-time refusal decided by the [`CaptureGate`]: the wire code plus
/// the `retry_after_ms` refill hint (nonzero only for
/// [`Refusal::RateLimited`], per the IDL).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureRefusal {
    pub code: Refusal,
    pub retry_after_ms: u32,
}

/// The enforcement-chokepoint seam (P1.4.4 plugs in here).
///
/// Called exactly once per `capture_frame`, before any pixel is copied.
/// `Ok(())` admits the capture; `Err` refuses it recoverably and the
/// service voices the refusal as `vitrin_grant.refused(observe, code,
/// retry_after_ms)` — the caller never sends a second verdict.
///
/// Invariant (PRD Doc 2 §5, plan P1.4.4): this is the **only** authority
/// decision on the capture path. Implementations carry the whole
/// connection → principal → grant → verbs → constraints check and the
/// per-grant token bucket; no other capture code may accept or refuse on
/// authority grounds. (The `no_surface` refusal the service itself emits
/// for a degenerate view is a resource-existence outcome, not an authority
/// decision — the IDL's "never a stale frame" rule.)
pub(crate) trait CaptureGate {
    fn admit_capture(&mut self) -> Result<(), CaptureRefusal>;
}

/// The M1.1 walking-skeleton gate: every capture is admitted, matching the
/// milestone's auto-approved-grant posture ("agent … captures a
/// pixel-correct frame under an auto-approved grant"). Replaced — not
/// wrapped — by the real chokepoint in P1.4.4.
pub(crate) struct AutoApprove;

impl CaptureGate for AutoApprove {
    fn admit_capture(&mut self) -> Result<(), CaptureRefusal> {
        Ok(())
    }
}

/// How one `capture_frame` request was answered (its terminal event, which
/// [`CaptureService::serve`] has already put on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureOutcome {
    /// `frame_ready` sent: a fresh sealed memfd left with the reply.
    Delivered,
    /// `vitrin_grant.refused(observe, code, …)` sent.
    Refused(Refusal),
}

/// The capture service: answers `capture_frame` requests on one realm view.
///
/// Owns nothing but the gate; pixels come in per call (the caller reads
/// them from the retained framebuffer) and leave as a fresh memfd per
/// request. Serving is infallible at the protocol level — every failure
/// after admission maps to a recoverable refusal (`no_surface`,
/// `internal`), never a torn connection; only transport-level send errors
/// (peer gone, queue overflow — the P1.2.3 disconnect policy) surface as
/// `Err`.
pub(crate) struct CaptureService<G> {
    gate: G,
}

impl<G: CaptureGate> CaptureService<G> {
    pub fn new(gate: G) -> Self {
        Self { gate }
    }

    /// Answer one `capture_frame`: consult the gate, copy the latest
    /// completed frame into a fresh sealed memfd, and send exactly one
    /// terminal event on `conn` — `frame_ready` (with the memfd riding
    /// `SCM_RIGHTS`) or `vitrin_grant.refused`. The service's copy of the
    /// fd is closed before this returns ("the server closes its own copy
    /// after sending"); `send_or_queue` duplicates it if the frame parks.
    ///
    /// Terminals are emitted in call order and never coalesced (the IDL's
    /// reply-bearing contract): callers dispatch pipelined `capture_frame`
    /// requests to `serve` in arrival order and the wire order follows.
    pub fn serve(
        &mut self,
        view: RealmViewFrame<'_>,
        ids: CaptureIds,
        conn: &mut Connection,
    ) -> Result<CaptureOutcome, TransportError> {
        // The single authority decision (see CaptureGate).
        if let Err(refusal) = self.gate.admit_capture() {
            return refuse(conn, ids.grant_id, refusal);
        }

        // Resource existence, not authority: a view with no pixels never
        // yields a frame (frame_ready's dimensions are always nonzero).
        // Today this arm is the degenerate-view backstop; when the realm's
        // surface can genuinely vanish (shim crash, P1.5+) that condition
        // lands here too.
        if view.width == 0 || view.height == 0 {
            return refuse(
                conn,
                ids.grant_id,
                CaptureRefusal {
                    code: Refusal::NoSurface,
                    retry_after_ms: 0,
                },
            );
        }

        // A nonzero-dimension view whose readback buffer is mis-sized is a
        // server-side invariant break (a corrupted readback, not a dead
        // realm): the IDL's `internal` refusal, never `no_surface` — an
        // agent must not conclude the shim crashed from a readback bug.
        let expected_len = view.width as usize * view.height as usize * BYTES_PER_PIXEL;
        if view.rgba.len() != expected_len {
            tracing::warn!(
                got = view.rgba.len(),
                expected = expected_len,
                "capture readback length mismatch, refusing internal"
            );
            return refuse(
                conn,
                ids.grant_id,
                CaptureRefusal {
                    code: Refusal::Internal,
                    retry_after_ms: 0,
                },
            );
        }

        // The copy: convert the readback to the wire pixel format and seal
        // it into a fresh memfd. A local failure here is the IDL's
        // `internal` refusal ("server-side failure during this capture"),
        // recoverable by design.
        let fd = match sealed_frame_memfd(&rgba_to_xrgb8888(view.rgba)) {
            Ok(fd) => fd,
            Err(err) => {
                tracing::warn!("capture memfd failed, refusing internal: {err}");
                return refuse(
                    conn,
                    ids.grant_id,
                    CaptureRefusal {
                        code: Refusal::Internal,
                        retry_after_ms: 0,
                    },
                );
            }
        };

        let frame = FrameReady {
            fd,
            format: Format::Xrgb8888,
            width: view.width,
            height: view.height,
            // Pinned by the IDL: stride == width * 4 exactly in version 1.
            stride: view.width * BYTES_PER_PIXEL as u32,
            // Always 0 in version 1; any set bit would be a server
            // protocol violation.
            flags: FrameFlags::default(),
        };
        let bytes = frame.encode(ids.view_id);
        conn.send_or_queue(&bytes, Some(frame.fd.as_fd()))?;
        // `frame` (and with it the server's only copy of the memfd) drops
        // here: no fd is retained across requests, so the process fd count
        // returns to baseline once the receiver closes its end.
        Ok(CaptureOutcome::Delivered)
    }
}

/// Voice a refusal as the capture's terminal:
/// `vitrin_grant.refused(observe, code, retry_after_ms)` on the grant
/// handle.
///
/// This is the single site where every capture refusal is encoded, so it
/// enforces the IDL invariant "retry_after_ms is nonzero only for
/// rate_limited" unconditionally: a faulty [`CaptureGate`] cannot put an
/// IDL-violating refusal on the wire — the field is zeroed (with a
/// warning) for every other code, in release builds too.
fn refuse(
    conn: &mut Connection,
    grant_id: u32,
    refusal: CaptureRefusal,
) -> Result<CaptureOutcome, TransportError> {
    let retry_after_ms = if refusal.code == Refusal::RateLimited {
        refusal.retry_after_ms
    } else {
        if refusal.retry_after_ms != 0 {
            tracing::warn!(
                code = ?refusal.code,
                retry_after_ms = refusal.retry_after_ms,
                "gate emitted nonzero retry_after_ms on a non-rate_limited \
                 refusal; zeroing it (IDL vitrin_grant.refused invariant)"
            );
        }
        0
    };
    let event = vitrin_grant::events::Refused {
        verb: Verb::OBSERVE,
        code: refusal.code,
        retry_after_ms,
    };
    conn.send_or_queue(&event.encode(grant_id), None)?;
    Ok(CaptureOutcome::Refused(refusal.code))
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
fn sealed_frame_memfd(frame: &[u8]) -> io::Result<OwnedFd> {
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
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::FileExt;
    use std::sync::{Mutex, MutexGuard};

    use vitrin_ipc::{Connection, Message};
    use vitrin_protocol::generated::vitrin_grant::{self, Refusal, Verb};
    use vitrin_protocol::generated::vitrin_view::{events::FrameReady, Format};

    use super::*;
    use crate::test_pattern;

    /// Serializes every test that creates or counts file descriptors, so
    /// the `/proc/self/fd` baseline assertions can never race another
    /// test's socketpairs/memfds on the harness's thread pool. Shared with
    /// the headless backend's event-loop test for the same reason.
    pub(crate) static FD_QUIESCE: Mutex<()> = Mutex::new(());

    /// Lock [`FD_QUIESCE`], surviving a poisoned mutex (a failed test must
    /// not cascade into every later fd test failing on `unwrap`).
    pub(crate) fn fd_lock() -> MutexGuard<'static, ()> {
        FD_QUIESCE.lock().unwrap_or_else(|e| e.into_inner())
    }

    const VIEW_ID: u32 = 7;
    const GRANT_ID: u32 = 5;
    const IDS: CaptureIds = CaptureIds {
        view_id: VIEW_ID,
        grant_id: GRANT_ID,
    };

    /// Number of open fds in this process, via `/proc/self/fd`. The
    /// `read_dir` handle itself appears in the listing, but identically in
    /// every measurement, so equality comparisons are exact.
    fn open_fd_count() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .expect("/proc/self/fd is readable on Linux")
            .count()
    }

    /// Serve one admitted capture of `rgba` and return the client-side
    /// received message.
    fn serve_one(
        server: &mut Connection,
        client: &mut Connection,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Message {
        let mut service = CaptureService::new(AutoApprove);
        let outcome = service
            .serve(
                RealmViewFrame {
                    rgba,
                    width,
                    height,
                },
                IDS,
                server,
            )
            .expect("serve must not hit a transport error on a fresh pair");
        assert_eq!(outcome, CaptureOutcome::Delivered);
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

    /// A gate that refuses everything with a fixed code — the P1.4.4 seam
    /// exercised from the refusing side.
    struct RefuseAll(CaptureRefusal);

    impl CaptureGate for RefuseAll {
        fn admit_capture(&mut self) -> Result<(), CaptureRefusal> {
            Err(self.0)
        }
    }

    /// M1.1 golden: a served capture is byte-for-byte the xrgb8888
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
        let frame = decode_frame(serve_one(&mut server, &mut client, &rgba, W, H));

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
    /// changes:
    ///
    /// ```sh
    /// VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core sdk_capture_golden
    /// ```
    ///
    /// then commit the rewritten file together with the change that
    /// motivated it (the Python capture tests keep consuming it).
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

        let first = decode_frame(serve_one(&mut server, &mut client, &rgba, W, H));
        let second = decode_frame(serve_one(&mut server, &mut client, &rgba, W, H));

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
        let frame = decode_frame(serve_one(&mut server, &mut client, &rgba, W, H));

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

    /// The acceptance criterion verbatim: after N capture/receive/close
    /// cycles the process's open-fd count is back at baseline — no fd is
    /// reused across requests and none leaks (server side drops its copy
    /// inside `serve`, receiver side on frame drop).
    #[test]
    fn fd_count_returns_to_baseline() {
        let _fd = fd_lock();
        const W: u32 = 64;
        const H: u32 = 48;
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(W, H);

        let baseline = open_fd_count();
        for _ in 0..8 {
            let frame = decode_frame(serve_one(&mut server, &mut client, &rgba, W, H));
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
    }

    /// A refusing gate produces exactly one `vitrin_grant.refused(observe,
    /// code, retry_after_ms)` on the grant object, with no fd — and the
    /// authority decision is visibly the gate's alone (the seam P1.4.4
    /// fills).
    #[test]
    fn refused_capture_voices_refused_on_the_grant() {
        let _fd = fd_lock();
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(16, 16);
        let baseline = open_fd_count();

        let mut service = CaptureService::new(RefuseAll(CaptureRefusal {
            code: Refusal::RateLimited,
            retry_after_ms: 250,
        }));
        let outcome = service
            .serve(
                RealmViewFrame {
                    rgba: &rgba,
                    width: 16,
                    height: 16,
                },
                IDS,
                &mut server,
            )
            .unwrap();
        assert_eq!(outcome, CaptureOutcome::Refused(Refusal::RateLimited));
        assert_eq!(open_fd_count(), baseline, "a refusal must not create fds");

        let msg = client.recv_message().unwrap().unwrap();
        assert!(msg.fd.is_none(), "a refusal carries no fd");
        let (object_id, refused) =
            vitrin_grant::events::Refused::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, GRANT_ID, "refused is an event on the grant");
        assert_eq!(refused.verb, Verb::OBSERVE);
        assert_eq!(refused.code, Refusal::RateLimited);
        assert_eq!(refused.retry_after_ms, 250);
    }

    /// A degenerate view (no pixels) refuses `no_surface` rather than
    /// delivering an empty frame — `frame_ready`'s dimensions are always
    /// nonzero.
    #[test]
    fn degenerate_view_refuses_no_surface() {
        let _fd = fd_lock();
        let (mut server, mut client) = Connection::pair().unwrap();
        let mut service = CaptureService::new(AutoApprove);
        let outcome = service
            .serve(
                RealmViewFrame {
                    rgba: &[],
                    width: 0,
                    height: 0,
                },
                IDS,
                &mut server,
            )
            .unwrap();
        assert_eq!(outcome, CaptureOutcome::Refused(Refusal::NoSurface));

        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, refused) =
            vitrin_grant::events::Refused::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, GRANT_ID);
        assert_eq!(refused.code, Refusal::NoSurface);
        assert_eq!(refused.retry_after_ms, 0);
    }

    /// The wire invariant "retry_after_ms nonzero only for rate_limited"
    /// holds even against a faulty gate: `refuse()` — the single site
    /// encoding every capture refusal — zeroes the field for any other
    /// code instead of trusting the gate's output.
    #[test]
    fn faulty_gate_retry_hint_is_zeroed_on_non_rate_limited() {
        let _fd = fd_lock();
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(16, 16);

        let mut service = CaptureService::new(RefuseAll(CaptureRefusal {
            code: Refusal::Preempted,
            retry_after_ms: 999,
        }));
        let outcome = service
            .serve(
                RealmViewFrame {
                    rgba: &rgba,
                    width: 16,
                    height: 16,
                },
                IDS,
                &mut server,
            )
            .unwrap();
        assert_eq!(outcome, CaptureOutcome::Refused(Refusal::Preempted));

        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, refused) =
            vitrin_grant::events::Refused::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, GRANT_ID);
        assert_eq!(refused.code, Refusal::Preempted);
        assert_eq!(
            refused.retry_after_ms, 0,
            "retry_after_ms must be zeroed on the wire for non-rate_limited"
        );
    }

    /// A nonzero-dimension view with a mis-sized readback buffer is a
    /// server-side invariant break: it refuses `internal`, never
    /// `no_surface` — a readback bug must not masquerade as a dead realm.
    #[test]
    fn mis_sized_readback_refuses_internal_not_no_surface() {
        let _fd = fd_lock();
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(16, 16);
        let mut service = CaptureService::new(AutoApprove);
        let outcome = service
            .serve(
                RealmViewFrame {
                    rgba: &rgba,
                    width: 32, // claims more pixels than the buffer holds
                    height: 32,
                },
                IDS,
                &mut server,
            )
            .unwrap();
        assert_eq!(outcome, CaptureOutcome::Refused(Refusal::Internal));

        let msg = client.recv_message().unwrap().unwrap();
        assert!(msg.fd.is_none(), "a refusal carries no fd");
        let (object_id, refused) =
            vitrin_grant::events::Refused::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, GRANT_ID);
        assert_eq!(refused.code, Refusal::Internal);
        assert_eq!(refused.retry_after_ms, 0);
    }

    /// Pipelined captures answer strictly in serve order: terminal n
    /// answers request n, mixing successes and refusals.
    #[test]
    fn terminals_arrive_in_request_order() {
        let _fd = fd_lock();
        const W: u32 = 16;
        const H: u32 = 16;
        let (mut server, mut client) = Connection::pair().unwrap();
        let rgba = test_pattern::render(W, H);

        // Request 1: delivered. Request 2: refused (degenerate view).
        // Request 3: delivered.
        let mut service = CaptureService::new(AutoApprove);
        for degenerate in [false, true, false] {
            let (pixels, w, h): (&[u8], u32, u32) = if degenerate {
                (&[], 0, 0)
            } else {
                (&rgba, W, H)
            };
            service
                .serve(
                    RealmViewFrame {
                        rgba: pixels,
                        width: w,
                        height: h,
                    },
                    IDS,
                    &mut server,
                )
                .unwrap();
        }

        let first = client.recv_message().unwrap().unwrap();
        assert_eq!(first.header.object_id, VIEW_ID);
        drop(decode_frame(first));
        let second = client.recv_message().unwrap().unwrap();
        assert_eq!(second.header.object_id, GRANT_ID);
        let third = client.recv_message().unwrap().unwrap();
        assert_eq!(third.header.object_id, VIEW_ID);
        drop(decode_frame(third));
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
