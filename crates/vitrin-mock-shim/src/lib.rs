// SPDX-License-Identifier: Apache-2.0
//! A mock per-app shim: the **permanent test fixture** for the shim-facing
//! side of the trusted core (issue #21, P1.3.4).
//!
//! This crate speaks the real shim wire protocol — `vitrin_shim_session` and
//! `vitrin_shim_surface`, encoded through the generated `vitrin-protocol`
//! types over a real `vitrin-ipc` [`Connection`] — so the core's shim server
//! is exercised end-to-end on actual frames and actual `SCM_RIGHTS` memfds
//! long before the real C shim (P1.6.2) exists. It is *not* throwaway test
//! scaffolding: it outlives P1.3.4 as the hostile-free reference peer for the
//! spawn path (P1.5.2, which may lift it into a small executable once the
//! fd-inheritance contract is defined there) and as the conformance
//! counterpart the C shim can be diffed against.
//!
//! The fixture is deliberately a *compliant* shim: it performs the canonical
//! flow g of `docs/protocol/10-vitrin_shim_surface.md` (read `configure`
//! synchronously, mint a surface, then attach/damage/commit and pace on
//! `frame_done`). Hostile behavior — bad geometry, fd bombs, protocol-order
//! violations — is handcrafted per-test in the core's own test modules, where
//! each violation is the point of the test.
//!
//! # Deterministic animation (plan hard constraint R7)
//!
//! Frame content is a pure integer function of the frame index —
//! [`frame_rgba`] / [`frame_xrgb8888`] — so pixel assertions need no image
//! codec anywhere: the core's tests regenerate the expected bytes in-process
//! and compare exactly. Every pixel of frame `n` differs from frame `n + 1`,
//! so any torn mix of two frames equals *neither* generator output — the
//! property the no-tearing acceptance test leans on.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};

use rustix::fs::MemfdFlags;
use vitrin_ipc::{Connection, TransportError};
use vitrin_protocol::error::DecodeError;
use vitrin_protocol::generated::vitrin_actuator_pointer::{Axis, ButtonState};
use vitrin_protocol::generated::vitrin_shim_seat as shim_seat;
use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};
use vitrin_protocol::generated::vitrin_shim_session as session;
use vitrin_protocol::generated::vitrin_shim_surface as shim_surface;
use vitrin_protocol::generated::vitrin_view::Format;
use vitrin_protocol::Fixed;

/// The shim-session bootstrap object: implicit object 1 on a shim connection
/// (conventions section 3).
pub const SHIM_SESSION_ID: u32 = 1;

/// Bytes per pixel of both version-1 formats (xrgb8888 / argb8888).
pub const BYTES_PER_PIXEL: usize = 4;

/// Everything that can go wrong on the mock shim's side of the wire.
#[derive(Debug)]
pub enum MockShimError {
    /// Transport failure (send/recv on the socketpair).
    Transport(TransportError),
    /// The core's bytes did not decode as the expected event.
    Decode(DecodeError),
    /// The core closed the connection while an event was awaited.
    Disconnected,
    /// The core sent an event the mock shim does not recognize (wrong
    /// object, unknown opcode) — a core-side bug the fixture surfaces
    /// loudly instead of skipping.
    UnexpectedEvent { object_id: u32, opcode: u8 },
    /// Local I/O failure building a buffer (memfd create/write).
    Io(io::Error),
}

impl From<TransportError> for MockShimError {
    fn from(e: TransportError) -> Self {
        MockShimError::Transport(e)
    }
}

impl From<DecodeError> for MockShimError {
    fn from(e: DecodeError) -> Self {
        MockShimError::Decode(e)
    }
}

impl From<io::Error> for MockShimError {
    fn from(e: io::Error) -> Self {
        MockShimError::Io(e)
    }
}

impl std::fmt::Display for MockShimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MockShimError::Transport(e) => write!(f, "transport: {e}"),
            MockShimError::Decode(e) => write!(f, "decode: {e}"),
            MockShimError::Disconnected => write!(f, "core closed the connection"),
            MockShimError::UnexpectedEvent { object_id, opcode } => {
                write!(f, "unexpected event: object {object_id}, opcode {opcode}")
            }
            MockShimError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for MockShimError {}

/// One decoded core-to-shim event on the surface object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceEvent {
    /// `vitrin_shim_surface.frame_done` — the pacing callback.
    FrameDone { time_ms: u32 },
    /// `vitrin_shim_surface.buffer_done` — buffer ownership returned.
    BufferDone {
        buffer_id: u32,
        status: shim_surface::BufferStatus,
    },
}

/// One decoded core-to-shim event on the seat object (`vitrin_shim_seat`).
/// Every variant carries the wire's `origin` tag — per-event, final
/// argument, per backward requirement B2 — so core-side tests assert tag
/// preservation on real wire bytes, exactly what the C shim (P1.6.3) will
/// decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeatEvent {
    /// `motion` — pointer moved, realm-view coordinates (24.8 fixed).
    Motion { x: Fixed, y: Fixed, origin: Origin },
    /// `button` — pointer button, Linux evdev code.
    Button {
        button: u32,
        state: ButtonState,
        origin: Origin,
    },
    /// `scroll` — high-resolution scroll, one notch = ±120.
    Scroll {
        axis: Axis,
        value120: i32,
        origin: Origin,
    },
    /// `key` — xkbcommon keysym, modifier-resolved.
    Key {
        keysym: u32,
        state: KeyState,
        origin: Origin,
    },
    /// `text` — UTF-8 string delivery.
    Text { text: String, origin: Origin },
}

/// What one paced animation run observed, for the harness to assert on.
#[derive(Debug, Default)]
pub struct AnimationStats {
    /// Frames fully rendered (attach + damage + commit + both terminals).
    pub frames_rendered: u32,
    /// Every `frame_done` presentation time, in arrival order.
    pub frame_done_times: Vec<u32>,
    /// Every `buffer_done`, in arrival order.
    pub buffer_dones: Vec<(u32, shim_surface::BufferStatus)>,
}

/// Deterministic frame content: tightly packed RGBA8888 (rows top-down,
/// alpha `0xFF`) for frame `n` at `width x height`. A pure integer function
/// of `(n, x, y)`; every pixel depends on `n`, so two frames never share a
/// pixel and a torn mix of frames matches neither.
pub fn frame_rgba(n: u32, width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
    for y in 0..height {
        for x in 0..width {
            out.extend_from_slice(&[
                ((n.wrapping_mul(31)).wrapping_add(x) % 251) as u8,
                ((n.wrapping_mul(17)).wrapping_add(y) % 241) as u8,
                ((n.wrapping_mul(7)).wrapping_add(x ^ y) % 239) as u8,
                0xff,
            ]);
        }
    }
    out
}

/// [`frame_rgba`] converted to the wire's little-endian xrgb8888 byte order
/// (B, G, R, X with X = `0xFF`) — the bytes the mock shim writes into the
/// memfd it attaches.
pub fn frame_xrgb8888(n: u32, width: u32, height: u32) -> Vec<u8> {
    rgba_to_xrgb8888(&frame_rgba(n, width, height))
}

/// RGBA bytes → little-endian xrgb8888 bytes (B, G, R, X), X pinned `0xFF`.
pub fn rgba_to_xrgb8888(rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (src, dst) in rgba
        .chunks_exact(BYTES_PER_PIXEL)
        .zip(out.chunks_exact_mut(BYTES_PER_PIXEL))
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 0xff;
    }
    out
}

/// Create a memfd holding exactly `bytes` — the shm buffer of an `attach`.
/// Deliberately unsealed: version-1 shim buffers carry no seal contract
/// (unlike capture memfds), and the core's copy-in must stay robust against
/// that.
pub fn memfd_with_bytes(bytes: &[u8]) -> io::Result<OwnedFd> {
    let fd = rustix::fs::memfd_create("vitrin-mock-shim-buffer", MemfdFlags::CLOEXEC)?;
    let mut file = File::from(fd);
    file.write_all(bytes)?;
    Ok(OwnedFd::from(file))
}

/// A compliant mock shim bound to one core connection.
///
/// Construct with [`MockShim::start`], which performs the session bring-up
/// (flow G of `docs/protocol/09-vitrin_shim_session.md`): one synchronous
/// read for `configure`, then `create_surface`. After that the frame loop
/// belongs to the caller — per-frame via [`attach_frame`](Self::attach_frame)
/// / [`commit`](Self::commit) / [`next_surface_event`](Self::next_surface_event),
/// or whole via [`run_paced_animation`](Self::run_paced_animation).
pub struct MockShim {
    conn: Connection,
    /// The realm identity the core announced in `configure` (informational).
    pub realm: String,
    /// Realm-view width from `configure`; buffers are rendered at this size.
    pub width: u32,
    /// Realm-view height from `configure`.
    pub height: u32,
    /// The surface object id this shim minted (above the bootstrap id,
    /// per the watermark rule).
    pub surface_id: u32,
    /// The seat object id, once minted via [`get_seat`](Self::get_seat).
    pub seat_id: Option<u32>,
    next_buffer_id: u32,
}

impl MockShim {
    /// Session bring-up on an established shim connection: block on the
    /// core's `configure` (guaranteed-first message), then mint the surface
    /// with `create_surface`.
    pub fn start(mut conn: Connection) -> Result<Self, MockShimError> {
        let msg = conn.recv_message()?.ok_or(MockShimError::Disconnected)?;
        let (object_id, configure) = session::events::Configure::decode(&msg.bytes, msg.fd)?;
        if object_id != SHIM_SESSION_ID {
            return Err(MockShimError::UnexpectedEvent {
                object_id,
                opcode: msg.header.opcode,
            });
        }

        // First client-allocated id above the bootstrap watermark.
        let surface_id = 2;
        let create = session::requests::CreateSurface {
            surface: surface_id,
        };
        conn.send_message(&create.encode(SHIM_SESSION_ID), None)?;

        Ok(Self {
            conn,
            realm: configure.realm,
            width: configure.width,
            height: configure.height,
            surface_id,
            seat_id: None,
            next_buffer_id: 1,
        })
    }

    /// Mint the session's input-delivery object (`get_seat`) — the seat
    /// the core addresses origin-tagged `vitrin_shim_seat` events to
    /// (P1.3.7). A well-behaved shim does this during startup; the fixture
    /// keeps it a separate step after [`start`](Self::start) so
    /// pre-existing surface-path tests keep their exact message counts.
    /// Returns the seat object id (minted above the surface id, per the
    /// watermark rule).
    pub fn get_seat(&mut self) -> Result<u32, MockShimError> {
        let seat_id = self.surface_id + 1;
        let get_seat = session::requests::GetSeat { seat: seat_id };
        self.conn
            .send_message(&get_seat.encode(SHIM_SESSION_ID), None)?;
        self.seat_id = Some(seat_id);
        Ok(seat_id)
    }

    /// Block for the next core-to-shim event on the seat object. Fails
    /// with [`MockShimError::UnexpectedEvent`] if the seat was never
    /// minted or the next event targets another object.
    pub fn next_seat_event(&mut self) -> Result<SeatEvent, MockShimError> {
        let msg = self
            .conn
            .recv_message()?
            .ok_or(MockShimError::Disconnected)?;
        let object_id = msg.header.object_id;
        let opcode = msg.header.opcode;
        if self.seat_id != Some(object_id) {
            return Err(MockShimError::UnexpectedEvent { object_id, opcode });
        }
        match opcode {
            shim_seat::events::Motion::OPCODE => {
                let (_, ev) = shim_seat::events::Motion::decode(&msg.bytes, msg.fd)?;
                Ok(SeatEvent::Motion {
                    x: ev.x,
                    y: ev.y,
                    origin: ev.origin,
                })
            }
            shim_seat::events::Button::OPCODE => {
                let (_, ev) = shim_seat::events::Button::decode(&msg.bytes, msg.fd)?;
                Ok(SeatEvent::Button {
                    button: ev.button,
                    state: ev.state,
                    origin: ev.origin,
                })
            }
            shim_seat::events::Scroll::OPCODE => {
                let (_, ev) = shim_seat::events::Scroll::decode(&msg.bytes, msg.fd)?;
                Ok(SeatEvent::Scroll {
                    axis: ev.axis,
                    value120: ev.value120,
                    origin: ev.origin,
                })
            }
            shim_seat::events::Key::OPCODE => {
                let (_, ev) = shim_seat::events::Key::decode(&msg.bytes, msg.fd)?;
                Ok(SeatEvent::Key {
                    keysym: ev.keysym,
                    state: ev.state,
                    origin: ev.origin,
                })
            }
            shim_seat::events::Text::OPCODE => {
                let (_, ev) = shim_seat::events::Text::decode(&msg.bytes, msg.fd)?;
                Ok(SeatEvent::Text {
                    text: ev.text,
                    origin: ev.origin,
                })
            }
            _ => Err(MockShimError::UnexpectedEvent { object_id, opcode }),
        }
    }

    /// Attach frame `n` as a fresh shm memfd at the configured view size
    /// (xrgb8888, tight stride), followed by a full-surface `damage`.
    /// Returns the buffer cookie used, for correlation with `buffer_done`.
    pub fn attach_frame(&mut self, n: u32) -> Result<u32, MockShimError> {
        let pixels = frame_xrgb8888(n, self.width, self.height);
        let fd = memfd_with_bytes(&pixels)?;
        let buffer_id = self.next_buffer_id;
        self.next_buffer_id += 1;
        let attach = shim_surface::requests::Attach {
            buffer_id,
            fd,
            kind: shim_surface::Kind::Shm,
            format: Format::Xrgb8888,
            width: self.width,
            height: self.height,
            stride: self.width * BYTES_PER_PIXEL as u32,
        };
        self.conn
            .send_message(&attach.encode(self.surface_id), Some(attach.fd.as_fd()))?;
        // `attach` drops here: the shim's copy of the memfd closes; the
        // core's SCM_RIGHTS duplicate is the one that matters.

        let damage = shim_surface::requests::Damage {
            x: 0,
            y: 0,
            width: self.width as i32,
            height: self.height as i32,
        };
        self.conn
            .send_message(&damage.encode(self.surface_id), None)?;
        Ok(buffer_id)
    }

    /// Attach an externally allocated buffer as `kind=dmabuf` (P1.3.5),
    /// followed by a full-surface `damage` — the compliant zero-copy attach
    /// of `docs/protocol/10-vitrin_shim_surface.md` flow g′, exactly what
    /// the real shim's v1 forwarding (P1.6.2) passes through untouched. The
    /// fixture does not allocate GPU buffers itself: the caller (a
    /// real-GPU test harness, or a hostile-path test passing a deliberate
    /// non-dmabuf) supplies the fd and its geometry. Returns the buffer
    /// cookie used, for correlation with `buffer_done`.
    pub fn attach_dmabuf(
        &mut self,
        fd: OwnedFd,
        format: Format,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<u32, MockShimError> {
        let buffer_id = self.next_buffer_id;
        self.next_buffer_id += 1;
        let attach = shim_surface::requests::Attach {
            buffer_id,
            fd,
            kind: shim_surface::Kind::Dmabuf,
            format,
            width,
            height,
            stride,
        };
        self.conn
            .send_message(&attach.encode(self.surface_id), Some(attach.fd.as_fd()))?;
        // `attach` drops here: the shim's duplicate of the dmabuf fd
        // closes; the core's SCM_RIGHTS duplicate is the one that matters
        // (and, zero-copy, the one the GPU keeps sampling until release).

        let damage = shim_surface::requests::Damage {
            x: 0,
            y: 0,
            width: width as i32,
            height: height as i32,
        };
        self.conn
            .send_message(&damage.encode(self.surface_id), None)?;
        Ok(buffer_id)
    }

    /// Send `commit`, atomically latching the pending attach + damage.
    pub fn commit(&mut self) -> Result<(), MockShimError> {
        let commit = shim_surface::requests::Commit {};
        self.conn
            .send_message(&commit.encode(self.surface_id), None)?;
        Ok(())
    }

    /// Block for the next core-to-shim event on the surface object.
    pub fn next_surface_event(&mut self) -> Result<SurfaceEvent, MockShimError> {
        let msg = self
            .conn
            .recv_message()?
            .ok_or(MockShimError::Disconnected)?;
        let object_id = msg.header.object_id;
        let opcode = msg.header.opcode;
        if object_id != self.surface_id {
            return Err(MockShimError::UnexpectedEvent { object_id, opcode });
        }
        match opcode {
            shim_surface::events::FrameDone::OPCODE => {
                let (_, ev) = shim_surface::events::FrameDone::decode(&msg.bytes, msg.fd)?;
                Ok(SurfaceEvent::FrameDone {
                    time_ms: ev.time_ms,
                })
            }
            shim_surface::events::BufferDone::OPCODE => {
                let (_, ev) = shim_surface::events::BufferDone::decode(&msg.bytes, msg.fd)?;
                Ok(SurfaceEvent::BufferDone {
                    buffer_id: ev.buffer_id,
                    status: ev.status,
                })
            }
            _ => Err(MockShimError::UnexpectedEvent { object_id, opcode }),
        }
    }

    /// The underlying transport connection, for harness-side wire probes
    /// (e.g. asserting that no event is pending before a presentation
    /// tick). The frame-loop methods above are the normal interface.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// The paced animation loop of the acceptance criteria: for each frame
    /// `0..frames`, attach + damage + commit, then **block until that
    /// commit's `frame_done` arrives** before rendering the next frame —
    /// the shim never runs ahead of the presentation cadence, exactly as a
    /// real shim fans `frame_done` out to its app's `wl_surface.frame`
    /// callbacks (PRD Doc 2 §4.4). Also collects each attach's
    /// `buffer_done`, asserting the exactly-once, attach-order contract.
    pub fn run_paced_animation(&mut self, frames: u32) -> Result<AnimationStats, MockShimError> {
        let mut stats = AnimationStats::default();
        for n in 0..frames {
            let buffer_id = self.attach_frame(n)?;
            self.commit()?;
            let mut frame_done = false;
            let mut buffer_done = false;
            while !(frame_done && buffer_done) {
                match self.next_surface_event()? {
                    SurfaceEvent::FrameDone { time_ms } => {
                        frame_done = true;
                        stats.frame_done_times.push(time_ms);
                    }
                    SurfaceEvent::BufferDone {
                        buffer_id: id,
                        status,
                    } => {
                        debug_assert_eq!(
                            id, buffer_id,
                            "buffer_done must return the cookie of this frame's attach"
                        );
                        buffer_done = true;
                        stats.buffer_dones.push((id, status));
                    }
                }
            }
            stats.frames_rendered = n + 1;
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_deterministic_and_pairwise_distinct() {
        let a = frame_rgba(3, 16, 8);
        assert_eq!(a, frame_rgba(3, 16, 8), "generator must be pure");
        let b = frame_rgba(4, 16, 8);
        // Every pixel differs between consecutive frames: a torn mix of two
        // frames can therefore never equal either generator output.
        for (pa, pb) in a
            .chunks_exact(BYTES_PER_PIXEL)
            .zip(b.chunks_exact(BYTES_PER_PIXEL))
        {
            assert_ne!(pa[0], pb[0], "R channel must depend on the frame index");
        }
    }

    #[test]
    fn xrgb_conversion_swizzles_and_pins_padding() {
        assert_eq!(
            rgba_to_xrgb8888(&[0x11, 0x22, 0x33, 0x44]),
            [0x33, 0x22, 0x11, 0xff]
        );
        let n = 7;
        let rgba = frame_rgba(n, 4, 4);
        let xrgb = frame_xrgb8888(n, 4, 4);
        for (s, d) in rgba
            .chunks_exact(BYTES_PER_PIXEL)
            .zip(xrgb.chunks_exact(BYTES_PER_PIXEL))
        {
            assert_eq!([d[2], d[1], d[0]], [s[0], s[1], s[2]]);
            assert_eq!(d[3], 0xff);
        }
    }
}
