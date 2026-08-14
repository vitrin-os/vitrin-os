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

    exec_shim(config)
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
        let is_file = !extra.is_dir();
        binds.push((target, Source::open(extra, is_file)?));
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

fn exec_shim(config: &Config) -> Result<std::convert::Infallible, Fail> {
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
