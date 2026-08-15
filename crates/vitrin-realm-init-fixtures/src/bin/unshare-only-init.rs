// SPDX-License-Identifier: Apache-2.0
//! `unshare-only-init` -- a substituted `--realm-init` that unshares all six
//! namespaces, mounts **nothing**, and reports a perfectly well-formed
//! handshake.
//!
//! # What it is for
//!
//! D-036 clause 4 says that "verifying six namespace inodes differ" cannot
//! carry a confinement claim, because a helper could unshare six namespaces
//! and build no filesystem at all. This binary *is* that helper. It exists so
//! `vitrin_core::spawn::verify_root_view`'s `st_dev` comparison has a test
//! that fails when the comparison is deleted -- which the confined-spawn
//! tests against the real helper provably do not (they stay green with the
//! guard removed, because a correct helper never trips it).
//!
//! It gets through:
//!
//! - C1, the version handshake (it links the real `Config` type, so a schema
//!   change breaks it loudly rather than silently);
//! - C2, the six-flag `unshare` -- genuinely, so the core's P13 inode reads
//!   all pass;
//! - C4/C5, the map exchange;
//! - C7/S1/K9, the fork, the `CHILD` frame and a `MOUNTED` frame.
//!
//! And then it stops. The core must refuse at P20 with `cause_class =
//! "root_view"`, because the PID-1 child's root is still the host's.
//!
//! **It never `execve`s anything.** A fixture that went on to run the shim
//! would be a program that produces an unconfined realm on a machine where
//! somebody ran the wrong test, and no assertion is worth that.

use std::os::fd::RawFd;

use vitrin_realm_init::{Frame, CONFIG_MAX, PRE_EXEC_EXIT};

const CLONE_FLAGS: libc::c_int = libc::CLONE_NEWUSER
    | libc::CLONE_NEWNS
    | libc::CLONE_NEWPID
    | libc::CLONE_NEWIPC
    | libc::CLONE_NEWUTS
    | libc::CLONE_NEWNET;

fn main() {
    // Same fd-0 discipline as the real helper: the config channel arrives on
    // stdin and is moved off it at once, so the core's EOF still means what
    // it means.
    let cfg = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
    if cfg < 3 {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
        if null < 0 || libc::dup3(null, 0, 0) < 0 {
            libc::_exit(PRE_EXEC_EXIT)
        }
        libc::close(null);
    }

    // C1. The `CONFIG` frame, decoded through the real codec -- so a schema
    // change breaks this fixture at compile time or at decode time rather
    // than letting it drift into speaking a dialect of its own.
    //
    // The *version* comparison the real helper makes is deliberately absent:
    // this binary's job is to reach P20, and refusing on a version skew
    // between two crates that are always released together would only turn a
    // checkpoint test into a version test.
    let Some(Frame::Config(_config)) = recv(cfg) else {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    };

    // C2. Real, and that is the whole point of the fixture.
    if unsafe { libc::unshare(CLONE_FLAGS) } < 0 {
        // This machine cannot grant the namespaces. The core's own
        // `tests::namespaces()` verdict should already have skipped the test;
        // exiting quietly is better than sending a frame that would be read
        // as a different refusal than the one that happened.
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }

    // C4. `UNSHARED`, then block until the core has written the maps.
    if !send(cfg, &Frame::Unshared) || !matches!(recv(cfg), Some(Frame::MapDone)) {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }

    // C7. Fork, so there is a PID 1 for the core to address.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }
    if pid == 0 {
        // The PID-1 child. It mounts nothing -- its root is still the host's,
        // which is exactly the state the core has to catch -- and it asserts
        // a mount table anyway, because a lying `MOUNTED` frame is precisely
        // what "the child's numbers are not evidence" means.
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
        send(
            cfg,
            &Frame::Mounted {
                count: 42,
                fingerprint: 0xdead_beef_dead_beef,
            },
        );
        // Wait to be killed. The core refuses at P20 and `GuardedChild` takes
        // the supervisor down; PDEATHSIG takes this process with it.
        loop {
            unsafe { libc::pause() };
        }
    }

    // The supervisor. Reports the child's host pid and then waits, exactly as
    // the real one does -- so the only difference between this program and a
    // working helper is the filesystem the child did not build.
    send(cfg, &Frame::Child { host_pid: pid });
    unsafe { libc::close(cfg) };
    loop {
        unsafe { libc::pause() };
    }
}

fn send(fd: RawFd, frame: &Frame) -> bool {
    let Ok(bytes) = frame.encode() else {
        return false;
    };
    let rc = unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) };
    rc >= 0
}

fn recv(fd: RawFd) -> Option<Frame> {
    let mut buf = vec![0u8; CONFIG_MAX];
    let rc = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
    if rc <= 0 {
        return None;
    }
    Frame::decode(&buf[..rc as usize]).ok()
}
