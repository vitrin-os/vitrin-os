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
//! sources put their fd in non-blocking mode ([`Interest::READ`],
//! [`Mode::Level`]) and drain until the kernel returns `EAGAIN`
//! ([`io::ErrorKind::WouldBlock`]), which the drain treats as "no more for
//! now, wait for the next readiness" -- never as an error. A partial frame
//! simply stays buffered on the [`Connection`] and resumes on the next
//! readiness. This is the Wayland/libwayland posture: the loop never blocks
//! on a peer.
//!
//! # What this module does *not* do
//!
//! Backpressure and misbehavior *policy* is P1.2.3, not here. In particular:
//!
//! - **Sends are best-effort.** With the fd non-blocking, a reply issued from
//!   a dispatch callback can hit `EAGAIN` if the peer's receive buffer is
//!   full; [`Connection::send_message`] surfaces that as an I/O error. A
//!   per-connection send queue that parks such a reply and flushes it on
//!   write-readiness is P1.2.3's job. Replies in P1.2.2 are small
//!   (handshake-shaped) and fit the socket buffer.
//! - **A peer violation kills only its own connection.** A framing/fd
//!   violation arrives as [`ConnectionEvent::Fault`] and the source removes
//!   itself; the loop and every other connection are untouched. A transient
//!   `accept(2)` failure arrives as [`ListenerEvent::AcceptError`] and the
//!   listener keeps listening. Neither is ever the source's own `Error`
//!   type, which calloop would propagate out of `dispatch` and tear the loop
//!   down.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};

use calloop::generic::{Generic, NoIoDrop};
use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use crate::{Connection, Listener, Message, PeerCred, TransportError};

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
    /// the core should forget the connection.
    Disconnected,
    /// The peer broke a framing/fd invariant, or a non-would-block I/O error
    /// occurred. The connection is dead and the source will be removed; the
    /// value is the diagnosis P1.2.3 turns into logged policy.
    Fault(TransportError),
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
        self.inner
            .process_events(readiness, token, |_readiness, conn_nodrop| {
                let mut action = PostAction::Continue;
                loop {
                    // SAFETY: `get_mut` yields `&mut Connection` for exactly one
                    // `recv_message`, which mutates only the connection's own
                    // reassembly buffers and reads its fd -- it never drops or
                    // moves the Connection, which stays owned by `inner` (and so
                    // registered with the poller) across the call. The borrow
                    // ends with the expression, before `conn_nodrop` is handed
                    // to the callback.
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
                            callback(ConnectionEvent::Fault(e), conn_nodrop);
                            action = PostAction::Remove;
                            break;
                        }
                    }
                }
                Ok(action)
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

/// Send a frame on the connection a message just arrived on, from inside a
/// [`ConnectionSource`] dispatch callback.
///
/// The callback receives a [`NoIoDrop<Connection>`](NoIoDrop) rather than a
/// bare `&mut Connection` (so it can never drop the registered fd); this is
/// the sanctioned way to reach [`Connection::send_message`] through it. Same
/// arguments and [`TransportError`] contract as `send_message`.
///
/// Reply is **best-effort in P1.2.2**: the fd is non-blocking, so a send can
/// fail with `EAGAIN` ([`TransportError::Io`]) if the peer's receive buffer is
/// full. The per-connection send queue that parks and flushes such a reply is
/// P1.2.3; handshake-shaped replies fit the socket buffer and do not hit this.
pub fn reply(
    conn: &mut NoIoDrop<Connection>,
    frame: &[u8],
    fd: Option<BorrowedFd<'_>>,
) -> Result<(), TransportError> {
    // SAFETY: send_message borrows the connection only for the send and
    // neither drops nor moves it; the Connection stays owned by the source's
    // `Generic` (and registered with the poller) across the call.
    unsafe { conn.get_mut() }.send_message(frame, fd)
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
