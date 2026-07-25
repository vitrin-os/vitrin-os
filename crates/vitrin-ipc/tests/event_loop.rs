// SPDX-License-Identifier: MPL-2.0
//! P1.2.2 acceptance: the transport plugs into a calloop event loop as an
//! ordinary event source. Both tests run the whole server side on a single
//! thread inside one [`EventLoop`] -- there is no dedicated IPC thread; the
//! only extra threads are the *clients*, standing in for separate principal
//! processes and using the plain blocking [`Connection`] API.
//!
//! Gated on the `server` feature (the calloop glue); a `--features client`
//! build compiles this file to nothing.
#![cfg(feature = "server")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use calloop::{EventLoop, LoopHandle};
use vitrin_ipc::{
    Connection, ConnectionEvent, ConnectionSource, FrameHeader, Listener, ListenerEvent,
    ListenerSource, HEADER_LEN,
};
use vitrin_protocol::wire::patch_size;

/// A unique scratch directory, short enough for `sun_path`. Distinct `tag`
/// per test so the parallel harness never shares a socket path.
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitrin-ipc-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One syntactically complete, fd-less frame. The payload carries no protocol
/// meaning -- the transport (and this test) stay exercisable without any.
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

/// The server side's dispatch state: a loop handle (so an accepted connection
/// can register its own read source on the same loop) plus a log of what the
/// core received, `(object_id, opcode, payload)` per frame.
struct Server {
    handle: LoopHandle<'static, Server>,
    received: Vec<(u32, u8, Vec<u8>)>,
}

/// Run `event_loop` in fixed slices until `done(&state)` or the deadline, then
/// return the state. Bounded so a wiring bug fails as a timeout, not a hang.
fn pump(
    mut event_loop: EventLoop<'static, Server>,
    mut state: Server,
    done: impl Fn(&Server) -> bool,
) -> Server {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(&state) && Instant::now() < deadline {
        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut state)
            .unwrap();
    }
    state
}

/// Acceptance criterion 1: the core accepts N concurrent connections and
/// dispatches their messages entirely inside its own event loop, with no
/// dedicated IPC thread.
#[test]
fn accepts_n_connections_in_one_event_loop() {
    const N: usize = 8;

    let path = scratch_dir("evloop-accept").join("core.sock");
    let listener = Listener::bind(&path).unwrap();

    let event_loop: EventLoop<'static, Server> = EventLoop::try_new().unwrap();
    let handle = event_loop.handle();

    // The listener source: each incoming connection gets its own read source
    // registered on the same loop from within this callback.
    handle
        .insert_source(
            ListenerSource::new(listener).unwrap(),
            |event, _, state| match event {
                ListenerEvent::Incoming(conn) => {
                    let source = ConnectionSource::new(conn).unwrap();
                    state
                        .handle
                        .insert_source(source, |event, _conn, state| {
                            if let ConnectionEvent::Message(msg) = event {
                                state.received.push((
                                    msg.header.object_id,
                                    msg.header.opcode,
                                    msg.bytes[HEADER_LEN..].to_vec(),
                                ));
                            }
                        })
                        .unwrap();
                }
                ListenerEvent::AcceptError(e) => panic!("unexpected accept error: {e}"),
            },
        )
        .unwrap();

    // N clients, one per thread, each a separate blocking principal: connect,
    // send one frame tagged with its index, drop. Spawned before the loop
    // runs; the connections queue in the listen backlog until accepted.
    let clients: Vec<_> = (0..N)
        .map(|i| {
            let path = path.clone();
            thread::spawn(move || {
                let mut c = Connection::connect(&path).unwrap();
                c.send_message(&frame(i as u32, 0, format!("hello-{i}").as_bytes()), None)
                    .unwrap();
            })
        })
        .collect();

    let state = pump(
        event_loop,
        Server {
            handle,
            received: Vec::new(),
        },
        |s| s.received.len() >= N,
    );
    for c in clients {
        c.join().unwrap();
    }

    assert_eq!(
        state.received.len(),
        N,
        "every connection's frame dispatched"
    );
    let mut ids: Vec<u32> = state.received.iter().map(|(id, _, _)| *id).collect();
    ids.sort_unstable();
    assert_eq!(ids, (0..N as u32).collect::<Vec<_>>());
    for (id, _op, payload) in &state.received {
        assert_eq!(payload, format!("hello-{id}").as_bytes());
    }
}

/// Acceptance criterion 2: the blocking client API round-trips request/reply
/// against the event-loop server, which replies from inside the dispatch
/// callback on the same connection.
#[test]
fn blocking_client_round_trips_against_event_loop_server() {
    let path = scratch_dir("evloop-roundtrip").join("core.sock");
    let listener = Listener::bind(&path).unwrap();

    let event_loop: EventLoop<'static, Server> = EventLoop::try_new().unwrap();
    let handle = event_loop.handle();

    // Echo server: reply on the same connection, opcode+1 marking the reply.
    handle
        .insert_source(ListenerSource::new(listener).unwrap(), |event, _, state| {
            if let ListenerEvent::Incoming(conn) = event {
                state
                    .handle
                    .insert_source(
                        ConnectionSource::new(conn).unwrap(),
                        |event, conn, state| {
                            if let ConnectionEvent::Message(msg) = event {
                                let payload = msg.bytes[HEADER_LEN..].to_vec();
                                state.received.push((
                                    msg.header.object_id,
                                    msg.header.opcode,
                                    payload.clone(),
                                ));
                                let reply_frame =
                                    frame(msg.header.object_id, msg.header.opcode + 1, &payload);
                                vitrin_ipc::reply(conn, &reply_frame, None).unwrap();
                            }
                        },
                    )
                    .unwrap();
            }
        })
        .unwrap();

    // Blocking client on its own thread: send a request, block on the reply,
    // hand it back over a channel, then flag completion. `done` is set only
    // after the reply arrived, which the server could only have sent from
    // inside its dispatch -- so observing `done` means the round-trip closed.
    let done = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let client = {
        let path = path.clone();
        let done = done.clone();
        thread::spawn(move || {
            let mut c = Connection::connect(&path).unwrap();
            c.send_message(&frame(7, 3, b"ping"), None).unwrap();
            let reply = c.recv_message().unwrap().unwrap();
            tx.send((
                reply.header.object_id,
                reply.header.opcode,
                reply.bytes[HEADER_LEN..].to_vec(),
            ))
            .unwrap();
            done.store(true, Ordering::SeqCst);
        })
    };

    let state = pump(
        event_loop,
        Server {
            handle,
            received: Vec::new(),
        },
        {
            let done = done.clone();
            move |_| done.load(Ordering::SeqCst)
        },
    );
    // Check the result on a short timeout *before* joining: on the happy path
    // `done` is already set, so the reply is waiting and `recv_timeout`
    // returns at once; on a regression where the server never replies, the
    // client is still parked in a blocking `recv_message`, so joining first
    // would wedge until the CI job cap -- fail fast here instead.
    let reply = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("round-trip did not complete before deadline");
    client.join().unwrap();
    assert_eq!(
        reply,
        (7, 4, b"ping".to_vec()),
        "reply echoes the request, opcode+1"
    );
    assert_eq!(
        state.received,
        vec![(7u32, 3u8, b"ping".to_vec())],
        "server dispatched exactly the one request frame"
    );
}
