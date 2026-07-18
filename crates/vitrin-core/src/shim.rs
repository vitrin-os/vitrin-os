//! The shim-facing protocol server (P1.3.4, issue #21): the core side of the
//! shim boundary — half of artifact A5's contract.
//!
//! [`ShimServer`] implements the server side of `vitrin_shim_session` and
//! `vitrin_shim_surface` exactly as `protocol/vitrin-v0.xml` pins them: the
//! `configure`-first bring-up, surface/seat minting under the watermark rule,
//! and the attach → damage → commit → `buffer_done`/`frame_done` frame loop.
//! Buffer path v0 is **shm/memfd copy-in** (plan decision D3): at commit the
//! core copies the attached buffer's pixels out of the shim's fd into
//! [`Scene::commit`] (the P1.3.3 seam) and owns its bytes from then on;
//! dmabuf import is the M1.5 optimization (P1.3.5) and, until it lands, a
//! `kind=dmabuf` commit takes the protocol's designed fallback path
//! (`buffer_done(import_failed)` — the shim re-attaches as shm).
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
//!   breaches, unknown opcodes, malformed payloads) — surface as
//!   [`ShimViolation`]. Shim connections carry **no fatal-error event** (the
//!   IDL: "a shim that violates the protocol is logged and its connection
//!   closed"), so the embedder logs the violation and closes the connection,
//!   with no goodbye on the wire — the same conclusion the transport layer
//!   reached for its own violations.
//!
//! Defensive validation of an attached memfd (all before any byte is
//! trusted): nonzero dimensions, `stride >= width * 4`, and fd size at
//! least `stride * height` — all in 128-bit math, so wire-derived `u32`
//! geometry can never wrap a check (the [`scene`](crate::scene) overflow
//! precedent). The copy itself uses `pread` bounded by the validated
//! geometry — never `mmap` — so a shim that shrinks its (deliberately
//! unsealed: version 1 demands no seals on shim buffers) memfd after attach
//! produces a clean short-read error at commit (`invalid_buffer`,
//! log-and-close), not a `SIGBUS` in the TCB. A buffer too large for the
//! renderer's limits ([`MAX_SURFACE_DIM`]/[`MAX_SURFACE_BYTES`] — also the
//! defense against sparse-memfd allocation bombs) takes the *recoverable*
//! `buffer_done(too_large)` path the IDL defines, not a kill. Both
//! version-1 formats (xrgb8888/argb8888) are supported by the copy-in
//! converter, so `format_unsupported` is unreachable here until a format
//! appends.
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
//! # Wiring (today: the test harness; at runtime: P1.5.2)
//!
//! Like [`crate::capture`], this module ships the protocol machine, not the
//! live wiring: nothing at runtime hands `vitrind` a shim connection until
//! the realm spawn manager (P1.5.2) inherits the socketpair at fork. The
//! calloop wiring pattern is exercised end-to-end by the tests in
//! [`crate::backend::headless`]: a [`ConnectionSource`]
//! (vitrin_ipc::ConnectionSource) dispatch calls [`ShimServer::handle_message`],
//! redraws on commit, then calls [`ShimServer::presented`] — with a real
//! mock shim (`vitrin-mock-shim`, the permanent fixture) on the other end
//! of the socketpair.

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

use crate::scene::{Scene, SurfaceContent, BYTES_PER_PIXEL};

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

/// One pending damage rectangle, verbatim off the wire (clamping is the
/// composition side's business; out-of-bounds is a non-error per the IDL).
#[derive(Debug, Clone, Copy)]
struct DamageRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl DamageRect {
    /// Fold `other` into `self` as a bounding box (saturating: damage is a
    /// hint, over-approximation is always legal).
    fn union(&mut self, other: DamageRect) {
        let x2 = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let y2 = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        self.x = self.x.min(other.x);
        self.y = self.y.min(other.y);
        self.width = x2.saturating_sub(self.x);
        self.height = y2.saturating_sub(self.y);
    }
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
    /// The single seat object, once minted (`get_seat`); events toward it
    /// are P1.6.3's, but the mint and its `already_initialized` rule are
    /// session protocol served here.
    seat_id: Option<u32>,
    /// Surface ids of commits whose `frame_done` is still owed, in commit
    /// order (FIFO-correlated per the IDL). Drained by [`presented`].
    ///
    /// [`presented`]: Self::presented
    owed_frame_dones: VecDeque<u32>,
}

impl ShimServer {
    pub fn new(config: ShimConfig) -> Self {
        Self {
            config,
            watermark: SHIM_SESSION_ID,
            surfaces: BTreeMap::new(),
            seat_id: None,
            owed_frame_dones: VecDeque::new(),
        }
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

    /// Dispatch one decoded frame from the shim connection.
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
                    self.handle_commit(object_id, scene, send)?;
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

    /// The shim connection is gone (EOF, transport fault, or protocol
    /// violation): drop the realm's surface from the scene so no capture
    /// can ever serve a stale frame, and release every buffer fd still
    /// held (they close as `self` drops — no fd leak). The embedder calls
    /// this exactly once, then forgets both server and connection.
    pub fn connection_closed(self, scene: &mut Scene) {
        scene.clear_surface();
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
        // The fd's actual size must cover the declared geometry. Checked
        // for shm only: a dmabuf's size is the import path's to validate
        // (P1.3.5), and this core never maps a dmabuf.
        if req.kind == shim_surface::Kind::Shm {
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
        // is `bad_order` (IDL). Under copy-in the only attach ever awaiting
        // its buffer_done is the pending one, so the check is exactly this.
        if let Some(pending) = &state.pending_buffer {
            if pending.kind == shim_surface::Kind::Dmabuf && pending.buffer_id == req.buffer_id {
                return Err(ShimViolation::BadOrder(
                    "re-attach of a dmabuf buffer_id still awaiting buffer_done",
                )
                .into());
            }
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
    /// [`Scene::commit`] on the shim path — copy-in happens entirely here,
    /// before the scene changes, so composition never observes a partially
    /// applied frame.
    fn handle_commit<F>(
        &mut self,
        surface_id: u32,
        scene: &mut Scene,
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
        // not a new buffer is pending.
        let damage = std::mem::take(&mut state.pending_damage);
        tracing::trace!(
            surface = surface_id,
            damage_rects = damage.len(),
            "shim commit"
        );

        match state.pending_buffer.take() {
            // Repaint: no new attach — re-present the current buffer with
            // the new damage. Legal, no buffer_done (no attach to answer).
            None => {}
            Some(pending) => {
                let buffer_id = pending.buffer_id;
                let status = self.apply_buffer(pending, scene)?;
                let event = shim_surface::events::BufferDone { buffer_id, status };
                send(&event.encode(surface_id))?;
            }
        }

        // Exactly one frame_done owed per commit — even when the buffer
        // failed (the previously committed content is re-presented), so the
        // app's pacing loop never stalls on a rejected buffer (IDL).
        self.owed_frame_dones.push_back(surface_id);
        Ok(())
    }

    /// Consume one pending buffer at commit: copy a valid shm buffer into
    /// the scene, or resolve the recoverable failure dispositions. Returns
    /// the `buffer_done` status; the buffer's fd closes on return in every
    /// arm ("the core takes ownership of the fd received in attach and
    /// closes it before or as it emits this event").
    fn apply_buffer(
        &mut self,
        pending: PendingBuffer,
        scene: &mut Scene,
    ) -> Result<shim_surface::BufferStatus, ShimFault> {
        // No dmabuf import in this core yet (P1.3.5): the designed
        // recoverable fallback — the shim re-attaches the same content as
        // shm. Previously committed content stays on screen.
        if pending.kind == shim_surface::Kind::Dmabuf {
            tracing::debug!(
                buffer_id = pending.buffer_id,
                "dmabuf attach before P1.3.5: directing shim to shm fallback"
            );
            return Ok(shim_surface::BufferStatus::ImportFailed);
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
            return Ok(shim_surface::BufferStatus::TooLarge);
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

        // Structurally exact by construction; mapped anyway so a future
        // copy-in bug surfaces as the protocol's own condition, never as a
        // half-applied commit.
        let content = SurfaceContent::from_rgba(rgba, pending.width, pending.height)
            .map_err(|e| ShimViolation::InvalidBuffer(e.to_string()))?;
        scene.commit(content);
        Ok(shim_surface::BufferStatus::Released)
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

    /// Receive and dispatch exactly `n` shim messages on the core side.
    /// Returns whether any of them latched a commit.
    fn process_n(
        server: &mut ShimServer,
        scene: &mut Scene,
        core: &mut Connection,
        n: usize,
    ) -> Result<bool, ShimFault> {
        let mut committed = false;
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            committed |=
                server.handle_message(msg, scene, &mut |frame| core.send_message(frame, None))?;
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
        // No dmabuf import until P1.3.5: the commit resolves as the IDL's
        // recoverable fallback (buffer_done(import_failed)), previous
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
        let _fd = crate::capture::tests::fd_lock();
        let baseline = { std::fs::read_dir("/proc/self/fd").unwrap().count() };
        let (mut server, mut scene, mut core, shim) = setup();
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.attach_frame(0).unwrap();
        mock.commit().unwrap();
        process_n(&mut server, &mut scene, &mut core, 4).unwrap();
        // Stage a pending attach the server still holds the fd of.
        mock.attach_frame(1).unwrap();
        process_n(&mut server, &mut scene, &mut core, 2).unwrap();
        assert_eq!(scene.compose(VIEW_W, VIEW_H), frame_rgba(0, VIEW_W, VIEW_H));

        // Shim death: the embedder tears the server down.
        server.connection_closed(&mut scene);
        drop(mock);
        drop(core);
        // Never a stale frame: the scene falls back to the background.
        assert_eq!(
            scene.compose(VIEW_W, VIEW_H),
            test_pattern::render(VIEW_W, VIEW_H)
        );
        // Every held buffer fd (including the staged pending one) closed.
        assert_eq!(
            std::fs::read_dir("/proc/self/fd").unwrap().count(),
            baseline,
            "no fd may leak on shim teardown"
        );
    }
}
