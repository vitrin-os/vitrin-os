// SPDX-License-Identifier: MPL-2.0
//! The shim-facing protocol server (P1.3.4, issue #21): the core side of the
//! shim boundary — half of artifact A5's contract.
//!
//! [`ShimServer`] implements the server side of `vitrin_shim_session` and
//! `vitrin_shim_surface` exactly as `protocol/vitrin-v0.xml` pins them: the
//! `configure`-first bring-up, surface/seat minting under the watermark rule,
//! and the attach → damage → commit → `buffer_done`/`frame_done` frame loop.
//! Buffer path v0 is **shm/memfd copy-in** (plan decision D3): at commit the
//! core copies the attached buffer's pixels out of the shim's fd into
//! [`Scene::commit`] (the P1.3.3 seam) and owns its bytes from then on.
//! The dmabuf zero-copy path (P1.3.5) is layered on top through the
//! [`DmabufImporter`] seam: an embedder with a GPU passes an importer and a
//! `kind=dmabuf` commit is really imported (see [`crate::dmabuf`] for the
//! mechanics — allowlist, hostile-fd probe, failure detection, deferred
//! release); an embedder without one (headless, CI) passes `None` and every
//! dmabuf commit takes the protocol's designed fallback path
//! (`buffer_done(import_failed)` — the shim re-attaches as shm). This server
//! stays the one policy site either way: it maps the importer's typed
//! outcomes onto the wire's dispositions at the same single commit site
//! that resolves shm.
//!
//! # Atomicity (Wayland's double-buffered state model)
//!
//! `attach` and `damage` mutate **pending** state only; nothing the
//! compositor composites changes until `commit` latches the pending state
//! wholesale. Partial state is unrepresentable in the composited output by
//! construction: the scene sees exactly one [`Scene::commit`] per shim
//! `commit`, with bytes fully copied in before the call, in the same
//! single-threaded dispatch. There is no window in which a redraw could
//! observe half of frame N and half of frame N+1.
//!
//! # Decisions this task settles (documented per issue #21)
//!
//! - **`damage` (or `commit`) without any prior `attach` is `bad_order`**,
//!   log-and-close. This is the IDL's own razor ("against a surface that has
//!   *never been attached*"): pending damage is meaningless with no buffer
//!   to scope it to, and a correct shim can always avoid it. After the first
//!   attach, `damage`+`commit` with no *pending* attach is the legal repaint
//!   ("re-presents the current buffer with the new damage").
//! - **Buffer-size change mid-stream is legal.** Every `attach` carries its
//!   own geometry and is validated independently against its own fd; at
//!   commit the new size replaces the old wholesale, and the scene
//!   letterboxes/center-crops a mismatched size at 1:1 (the P1.3.3
//!   decision). The IDL never freezes a surface's size — deliberately so:
//!   `vitrin_shim_session.configure` says a version-1 core "may letterbox a
//!   fixed-size buffer instead of re-configuring", which presumes attaches
//!   whose size differs from the view are representable mid-stream. A
//!   mid-resize app therefore never dies for racing a configure.
//!
//! # Hostile-shim posture
//!
//! The shim is **outside the TCB**: every message and every fd is validated
//! before use, and every violation funnels into one of two existing kill
//! paths — no third policy layer:
//!
//! - Transport-level misbehavior (fd bombs, oversized frames, slow readers)
//!   never reaches this module: `vitrin-ipc` classifies it into
//!   [`DisconnectReason`](vitrin_ipc::DisconnectReason) and the connection
//!   source removes itself (the P1.2.3 funnel, reused wholesale).
//! - Protocol-level violations — the IDL's named log-and-close conditions
//!   (`invalid_buffer`, `bad_order`, `already_initialized`) plus the generic
//!   object-graph violations every class shares (unknown ids, watermark
//!   breaches, unknown opcodes, malformed payloads), plus the documented
//!   per-connection resource bounds (the conventions' live-object cap:
//!   [`MAX_LIVE_SURFACES`], fatal `resource_exhausted`) — surface as
//!   [`ShimViolation`]. Shim connections carry **no fatal-error event** (the
//!   IDL: "a shim that violates the protocol is logged and its connection
//!   closed"), so the embedder logs the violation and closes the connection,
//!   with no goodbye on the wire — the same conclusion the transport layer
//!   reached for its own violations.
//!
//! Defensive validation of an attached memfd (all before any byte is
//! trusted): the fd must actually be shmem-backed (`F_GET_SEALS` succeeds —
//! the kernel implements it only for memfd/tmpfs/hugetlb files, so a
//! hostile shim cannot substitute a regular file on a FUSE mount or slow
//! storage whose reads would block the single-threaded core loop; the
//! probe is purely in-kernel, so it is safe *before* `fstat`, which on a
//! hostile FUSE fd could itself block), nonzero dimensions, `stride >=
//! width * 4`, and fd size at least `stride * height` — all in 128-bit
//! math, so wire-derived `u32` geometry can never wrap a check (the
//! [`scene`](crate::scene) overflow precedent). Shim buffers remain
//! deliberately unsealed — version 1 demands no seals, the probe only
//! requires the fd to be seal-*capable* — so the copy itself uses `pread`
//! bounded by the validated geometry — never `mmap` — and a shim that
//! shrinks its memfd after attach produces a clean short-read error at
//! commit (`invalid_buffer`, log-and-close), not a `SIGBUS` in the TCB. A buffer too large for the
//! renderer's limits ([`MAX_SURFACE_DIM`]/[`MAX_SURFACE_BYTES`] — also the
//! defense against sparse-memfd allocation bombs) takes the *recoverable*
//! `buffer_done(too_large)` path the IDL defines, not a kill. Both
//! version-1 formats (xrgb8888/argb8888) are supported by the copy-in
//! converter, so `format_unsupported` is unreachable on the shm arm until a
//! format appends.
//!
//! An attached **dmabuf** fd is validated by the path that will actually
//! touch it, and only there: at attach the core stages the fd untouched (no
//! syscall — even `fstat` can block on a hostile fd), and at commit the
//! importer, if one is present, runs the non-blocking fdinfo authenticity +
//! size probe before its first driver call (the `F_GET_SEALS` analogue —
//! [`crate::dmabuf`]'s module docs carry the full argument). A core without
//! an importer never touches the fd at all and answers the designed
//! `import_failed` fallback. An fd lie the probe catches is the same
//! `invalid_buffer` log-and-close as the shm arm's; a genuine import
//! failure (either failure time — EGL rejection or first-render, collapsed
//! by the importer's probe composite) is the recoverable
//! `buffer_done(import_failed)` fallback event, never a silent black frame.
//!
//! # Zero-copy release semantics (P1.3.5)
//!
//! A successfully imported dmabuf is **retained**: the GPU samples the
//! client's buffer at every composite, so its `buffer_done(released)` is
//! deferred until the next successful commit replaces it (or the connection
//! dies, where no event exists) — the IDL's "at GPU-done under dmabuf
//! passthrough", proven by the importer's synchronous replacement/retire
//! sync. Re-attaching the retained cookie is `bad_order` (its ownership has
//! not returned). Failure dispositions stay prompt even while an earlier
//! buffer is retained — the deadlock-free reading of the IDL's two timing
//! regimes; see [`crate::dmabuf`]. The per-server [`CopyMeter`] instruments
//! the whole claim: the shm arm records exactly one client-pixel copy per
//! commit, the dmabuf arm records none.
//!
//! # Frame pacing (`frame_done`)
//!
//! The server is deliberately **time-agnostic**: a `commit` queues exactly
//! one owed `frame_done` (FIFO, per the IDL "at most one outstanding per
//! commit"), and the embedder calls [`ShimServer::presented`] when the
//! committed content has actually been presented — after the composite
//! completes on the headless path ("or, headless, after it would have
//! been"), at the host frame callback in nested mode. That is what relays
//! the *true* display cadence to the shim (PRD Doc 2 §4.4) instead of
//! letting the app render blind. A commit whose attached buffer failed
//! (`buffer_done` with a failure status) still receives its `frame_done`,
//! per the IDL — the app's pacing loop never stalls on a rejected buffer.
//!
//! # Wiring
//!
//! Fully wired. [`crate::session::start_realm`] forks the realm's shim,
//! sends `configure` on the still-blocking socketpair, and registers the
//! connection as a `vitrin_ipc::ConnectionSource`; `session::dispatch_shim`
//! then drives [`ShimServer::handle_message`] against the backend's scene,
//! and `RealmLifecycle` owns the process from fork to grave. The path is
//! exercised end to end by `session`'s own tests, which really fork
//! `vitrin-mock-shim` (the permanent fixture) and run a whole paced session
//! over the real event loop.
//!
//! **The runtime loop coalesces; the reference wiring in
//! [`crate::backend::headless`]'s test does not, and must not be copied.**
//! That harness composites synchronously inside the per-message dispatch
//! (one redraw per commit), which is exactly what the pacing test needs but
//! would hand a hostile shim CPU amplification: after one legal attach, a
//! repaint `commit` is a 12-byte message, so redrawing per commit buys a
//! full-output composite per 12 wire bytes. The runtime instead marks the
//! scene dirty when a commit latches and does at most one redraw +
//! [`presented`] per loop iteration, in `session::post_dispatch`.
//! `ShimServer` is built for that shape — [`wants_presentation`] is the
//! embedder's cue, and one [`presented`] call drains every owed
//! `frame_done` at once, so pacing is untouched and only the composites are
//! coalesced.
//!
//! [`presented`]: ShimServer::presented
//! [`wants_presentation`]: ShimServer::wants_presentation

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

use vitrin_ipc::{Message, TransportError};
use vitrin_protocol::error::DecodeError;
use vitrin_protocol::generated::vitrin_shim_session as session;
use vitrin_protocol::generated::vitrin_shim_surface as shim_surface;
use vitrin_protocol::generated::vitrin_view::Format;

use crate::dmabuf::{CopyMeter, DmabufImporter, DmabufSpec, ImportDenied};
use crate::scene::{DamageRect, Scene, SurfaceContent, BYTES_PER_PIXEL};

/// The shim-session bootstrap object: implicit object 1 on a shim
/// connection, never created by a message (conventions section 3).
pub(crate) const SHIM_SESSION_ID: u32 = 1;

/// Top of the client-allocated object-id range; ids above it are reserved
/// to the server (conventions section 3, unused in version 0).
const CLIENT_ID_MAX: u32 = 0xfeff_ffff;

/// Renderer per-dimension limit. A committed buffer wider or taller than
/// this takes the recoverable `buffer_done(too_large)` path (the IDL's
/// "buffer exceeds the renderer's limits"). 16384 matches the common GPU
/// max-texture-size the dmabuf path (P1.3.5) will inherit.
pub(crate) const MAX_SURFACE_DIM: u32 = 16384;

/// Renderer total-size limit: the copy-in allocation cap. Without it a
/// hostile shim could attach a sparse memfd declaring gigantic (but
/// size-consistent — `ftruncate` allocates nothing) geometry and make the
/// copy-in allocate unbounded memory inside the TCB. 128 MiB admits an
/// 8K xrgb8888 buffer (7680 x 4320 x 4 ≈ 127 MiB) with the same
/// recoverable `too_large` disposition, never a kill.
pub(crate) const MAX_SURFACE_BYTES: u128 = 128 * 1024 * 1024;

/// Cap on live surfaces per shim connection — the conventions' documented
/// per-connection live-object cap (fatal `resource_exhausted`), applied to
/// the one object class a shim can mint without bound. Version 1 defines no
/// destructors, so every `create_surface` is a permanent per-connection
/// allocation: a `SurfaceState` (heap) plus, via a staged attach, up to one
/// retained buffer fd. Without a cap a hostile shim streaming 12-byte
/// `create_surface` frames grows core memory O(messages) and can drive the
/// whole process to EMFILE by staging one fd per surface — so the cap
/// bounds both at O(1). Version 1 composites a single-maximized layout;
/// 16 is generous headroom for any legitimate multi-surface shim.
pub(crate) const MAX_LIVE_SURFACES: usize = 16;

/// Cap on pending damage rectangles retained verbatim between commits.
/// Damage is a hint the core may over-approximate, so rectangles past the
/// cap are folded into a running bounding box instead of growing the Vec —
/// a shim streaming damage forever without committing pins O(1) memory,
/// not O(messages).
const MAX_DAMAGE_RECTS: usize = 64;

/// A protocol violation by the shim: one of the IDL's named log-and-close
/// conditions, or a generic object-graph violation every connection class
/// shares. Always connection-fatal — the embedder logs it (the variant
/// names match the IDL's condition vocabulary so the core log is greppable
/// against the spec) and closes the connection without a wire goodbye
/// (shim connections have no fatal-error event).
#[derive(Debug)]
pub(crate) enum ShimViolation {
    /// `invalid_buffer`: attach geometry inconsistent with the fd's actual
    /// size, a zero dimension, a stride below `width * 4`, or a copy-in
    /// read that failed because the fd lied (e.g. shrunk after attach).
    InvalidBuffer(String),
    /// `bad_order`: damage/commit against a never-attached surface, or
    /// re-attaching a dmabuf `buffer_id` that has not yet received its
    /// `buffer_done`.
    BadOrder(&'static str),
    /// `already_initialized`: a second `get_seat` on the session.
    AlreadyInitialized,
    /// `resource_exhausted`: a documented per-connection resource bound was
    /// breached — the live-object cap ([`MAX_LIVE_SURFACES`]). A
    /// denial-of-service confinement, not a semantic judgement
    /// (conventions error taxonomy).
    ResourceExhausted(&'static str),
    /// `invalid_object`: unknown/foreign object id, or a `new_id` at or
    /// below the watermark / outside the client range.
    InvalidObject {
        object_id: u32,
        detail: &'static str,
    },
    /// An opcode the target object's interface does not define (includes
    /// any request on the events-only `vitrin_shim_seat`).
    UnknownOpcode { object_id: u32, opcode: u8 },
    /// The frame did not decode as the selected message (bad enum value,
    /// truncated payload, fd-count contradiction, ...).
    Malformed(DecodeError),
}

impl fmt::Display for ShimViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShimViolation::InvalidBuffer(detail) => write!(f, "invalid_buffer: {detail}"),
            ShimViolation::BadOrder(detail) => write!(f, "bad_order: {detail}"),
            ShimViolation::AlreadyInitialized => {
                write!(f, "already_initialized: second get_seat on the session")
            }
            ShimViolation::ResourceExhausted(detail) => {
                write!(f, "resource_exhausted: {detail}")
            }
            ShimViolation::InvalidObject { object_id, detail } => {
                write!(f, "invalid_object: id {object_id}: {detail}")
            }
            ShimViolation::UnknownOpcode { object_id, opcode } => {
                write!(f, "unknown opcode {opcode} on object {object_id}")
            }
            ShimViolation::Malformed(e) => write!(f, "malformed message: {e}"),
        }
    }
}

impl From<DecodeError> for ShimViolation {
    fn from(e: DecodeError) -> Self {
        ShimViolation::Malformed(e)
    }
}

/// Why [`ShimServer::handle_message`] gave up on the connection: either the
/// shim violated the protocol (log and close — the shim-class analogue of
/// the transport's [`DisconnectReason`](vitrin_ipc::DisconnectReason)
/// funnel) or sending an event hit a terminal transport condition (the
/// existing P1.2.3 policy already covers it: the event-loop source observes
/// the sticky state and removes the connection).
#[derive(Debug)]
pub(crate) enum ShimFault {
    Violation(ShimViolation),
    Transport(TransportError),
}

impl fmt::Display for ShimFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShimFault::Violation(v) => write!(f, "shim protocol violation: {v}"),
            ShimFault::Transport(e) => write!(f, "transport: {e}"),
        }
    }
}

impl From<ShimViolation> for ShimFault {
    fn from(v: ShimViolation) -> Self {
        ShimFault::Violation(v)
    }
}

impl From<TransportError> for ShimFault {
    fn from(e: TransportError) -> Self {
        ShimFault::Transport(e)
    }
}

/// What the core announces in `configure`: the realm identity assigned at
/// fork (informational — the shim can never assert it) and the realm-view
/// geometry the shim advertises to its app.
pub(crate) struct ShimConfig {
    pub realm: String,
    pub width: u32,
    pub height: u32,
}

/// A staged (pending) buffer: the decoded `attach`, fd owned until the
/// commit that consumes it (or the supersede/teardown that releases it).
struct PendingBuffer {
    buffer_id: u32,
    fd: std::os::fd::OwnedFd,
    kind: shim_surface::Kind,
    format: Format,
    width: u32,
    height: u32,
    stride: u32,
}

/// Per-surface protocol state: the double-buffered pending set.
#[derive(Default)]
struct SurfaceState {
    /// Whether any `attach` was ever received — the `bad_order` razor for
    /// `damage`/`commit` (see the module docs' settled decision).
    ever_attached: bool,
    pending_buffer: Option<PendingBuffer>,
    pending_damage: Vec<DamageRect>,
}

/// The zero-copy buffer the core is currently holding (P1.3.5): a
/// successfully imported dmabuf whose `buffer_done(released)` is deferred
/// until content replaces it (the module docs' release semantics). One per
/// server, mirroring the scene's single content slot — but tagged with its
/// surface so the deferred event reaches the right object.
struct RetainedDmabuf {
    surface_id: u32,
    buffer_id: u32,
}

/// The per-connection shim protocol server. One instance per shim
/// connection; single-threaded, driven by decoded [`Message`]s from the
/// connection's event source and by the embedder's presentation cadence.
pub(crate) struct ShimServer {
    config: ShimConfig,
    /// Highest object id allocated on this connection (starts at the
    /// bootstrap id); every `new_id` must exceed it — strictly increasing,
    /// never reused (conventions 3.1).
    watermark: u32,
    /// Surface object id → protocol state. `BTreeMap` so iteration (and
    /// hence log output) is deterministic.
    surfaces: BTreeMap<u32, SurfaceState>,
    /// The single seat object, once minted (`get_seat`): the address
    /// [`deliver_seat_event`](Self::deliver_seat_event) encodes routed
    /// input toward (P1.3.7). The mint and its `already_initialized` rule
    /// are session protocol served here; the shim-side replay is P1.6.3.
    seat_id: Option<u32>,
    /// Surface ids of commits whose `frame_done` is still owed, in commit
    /// order (FIFO-correlated per the IDL). Drained by [`presented`].
    ///
    /// [`presented`]: Self::presented
    owed_frame_dones: VecDeque<u32>,
    /// The retained zero-copy buffer, if any (P1.3.5): its
    /// `buffer_done(released)` is owed at replacement. The importer holds
    /// the matching GPU texture; this is the protocol-side half.
    retained_dmabuf: Option<RetainedDmabuf>,
    /// Zero-copy instrumentation: every core-side CPU copy of client
    /// pixels (the shm copy-in — the only such site) is recorded here.
    copy_meter: CopyMeter,
}

impl ShimServer {
    pub fn new(config: ShimConfig) -> Self {
        Self {
            config,
            watermark: SHIM_SESSION_ID,
            surfaces: BTreeMap::new(),
            seat_id: None,
            owed_frame_dones: VecDeque::new(),
            retained_dmabuf: None,
            copy_meter: CopyMeter::default(),
        }
    }

    /// The client-pixel copy instrumentation (P1.3.5): tests assert the shm
    /// path copies exactly once per commit and the dmabuf path never; the
    /// M1.5 perf report reads the same numbers.
    ///
    /// Test-only today, and narrowly so: the meter itself is maintained on
    /// every commit the runtime handles: only this *reader* has no runtime
    /// caller, until the M1.5 perf report grows one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn copy_meter(&self) -> &CopyMeter {
        &self.copy_meter
    }

    /// Send `vitrin_shim_session.configure` — the core's first message on a
    /// shim connection, guaranteed to precede the processing of any shim
    /// request. Call once, immediately after establishing the connection
    /// and before dispatching anything the shim sent.
    pub fn send_configure<F>(&self, send: &mut F) -> Result<(), TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let configure = session::events::Configure {
            realm: self.config.realm.clone(),
            width: self.config.width,
            height: self.config.height,
        };
        send(&configure.encode(SHIM_SESSION_ID))
    }

    /// **Re-send `configure` at a new realm-view size**, updating this
    /// server's record of what the shim was told (WS-E.1.4, issue #210).
    ///
    /// The IDL says `configure` "may be re-sent when the view resizes",
    /// and this is the core finally doing it: a realm in the fullscreen
    /// arrangement has its view size track the output's, so it is
    /// re-configured on entering that arrangement and again on every later
    /// output resize. A realm in the windowed arrangement is never
    /// re-configured at all — the core imposes no size and `Scene::compose`
    /// letterboxes whatever the app keeps committing.
    ///
    /// Returns `Ok(false)` and sends **nothing** when the size is
    /// unchanged. That is not an optimization: a re-sent `configure` makes
    /// a shim re-mode its output and re-configure its toplevel, which a
    /// real app answers with a fresh buffer, so sending one per redundant
    /// request would let a holder drive an app's resize loop at its
    /// max_event_rate.
    ///
    /// The realm identity is never re-stated to a different value — it is
    /// assigned at fork and this only ever carries the same string back —
    /// so nothing a shim could have cached about who it is can change here.
    pub fn reconfigure<F>(
        &mut self,
        width: u32,
        height: u32,
        send: &mut F,
    ) -> Result<bool, TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        if (self.config.width, self.config.height) == (width, height) {
            return Ok(false);
        }
        self.config.width = width;
        self.config.height = height;
        self.send_configure(send)?;
        Ok(true)
    }

    /// The realm-view size this server last told its shim (its spawn
    /// `configure`, or the most recent [`Self::reconfigure`]).
    ///
    /// Test-only, and narrowly so: the *record* is maintained on every
    /// reconfigure the runtime performs, and only this reader has no runtime
    /// caller — nothing in the core needs to ask what it already decided.
    /// It is what lets a test assert that `set_fullscreen` reached the shim
    /// as a real `configure`, rather than that a flag flipped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn configured_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Dispatch one decoded frame from the shim connection.
    ///
    /// `importer` is the embedder's dmabuf import capability (P1.3.5):
    /// `Some` on a GPU-backed embedder, `None` on headless/CI paths, and
    /// **the same importer for the connection's whole lifetime** — it holds
    /// the retained zero-copy buffer between commits, so swapping or
    /// dropping it mid-connection would orphan that buffer's release.
    ///
    /// Returns `Ok(true)` iff a `commit` was latched — the embedder should
    /// recomposite (redraw) and, once that content has been presented, call
    /// [`presented`](Self::presented) so the owed `frame_done` goes out.
    /// `Err` means the connection must die: log the fault and close (see
    /// [`ShimFault`] for the two funnels).
    pub fn handle_message<F>(
        &mut self,
        msg: Message,
        scene: &mut Scene,
        importer: Option<&mut dyn DmabufImporter>,
        send: &mut F,
    ) -> Result<bool, ShimFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let object_id = msg.header.object_id;
        let opcode = msg.header.opcode;
        if object_id == SHIM_SESSION_ID {
            match opcode {
                session::requests::CreateSurface::OPCODE => {
                    let (_, req) = session::requests::CreateSurface::decode(&msg.bytes, msg.fd)
                        .map_err(ShimViolation::from)?;
                    // The live-object cap precedes the mint: version 1 has
                    // no destructors, so every surface is a permanent
                    // allocation of core memory (and, via a staged attach,
                    // up to one retained fd) — unbounded minting is the
                    // conventions' fatal resource_exhausted, not a legal
                    // stream.
                    if self.surfaces.len() >= MAX_LIVE_SURFACES {
                        return Err(
                            ShimViolation::ResourceExhausted("live-surface cap exceeded").into(),
                        );
                    }
                    self.allocate_id(req.surface)?;
                    self.surfaces.insert(req.surface, SurfaceState::default());
                    Ok(false)
                }
                session::requests::GetSeat::OPCODE => {
                    let (_, req) = session::requests::GetSeat::decode(&msg.bytes, msg.fd)
                        .map_err(ShimViolation::from)?;
                    self.allocate_id(req.seat)?;
                    if self.seat_id.is_some() {
                        return Err(ShimViolation::AlreadyInitialized.into());
                    }
                    self.seat_id = Some(req.seat);
                    Ok(false)
                }
                _ => Err(ShimViolation::UnknownOpcode { object_id, opcode }.into()),
            }
        } else if self.surfaces.contains_key(&object_id) {
            match opcode {
                shim_surface::requests::Attach::OPCODE => {
                    self.handle_attach(object_id, msg, send)?;
                    Ok(false)
                }
                shim_surface::requests::Damage::OPCODE => {
                    let (_, req) = shim_surface::requests::Damage::decode(&msg.bytes, msg.fd)
                        .map_err(ShimViolation::from)?;
                    self.handle_damage(object_id, req)?;
                    Ok(false)
                }
                shim_surface::requests::Commit::OPCODE => {
                    shim_surface::requests::Commit::decode(&msg.bytes, msg.fd)
                        .map_err(ShimViolation::from)?;
                    self.handle_commit(object_id, scene, importer, send)?;
                    Ok(true)
                }
                _ => Err(ShimViolation::UnknownOpcode { object_id, opcode }.into()),
            }
        } else if self.seat_id == Some(object_id) {
            // vitrin_shim_seat is events-only: it defines no requests, so
            // any opcode toward it is a violation.
            Err(ShimViolation::UnknownOpcode { object_id, opcode }.into())
        } else {
            Err(ShimViolation::InvalidObject {
                object_id,
                detail: "unknown object id",
            }
            .into())
        }
    }

    /// Whether any commit is still owed its `frame_done` — the embedder's
    /// cue to schedule a presentation.
    pub fn wants_presentation(&self) -> bool {
        !self.owed_frame_dones.is_empty()
    }

    /// The committed content has been presented (headless: the composite
    /// completed; nested: the host frame callback fired) at monotonic
    /// `time_ms`. Sends every owed `frame_done` in commit order — exactly
    /// one per commit, so the shim's app paces to the true output cadence.
    pub fn presented<F>(&mut self, time_ms: u32, send: &mut F) -> Result<(), TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        while let Some(surface_id) = self.owed_frame_dones.pop_front() {
            let event = shim_surface::events::FrameDone { time_ms };
            send(&event.encode(surface_id))?;
        }
        Ok(())
    }

    /// Whether the shim has minted its seat yet (`get_seat` processed). The
    /// core can only deliver a seat event — and record its delivery-point
    /// audit entry (issue #83) — once it has; a test predicate pumps the loop
    /// until this is true before driving `route_seat`.
    #[cfg(test)]
    pub(crate) fn seat_minted(&self) -> bool {
        self.seat_id.is_some()
    }

    /// Deliver one routed, origin-tagged seat event to the shim (P1.3.7):
    /// the delivery half of the input path, encoding the
    /// [`SeatDelivery`](crate::input::SeatDelivery) toward the session's
    /// seat object. Returns whether it went out: input the core routes to
    /// the realm before the shim has minted its seat "has no destination
    /// and is dropped" (IDL `vitrin_shim_session.get_seat`) — a trace, not
    /// an error. The origin tag rides the wire on every event; this method
    /// only addresses, it never constructs or re-tags (B2).
    pub fn deliver_seat_event<F>(
        &self,
        event: &crate::input::SeatDelivery,
        send: &mut F,
    ) -> Result<bool, TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let Some(seat_id) = self.seat_id else {
            tracing::trace!("seat event dropped: shim has not minted its seat yet");
            return Ok(false);
        };
        send(&event.encode(seat_id))?;
        Ok(true)
    }

    /// The shim connection is gone (EOF, transport fault, or protocol
    /// violation): drop the realm's surface from the scene so no capture
    /// can ever serve a stale frame, retire any retained zero-copy content
    /// (the importer's texture — and with it the dmabuf-backed buffer — is
    /// dropped; no `released` event exists to send on a dead connection, per
    /// the IDL's "on connection death the core closes every attach fd it
    /// still holds"), reset **this realm's** input-router state so no
    /// implicit-grab or pointer state leaks into the next shim generation (a
    /// stale grab would route off-surface input to an app that never saw the
    /// press — a phantom grab), and release every staged buffer fd (they
    /// close as `self` drops — no fd leak). The embedder calls this exactly
    /// once, with the connection's importer and the session's router, then
    /// forgets server, importer state, and connection. Taking the router
    /// here keeps realm teardown a single funnel: an embedder (P1.5.2)
    /// cannot reset the scene but forget the seat state.
    ///
    /// `realm` is why the router reset is [`InputRouter::reset_for`] and not
    /// a bare clear: one router serves a session that may hold several
    /// realms (WS-E.1.2), so a realm dying must forget its **own** seat
    /// state and leave a sibling's alone — otherwise a key the sibling's app
    /// is holding latches down when an unrelated realm exits. The `scene` and
    /// the `importer` handed in are likewise **this realm's own** since
    /// WS-E.1.3 (the embedder resolves both by realm id in
    /// `Presenter::teardown_view`: the scene from `RealmScenes::scene_mut`,
    /// the importer over `RealmGpuContent::slot_mut`), so this drops exactly
    /// the dying realm's surface and exactly its own retained texture, and
    /// leaves every sibling's composed view byte-identical and every
    /// sibling's GPU content resident. They used to be the session's one
    /// scene and one retained slot, where clearing them took whatever
    /// happened to be there -- fail-closed, but not scoped.
    ///
    /// [`InputRouter::reset_for`]: crate::input::InputRouter::reset_for
    pub fn connection_closed<H: crate::input::PreemptionHook>(
        self,
        realm: &crate::grants::RealmId,
        scene: &mut Scene,
        importer: Option<&mut dyn DmabufImporter>,
        router: &mut crate::input::InputRouter<H>,
    ) {
        scene.clear_surface();
        if let Some(importer) = importer {
            importer.clear();
        }
        router.reset_for(realm);
        // `self` drops here: every `PendingBuffer.fd` closes with it.
    }

    /// Enforce the watermark rule (conventions 3.1) for one `new_id`:
    /// strictly increasing, never reused, inside the client range.
    fn allocate_id(&mut self, id: u32) -> Result<(), ShimViolation> {
        if id <= self.watermark || id > CLIENT_ID_MAX {
            return Err(ShimViolation::InvalidObject {
                object_id: id,
                detail: "new_id at/below the watermark or outside the client range",
            });
        }
        self.watermark = id;
        Ok(())
    }

    /// `attach`: validate the buffer defensively, then stage it as pending.
    /// A superseded pending buffer is released promptly (`buffer_done
    /// (released)`, unused) — no attach ever leaves ownership dangling.
    fn handle_attach<F>(
        &mut self,
        surface_id: u32,
        msg: Message,
        send: &mut F,
    ) -> Result<(), ShimFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let (_, req) = shim_surface::requests::Attach::decode(&msg.bytes, msg.fd)
            .map_err(ShimViolation::from)?;

        // Geometry validation before anything else — a zero dimension or a
        // stride that cannot hold a row is `invalid_buffer` regardless of
        // kind. 128-bit math: wire-derived u32s can never wrap a check.
        if req.width == 0 || req.height == 0 {
            return Err(ShimViolation::InvalidBuffer(format!(
                "zero dimension: {}x{}",
                req.width, req.height
            ))
            .into());
        }
        let min_stride = u64::from(req.width) * BYTES_PER_PIXEL as u64;
        if u64::from(req.stride) < min_stride {
            return Err(ShimViolation::InvalidBuffer(format!(
                "stride {} below width * 4 = {min_stride}",
                req.stride
            ))
            .into());
        }
        // fd validation, shm only: a dmabuf's fd is the import path's to
        // validate (P1.3.5), and this core never touches a dmabuf fd.
        if req.kind == shim_surface::Kind::Shm {
            // The fd must actually be shmem-backed before *any* other fd
            // syscall trusts it: `F_GET_SEALS` is implemented by the kernel
            // only for memfd/tmpfs/hugetlb files, entirely in-kernel — a
            // FUSE server cannot fake it, and unlike `fstat` it can never
            // block on hostile filesystem I/O. Without this probe a hostile
            // shim could pass a regular file on a mount it controls whose
            // reads stall, wedging the single-threaded core loop at commit
            // (a hang never reaches the DisconnectReason funnel). Shmem
            // reads cannot block on attacker-controlled I/O. No seals are
            // *required* (shim buffers stay unsealed); the fd merely has to
            // be seal-capable.
            if rustix::fs::fcntl_get_seals(&req.fd).is_err() {
                return Err(ShimViolation::InvalidBuffer(
                    "kind=shm fd is not memfd/shmem-backed (F_GET_SEALS failed)".into(),
                )
                .into());
            }
            // The fd's actual size must cover the declared geometry.
            let required = u128::from(req.stride) * u128::from(req.height);
            let actual = rustix::fs::fstat(&req.fd)
                .map(|st| st.st_size.max(0) as u128)
                .unwrap_or(0);
            if actual < required {
                return Err(ShimViolation::InvalidBuffer(format!(
                    "fd size {actual} below stride * height = {required}"
                ))
                .into());
            }
        }

        let state = self
            .surfaces
            .get_mut(&surface_id)
            .expect("dispatch routed on a known surface id");

        // Re-attaching a dmabuf cookie whose buffer_done has not returned
        // is `bad_order` (IDL). Two places can be awaiting one: the pending
        // slot (staged, uncommitted) and — under zero-copy (P1.3.5) — the
        // retained slot, whose buffer the GPU is still sampling; the check
        // covers both, regardless of the *new* attach's kind (the cookie
        // names a buffer whose ownership has not returned, which is the
        // whole razor).
        if let Some(pending) = &state.pending_buffer {
            if pending.kind == shim_surface::Kind::Dmabuf && pending.buffer_id == req.buffer_id {
                return Err(ShimViolation::BadOrder(
                    "re-attach of a dmabuf buffer_id still awaiting buffer_done",
                )
                .into());
            }
        }
        if self
            .retained_dmabuf
            .as_ref()
            .is_some_and(|r| r.surface_id == surface_id && r.buffer_id == req.buffer_id)
        {
            return Err(ShimViolation::BadOrder(
                "re-attach of a retained dmabuf buffer_id still awaiting buffer_done",
            )
            .into());
        }

        // Supersede: the replaced pending attach is released promptly,
        // unused (exactly-once buffer_done, in attach order).
        if let Some(old) = state.pending_buffer.take() {
            let event = shim_surface::events::BufferDone {
                buffer_id: old.buffer_id,
                status: shim_surface::BufferStatus::Released,
            };
            send(&event.encode(surface_id))?;
            // `old` (and its fd) drops here.
        }

        state.ever_attached = true;
        state.pending_buffer = Some(PendingBuffer {
            buffer_id: req.buffer_id,
            fd: req.fd,
            kind: req.kind,
            format: req.format,
            width: req.width,
            height: req.height,
            stride: req.stride,
        });
        Ok(())
    }

    /// `damage`: accumulate one pending rectangle (bounded; see
    /// [`MAX_DAMAGE_RECTS`]). Out-of-bounds or degenerate rectangles are a
    /// non-error — clamping happens at composition, and version 1
    /// recomposites the full view anyway (damage is a verifiable hint, per
    /// the IDL, not a correctness input).
    fn handle_damage(
        &mut self,
        surface_id: u32,
        req: shim_surface::requests::Damage,
    ) -> Result<(), ShimViolation> {
        let state = self
            .surfaces
            .get_mut(&surface_id)
            .expect("dispatch routed on a known surface id");
        if !state.ever_attached {
            return Err(ShimViolation::BadOrder(
                "damage against a never-attached surface",
            ));
        }
        let rect = DamageRect {
            x: req.x,
            y: req.y,
            width: req.width,
            height: req.height,
        };
        if state.pending_damage.len() < MAX_DAMAGE_RECTS {
            state.pending_damage.push(rect);
        } else {
            let last = state
                .pending_damage
                .last_mut()
                .expect("cap is nonzero, so a full Vec has a last element");
            last.union(rect);
        }
        Ok(())
    }

    /// `commit`: atomically latch pending state. The one call site of
    /// [`Scene::commit_with_damage`] on the shim path — copy-in happens
    /// entirely here, before the scene changes, so composition never
    /// observes a partially applied frame — and the one call site of
    /// [`DmabufImporter::import`], so zero-copy content latches at exactly
    /// the same seam.
    fn handle_commit<F>(
        &mut self,
        surface_id: u32,
        scene: &mut Scene,
        importer: Option<&mut dyn DmabufImporter>,
        send: &mut F,
    ) -> Result<(), ShimFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let state = self
            .surfaces
            .get_mut(&surface_id)
            .expect("dispatch routed on a known surface id");
        if !state.ever_attached {
            return Err(ShimViolation::BadOrder("commit against a never-attached surface").into());
        }

        // Damage is latched (and logged — per-rectangle wire travel exists
        // so damage-only updates are verifiable in the core log) whether or
        // not a new buffer is pending. Folded to one bounding box here — the
        // one hint [`Scene::commit_with_damage`] wants — rather than handing
        // the whole `Vec` further down; an empty `Vec` (no `damage` request
        // this commit — a legal but coarse shim) folds to `None`, which the
        // scene treats as "cannot be bounded", never as "nothing changed".
        let damage_rects = std::mem::take(&mut state.pending_damage);
        let damage_hint = damage_rects.iter().copied().reduce(|mut acc, r| {
            acc.union(r);
            acc
        });
        tracing::trace!(
            surface = surface_id,
            damage_rects = damage_rects.len(),
            "shim commit"
        );

        match state.pending_buffer.take() {
            // Repaint: no new attach — re-present the current buffer with
            // the new damage. Legal, no buffer_done (no attach to answer).
            // Nothing about the *content* changed (the same buffer is being
            // re-presented), so there is nothing for the scene's damage
            // bookkeeping to accumulate here — only a new attach's copy-in
            // produces content the scene has not already seen.
            None => {}
            Some(pending) => {
                let buffer_id = pending.buffer_id;
                // `None` = disposition deferred: a successful zero-copy
                // import is retained and answers at replacement (the
                // module docs' release semantics); any deferred release it
                // triggered has already been sent, in attach order.
                if let Some(status) =
                    self.apply_buffer(surface_id, pending, damage_hint, scene, importer, send)?
                {
                    let event = shim_surface::events::BufferDone { buffer_id, status };
                    send(&event.encode(surface_id))?;
                }
            }
        }

        // Exactly one frame_done owed per commit — even when the buffer
        // failed (the previously committed content is re-presented), so the
        // app's pacing loop never stalls on a rejected buffer (IDL).
        self.owed_frame_dones.push_back(surface_id);
        Ok(())
    }

    /// Consume one pending buffer at commit: copy a valid shm buffer into
    /// the scene, import a valid dmabuf zero-copy, or resolve the
    /// recoverable failure dispositions. Returns the `buffer_done` status —
    /// or `None` when the disposition is deferred (a retained zero-copy
    /// import; see the module docs). The buffer's fd closes on return in
    /// **every** arm — a retaining import consumes it too ("the core takes
    /// ownership of the fd received in attach and closes it before or as
    /// it emits this event"): what a retained import holds is the kernel
    /// buffer via its EGLImage, never an open fd (see
    /// [`crate::dmabuf::GpuContent`]).
    fn apply_buffer<F>(
        &mut self,
        surface_id: u32,
        pending: PendingBuffer,
        damage_hint: Option<DamageRect>,
        scene: &mut Scene,
        importer: Option<&mut dyn DmabufImporter>,
        send: &mut F,
    ) -> Result<Option<shim_surface::BufferStatus>, ShimFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        if pending.kind == shim_surface::Kind::Dmabuf {
            // The zero-copy path has no separate "upload" step to limit —
            // the GPU samples the client's own buffer directly — so the
            // damage hint has nothing to do here.
            return self.apply_dmabuf(surface_id, pending, importer, send);
        }

        // Renderer limits: recoverable too_large, never a kill (and the
        // sparse-memfd allocation-bomb defense — see the constants).
        let total_bytes =
            u128::from(pending.width) * u128::from(pending.height) * BYTES_PER_PIXEL as u128;
        if pending.width > MAX_SURFACE_DIM
            || pending.height > MAX_SURFACE_DIM
            || total_bytes > MAX_SURFACE_BYTES
        {
            tracing::debug!(
                width = pending.width,
                height = pending.height,
                "buffer exceeds renderer limits: too_large"
            );
            return Ok(Some(shim_surface::BufferStatus::TooLarge));
        }

        // The D3 copy-in: pread rows bounded by the validated geometry —
        // never a mapping — into freshly owned RGBA bytes. A short read
        // means the fd lied since attach (e.g. shrunk: shim buffers are
        // unsealed) — the invalid_buffer log-and-close condition.
        let file = File::from(pending.fd);
        let rgba = copy_in(
            &file,
            pending.width,
            pending.height,
            pending.stride,
            pending.format,
        )
        .map_err(|e| ShimViolation::InvalidBuffer(format!("copy-in failed: {e}")))?;
        self.copy_meter.record(rgba.len());

        // Structurally exact by construction; mapped anyway so a future
        // copy-in bug surfaces as the protocol's own condition, never as a
        // half-applied commit.
        let content = SurfaceContent::from_rgba(rgba, pending.width, pending.height)
            .map_err(|e| ShimViolation::InvalidBuffer(e.to_string()))?;
        scene.commit_with_damage(content, damage_hint);

        // CPU content replaced any retained zero-copy buffer: retire the
        // GPU side (synced — GPU-done is proven, not assumed) and send the
        // deferred release *first*, preserving attach order along the
        // release chain (that buffer was attached before this one).
        if let Some(prev) = self.retained_dmabuf.take() {
            debug_assert!(
                importer.is_some(),
                "a retained dmabuf implies the embedder has an importer (its contract)"
            );
            if let Some(importer) = importer {
                importer.clear();
            }
            let event = shim_surface::events::BufferDone {
                buffer_id: prev.buffer_id,
                status: shim_surface::BufferStatus::Released,
            };
            send(&event.encode(prev.surface_id))?;
        }
        Ok(Some(shim_surface::BufferStatus::Released))
    }

    /// The `kind=dmabuf` arm of [`apply_buffer`](Self::apply_buffer): the
    /// P1.3.5 zero-copy attempt, resolved in the settled order — renderer
    /// dimension limit, import capability, the D3 allowlist (all pure, no
    /// syscall on the fd) — then the importer's own fd probe + import +
    /// probe render, mapping its typed outcome onto the wire dispositions.
    /// This is policy mapping only; every mechanic lives in
    /// [`crate::dmabuf`].
    fn apply_dmabuf<F>(
        &mut self,
        surface_id: u32,
        pending: PendingBuffer,
        importer: Option<&mut dyn DmabufImporter>,
        send: &mut F,
    ) -> Result<Option<shim_surface::BufferStatus>, ShimFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        // GPU max-texture dimension limit: the same recoverable too_large
        // as shm. MAX_SURFACE_BYTES is deliberately *not* applied — it caps
        // the copy-in allocation, and zero-copy allocates nothing
        // core-side.
        if pending.width > MAX_SURFACE_DIM || pending.height > MAX_SURFACE_DIM {
            tracing::debug!(
                width = pending.width,
                height = pending.height,
                "dmabuf exceeds renderer dimension limits: too_large"
            );
            return Ok(Some(shim_surface::BufferStatus::TooLarge));
        }
        // No import capability in this embedder (headless/pixman, CI): the
        // designed fallback — the shim re-attaches the same content as shm.
        // The fd is never touched. Previously committed content stays on
        // screen.
        let Some(importer) = importer else {
            tracing::debug!(
                buffer_id = pending.buffer_id,
                "no dmabuf import capability: directing shim to shm fallback"
            );
            return Ok(Some(shim_surface::BufferStatus::ImportFailed));
        };
        // The D3 allowlist, before any driver call and before any syscall
        // on the fd: xrgb8888/argb8888 with the implied linear modifier,
        // intersected with the renderer's cached import-format table.
        if !importer.supports(pending.format) {
            tracing::debug!(
                buffer_id = pending.buffer_id,
                format = ?pending.format,
                "dmabuf format/modifier outside the renderer's allowlist"
            );
            return Ok(Some(shim_surface::BufferStatus::FormatUnsupported));
        }

        let spec = DmabufSpec {
            format: pending.format,
            width: pending.width,
            height: pending.height,
            stride: pending.stride,
        };
        match importer.import(&spec, pending.fd) {
            // The fd lied (not a dmabuf / size below the geometry): the
            // same invalid_buffer log-and-close razor as the shm arm.
            Err(ImportDenied::FdLie(detail)) => Err(ShimViolation::InvalidBuffer(detail).into()),
            // A genuine import failure — either failure time, collapsed by
            // the importer's probe composite: the recoverable fallback
            // event. Previously committed content stays on screen.
            Err(ImportDenied::Refusal(err)) => {
                tracing::debug!(buffer_id = pending.buffer_id, %err, "directing shim to shm fallback");
                Ok(Some(shim_surface::BufferStatus::ImportFailed))
            }
            Ok(()) => {
                // Zero-copy content latched. The import's synchronous probe
                // drained the GL queue, so the previously retained buffer
                // is provably GPU-done: send its deferred release now
                // (attach order along the release chain), then retain this
                // one — its own buffer_done is deferred to replacement.
                if let Some(prev) = self.retained_dmabuf.take() {
                    let event = shim_surface::events::BufferDone {
                        buffer_id: prev.buffer_id,
                        status: shim_surface::BufferStatus::Released,
                    };
                    send(&event.encode(prev.surface_id))?;
                }
                self.retained_dmabuf = Some(RetainedDmabuf {
                    surface_id,
                    buffer_id: pending.buffer_id,
                });
                Ok(None)
            }
        }
    }
}

/// Copy `height` rows of `width * 4` bytes out of the shim's fd at the
/// declared stride, converting the wire pixel format (little-endian
/// x/argb8888: bytes B,G,R,X|A) to the scene's tightly packed RGBA8888.
/// xrgb8888's padding byte is never trusted (alpha forced opaque);
/// argb8888's alpha is carried through (composition is opaque regardless —
/// the P1.3.3 decision — so this only preserves the client's own bytes).
fn copy_in(
    file: &File,
    width: u32,
    height: u32,
    stride: u32,
    format: Format,
) -> io::Result<Vec<u8>> {
    let row_bytes = width as usize * BYTES_PER_PIXEL;
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    let mut row = vec![0u8; row_bytes];
    for y in 0..u64::from(height) {
        file.read_exact_at(&mut row, y * u64::from(stride))?;
        for px in row.chunks_exact(BYTES_PER_PIXEL) {
            let alpha = match format {
                Format::Xrgb8888 => 0xff,
                Format::Argb8888 => px[3],
            };
            rgba.extend_from_slice(&[px[2], px[1], px[0], alpha]);
        }
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsFd, OwnedFd};

    use rustix::fs::MemfdFlags;
    use vitrin_ipc::Connection;
    use vitrin_mock_shim::{frame_rgba, frame_xrgb8888, MockShim, SurfaceEvent};
    use vitrin_protocol::generated::vitrin_shim_surface::{BufferStatus, Kind};

    use super::*;
    use crate::test_pattern;

    const VIEW_W: u32 = 64;
    const VIEW_H: u32 = 48;
    const SURFACE_ID: u32 = 2;

    /// A fresh server + socketpair: core end, shim end, with `configure`
    /// already sent (the guaranteed-first message).
    fn setup() -> (ShimServer, Scene, Connection, Connection) {
        let (mut core, shim) = Connection::pair().expect("socketpair");
        let server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            width: VIEW_W,
            height: VIEW_H,
        });
        server
            .send_configure(&mut |frame| core.send_message(frame, None))
            .expect("configure");
        (server, Scene::new(), core, shim)
    }

    /// Receive and dispatch exactly `n` shim messages on the core side,
    /// with no dmabuf import capability (the headless/CI embedder shape).
    /// Returns whether any of them latched a commit.
    fn process_n(
        server: &mut ShimServer,
        scene: &mut Scene,
        core: &mut Connection,
        n: usize,
    ) -> Result<bool, ShimFault> {
        process_n_with(server, scene, core, None, n)
    }

    /// [`process_n`] with an explicit importer — the GPU-embedder shape,
    /// driven in CI through mock importers.
    fn process_n_with(
        server: &mut ShimServer,
        scene: &mut Scene,
        core: &mut Connection,
        mut importer: Option<&mut dyn DmabufImporter>,
        n: usize,
    ) -> Result<bool, ShimFault> {
        let mut committed = false;
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            // Fresh reborrow per iteration, via `match` — a closure-based
            // `as_deref_mut`/`map` would pin the borrow to the outer
            // lifetime.
            let imp: Option<&mut dyn DmabufImporter> = match &mut importer {
                Some(i) => Some(&mut **i),
                None => None,
            };
            committed |= server
                .handle_message(msg, scene, imp, &mut |frame| core.send_message(frame, None))?;
        }
        Ok(committed)
    }

    /// A memfd holding exactly `bytes`, unsealed (shim buffers carry no
    /// seal contract).
    fn memfd(bytes: &[u8]) -> OwnedFd {
        let fd = rustix::fs::memfd_create("vitrin-shim-test-buffer", MemfdFlags::CLOEXEC)
            .expect("memfd_create");
        let mut file = File::from(fd);
        file.write_all(bytes).expect("write buffer");
        OwnedFd::from(file)
    }

    /// Handcrafted attach with full control over the declared geometry —
    /// the hostile-shim tool the compliant `MockShim` deliberately lacks.
    #[allow(clippy::too_many_arguments)]
    fn send_attach(
        shim: &mut Connection,
        surface_id: u32,
        buffer_id: u32,
        fd: OwnedFd,
        kind: Kind,
        format: Format,
        width: u32,
        height: u32,
        stride: u32,
    ) {
        let attach = shim_surface::requests::Attach {
            buffer_id,
            fd,
            kind,
            format,
            width,
            height,
            stride,
        };
        shim.send_message(&attach.encode(surface_id), Some(attach.fd.as_fd()))
            .expect("send attach");
    }

    fn send_commit(shim: &mut Connection, surface_id: u32) {
        shim.send_message(&shim_surface::requests::Commit {}.encode(surface_id), None)
            .expect("send commit");
    }

    fn send_damage(shim: &mut Connection, surface_id: u32, x: i32, y: i32, w: i32, h: i32) {
        shim.send_message(
            &shim_surface::requests::Damage {
                x,
                y,
                width: w,
                height: h,
            }
            .encode(surface_id),
            None,
        )
        .expect("send damage");
    }

    fn assert_violation(result: Result<bool, ShimFault>, want: &str) {
        match result {
            Err(ShimFault::Violation(v)) => {
                let text = v.to_string();
                assert!(
                    text.contains(want),
                    "expected a `{want}` violation, got: {text}"
                );
            }
            other => panic!("expected a `{want}` violation, got: {other:?}"),
        }
    }

    /// Make a Connection's fd non-blocking so a receive probe can assert
    /// "nothing on the wire" without blocking the test.
    fn set_nonblocking(conn: &Connection) {
        let flags = rustix::fs::fcntl_getfl(conn.as_fd()).unwrap();
        rustix::fs::fcntl_setfl(conn.as_fd(), flags | rustix::fs::OFlags::NONBLOCK).unwrap();
    }

    /// Assert no complete message is waiting on `conn` (which must be
    /// non-blocking).
    fn assert_wire_silent(conn: &mut Connection) {
        match conn.recv_message() {
            Err(TransportError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("expected a silent wire, got: {other:?}"),
        }
    }

    #[test]
    fn configure_is_the_first_message_with_the_assigned_identity() {
        let _fd = crate::capture::tests::fd_lock();
        let (_server, _scene, _core, shim) = setup();
        let mut shim = shim;
        let msg = shim.recv_message().unwrap().unwrap();
        let (object_id, configure) =
            session::events::Configure::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, SHIM_SESSION_ID);
        assert_eq!(configure.realm, "realm-0");
        assert_eq!((configure.width, configure.height), (VIEW_W, VIEW_H));
    }

    #[test]
    fn frame_loop_commits_into_the_scene_and_answers_buffer_then_frame_done() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        assert_eq!((mock.width, mock.height), (VIEW_W, VIEW_H));

        // create_surface, then one full frame: attach + damage + commit.
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        let committed = process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        assert!(committed, "commit must ask the embedder to redraw");

        // Atomic latch: the scene now composes exactly frame 0.
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(0, VIEW_W, VIEW_H));

        // buffer_done(released) is prompt (the copy is done) and precedes
        // the presentation-paced frame_done.
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 1,
                status: BufferStatus::Released,
            }
        );
        assert!(server.wants_presentation());
        server
            .presented(42, &mut |frame| core.send_message(frame, None))
            .unwrap();
        assert!(!server.wants_presentation());
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::FrameDone { time_ms: 42 }
        );
    }

    #[test]
    fn pending_state_is_invisible_until_commit() {
        // The no-tearing acceptance, at the seam where it is decided: a
        // commit racing a redraw can never yield a torn mix because
        // *nothing* between attach and commit changes what compose
        // returns. Every intermediate compose equals frame N or frame N+1
        // exactly — and each mock frame differs in every pixel, so a mix
        // would equal neither.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");

        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        let frame0 = frame_rgba(0, VIEW_W, VIEW_H);
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame0);

        // Stage frame 1 (attach + damage) without committing: a redraw at
        // any interleaving point still composes frame 0, byte for byte.
        mock.attach_frame(1).unwrap();
        process_n(&mut server, &mut scene, &mut core, 1).unwrap(); // attach only
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            frame0,
            "attach staged, not applied"
        );
        process_n(&mut server, &mut scene, &mut core, 1).unwrap(); // damage
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            frame0,
            "damage staged, not applied"
        );

        // The commit latches wholesale: the very next compose is frame 1
        // exactly — never a mix.
        mock.commit().unwrap();
        let committed = process_n(&mut server, &mut scene, &mut core, 1).unwrap();
        assert!(committed);
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(1, VIEW_W, VIEW_H));
    }

    #[test]
    fn frame_done_paces_to_the_presentation_cadence() {
        // The pacing acceptance in miniature, with the presentation clock
        // under test control: the compliant shim renders frame N+1 only
        // after frame N's frame_done, so with T presentation ticks it
        // completes exactly T frames — it can never run ahead of the
        // cadence, and no frame_done exists before its tick.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        process_n(&mut server, &mut scene, &mut core, 1).unwrap(); // create_surface
        set_nonblocking(mock_conn(&mut mock));

        const TICKS: u32 = 4;
        for n in 0..TICKS {
            mock.attach_frame(n).unwrap();
            mock.commit().unwrap();
            process_n(&mut server, &mut scene, &mut core, 3).unwrap();
            // buffer_done is prompt...
            assert_eq!(
                mock.next_surface_event().unwrap(),
                SurfaceEvent::BufferDone {
                    buffer_id: n + 1,
                    status: BufferStatus::Released,
                }
            );
            // ...but no frame_done exists before the presentation tick:
            // the shim is blocked, unable to render ahead of the cadence.
            assert_wire_silent(mock_conn(&mut mock));
            assert!(server.wants_presentation());

            // Tick: present, and the owed frame_done is released.
            server
                .presented(n * 16, &mut |frame| core.send_message(frame, None))
                .unwrap();
            assert_eq!(
                mock.next_surface_event().unwrap(),
                SurfaceEvent::FrameDone { time_ms: n * 16 }
            );
            assert_eq!(
                scene.compose(VIEW_W, VIEW_H),
                frame_rgba(n, VIEW_W, VIEW_H),
                "tick {n} presents exactly frame {n}"
            );
        }
    }

    /// Reach the mock shim's transport connection for wire probes. (The
    /// fixture keeps it private by design; tests that must probe the raw
    /// wire go through this accessor helper.)
    fn mock_conn(mock: &mut MockShim) -> &mut Connection {
        mock.connection_mut()
    }

    #[test]
    fn commits_between_presentations_each_get_a_frame_done_in_order() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");

        // Two full frames committed before any presentation: both owed.
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        mock.attach_frame(1).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 7).unwrap();

        // Both buffer_dones, in attach order.
        for id in [1, 2] {
            assert_eq!(
                mock.next_surface_event().unwrap(),
                SurfaceEvent::BufferDone {
                    buffer_id: id,
                    status: BufferStatus::Released,
                }
            );
        }
        // One presentation flushes both owed frame_dones (one per commit).
        server
            .presented(7, &mut |frame| core.send_message(frame, None))
            .unwrap();
        for _ in 0..2 {
            assert_eq!(
                mock.next_surface_event().unwrap(),
                SurfaceEvent::FrameDone { time_ms: 7 }
            );
        }
        assert!(!server.wants_presentation());
        // The scene holds the last commit.
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(1, VIEW_W, VIEW_H));
    }

    #[test]
    fn repaint_commit_without_new_attach_is_legal() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        mock.next_surface_event().unwrap(); // buffer_done
        server
            .presented(1, &mut |frame| core.send_message(frame, None))
            .unwrap();
        mock.next_surface_event().unwrap(); // frame_done of the first commit

        // Damage + commit with no new attach: a repaint. No buffer_done
        // (there is no attach to answer), one owed frame_done, content
        // unchanged.
        send_damage(mock_conn(&mut mock), SURFACE_ID, 0, 0, 8, 8);
        send_commit(mock_conn(&mut mock), SURFACE_ID);
        let committed = process_n(&mut server, &mut scene, &mut core, 2).unwrap();
        assert!(committed);
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(0, VIEW_W, VIEW_H));
        server
            .presented(9, &mut |frame| core.send_message(frame, None))
            .unwrap();
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::FrameDone { time_ms: 9 }
        );
    }

    #[test]
    fn superseded_attach_is_released_promptly_and_unused() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");

        // Attach frame 0 (cookie 1), then supersede with frame 1 (cookie
        // 2) before any commit.
        mock.attach_frame(0).unwrap();
        mock.attach_frame(1).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 6).unwrap();

        // The superseded attach's ownership returns first (attach order),
        // released without ever being used; the commit then uses cookie 2.
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 1,
                status: BufferStatus::Released,
            }
        );
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 2,
                status: BufferStatus::Released,
            }
        );
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(1, VIEW_W, VIEW_H));
    }

    #[test]
    fn buffer_size_change_mid_stream_is_legal_and_letterboxed() {
        // The settled mid-stream-resize decision (module docs): each attach
        // carries its own geometry; commit replaces the size wholesale and
        // the scene letterboxes the mismatch at 1:1.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        {
            let msg = shim.recv_message().unwrap().unwrap(); // configure
            drop(msg);
        }
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();

        // First commit at the view size, second at half size.
        let full = frame_xrgb8888(0, VIEW_W, VIEW_H);
        send_attach(
            &mut shim,
            SURFACE_ID,
            1,
            memfd(&full),
            Kind::Shm,
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        );
        send_commit(&mut shim, SURFACE_ID);
        let (sw, sh) = (VIEW_W / 2, VIEW_H / 2);
        let small = frame_xrgb8888(1, sw, sh);
        send_attach(
            &mut shim,
            SURFACE_ID,
            2,
            memfd(&small),
            Kind::Shm,
            Format::Xrgb8888,
            sw,
            sh,
            sw * 4,
        );
        send_commit(&mut shim, SURFACE_ID);
        process_n(&mut server, &mut scene, &mut core, 5).unwrap();

        // The composed view letterboxes the smaller buffer, centered 1:1
        // (the P1.3.3 matte), client bytes unresampled.
        let out = scene.compose(VIEW_W, VIEW_H);
        let expected_small = frame_rgba(1, sw, sh);
        let (ox, oy) = (((VIEW_W - sw) / 2) as usize, ((VIEW_H - sh) / 2) as usize);
        for y in 0..sh as usize {
            let dst = ((oy + y) * VIEW_W as usize + ox) * BYTES_PER_PIXEL;
            let src = y * sw as usize * BYTES_PER_PIXEL;
            assert_eq!(
                out[dst..dst + sw as usize * BYTES_PER_PIXEL],
                expected_small[src..src + sw as usize * BYTES_PER_PIXEL],
                "resized client row {y} must appear unscaled"
            );
        }
        assert_eq!(
            out[..4],
            crate::scene::LETTERBOX_RGBA,
            "matte at the corner after the resize"
        );
    }

    #[test]
    fn damage_before_any_attach_is_bad_order() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        send_damage(&mut shim, SURFACE_ID, 0, 0, 1, 1);
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 2),
            "bad_order",
        );
    }

    #[test]
    fn commit_before_any_attach_is_bad_order() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        send_commit(&mut shim, SURFACE_ID);
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 2),
            "bad_order",
        );
    }

    #[test]
    fn invalid_geometry_is_invalid_buffer() {
        let _fd = crate::capture::tests::fd_lock();
        for (w, h, stride, bytes) in [
            // Zero dimensions.
            (0u32, 4u32, 16u32, 64usize),
            (4, 0, 16, 64),
            // Stride below width * 4.
            (4, 4, 15, 64),
            // fd smaller than stride * height.
            (4, 4, 16, 63),
        ] {
            let (mut server, mut scene, mut core, mut shim) = setup();
            shim.recv_message().unwrap().unwrap(); // configure
            shim.send_message(
                &session::requests::CreateSurface {
                    surface: SURFACE_ID,
                }
                .encode(SHIM_SESSION_ID),
                None,
            )
            .unwrap();
            send_attach(
                &mut shim,
                SURFACE_ID,
                1,
                memfd(&vec![0u8; bytes]),
                Kind::Shm,
                Format::Xrgb8888,
                w,
                h,
                stride,
            );
            assert_violation(
                process_n(&mut server, &mut scene, &mut core, 2),
                "invalid_buffer",
            );
        }
    }

    #[test]
    fn huge_geometry_cannot_wrap_the_size_check() {
        // u32::MAX x u32::MAX at stride u32::MAX: stride * height overflows
        // 64-bit math; the 128-bit check must reject it against any real fd
        // size rather than wrapping to a small number.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        send_attach(
            &mut shim,
            SURFACE_ID,
            1,
            memfd(&[0u8; 64]),
            Kind::Shm,
            Format::Xrgb8888,
            u32::MAX,
            u32::MAX,
            u32::MAX,
        );
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 2),
            "invalid_buffer",
        );
    }

    #[test]
    fn sparse_giant_buffer_is_recoverable_too_large() {
        // A sparse memfd can honestly *have* a giant size (ftruncate
        // allocates nothing), so the fd-size check passes — the renderer
        // limit then takes the recoverable buffer_done(too_large) path
        // instead of allocating gigabytes inside the TCB. Previously
        // committed content survives.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        mock.next_surface_event().unwrap(); // buffer_done frame 0

        // 32768 x 32768 x 4 = 4 GiB declared, honest sparse size.
        let (gw, gh) = (MAX_SURFACE_DIM * 2, MAX_SURFACE_DIM * 2);
        let fd = rustix::fs::memfd_create("vitrin-shim-test-sparse", MemfdFlags::CLOEXEC).unwrap();
        rustix::fs::ftruncate(&fd, u64::from(gw) * 4 * u64::from(gh)).unwrap();
        send_attach(
            mock_conn(&mut mock),
            SURFACE_ID,
            99,
            fd,
            Kind::Shm,
            Format::Xrgb8888,
            gw,
            gh,
            gw * 4,
        );
        send_commit(mock_conn(&mut mock), SURFACE_ID);
        let committed = process_n(&mut server, &mut scene, &mut core, 2).unwrap();
        assert!(committed, "a failed-buffer commit still presents");

        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 99,
                status: BufferStatus::TooLarge,
            }
        );
        // Previous content stays; the pacing loop is not stalled.
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(0, VIEW_W, VIEW_H));
        server
            .presented(3, &mut |frame| core.send_message(frame, None))
            .unwrap();
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::FrameDone { time_ms: 3 }
        );
    }

    #[test]
    fn shrunk_memfd_after_attach_fails_commit_as_invalid_buffer() {
        // Shim buffers are unsealed, so the fd can lie *after* the attach
        // validation. The pread copy-in turns that into a clean
        // invalid_buffer kill — never a SIGBUS in the TCB (which is why
        // commit never maps the fd).
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        let pixels = frame_xrgb8888(0, VIEW_W, VIEW_H);
        let fd = memfd(&pixels);
        let shrink_handle = fd.try_clone().unwrap();
        send_attach(
            &mut shim,
            SURFACE_ID,
            1,
            fd,
            Kind::Shm,
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        );
        // Attach validates fine against the honest size...
        process_n(&mut server, &mut scene, &mut core, 2).unwrap();
        // ...then the shim shrinks the file out from under the core.
        rustix::fs::ftruncate(&shrink_handle, 16).unwrap();
        send_commit(&mut shim, SURFACE_ID);
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 1),
            "invalid_buffer",
        );
        // The scene never saw a partial frame.
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            test_pattern::render(VIEW_W, VIEW_H)
        );
    }

    #[test]
    fn dmabuf_commit_directs_shm_fallback_and_still_paces() {
        // The no-importer embedder (headless/pixman — the CI path): a
        // dmabuf commit resolves as the IDL's recoverable fallback
        // (buffer_done(import_failed)) with the fd never touched, previous
        // content survives, and the commit still earns its frame_done —
        // flow g-prime of the prose page.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        mock.next_surface_event().unwrap(); // buffer_done frame 0

        // "dmabuf" attach (any fd: the core must not touch it) + commit.
        send_attach(
            mock_conn(&mut mock),
            SURFACE_ID,
            50,
            memfd(&[0u8; 4]),
            Kind::Dmabuf,
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        );
        send_commit(mock_conn(&mut mock), SURFACE_ID);
        let committed = process_n(&mut server, &mut scene, &mut core, 2).unwrap();
        assert!(committed);

        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 50,
                status: BufferStatus::ImportFailed,
            }
        );
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            frame_rgba(0, VIEW_W, VIEW_H),
            "previously committed content remains on screen"
        );
        server
            .presented(5, &mut |frame| core.send_message(frame, None))
            .unwrap();
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::FrameDone { time_ms: 5 },
            "a rejected buffer must not stall the pacing loop"
        );

        // The shm fallback then lands normally.
        mock.attach_frame(1).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 3).unwrap();
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(1, VIEW_W, VIEW_H));
    }

    #[test]
    fn dmabuf_reattach_before_buffer_done_is_bad_order() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        send_attach(
            &mut shim,
            SURFACE_ID,
            7,
            memfd(&[0u8; 4]),
            Kind::Dmabuf,
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        );
        send_attach(
            &mut shim,
            SURFACE_ID,
            7,
            memfd(&[0u8; 4]),
            Kind::Dmabuf,
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        );
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 3),
            "bad_order",
        );
    }

    /// A scriptable [`DmabufImporter`] — the CI stand-in for the GLES
    /// importer, letting every policy mapping in `apply_dmabuf` be driven
    /// without a GPU. It also counts calls, so tests can prove ordering
    /// claims ("the allowlist is checked *before* any import").
    struct MockImporter {
        supported: Vec<Format>,
        /// Scripted outcomes for successive `import` calls.
        outcomes: std::collections::VecDeque<Result<(), ImportDenied>>,
        import_calls: usize,
        /// Mirrors the renderer-side retained-content slot.
        holds_content: bool,
        clears: usize,
    }

    impl MockImporter {
        fn new(
            supported: &[Format],
            outcomes: impl IntoIterator<Item = Result<(), ImportDenied>>,
        ) -> Self {
            Self {
                supported: supported.to_vec(),
                outcomes: outcomes.into_iter().collect(),
                import_calls: 0,
                holds_content: false,
                clears: 0,
            }
        }
    }

    impl DmabufImporter for MockImporter {
        fn supports(&self, format: Format) -> bool {
            self.supported.contains(&format)
        }

        fn import(
            &mut self,
            _spec: &DmabufSpec,
            _fd: std::os::fd::OwnedFd,
        ) -> Result<(), ImportDenied> {
            self.import_calls += 1;
            let outcome = self
                .outcomes
                .pop_front()
                .expect("import called more often than the test scripted");
            if outcome.is_ok() {
                self.holds_content = true;
            }
            outcome
        }

        fn clear(&mut self) {
            self.clears += 1;
            self.holds_content = false;
        }
    }

    fn refusal() -> ImportDenied {
        ImportDenied::Refusal(crate::dmabuf::ImportError {
            stage: crate::dmabuf::ImportStage::FirstRender,
            detail: "scripted failure".into(),
        })
    }

    #[test]
    fn dmabuf_allowlist_miss_is_format_unsupported_before_any_import() {
        // The D3 allowlist is enforced before any driver call: a format the
        // renderer's table does not carry resolves as the recoverable
        // format_unsupported event with the importer's import path — and
        // therefore the fd — never touched. The connection survives and
        // the shm path keeps working.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut importer = MockImporter::new(&[Format::Xrgb8888], []);
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 1).unwrap();

        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Argb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();

        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 1,
                status: BufferStatus::FormatUnsupported,
            }
        );
        assert_eq!(
            importer.import_calls, 0,
            "an allowlist miss must never reach the import path"
        );
        server
            .presented(1, &mut |frame| core.send_message(frame, None))
            .unwrap();
        mock.next_surface_event().unwrap(); // frame_done

        // The directed shm fallback lands normally afterwards.
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(0, VIEW_W, VIEW_H));
    }

    #[test]
    fn dmabuf_import_refusal_falls_back_to_shm_end_to_end() {
        // The R3 acceptance in CI: an import failure (here scripted at the
        // first-render stage — the same mapping covers EGL-time rejection)
        // produces the explicit fallback event, previously committed
        // content stays on screen, pacing continues, and the surface then
        // keeps updating via shm.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut importer =
            MockImporter::new(&[Format::Xrgb8888, Format::Argb8888], [Err(refusal())]);

        // Frame 0 over shm first, so "previous content" is real content.
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 4).unwrap();
        mock.next_surface_event().unwrap(); // buffer_done frame 0
        server
            .presented(1, &mut |frame| core.send_message(frame, None))
            .unwrap();
        mock.next_surface_event().unwrap(); // frame_done

        // The failing dmabuf commit: explicit fallback event, no kill.
        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        let committed =
            process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert!(committed, "a failed-buffer commit still presents");
        assert_eq!(importer.import_calls, 1);
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 2,
                status: BufferStatus::ImportFailed,
            }
        );
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            frame_rgba(0, VIEW_W, VIEW_H),
            "previously committed content remains on screen"
        );
        server
            .presented(2, &mut |frame| core.send_message(frame, None))
            .unwrap();
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::FrameDone { time_ms: 2 },
            "a rejected buffer must not stall the pacing loop"
        );

        // The surface keeps updating via shm afterwards (the acceptance's
        // second half), and the copy meter shows the copy-in regime.
        mock.attach_frame(1).unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(1, VIEW_W, VIEW_H));
        assert_eq!(
            server.copy_meter().copies(),
            2,
            "exactly the two shm commits copied pixels; the dmabuf commit copied none"
        );
    }

    #[test]
    fn dmabuf_fd_lie_reported_by_the_importer_is_invalid_buffer() {
        // The importer's fd probe found a lie (not a dmabuf / size below
        // the geometry): the same invalid_buffer log-and-close razor as the
        // shm arm — connection-fatal, no fallback event.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut importer = MockImporter::new(
            &[Format::Xrgb8888],
            [Err(ImportDenied::FdLie("scripted fd lie".into()))],
        );
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 1).unwrap();
        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        assert_violation(
            process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3),
            "invalid_buffer",
        );
    }

    #[test]
    fn successful_dmabuf_import_defers_release_until_replacement() {
        // The zero-copy release semantics (module docs), end to end over
        // the wire: a successfully imported buffer earns no buffer_done at
        // its own commit — the GPU keeps sampling it — and is released
        // exactly when the next successful commit replaces it; a shm
        // commit retires the retained buffer (importer.clear, synced)
        // before its own prompt release, preserving attach order along the
        // release chain. The CPU scene is never touched by zero-copy
        // content.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut importer = MockImporter::new(&[Format::Xrgb8888], [Ok(()), Ok(())]);
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 1).unwrap();
        set_nonblocking(mock_conn(&mut mock));

        // Commit A (cookie 1): imported, retained, unanswered.
        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert!(importer.holds_content);
        assert_wire_silent(mock_conn(&mut mock)); // no buffer_done: deferred
        server
            .presented(1, &mut |frame| core.send_message(frame, None))
            .unwrap();
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::FrameDone { time_ms: 1 },
            "pacing is independent of the deferred buffer disposition"
        );
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            test_pattern::render(VIEW_W, VIEW_H),
            "zero-copy content never enters the CPU scene"
        );

        // Commit B (cookie 2): replaces A — A's released arrives now.
        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 1,
                status: BufferStatus::Released,
            },
            "the replaced buffer is released exactly at replacement"
        );
        assert_wire_silent(mock_conn(&mut mock)); // B itself: deferred
        server
            .presented(2, &mut |frame| core.send_message(frame, None))
            .unwrap();
        mock.next_surface_event().unwrap(); // frame_done

        // Commit C over shm (cookie 3): retires B (clear + sync) and
        // releases it BEFORE C's own prompt release — attach order.
        mock.attach_frame(5).unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert_eq!(importer.clears, 1, "shm content must retire the GPU side");
        assert!(!importer.holds_content);
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 2,
                status: BufferStatus::Released,
            },
            "the retained buffer's release precedes the shm buffer's (attach order)"
        );
        assert_eq!(
            mock.next_surface_event().unwrap(),
            SurfaceEvent::BufferDone {
                buffer_id: 3,
                status: BufferStatus::Released,
            }
        );
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(5, VIEW_W, VIEW_H));
        assert_eq!(
            server.copy_meter().copies(),
            1,
            "only the shm commit copied client pixels"
        );
    }

    #[test]
    fn reattach_of_retained_dmabuf_cookie_is_bad_order() {
        // The deferred-release consequence the IDL already legislates: the
        // retained cookie's ownership has not returned, so re-attaching it
        // — with either kind — is the bad_order log-and-close.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut importer = MockImporter::new(&[Format::Xrgb8888], [Ok(())]);
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 1).unwrap();

        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();

        // Cookie 1 is retained; a hostile re-attach names it again.
        send_attach(
            mock_conn(&mut mock),
            SURFACE_ID,
            1,
            memfd(&[0u8; 4]),
            Kind::Dmabuf,
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        );
        assert_violation(
            process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 1),
            "bad_order",
        );
    }

    #[test]
    fn teardown_with_retained_dmabuf_clears_the_importer() {
        // Shim death while a zero-copy buffer is retained: no released
        // event exists to send (the wire is gone), but the importer must
        // drop its texture — and with it the dmabuf fd — on the same
        // teardown path that clears the scene.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut importer = MockImporter::new(&[Format::Xrgb8888], [Ok(())]);
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 1).unwrap();
        mock.attach_dmabuf(
            memfd(&[0u8; 4]),
            Format::Xrgb8888,
            VIEW_W,
            VIEW_H,
            VIEW_W * 4,
        )
        .unwrap();
        mock.commit().unwrap();
        process_n_with(&mut server, &mut scene, &mut core, Some(&mut importer), 3).unwrap();
        assert!(importer.holds_content);

        let mut router = crate::input::InputRouter::detached(crate::input::NoopHook);
        server.connection_closed(
            &crate::grants::RealmId::new("realm-0"),
            &mut scene,
            Some(&mut importer),
            &mut router,
        );
        assert_eq!(importer.clears, 1);
        assert!(
            !importer.holds_content,
            "teardown must drop the GPU content"
        );
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            test_pattern::render(VIEW_W, VIEW_H)
        );
    }

    #[test]
    fn second_get_seat_is_already_initialized() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        for seat in [2u32, 3u32] {
            shim.send_message(
                &session::requests::GetSeat { seat }.encode(SHIM_SESSION_ID),
                None,
            )
            .unwrap();
        }
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 2),
            "already_initialized",
        );
    }

    #[test]
    fn watermark_violations_are_invalid_object() {
        let _fd = crate::capture::tests::fd_lock();
        // Reuse of the bootstrap id, a non-increasing id, and the reserved
        // server range each kill the connection.
        for bad_id in [SHIM_SESSION_ID, 0u32, 0xff00_0000u32] {
            let (mut server, mut scene, mut core, mut shim) = setup();
            shim.recv_message().unwrap().unwrap(); // configure
            shim.send_message(
                &session::requests::CreateSurface { surface: bad_id }.encode(SHIM_SESSION_ID),
                None,
            )
            .unwrap();
            assert_violation(
                process_n(&mut server, &mut scene, &mut core, 1),
                "invalid_object",
            );
        }
        // Non-increasing relative to an earlier mint.
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        for id in [5u32, 3u32] {
            shim.send_message(
                &session::requests::CreateSurface { surface: id }.encode(SHIM_SESSION_ID),
                None,
            )
            .unwrap();
        }
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 2),
            "invalid_object",
        );
    }

    #[test]
    fn surface_flood_is_resource_exhausted() {
        // The conventions' per-connection live-object cap: version 1 has no
        // destructors, so a shim streaming create_surface pins core memory
        // (and potentially one staged fd) per surface forever. The mint
        // past the cap is the fatal resource_exhausted confinement — the
        // core's state stays O(1), not O(messages).
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        for i in 0..=MAX_LIVE_SURFACES as u32 {
            shim.send_message(
                &session::requests::CreateSurface { surface: 2 + i }.encode(SHIM_SESSION_ID),
                None,
            )
            .unwrap();
        }
        // The first MAX_LIVE_SURFACES mints are legal...
        process_n(&mut server, &mut scene, &mut core, MAX_LIVE_SURFACES).unwrap();
        assert_eq!(server.surfaces.len(), MAX_LIVE_SURFACES);
        // ...the one past the cap kills the connection.
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 1),
            "resource_exhausted",
        );
    }

    #[test]
    fn non_shmem_shm_fd_is_rejected_at_attach() {
        // A kind=shm fd that is not memfd/shmem-backed must die at attach,
        // before any other syscall trusts it: reads (or even fstat) on a
        // hostile fd — e.g. a file on a shim-controlled FUSE mount — can
        // block, wedging the single-threaded core loop with no funnel able
        // to fire. /dev/null stands in as a deterministic non-shmem fd
        // (F_GET_SEALS fails on it as it does on any non-shmem file).
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        let hostile = OwnedFd::from(File::open("/dev/null").unwrap());
        send_attach(
            &mut shim,
            SURFACE_ID,
            1,
            hostile,
            Kind::Shm,
            Format::Xrgb8888,
            1,
            1,
            4,
        );
        // The distinctive probe message, not the fd-size check: the seals
        // probe must be what rejects the fd (it runs first, because fstat
        // itself is unsafe on a hostile fd).
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 2),
            "not memfd/shmem-backed",
        );
    }

    #[test]
    fn unknown_object_and_unknown_opcode_are_fatal() {
        let _fd = crate::capture::tests::fd_lock();
        // A request toward an id nobody minted.
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        send_commit(&mut shim, 1234);
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 1),
            "invalid_object",
        );

        // An opcode the session does not define.
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        let mut frame = session::requests::GetSeat { seat: 2 }.encode(SHIM_SESSION_ID);
        frame[6] = 9; // opcode byte of the 8-byte header
        shim.send_message(&frame, None).unwrap();
        assert_violation(
            process_n(&mut server, &mut scene, &mut core, 1),
            "unknown opcode",
        );
    }

    #[test]
    fn damage_flood_stays_bounded() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.attach_frame(0).unwrap();
        process_n(&mut server, &mut scene, &mut core, 3).unwrap();

        // A shim streaming damage forever without committing must pin O(1)
        // memory, not O(messages): rectangles past the cap fold into a
        // bounding box.
        for i in 0..(MAX_DAMAGE_RECTS as i32 * 4) {
            send_damage(mock_conn(&mut mock), SURFACE_ID, i, i, 1, 1);
            process_n(&mut server, &mut scene, &mut core, 1).unwrap();
        }
        let state = server.surfaces.get(&SURFACE_ID).unwrap();
        assert_eq!(state.pending_damage.len(), MAX_DAMAGE_RECTS);
    }

    #[test]
    fn argb_buffers_copy_in_with_client_bytes_intact() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        // One argb8888 pixel with non-opaque alpha: wire bytes B,G,R,A.
        send_attach(
            &mut shim,
            SURFACE_ID,
            1,
            memfd(&[0x30, 0x20, 0x10, 0x00]),
            Kind::Shm,
            Format::Argb8888,
            1,
            1,
            4,
        );
        send_commit(&mut shim, SURFACE_ID);
        process_n(&mut server, &mut scene, &mut core, 3).unwrap();
        // Composition is opaque (P1.3.3): the client's color bytes appear
        // exactly, alpha forced 0xFF at compose.
        assert_eq!(scene.compose(1, 1), [0x10, 0x20, 0x30, 0xff]);
    }

    #[test]
    fn stride_padded_rows_copy_only_the_validated_bounds() {
        // stride > width * 4: the copy must read exactly width * 4 bytes
        // per row at the declared stride, never the padding.
        let _fd = crate::capture::tests::fd_lock();
        let (mut server, mut scene, mut core, mut shim) = setup();
        shim.recv_message().unwrap().unwrap(); // configure
        shim.send_message(
            &session::requests::CreateSurface {
                surface: SURFACE_ID,
            }
            .encode(SHIM_SESSION_ID),
            None,
        )
        .unwrap();
        let (w, h, stride) = (2u32, 2u32, 16u32); // 8 payload + 8 padding per row
        let rgba = frame_rgba(4, w, h);
        let tight = vitrin_mock_shim::rgba_to_xrgb8888(&rgba);
        let mut padded = Vec::new();
        for row in tight.chunks_exact(w as usize * 4) {
            padded.extend_from_slice(row);
            padded.extend_from_slice(&[0xde; 8]); // padding the core must ignore
        }
        send_attach(
            &mut shim,
            SURFACE_ID,
            1,
            memfd(&padded),
            Kind::Shm,
            Format::Xrgb8888,
            w,
            h,
            stride,
        );
        send_commit(&mut shim, SURFACE_ID);
        process_n(&mut server, &mut scene, &mut core, 3).unwrap();
        assert_eq!(scene.compose(w, h), rgba);
    }

    #[test]
    fn connection_teardown_clears_the_scene_and_leaks_no_fd() {
        // Process-isolated, not merely `fd_lock`-serialized: the measured
        // quantity is the process-global open-fd count, which a
        // *non-measuring* test on the harness pool perturbs whether or not
        // it holds the lock (`capture::tests::run_fd_isolated`'s docs, the
        // #74/#80 race). This test used to read `/proc/self/fd` directly,
        // which is exactly the pattern `open_fd_count`'s debug_assert
        // exists to forbid -- and reading it directly is how it evaded
        // that guard. It went red the moment an unrelated test was added
        // and the thread schedule shifted.
        crate::capture::tests::run_fd_isolated(
            "shim::tests::connection_teardown_clears_the_scene_and_leaks_no_fd",
            connection_teardown_clears_the_scene_and_leaks_no_fd_body,
        );
    }

    fn connection_teardown_clears_the_scene_and_leaks_no_fd_body() {
        let baseline = crate::capture::tests::open_fd_count();
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        // Stage a pending attach the server still holds the fd of.
        mock.attach_frame(1).unwrap();
        process_n(&mut server, &mut scene, &mut core, 2).unwrap();
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(0, VIEW_W, VIEW_H));

        // Stage an implicit grab in the realm's router (emulated is the
        // origin a test outside `crate::input` can mint; grab mechanics
        // are origin-blind) so the teardown funnel's reset is observable.
        use crate::input::{InputRouter, NoopHook, SeatInput, SeatInputKind};
        use vitrin_protocol::generated::vitrin_actuator_pointer::ButtonState;
        let realm = crate::grants::RealmId::new("realm-0");
        let mut router = InputRouter::detached(NoopHook);
        // The router's seat state is per realm and per shim generation, so
        // every route below names the realm, exactly as `session::route_seat`
        // does -- or the scoped reset has nothing of this realm's to forget.
        let view = (VIEW_W, VIEW_H);
        let surface = Some((VIEW_W, VIEW_H));
        let button = |state| SeatInputKind::Button {
            button: 0x110,
            state,
        };
        assert!(router
            .route_emulated(
                &realm,
                SeatInput::emulated(SeatInputKind::Motion { x: 1.0, y: 1.0 }),
                view,
                surface,
            )
            .is_some());
        assert!(router
            .route_emulated(
                &realm,
                SeatInput::emulated(button(ButtonState::Pressed)),
                view,
                surface
            )
            .is_some());

        // Shim death: the embedder tears the server down — one funnel for
        // all realm state (scene, buffer fds, and this realm's seat/router
        // state).
        server.connection_closed(&realm, &mut scene, None, &mut router);
        // No grab survived the generation: the stale release is unpaired
        // at the next shim's seat and dropped.
        assert!(router
            .route_emulated(
                &realm,
                SeatInput::emulated(button(ButtonState::Released)),
                view,
                surface
            )
            .is_none());
        drop(mock);
        drop(core);
        // Never a stale frame: the scene falls back to the background.
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            test_pattern::render(VIEW_W, VIEW_H)
        );
        // Every held buffer fd (including the staged pending one) closed.
        assert_eq!(
            crate::capture::tests::open_fd_count(),
            baseline,
            "no fd may leak on shim teardown"
        );
    }

    /// Cross-track conformance: the **real C shim** (E6, P1.6.2) driven by
    /// the **real core**, over a socketpair the real spawn manager placed.
    ///
    /// Everything else in this module tests the core against
    /// `vitrin-mock-shim` -- a compliant peer written by the same hand, in
    /// the same language, from the same understanding of the spec. That is
    /// exactly the shape of test that agrees with itself: if both sides
    /// misread the IDL identically, it passes. This test is the one that
    /// cannot, because the peer is a separately built C program that
    /// consumes only the generated wire header and the prose.
    ///
    /// It is opt-in: `shim/` is outside the Cargo workspace (Meson, C), so
    /// `cargo test` cannot build the binary and must not fail for its
    /// absence. Point `VITRIN_C_SHIM_BIN` at it to run the check:
    ///
    /// ```text
    /// meson setup shim/build shim && meson compile -C shim/build
    /// VITRIN_C_SHIM_BIN=$PWD/shim/build/vitrin-shim cargo test -p vitrin-core c_shim
    /// ```
    ///
    /// Opt-in, but NOT silently so under CI. A test that skips itself
    /// reports green while proving nothing, and that is not a hypothetical
    /// here: this check shipped wired to no job at all, passing on every
    /// run without ever spawning a shim. So CI must say which it means --
    /// point `VITRIN_C_SHIM_BIN` at a built shim (the `conformance` job) or
    /// declare the skip with `VITRIN_C_SHIM_CONFORMANCE_SKIP` (the `rust`
    /// job, which has no C toolchain). A job that does neither fails here,
    /// which is the only way an unwired cross-track check cannot masquerade
    /// as a passing one.
    ///
    /// What passing proves, none of which the C-side harness can establish
    /// on its own: the C shim's frames survive THIS server's validation --
    /// the `F_GET_SEALS` shmem probe, the 128-bit geometry-versus-fd-size
    /// arithmetic, the watermark rule on its two minted ids, the
    /// `bad_order` razor, and the copy-in that actually reads its memfd.
    #[test]
    fn c_shim_conforms_to_the_real_core() {
        let Some(shim_bin) = std::env::var_os("VITRIN_C_SHIM_BIN") else {
            assert!(
                std::env::var_os("CI").is_none()
                    || std::env::var_os("VITRIN_C_SHIM_CONFORMANCE_SKIP").is_some(),
                "VITRIN_C_SHIM_BIN is unset in CI, so the C shim was never built and this \
                 cross-track check proved nothing. Build the shim and point the variable at \
                 it (see the `conformance` job in .github/workflows/ci.yml), or set \
                 VITRIN_C_SHIM_CONFORMANCE_SKIP=1 in a job that cannot build C."
            );
            eprintln!("skipping: set VITRIN_C_SHIM_BIN to the built shim/build/vitrin-shim");
            return;
        };
        let shim_bin = std::path::PathBuf::from(shim_bin);
        assert!(
            shim_bin.is_file(),
            "VITRIN_C_SHIM_BIN does not name a file: {}",
            shim_bin.display()
        );

        let _fd = crate::capture::tests::fd_lock();
        let base = crate::spawn::tests::scratch();
        // The C shim is the core-inserted `--shim` binary under test (issue
        // #103), the realm's fd-3 peer -- so it is the shim *program*, not the
        // app. Since #104 the shim actually forks/execs the app command the
        // core conveys after `--`, so the app can no longer be the shim itself
        // (a second vitrin-shim, execed with no fd 3, would refuse to start and
        // its immediate exit would race the shim's commit down via SIGCHLD).
        // The app is instead a trivial `sleep`: it never connects to the shim's
        // Wayland socket, so the shim still composites and commits its EMPTY
        // realm view -- the frame this test validates -- while the app stays
        // alive until teardown reaps it. The duration is a few minutes, not a
        // day: it comfortably outlives DEADLINE (10s) so the app is always live
        // during the test, but if the watchdog below has to SIGKILL a wedged
        // shim -- bypassing its reap -- the orphaned `sleep` clears in minutes
        // rather than lingering for 24h on a developer's machine.
        let paths = crate::spawn::SpawnPaths::under(&base, &shim_bin);
        let app_bin = ["/usr/bin/sleep", "/bin/sleep"]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.is_file())
            .expect("the conformance app needs a `sleep` at /usr/bin or /bin");
        let (mut recorder, _log) = crate::recorder::tests::scratch_recorder("c-shim-conformance");

        // The shim must run headless on the software renderer: this test is
        // the CI path, and CI has no GPU. Those names are passed through the
        // spawn manager's own allowlist rather than set on the child
        // directly, because the allowlist is the only route a realm's app
        // environment is allowed to grow by.
        let env_allow: Vec<String> = [
            "WLR_BACKENDS",
            "WLR_RENDERER",
            "WLR_RENDERER_ALLOW_SOFTWARE",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let realm = crate::realm::tests::realm_with_spawn(
            "realm-0",
            &app_bin,
            &["300".to_string()],
            &env_allow,
        );
        let mut spawned = crate::spawn::spawn_realm_with_env(
            &realm,
            &paths,
            &mut recorder,
            crate::spawn::SpawnOrigin::Startup,
            |name| match name {
                "WLR_BACKENDS" => Some("headless".into()),
                "WLR_RENDERER" => Some("pixman".into()),
                "WLR_RENDERER_ALLOW_SOFTWARE" => Some("1".into()),
                _ => None,
            },
        )
        .expect("the C shim must spawn");

        let mut server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            width: VIEW_W,
            height: VIEW_H,
        });
        let mut scene = Scene::new();
        server
            .send_configure(&mut |frame| spawned.connection_mut().send_message(frame, None))
            .expect("configure is the core's first message");

        // A watchdog, because the receive below is blocking: a shim that
        // says nothing must fail this test as a closed connection, not hang
        // the suite forever. Killing it produces EOF, which the loop reports.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog = {
            let done = std::sync::Arc::clone(&done);
            let pid = spawned.pid();
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + crate::spawn::tests::DEADLINE;
                while std::time::Instant::now() < deadline {
                    if done.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            })
        };

        // Drive the server until the shim's first commit latches. With no
        // app connected the shim still composites its (empty) realm view,
        // which is the frame under test: what matters here is that the
        // wire, the fd and the geometry are the ones this server accepts.
        let mut committed = false;
        let mut saw_surface_mint = false;
        while !committed {
            let Some(msg) = spawned
                .connection_mut()
                .recv_message()
                .expect("the C shim's framing must be readable")
            else {
                panic!("the C shim closed the connection before committing");
            };
            if msg.header.object_id == SHIM_SESSION_ID
                && msg.header.opcode == session::requests::CreateSurface::OPCODE
            {
                saw_surface_mint = true;
            }
            // The real dispatch, with the real validation. A violation --
            // a stride lie, a non-shmem fd, an id at or below the watermark,
            // a commit before any attach -- surfaces here as a ShimFault,
            // which is precisely the failure this test exists to catch.
            let conn = spawned.connection_mut();
            committed = server
                .handle_message(msg, &mut scene, None, &mut |frame| {
                    conn.send_message(frame, None)
                })
                .expect("the C shim must not violate the shim protocol");
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        watchdog.join().expect("watchdog thread");
        assert!(saw_surface_mint, "the shim must mint its surface");

        // The commit really reached the scene: the realm view now has the
        // shim's composed content at the geometry the core configured.
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H).len(),
            VIEW_W as usize * VIEW_H as usize * BYTES_PER_PIXEL,
            "the committed frame must compose at the configured realm-view size"
        );
        assert!(
            server.wants_presentation(),
            "a commit owes exactly one frame_done"
        );
        server
            .presented(1234, &mut |frame| {
                spawned.connection_mut().send_message(frame, None)
            })
            .expect("frame_done goes out");

        // Orderly teardown: SIGTERM the shim rather than SIGKILL it, so it runs
        // its own teardown and reaps the `sleep` app it forked (#104) -- a
        // SIGKILL would leave that app orphaned for its full duration. This
        // also exercises, from the real core's side, that killing the shim
        // takes its app down with it (P1.5.2).
        if let Some(pid) = rustix::process::Pid::from_raw(spawned.pid() as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        let _ = spawned.child_mut().wait();
        let _ = std::fs::remove_dir_all(&base);
    }
}
