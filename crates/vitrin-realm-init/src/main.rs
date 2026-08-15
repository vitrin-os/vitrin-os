// SPDX-License-Identifier: MPL-2.0
//! `vitrin-realm-init` -- the per-realm namespace helper `vitrind` execs
//! (P2.6.2, issue #186, D-036).
//!
//! Read the library half's module docs first (`src/lib.rs`): they hold the
//! *why* -- exec-then-unshare, why a supervisor process is forced, and the
//! one correction to the record about `pid_for_children`. This file is the
//! sequence.
//!
//! # Three processes, and which one is which
//!
//! ```text
//!   vitrind  --execve-->  helper  --unshare(6)-->  helper (confined)
//!                            |
//!                            +-- fork --> PID 1 of the new pid namespace:
//!                            |              mount table, pivot_root,
//!                            |              execve(shim)
//!                            +-- becomes the SUPERVISOR: stays in the host
//!                                pid namespace, is the process vitrind's
//!                                `std::process::Child` names, forwards
//!                                SIGTERM/SIGINT to PID 1, and mirrors its
//!                                exit
//! ```
//!
//! # Allocation is allowed here, and that is the point
//!
//! `vitrin_core::spawn`'s `pre_exec` closure is syscalls and no Rust because
//! the core is multi-threaded and a forked child inherits locks it can never
//! release. **This binary is single-threaded** -- it starts no thread and
//! links no runtime that starts one -- so the `fork` below is a fork of a
//! one-thread process and the child may allocate, format and read files
//! freely. Building a twenty-entry mount table in ordinary Rust instead of in
//! async-signal-safe primitives is the whole reason a separate binary exists
//! rather than a longer closure.
//!
//! # What this process may decide: nothing
//!
//! Every path it mounts is one the **core** canonicalized and audited before
//! the fork and delivered over the config channel. It resolves no name of its
//! own, reads no environment variable, and consults no file for policy. It
//! passes its `environ` to the shim verbatim, so environment composition
//! stays at one site (`vitrin_core::spawn::child_env`).
//!
//! And nothing it says is evidence. The frames it sends are a handshake, not
//! a proof: the core's own reads of `/proc/<pid>/ns/*`, `/proc/<pid>/root`
//! and the canary set are what license the spawn. A substituted helper that
//! unshared six namespaces and mounted nothing would pass every check in
//! *this* file and be refused by those.

use std::ffi::{CStr, CString, OsStr};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use vitrin_realm_init::{
    mount_fingerprint, Config, Frame, Stage, CONFIG_MAX, IN_REALM_HOME, IN_REALM_RUNTIME_DIR,
    IN_REALM_SHIM, PRE_EXEC_EXIT,
};

mod landlock;

extern "C" {
    /// The environment this process was `execve`'d with -- exactly what
    /// `vitrin_core::spawn::child_env` composed after an `env_clear`. Read,
    /// never modified, and handed to the shim's `execve` verbatim.
    static environ: *const *const libc::c_char;
}

/// The directory the new root tmpfs is staged on before `pivot_root`.
///
/// `/tmp` is chosen for being universally present and for being shadowed only
/// inside this process's own private mount namespace, so nothing on the host
/// observes the change. There is deliberately **no fallback candidate**: a
/// fallback list is a second shape, and a host where `/tmp` is not a
/// directory owned by root or this uid is a host whose posture the core
/// should refuse rather than work around.
const STAGE_DIR: &str = "/tmp";

/// The six flags, in one call -- the exact request
/// `vitrin_core::spawn::isolation`'s `ns.all` row probes, so the preflight
/// measures the call the spawn actually issues.
///
/// One call rather than six: `CLONE_NEWNS` needs `CAP_SYS_ADMIN`, which an
/// unprivileged process only has *inside* a new user namespace, so
/// `CLONE_NEWUSER` has to be in the same `unshare` as the rest.
const CLONE_FLAGS: libc::c_int = libc::CLONE_NEWUSER
    | libc::CLONE_NEWNS
    | libc::CLONE_NEWPID
    | libc::CLONE_NEWIPC
    | libc::CLONE_NEWUTS
    | libc::CLONE_NEWNET;

/// The descriptor the shim's core connection occupies, placed there by the
/// core's `pre_exec` closure and inherited across *both* `execve`s. The one
/// descriptor that must not be marked close-on-exec.
const SHIM_CORE_FD: RawFd = 3;

/// A refusal, at a named stage, with the kernel's own errno.
#[derive(Debug, Clone, Copy)]
struct Fail {
    stage: Stage,
    errno: i32,
}

impl Fail {
    fn at(stage: Stage) -> Fail {
        Fail {
            stage,
            errno: errno(),
        }
    }

    fn with(stage: Stage, errno: i32) -> Fail {
        Fail { stage, errno }
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `rc < 0` becomes a [`Fail`] carrying `errno`; anything else passes through.
///
/// **Every failure is also written to stderr**, which is the realm's own log
/// file, which the core tails into its own stderr when a realm dies during
/// bring-up. The frame that goes back to the core carries a stage and an
/// errno -- the right size for a closed `cause_class` vocabulary, and far too
/// small to debug a twenty-entry mount table with. This is the other half,
/// and it is why "a shim that cannot say why it failed to start is
/// undebuggable" stays false now that diagnostics no longer share the
/// operator's terminal.
///
/// It costs nothing on the success path and this process makes a few hundred
/// syscalls in its whole life.
fn check(rc: libc::c_int, stage: Stage) -> Result<libc::c_int, Fail> {
    if rc < 0 {
        let fail = Fail::at(stage);
        eprintln!(
            "vitrin-realm-init: {:?} step failed: {}",
            fail.stage,
            std::io::Error::from_raw_os_error(fail.errno)
        );
        return Err(fail);
    }
    Ok(rc)
}

/// [`check`], with the step named -- for the calls where "a Mount step failed"
/// would leave a reader to guess which of twenty it was.
fn step(label: &str, rc: libc::c_int, stage: Stage) -> Result<libc::c_int, Fail> {
    if rc < 0 {
        let fail = Fail::at(stage);
        eprintln!(
            "vitrin-realm-init: {label}: {}",
            std::io::Error::from_raw_os_error(fail.errno)
        );
        return Err(fail);
    }
    Ok(rc)
}

/// The config channel: one `SOCK_SEQPACKET` descriptor, so a frame is a
/// datagram and there is no framing to get wrong.
///
/// A bare [`RawFd`] and **no `Drop`**, deliberately. Two of this process's
/// three roles close this descriptor explicitly at a point the protocol
/// depends on -- the supervisor right after it sends `CHILD`, so that the
/// core's EOF means "the shim `execve`'d" rather than "one of two holders let
/// go" -- and an owning type would then either double-close it or need a
/// disarm dance for no gain. Every path out of this process is an `execve` or
/// an `_exit`, both of which close descriptors without help.
struct Chan(RawFd);

impl Chan {
    fn send(&self, frame: &Frame) -> Result<(), Fail> {
        let bytes = frame
            .encode()
            .map_err(|_| Fail::with(Stage::Internal, libc::EMSGSIZE))?;
        // SAFETY: `bytes` is a live slice for exactly the length passed.
        let rc = unsafe {
            libc::send(
                self.0,
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if rc < 0 {
            return Err(Fail::at(Stage::Internal));
        }
        Ok(())
    }

    /// Receive one datagram. `stage` is the refusal this call's failures are
    /// reported under, so a malformed `CONFIG` reads as a version skew while
    /// a malformed `PROCEED` reads as a protocol fault.
    fn recv(&self, stage: Stage) -> Result<Frame, Fail> {
        let mut buf = vec![0u8; CONFIG_MAX];
        // SAFETY: `buf` is a live owned allocation of the length passed.
        let rc = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if rc < 0 {
            return Err(Fail::at(stage));
        }
        Frame::decode(&buf[..rc as usize]).map_err(|_| Fail::with(stage, libc::EPROTO))
    }

    /// Report a refusal and leave. Best effort by necessity: if the core is
    /// already gone there is nowhere to report to, and the `_exit` still has
    /// to happen.
    fn die(&self, fail: Fail) -> ! {
        let _ = self.send(&Frame::Fail {
            stage: fail.stage,
            errno: fail.errno,
        });
        // SAFETY: `_exit` runs no destructor and no `atexit` handler, which is
        // what a process half-way through building a mount namespace wants.
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }
}

fn main() {
    // C0. The config channel arrives on **stdin**, and that choice is what
    // makes the core's `pre_exec` closure change by zero lines: there is no
    // second fixed descriptor number to place, so no second `dup3` and no way
    // to collide with std's exec-report pipe. It is moved off fd 0 at once and
    // is close-on-exec, so it reaches neither the shim nor the app -- which is
    // the objection `vitrin_core::spawn`'s module docs raise against the
    // inetd shape, answered rather than ignored.
    //
    // SAFETY: `fcntl(F_DUPFD_CLOEXEC)` returns a fresh descriptor owned by
    // nobody else.
    let cfg_fd = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, SHIM_CORE_FD) };
    if cfg_fd < SHIM_CORE_FD {
        // No channel means nowhere to report to; the core sees an empty
        // handshake plus this exit code, and refuses.
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }
    let flags = unsafe { libc::fcntl(cfg_fd, libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
        unsafe { libc::_exit(PRE_EXEC_EXIT) }
    }
    // **And fd 0 is replaced immediately, which is not tidiness.** The core
    // learns that the shim `execve`'d by seeing EOF on this channel, and EOF
    // requires *every* copy to be gone. The supervisor closes the duplicate
    // above right after it sends `CHILD`; if the original were left on fd 0
    // it would sit there for the supervisor's whole life and the core would
    // wait out its 250 ms deadline on a realm that had started perfectly.
    // Measured, after exactly that: the handshake reached `PROCEED` in 2 ms
    // and then timed out.
    //
    // `/dev/null` rather than a bare `close`, so no later `open` in this
    // process can land on descriptor 0 and be clobbered by K12's `dup3`.
    // SAFETY: a NUL-terminated literal path; `dup3` onto 0 replaces whatever
    // was there.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
        if null < 0 || libc::dup3(null, 0, 0) < 0 {
            libc::_exit(PRE_EXEC_EXIT)
        }
        libc::close(null);
    }
    let chan = Chan(cfg_fd);

    match run(&chan) {
        Ok(never) => match never {},
        Err(fail) => chan.die(fail),
    }
}

/// Everything up to and including the second `execve`. The `Ok` type is
/// uninhabited because a successful `execve` never returns.
fn run(chan: &Chan) -> Result<std::convert::Infallible, Fail> {
    // C1. The version handshake, before anything else happens. A stale
    // sibling binary beside a new `vitrind` -- a `--shim` from
    // `/usr/lib/vitrin` next to a helper from `target/debug` -- is refused,
    // never silently executed: the two halves negotiate a mount table, and a
    // divergence there is a divergence in what got confined.
    let config = match chan.recv(Stage::Version)? {
        Frame::Config(config) => *config,
        _ => return Err(Fail::with(Stage::Version, libc::EPROTO)),
    };
    if config.schema_version != env!("CARGO_PKG_VERSION") {
        return Err(Fail::with(Stage::Version, libc::EPROTO));
    }

    // Before the `unshare`, because after it every unmapped owner reads back
    // as `overflowuid` and the check below could not tell root from a
    // stranger. See [`verify_stage_dir`].
    let stage_dev = verify_stage_dir()?;

    // C2. One `unshare`, six flags.
    //
    // SAFETY: `unshare` mutates only this process, which is exactly what this
    // binary exists to have happen.
    check(unsafe { libc::unshare(CLONE_FLAGS) }, Stage::Unshare)?;

    // C3 IS NOT HERE, AND THAT IS A MEASURED REFUTATION RATHER THAN AN
    // OMISSION.
    //
    // The design called for `setgroups(0, NULL)` immediately after the
    // `unshare` and before the parent writes `deny`, on the reasoning that
    // this is the one window in which it is legal -- because `setgroups=deny`
    // blocks the *call* and drops nothing, so without it the realm keeps the
    // operator's whole supplementary group list (`video`, `render`, `input`,
    // `docker`) as real kgids that permission checks still compare.
    //
    // **The reasoning about the consequence is right. The window does not
    // exist.** `may_setgroups()` is `ns_capable_setid(ns, CAP_SETGID) &&
    // userns_may_setgroups(ns)`, and `userns_may_setgroups` is
    // `ns->gid_map.nr_extents != 0 && (ns->flags & USERNS_SETGROUPS_ALLOWED)`.
    // Those two conjuncts cannot both hold for an unprivileged realm:
    //
    // - **Before the maps are written** there are no gid extents, so the
    //   first conjunct is false.
    // - **After they are written** `USERNS_SETGROUPS_ALLOWED` is gone, so the
    //   second is -- and it has to be gone, because `new_idmap_permitted`
    //   admits an unprivileged single-id `gid_map` only when
    //   `!(ns->flags & USERNS_SETGROUPS_ALLOWED)`. Writing `deny` is the
    //   *precondition* for having a gid map at all.
    //
    // Measured on this kernel (7.1.8) rather than read: `setgroups(0, NULL)`
    // returns `EPERM` in both windows, and `getgroups()` inside the finished
    // realm reports the operator's six entries -- the five unmapped ones
    // rendered as `overflowgid` (65534) and the mapped one as itself.
    //
    // **The residual is therefore published, not closed** (`limits.md`, and
    // the `supplementary_groups_retained` field on every `realm_spawned`
    // entry): a realm keeps the operator's supplementary group memberships as
    // kernel group ids, and the mount table really is the only barrier
    // against them. It is the same state every unprivileged container runtime
    // is in, for the same kernel reason, and closing it needs D-020(3)'s
    // provisioned per-uid tier -- a `newgidmap`-class helper can write a
    // multi-id map without `deny`, which is the only shape in which
    // `setgroups(0, NULL)` becomes reachable.

    // C4. D-020(1)'s sync pipe: the child announces and blocks, the parent
    // writes the maps. The parent writes them rather than this process, even
    // though the kernel would permit the self-write, because a self-written
    // map puts a policy decision inside the confined side and leaves the core
    // with no checkpoint at all.
    chan.send(&Frame::Unshared)?;
    match chan.recv(Stage::MapVerify)? {
        Frame::MapDone => {}
        _ => return Err(Fail::with(Stage::MapVerify, libc::EPROTO)),
    }

    // C5. Verify the maps from in here too, four ways. `getuid()` alone is
    // refused as the check: under an identity map it is vacuous when the euid
    // happens to be 65534 (`overflowuid`), it says nothing about the gid, and
    // it says nothing at all about `setgroups`.
    verify_maps(&config)?;

    // C6. `chdir("/")` BEFORE the fork, so `pivot_root`'s documented
    // relocation moves both halves and neither pins the old tree. The core
    // pointed `current_dir` at a host path, and a supervisor still sitting in
    // it would hold the old root alive after the detach.
    check(unsafe { libc::chdir(c"/".as_ptr()) }, Stage::Internal)?;

    // Block the three signals the supervisor's loop consumes BEFORE the fork,
    // so there is no window in which a `SIGTERM` from the core's ladder is
    // taken by the default disposition and kills the supervisor outright.
    let ladder = sigset(&[libc::SIGTERM, libc::SIGINT, libc::SIGCHLD]);
    check(
        unsafe { libc::sigprocmask(libc::SIG_BLOCK, &ladder, std::ptr::null_mut()) },
        Stage::Internal,
    )?;

    // C7. The supervisor-liveness pipe, then the fork that produces PID 1.
    //
    // The pipe exists because `getppid()` cannot answer the question. Across
    // a pid-namespace boundary the supervisor has no pid in the child's
    // namespace, so `getppid()` returns 0 whether the supervisor is alive or
    // dead. It therefore appears nowhere in this file, and a test asserts the
    // 0 directly, so a kernel that ever changed it fails loudly instead of
    // silently reactivating a race.
    let mut pipe_fds = [0 as libc::c_int; 2];
    check(
        unsafe { libc::pipe2(pipe_fds.as_mut_ptr(), libc::O_CLOEXEC) },
        Stage::Fork,
    )?;
    let (pipe_read, pipe_write) = (pipe_fds[0], pipe_fds[1]);

    // SAFETY: this process is single-threaded (module docs), so the child of
    // this fork inherits no lock it cannot release and may run ordinary Rust.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => Err(Fail::at(Stage::Fork)),
        0 => {
            unsafe { libc::close(pipe_write) };
            pid_one(chan, &config, pipe_read, stage_dev)
        }
        child => {
            unsafe { libc::close(pipe_read) };
            supervise(chan, child, pipe_write)
        }
    }
}

/// Build a `sigset_t` holding exactly `signals`.
fn sigset(signals: &[libc::c_int]) -> libc::sigset_t {
    // SAFETY: `sigemptyset`/`sigaddset` fully initialize the value before any
    // read of it.
    unsafe {
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut set);
        for sig in signals {
            libc::sigaddset(&mut set, *sig);
        }
        set
    }
}

/// C5: the child's own read-back of what the parent said it wrote.
fn verify_maps(config: &Config) -> Result<(), Fail> {
    let bad = |e: i32| Fail::with(Stage::MapVerify, e);

    let setgroups = std::fs::read_to_string("/proc/self/setgroups").map_err(|_| bad(libc::EIO))?;
    if setgroups.trim() != "deny" {
        return Err(bad(libc::EPERM));
    }

    // The kernel **reformats** an id map on read-back to `%10u %10u %10u`, so
    // a literal byte comparison against `"1000 1000 1\n"` fails on every
    // kernel. The triple is what was written and the triple is what is
    // compared -- and the single-line shape is checked too, because a
    // multi-line map is not something an unprivileged writer can produce, so
    // seeing one means something other than the core wrote it.
    for (path, expected) in [
        ("/proc/self/uid_map", config.inner_uid),
        ("/proc/self/gid_map", config.inner_gid),
    ] {
        let text = std::fs::read_to_string(path).map_err(|_| bad(libc::EIO))?;
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let line = lines.next().ok_or_else(|| bad(libc::EINVAL))?;
        if lines.next().is_some() {
            return Err(bad(libc::EINVAL));
        }
        let fields: Vec<u32> = line
            .split_whitespace()
            .filter_map(|f| f.parse().ok())
            .collect();
        if fields != vec![expected, expected, 1] {
            return Err(bad(libc::EINVAL));
        }
    }

    // There is deliberately **no `getgroups() == 0` assertion here.** The
    // design had one, paired with the `setgroups(0, NULL)` that C3 shows is
    // impossible; asserting an empty group list would fail on every machine
    // whose operator belongs to any group at all. What the realm retained is
    // measured by the *parent* instead, from `/proc/<pid>/status`, and
    // journaled as the residual it is.

    // SAFETY: pure queries with no out-parameters.
    let (uid, euid, gid, egid) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::getgid(),
            libc::getegid(),
        )
    };
    if uid != config.inner_uid
        || euid != config.inner_uid
        || gid != config.inner_gid
        || egid != config.inner_gid
    {
        return Err(bad(libc::EINVAL));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// S1-S3: the supervisor
// ---------------------------------------------------------------------------

/// The supervisor: host-pid-namespace resident, the process the core's
/// `Child` names, and from here on a signal relay and nothing else.
fn supervise(
    chan: &Chan,
    child: libc::pid_t,
    pipe_write: RawFd,
) -> Result<std::convert::Infallible, Fail> {
    // S1. The core connection belongs to the shim alone, and this process is
    // not the shim.
    unsafe { libc::close(SHIM_CORE_FD) };
    chan.send(&Frame::Child { host_pid: child })?;
    // Closed here so the core's EOF means "the shim `execve`'d" rather than
    // "one of two holders let go". After this the supervisor cannot report
    // anything, which is correct: everything left to say is PID 1's.
    unsafe { libc::close(chan.0) };

    // S2. Forward, never exit. Exiting on `SIGTERM` would collapse the
    // termination ladder's rung 2 into rung 3 every single time: the core's
    // polite request would kill the supervisor, PDEATHSIG would `SIGKILL` the
    // shim, and the grace period a legitimate handler exists to use would
    // never be reached.
    let wait_for = sigset(&[libc::SIGTERM, libc::SIGINT, libc::SIGCHLD]);
    loop {
        // Reap first, so a child that died before the first `sigwaitinfo` --
        // or whose `SIGCHLD` was coalesced with another -- is still noticed.
        let mut status: libc::c_int = 0;
        // SAFETY: `child` is this process's own child and `status` is a valid
        // out-parameter.
        let reaped = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if reaped == child {
            mirror_exit(status, pipe_write);
        }
        if reaped < 0 && errno() == libc::ECHILD {
            // Nothing left to supervise and no status to mirror.
            unsafe { libc::_exit(PRE_EXEC_EXIT) };
        }

        // SAFETY: `wait_for` is a fully initialized set and `info` a valid
        // out-parameter.
        let sig = unsafe {
            let mut info = std::mem::zeroed::<libc::siginfo_t>();
            libc::sigwaitinfo(&wait_for, &mut info)
        };
        if sig < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            unsafe { libc::_exit(PRE_EXEC_EXIT) };
        }
        if sig == libc::SIGTERM || sig == libc::SIGINT {
            // By host pid: the supervisor is the only process that knows both
            // numbers, and the ladder's rung 2 reaches the shim through it.
            unsafe { libc::kill(child, sig) };
        }
    }
}

/// S3. Mirror the shim's exit exactly.
///
/// **After `execve` succeeds the supervisor never invents a code.** A normal
/// exit is re-exited with the same status; a signal death is re-raised with
/// the same signal, so `vitrin_core::lifecycle::ExitClass::of` sees
/// `WIFSIGNALED` with the signal the shim actually died of rather than an
/// exit code this process made up.
fn mirror_exit(status: libc::c_int, pipe_write: RawFd) -> ! {
    // Closed first: if PID 1 somehow outlived its own reap notification, the
    // HUP is its second cue to go.
    unsafe { libc::close(pipe_write) };
    if libc::WIFEXITED(status) {
        unsafe { libc::_exit(libc::WEXITSTATUS(status)) };
    }
    if libc::WIFSIGNALED(status) {
        let sig = libc::WTERMSIG(status);
        // SAFETY: resetting to the default disposition, unblocking exactly
        // that signal and raising it is the standard "die the way the child
        // died" idiom.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            let just = sigset(&[sig]);
            libc::sigprocmask(libc::SIG_UNBLOCK, &just, std::ptr::null_mut());
            libc::raise(sig);
            // Reachable only if the signal was somehow still not delivered.
            libc::_exit(128 + sig);
        }
    }
    unsafe { libc::_exit(PRE_EXEC_EXIT) }
}

// ---------------------------------------------------------------------------
// K1-K14: PID 1 of the realm
// ---------------------------------------------------------------------------

fn pid_one(
    chan: &Chan,
    config: &Config,
    pipe_read: RawFd,
    stage_dev: u64,
) -> Result<std::convert::Infallible, Fail> {
    // K1. Two mechanisms, because one of them has a window.
    //
    // `PR_SET_PDEATHSIG` is the durable half: when the supervisor dies this
    // process is `SIGKILL`ed, and the kernel's rule that a pid namespace dies
    // with its init takes the app with it -- which is what closes the orphan
    // residual `vitrin_core::lifecycle` used to publish.
    //
    // The window is the instant between the `fork` and this `prctl`: a
    // supervisor that died inside it would never have armed anything. The
    // pipe closes that window. `getppid()` cannot: across the pid-namespace
    // boundary the supervisor has no pid here, so it returns 0 either way.
    check(
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) },
        Stage::Internal,
    )?;
    let mut poll_fd = libc::pollfd {
        fd: pipe_read,
        events: 0,
        revents: 0,
    };
    // SAFETY: one valid `pollfd` for the count given, with a zero timeout.
    if unsafe { libc::poll(&mut poll_fd, 1, 0) } > 0 && poll_fd.revents & libc::POLLHUP != 0 {
        unsafe { libc::_exit(PRE_EXEC_EXIT) };
    }

    // A session of its own, and this is a confinement step rather than
    // housekeeping. `/dev/tty` names the *controlling terminal of the
    // opener*, so a realm that kept the core's would reach the operator's
    // terminal through a device node no mount flag can revoke -- the same
    // hole the per-realm log file closes for fds 1 and 2. After `setsid` the
    // realm has no controlling terminal, `/dev/tty` opens return `ENXIO`, and
    // that is a defined answer every program already handles.
    check(unsafe { libc::setsid() }, Stage::Internal)?;

    build_mount_table(config, stage_dev)?;

    // K9. Child-asserted, and journaled as such: a substituted helper could
    // send any numbers it liked, which is precisely why the core's root-view
    // check exists and why these two fields are not what licenses the spawn.
    let mountinfo = std::fs::read("/proc/self/mountinfo").unwrap_or_default();
    let count = mountinfo.iter().filter(|b| **b == b'\n').count() as u32;
    chan.send(&Frame::Mounted {
        count,
        fingerprint: mount_fingerprint(&mountinfo),
    })?;

    // The core's root-view check happens on the other side of this `recv`.
    match chan.recv(Stage::Internal)? {
        Frame::Proceed => {}
        Frame::Fail { stage, errno } => return Err(Fail::with(stage, errno)),
        _ => return Err(Fail::with(Stage::Internal, libc::EPROTO)),
    }

    exec_shim(chan, config)
}

// ---------------------------------------------------------------------------
// The mount table
// ---------------------------------------------------------------------------

// Spelled here rather than taken from `libc`, whose `MS_*` constants are
// `c_ulong` on some targets and `c_int` on others; the whole table wants one
// type.
const MS_RDONLY: libc::c_ulong = 1;
const MS_NOSUID: libc::c_ulong = 2;
const MS_NODEV: libc::c_ulong = 4;
const MS_NOEXEC: libc::c_ulong = 8;
const MS_REMOUNT: libc::c_ulong = 32;
const MS_BIND: libc::c_ulong = 4096;
const MS_REC: libc::c_ulong = 16384;
const MS_PRIVATE: libc::c_ulong = 1 << 18;

/// Flags every read-only bind in the table carries.
const RO_BIND: libc::c_ulong = MS_RDONLY | MS_NOSUID | MS_NODEV;
/// Flags every writable bind carries: `noexec` too, because nothing the realm
/// can write should be something the realm can then run.
const RW_BIND: libc::c_ulong = MS_NOSUID | MS_NODEV | MS_NOEXEC;
/// Flags a **device node** bind carries.
///
/// `MS_NODEV` is deliberately absent, and its absence is the whole point:
/// `nodev` means "do not interpret device special files on this mount", so
/// setting it on the bind of `/dev/null` makes `/dev/null` unopenable. The
/// nodes are borrowed by bind because `mknod` of a device node is impossible
/// in a non-initial user namespace.
const DEV_BIND: libc::c_ulong = MS_NOSUID | MS_NOEXEC;

fn cstr(s: &OsStr) -> Result<CString, Fail> {
    CString::new(s.as_bytes()).map_err(|_| Fail::with(Stage::Mount, libc::EINVAL))
}

fn cstr_str(s: &str) -> Result<CString, Fail> {
    cstr(OsStr::new(s))
}

fn mount_raw(
    source: Option<&CStr>,
    target: &CStr,
    fstype: Option<&CStr>,
    flags: libc::c_ulong,
    data: Option<&CStr>,
    stage: Stage,
) -> Result<(), Fail> {
    let no_str: *const libc::c_char = std::ptr::null();
    // SAFETY: every pointer is either null or a live `CStr` for the call's
    // duration; `mount` reads them and retains nothing.
    let rc = unsafe {
        libc::mount(
            source.map_or(no_str, |s| s.as_ptr()),
            target.as_ptr(),
            fstype.map_or(no_str, |s| s.as_ptr()),
            flags,
            data.map_or(std::ptr::null(), |d| d.as_ptr().cast()),
        )
    };
    if rc < 0 {
        // **Which entry**, on stderr, before the frame goes back. The frame
        // carries a stage and an errno, which is the right size for a closed
        // vocabulary and far too small to debug a twenty-entry table with; the
        // core's per-realm log is where the detail belongs, and the core tails
        // that log into its own stderr when a realm dies during bring-up. This
        // line is why "a shim that cannot say why it failed to start is
        // undebuggable" stays false after diagnostics moved off the terminal.
        eprintln!(
            "vitrin-realm-init: mount(source={:?}, target={:?}, fstype={:?}, flags={:#x}, \
             data={:?}) failed: {}",
            source.map(CStr::to_string_lossy),
            target.to_string_lossy(),
            fstype.map(CStr::to_string_lossy),
            flags,
            data.map(CStr::to_string_lossy),
            std::io::Error::last_os_error()
        );
    }
    check(rc, stage).map(|_| ())
}

/// `mkdir -p`, **relative to the current working directory** (the staged new
/// root).
///
/// Relative on purpose: after K3 every target in the table is named relative
/// to the staged root, so no absolute host path is resolved a second time and
/// a symlink on the host cannot redirect a target.
fn mkdir_rel(rel: &str) -> Result<(), Fail> {
    let mut so_far = PathBuf::new();
    for part in Path::new(rel).components() {
        so_far.push(part);
        let c = cstr(so_far.as_os_str())?;
        // SAFETY: `c` is a live NUL-terminated relative path.
        if unsafe { libc::mkdir(c.as_ptr(), 0o755) } < 0 && errno() != libc::EEXIST {
            return step(&format!("mkdir {}", so_far.display()), -1, Stage::Mount).map(|_| ());
        }
    }
    Ok(())
}

/// Create the empty regular file a *file* bind needs as its target.
fn touch_rel(rel: &str) -> Result<(), Fail> {
    if let Some(parent) = Path::new(rel).parent() {
        if !parent.as_os_str().is_empty() {
            mkdir_rel(&parent.to_string_lossy())?;
        }
    }
    let c = cstr_str(rel)?;
    // SAFETY: `c` is a live NUL-terminated relative path.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_CREAT | libc::O_WRONLY | libc::O_CLOEXEC,
            0o644,
        )
    };
    if fd < 0 {
        return if errno() == libc::EEXIST {
            Ok(())
        } else {
            step(&format!("create bind target {rel}"), -1, Stage::Mount).map(|_| ())
        };
    }
    unsafe { libc::close(fd) };
    Ok(())
}

/// A bind mount, then the remount that actually applies the per-mount flags.
///
/// Both calls are required: `MS_BIND` alone copies the source mount's flags,
/// and `MS_RDONLY` in the first call is silently ignored for a bind. The
/// second call, `MS_REMOUNT | MS_BIND`, is the one that makes the target
/// read-only, `nosuid` and `nodev`.
///
/// The resulting posture is **kernel-enforced, not convention**: a nested
/// `unshare(CLONE_NEWUSER | CLONE_NEWNS)` by the app cannot relax any of these
/// flags, because `copy_mnt_ns` sets `MNT_LOCK_*` on every mount whose owning
/// user namespace differs from the new mount namespace's. Worth stating
/// because a reader will ask.
fn bind(source: &Source, target_rel: &str, flags: libc::c_ulong) -> Result<(), Fail> {
    if source.is_file {
        touch_rel(target_rel)?;
    } else {
        mkdir_rel(target_rel)?;
    }
    // The source is named by **descriptor**, through `/proc/self/fd/N`, and
    // never by the path it was opened from. Two reasons, one of which is a
    // correctness bug and not a hardening nicety:
    //
    // 1. The new root is staged on `/tmp`, so any bind source *underneath*
    //    `/tmp` is shadowed by the time the table is built and would bind the
    //    wrong thing or nothing at all. A descriptor opened before the
    //    staging still names the file it named.
    // 2. It closes the window between **this process's `open`** and this
    //    mount, which is the window the mount table itself creates: every
    //    source is opened once in `open_sources` and then mounted many lines
    //    later, and re-resolving the name at each mount site would give a
    //    racing writer that many chances instead of one.
    //
    // What it does **not** close, stated plainly because the honest version
    // of this note is worth more than the flattering one: the window between
    // the core's `canonicalize` + audit and `Source::open` here. That is a
    // second, independent path resolution in a second process, and it is
    // performed without `O_NOFOLLOW`. It carries exactly the residual
    // `vitrin_core::spawn::audit_program_at_spawn` records for the program it
    // audits -- an attacker who can replace a component of an audited path
    // between the audit and the open wins, and the defence is the
    // trusted-writer rule over every directory on the path, not the ordering
    // of these two calls.
    let src = cstr_str(&format!("/proc/self/fd/{}", source.fd))?;
    let dst = cstr_str(target_rel)?;
    mount_raw(Some(&src), &dst, None, MS_BIND, None, Stage::Mount)?;
    mount_raw(
        None,
        &dst,
        None,
        MS_REMOUNT | MS_BIND | flags,
        None,
        Stage::Mount,
    )
}

/// A bind source, held open from **before** the new root is staged.
///
/// See [`bind`] for why this is a descriptor and not a path.
struct Source {
    fd: libc::c_int,
    is_file: bool,
}

impl Source {
    /// `O_PATH`, so no read permission on the source is required and a device
    /// node is not actually opened -- only named.
    fn open(path: &Path, is_file: bool) -> Result<Source, Fail> {
        let c = cstr(path.as_os_str())?;
        // SAFETY: `c` is a live NUL-terminated path.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_PATH | libc::O_CLOEXEC | if is_file { 0 } else { libc::O_DIRECTORY },
            )
        };
        step(
            &format!("open bind source {}", path.display()),
            fd,
            Stage::Mount,
        )?;
        Ok(Source { fd, is_file })
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this type is the only owner of `fd`.
        unsafe { libc::close(self.fd) };
    }
}

fn mount_fs(fstype: &str, target_rel: &str, flags: libc::c_ulong, data: &str) -> Result<(), Fail> {
    mkdir_rel(target_rel)?;
    let ty = cstr_str(fstype)?;
    let dst = cstr_str(target_rel)?;
    let opts = cstr_str(data)?;
    mount_raw(
        Some(&ty),
        &dst,
        Some(&ty),
        flags,
        if data.is_empty() { None } else { Some(&opts) },
        Stage::Mount,
    )
}

fn strip_leading_slash(p: &str) -> &str {
    p.strip_prefix('/').unwrap_or(p)
}

fn symlink_rel(target: &str, link_rel: &str) -> Result<(), Fail> {
    let t = cstr_str(target)?;
    let l = cstr_str(link_rel)?;
    // SAFETY: both are live NUL-terminated strings.
    if unsafe { libc::symlink(t.as_ptr(), l.as_ptr()) } < 0 && errno() != libc::EEXIST {
        return step(&format!("symlink {link_rel} -> {target}"), -1, Stage::Mount).map(|_| ());
    }
    Ok(())
}

/// Everything the table binds, opened while the host tree is still visible.
///
/// Collected as one struct rather than opened at each mount site because the
/// *ordering* is the point: every one of these has to be a descriptor before
/// [`stage_new_root`] runs, and a single construction site is what makes that
/// checkable by reading twenty lines instead of two hundred.
struct Sources {
    usr: Source,
    etc: Source,
    /// The host's `/bin`-class compatibility shape, already resolved: a
    /// symlink contributes its target, a real directory contributes a
    /// descriptor. Mirrored rather than assumed, because a merged-`/usr`
    /// distribution and a split-`/usr` one disagree and guessing either way
    /// breaks every `#!/bin/sh` on the other kind of machine.
    compat: Vec<(String, CompatEntry)>,
    dev_nodes: Vec<(String, Source)>,
    render_nodes: Vec<(String, Source)>,
    runtime_dir: Source,
    storage_dir: Source,
    shim: Source,
    app_dir: Option<(String, Source)>,
    binds: Vec<(String, Source)>,
}

enum CompatEntry {
    Symlink(String),
    Directory(Source),
}

const COMPAT_NAMES: [&str; 6] = ["bin", "sbin", "lib", "lib64", "lib32", "libx32"];
const DEV_NODES: [&str; 6] = ["null", "zero", "full", "random", "urandom", "tty"];

/// Does this `binds` entry name a **file** rather than a directory?
///
/// `realm.toml` accepts either (`examples/realm.toml`: "a regular file or
/// directory"), and the two answers are not interchangeable in either of the
/// two places this binary acts on a bind:
///
/// - [`open_sources`] must decide `O_DIRECTORY` or not, and [`bind`] must
///   decide `mkdir` or `touch` for the target;
/// - [`landlock::grants`] must decide whether the rule may name
///   `LANDLOCK_ACCESS_FS_READ_DIR`, which on a regular file is not an unused
///   right but an **`EINVAL` from `landlock_add_rule`**.
///
/// **One predicate, both call sites**, because two would be two chances to
/// disagree -- and a disagreement here is a realm whose mount table binds a
/// file while its ruleset describes a directory. The two run at different
/// moments and on different trees (this one before `pivot_root` on the host's
/// path, the ruleset's after it on the in-realm path of the same name), and
/// they still agree by construction: the target [`bind`] created is a file
/// exactly when this answered `true` for the source.
///
/// A path that does not resolve answers `true`, which is deliberate: `is_dir`
/// cannot distinguish "absent" from "not a directory", and the caller that
/// then opens it reports the absence with its own errno rather than having it
/// guessed at here.
fn bind_is_file(path: &Path) -> bool {
    !path.is_dir()
}

fn open_sources(config: &Config) -> Result<Sources, Fail> {
    let mut compat = Vec::new();
    for name in COMPAT_NAMES {
        let host = PathBuf::from("/").join(name);
        let Ok(meta) = std::fs::symlink_metadata(&host) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let Ok(target) = std::fs::read_link(&host) else {
                continue;
            };
            compat.push((
                name.to_string(),
                CompatEntry::Symlink(target.to_string_lossy().into_owned()),
            ));
        } else if meta.is_dir() {
            compat.push((
                name.to_string(),
                CompatEntry::Directory(Source::open(&host, false)?),
            ));
        }
    }

    let mut dev_nodes = Vec::new();
    for node in DEV_NODES {
        dev_nodes.push((
            format!("dev/{node}"),
            Source::open(&PathBuf::from("/dev").join(node), true)?,
        ));
    }
    let mut render_nodes = Vec::new();
    for node in &config.render_nodes {
        let Some(name) = node.file_name() else {
            return Err(Fail::with(Stage::Mount, libc::EINVAL));
        };
        render_nodes.push((
            format!("dev/dri/{}", name.to_string_lossy()),
            Source::open(node, true)?,
        ));
    }

    let app_dir = match &config.app_dir {
        Some(dir) => Some((
            strip_leading_slash(&dir.to_string_lossy()).to_string(),
            Source::open(dir, false)?,
        )),
        None => None,
    };
    let mut binds = Vec::new();
    for extra in &config.binds {
        let target = strip_leading_slash(&extra.to_string_lossy()).to_string();
        // A file bind for a file, a directory bind for a directory: the core
        // already established which it is when it audited the source.
        binds.push((target, Source::open(extra, bind_is_file(extra))?));
    }

    Ok(Sources {
        usr: Source::open(Path::new("/usr"), false)?,
        etc: Source::open(Path::new("/etc"), false)?,
        compat,
        dev_nodes,
        render_nodes,
        runtime_dir: Source::open(&config.runtime_dir, false)?,
        storage_dir: Source::open(&config.storage_dir, false)?,
        shim: Source::open(&config.shim_source, true)?,
        app_dir,
        binds,
    })
}

/// K2-K8: everything between "we are PID 1" and "the old root is gone".
fn build_mount_table(config: &Config, stage_dev: u64) -> Result<(), Fail> {
    // Before anything is mounted, while the host tree is still the tree this
    // process sees. See [`bind`] for why every source is a descriptor.
    let sources = open_sources(config)?;

    // K2. FIRST, always. `systemd` leaves `/` shared, `pivot_root` refuses a
    // shared new root, and without this every mount below would propagate
    // back onto the host's own tree.
    let slash = cstr_str("/")?;
    mount_raw(None, &slash, None, MS_REC | MS_PRIVATE, None, Stage::Mount)?;

    // K3. Stage the new root, and move into it.
    stage_new_root(config, stage_dev)?;

    // K4. The table. The order is not arbitrary: `/usr` first, because the
    // compatibility symlinks a merged-`/usr` host needs point into it, and
    // `/proc` only after the fork -- already true, since this is PID 1 --
    // because a fresh procfs requires the mounter to be in a new pid
    // namespace.
    bind(&sources.usr, "usr", RO_BIND)?;
    for (name, entry) in &sources.compat {
        match entry {
            CompatEntry::Symlink(target) => symlink_rel(target, name)?,
            CompatEntry::Directory(source) => bind(source, name, RO_BIND)?,
        }
    }
    // A read-only `/etc` is not an `/etc` without secrets: `/etc/shadow`,
    // `/etc/ssh/*_key` and NetworkManager's `system-connections` are
    // protected by mode bits, not by this bind. Stated because "read-only"
    // reads as harmless.
    bind(&sources.etc, "etc", RO_BIND)?;

    mount_fs("proc", "proc", MS_NOSUID | MS_NODEV | MS_NOEXEC, "")?;
    // A **fresh** sysfs, never a bind. A bind of the host's would show
    // `/sys/class/net` -- the host's interface names and MAC addresses --
    // inside a fresh network namespace, defeating half of `CLONE_NEWNET`. A
    // fresh mount is netns-scoped, and it is legal here because the mounter
    // holds `CAP_SYS_ADMIN` in the user namespace owning a non-initial
    // network namespace.
    mount_fs(
        "sysfs",
        "sys",
        MS_RDONLY | MS_NOSUID | MS_NODEV | MS_NOEXEC,
        "",
    )?;

    // `nosuid` and `noexec` but deliberately **not** `nodev` -- see [`DEV_BIND`].
    mount_fs(
        "tmpfs",
        "dev",
        MS_NOSUID | MS_NOEXEC,
        &format!("mode=0755,size={}", config.caps.dev),
    )?;
    for (target, source) in &sources.dev_nodes {
        bind(source, target, DEV_BIND)?;
    }
    // Read-write, and never `card*` or `controlD*`: `MS_RDONLY` on a render
    // node makes it unopenable for write, which disables the node rather than
    // restricting it. The GPU ioctl surface and cross-realm GPU-memory side
    // channels it carries are real and unaddressed here -- a published cost
    // of binding the node at all, which is the price of not silently
    // disabling every accelerated app in every realm.
    //
    // **P2.6.3's Landlock ruleset does not change that, and the reason
    // belongs beside the mount rather than only in the ladder.** Rung 5 buys
    // `LANDLOCK_ACCESS_FS_IOCTL_DEV`, which is one all-or-nothing bit per
    // granted hierarchy -- there is no "these ioctl commands but not those".
    // An app that cannot `ioctl` its render node cannot render, so
    // `landlock::grants` **grants** `IOCTL_DEV` here. What that rung narrows
    // is every *other* device node in this table (`/dev/null`, `/dev/zero`,
    // `/dev/full`, `/dev/random`, `/dev/urandom`, `/dev/tty`), which lose
    // `ioctl` entirely. The published limit above survives the rung intact.
    for (target, source) in &sources.render_nodes {
        bind(source, target, DEV_BIND)?;
    }
    mount_fs(
        "tmpfs",
        "dev/shm",
        MS_NOSUID | MS_NODEV | MS_NOEXEC,
        &format!("mode=1777,size={}", config.caps.shm),
    )?;
    // `newinstance`, so the realm's ptys are its own. No `gid=`: the `tty`
    // group's gid is unmapped inside a single-id map, and passing it would
    // fail the mount outright.
    mount_fs(
        "devpts",
        "dev/pts",
        MS_NOSUID | MS_NOEXEC,
        "newinstance,ptmxmode=0666,mode=0620",
    )?;
    symlink_rel("pts/ptmx", "dev/ptmx")?;

    // **`/dev` becomes read-only once it is populated**, and it has to: the
    // published writable set is `{/run/vitrin, /vitrin/home, /tmp, /dev/shm}`,
    // and a writable `/dev` would have made that claim false the day it was
    // written. Caught by the parent-side mountinfo assertion in
    // `vitrin_core::spawn`'s tests, which is the point of asserting the set
    // rather than describing it.
    //
    // The submounts keep their own flags -- `MS_REMOUNT` acts on one mount,
    // not a subtree -- so `/dev/shm` stays writable, `/dev/pts` can still
    // allocate ptys, and `/dev/null` is still openable for write.
    let dev = cstr_str("dev")?;
    mount_raw(
        None,
        &dev,
        None,
        MS_REMOUNT | MS_BIND | MS_RDONLY | MS_NOSUID | MS_NOEXEC,
        None,
        Stage::Mount,
    )?;

    // `nosuid` and `nodev` but **not** `noexec`: enough real software still
    // writes a helper into `/tmp` and runs it that `noexec` here would read as
    // a vitrin bug rather than as a policy.
    mount_fs(
        "tmpfs",
        "tmp",
        MS_NOSUID | MS_NODEV,
        &format!("mode=1777,size={}", config.caps.tmp),
    )?;

    // The realm's control plane: a bind of the directory the core created,
    // purged and `flock`ed. It stays a bind rather than becoming a
    // realm-private tmpfs because a child that created its own runtime
    // directory would be choosing its own confinement, and because the
    // property a private tmpfs was wanted for -- `../core.sock` unreachable --
    // is delivered by the mount namespace either way: `..` from a bind target
    // resolves to the target's parent in *this* tree.
    bind(
        &sources.runtime_dir,
        strip_leading_slash(IN_REALM_RUNTIME_DIR),
        RW_BIND,
    )?;
    bind(
        &sources.storage_dir,
        strip_leading_slash(IN_REALM_HOME),
        RW_BIND,
    )?;
    // The shim has to be bound at all because in a development tree it lives
    // in `target/debug` under `$HOME` -- the exact tree this task exists to
    // hide from the realm. No `noexec` here, for obvious reasons.
    bind(&sources.shim, strip_leading_slash(IN_REALM_SHIM), RO_BIND)?;

    if let Some((target, source)) = &sources.app_dir {
        bind(source, target, RO_BIND)?;
    }
    for (target, source) in &sources.binds {
        bind(source, target, RO_BIND)?;
    }

    // K5. The new root becomes read-only. Without it the app owns the root
    // tmpfs -- it runs as the mapped uid -- and can create files at `/`,
    // including replacing the shim binary's bind target. After it, the realm's
    // entire writable set is exactly
    // `{/run/vitrin, /vitrin/home, /tmp, /dev/shm}`.
    let here = cstr_str(".")?;
    mount_raw(
        None,
        &here,
        None,
        MS_REMOUNT | MS_BIND | MS_RDONLY | MS_NOSUID | MS_NODEV,
        None,
        Stage::Mount,
    )?;

    // K6. `uname -n` inside the realm names the realm, not the operator's
    // machine.
    let host = config.realm_id.as_bytes();
    step(
        "sethostname",
        // SAFETY: a live byte slice and its length.
        unsafe { libc::sethostname(host.as_ptr().cast(), host.len()) },
        Stage::Mount,
    )?;

    // K7. Bring `lo` up. Omitting it makes a whole class of app breakage --
    // anything that talks to itself over TCP -- look like a mount-table bug.
    bring_loopback_up()?;

    // K8. `pivot_root`, never `chroot`: a `chroot` leaves the old tree mounted
    // and reachable from any descriptor that predates it.
    pivot(&here)
}

/// Verify [`STAGE_DIR`] and return its device number.
///
/// **Called before the `unshare`, and that ordering is forced by a trap worth
/// naming.** Inside a new user namespace with a single-id map, `stat` renders
/// every unmapped owner as `overflowuid` (65534) -- so `/tmp`, which is owned
/// by root, comes back owned by "nobody" and an "owned by root or by us"
/// check refuses on a perfectly ordinary machine. Measured, after this exact
/// mistake: the first version of this function ran after C2 and failed
/// `EPERM` on every spawn.
///
/// The general rule, since the next person will hit it too: **an ownership
/// check inside a realm's user namespace can only speak about the one id that
/// is mapped.** Everything else is 65534, including root, including two
/// different accounts that are then indistinguishable from each other.
fn verify_stage_dir() -> Result<u64, Fail> {
    // `lstat`, so a symlink at `/tmp` is seen as a symlink rather than
    // followed to wherever somebody pointed it.
    let before = lstat_path(STAGE_DIR)?;
    if before.st_mode & libc::S_IFMT != libc::S_IFDIR {
        eprintln!("vitrin-realm-init: {STAGE_DIR} is not a directory");
        return Err(Fail::with(Stage::Mount, libc::ENOTDIR));
    }
    // SAFETY: pure queries.
    let euid = unsafe { libc::geteuid() };
    if before.st_uid != 0 && before.st_uid != euid {
        eprintln!(
            "vitrin-realm-init: {STAGE_DIR} is owned by uid {}, neither root nor {euid}",
            before.st_uid
        );
        return Err(Fail::with(Stage::Mount, libc::EPERM));
    }
    Ok(before.st_dev as u64)
}

/// K3. Mount the new root tmpfs on [`STAGE_DIR`] and move into it.
fn stage_new_root(config: &Config, stage_dev: u64) -> Result<(), Fail> {
    let stage = cstr_str(STAGE_DIR)?;
    let ty = cstr_str("tmpfs")?;
    let opts = cstr_str(&format!("mode=0755,size={}", config.caps.root))?;
    mount_raw(
        Some(&ty),
        &stage,
        Some(&ty),
        MS_NOSUID | MS_NODEV,
        Some(&opts),
        Stage::Mount,
    )?;

    // `chdir(STAGE_DIR)`, not `chdir(".")`. `.` resolves to the current
    // directory itself and performs no lookup, so it would never cross onto
    // the mount just made: the process would stay on the shadowed host
    // directory and build the entire table there, on the host's disk, with
    // nothing to show for it. The absolute path is safe to resolve precisely
    // because it now resolves inside this process's own private mount
    // namespace -- and the `st_dev` comparison below is the proof that it
    // landed on the new mount rather than on the old directory.
    check(unsafe { libc::chdir(stage.as_ptr()) }, Stage::Mount)?;
    let after = lstat_path(".")?;
    if after.st_dev as u64 == stage_dev {
        eprintln!(
            "vitrin-realm-init: chdir({STAGE_DIR}) landed on the shadowed host directory, not \
             on the tmpfs just mounted"
        );
        return Err(Fail::with(Stage::Mount, libc::EBUSY));
    }
    Ok(())
}

fn lstat_path(path: &str) -> Result<libc::stat, Fail> {
    let c = cstr_str(path)?;
    // SAFETY: `c` is live and `st` is a valid out-parameter.
    unsafe {
        let mut st = std::mem::zeroed::<libc::stat>();
        check(libc::lstat(c.as_ptr(), &mut st), Stage::Mount)?;
        Ok(st)
    }
}

/// K7. `SIOCSIFFLAGS` on `lo`, in the realm's own network namespace.
fn bring_loopback_up() -> Result<(), Fail> {
    // The `ifreq` layout, spelled here rather than taken from `libc`: the
    // crate wraps the tail in a union whose field names are not stable across
    // targets, and this call needs two fields. The name array is 16 bytes
    // (`IFNAMSIZ`) and the union is 24, so the flags word sits at offset 16 in
    // both spellings.
    #[repr(C)]
    struct IfReq {
        name: [libc::c_char; 16],
        flags: libc::c_short,
        _union_tail: [u8; 22],
    }
    const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
    const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
    const IFF_UP: libc::c_short = 0x1;

    // SAFETY: the socket is only a handle for the ioctl; nothing is sent on it.
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    step("socket(AF_INET) for the loopback ioctl", sock, Stage::Mount)?;
    let mut req = IfReq {
        name: [0; 16],
        flags: 0,
        _union_tail: [0; 22],
    };
    for (i, b) in b"lo".iter().enumerate() {
        req.name[i] = *b as libc::c_char;
    }
    // SAFETY: `req` matches the kernel's `struct ifreq` for these two commands
    // in the fields they touch, and `sock` is an open `AF_INET` socket.
    let outcome = unsafe {
        if libc::ioctl(sock, SIOCGIFFLAGS as _, &mut req) < 0 {
            step("SIOCGIFFLAGS on lo", -1, Stage::Mount).map(|_| ())
        } else {
            req.flags |= IFF_UP;
            step(
                "SIOCSIFFLAGS on lo",
                libc::ioctl(sock, SIOCSIFFLAGS as _, &req),
                Stage::Mount,
            )
            .map(|_| ())
        }
    };
    unsafe { libc::close(sock) };
    outcome
}

/// K8. The `pivot_root(".", ".")` form: the new root is also the put-old
/// directory, so the old tree is stacked on top of it and detached
/// immediately, leaving no directory in the new tree that names it.
fn pivot(here: &CStr) -> Result<(), Fail> {
    // SAFETY: both arguments name the same live path, which is the documented
    // spelling of this idiom.
    let rc = unsafe { libc::syscall(libc::SYS_pivot_root, here.as_ptr(), here.as_ptr()) };
    if rc < 0 {
        return Err(Fail::at(Stage::PivotRoot));
    }
    // SAFETY: `.` is now the stacked old root.
    check(
        unsafe { libc::umount2(here.as_ptr(), libc::MNT_DETACH) },
        Stage::PivotRoot,
    )?;
    check(unsafe { libc::chdir(c"/".as_ptr()) }, Stage::PivotRoot)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// K10-K14: the last steps before the shim
// ---------------------------------------------------------------------------

fn exec_shim(chan: &Chan, config: &Config) -> Result<std::convert::Infallible, Fail> {
    // K9b. **The realm refuses to create user namespaces inside itself.**
    //
    // Before K10, because writing this file needs `CAP_SYS_RESOURCE` in the
    // realm's own user namespace and K10 is where every capability goes; and
    // after K8, because `/proc` has to be the realm's own procfs by then.
    deny_nested_user_namespaces()?;

    // K10. With an identity map the app would leave `execve` with an empty
    // permitted set anyway -- there is no valid `make_kuid(ns, 0)`, so
    // `cap_bprm_creds_from_file`'s root special case never fires -- but the
    // drop is explicit so the property is a step somebody can read rather than
    // a derivation somebody has to reconstruct.
    drop_all_capabilities()?;

    // Re-armed after the credential change, and read back. `commit_creds`
    // clears `pdeath_signal` when the new credentials are not a subset of the
    // old, and reasoning about which direction that subset test runs is
    // exactly the kind of thing that stays true until a kernel release makes
    // it false. This costs two syscalls and turns an argument into an
    // assertion.
    check(
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) },
        Stage::Internal,
    )?;
    let mut armed: libc::c_int = 0;
    check(
        unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut armed as *mut libc::c_int) },
        Stage::Internal,
    )?;
    if armed != libc::SIGKILL {
        return Err(Fail::with(Stage::Internal, libc::EPERM));
    }

    // K11. A deliberate, argued exception to the core's mask clearing.
    //
    // The shim is about to be PID 1 of a pid namespace, and a `SIG_DFL` signal
    // sent to a namespace init from an ancestor namespace is *discarded* by
    // `sig_task_ignored`. A **blocked** signal is not: `sig_ignored` returns
    // early for anything in `t->blocked`, so the signal stays pending until
    // the shim installs its own handler. The shim installs its signalfd
    // sources only after `wl_display_add_socket`, `wlr_backend_start` and the
    // upstream connection -- so without this line the ladder's rung 2 is a
    // silent no-op for the whole of shim startup. The app is unaffected: the
    // shim resets the mask for its app child.
    //
    // `SIG_SETMASK` and not `SIG_BLOCK`: `SIGCHLD` was blocked before the fork
    // for the supervisor's loop, and a shim that could not see its own app
    // exit would be a worse bug than one that ignores a `SIGTERM`.
    let mask = sigset(&[libc::SIGTERM, libc::SIGINT]);
    check(
        unsafe { libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) },
        Stage::Internal,
    )?;

    // K12. stdin from the **realm's** `/dev/null`, restoring the core's stdin
    // policy one layer down: the operator's keystrokes are not ambient
    // authority the app gets to compete for.
    let devnull = cstr_str("/dev/null")?;
    // SAFETY: `devnull` is a live NUL-terminated path.
    let null_fd = unsafe { libc::open(devnull.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    check(null_fd, Stage::Internal)?;
    check(unsafe { libc::dup3(null_fd, 0, 0) }, Stage::Internal)?;
    unsafe { libc::close(null_fd) };

    // K12b. **The Landlock ruleset** (P2.6.3, #187), and its position in this
    // sequence is forced from both sides.
    //
    // It must come after K10's `PR_SET_NO_NEW_PRIVS` (`landlock_restrict_self`
    // refuses without it) and after the `pivot_root`, because every path it
    // names is an in-realm absolute path. It must come *before* K13 below,
    // because each rule is an `O_PATH` descriptor and `close_range` only
    // MARKS descriptors close-on-exec -- a rule fd opened after it would
    // reach the app unmarked. And everything after it -- the `chdir`, the
    // argv assembly, the `execve` -- must fall inside the granted set, which
    // is why `/vitrin/shim` and `/run/vitrin` are grants rather than
    // afterthoughts.
    //
    // The frame is sent *here*, before `execve`, because after it there is no
    // sender: the channel is `FD_CLOEXEC` and its closing is the success
    // signal. Child-asserted, like `MOUNTED`, and journaled as such.
    let enforced = landlock::apply(config)?;
    chan.send(&Frame::Landlocked {
        rung: enforced.rung,
        kernel_abi: enforced.kernel_abi,
    })?;

    // K13. Not a convention, a syscall. A surviving `O_DIRECTORY` descriptor
    // on the old root is a **complete pivot escape** -- `openat(fd, "../..")`
    // and `fchdir(fd)` both work through it -- and after the `MNT_DETACH`
    // above it is the only remaining handle on the host tree. The range starts
    // at 4 because fd 3 is the shim's core connection and must NOT be marked.
    //
    // Fail closed: a child confined only on the assumption that every `open`
    // in this file remembered `O_CLOEXEC` is not confined, it is trusted.
    check(
        unsafe {
            libc::close_range(
                4,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            )
        },
        Stage::Internal,
    )?;
    let core_flags = unsafe { libc::fcntl(SHIM_CORE_FD, libc::F_GETFD) };
    if core_flags < 0 || core_flags & libc::FD_CLOEXEC != 0 {
        return Err(Fail::with(Stage::Internal, libc::EBADF));
    }

    // K14. The realm's own runtime directory as cwd -- the in-realm spelling
    // of what the core sets on the unconfined path, so a relative path the app
    // resolves lands in its private scratch either way.
    let cwd = cstr_str(IN_REALM_RUNTIME_DIR)?;
    check(unsafe { libc::chdir(cwd.as_ptr()) }, Stage::Exec)?;

    let argv: Vec<CString> = config
        .argv
        .iter()
        .map(|a| cstr(a.as_os_str()))
        .collect::<Result<_, _>>()?;
    let Some(program) = argv.first() else {
        return Err(Fail::with(Stage::Exec, libc::EINVAL));
    };
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());

    // SAFETY: `ptrs` is NULL-terminated and every element points into `argv`,
    // which outlives the call; `environ` is this process's own.
    unsafe {
        libc::execve(program.as_ptr(), ptrs.as_ptr(), environ);
    }
    // Reachable only on failure. The frame still escapes because the config
    // channel is `FD_CLOEXEC`, and `FD_CLOEXEC` takes effect only on a
    // *successful* `execve` -- the same trick std uses to report its own exec
    // failures, and the reason no `READY` frame exists in this protocol.
    Err(Fail::at(Stage::Exec))
}

/// The per-user-namespace ucount limit on how many user namespaces may be
/// created *inside* this one.
///
/// A per-namespace file, not a global one, and that is the whole mechanism:
/// `kernel/ucount.c`'s `set_lookup` returns `current_user_ns()->set`, so the
/// value this process writes is the realm's own and the host's stays untouched.
/// Descendants inherit the refusal because `create_user_ns` charges the new
/// namespace up the whole parent chain and refuses at the first level whose
/// limit is exceeded.
const MAX_USER_NAMESPACES: &str = "/proc/sys/user/max_user_namespaces";

/// K9b. **A realm may not create user namespaces inside itself.**
///
/// # Why this is a hardening and not a workaround for one app
///
/// It removes no capability the app had. **A Landlock filesystem domain
/// forbids `mount(2)` unconditionally** -- measured on this repo's box (Arch,
/// kernel `7.1.8-arch1-3`, Landlock ABI 9, 2026-08-15) with a probe holding the
/// granted rights constant at "everything on `/`" and varying only the handled
/// mask: with `handled_access_fs = 0` (scoped only) the `MS_REC|MS_SLAVE`
/// remount of `/` returns 0; with `EXECUTE` alone handled it returns `EPERM`;
/// with the full rung-9 mask handled *and every one of those rights granted on
/// `/`* it still returns `EPERM`. So mounting is not an access right, no rule
/// can grant it back, and a nested user namespace inside a realm can create no
/// mounts. It was already useless; this makes it unavailable.
///
/// What changes is **which error a nested sandbox receives**, and that turns
/// out to matter more than the capability did. Without this step a sandbox
/// library gets an opaque `EPERM` from a `mount(2)` deep inside its own setup,
/// long after it decided a sandbox was available. With it, the refusal arrives
/// at `unshare(CLONE_NEWUSER)` -- the conventional, decades-old "this host does
/// not allow unprivileged user namespaces" answer that every such library
/// already has a branch for. Measured consequence on this box: `bwrap` prints
/// `Creating new namespace failed`, which is on `glycin`'s known-string list,
/// so `glycin` takes the graceful fallback it already ships instead of dying
/// sandboxed and taking a GTK app's `g_error` down with it.
///
/// **It does not make nested sandboxes work.** They remain structurally
/// impossible inside a realm, for the mount reason above. What it buys is that
/// an app which asks for one now degrades instead of aborting.
///
/// And it removes real attack surface on its own terms: nested user-namespace
/// creation is a recurring source of kernel privilege-escalation CVEs, and a
/// realm -- whose entire mount table is composed by the core before the fork --
/// has no legitimate use for one.
///
/// # Fail closed
///
/// A write that silently did nothing would leave this file describing a
/// property the realm does not have, so the value is read back and a refusal is
/// a refusal. There is no kernel this can legitimately fail on: the file is
/// registered for every user namespace when `CONFIG_USER_NS` is on, and this
/// process is running inside one it just created.
fn deny_nested_user_namespaces() -> Result<(), Fail> {
    // `CAP_SYS_RESOURCE` in the owning user namespace is what makes this file
    // writable at all (`kernel/ucount.c`'s `set_permissions`); everyone else
    // gets read-only. K10 is the step that ends this window, which is why this
    // one comes first.
    if let Err(e) = std::fs::write(MAX_USER_NAMESPACES, b"0\n") {
        eprintln!("vitrin-realm-init: write {MAX_USER_NAMESPACES}=0: {e}");
        return Err(Fail::with(Stage::Internal, e.raw_os_error().unwrap_or(0)));
    }
    // Read back, because "the write returned 0" and "the limit is 0" are two
    // claims and only the second one is the property.
    let back = match std::fs::read_to_string(MAX_USER_NAMESPACES) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("vitrin-realm-init: read back {MAX_USER_NAMESPACES}: {e}");
            return Err(Fail::with(Stage::Internal, e.raw_os_error().unwrap_or(0)));
        }
    };
    if back.trim() != "0" {
        eprintln!(
            "vitrin-realm-init: {MAX_USER_NAMESPACES} read back as {:?}, not 0",
            back.trim()
        );
        return Err(Fail::with(Stage::Internal, libc::EPERM));
    }
    Ok(())
}

/// K10. Bounding set, then the three id sets, then ambient.
///
/// The order is forced: `PR_CAPBSET_DROP` needs `CAP_SETPCAP` in the
/// *effective* set, so the bounding set has to be emptied before the effective
/// set is.
fn drop_all_capabilities() -> Result<(), Fail> {
    check(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        Stage::Internal,
    )?;

    let last = std::fs::read_to_string("/proc/sys/kernel/cap_last_cap")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        // A kernel that will not say gets the whole range attempted; `EINVAL`
        // on an unknown capability is tolerated below, so the fallback costs
        // nothing but syscalls.
        .unwrap_or(63);
    for cap in 0..=last {
        // SAFETY: a prctl with an integer argument and no out-parameter.
        let rc = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) };
        if rc < 0 && errno() != libc::EINVAL {
            return Err(Fail::at(Stage::Internal));
        }
    }

    #[repr(C)]
    struct CapHeader {
        version: u32,
        pid: libc::c_int,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CapData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    let header = CapHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let data = [CapData::default(); 2];
    // SAFETY: both structs match the kernel's `__user_cap_header_struct` and a
    // two-element `__user_cap_data_struct` for capability version 3.
    let rc = unsafe { libc::syscall(libc::SYS_capset, &header as *const CapHeader, data.as_ptr()) };
    if rc < 0 {
        return Err(Fail::at(Stage::Internal));
    }

    // Ambient last. The set empties automatically when permitted does, but
    // stating it makes "the app holds zero capabilities" a list of steps
    // rather than a chain of implications.
    // SAFETY: a prctl with integer arguments and no out-parameter.
    let rc = unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    };
    if rc < 0 && errno() != libc::EINVAL {
        return Err(Fail::at(Stage::Internal));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The Landlock half of this binary, measured rather than described
/// (P2.6.3, issue #187).
///
/// # Why these fork, and what the fork is allowed to touch
///
/// `landlock_restrict_self` is **irreversible**: a process that enforces a
/// domain keeps it until it dies. A test that applied one in-process would
/// confine the whole test binary and every test after it, so each measurement
/// runs in a forked child that enforces, measures one syscall and `_exit`s.
///
/// The cargo test harness is multi-threaded, so the child may call nothing
/// that allocates or locks. That is why [`landlock::create_ruleset`],
/// [`landlock::add_path_rule`] and [`landlock::restrict_self`] are
/// allocation-free and take a `&CStr` the parent built: the split is for this
/// caller, not for tidiness.
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt;
    use vitrin_realm_init::{plan_rung, LandlockRequest, TmpfsCaps, LANDLOCK_BUILD_MAX_RUNG};

    /// Exit codes the forked bodies below use for their own failures, chosen
    /// above every errno so a setup failure can never be read as a kernel
    /// answer.
    const SETUP_FAILED: i32 = 200;

    fn fork_and_measure(body: impl Fn() -> i32) -> i32 {
        // SAFETY: the child reaches `_exit` through syscalls only -- no
        // allocation, no locking, no destructor -- which is the contract every
        // `body` below honors.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
        if pid == 0 {
            let code = body();
            unsafe { libc::_exit(code.clamp(0, 255)) };
        }
        let mut status: libc::c_int = 0;
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(waited, pid, "the measurement child was not reaped");
        assert!(
            libc::WIFEXITED(status),
            "the measurement child did not exit normally"
        );
        libc::WEXITSTATUS(status)
    }

    /// A directory of this test's own, plus one 100-byte file in it.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("vitrin-landlock-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch directory");
            Scratch { dir }
        }

        fn file(&self, name: &str, len: usize) -> PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, vec![b'x'; len]).expect("scratch file");
            path
        }

        fn c(path: &Path) -> CString {
            CString::new(path.as_os_str().as_bytes()).expect("a path with no NUL")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn kernel_abi_or_skip(minimum: u32, what: &str) -> Option<u32> {
        match landlock::kernel_abi() {
            Ok(abi) if abi >= minimum => Some(abi),
            Ok(abi) => {
                eprintln!(
                    "SKIP {what}: this kernel reports Landlock ABI {abi}, and the measurement \
                     needs {minimum}. A skip, loudly, rather than a pass: a test that cannot \
                     run has measured nothing."
                );
                None
            }
            Err(fail) => {
                eprintln!(
                    "SKIP {what}: this kernel answered errno {} to the Landlock ABI query, so \
                     no ruleset can be built here at all.",
                    fail.errno
                );
                None
            }
        }
    }

    #[test]
    fn a_kernel_newer_than_this_build_is_clamped_and_the_clamp_is_visible() {
        // Constructed, never waited for: ABI 10 is already in mainline and
        // absent from this build's headers, so "a kernel this build does not
        // know" is a present-tense fact rather than a future one.
        let plan = plan_rung(42, LandlockRequest::Highest);
        assert_eq!(
            plan.rung, LANDLOCK_BUILD_MAX_RUNG,
            "a kernel above this build's ladder must be clamped down to it"
        );
        assert!(
            plan.clamped_by_build,
            "the clamp happened and was not reported; a build confining a newer kernel at an \
             older rung must say so rather than leave two numbers to be diffed"
        );
        // And the clamp is not claimed where it did not happen.
        let exact = plan_rung(LANDLOCK_BUILD_MAX_RUNG, LandlockRequest::Highest);
        assert_eq!(exact.rung, LANDLOCK_BUILD_MAX_RUNG);
        assert!(!exact.clamped_by_build);
        let older = plan_rung(3, LandlockRequest::Highest);
        assert_eq!(older.rung, 3);
        assert!(!older.clamped_by_build);
    }

    #[test]
    fn the_cap_never_raises_the_rung_above_the_kernel_or_the_build() {
        // **This is the whole of what the cap guarantees, and the name says
        // so.** It replaces a test called `the_cap_can_only_weaken_and_never_
        // raise`, whose second half this still asserts and whose first half
        // was false: see `rung_one_forbids_reparenting_that_the_rung_above_
        // permits` below, which measures rung 1 being STRICTER than rung 2.
        // Comparing rung NUMBERS can never catch that, so the claim moved to
        // a test that compares behaviour and this one stopped making it.
        assert_eq!(plan_rung(9, LandlockRequest::CappedAt(3)).rung, 3);
        assert_eq!(plan_rung(1, LandlockRequest::CappedAt(9)).rung, 1);
        assert_eq!(plan_rung(3, LandlockRequest::CappedAt(3)).rung, 3);
        // A capped session on a kernel above the build's ladder is capped by
        // the operator AND clamped by the build; both are true and the flags
        // do not cancel.
        let both = plan_rung(42, LandlockRequest::CappedAt(2));
        assert_eq!(both.rung, 2);
        assert!(both.clamped_by_build);
        // Exhaustively, over every pair this build can be asked for: the
        // planned rung is never above the kernel's, never above the build's
        // ladder, and never above the cap. Three separate ceilings, each of
        // which has to hold on its own.
        for kernel in 0..=12 {
            for request in [LandlockRequest::Highest]
                .into_iter()
                .chain((1..=LANDLOCK_BUILD_MAX_RUNG).map(LandlockRequest::CappedAt))
            {
                let plan = plan_rung(kernel, request);
                assert!(plan.rung <= kernel, "{request} on kernel {kernel}");
                assert!(
                    plan.rung <= LANDLOCK_BUILD_MAX_RUNG,
                    "{request} on kernel {kernel}"
                );
                if let Some(cap) = request.cap() {
                    assert!(plan.rung <= cap, "{request} on kernel {kernel}");
                }
            }
        }
    }

    /// **The invariant this build actually has**, measured rather than
    /// asserted, and the reason "the cap can only ever weaken" was struck
    /// from every page that said it.
    ///
    /// A Landlock domain denies reparenting -- `rename(2)` and `link(2)`
    /// across directories -- whenever its ruleset does not *handle* `REFER`,
    /// which no ruleset below ABI 2 can. So a realm capped at rung 1 cannot
    /// move a file between two directories **inside its own writable
    /// storage**, and a realm at rung 2 or above can. Climbing the ladder
    /// loosens the domain here; the cap is a dial, not a one-way weakening.
    #[test]
    fn rung_one_forbids_reparenting_that_the_rung_above_permits() {
        let Some(_abi) = kernel_abi_or_skip(2, "the REFER rung") else {
            return;
        };
        let scratch = Scratch::new("refer");
        let from_dir = scratch.dir.join("a");
        let to_dir = scratch.dir.join("b");
        std::fs::create_dir_all(&from_dir).expect("source directory");
        std::fs::create_dir_all(&to_dir).expect("destination directory");
        let dir_c = Scratch::c(&scratch.dir);
        let source = Scratch::c(&from_dir.join("f"));
        let across = Scratch::c(&to_dir.join("f"));
        let within = Scratch::c(&from_dir.join("g"));

        // The realm's own writable-hierarchy rights, spelled by bit so this
        // measurement does not move when the grant table does. `REFER` is in
        // the set: the point is that GRANTING it is not enough below rung 2,
        // because the mask the ruleset HANDLES is what decides.
        const WRITE_TREE: u64 = (1 << 1)
            | (1 << 2)
            | (1 << 3)
            | (1 << 4)
            | (1 << 5)
            | (1 << 7)
            | (1 << 8)
            | (1 << 13)
            | (1 << 14);

        let measure = |rung: u32, rights: u64, to: &CString| -> i32 {
            let _ = std::fs::remove_file(from_dir.join("f"));
            let _ = std::fs::remove_file(to_dir.join("f"));
            let _ = std::fs::remove_file(from_dir.join("g"));
            std::fs::write(from_dir.join("f"), b"x").expect("the file to move");
            // Masked by the rung, exactly as `landlock::apply` masks every
            // grant: a rule naming a bit the ruleset does not handle is
            // `EINVAL`. Doing it here rather than writing two rights
            // constants is what keeps this a measurement of the RUNG with
            // one grant table, which is the comparison being made.
            let rights = rights & landlock::handled_access_fs_for_test(rung);
            fork_and_measure(|| {
                if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
                    return SETUP_FAILED;
                }
                let Ok((ruleset, got)) = landlock::create_ruleset(rung) else {
                    return SETUP_FAILED + 1;
                };
                if got != rung {
                    return SETUP_FAILED + 2;
                }
                if landlock::add_path_rule(ruleset, &dir_c, rights).is_err() {
                    return SETUP_FAILED + 3;
                }
                if landlock::restrict_self(ruleset, 0).is_err() {
                    return SETUP_FAILED + 4;
                }
                if unsafe { libc::rename(source.as_ptr(), to.as_ptr()) } < 0 {
                    return errno();
                }
                0
            })
        };

        // Rung 1: the domain cannot handle `REFER`, so it denies reparenting
        // outright. `EXDEV` and not `EACCES` -- the kernel reports it as a
        // cross-device link so that a caller falls back to copy+unlink rather
        // than treating it as a permission problem.
        assert_eq!(
            measure(1, WRITE_TREE, &across),
            libc::EXDEV,
            "a rung-1 domain must REFUSE a cross-directory rename inside its own granted \
             hierarchy. If this passed, the published claim that `--landlock=abi:1` \
             reproduces an ABI-1 kernel is wrong in the direction that matters"
        );
        // And the denial is reparenting *specifically*, not renaming: the
        // same rung renames within one directory. Without this the assertion
        // above would also pass for a domain that had denied every write.
        assert_eq!(
            measure(1, WRITE_TREE, &within),
            0,
            "a rung-1 domain renamed nothing at all, so the cross-directory denial above is \
             not about reparenting"
        );
        // Rung 2: the same grants, one rung up, and the domain now PERMITS
        // what rung 1 forbade. This is the direction that makes "the cap can
        // only weaken" false.
        assert_eq!(
            measure(2, WRITE_TREE, &across),
            0,
            "rung 2 handles REFER and the hierarchy grants it, so the reparenting rung 1 \
             denied must succeed -- that inequality IS the corrected invariant"
        );
        // Handled but not granted is still a denial, so the rung is not a
        // blanket permission either.
        assert_eq!(
            measure(2, WRITE_TREE & !(1 << 13), &across),
            libc::EXDEV,
            "rung 2 with REFER withheld from the hierarchy must still deny; if it does not, \
             rung 2's permission above came from the RUNG rather than from the grant"
        );
    }

    #[test]
    fn asking_for_no_ruleset_asks_the_kernel_nothing() {
        // The defect this pins: `apply` queried the ABI before reading the
        // request, and `kernel_abi` reports `ENOSYS` as a failure -- so on a
        // kernel with no Landlock at all, EVERY realm spawn died, including
        // the ones that had asked for no ruleset. `--landlock=off` is exactly
        // the flag for such a machine (`spawn/isolation.rs`'s `admit` waives
        // the floor for it), so the flag was documented, refused by the code,
        // and the operator's only remaining move was `--isolation=off` --
        // which drops the namespaces too.
        //
        // The probe answers `ENOSYS` *and records being asked*, because on
        // every machine this suite runs the real query succeeds: without the
        // second half, moving the short-circuit back below the query would
        // leave this test green.
        let asked = Cell::new(false);
        let probe = || {
            asked.set(true);
            Err(Fail::with(Stage::Landlock, libc::ENOSYS))
        };

        let mut config = landlock_test_config();
        config.landlock = LandlockRequest::Off;
        let enforced = landlock::apply_with(&config, probe).expect(
            "a session that asked for NO ruleset must not be refused by a kernel that has no \
             Landlock to give it",
        );
        assert_eq!(enforced.rung, 0);
        assert_eq!(
            enforced.kernel_abi, 0,
            "nothing measured the ABI, so the field is `not measured` and not a rung"
        );
        assert!(
            !asked.get(),
            "`--landlock=off` consulted the kernel. On a kernel without Landlock that \
             consultation is a refusal, and the whole realm spawn dies for a ruleset nobody \
             asked for"
        );

        // Non-vacuity, in the same test: a session that DID ask for a ruleset
        // still fails on the same probe, so the arm above is a short-circuit
        // rather than a swallowed error.
        let asked = Cell::new(false);
        let probe = || {
            asked.set(true);
            Err(Fail::with(Stage::Landlock, libc::ENOSYS))
        };
        let mut config = landlock_test_config();
        config.landlock = LandlockRequest::Highest;
        let fail = landlock::apply_with(&config, probe)
            .expect_err("a session that asked for a ruleset must fail closed");
        assert_eq!(fail.errno, libc::ENOSYS);
        assert!(asked.get(), "the request path must consult the kernel");
    }

    /// A `Config` whose only interesting fields are `landlock` and `binds`.
    ///
    /// Every path in it is deliberately absurd. For the short-circuit tests
    /// that is the assertion: reaching the grant table at all would mean the
    /// short-circuit under test did not happen. For
    /// `a_bind_naming_a_file_gets_rights_the_kernel_accepts`, which builds the
    /// table on purpose, the absurdity is merely harmless -- `grants` composes
    /// strings and `stat`s the `binds` entries, and opens nothing.
    fn landlock_test_config() -> Config {
        Config {
            schema_version: "test".to_string(),
            realm_id: "realm-0".to_string(),
            inner_uid: 1000,
            inner_gid: 1000,
            runtime_dir: PathBuf::from("/nonexistent/run"),
            storage_dir: PathBuf::from("/nonexistent/storage"),
            shim_source: PathBuf::from("/nonexistent/shim"),
            app_dir: None,
            render_nodes: Vec::new(),
            binds: Vec::new(),
            argv: vec![OsString::from("/nonexistent/shim")],
            caps: TmpfsCaps::DEFAULT,
            landlock: LandlockRequest::Off,
        }
    }

    /// **A `binds` entry naming a FILE must not be granted `READ_DIR`**, and
    /// the reason is not tidiness: the rule is refused.
    ///
    /// `examples/realm.toml` documents a `binds` source as "a regular file or
    /// directory", so a file bind is a shipped configuration. The first
    /// version of `landlock::grants` handed every entry `READ_FILE|READ_DIR|
    /// EXECUTE`, with a comment claiming `READ_DIR` on a file "is simply never
    /// checked". The right is indeed never checked -- but the **rule addition**
    /// is `EINVAL`, every bind is `required`, and `apply` fails closed, so one
    /// file bind in `realm.toml` took every realm on the box down at the
    /// shipped default. A crash, from a documented configuration, on the
    /// default path.
    ///
    /// The fix is a branch, and `landlock::grants` takes it from
    /// [`bind_is_file`] -- the **same** predicate the mount table used to
    /// decide `mkdir` or `touch` for the target, so the ruleset cannot describe
    /// a directory where the mount built a file.
    ///
    /// Four assertions, and the last is the one that cannot be argued with: a
    /// directory bind carries read+list+execute, a file bind carries
    /// read+execute with **no** `READ_DIR`, the kernel **accepts** both of
    /// those, and the kernel **refuses** with `EINVAL` exactly what the broken
    /// table produced for a file. That last one is the defect, re-measured on
    /// every run rather than described in a comment; without it the others
    /// would still pass on a kernel that had quietly started tolerating
    /// `READ_DIR` on files.
    ///
    /// No fork: `landlock_create_ruleset` and `landlock_add_rule` confine
    /// nothing. Only `restrict_self` does, and this test never calls it.
    #[test]
    fn a_bind_naming_a_file_gets_rights_the_kernel_accepts() {
        let Some(_abi) = kernel_abi_or_skip(1, "the file-bind grant") else {
            return;
        };
        let scratch = Scratch::new("file-bind");
        let dir_bind = scratch.dir.join("vendor-tree");
        std::fs::create_dir_all(&dir_bind).expect("the directory bind source");
        let file_bind = scratch.file("vendor.so", 64);

        // Spelled by bit, so this measurement does not move when the grant
        // table's named constants do.
        const EXECUTE: u64 = 1 << 0;
        const READ_FILE: u64 = 1 << 2;
        const READ_DIR: u64 = 1 << 3;

        let mut config = landlock_test_config();
        config.binds = vec![dir_bind.clone(), file_bind.clone()];
        // A render node, because it is the THIRD file row and the one that was
        // missed: a bound render node is a character device, `READ_DIR` on a
        // non-directory is `EINVAL` from `landlock_add_rule`, and the rule is
        // required -- so granting it the directory rights took every realm on
        // the box down at the shipped default (measured 2026-08-15, found by
        // `tests/integration/test_real_confinement.py` refusing to boot). The
        // path is composed, never opened, so this needs no GPU on the machine.
        config.render_nodes = vec![PathBuf::from("/dev/dri/renderD128")];
        let table = landlock::grants_for_test(&config).expect("the grant table builds");
        let rights_of = |path: &Path| -> u64 {
            let want = Scratch::c(path);
            table
                .iter()
                .find(|(got, _)| *got == want)
                .unwrap_or_else(|| panic!("no grant for the bind {}", path.display()))
                .1
        };
        assert_eq!(
            rights_of(&dir_bind),
            READ_FILE | READ_DIR | EXECUTE,
            "a DIRECTORY bind carries read, list and execute"
        );
        assert_eq!(
            rights_of(&file_bind),
            READ_FILE | EXECUTE,
            "a FILE bind carries read and execute and crucially NOT READ_DIR, which is \
             what the kernel refuses on a regular file"
        );
        // The regression guard, stated over every `binds` entry rather than only
        // the two constructed above: an operator's bind may name a regular file,
        // so the branch has to be taken per entry and not once for the loop.
        // Deliberately NOT stated over the whole table -- `/usr`, `/etc` and the
        // writable hierarchies carry `READ_DIR` because they are directories by
        // construction; only `binds` and the shim file may not.
        for bind in &config.binds {
            let rights = rights_of(bind);
            if bind_is_file(bind) {
                assert_eq!(
                    rights & READ_DIR,
                    0,
                    "the `binds` grant for the regular file {} carries READ_DIR ({rights:#x}); \
                     the kernel refuses that rule and every bind is required, so this is every \
                     realm on the box refusing to start",
                    bind.display()
                );
            } else {
                assert_eq!(
                    rights & READ_DIR,
                    READ_DIR,
                    "the `binds` grant for the directory {} lost READ_DIR ({rights:#x}), so the \
                     realm cannot list a hierarchy the operator asked for",
                    bind.display()
                );
            }
        }
        // The shim binary is the second file rule, and it is the one whose loss
        // is a realm that will not start at all.
        assert_eq!(
            rights_of(Path::new(IN_REALM_SHIM)),
            READ_FILE | EXECUTE,
            "the shim binary is a FILE bind: read and execute, never READ_DIR"
        );
        // The third: a render node. Asserted as "no READ_DIR" rather than as an
        // exact rights word, because which write and ioctl bits it carries is
        // the render-node policy's business and is argued in `landlock::grants`
        // -- what may never come back is the directory bit.
        let node = rights_of(Path::new("/dev/dri/renderD128"));
        assert_eq!(
            node & READ_DIR,
            0,
            "a render node is a character device and its rule may never name READ_DIR; it \
             carries {node:#x}, and `landlock_add_rule` answers EINVAL for that -- a refused \
             realm, not an ignored bit"
        );
        assert_eq!(
            node & READ_FILE,
            READ_FILE,
            "the render node lost READ_FILE, so the app cannot open the node at all"
        );
        // And there is NO rule on the realm root. That absence is what makes
        // `/vitrin` and the `$HOME` stub answer EACCES inside a real realm
        // (`tests/integration/test_real_confinement.py`), so a widening that
        // added one fails here rather than in an integration gate.
        assert!(
            !table.iter().any(|(path, _)| path.as_c_str()
                == CString::new("/").expect("a NUL-free literal").as_c_str()),
            "the grant table has a rule on the realm root. A READ_DIR rule on `/` covers every \
             directory beneath it, which takes the $HOME canary and the /vitrin denial in \
             tests/integration/test_real_confinement.py red -- measured 2026-08-15, and it buys \
             no app compatibility (see landlock::grants)"
        );

        // The kernel's own verdict on both, which is what makes the pinning
        // above a measurement rather than a restatement of the code.
        let (ruleset, rung) = landlock::create_ruleset(1).expect("a rung-1 ruleset");
        assert_eq!(rung, 1);
        let dir_c = Scratch::c(&dir_bind);
        let file_c = Scratch::c(&file_bind);
        let added = |path: &CString, rights: u64| landlock::add_path_rule(ruleset, path, rights);
        assert!(
            added(&dir_c, rights_of(&dir_bind)).is_ok(),
            "the kernel refused the directory bind's rights"
        );
        assert!(
            added(&file_c, rights_of(&file_bind)).is_ok(),
            "the kernel refused the file bind's rights, so `apply` would refuse the realm"
        );
        // The defect itself: the rights the old table produced for a file.
        let fail = added(&file_c, READ_FILE | READ_DIR | EXECUTE)
            .expect_err("READ_DIR on a REGULAR FILE must be refused by landlock_add_rule");
        assert_eq!(
            fail.errno,
            libc::EINVAL,
            "the refusal must be EINVAL -- if this kernel accepts READ_DIR on a file, the \
             two assertions above are measuring nothing"
        );
        // SAFETY: this test's own descriptor, used nowhere else.
        unsafe { libc::close(ruleset) };
    }

    /// The audit-log diagnostic (P2.6.3 follow-up): **off unless asked for,
    /// and a flags word the kernel actually takes when it is**.
    ///
    /// Landlock's default is to stop auditing a domain's denials at `execve`,
    /// which is the wrong side of the boundary for this project: the denials
    /// worth measuring are the *app's*. `LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON`
    /// moves them back into view.
    ///
    /// Two halves. The table half pins that nothing but an explicit request at
    /// a rung that supports the flag can produce a nonzero flags word -- an
    /// instrument that could switch itself on is not an instrument. The kernel
    /// half runs in a forked child, because it is the one place in this file
    /// that must actually call `restrict_self`, and it measures both
    /// directions: the flag is accepted, and a bit the kernel does *not* define
    /// is refused `EINVAL`. Without the second, "accepted" would be consistent
    /// with a kernel that ignores the whole flags word.
    #[test]
    fn the_audit_log_flag_is_off_unless_asked_for_and_the_kernel_takes_it() {
        const LOG_NEW_EXEC_ON: libc::c_uint = 1 << 1;
        // Every rung, never asked for: zero. This is the shipped path.
        for rung in 1..=LANDLOCK_BUILD_MAX_RUNG {
            assert_eq!(
                landlock::restrict_self_flags_for_test(rung, false),
                0,
                "rung {rung} produced a nonzero flags word without the diagnostic being asked \
                 for; every shipped session enforces with flags=0"
            );
        }
        // Asked for, but below the rung that defines the bit: still zero,
        // because there the bit is EINVAL rather than ignored, and a realm
        // must not fail to start over an instrument.
        for rung in 1..7 {
            assert_eq!(
                landlock::restrict_self_flags_for_test(rung, true),
                0,
                "rung {rung} is below ABI 7, where the log flags do not exist"
            );
        }
        for rung in 7..=LANDLOCK_BUILD_MAX_RUNG {
            assert_eq!(
                landlock::restrict_self_flags_for_test(rung, true),
                LOG_NEW_EXEC_ON,
                "rung {rung} supports the log flags and the diagnostic was asked for"
            );
        }

        let Some(_abi) = kernel_abi_or_skip(7, "the Landlock audit-log flag") else {
            return;
        };
        // The kernel's verdict. `restrict_self` is irreversible, so this is
        // the one thing here that has to fork.
        let measure = |flags: libc::c_uint| -> i32 {
            fork_and_measure(move || {
                if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
                    return SETUP_FAILED;
                }
                let Ok((ruleset, got)) = landlock::create_ruleset(7) else {
                    return SETUP_FAILED + 1;
                };
                if got != 7 {
                    return SETUP_FAILED + 2;
                }
                match landlock::restrict_self(ruleset, flags) {
                    Ok(()) => 0,
                    Err(fail) => fail.errno,
                }
            })
        };
        assert_eq!(
            measure(landlock::restrict_self_flags_for_test(7, true)),
            0,
            "the kernel refused LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON at rung 7, so the \
             diagnostic would take every realm down instead of logging one"
        );
        assert_eq!(
            measure(1 << 30),
            libc::EINVAL,
            "the kernel accepted a flags bit that does not exist, so the acceptance above \
             says nothing about the bit this build passes"
        );
    }

    #[test]
    fn a_realm_can_write_where_it_was_granted_and_nowhere_else() {
        // #187's floor criterion, restated as this build measures it: "a
        // write from inside the realm to a path outside the private storage
        // and outside the designated set fails EACCES". Rung 1 is enough --
        // this is the primitive the whole ladder stands on -- so it runs on
        // every kernel that has Landlock at all.
        let Some(_abi) = kernel_abi_or_skip(1, "the write-set floor") else {
            return;
        };
        let scratch = Scratch::new("write-set");
        let granted = scratch.dir.join("granted");
        let ungranted = scratch.dir.join("ungranted");
        std::fs::create_dir_all(&granted).expect("granted directory");
        std::fs::create_dir_all(&ungranted).expect("ungranted directory");
        let granted_c = Scratch::c(&granted);
        let inside = Scratch::c(&granted.join("new"));
        let outside = Scratch::c(&ungranted.join("new"));

        // The rights the realm's own writable hierarchies carry, minus the
        // ones that need a rung above 1.
        const WRITE_AT_RUNG_1: u64 = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 8);

        for (target, expected, what) in [
            (&inside, 0, "a write inside the granted hierarchy"),
            (&outside, libc::EACCES, "a write outside every granted rule"),
        ] {
            let code = fork_and_measure(|| {
                // Landlock's precondition, which K10 has already set in the
                // real sequence and which a test binary has not.
                if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
                    return SETUP_FAILED;
                }
                let Ok((ruleset, _)) = landlock::create_ruleset(1) else {
                    return SETUP_FAILED + 1;
                };
                if landlock::add_path_rule(ruleset, &granted_c, WRITE_AT_RUNG_1).is_err() {
                    return SETUP_FAILED + 2;
                }
                if landlock::restrict_self(ruleset, 0).is_err() {
                    return SETUP_FAILED + 3;
                }
                let fd = unsafe {
                    libc::open(
                        target.as_ptr(),
                        libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC,
                        0o600,
                    )
                };
                if fd < 0 {
                    return errno();
                }
                unsafe { libc::close(fd) };
                0
            });
            assert_eq!(
                code, expected,
                "{what}: expected {expected}, got {code}. The pair is the point -- the denial \
                 is only evidence because the grant in the same run succeeded"
            );
        }
    }

    #[test]
    fn the_truncate_rung_is_measured_and_its_absence_is_measured_with_it() {
        // The rung the plan row names, and the one whose absence is directly
        // ransomware-relevant: below ABI 3 a payload that cannot WRITE a file
        // can still destroy it.
        //
        // Three runs, and all three are needed. The first shows the rung's
        // absence is a real weakening rather than a story; the second shows
        // the rung closes it; the third is the positive control that keeps the
        // second honest, because "EACCES" could otherwise mean "Landlock is
        // simply on" rather than "this right was withheld".
        let Some(_abi) = kernel_abi_or_skip(3, "the TRUNCATE rung") else {
            return;
        };
        let scratch = Scratch::new("truncate");
        let victim = scratch.file("victim", 100);
        let dir_c = Scratch::c(&scratch.dir);
        let victim_c = Scratch::c(&victim);

        const READ_ONLY: u64 = (1 << 2) | (1 << 3);
        const WRITE_WITH_TRUNCATE: u64 = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 14);

        let measure = |rung: u32, rights: u64| -> (i32, u64) {
            std::fs::write(&victim, vec![b'x'; 100]).expect("re-fill the victim");
            let code = fork_and_measure(|| {
                if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
                    return SETUP_FAILED;
                }
                let Ok((ruleset, got)) = landlock::create_ruleset(rung) else {
                    return SETUP_FAILED + 1;
                };
                if got != rung {
                    // The ladder stepped down under us, so the run would be
                    // measuring a rung nobody asked for.
                    return SETUP_FAILED + 2;
                }
                if landlock::add_path_rule(ruleset, &dir_c, rights).is_err() {
                    return SETUP_FAILED + 3;
                }
                if landlock::restrict_self(ruleset, 0).is_err() {
                    return SETUP_FAILED + 4;
                }
                if unsafe { libc::truncate(victim_c.as_ptr(), 0) } < 0 {
                    return errno();
                }
                0
            });
            let size = std::fs::metadata(&victim)
                .expect("the victim still exists")
                .len();
            (code, size)
        };

        let (code, size) = measure(2, READ_ONLY);
        assert_eq!(
            code, 0,
            "at rung 2 `truncate(2)` is UNGATED, so it must succeed -- that is the weakening \
             the rung above closes, and it is measured rather than asserted"
        );
        assert_eq!(
            size, 0,
            "the file survived a truncate that reported success"
        );

        let (code, size) = measure(3, READ_ONLY);
        assert_eq!(
            code,
            libc::EACCES,
            "at rung 3 `truncate(2)` on a read-granted path must fail EACCES"
        );
        assert_eq!(size, 100, "the file was destroyed despite the denial");

        let (code, size) = measure(3, WRITE_WITH_TRUNCATE);
        assert_eq!(
            code, 0,
            "the positive control failed: with TRUNCATE granted the same call must succeed, or \
             the denial above proves only that Landlock was switched on"
        );
        assert_eq!(size, 0);
    }

    /// The cumulative `handled_access_fs` table, pinned.
    ///
    /// **It replaces a test called `the_rung_masks_grow_and_never_shrink`**,
    /// whose closing assertion -- every rung handles a superset of its
    /// predecessor's bits -- was true, was read as "the ladder only ever
    /// gets stricter", and is in fact the mechanism by which a *higher* rung
    /// permits MORE: a handled bit that the grant table also grants is a
    /// right the domain allows, and an unhandled `REFER` is a denial. The
    /// superset relation is still asserted, because a rung that dropped a
    /// predecessor's bit would be a table edit nobody intended -- but it is
    /// labelled for what it is, and the behavioural claim lives in
    /// `rung_one_forbids_reparenting_that_the_rung_above_permits`.
    #[test]
    fn the_rung_masks_pin_a_measured_table() {
        // Every value here was measured against this kernel rather than
        // recalled, and the test pins the cumulative shape so a hand edit to
        // one rung cannot silently move another.
        let masks: Vec<u64> = (1..=LANDLOCK_BUILD_MAX_RUNG)
            .map(landlock::handled_access_fs_for_test)
            .collect();
        assert_eq!(
            masks,
            vec![
                0x1fff,  // 1
                0x3fff,  // 2: +REFER
                0x7fff,  // 3: +TRUNCATE
                0x7fff,  // 4: net only, the FS mask is flat here
                0xffff,  // 5: +IOCTL_DEV
                0xffff,  // 6: scoped only
                0xffff,  // 7: restrict_self flags only
                0xffff,  // 8: restrict_self flags only
                0x1ffff, // 9: +RESOLVE_UNIX
            ],
            "the cumulative FS masks are a measured table, not a derivation"
        );
        for pair in masks.windows(2) {
            assert_eq!(
                pair[0] & pair[1],
                pair[0],
                "a rung dropped a bit its predecessor handled. That is a table edit, NOT a \
                 statement that the higher rung confines more: `REFER` is handled from rung 2 \
                 and handling it is what LETS a realm reparent files"
            );
        }
        // The one bit whose arrival loosens the domain, named here so the
        // superset assertion above cannot be re-read as a strictness claim.
        const REFER: u64 = 1 << 13;
        assert_eq!(masks[0] & REFER, 0, "rung 1 cannot handle REFER");
        assert_eq!(masks[1] & REFER, REFER, "rung 2 handles it, and permits it");
        // Rung 6 is the only one that moves `scoped`, and it moves nothing on
        // the FS mask -- the flatness above is real and is shown rather than
        // hidden.
        assert_eq!(landlock::scoped_for_test(5), 0);
        assert_eq!(landlock::scoped_for_test(6), 0x3);
        assert_eq!(landlock::scoped_for_test(LANDLOCK_BUILD_MAX_RUNG), 0x3);
    }
}
