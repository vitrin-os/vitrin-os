//! Calloop event-source glue for the trusted core (the `server` feature).
//!
//! This module is what lets `vitrind` stay a single-threaded compositor whose
//! protocol handling is *just another event source*: no dedicated IPC thread,
//! no async runtime (plan risk R7 -- the TCB dependency budget). It wraps the
//! blocking [`Listener`] and [`Connection`] from this crate as
//! [calloop](calloop) event sources that the core inserts into the same
//! [`EventLoop`](calloop::EventLoop) that already drives its backend, signal,
//! and frame-timer sources.
//!
//! Two sources, mirroring the two things the loop must react to:
//!
//! - [`ListenerSource`] becomes readable when a principal connects; each
//!   readiness drains `accept(2)` to exhaustion and hands the core one
//!   [`ListenerEvent::Incoming`] per new [`Connection`] (peer credentials
//!   already captured). The core's usual move is to register each incoming
//!   connection as a [`ConnectionSource`] on the same loop.
//! - [`ConnectionSource`] becomes readable when a peer sends bytes; each
//!   readiness drains complete frames to exhaustion and hands the core one
//!   [`ConnectionEvent::Message`] per decoded frame. The connection is
//!   surfaced to the callback as metadata so a reply can go out on the same
//!   `sendmsg` turn.
//!
//! # Readiness contract (why the fds go non-blocking)
//!
//! The blocking send/recv of [`Connection`] is exactly right for the SDK
//! client, but inside the compositor loop a blocking `recvmsg` would stall
//! every other source -- the backend, the frame clock, SIGTERM. So both
//! sources put their fd in non-blocking mode ([`Mode::Level`];
//! [`Interest::READ`], with a [`ConnectionSource`] adding write-interest
//! exactly while replies are parked in its send queue) and drain until the
//! kernel returns `EAGAIN` ([`io::ErrorKind::WouldBlock`]), which the drain
//! treats as "no more for now, wait for the next readiness" -- never as an
//! error. A partial frame simply stays buffered on the [`Connection`] and
//! resumes on the next readiness. This is the Wayland/libwayland posture:
//! the loop never blocks on a peer.
//!
//! # Backpressure & misbehavior policy (P1.2.3)
//!
//! This module *is* the policy layer the transport's violations feed. The
//! posture is Wayland's, adopted wholesale: **a misbehaving client dies; the
//! loop never blocks and never buffers unboundedly on a client's behalf.**
//!
//! - **Slow readers.** [`reply`] parks what the kernel will not take in the
//!   connection's bounded send queue
//!   ([`Connection::send_or_queue`]); the source adds write-interest while
//!   anything is parked and flushes on write-readiness. A queue pushed past
//!   [`crate::MAX_SEND_QUEUE_BYTES`] bytes or [`crate::MAX_SEND_QUEUE_FDS`]
//!   parked fds means the peer stopped reading: the connection is removed
//!   with [`DisconnectReason::SlowReader`] -- it is never allowed to stall
//!   the loop, grow the queue without bound, or pin unbounded fds.
//! - **Dead and half-dead peers.** A fatal send I/O error (e.g. `EPIPE`
//!   from a peer that shut down its read side but keeps writing) poisons
//!   the connection's send path; the source observes the sticky poison
//!   after the dispatch and removes the connection with
//!   [`DisconnectReason::PeerAborted`] -- send failures are just as
//!   terminal as receive failures, even when nothing is parked.
//! - **Oversized messages and fd-bombs.** The transport's receive path
//!   already makes these connection-fatal
//!   ([`TransportError::PeerViolation`]); here each is classified into a
//!   [`DisconnectReason`], logged via `tracing`, and the connection removed.
//! - **A misbehaving peer kills only its own connection.** Every terminal
//!   condition arrives as [`ConnectionEvent::Fault`] /
//!   [`ConnectionEvent::Disconnected`] and the source removes itself; the
//!   loop and every other connection are untouched. A transient `accept(2)`
//!   failure arrives as [`ListenerEvent::AcceptError`] and the listener
//!   keeps listening. Neither is ever the source's own `Error` type, which
//!   calloop would propagate out of `dispatch` and tear the loop down.
//! - **No goodbye on the wire.** A connection killed for misbehavior is
//!   closed without a best-effort terminal protocol error: protocol v0
//!   defines no such message, and inventing one here would be an unpaired
//!   wire-format change belonging to `track:protocol` (see the crate docs).
//!   Any parked replies to the dying peer are dropped with it.

use std::fmt;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};

use calloop::generic::{Generic, NoIoDrop};
use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use crate::error::PeerViolation;
use crate::{Connection, Listener, Message, PeerCred, TransportError};

/// Why the event-loop glue terminated a connection -- the single
/// classification `vitrind` logs and cleans up on. Every variant is
/// connection-fatal; the connection is already dead (and its source removed)
/// when the callback sees this inside [`ConnectionEvent::Fault`].
///
/// The taxonomy follows the P1.2.3 misbehavior classes **as seen by the
/// operator/policy layer** (graduated handling: rate-limiting, per-uid
/// banning): [`PeerAborted`](Self::PeerAborted) is the one non-misbehavior
/// variant, so crash-shaped disconnects never pollute misbehavior counters.
/// The wire-error mapping (conventions section 5) is noted per variant.
#[derive(Debug)]
pub enum DisconnectReason {
    /// The peer stopped reading: its send queue hit
    /// [`crate::MAX_SEND_QUEUE_BYTES`] (with `queued` bytes already parked)
    /// or [`crate::MAX_SEND_QUEUE_FDS`] parked fds.
    SlowReader { queued: usize },
    /// A size violation: a frame header declared a size below the 8-byte
    /// header minimum ([`PeerViolation::UndersizedSizeField`]) -- the one
    /// size violation the u16 `size` field can express (a size *above*
    /// [`crate::MAX_MESSAGE_SIZE`] is inexpressible). Maps to the fatal
    /// `oversized` wire condition.
    Oversized(TransportError),
    /// An fd-passing violation -- more fds than the message schema allows
    /// (fatal `fd_violation`): a header declaring >1 fd, an fd attached to a
    /// frame that does not declare it, more unclaimed fds than
    /// [`crate::MAX_UNCLAIMED_FDS`], or an ancillary payload the kernel had
    /// to truncate.
    FdBomb(PeerViolation),
    /// The peer went away out from under the connection rather than
    /// violating anything: the stream ended inside a declared frame
    /// ([`TransportError::Eof`] -- e.g. the peer was killed between the
    /// partial writes of a frame) or an OS-level I/O failure on the socket
    /// ([`TransportError::Io`], e.g. `EPIPE`/`ECONNRESET`, including a
    /// sticky send-side poison observed after a dispatch). Equally fatal to
    /// the connection, but *not* misbehavior -- policy consumers must not
    /// count it toward graduated sanctions.
    PeerAborted(TransportError),
    /// Any other terminal transport condition, e.g. a frame that declared an
    /// fd but carried none ([`PeerViolation::MissingFd`]).
    ProtocolError(TransportError),
}

impl From<TransportError> for DisconnectReason {
    /// Classify a terminal transport error into the reason `vitrind` logs.
    fn from(e: TransportError) -> Self {
        match e {
            TransportError::SendQueueFull { queued } => DisconnectReason::SlowReader { queued },
            TransportError::Eof { .. } | TransportError::Io(_) => DisconnectReason::PeerAborted(e),
            TransportError::PeerViolation(PeerViolation::UndersizedSizeField { .. }) => {
                DisconnectReason::Oversized(e)
            }
            TransportError::PeerViolation(
                v @ (PeerViolation::FdCountExceeded { .. }
                | PeerViolation::UnsolicitedFd
                | PeerViolation::UnclaimedFdOverflow
                | PeerViolation::AncillaryTruncated),
            ) => DisconnectReason::FdBomb(v),
            other => DisconnectReason::ProtocolError(other),
        }
    }
}

impl fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DisconnectReason::SlowReader { queued } => write!(
                f,
                "slow reader: {queued} bytes parked, cap {} exceeded",
                crate::MAX_SEND_QUEUE_BYTES
            ),
            DisconnectReason::Oversized(e) => write!(f, "oversized: {e}"),
            DisconnectReason::FdBomb(v) => write!(f, "fd bomb: {v}"),
            DisconnectReason::PeerAborted(e) => write!(f, "peer aborted: {e}"),
            DisconnectReason::ProtocolError(e) => write!(f, "protocol error: {e}"),
        }
    }
}

/// The one place a terminated connection gets its reason logged: every
/// disconnect path (receive violation, flush/send failure, slow-reader
/// overflow) funnels through here before the [`ConnectionEvent::Fault`]
/// callback. Misbehavior classes log at WARN; a peer that merely went away
/// ([`DisconnectReason::PeerAborted`] -- crash-shaped, not hostile) logs at
/// INFO so it never reads as actionable misbehavior noise.
fn log_disconnect(conn: &Connection, reason: &DisconnectReason) {
    let cred = conn.peer_cred();
    match reason {
        DisconnectReason::PeerAborted(_) => tracing::info!(
            peer_uid = cred.uid,
            peer_pid = ?cred.pid,
            %reason,
            "terminating connection: peer went away"
        ),
        _ => tracing::warn!(
            peer_uid = cred.uid,
            peer_pid = ?cred.pid,
            %reason,
            "terminating connection for misbehavior"
        ),
    }
}

/// Put `fd` into non-blocking mode, preserving its other open-file flags.
///
/// The listening socket and each accepted/connected socket are created
/// blocking (P1.2.1's constructors do not set `O_NONBLOCK`); the event-loop
/// sources flip that bit so the compositor loop drains readiness without ever
/// blocking on a peer. `CLOEXEC` is a descriptor flag, not an open-file flag,
/// so toggling `O_NONBLOCK` here leaves this crate's close-on-exec discipline
/// intact.
fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}

/// What a [`ConnectionSource`] hands the core for one readiness.
///
/// A single readiness fires the dispatch callback once per complete frame
/// ([`Message`]), then at most once more with a terminal variant
/// ([`Disconnected`](ConnectionEvent::Disconnected) or
/// [`Fault`](ConnectionEvent::Fault)) after which the source removes itself
/// from the loop.
#[derive(Debug)]
pub enum ConnectionEvent {
    /// One complete decoded frame. `bytes` + `fd` feed straight into the
    /// generated `decode` for the message's object/opcode.
    Message(Message),
    /// The peer closed cleanly between frames. The source will be removed;
    /// the core should forget the connection. Any replies still parked in
    /// its send queue are dropped with it.
    Disconnected,
    /// The connection was terminated by policy: slow reader, oversized
    /// frame, fd bomb, or any other terminal transport condition. Already
    /// logged via `tracing` by this module; the source removes itself, so
    /// the core only has to forget the connection.
    Fault(DisconnectReason),
}

/// A [`Connection`] wired into a calloop loop as a read-readiness source.
///
/// Insert it with the usual calloop callback shape `|event, conn, state|`.
/// `conn` is a [`NoIoDrop<Connection>`](NoIoDrop) -- calloop's guard that the
/// registered fd cannot be dropped out from under the poller. Through it you
/// can [`peer_cred`](Connection::peer_cred) (a `&self` method, reached by
/// deref) and reply with [`reply`]; there is deliberately no safe way to get
/// a `&mut Connection` out of it, so no dispatch code can replace or drop the
/// live connection by accident. This keeps calloop's fd-liveness invariant
/// **type-enforced end to end** -- the trusted core needs no `unsafe`, and the
/// invariant is not weakened to a prose contract.
pub struct ConnectionSource {
    inner: Generic<Connection>,
}

impl ConnectionSource {
    /// Wrap an accepted/connected [`Connection`], switching its fd to
    /// non-blocking so draining never stalls the loop.
    pub fn new(conn: Connection) -> io::Result<Self> {
        set_nonblocking(conn.as_fd())?;
        Ok(Self {
            inner: Generic::new(conn, Interest::READ, Mode::Level),
        })
    }

    /// The peer credentials captured when the connection was created --
    /// useful to close over at `insert_source` time so the dispatch callback
    /// can tag messages with their principal's identity (P1.4.1).
    pub fn peer_cred(&self) -> PeerCred {
        self.inner.get_ref().peer_cred()
    }
}

impl EventSource for ConnectionSource {
    type Event = ConnectionEvent;
    // `NoIoDrop<Connection>`, not `Connection`: the callback gets calloop's
    // guard, which offers no safe `&mut Connection` (and hence no way to drop
    // the registered fd from safe code), keeping the poller's fd-liveness
    // invariant type-enforced. Replies go through [`reply`], which confines
    // the one required `unsafe` to this crate.
    type Metadata = NoIoDrop<Connection>;
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut post = self
            .inner
            .process_events(readiness, token, |_readiness, conn_nodrop| {
                // Flush parked replies first: write-readiness may be why this
                // dispatch fired, and freeing queue space gives replies issued
                // by the callback below the best chance of going out inline.
                if conn_nodrop.queued_send_bytes() > 0 {
                    // SAFETY: `get_mut` yields `&mut Connection` for exactly
                    // one `flush_send_queue`, which mutates only the
                    // connection's own send queue and writes its fd -- it
                    // never drops or moves the Connection, which stays owned
                    // by `inner` (and so registered with the poller) across
                    // the call.
                    if let Err(e) = unsafe { conn_nodrop.get_mut() }.flush_send_queue() {
                        let reason = DisconnectReason::from(e);
                        log_disconnect(conn_nodrop, &reason);
                        callback(ConnectionEvent::Fault(reason), conn_nodrop);
                        return Ok(PostAction::Remove);
                    }
                }
                let mut action = PostAction::Continue;
                loop {
                    // SAFETY: as above -- `get_mut` yields `&mut Connection`
                    // for exactly one `recv_message`, which mutates only the
                    // connection's own reassembly buffers and reads its fd;
                    // it never drops or moves the Connection. The borrow ends
                    // with the expression, before `conn_nodrop` is handed to
                    // the callback.
                    let received = unsafe { conn_nodrop.get_mut() }.recv_message();
                    match received {
                        Ok(Some(msg)) => callback(ConnectionEvent::Message(msg), conn_nodrop),
                        Ok(None) => {
                            callback(ConnectionEvent::Disconnected, conn_nodrop);
                            action = PostAction::Remove;
                            break;
                        }
                        // Drained: the kernel has no more bytes right now.
                        // Any partial frame stays buffered for next readiness.
                        Err(TransportError::Io(ref e)) if e.kind() == io::ErrorKind::WouldBlock => {
                            break
                        }
                        Err(e) => {
                            let reason = DisconnectReason::from(e);
                            log_disconnect(conn_nodrop, &reason);
                            callback(ConnectionEvent::Fault(reason), conn_nodrop);
                            action = PostAction::Remove;
                            break;
                        }
                    }
                }
                // Replies issued by the callback above may have hit the
                // send-queue cap or a fatal send I/O error. Both are sticky
                // on the Connection, so neither can be missed here, and the
                // connection dies in the same dispatch that hit it. The
                // overflow check makes the slow-reader policy *prompt* (the
                // loop never parks past-cap data); the poison check is the
                // *only* observation point for a send error with an empty
                // queue (e.g. EPIPE from a peer that shut down its read side
                // but keeps writing) -- without it such a connection would
                // live forever with every reply silently failing, and a
                // partial write followed by an error would leave a torn
                // frame on a still-registered connection.
                if matches!(action, PostAction::Continue) {
                    if conn_nodrop.send_queue_overflowed() {
                        let reason = DisconnectReason::SlowReader {
                            queued: conn_nodrop.queued_send_bytes(),
                        };
                        log_disconnect(conn_nodrop, &reason);
                        callback(ConnectionEvent::Fault(reason), conn_nodrop);
                        action = PostAction::Remove;
                    } else if let Some(kind) = conn_nodrop.send_poisoned() {
                        let reason = DisconnectReason::from(TransportError::Io(kind.into()));
                        log_disconnect(conn_nodrop, &reason);
                        callback(ConnectionEvent::Fault(reason), conn_nodrop);
                        action = PostAction::Remove;
                    }
                }
                Ok(action)
            })?;
        // Interest management: watch for write-readiness exactly while
        // replies are parked. `Reregister` makes calloop call `reregister`,
        // which re-registers with the updated `interest` field.
        if matches!(post, PostAction::Continue) {
            let want_write = self.inner.get_ref().queued_send_bytes() > 0;
            if self.inner.interest.writable != want_write {
                self.inner.interest = if want_write {
                    Interest::BOTH
                } else {
                    Interest::READ
                };
                post = PostAction::Reregister;
            }
        }
        Ok(post)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.inner.unregister(poll)
    }
}

/// Send a frame on the connection a message just arrived on, from inside a
/// [`ConnectionSource`] dispatch callback.
///
/// The callback receives a [`NoIoDrop<Connection>`](NoIoDrop) rather than a
/// bare `&mut Connection` (so it can never drop the registered fd); this is
/// the sanctioned way to reach [`Connection::send_or_queue`] through it. Same
/// arguments and [`TransportError`] contract as `send_or_queue`.
///
/// **Never blocks, never fails on a full socket** (P1.2.3): what the kernel
/// will not take is parked in the connection's bounded send queue and flushed
/// by the source on write-readiness. Two failures matter, and for both the
/// caller should stop replying and simply return -- the source observes the
/// sticky state when the dispatch callback returns, emits the matching
/// [`ConnectionEvent::Fault`], and removes the connection:
///
/// - [`TransportError::SendQueueFull`] -- the peer has stopped reading and
///   its parked bytes/fds hit [`crate::MAX_SEND_QUEUE_BYTES`] /
///   [`crate::MAX_SEND_QUEUE_FDS`] ([`DisconnectReason::SlowReader`]).
/// - [`TransportError::Io`] -- a fatal send error (e.g. `EPIPE` because the
///   peer went away); the connection's send side is poisoned
///   ([`DisconnectReason::PeerAborted`]).
pub fn reply(
    conn: &mut NoIoDrop<Connection>,
    frame: &[u8],
    fd: Option<BorrowedFd<'_>>,
) -> Result<(), TransportError> {
    // SAFETY: send_or_queue borrows the connection only for the send and
    // neither drops nor moves it; the Connection stays owned by the source's
    // `Generic` (and registered with the poller) across the call.
    unsafe { conn.get_mut() }.send_or_queue(frame, fd)
}

/// What a [`ListenerSource`] hands the core for one readiness.
#[derive(Debug)]
pub enum ListenerEvent {
    /// A newly accepted connection, its `SO_PEERCRED` already captured. The
    /// core typically wraps it in a [`ConnectionSource`] and inserts it on
    /// the same loop. The fd is still blocking; [`ConnectionSource::new`]
    /// switches it to non-blocking.
    Incoming(Connection),
    /// A transient `accept(2)` failure (e.g. `EMFILE`). The listener stays
    /// valid and keeps listening; this is surfaced so the core can log or
    /// shed load. Graduated handling is P1.2.3.
    AcceptError(io::Error),
}

/// A [`Listener`] wired into a calloop loop as a read-readiness source.
///
/// Insert it with `|event, (), state|`; on [`ListenerEvent::Incoming`] the
/// core registers a [`ConnectionSource`] for the new connection.
pub struct ListenerSource {
    inner: Generic<Listener>,
}

impl ListenerSource {
    /// Wrap a bound [`Listener`], switching its fd to non-blocking so the
    /// accept drain terminates on `EAGAIN` instead of blocking the loop.
    pub fn new(listener: Listener) -> io::Result<Self> {
        set_nonblocking(listener.as_fd())?;
        Ok(Self {
            inner: Generic::new(listener, Interest::READ, Mode::Level),
        })
    }
}

impl EventSource for ListenerSource {
    type Event = ListenerEvent;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.inner
            .process_events(readiness, token, |_readiness, listener| {
                // `Listener::accept` reads only the listening socket (a new fd
                // per call) and mutates no per-connection state, so the shared
                // `&Listener` NoIoDrop derefs to is enough -- no unsafe here.
                loop {
                    match listener.accept() {
                        Ok(conn) => callback(ListenerEvent::Incoming(conn), &mut ()),
                        // Backlog drained: wait for the next readiness.
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            // Surface as an event, never the source's Error
                            // (which calloop propagates out of dispatch,
                            // tearing down the loop). Break this drain round
                            // rather than looping on a repeating error within
                            // one readiness -- but note a persistent failure
                            // (e.g. EMFILE that keeps the socket readable)
                            // still re-fires across dispatches; graduated
                            // handling (disable/backoff) is P1.2.3.
                            callback(ListenerEvent::AcceptError(e), &mut ());
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            })
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.inner.unregister(poll)
    }
}
