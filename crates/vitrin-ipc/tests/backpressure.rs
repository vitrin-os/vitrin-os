//! P1.2.3 acceptance: backpressure & misbehavior policy. A client that stops
//! reading is disconnected at the send-queue cap; oversized frames and
//! fd-bombs terminate the connection with a logged reason; and the compositor
//! loop is never blocked while a misbehaving client is handled.
//!
//! Same harness shape as `event_loop.rs`: the whole server side runs on a
//! single thread inside one calloop [`EventLoop`]; clients are threads using
//! the blocking [`Connection`] API (or raw syscalls, for the hostile cases).
//!
//! Gated on the `server` feature; a `--features client` build compiles this
//! file to nothing.
#![cfg(feature = "server")]

use std::fmt::Write as _;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use calloop::{EventLoop, LoopHandle};
use vitrin_ipc::{
    Connection, ConnectionEvent, ConnectionSource, DisconnectReason, FrameHeader, Listener,
    ListenerEvent, ListenerSource, TransportError, HEADER_LEN, MAX_MESSAGE_SIZE,
    MAX_SEND_QUEUE_BYTES,
};
use vitrin_protocol::wire::patch_size;

/// The object id whose messages the test server answers with a hostile-scale
/// burst of max-size replies (must overflow the send queue); everything else
/// is echoed (opcode + 1).
const BURST_ID: u32 = 0xB0B;
/// Object id answered with exactly [`PARKED_FRAMES`] max-size replies: enough
/// to exceed the shrunken kernel buffer (so frames park), never enough to
/// exceed the cap (4 x 65 535 = 262 140 <= 262 144).
const PARK_ID: u32 = 0xAB;
const PARKED_FRAMES: usize = 4;

/// A unique scratch directory, short enough for `sun_path`.
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitrin-ipc-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One syntactically complete, fd-less frame.
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

/// A frame of exactly [`MAX_MESSAGE_SIZE`] bytes.
fn max_frame(object_id: u32) -> Vec<u8> {
    frame(object_id, 0, &vec![0xAAu8; MAX_MESSAGE_SIZE - HEADER_LEN])
}

/// Push raw bytes (no framing help) into the connection's socket.
fn raw_send(conn: &Connection, bytes: &[u8]) {
    let mut sent = 0;
    while sent < bytes.len() {
        sent += rustix::net::send(
            conn.as_fd(),
            &bytes[sent..],
            rustix::net::SendFlags::NOSIGNAL,
        )
        .unwrap();
    }
}

/// Push bytes plus an arbitrary fd list in one sendmsg, bypassing
/// `send_message`'s one-fd validation -- the hostile-peer harness.
fn raw_send_with_fds(conn: &Connection, bytes: &[u8], fds: &[BorrowedFd<'_>]) {
    use std::io::IoSlice;
    use std::mem::MaybeUninit;
    let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(32))];
    let mut cmsg = rustix::net::SendAncillaryBuffer::new(&mut space);
    assert!(cmsg.push(rustix::net::SendAncillaryMessage::ScmRights(fds)));
    let sent = rustix::net::sendmsg(
        conn.as_fd(),
        &[IoSlice::new(bytes)],
        &mut cmsg,
        rustix::net::SendFlags::NOSIGNAL,
    )
    .unwrap();
    assert_eq!(sent, bytes.len(), "test frames fit one sendmsg");
}

/// Server-side dispatch state shared by every test.
struct Server {
    handle: LoopHandle<'static, Server>,
    /// `(object_id, opcode)` per dispatched frame.
    received: Vec<(u32, u8)>,
    /// Every [`ConnectionEvent::Fault`] reason, in order.
    faults: Vec<DisconnectReason>,
    /// Clean [`ConnectionEvent::Disconnected`] count.
    disconnects: usize,
    /// Set once a `BURST_ID` reply loop has hit [`TransportError::SendQueueFull`].
    burst_overflowed: bool,
}

/// Start the standard test server on `path`: every incoming connection gets
/// its kernel send buffer shrunk to the minimum (so queueing is exercised
/// deterministically instead of hiding inside default ~208 KiB socket
/// buffers), then dispatches per the `BURST_ID`/`PARK_ID`/echo routing.
fn start_server(path: &Path) -> (EventLoop<'static, Server>, Server) {
    let listener = Listener::bind(path).unwrap();
    let event_loop: EventLoop<'static, Server> = EventLoop::try_new().unwrap();
    let handle = event_loop.handle();

    handle
        .insert_source(ListenerSource::new(listener).unwrap(), |event, _, state| {
            match event {
                ListenerEvent::Incoming(conn) => {
                    // Minimize the kernel-side slack (the kernel clamps to
                    // its floor, ~4 KiB): replies must park in the userspace
                    // queue almost immediately.
                    rustix::net::sockopt::set_socket_send_buffer_size(conn.as_fd(), 4096).unwrap();
                    let source = ConnectionSource::new(conn).unwrap();
                    state
                        .handle
                        .insert_source(source, |event, conn, state| match event {
                            ConnectionEvent::Message(msg) => {
                                state.received.push((msg.header.object_id, msg.header.opcode));
                                match msg.header.object_id {
                                    BURST_ID => {
                                        // Hostile-scale burst: reply until the
                                        // cap kills the connection. Bounded so
                                        // a broken cap fails the test instead
                                        // of hanging it.
                                        for i in 0..1024 {
                                            match vitrin_ipc::reply(conn, &max_frame(i), None) {
                                                Ok(()) => {}
                                                Err(TransportError::SendQueueFull { .. }) => {
                                                    state.burst_overflowed = true;
                                                    break;
                                                }
                                                Err(e) => panic!("unexpected reply error: {e}"),
                                            }
                                        }
                                        assert!(
                                            state.burst_overflowed,
                                            "64 MiB of replies must overflow the {MAX_SEND_QUEUE_BYTES}-byte cap"
                                        );
                                    }
                                    PARK_ID => {
                                        // A burst that parks but stays under
                                        // the cap: every reply must succeed.
                                        for i in 0..PARKED_FRAMES {
                                            vitrin_ipc::reply(conn, &max_frame(i as u32), None)
                                                .unwrap();
                                        }
                                    }
                                    _ => {
                                        let payload = &msg.bytes[HEADER_LEN..];
                                        let reply = frame(
                                            msg.header.object_id,
                                            msg.header.opcode + 1,
                                            payload,
                                        );
                                        vitrin_ipc::reply(conn, &reply, None).unwrap();
                                    }
                                }
                            }
                            ConnectionEvent::Disconnected => state.disconnects += 1,
                            ConnectionEvent::Fault(reason) => state.faults.push(reason),
                        })
                        .unwrap();
                }
                ListenerEvent::AcceptError(e) => panic!("unexpected accept error: {e}"),
            }
        })
        .unwrap();

    let state = Server {
        handle,
        received: Vec::new(),
        faults: Vec::new(),
        disconnects: 0,
        burst_overflowed: false,
    };
    (event_loop, state)
}

/// Run the loop in fixed slices until `done(&state)` or the deadline, then
/// return the state. Bounded so a wiring bug fails as a timeout, not a hang.
fn pump(
    event_loop: &mut EventLoop<'static, Server>,
    state: &mut Server,
    done: impl Fn(&Server) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(state) && Instant::now() < deadline {
        event_loop
            .dispatch(Some(Duration::from_millis(50)), state)
            .unwrap();
    }
}

/// A minimal `tracing` subscriber capturing every event as one line of
/// `level field=value ...` text, so tests can assert a termination was
/// actually *logged*, not just surfaced as a [`ConnectionEvent::Fault`].
struct LogCapture(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for LogCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Visitor<'a>(&'a mut String);
        impl tracing::field::Visit for Visitor<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let _ = write!(self.0, "{}={:?} ", field.name(), value);
            }
        }
        let mut line = format!("{} ", event.metadata().level());
        event.record(&mut Visitor(&mut line));
        self.0.lock().unwrap().push(line);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Install a capturing subscriber around `f` (the event loop runs on this
/// thread, so `with_default`'s thread-local scope sees every core-side log).
fn capture_logs(f: impl FnOnce()) -> Vec<String> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(LogCapture(lines.clone()), f);
    let lines = lines.lock().unwrap().clone();
    lines
}

fn assert_logged(lines: &[String], needle: &str) {
    assert!(
        lines
            .iter()
            .any(|l| l.contains("terminating connection") && l.contains(needle)),
        "expected a termination log mentioning {needle:?}; got: {lines:#?}"
    );
}

// --- Acceptance 1: slow reader hits the send-queue cap -> disconnected -----

#[test]
fn slow_reader_disconnected_at_send_queue_cap() {
    let path = scratch_dir("bp-slow").join("core.sock");
    let (mut event_loop, mut state) = start_server(&path);

    // The hostile client: request the burst, then never read a byte. It
    // holds its end open until released, so the disconnect the server sees
    // can only come from the slow-reader policy, not from EOF.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let client = {
        let path = path.clone();
        thread::spawn(move || {
            let mut c = Connection::connect(&path).unwrap();
            c.send_message(&frame(BURST_ID, 0, b"flood me"), None)
                .unwrap();
            release_rx.recv().unwrap();
        })
    };

    let lines = capture_logs(|| {
        pump(&mut event_loop, &mut state, |s| !s.faults.is_empty());
    });
    release_tx.send(()).unwrap();
    client.join().unwrap();

    assert!(state.burst_overflowed, "reply() must report SendQueueFull");
    assert!(
        matches!(
            state.faults.as_slice(),
            [DisconnectReason::SlowReader { queued }] if *queued > 0 && *queued <= MAX_SEND_QUEUE_BYTES
        ),
        "expected exactly one SlowReader fault, got {:?}",
        state.faults
    );
    assert_eq!(state.disconnects, 0, "policy kill, not a clean disconnect");
    assert_logged(&lines, "slow reader");
}

// --- Acceptance 3a: oversized message -> terminated + logged ---------------

#[test]
fn oversized_frame_terminates_and_logs() {
    let path = scratch_dir("bp-oversized").join("core.sock");
    let (mut event_loop, mut state) = start_server(&path);

    // The one size violation the u16 `size` field can express (see the
    // MAX_MESSAGE_SIZE rationale in lib.rs): a header declaring fewer bytes
    // than the header itself -- fatal `oversized` in the conventions'
    // taxonomy. Byte layout: object_id u32 | size u16 (LE, offset 4) |
    // opcode u8 | fd_count u8.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let client = {
        let path = path.clone();
        thread::spawn(move || {
            let c = Connection::connect(&path).unwrap();
            let mut bytes = frame(1, 0, &[]);
            bytes[4..6].copy_from_slice(&4u16.to_le_bytes());
            raw_send(&c, &bytes);
            // Keep the socket open: the fault must come from the size check,
            // not from any EOF race.
            release_rx.recv().unwrap();
        })
    };

    let lines = capture_logs(|| {
        pump(&mut event_loop, &mut state, |s| !s.faults.is_empty());
    });
    release_tx.send(()).unwrap();
    client.join().unwrap();

    assert!(
        matches!(state.faults.as_slice(), [DisconnectReason::Oversized(_)]),
        "expected exactly one Oversized fault, got {:?}",
        state.faults
    );
    assert_logged(&lines, "oversized");
}

// --- Acceptance 3b: fd-bomb -> terminated + logged -------------------------

#[test]
fn fd_bomb_terminates_and_logs() {
    let path = scratch_dir("bp-fdbomb").join("core.sock");
    let (mut event_loop, mut state) = start_server(&path);

    // Attach fds to a frame that declares none -- the "more fds than the
    // message schema allows" class. The transport detects it positionally
    // (conventions 2.4) and the policy layer classifies it FdBomb.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let client = {
        let path = path.clone();
        thread::spawn(move || {
            let c = Connection::connect(&path).unwrap();
            let (reader, writer) = std::io::pipe().unwrap();
            raw_send_with_fds(
                &c,
                &frame(9, 0, b"no fd declared"),
                &[reader.as_fd(), writer.as_fd()],
            );
            release_rx.recv().unwrap();
        })
    };

    let lines = capture_logs(|| {
        pump(&mut event_loop, &mut state, |s| !s.faults.is_empty());
    });
    release_tx.send(()).unwrap();
    client.join().unwrap();

    assert!(
        matches!(state.faults.as_slice(), [DisconnectReason::FdBomb(_)]),
        "expected exactly one FdBomb fault, got {:?}",
        state.faults
    );
    assert_logged(&lines, "fd bomb");
}

// --- Good-path backpressure: parked replies flush when the reader resumes --

#[test]
fn parked_replies_flush_when_reader_resumes() {
    let path = scratch_dir("bp-park").join("core.sock");
    let (mut event_loop, mut state) = start_server(&path);

    // A momentarily-slow but compliant client: request a burst that must
    // park (the server's kernel send buffer holds ~8 KiB of the ~256 KiB
    // burst), dawdle, then read everything. The parked remainder can only
    // arrive via flush-on-write-readiness.
    let (tx, rx) = mpsc::channel();
    let client = {
        let path = path.clone();
        thread::spawn(move || {
            let mut c = Connection::connect(&path).unwrap();
            c.send_message(&frame(PARK_ID, 0, b"stall then read"), None)
                .unwrap();
            thread::sleep(Duration::from_millis(200));
            for _ in 0..PARKED_FRAMES {
                let msg = c.recv_message().unwrap().expect("a parked reply");
                assert_eq!(msg.header.size as usize, MAX_MESSAGE_SIZE);
            }
            tx.send(()).unwrap();
            // Dropping `c` closes cleanly.
        })
    };

    pump(&mut event_loop, &mut state, |s| s.disconnects >= 1);
    rx.recv_timeout(Duration::from_secs(2))
        .expect("client did not receive all parked replies");
    client.join().unwrap();

    assert!(
        state.faults.is_empty(),
        "no fault expected: {:?}",
        state.faults
    );
    assert_eq!(state.disconnects, 1, "client must end with a clean close");
}

// --- Acceptance 2: the loop is not blocked while misbehavior is handled ----

#[test]
fn loop_stays_responsive_while_misbehaving_client_handled() {
    let path = scratch_dir("bp-timing").join("core.sock");
    let (mut event_loop, mut state) = start_server(&path);

    // Hostile client: triggers the flood, never reads, stays connected for
    // the whole test so its kernel buffers remain full.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let hostile = {
        let path = path.clone();
        thread::spawn(move || {
            let mut c = Connection::connect(&path).unwrap();
            c.send_message(&frame(BURST_ID, 0, b"flood me"), None)
                .unwrap();
            release_rx.recv().unwrap();
        })
    };

    // Handle the misbehavior (bounded): the whole burst + overflow + kill
    // must complete well inside the pump deadline -- a loop wedged in a
    // blocking send to the hostile client would time out here.
    let handled_in = {
        let start = Instant::now();
        pump(&mut event_loop, &mut state, |s| !s.faults.is_empty());
        start.elapsed()
    };
    assert!(
        matches!(
            state.faults.as_slice(),
            [DisconnectReason::SlowReader { .. }]
        ),
        "hostile client must die as a slow reader, got {:?}",
        state.faults
    );
    assert!(
        handled_in < Duration::from_secs(4),
        "misbehavior handling stalled the loop for {handled_in:?}"
    );

    // While the hostile client is still connected (buffers full, never
    // read), a well-behaved client's round-trips must complete promptly.
    let (tx, rx) = mpsc::channel();
    let polite = {
        let path = path.clone();
        thread::spawn(move || {
            let mut c = Connection::connect(&path).unwrap();
            for i in 0..3u8 {
                let start = Instant::now();
                c.send_message(&frame(7, i, b"ping"), None).unwrap();
                let reply = c.recv_message().unwrap().expect("echo reply");
                assert_eq!(reply.header.opcode, i + 1);
                tx.send(start.elapsed()).unwrap();
            }
        })
    };

    let mut latencies = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while latencies.len() < 3 && Instant::now() < deadline {
        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut state)
            .unwrap();
        while let Ok(latency) = rx.try_recv() {
            latencies.push(latency);
        }
    }
    assert_eq!(
        latencies.len(),
        3,
        "round-trips did not complete: loop blocked by the misbehaving client"
    );
    polite.join().unwrap();
    release_tx.send(()).unwrap();
    hostile.join().unwrap();

    for latency in latencies {
        assert!(
            latency < Duration::from_secs(2),
            "round-trip took {latency:?} with a misbehaving client present"
        );
    }
}
