// SPDX-License-Identifier: MPL-2.0
//! Acceptance criterion (issue #14): the open-fd count stays flat over
//! 10 000 messages, half of them carrying an fd via `SCM_RIGHTS`.
//!
//! This test lives in its own integration-test binary on purpose: it is the
//! only test in this process, so `/proc/self/fd` accounting cannot race
//! other tests opening and closing descriptors on parallel threads.

use std::os::fd::AsFd;

use vitrin_ipc::Connection;
use vitrin_protocol::wire::{patch_size, FrameHeader};

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

/// Number of open descriptors, via /proc/self/fd. The directory fd that
/// `read_dir` itself holds appears in every snapshot equally, so snapshots
/// are comparable.
fn open_fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}

#[test]
fn open_fd_count_flat_over_10k_messages() {
    let (mut a, mut b) = Connection::pair().unwrap();
    let with_fd = frame(1, 0, 1, &[0xab; 24]);
    let without_fd = frame(2, 1, 0, &[0xcd; 16]);

    // Warm-up: exercise both paths once so lazily-created descriptors (if
    // any) exist before the baseline snapshot.
    {
        let (_r, w) = std::io::pipe().unwrap();
        a.send_message(&with_fd, Some(w.as_fd())).unwrap();
        drop(w);
        let msg = b.recv_message().unwrap().unwrap();
        assert!(msg.fd.is_some());
    }

    let baseline = open_fd_count();

    for i in 0..10_000u32 {
        if i % 2 == 0 {
            // An fd-bearing message: a fresh pipe write-end crosses the
            // socket, is received CLOEXEC, and is dropped with the Message.
            let (r, w) = std::io::pipe().unwrap();
            a.send_message(&with_fd, Some(w.as_fd())).unwrap();
            drop(w);
            drop(r);
            let msg = b.recv_message().unwrap().unwrap();
            assert!(msg.fd.is_some(), "message {i} must carry its fd");
        } else {
            a.send_message(&without_fd, None).unwrap();
            let msg = b.recv_message().unwrap().unwrap();
            assert!(msg.fd.is_none());
        }
    }

    assert_eq!(
        open_fd_count(),
        baseline,
        "open-fd count must stay flat over 10k messages (5k of them fd-bearing)"
    );
}
