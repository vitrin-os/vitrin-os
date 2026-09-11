// SPDX-License-Identifier: MPL-2.0
//! The woken turn: [`Outbox::wake`] asks the loop for a dispatch of a
//! connection nobody is speaking on, and the source delivers it as
//! [`ConnectionEvent::Woken`].
//!
//! Why this needs its own acceptance file: everything else in this crate is
//! reactive -- a peer speaks, the core answers on the same turn. A picker's
//! confirm arrives on a *physical-input* turn instead, and the answer it must
//! write is a descriptor, which only [`vitrin_ipc::reply`] can send and only
//! a dispatch callback can reach. So the property under test is not "a frame
//! got through" but "an occasion to write appeared, on a socket that carried
//! no traffic to cause it".
//!
//! The harness is a socketpair ([`Connection::pair`]) with the core half
//! registered on a single-threaded [`EventLoop`] and the peer half left in
//! this thread, silent. No listener, no client thread: a peer that never
//! speaks is exactly the case these tests are about, so giving it a thread
//! would only add a way for the test to pass for the wrong reason.
//!
//! Gated on the `server` feature (the calloop glue); a `--features client`
//! build compiles this file to nothing.
#![cfg(feature = "server")]

use std::io;
use std::time::{Duration, Instant};

use calloop::generic::NoIoDrop;
use calloop::EventLoop;
use vitrin_ipc::{
    Connection, ConnectionEvent, ConnectionSource, FrameHeader, Message, Outbox, TransportError,
    HEADER_LEN,
};
use vitrin_protocol::wire::patch_size;

/// One syntactically complete, fd-less frame. The payload carries no protocol
/// meaning; the transport stays exercisable without any.
fn frame(object_id: u32, opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    FrameHeader {
        object_id,
        size: 0,
        opcode,
        fd_count: 0,
    }
    .encode_with_placeholder_size(&mut out);
    out.extend_from_slice(payload);
    patch_size(&mut out);
    out
}

/// What the core's dispatch callback saw, in the order it saw it. Ordering is
/// the point of one of these tests, so the log is a `Vec` and never a set.
#[derive(Debug, PartialEq, Eq)]
enum Ev {
    Message(u32, u8),
    Woken,
    Disconnected,
    Fault,
}

/// The core side's dispatch state.
struct Core {
    events: Vec<Ev>,
    /// A frame to write from inside the next [`ConnectionEvent::Woken`], if
    /// any. This is the whole reason the turn exists, so one test arms it and
    /// then proves the bytes reached a peer that said nothing.
    reply_on_wake: Option<Vec<u8>>,
}

impl Core {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            reply_on_wake: None,
        }
    }
}

/// The dispatch callback. Deliberately does *not* re-arm anything: a core
/// that ignores its wake must leave the loop quiescent, which `pump_quiet`
/// checks.
fn dispatch(event: ConnectionEvent, conn: &mut NoIoDrop<Connection>, core: &mut Core) {
    match event {
        ConnectionEvent::Message(msg) => core
            .events
            .push(Ev::Message(msg.header.object_id, msg.header.opcode)),
        ConnectionEvent::Woken => {
            core.events.push(Ev::Woken);
            if let Some(frame) = core.reply_on_wake.take() {
                vitrin_ipc::reply(conn, &frame, None).expect("reply from inside the woken turn");
            }
        }
        ConnectionEvent::Disconnected => core.events.push(Ev::Disconnected),
        ConnectionEvent::Fault(_) => core.events.push(Ev::Fault),
    }
}

/// A core-side [`ConnectionSource`] with an [`Outbox`], registered on a fresh
/// loop, plus the peer end of the socketpair.
///
/// The peer is switched to non-blocking: every read in this file is an
/// assertion about what did or did not arrive, and on a regression the honest
/// outcome is a failed assertion, not a test binary parked forever in
/// `recvmsg`.
fn wire() -> (EventLoop<'static, Core>, Outbox, Connection) {
    let (core_side, peer) = Connection::pair().unwrap();
    let flags = rustix::fs::fcntl_getfl(&peer).unwrap();
    rustix::fs::fcntl_setfl(&peer, flags | rustix::fs::OFlags::NONBLOCK).unwrap();

    let (source, outbox) = ConnectionSource::with_outbox(core_side).unwrap();
    let event_loop: EventLoop<'static, Core> = EventLoop::try_new().unwrap();
    event_loop.handle().insert_source(source, dispatch).unwrap();
    (event_loop, outbox, peer)
}

/// Dispatch in short slices until `done`, or fail. Bounded so a wiring
/// regression fails as an assertion rather than hanging the harness.
fn pump_until(
    event_loop: &mut EventLoop<'static, Core>,
    core: &mut Core,
    done: impl Fn(&Core) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(core) && Instant::now() < deadline {
        event_loop
            .dispatch(Some(Duration::from_millis(20)), core)
            .unwrap();
    }
    assert!(
        done(core),
        "condition not reached before the deadline; log = {:?}",
        core.events
    );
}

/// Dispatch a few more times and require that the log did not grow: nothing
/// was delivered that nobody asked for.
fn pump_quiet(event_loop: &mut EventLoop<'static, Core>, core: &mut Core) {
    let before = core.events.len();
    for _ in 0..3 {
        event_loop
            .dispatch(Some(Duration::from_millis(20)), core)
            .unwrap();
    }
    assert_eq!(
        core.events.len(),
        before,
        "unasked-for events delivered: {:?}",
        &core.events[before..]
    );
}

/// Read one frame from the peer within `budget`, or `None`. The peer is
/// non-blocking, so `WouldBlock` means "not yet", not "never".
fn recv_within(peer: &mut Connection, budget: Duration) -> Option<Message> {
    let deadline = Instant::now() + budget;
    loop {
        match peer.recv_message() {
            Ok(Some(msg)) => return Some(msg),
            Ok(None) => return None,
            Err(TransportError::Io(ref e)) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("peer receive failed: {e}"),
        }
    }
}

/// One `wake()` is one turn: it is delivered, it is delivered once, and a
/// loop that keeps running afterwards delivers nothing more. The second half
/// is what stops a core that ignores `Woken` from spinning the event loop --
/// the flag is cleared *before* the callback runs, so nothing the callback
/// does or fails to do can re-arm it.
#[test]
fn a_wake_delivers_exactly_one_turn() {
    let (mut ev, outbox, _peer) = wire();
    let mut core = Core::new();

    // Baseline: an unasked-for turn is not a thing that happens.
    pump_quiet(&mut ev, &mut core);

    outbox.wake();
    assert!(outbox.wake_pending(), "asked for, not yet delivered");
    pump_until(&mut ev, &mut core, |c| !c.events.is_empty());
    assert_eq!(core.events, vec![Ev::Woken]);
    assert!(
        !outbox.wake_pending(),
        "the delivery clears the request; a still-set flag would re-arm \
         write-interest and spin"
    );

    pump_quiet(&mut ev, &mut core);
    assert_eq!(core.events, vec![Ev::Woken], "one wake, one turn");

    // ...and the mechanism is not one-shot: a second ask is a second turn.
    outbox.wake();
    pump_until(&mut ev, &mut core, |c| c.events.len() >= 2);
    assert_eq!(core.events, vec![Ev::Woken, Ev::Woken]);
}

/// The whole point: a peer with an empty outbox, which has never spoken and
/// never will, is still handed a turn -- and the turn really is an occasion
/// to write, because the core writes on it and the silent peer receives it.
///
/// A wake is also not a queue entry: `pending()` is zero on both sides of it.
#[test]
fn a_wake_on_an_empty_outbox_still_produces_the_turn() {
    let (mut ev, outbox, mut peer) = wire();
    let mut core = Core::new();
    // The core has no other occasion on which to write this: the peer sends
    // nothing for it to reply to.
    core.reply_on_wake = Some(frame(9, 1, b"designated"));

    assert_eq!(outbox.pending(), 0, "nothing queued before");
    outbox.wake();
    assert_eq!(outbox.pending(), 0, "a wake is not a queued frame");

    pump_until(&mut ev, &mut core, |c| !c.events.is_empty());
    assert_eq!(core.events, vec![Ev::Woken]);

    let got = recv_within(&mut peer, Duration::from_secs(2))
        .expect("the frame written from inside the woken turn reached the silent peer");
    assert_eq!((got.header.object_id, got.header.opcode), (9, 1));
    assert_eq!(&got.bytes[HEADER_LEN..], b"designated");
    assert!(got.fd.is_none());
}

/// A frame that arrived in the same turn as the wake is dispatched **first**.
///
/// Both the inbound bytes and the ping are pending before the loop runs even
/// once, so calloop sees both fds ready in one poll and may report them in
/// either order. The ordering still holds either way, because the wake is
/// emitted from the *connection's* own dispatch and only after that
/// dispatch's receive drain: on the ping-first ordering the ping cannot emit
/// anything at all, and on the connection-first ordering the drain has
/// already run. That is what lets a core treat an inbound cancel as arriving
/// before the write it cancels.
#[test]
fn a_message_in_the_same_turn_is_delivered_before_the_wake() {
    let (mut ev, outbox, mut peer) = wire();
    let mut core = Core::new();

    peer.send_message(&frame(4, 2, b"cancel"), None).unwrap();
    outbox.wake();

    pump_until(&mut ev, &mut core, |c| c.events.len() >= 2);
    assert_eq!(core.events, vec![Ev::Message(4, 2), Ev::Woken]);
}

/// A wake asked for just before the last [`Outbox`] clone is dropped is still
/// delivered.
///
/// This is the case that decides where the flag lives. calloop's eventfd ping
/// folds "closed" into the same counter as "pinged", so the single dispatch
/// that observes this wake is also the one that retires the outbox sink. A
/// flag stored inside the sink would be destroyed by that retirement and the
/// turn would be lost without a trace; on the source, it outlives the sink
/// and is still read by the interest recomputation that delivers it.
#[test]
fn a_dropped_outbox_sink_does_not_lose_a_pending_wake() {
    let (mut ev, outbox, _peer) = wire();
    let mut core = Core::new();

    outbox.wake();
    drop(outbox);

    pump_until(&mut ev, &mut core, |c| !c.events.is_empty());
    assert_eq!(core.events, vec![Ev::Woken]);
}

/// Nothing in the wake path can carry a descriptor.
#[test]
fn nothing_in_the_wake_path_can_carry_an_fd() {
    // Compile-time half. `wake` takes `&self` and nothing else, and `Woken`
    // is a unit variant carrying no payload. Widen either -- an fd argument,
    // a frame, a field on the variant -- and these two lines stop compiling,
    // which is the earliest and loudest place to catch it.
    let _: fn(&Outbox) = Outbox::wake;
    let _: ConnectionEvent = ConnectionEvent::Woken;

    // Run-time half. A wake puts *nothing* on the wire, so there is no
    // message for an ancillary payload to ride: on a SOCK_STREAM socket the
    // kernel will not carry an `SCM_RIGHTS` control message without at least
    // one byte of ordinary data, so a peer that receives no bytes has
    // provably received no descriptor.
    let (mut ev, outbox, mut peer) = wire();
    let mut core = Core::new();

    outbox.wake();
    pump_until(&mut ev, &mut core, |c| !c.events.is_empty());
    assert_eq!(core.events, vec![Ev::Woken]);

    // Give a hypothetical stray write time to land before concluding it did
    // not happen; `recv_within` returning `None` here is the assertion.
    assert!(
        recv_within(&mut peer, Duration::from_millis(200)).is_none(),
        "the wake path put something on the wire"
    );
}

/// A wake is never delivered on a connection the dispatch has already
/// condemned, and never at all once the source has left the loop.
///
/// Both halves are claims the source's own docs make and nothing else here
/// checks. They matter because the caller of `wake` is holding something: for
/// the file picker, an already-open descriptor waiting for the turn that will
/// hand it over. If a wake could be delivered *after* the terminal event, the
/// core would be handed a write occasion on a corpse; if the core could tell a
/// lost turn from a pending one by looking at the `Outbox`, it would be
/// tempted to poll instead of releasing the fd on `Disconnected`. Neither is
/// true, and the second is the sharper edge, so it is asserted rather than
/// left to prose.
#[test]
fn a_wake_is_never_delivered_once_the_connection_is_condemned() {
    let (mut ev, outbox, peer) = wire();
    let mut core = Core::new();

    // Same turn: the ping and the peer's EOF are both pending before the loop
    // runs once, in either poll order. The drain reaches `Ok(None)` and sets
    // `PostAction::Remove`, and the emit is inside the `Continue` arm, so the
    // turn is dropped rather than handed out on a dying connection.
    outbox.wake();
    drop(peer);

    pump_until(&mut ev, &mut core, |c| !c.events.is_empty());
    assert_eq!(
        core.events,
        vec![Ev::Disconnected],
        "a condemned connection is not handed an occasion to write"
    );
    pump_quiet(&mut ev, &mut core);

    // After the source is gone the `Outbox` is still alive and `wake` is
    // still callable -- it just does nothing, for ever, with no way to say so.
    // A core waiting on `Woken` to release a descriptor would wait for ever;
    // it has to release on the terminal event above instead.
    outbox.wake();
    pump_quiet(&mut ev, &mut core);
    assert_eq!(core.events, vec![Ev::Disconnected]);
    assert!(
        outbox.wake_pending(),
        "the request stays set for good: `wake_pending` cannot distinguish a \
         pending turn from a lost one, which is why the fd is released on the \
         terminal event and not by polling this"
    );
}
