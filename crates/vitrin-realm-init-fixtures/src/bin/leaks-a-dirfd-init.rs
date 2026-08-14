// SPDX-License-Identifier: Apache-2.0
//! `leaks-a-dirfd-init` -- opens one directory descriptor **without**
//! `O_CLOEXEC` and then `execve`s the real `vitrin-realm-init` in its place.
//!
//! # What it is for
//!
//! K13 (`close_range(4, ~0, CLOSE_RANGE_CLOEXEC)` before the shim's `execve`)
//! is the step that stops a surviving `O_DIRECTORY` handle on the old root
//! from being a complete `pivot_root` escape: `openat(fd, "../../..")` and
//! `fchdir(fd)` both work through one, and after the `MNT_DETACH` it is the
//! only remaining handle on the host tree.
//!
//! Deleting that call leaves the whole suite green, because every `open` in
//! the real helper carries `O_CLOEXEC` -- which is precisely the invariant
//! `close_range` exists so that nothing has to depend on. This fixture
//! removes the dependency: it leaks exactly one descriptor the helper itself
//! would never leak, so the escape is **demonstrated** and then observed to
//! be closed.
//!
//! # Why it is a wrapper and not a fork of the helper
//!
//! `execve` keeps the pid, so the core's `Child`, its `/proc/<pid>/ns/*`
//! reads and its map writes all still address the same process. Everything
//! after this file is the real, unmodified helper -- which means the test
//! this fixture serves is a test of the shipped code path, not of a copy of
//! it that could drift.
//!
//! The directory it leaks is the **host root**, which is the worst case
//! rather than a convenient one: `fchdir` on that descriptor hands the holder
//! the entire host filesystem. It is also the case a path-based assertion
//! cannot catch -- `readlink /proc/<shim>/fd/N` says `/`, and `/` exists
//! inside the realm too -- so the test that consumes this fixture compares
//! `(st_dev, st_ino)` against the host root instead of comparing names.

use std::os::fd::RawFd;

use vitrin_realm_init::PRE_EXEC_EXIT;

fn main() {
    // The leak. No `O_CLOEXEC`, deliberately, and `O_DIRECTORY` because a
    // directory handle is the one that is an escape rather than a nuisance.
    let leaked: RawFd = unsafe { libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    if leaked < 0 {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }
    // Belt and braces: a fixture that silently failed to leak would make the
    // test it serves vacuous in the other direction.
    let flags = unsafe { libc::fcntl(leaked, libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC != 0 {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }

    // The real helper, beside this binary. Resolved as a sibling rather than
    // from the environment because the core `env_clear()`s the helper's
    // environment, and inventing a variable for a fixture would mean the
    // helper's contract differed between tests and production.
    let Ok(exe) = std::env::current_exe() else {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    };
    let Some(dir) = exe.parent() else {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    };
    let real = dir.join("vitrin-realm-init");
    let Ok(path) = std::ffi::CString::new(real.as_os_str().as_encoded_bytes()) else {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    };

    // argv[0] only: the core passes the helper no arguments, and inventing
    // one here would be a difference between the fixture's run and the real
    // one.
    let argv = [path.as_ptr(), std::ptr::null()];
    unsafe {
        libc::execv(path.as_ptr(), argv.as_ptr());
        libc::_exit(PRE_EXEC_EXIT)
    }
}
