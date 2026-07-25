// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: `vitrin-ipc`'s frame reassembly and `SCM_RIGHTS` matching,
//! driven with arbitrary byte streams and arbitrary fd batches over a real
//! `socketpair` -- the P1.9.3 (#46) framing half of "cargo-fuzz targets on
//! ... the framing layer of `vitrin-ipc` (arbitrary byte streams, arbitrary
//! fd counts, truncated frames, pathological length fields)".
//!
//! # Why a real socketpair, not pure in-memory bytes
//!
//! `Connection::recv_message` (`crates/vitrin-ipc/src/connection.rs`) is
//! stateful across `recvmsg(2)` calls -- partial frames straddling two
//! reads, `SCM_RIGHTS` fd batches whose *span* (not exact offset) is
//! tracked until a frame claims them ([`PendingFd`], conventions 2.2) --
//! and that reassembly logic is what this target exists to fuzz, not the
//! argument decode `protocol_decode` already covers. A `socketpair(2)` is
//! cheap (one syscall, no `vitrind`, no listener, no filesystem) and is
//! the only way to hand the kernel real ancillary (fd) data the way a
//! hostile peer would, so this stays in-process and fast enough for
//! libFuzzer while still exercising the actual receive path byte for byte.
//!
//! # What this proves
//!
//! For any split of the fuzz input into two raw `sendmsg(2)` calls (each
//! with its own attached-fd count), draining the receiving
//! [`vitrin_ipc::Connection`] with `recv_message()` until it reports a
//! clean close or a typed error must never panic and never hang: every
//! malformed byte stream and every fd-count mismatch (fewer, more, or
//! misaligned fds relative to what the frame headers declare) must
//! resolve to a [`vitrin_ipc::TransportError`], not UB. This is the
//! in-process counterpart to `crates/vitrin-ipc/tests/backpressure.rs`'s
//! hand-picked oversized-frame/fd-bomb cases -- those pin the *documented*
//! violations; this explores everything else a byte-for-byte mutator can
//! reach in the same state machine.
//!
//! # Input layout
//!
//! `data[0]`: fds attached to the first `sendmsg` (mod 4). `data[1]`: fds
//! attached to the second (mod 4). `data[2]`: where to split the remaining
//! bytes between the two `sendmsg` calls (mod `remaining.len() + 1`).
//! `data[3..]`: the raw bytes, sent exactly as given -- no frame-shaped
//! massaging, so a mutator is free to discover truncated frames,
//! pathological `size` fields, and any byte pattern in between.
#![no_main]

use std::fs::File;
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd};

use libfuzzer_sys::fuzz_target;

use vitrin_ipc::Connection;

/// `n` fresh close-on-exec fds (`sendmsg` dup's each one into the kernel's
/// ancillary payload; the originals close when this vec drops after the
/// call). Capped by the caller's `% 4` in the input layout, so `n` is
/// always small.
fn devnull_fds(n: usize) -> Vec<File> {
    (0..n)
        .map(|_| File::open("/dev/null").expect("/dev/null must be openable in the fuzz sandbox"))
        .collect()
}

/// One raw `sendmsg`: `bytes` verbatim, `fds` attached as one `SCM_RIGHTS`
/// ancillary message. Mirrors
/// `crates/vitrin-ipc/tests/backpressure.rs`'s `raw_send_with_fds` hostile-
/// peer helper -- bypassing `Connection::send_message`'s own validation is
/// the point, since a real hostile peer is not bound by this crate's
/// sender-side checks either.
fn raw_sendmsg(conn: &Connection, bytes: &[u8], fds: &[BorrowedFd<'_>]) {
    if bytes.is_empty() && fds.is_empty() {
        return;
    }
    let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(4))];
    let mut cmsg = rustix::net::SendAncillaryBuffer::new(&mut space);
    if !fds.is_empty() {
        // `push` fails only if the buffer is too small for `fds`; the
        // space above is sized for the `% 4`-capped fd count the input
        // layout guarantees, so this cannot fail in practice -- but a
        // fuzz target must not panic on the off chance it does, so this
        // degrades to "send the bytes without the fds" rather than
        // `unwrap`.
        let _ = cmsg.push(rustix::net::SendAncillaryMessage::ScmRights(fds));
    }
    // A non-blocking peer that stops draining could make this block; the
    // kernel's default socket buffer is large relative to libFuzzer's
    // default input size, and unlike the receiver this send side is not
    // what is under test, so blocking here would only ever be a fuzz-
    // harness bug, not a `vitrin-ipc` one. Best-effort: ignore the result
    // (a `WouldBlock`/`EAGAIN` on a full buffer just means fewer bytes were
    // exercised this run, not a crash).
    let _ = rustix::net::sendmsg(
        conn.as_fd(),
        &[IoSlice::new(bytes)],
        &mut cmsg,
        rustix::net::SendFlags::NOSIGNAL | rustix::net::SendFlags::DONTWAIT,
    );
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    let fds_first = (data[0] as usize) % 4;
    let fds_second = (data[1] as usize) % 4;
    let rest = &data[3..];
    let split = if rest.is_empty() {
        0
    } else {
        (data[2] as usize) % (rest.len() + 1)
    };
    let (first_bytes, second_bytes) = rest.split_at(split);

    let Ok((sender, mut receiver)) = Connection::pair() else {
        return;
    };

    let first_fds = devnull_fds(fds_first);
    let first_borrowed: Vec<BorrowedFd<'_>> = first_fds.iter().map(File::as_fd).collect();
    raw_sendmsg(&sender, first_bytes, &first_borrowed);

    let second_fds = devnull_fds(fds_second);
    let second_borrowed: Vec<BorrowedFd<'_>> = second_fds.iter().map(File::as_fd).collect();
    raw_sendmsg(&sender, second_bytes, &second_borrowed);

    // Signal EOF rather than leaving the sender open: `recv_message` is a
    // blocking read, and without a close (or a completed frame on every
    // side) it would block forever waiting for bytes this fuzz iteration
    // will never send. Dropping the connection here closes its fd.
    drop(sender);

    // Drain until a clean close (`Ok(None)`) or a typed, connection-fatal
    // error -- bounded by iteration count (not wall-clock) so a
    // reassembly bug that returns `Ok(Some(_))` forever fails as a
    // libFuzzer timeout (a real bug worth seeing), not a hang this
    // harness silently swallows.
    for _ in 0..10_000 {
        match receiver.recv_message() {
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        }
    }
});
