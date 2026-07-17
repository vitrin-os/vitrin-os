//! Transport behavior over real sockets: fd round-trips, peer credentials,
//! frame reassembly, violation handling, CLOEXEC hygiene, listener
//! lifecycle. The fd-leak acceptance test lives alone in `fd_leak.rs` so
//! its `/proc/self/fd` accounting cannot race this binary's parallel tests.

use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::PathBuf;

use rustix::io::{fcntl_getfd, FdFlags};
use vitrin_ipc::{
    Connection, FrameHeader, Listener, LocalMisuse, PeerViolation, TransportError,
    MAX_UNCLAIMED_FDS,
};
use vitrin_protocol::generated::vitrin_view;
use vitrin_protocol::wire::patch_size;

/// Build one syntactically complete frame. The payload carries no protocol
/// meaning -- this crate must stay exercisable without any.
fn frame(object_id: u32, opcode: u8, fd_count: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    FrameHeader {
        object_id,
        size: 0,
        opcode,
        fd_count,
    }
    .encode_with_placeholder_size(&mut out);
    out.extend_from_slice(payload);
    patch_size(&mut out);
    out
}

fn assert_cloexec(fd: BorrowedFd<'_>, what: &str) {
    assert!(
        fcntl_getfd(fd).unwrap().contains(FdFlags::CLOEXEC),
        "{what} must be close-on-exec"
    );
}

fn self_cred() -> (i32, u32, u32) {
    (
        std::process::id() as i32,
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    )
}

/// A unique scratch directory; short enough for sun_path.
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitrin-ipc-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Push raw bytes (no framing help, no ancillary) into one end's socket.
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

/// Push bytes plus an arbitrary fd list in a single sendmsg, bypassing
/// `send_message`'s one-fd validation -- the hostile-peer harness.
fn raw_send_with_fds(conn: &Connection, bytes: &[u8], fds: &[BorrowedFd<'_>]) {
    use std::io::IoSlice;
    use std::mem::MaybeUninit;
    // Space for plenty of fds; tests stay well under this.
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
    assert_eq!(
        sent,
        bytes.len(),
        "test frames are small enough for one sendmsg"
    );
}

// --- Acceptance: fd round-trip across a socketpair via SCM_RIGHTS ---------

#[test]
fn fd_round_trip_across_socketpair() {
    let (mut a, mut b) = Connection::pair().unwrap();
    let (mut reader, writer) = std::io::pipe().unwrap();

    let f = frame(7, 3, 1, &[1, 2, 3, 4]);
    a.send_message(&f, Some(writer.as_fd())).unwrap();
    drop(writer); // sender's copy gone; the in-flight duplicate survives

    let msg = b.recv_message().unwrap().expect("one message");
    assert_eq!(msg.header.object_id, 7);
    assert_eq!(msg.header.opcode, 3);
    assert_eq!(msg.header.fd_count, 1);
    assert_eq!(msg.bytes, f);

    // The received fd is live: write through it, read from the local end.
    let received = msg.fd.expect("fd must accompany an fd_count=1 frame");
    assert_cloexec(received.as_fd(), "fd received via SCM_RIGHTS");
    let mut through: fs::File = received.into();
    through.write_all(b"ping").unwrap();
    drop(through);
    let mut got = String::new();
    reader.read_to_string(&mut got).unwrap();
    assert_eq!(got, "ping");
}

/// Same seam the real stack uses: a generated fd-bearing message
/// (`vitrin_view.frame_ready`) encoded by vitrin-protocol, moved by this
/// crate, decoded by vitrin-protocol -- transport and codec agree on where
/// the fd lives (out-of-band, never in the byte buffer).
#[test]
fn generated_fd_message_end_to_end() {
    let (mut a, mut b) = Connection::pair().unwrap();
    let (mut reader, writer) = std::io::pipe().unwrap();

    let event = vitrin_view::events::FrameReady {
        fd: writer.into(),
        format: vitrin_view::Format::Xrgb8888,
        width: 1280,
        height: 800,
        stride: 5120,
        flags: vitrin_view::FrameFlags::default(),
    };
    let bytes = event.encode(42);
    a.send_message(&bytes, Some(event.fd.as_fd())).unwrap();
    drop(event);

    let msg = b.recv_message().unwrap().unwrap();
    let (object_id, decoded) = vitrin_view::events::FrameReady::decode(&msg.bytes, msg.fd).unwrap();
    assert_eq!(object_id, 42);
    assert_eq!(decoded.width, 1280);
    assert_eq!(decoded.stride, 5120);

    let mut through: fs::File = decoded.fd.into();
    through.write_all(b"frame").unwrap();
    drop(through);
    let mut got = String::new();
    reader.read_to_string(&mut got).unwrap();
    assert_eq!(got, "frame");
}

// --- Acceptance: SO_PEERCRED recorded and readable ------------------------

#[test]
fn peer_cred_on_socketpair() {
    let (a, b) = Connection::pair().unwrap();
    let (pid, uid, gid) = self_cred();
    for conn in [&a, &b] {
        let cred = conn.peer_cred();
        assert_eq!(cred.pid, pid);
        assert_eq!(cred.uid, uid);
        assert_eq!(cred.gid, gid);
    }
}

#[test]
fn peer_cred_recorded_at_accept() {
    let dir = scratch_dir("peercred");
    let path = dir.join("core.sock");
    let listener = Listener::bind(&path).unwrap();

    let client = std::thread::spawn({
        let path = path.clone();
        move || {
            let mut c = Connection::connect(&path).unwrap();
            c.send_message(&frame(1, 0, 0, &[]), None).unwrap();
            c
        }
    });

    let mut accepted = listener.accept().unwrap();
    let (pid, uid, gid) = self_cred();
    let cred = accepted.peer_cred();
    assert_eq!((cred.pid, cred.uid, cred.gid), (pid, uid, gid));

    // Credentials were captured at accept, but the connection also works.
    let msg = accepted.recv_message().unwrap().unwrap();
    assert_eq!(msg.header.object_id, 1);

    let client = client.join().unwrap();
    let cred = client.peer_cred();
    assert_eq!((cred.pid, cred.uid, cred.gid), (pid, uid, gid));

    drop(listener);
    fs::remove_dir_all(&dir).unwrap();
}

// --- Framing over a byte stream -------------------------------------------

#[test]
fn coalesced_and_fragmented_frames_reassemble() {
    let (a, mut b) = Connection::pair().unwrap();

    let f1 = frame(1, 0, 0, &[0xaa; 12]);
    let f2 = frame(2, 1, 0, &[0xbb; 4]);
    let f3 = frame(3, 2, 0, &[0xdd; 4]);

    // f1 and f2 coalesced into a single write; f3 split mid-header and
    // mid-payload across three writes.
    let mut coalesced = f1.clone();
    coalesced.extend_from_slice(&f2);
    raw_send(&a, &coalesced);
    raw_send(&a, &f3[..5]);
    raw_send(&a, &f3[5..9]);
    raw_send(&a, &f3[9..]);
    drop(a);

    let got1 = b.recv_message().unwrap().unwrap();
    let got2 = b.recv_message().unwrap().unwrap();
    let got3 = b.recv_message().unwrap().unwrap();
    assert_eq!((got1.header.object_id, got1.bytes), (1, f1));
    assert_eq!((got2.header.object_id, got2.bytes), (2, f2));
    assert_eq!((got3.header.object_id, got3.bytes), (3, f3));

    // Peer closed cleanly between frames.
    assert!(b.recv_message().unwrap().is_none());
}

#[test]
fn eof_mid_frame_is_an_error() {
    let (a, mut b) = Connection::pair().unwrap();
    let f = frame(9, 0, 0, &[0xcc; 16]);
    raw_send(&a, &f[..10]);
    drop(a);
    match b.recv_message() {
        Err(TransportError::Eof { buffered: 10 }) => {}
        other => panic!("expected Eof {{ buffered: 10 }}, got {other:?}"),
    }
}

// --- Peer violations kill (and poison) the connection ---------------------

#[test]
fn undersized_size_field_is_a_violation_and_poisons() {
    let (a, mut b) = Connection::pair().unwrap();
    // Hand-craft a header whose size field (4) is below the 8-byte minimum.
    let mut bytes = Vec::new();
    FrameHeader {
        object_id: 1,
        size: 0,
        opcode: 0,
        fd_count: 0,
    }
    .encode_with_placeholder_size(&mut bytes);
    bytes[4..6].copy_from_slice(&4u16.to_le_bytes());
    raw_send(&a, &bytes);

    for attempt in 0..2 {
        match b.recv_message() {
            Err(TransportError::PeerViolation(PeerViolation::UndersizedSizeField { size: 4 })) => {}
            other => panic!("attempt {attempt}: expected UndersizedSizeField, got {other:?}"),
        }
    }
}

#[test]
fn fd_count_above_one_is_a_violation() {
    let (a, mut b) = Connection::pair().unwrap();
    raw_send(&a, &frame(1, 0, 2, &[]));
    match b.recv_message() {
        Err(TransportError::PeerViolation(PeerViolation::FdCountExceeded { fd_count: 2 })) => {}
        other => panic!("expected FdCountExceeded, got {other:?}"),
    }
}

#[test]
fn declared_fd_that_never_arrives_is_a_violation() {
    let (a, mut b) = Connection::pair().unwrap();
    raw_send(&a, &frame(1, 0, 1, &[]));
    match b.recv_message() {
        Err(TransportError::PeerViolation(PeerViolation::MissingFd)) => {}
        other => panic!("expected MissingFd, got {other:?}"),
    }
}

#[test]
fn fd_bomb_in_one_sendmsg_is_a_violation() {
    let (a, mut b) = Connection::pair().unwrap();
    // More fds in one sendmsg than the receive ancillary buffer admits.
    let pipes: Vec<_> = (0..MAX_UNCLAIMED_FDS + 4)
        .map(|_| std::io::pipe().unwrap())
        .collect();
    let fds: Vec<BorrowedFd<'_>> = pipes.iter().map(|(r, _)| r.as_fd()).collect();
    raw_send_with_fds(&a, &frame(1, 0, 0, &[]), &fds);
    match b.recv_message() {
        Err(TransportError::PeerViolation(PeerViolation::AncillaryTruncated)) => {}
        other => panic!("expected AncillaryTruncated, got {other:?}"),
    }
}

#[test]
fn unclaimed_fd_buildup_is_a_violation() {
    let (a, mut b) = Connection::pair().unwrap();
    let pipes: Vec<_> = (0..MAX_UNCLAIMED_FDS + 1)
        .map(|_| std::io::pipe().unwrap())
        .collect();

    // Each message smuggles one fd its header never claims (fd_count=0).
    // The transport tolerates a queue of MAX_UNCLAIMED_FDS, then kills.
    for (i, (r, _)) in pipes.iter().enumerate() {
        raw_send_with_fds(&a, &frame(i as u32, 0, 0, &[]), &[r.as_fd()]);
    }
    for i in 0..MAX_UNCLAIMED_FDS {
        let msg = b.recv_message().unwrap().unwrap();
        assert_eq!(msg.header.object_id, i as u32);
        assert!(msg.fd.is_none());
    }
    match b.recv_message() {
        Err(TransportError::PeerViolation(PeerViolation::UnclaimedFdOverflow)) => {}
        other => panic!("expected UnclaimedFdOverflow, got {other:?}"),
    }
}

// --- send_message validates before writing --------------------------------

#[test]
fn send_message_rejects_contradictory_frames() {
    let (mut a, _b) = Connection::pair().unwrap();
    let (reader, _writer) = std::io::pipe().unwrap();

    // Too short for a header.
    match a.send_message(&[0u8; 4], None) {
        Err(TransportError::LocalMisuse(LocalMisuse::FrameTooShort { len: 4 })) => {}
        other => panic!("expected FrameTooShort, got {other:?}"),
    }

    // Size field disagrees with the buffer length.
    let f = frame(1, 0, 0, &[0; 8]);
    match a.send_message(&f[..f.len() - 4], None) {
        Err(TransportError::LocalMisuse(LocalMisuse::SizeFieldMismatch {
            declared: 16,
            actual: 12,
        })) => {}
        other => panic!("expected SizeFieldMismatch, got {other:?}"),
    }

    // Declared fd without an attached one, and vice versa.
    match a.send_message(&frame(1, 0, 1, &[]), None) {
        Err(TransportError::LocalMisuse(LocalMisuse::FdCountMismatch {
            declared: 1,
            attached: false,
        })) => {}
        other => panic!("expected FdCountMismatch, got {other:?}"),
    }
    match a.send_message(&frame(1, 0, 0, &[]), Some(reader.as_fd())) {
        Err(TransportError::LocalMisuse(LocalMisuse::FdCountMismatch {
            declared: 0,
            attached: true,
        })) => {}
        other => panic!("expected FdCountMismatch, got {other:?}"),
    }

    // fd_count beyond the v0 maximum, even with an fd attached.
    match a.send_message(&frame(1, 0, 3, &[]), Some(reader.as_fd())) {
        Err(TransportError::LocalMisuse(LocalMisuse::FdCountMismatch {
            declared: 3,
            attached: true,
        })) => {}
        other => panic!("expected FdCountMismatch, got {other:?}"),
    }
}

// --- CLOEXEC hygiene -------------------------------------------------------

#[test]
fn every_created_fd_is_cloexec() {
    let (a, b) = Connection::pair().unwrap();
    assert_cloexec(a.as_fd(), "socketpair end a");
    assert_cloexec(b.as_fd(), "socketpair end b");

    let dir = scratch_dir("cloexec");
    let path = dir.join("core.sock");
    let listener = Listener::bind(&path).unwrap();
    assert_cloexec(listener.as_fd(), "listening socket");

    let client = std::thread::spawn({
        let path = path.clone();
        move || Connection::connect(&path).unwrap()
    });
    let accepted = listener.accept().unwrap();
    assert_cloexec(accepted.as_fd(), "accepted socket");
    let client = client.join().unwrap();
    assert_cloexec(client.as_fd(), "connecting socket");

    drop(listener);
    fs::remove_dir_all(&dir).unwrap();
}

// --- Listener lifecycle ----------------------------------------------------

#[test]
fn listener_recovers_stale_socket_but_respects_live_one() {
    let dir = scratch_dir("listener");
    let path = dir.join("core.sock");

    // A crashed predecessor: socket file present, no lock held.
    {
        let fd = rustix::net::socket_with(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::STREAM,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let addr = rustix::net::SocketAddrUnix::new(&path).unwrap();
        rustix::net::bind(&fd, &addr).unwrap();
        // fd closes here; the socket file stays behind.
    }
    assert!(path.exists(), "stale socket file left on disk");
    let listener = Listener::bind(&path).expect("stale socket must be reclaimed");

    // While it lives, a second bind must refuse.
    match Listener::bind(&path) {
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse),
        Ok(_) => panic!("second bind on a live socket must fail"),
    }

    // Dropping cleans up socket and lock file; a rebind then succeeds.
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    assert!(lock_path.exists());
    drop(listener);
    assert!(!path.exists(), "socket file must be unlinked on drop");
    assert!(!lock_path.exists(), "lock file must be unlinked on drop");
    let again = Listener::bind(&path).unwrap();
    drop(again);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn listener_creates_runtime_dir_mode_0700() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch_dir("perms");
    let dir = root.join("nested").join("vitrin-0");
    let path = dir.join("core.sock");
    assert!(!dir.exists());
    let listener = Listener::bind(&path).unwrap();
    let mode = fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o700,
        "runtime dir must be private to the user"
    );
    assert_eq!(listener.socket_path(), path);
    drop(listener);
    fs::remove_dir_all(&root).unwrap();
}

// --- Bidirectional sanity ---------------------------------------------------

#[test]
fn both_directions_work_on_one_connection() {
    let (mut a, mut b) = Connection::pair().unwrap();
    a.send_message(&frame(1, 0, 0, b"ping"), None).unwrap();
    let got = b.recv_message().unwrap().unwrap();
    assert_eq!(&got.bytes[8..], b"ping");
    b.send_message(&frame(2, 0, 0, b"pong"), None).unwrap();
    let got = a.recv_message().unwrap().unwrap();
    assert_eq!(&got.bytes[8..], b"pong");
}
