// SPDX-License-Identifier: MPL-2.0
//! Transport-level errors.
//!
//! The split mirrors who is at fault and therefore who must react:
//!
//! - [`TransportError::Io`] -- the operating system failed us; retry or
//!   drop, nothing protocol-shaped to decide.
//! - [`TransportError::Eof`] -- the peer closed mid-frame. (A close *between*
//!   frames is not an error: `recv_message` returns `Ok(None)`.)
//! - [`TransportError::LocalMisuse`] -- *our* caller handed
//!   `send_message` a frame/fd combination that contradicts itself. This is
//!   a local bug surfaced as an error rather than a panic because the same
//!   [`Connection`](crate::Connection) type also serves unprivileged
//!   clients (the Rust SDK, test harnesses), where aborting the process is
//!   the wrong failure mode. Nothing is written to the wire.
//! - [`TransportError::PeerViolation`] -- the peer broke a framing or
//!   fd-passing invariant. Every variant is connection-fatal in the
//!   conventions' error taxonomy (section 5: these conditions "die", they
//!   are never recoverable events); the *graduated* misbehavior policy --
//!   what to log, whether to send a terminal error event first -- is
//!   P1.2.3's job and consumes these values. After a violation the
//!   connection is poisoned: further `recv_message` calls return the same
//!   violation rather than desynchronized garbage.
//! - [`TransportError::SendQueueFull`] -- the peer stopped *reading*: the
//!   non-blocking send path's per-connection queue hit
//!   [`crate::MAX_SEND_QUEUE_BYTES`] or [`crate::MAX_SEND_QUEUE_FDS`].
//!   Not a wire violation (nothing the peer
//!   sent was malformed) but the same disposition: the connection dies
//!   (`DisconnectReason::SlowReader` in the event-loop policy), because the
//!   only alternatives are unbounded buffering or blocking the compositor
//!   loop. Sticky like a poison: later sends keep returning it.

use std::fmt;
use std::io;

/// Any failure of [`Connection::send_message`](crate::Connection::send_message)
/// or [`Connection::recv_message`](crate::Connection::recv_message).
#[derive(Debug)]
pub enum TransportError {
    /// A syscall failed (including `EPIPE` on send after the peer closed).
    Io(io::Error),
    /// The peer closed the connection in the middle of a frame:
    /// `buffered` bytes of an incomplete frame had already arrived.
    /// Maps to fatal `oversized` in the conventions' taxonomy (the frame's
    /// declared size exceeds what the stream delivered).
    Eof { buffered: usize },
    /// `send_message` was called with self-contradictory arguments; nothing
    /// was sent.
    LocalMisuse(LocalMisuse),
    /// The peer violated a framing or fd-passing invariant; the connection
    /// is dead. See [`PeerViolation`].
    PeerViolation(PeerViolation),
    /// A non-blocking send could not be parked because the connection's
    /// send queue already holds `queued` bytes and accepting the frame
    /// would push it past [`crate::MAX_SEND_QUEUE_BYTES`] bytes or
    /// [`crate::MAX_SEND_QUEUE_FDS`] parked fds: the peer has
    /// stopped reading (slow reader). Sticky like a receive-side poison --
    /// every later [`Connection::send_or_queue`](crate::Connection::send_or_queue)
    /// returns it again; the P1.2.3 policy is to drop the connection, never
    /// to stall the loop waiting for the peer to drain.
    SendQueueFull { queued: usize },
}

/// A self-contradictory `send_message` call, caught before any byte hits
/// the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalMisuse {
    /// The buffer is shorter than the 8-byte frame header.
    FrameTooShort { len: usize },
    /// The header's `size` field does not equal the buffer length.
    SizeFieldMismatch { declared: u16, actual: usize },
    /// The header's `fd_count` disagrees with the fd argument actually
    /// passed (or exceeds [`crate::MAX_FDS_PER_MESSAGE`]).
    FdCountMismatch { declared: u8, attached: bool },
}

/// A connection-fatal framing or fd-passing violation by the peer.
///
/// Wire-error taxonomy mapping (`docs/protocol/00-conventions.md` section
/// 5), for the layer that turns these into terminal protocol errors:
/// [`UndersizedSizeField`](PeerViolation::UndersizedSizeField) is
/// `oversized`; the four fd variants are `fd_violation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerViolation {
    /// A frame header declared a total size below the 8-byte header length
    /// -- the one size violation a `u16` size field can express.
    UndersizedSizeField { size: u16 },
    /// A frame header declared more fds than protocol v0's
    /// one-fd-per-message invariant allows.
    FdCountExceeded { fd_count: u8 },
    /// A frame declared `fd_count = 1` but its bytes completed without an
    /// accompanying `SCM_RIGHTS` fd attached to them.
    MissingFd,
    /// An fd arrived attached to the bytes of a frame that does not declare
    /// it: either a frame declaring `fd_count = 0` carried an fd, or a
    /// one-fd frame carried more than one -- "fds attached to a message
    /// that declares none" (conventions 2.4), fatal `fd_violation`.
    /// Enforced here because no layer above the transport can observe an
    /// undeclared fd, and an undetected one would shift positional
    /// matching for every later fd-bearing frame.
    UnsolicitedFd,
    /// More received fds are queued than [`crate::MAX_UNCLAIMED_FDS`];
    /// the peer is shipping fds no frame claims (fd-bomb).
    UnclaimedFdOverflow,
    /// The kernel truncated the ancillary payload (`MSG_CTRUNC`): a single
    /// `sendmsg` carried more fds than the receive buffer admits. The
    /// kernel closes the undelivered fds; we kill the connection.
    AncillaryTruncated,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "transport I/O error: {e}"),
            TransportError::Eof { buffered } => write!(
                f,
                "peer closed the connection mid-frame ({buffered} bytes of an incomplete frame buffered)"
            ),
            TransportError::LocalMisuse(m) => write!(f, "local send_message misuse: {m}"),
            TransportError::PeerViolation(v) => write!(f, "peer violation: {v}"),
            TransportError::SendQueueFull { queued } => write!(
                f,
                "send queue full: {queued} bytes already parked for a peer that is not reading (caps: {} bytes, {} fds)",
                crate::MAX_SEND_QUEUE_BYTES,
                crate::MAX_SEND_QUEUE_FDS
            ),
        }
    }
}

impl fmt::Display for LocalMisuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocalMisuse::FrameTooShort { len } => {
                write!(
                    f,
                    "frame buffer of {len} bytes is shorter than the 8-byte header"
                )
            }
            LocalMisuse::SizeFieldMismatch { declared, actual } => write!(
                f,
                "header size field declares {declared} bytes but the buffer holds {actual}"
            ),
            LocalMisuse::FdCountMismatch { declared, attached } => write!(
                f,
                "header declares fd_count={declared} but an fd was {}",
                if *attached {
                    "attached"
                } else {
                    "not attached"
                }
            ),
        }
    }
}

impl fmt::Display for PeerViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerViolation::UndersizedSizeField { size } => {
                write!(
                    f,
                    "frame header declares size {size}, below the 8-byte minimum"
                )
            }
            PeerViolation::FdCountExceeded { fd_count } => write!(
                f,
                "frame header declares fd_count={fd_count}, above the one-fd-per-message limit"
            ),
            PeerViolation::MissingFd => {
                write!(f, "frame declared one fd but its bytes arrived without one")
            }
            PeerViolation::UnsolicitedFd => write!(
                f,
                "fd attached to the bytes of a frame that does not declare it"
            ),
            PeerViolation::UnclaimedFdOverflow => write!(
                f,
                "peer shipped more unclaimed fds than the {} the transport tolerates",
                crate::MAX_UNCLAIMED_FDS
            ),
            PeerViolation::AncillaryTruncated => {
                write!(
                    f,
                    "ancillary payload truncated (MSG_CTRUNC): too many fds in one sendmsg"
                )
            }
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransportError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        TransportError::Io(e)
    }
}

impl From<LocalMisuse> for TransportError {
    fn from(m: LocalMisuse) -> Self {
        TransportError::LocalMisuse(m)
    }
}

impl From<PeerViolation> for TransportError {
    fn from(v: PeerViolation) -> Self {
        TransportError::PeerViolation(v)
    }
}
