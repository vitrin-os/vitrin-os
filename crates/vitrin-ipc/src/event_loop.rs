// SPDX-License-Identifier: MPL-2.0
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

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::rc::Rc;

use calloop::generic::{Generic, NoIoDrop};
use calloop::ping::{make_ping, Ping, PingSource};
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
    /// The out-of-dispatch send path, when one was requested at
    /// construction ([`ConnectionSource::with_outbox`]). `None` for a
    /// connection whose every reply is issued from inside its own dispatch
    /// callback, which is the ordinary principal-connection shape.
    outbox: Option<OutboxSink>,
}

/// The source-side half of an [`Outbox`]: the queue itself plus the
/// [`PingSource`] whose readiness is what gets this source dispatched when
/// the queue gains a frame.
struct OutboxSink {
    /// Woken by [`Outbox::send`]; drained (and the queue with it) inside
    /// [`ConnectionSource::process_events`].
    ping: PingSource,
    queue: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

/// How many frames may sit in an [`Outbox`] before the producer is told the
/// peer is not keeping up.
///
/// This is a *second* bound in front of [`crate::MAX_SEND_QUEUE_BYTES`],
/// not a replacement for it: the connection's own send queue only starts
/// filling once the loop has actually dispatched this source, and an
/// enqueue-side flood (a runaway actuation stream, a compositor ticking
/// faster than the peer reads) would otherwise grow unbounded in the gap.
/// The number is deliberately small — the frames that ride an outbox are
/// input events and frame callbacks, both of which are worthless stale, so
/// a deep queue would buy latency instead of resilience.
pub const MAX_OUTBOX_FRAMES: usize = 64;

/// A send handle for a [`Connection`] that a [`ConnectionSource`] owns.
///
/// # Why this exists
///
/// Once a [`Connection`] is registered, the only sanctioned way to write to
/// it is [`reply`], which needs the [`NoIoDrop<Connection>`](NoIoDrop) the
/// dispatch callback is handed — so *by construction* the core can only
/// speak to a peer while that peer is speaking to it. For a request/response
/// peer (a principal connection) that is exactly right and no more is
/// wanted.
///
/// It is wrong for the two things the compositor must push at a peer that is
/// sitting silent in a blocking `recv`: **seat events** (the human or an
/// authorized agent actuated; the shim is by definition quiet, that is what
/// waiting for input *is*) and **`frame_done`** (presentation completed on
/// the compositor's cadence, not on the shim's). Neither has any inbound
/// message to ride, and a design that deferred them to the peer's next
/// readiness would deliver input only to peers that did not need any.
///
/// So an outbox is a queue plus a wakeup. [`Outbox::send`] appends and pings;
/// the ping is a second fd registered by the same [`ConnectionSource`], so
/// the loop dispatches that source, which drains the queue through
/// [`Connection::send_or_queue`] — the same call [`reply`] makes, subject to
/// the same backpressure, poisoning, and [`DisconnectReason::SlowReader`]
/// policy. Nothing bypasses the transport's own rules; the outbox only
/// supplies the *occasion* to write.
///
/// # Frames only, never fds
///
/// [`Outbox::send`] takes no fd. Everything version 1 pushes this way is
/// pure wire bytes, and the one event that carries an fd
/// (`vitrin_view.frame_ready`'s memfd) is a reply on a principal
/// connection, which already has [`reply`]. Refusing fds here keeps the
/// queue plain `Vec<u8>` and means a parked outbox can never pin a
/// descriptor open.
#[derive(Clone)]
pub struct Outbox {
    queue: Rc<RefCell<VecDeque<Vec<u8>>>>,
    ping: Ping,
}

impl Outbox {
    /// Queue one complete encoded frame and wake the loop so the owning
    /// [`ConnectionSource`] drains it.
    ///
    /// `Err(TransportError::SendQueueFull)` means the queue hit
    /// [`MAX_OUTBOX_FRAMES`]: the peer is not reading. The frame is **not**
    /// queued. Treat it exactly as a full send queue — stop producing for
    /// this peer and let the connection die on the transport's own
    /// slow-reader policy, which the drain below will trip on the next
    /// dispatch.
    pub fn send(&self, frame: &[u8]) -> Result<(), TransportError> {
        let mut queue = self.queue.borrow_mut();
        if queue.len() >= MAX_OUTBOX_FRAMES {
            return Err(TransportError::SendQueueFull {
                queued: queue.iter().map(Vec::len).sum(),
            });
        }
        queue.push_back(frame.to_vec());
        drop(queue);
        // After the push, never before: a wakeup that arrives ahead of the
        // frame would drain nothing and the frame would then wait for an
        // unrelated readiness.
        self.ping.ping();
        Ok(())
    }

    /// Frames queued but not yet handed to the connection. Zero on any
    /// quiescent loop; non-zero only between a [`send`](Self::send) and the
    /// dispatch it woke.
    pub fn pending(&self) -> usize {
        self.queue.borrow().len()
    }
}

impl fmt::Debug for Outbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbox")
            .field("pending", &self.pending())
            .finish()
    }
}

impl ConnectionSource {
    /// Wrap an accepted/connected [`Connection`], switching its fd to
    /// non-blocking so draining never stalls the loop.
    pub fn new(conn: Connection) -> io::Result<Self> {
        set_nonblocking(conn.as_fd())?;
        Ok(Self {
            inner: Generic::new(conn, Interest::READ, Mode::Level),
            outbox: None,
        })
    }

    /// [`ConnectionSource::new`] plus an [`Outbox`]: a handle the core keeps
    /// outside the loop for pushing frames at a peer that is not talking.
    ///
    /// Read [`Outbox`]'s docs before reaching for this. A connection whose
    /// traffic is entirely request/response wants [`new`](Self::new); an
    /// outbox on such a connection is a second write path with no second
    /// purpose, and every write path is somewhere ordering can go wrong.
    ///
    /// The [`Outbox`] is `Clone` and neither half keeps the other alive: drop
    /// every clone and the source simply stops being woken (the drained
    /// queue is empty, so nothing is lost); drop the source — which is how a
    /// connection is closed here — and [`Outbox::send`] keeps succeeding into
    /// a queue nobody reads. That asymmetry is deliberate and is why the
    /// core must forget its outbox at the same moment it forgets the
    /// connection.
    pub fn with_outbox(conn: Connection) -> io::Result<(Self, Outbox)> {
        set_nonblocking(conn.as_fd())?;
        let (ping, ping_source) = make_ping()?;
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let source = Self {
            inner: Generic::new(conn, Interest::READ, Mode::Level),
            outbox: Some(OutboxSink {
                ping: ping_source,
                queue: Rc::clone(&queue),
            }),
        };
        Ok((source, Outbox { queue, ping }))
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
        // Split the borrow so the outbox drain can hold the queue and the
        // connection at once; `self` is reassembled implicitly at the end.
        let Self { inner, outbox } = self;

        // The outbox half of this source, if any. Its ping is a *second*
        // registered fd, so this dispatch may be for the ping alone, with
        // the connection's own fd not ready at all — hence the drain runs
        // before, and independently of, the read path below.
        //
        // Nothing here reports send failures: it does not have to. A frame
        // that could not be handed over leaves the connection's sticky
        // overflow/poison state set, and the interest recomputation at the
        // bottom of this function then adds write-interest, so the *inner*
        // source dispatches on write-readiness and runs the one
        // fault-reporting path there is. Duplicating that reporting here
        // would be a second place a connection can be declared dead.
        let mut outbox_closed = false;
        if let Some(sink) = outbox.as_mut() {
            let mut pinged = false;
            let action = sink
                .ping
                .process_events(readiness, token, |(), &mut ()| pinged = true)
                .map_err(io::Error::other)?;
            outbox_closed = matches!(action, PostAction::Remove);
            if pinged {
                // SAFETY: as in the read path below -- `get_mut` yields
                // `&mut Connection` for `send_or_queue` calls that mutate
                // only the connection's own send queue and write its fd;
                // the Connection stays owned by `inner` (and registered
                // with the poller) across them.
                let conn = unsafe { inner.get_mut() };
                let mut queued = sink.queue.borrow_mut();
                while let Some(frame) = queued.pop_front() {
                    // No fd rides an outbox frame, by construction
                    // ([`Outbox`]).
                    if conn.send_or_queue(&frame, None).is_err() {
                        // Sticky on the Connection; the write-readiness
                        // dispatch will classify and report it. Everything
                        // still queued is dropped with the connection, so
                        // hanging on to it would only pin memory for a peer
                        // that is already dying.
                        queued.clear();
                        break;
                    }
                }
            }
        }
        // Every `Ping` handle was dropped: nothing can wake this source
        // through the outbox again, so retire its registration rather than
        // polling a permanently-quiet fd for the connection's whole life.
        if outbox_closed {
            *outbox = None;
        }

        let mut post = inner.process_events(readiness, token, |_readiness, conn_nodrop| {
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
            let want_write = inner.get_ref().queued_send_bytes() > 0;
            if inner.interest.writable != want_write {
                inner.interest = if want_write {
                    Interest::BOTH
                } else {
                    Interest::READ
                };
                post = PostAction::Reregister;
            }
        }
        Ok(post)
    }

    // Both fds go through the same `TokenFactory`, so each child mints its
    // own token and each child's `process_events` ignores readiness that is
    // not its own -- which is what makes the compound source above safe to
    // drive by calling both children unconditionally.
    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.register(poll, token_factory)?;
        if let Some(sink) = self.outbox.as_mut() {
            sink.ping.register(poll, token_factory)?;
        }
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.reregister(poll, token_factory)?;
        if let Some(sink) = self.outbox.as_mut() {
            sink.ping.reregister(poll, token_factory)?;
        }
        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.inner.unregister(poll)?;
        if let Some(sink) = self.outbox.as_mut() {
            sink.ping.unregister(poll)?;
        }
        Ok(())
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
