//! The realm spawn model (P1.5.2, issue #31): how the trusted core launches
//! the one confined app a realm owns, and the *only* place in the TCB that
//! creates a process.
//!
//! [`crate::realm`] owns the description ([`SpawnConfig`]); this module
//! executes it. One call, [`spawn_realm`], performs the whole PRD Doc 2 §4.1
//! sequence: create the core-to-shim socketpair, prepare the realm's private
//! runtime directory, compose the child's environment from nothing, fork,
//! place the shim's end of the socketpair at a fixed descriptor, and `exec`
//! the shim. The shim then serves an app-facing Wayland socket inside that
//! private directory and `exec`s the app -- whose environment names only
//! that socket, so "the app's universe contains only its own shim" is a
//! structural fact rather than a policy the app could disagree with.
//!
//! Everything here is *mechanism*. No window-management, layout, or
//! decoration policy lives in this file (PRD Doc 2 §2, the Nitpicker/Qubes
//! lesson), and nothing here consults or amends the grant table: spawning a
//! realm confers no authority on anybody, and the enforcement chokepoint
//! ([`crate::enforcement`]) remains the single site where authority is
//! checked.
//!
//! # Identity is assigned at fork, never claimed
//!
//! The shim performs **no handshake and presents no credential**. It is
//! born holding one end of a socketpair the core created for exactly one
//! realm, and *holding that descriptor is what makes it that realm's shim*
//! (PRD Doc 2 §4.1; conventions §1.2). There is nothing to steal, forge,
//! replay, or leak: the authority is the descriptor, the descriptor is a
//! kernel object, and it exists in exactly two processes.
//!
//! This is why [`crate::shim::ShimServer`] has no authentication path while
//! [`crate::principal::PrincipalServer`] has an elaborate one. The two
//! connection classes reach the core by different mechanisms *on purpose*:
//! an agent connects to a named socket and must prove who it is; a shim
//! cannot connect at all, and is instead handed its identity by the only
//! process that could have created it.
//!
//! # Which descriptor the shim receives, and why not the alternatives
//!
//! [`SHIM_CORE_FD`] -- **file descriptor 3**, a compile-time constant on
//! both sides, announced nowhere. The alternatives were considered and
//! rejected for confinement reasons, not taste:
//!
//! - **An environment variable naming the number** (the `WAYLAND_SOCKET`
//!   shape) is the worst option available. [`crate::realm::RESERVED_ENV`]
//!   already refuses `WAYLAND_SOCKET` precisely because a variable whose
//!   *value is a descriptor number* survives into every descendant's
//!   environment and turns an inherited fd into ambient authority for
//!   anything that reads `/proc/self/environ`. Minting `VITRIN_CORE_FD`
//!   would rebuild that exact hazard one layer down, in the process whose
//!   whole job is to *be* the confinement boundary.
//! - **argv** is better (it does not survive `exec` into the app) but still
//!   publishes the number through world-readable `/proc/<pid>/cmdline`, and
//!   it adds a parse step -- and therefore a malformed-input failure mode --
//!   to the one interface that must never have one.
//! - **stdin (fd 0), inetd-style** needs no `dup` at all and would let this
//!   module contain zero `unsafe`. It is rejected because it makes the safe
//!   default dangerous: a shim that spawns its app with the ordinary
//!   inherit-stdio behavior would hand the *app* a live core connection.
//!   With fd 3 the ordinary behavior is safe and exactly one deliberate step
//!   is required of the shim (below); with fd 0 the ordinary behavior is a
//!   catastrophic bypass. Confinement beats `unsafe`-avoidance here.
//!
//! **The shim's obligation** (normative for the C shim, E6/#33; demonstrated
//! by `vitrin-mock-shim`): fd 3 arrives with `FD_CLOEXEC` *cleared* -- that
//! is how it survived `execve` -- so the shim must set `FD_CLOEXEC` on it
//! immediately, before it spawns anything. One line, once, at startup;
//! after it, no descendant of the shim can inherit the core connection.
//!
//! # `fork`/`exec` rather than `posix_spawn`, and how the gap stays safe
//!
//! The child needs one thing `posix_spawn` file actions express awkwardly
//! and portably-questionably (a `dup2` onto the same descriptor to clear
//! `FD_CLOEXEC` is a POSIX special case, not an obvious one), so this module
//! uses `fork`/`exec` via [`std::process::Command`] plus
//! [`CommandExt::pre_exec`]. That choice is *forced* rather than incidental:
//! std skips its `posix_spawn` fast path whenever a `pre_exec` closure is
//! registered, so registering one guarantees the fork path deterministically
//! instead of depending on which other options happen to be set.
//!
//! Everything between `fork` and `execve` in a multi-threaded process must
//! be async-signal-safe: only the forking thread survives into the child, so
//! any lock another thread held at fork time is held forever, and the
//! allocator is one such lock. The core *is* multi-threaded (Smithay's
//! backends and their EGL/GL stacks spawn threads), so this is a live
//! hazard, not a formality. It is discharged as follows:
//!
//! - **std does its allocating in the parent.** `Command::spawn` captures
//!   the environment into a `CStringArray`, resolves argv, and opens the
//!   `Stdio::null()` descriptor *before* forking; the post-fork path is
//!   `dup2` for stdio, a `signal(SIGPIPE, SIG_DFL)` reset, the registered
//!   closures, an `environ` pointer swap, and `execvp`. It also takes the
//!   environment read lock across the fork and `mem::forget`s it in the
//!   child, so no non-async-signal-safe unlock happens there.
//! - **The registered closure is two syscalls and no Rust.** It captures a
//!   single `RawFd` by value -- an integer -- and calls `libc::dup3` (or
//!   `libc::fcntl`), then constructs its error with
//!   `io::Error::last_os_error()`, which is `from_raw_os_error` over
//!   `errno` and allocates nothing. There is no `String`, no `Vec`, no
//!   `PathBuf`, no formatting, no logging, and no `Drop` impl in scope that
//!   could take a lock.
//! - **Order is exploited, not assumed.** std runs its stdio `dup2`s
//!   *before* the closures, so the closure's `dup3` onto fd 3 cannot race
//!   or be undone by stdio setup. The parent independently guarantees the
//!   source descriptor is `>= 3` (see [`spawn_realm`]), so stdio placement
//!   can never clobber it either.
//!
//! That is the entire `unsafe` surface of this module: one closure, two
//! possible syscalls, no memory operations.
//!
//! # The realm's private runtime directory
//!
//! Layout is [`vitrin_ipc::paths`]'s (P1.2.1):
//! `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>/`, holding the shim's app-facing
//! `wayland-0` socket. The decisions this task owed:
//!
//! - **The core creates it, before the fork, or the spawn does not happen.**
//!   A child that created its own runtime directory would be choosing its
//!   own confinement, and a directory that appeared *after* `exec` would
//!   leave a window in which `WAYLAND_DISPLAY` named nothing.
//! - **Mode `0700`, owner-only.** It holds a socket that drives the app;
//!   anything wider lets another account on the machine connect to the shim
//!   and operate the realm. Note the honest limit: the core, the shim, and
//!   the app all run as the *same* uid in the MVP, so `0700` bounds other
//!   users, not other processes of this user. Separating those is the
//!   powerbox work (E2.6/E2.7), not something a mode bit can do.
//! - **A pre-existing directory is stale garbage and is purged, not
//!   reused.** Exactly one core serves a given runtime tree (the listener's
//!   `flock` guard enforces that), so a directory already at this path
//!   belongs to a run that is gone; reusing it would carry a dead run's
//!   socket file and scratch into a new run's confinement. It is removed and
//!   recreated -- but only after being opened `O_NOFOLLOW | O_DIRECTORY` and
//!   confirmed to be a real directory owned by this euid, because a blind
//!   recursive delete through a planted symlink is how a cleanup routine
//!   deletes a home directory.
//! - **Removal at exit belongs to P1.5.3 (#32)**, which owns lifecycle. What
//!   this task guarantees is that a crashed run is self-healing at the next
//!   start (the purge above) and that a *failed* spawn leaves nothing behind
//!   ([`RuntimeDirGuard`]): fail-closed means no half-prepared realm.
//!
//! # Environment hygiene
//!
//! The child's environment is built from **nothing** (`env_clear`), then:
//!
//! 1. the realm's allow-listed names ([`SpawnConfig::inherited_env`]), each
//!    filtered against [`crate::realm::RESERVED_ENV`];
//! 2. `WAYLAND_DISPLAY`, the absolute path of the shim's private socket;
//! 3. `XDG_RUNTIME_DIR`, the realm's own private runtime directory.
//!
//! `DISPLAY`, `WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, `XAUTHORITY` and
//! `XDG_RUNTIME_DIR` therefore cannot reach the child with host values *by
//! construction*, independent of what any configuration says. The loader
//! already refuses them ([`crate::realm`]), and this module refuses them
//! again with [`SpawnError::ReservedEnv`] -- deliberately redundant: the
//! filter is the guarantee, the error is the alarm. A reserved name arriving
//! here means the validator was bypassed, and a TCB that silently repairs a
//! bypassed validator hides the bug that matters.
//!
//! Stdio: **stdin is `/dev/null`**, stdout and stderr are inherited. A child
//! sharing the operator's terminal *stdin* would be competing for the
//! operator's keystrokes, which is precisely the ambient input authority
//! this display server exists to mediate. Diagnostics keep their inherited
//! path because a shim that cannot say why it failed to start is
//! undebuggable; the residual cost is that a hostile child can write
//! terminal escape sequences to the operator's terminal -- real, small, and
//! part of the D9 posture stated below.
//!
//! # SECURITY POSTURE OF THE MVP -- READ THIS BEFORE BELIEVING ANY OF THE ABOVE
//!
//! ## D9 (settled): the child is NOT sandboxed in the MVP
//!
//! Confinement here is **environment-structural only**: a private socket, a
//! scrubbed environment, a private runtime directory, and a closed
//! descriptor table. That is the complete list.
//!
//! There are **no namespaces, no seccomp filter, and no Landlock policy**.
//! The spawned shim and its app run as the core's own uid with the core's
//! full view of the filesystem, the network, and every socket on the
//! machine. A confined app that *ignores* `WAYLAND_DISPLAY` and connects
//! directly to a path it already knows is not stopped by anything in this
//! file. PRD Doc 2 §4.1 describes the child as spawned "in an unprivileged
//! sandbox (namespaces/seccomp)"; **this build does not do that yet**, and
//! nothing in this module should be read as claiming otherwise. Real
//! sandboxing arrives with the Phase-2 powerbox (E2.6/E2.7) and the
//! network-authority pillar (PRD P13). Until then, environment hygiene is a
//! *confinement of the well-behaved*, not a containment of the hostile.
//!
//! ## The session D-Bus hole (settled, known, deliberate)
//!
//! The session bus stays reachable in the MVP because Firefox -- the P1
//! acceptance app -- wants it. Precisely what that means here:
//!
//! - The core injects no `DBUS_SESSION_BUS_ADDRESS` and points
//!   `XDG_RUNTIME_DIR` at the realm's private directory, so the bus is not
//!   *advertised* to the child, and a well-behaved client finds nothing.
//! - That is advertisement, not reachability. `/run/user/<uid>/bus` is still
//!   on the filesystem and still connectable by any process of this uid, and
//!   the abstract-socket namespace is still shared. Nothing here prevents a
//!   child from connecting to it directly.
//! - In practice an operator running Firefox will allow-list
//!   `DBUS_SESSION_BUS_ADDRESS` in `realm.toml`, which turns the implicit
//!   hole into an explicit, audited one.
//!
//! Session-bus reach is a lateral-escape path of exactly the shape PRD Doc 2
//! §15 catalogues (D-Bus activation of a privileged helper), and it is
//! closed by **P13** in Phase 2 -- own network namespace plus an empty mount
//! namespace, so there is nothing to reach rather than nothing advertised.
//! It is not closed by this file and cannot be.
//!
//! # Not in this task
//!
//! Crash detection, `SIGCHLD` reaping, exit propagation, and shutdown
//! ordering are all P1.5.3 (#32). This module deliberately keeps the
//! [`std::process::Child`] handle alive inside [`SpawnedRealm`] and never
//! waits on it, so #32 inherits an unreaped, unlost process handle rather
//! than having to re-derive one from a pid.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use rustix::fs::{FileType, Mode, OFlags};
use vitrin_ipc::paths;
use vitrin_ipc::{Connection, TransportError};

use crate::grants::RealmId;
use crate::realm::{untrusted_writer, Realm, SpawnConfig, RESERVED_ENV};
use crate::recorder::{Event, Recorder};
use crate::shim::{ShimConfig, ShimServer};

/// The descriptor the shim's end of the core socketpair occupies in the
/// spawned child: **3**, the first descriptor past stdio, fixed at compile
/// time on both sides and communicated by nothing (module docs).
///
/// Holding this descriptor *is* being this realm's shim. The shim must set
/// `FD_CLOEXEC` on it at startup so no app it later spawns inherits it.
pub(crate) const SHIM_CORE_FD: RawFd = 3;

/// Mode of a realm's private runtime directory: owner-only. It holds the
/// shim's app-facing Wayland socket (module docs).
const RUNTIME_DIR_MODE: u32 = 0o700;

/// Where a session's runtime tree lives. Held explicitly rather than read
/// from the environment at each use so the spawn path is deterministic and
/// testable against a scratch base -- the same reason
/// [`vitrin_ipc::paths`] ships a `*_in` form of every helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnPaths {
    xdg_runtime_dir: PathBuf,
}

impl SpawnPaths {
    /// The session's real runtime tree, from `$XDG_RUNTIME_DIR`.
    pub fn from_env() -> Result<Self, paths::PathError> {
        Ok(Self {
            xdg_runtime_dir: paths::xdg_runtime_dir()?,
        })
    }

    /// A runtime tree rooted at an explicit base (tests, and any future
    /// caller that must not depend on ambient environment).
    pub fn under(xdg_runtime_dir: impl Into<PathBuf>) -> Self {
        Self {
            xdg_runtime_dir: xdg_runtime_dir.into(),
        }
    }

    /// `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>` -- validating the id through
    /// the transport's own rule, which is what keeps "legal realm id" a
    /// single definition across the two crates.
    fn realm_dir(&self, realm_id: &str) -> Result<PathBuf, paths::PathError> {
        paths::shim_runtime_dir_in(&self.xdg_runtime_dir, realm_id)
    }

    /// `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>/wayland-0` -- the value the
    /// child's `WAYLAND_DISPLAY` carries, absolute.
    fn shim_socket(&self, realm_id: &str) -> Result<PathBuf, paths::PathError> {
        paths::shim_socket_path_in(&self.xdg_runtime_dir, realm_id)
    }
}

/// A realm whose app has been launched: the core's half of the identity
/// pair (the socketpair end the shim does *not* hold), the process handle,
/// and the private directory the realm was given.
///
/// The [`Child`] is retained and never waited on: reaping, crash detection,
/// and exit propagation are P1.5.3 (#32), and losing the handle here would
/// force that task to re-derive one from a pid -- which is exactly the
/// pid-reuse race a `Child` exists to avoid.
#[derive(Debug)]
pub(crate) struct SpawnedRealm {
    realm_id: RealmId,
    child: Child,
    runtime_dir: PathBuf,
    connection: Connection,
}

impl SpawnedRealm {
    /// The realm this process serves -- the same id the wire addresses and
    /// grant rows key on.
    pub fn realm_id(&self) -> &RealmId {
        &self.realm_id
    }

    /// The shim's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The realm's private runtime directory (mode `0700`), which the shim
    /// binds its app-facing socket inside.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// The core's end of the identity socketpair, for the event loop to
    /// register (`AsFd`) and dispatch.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// The process handle, unreaped. Exposed so P1.5.3 (#32) -- and, today,
    /// this module's tests -- can terminate and wait deterministically.
    /// This task deliberately implements no lifecycle policy of its own.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Bring the shim session up on the inherited connection: build the
    /// per-connection [`ShimServer`] this realm's socketpair terminates in
    /// and send `vitrin_shim_session.configure`, the core's guaranteed-first
    /// message on a shim connection (P1.3.4).
    ///
    /// This is the whole wiring between "a process exists" and "the core
    /// serves it": the server's realm identity comes from the socketpair's
    /// realm, never from anything the shim said, which is identity-at-fork
    /// expressed one layer up.
    ///
    /// Blocking, deliberately: `configure` is a few dozen bytes into an
    /// empty kernel socket buffer on a freshly created pair, so it cannot
    /// park the compositor. Everything after it belongs to the event loop's
    /// non-blocking path.
    pub fn start_shim_session(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<ShimServer, TransportError> {
        let server = ShimServer::new(ShimConfig {
            realm: self.realm_id.as_str().to_string(),
            width,
            height,
        });
        let conn = &mut self.connection;
        server.send_configure(&mut |bytes: &[u8]| conn.send_message(bytes, None))?;
        Ok(server)
    }
}

/// Why a spawn did not happen. Every variant is a **refusal**, never a
/// partially-launched realm: the guard in [`spawn_realm`] removes anything
/// this call created before the error leaves the function (fail closed -- a
/// half-confined child is worse than no child).
#[derive(Debug)]
pub(crate) enum SpawnError {
    /// The realm id does not name a legal runtime directory.
    Path(paths::PathError),
    /// The realm's private runtime directory could not be prepared.
    RuntimeDir { path: PathBuf, detail: String },
    /// The program failed the trusted-writer audit *at spawn time* -- the
    /// re-check on the descriptor, not a re-reading of the config.
    ProgramAudit { path: PathBuf, detail: String },
    /// A [`RESERVED_ENV`] name reached the spawn path, which is only
    /// possible if the config validator was bypassed (module docs).
    ReservedEnv { name: &'static str },
    /// The core-to-shim socketpair could not be created.
    Socketpair(io::Error),
    /// `fork`/`exec` itself failed (no such program, not executable, a
    /// resource limit).
    Exec { command: PathBuf, source: io::Error },
}

impl SpawnError {
    /// A fixed label for the flight recorder. Never free-form `Display`
    /// text: the recorder's convention is that a `cause_class` is drawn
    /// from a closed vocabulary a reader can switch on.
    pub fn cause_class(&self) -> &'static str {
        match self {
            SpawnError::Path(_) => "invalid_realm_id",
            SpawnError::RuntimeDir { .. } => "runtime_dir",
            SpawnError::ProgramAudit { .. } => "program_audit",
            SpawnError::ReservedEnv { .. } => "reserved_env",
            SpawnError::Socketpair(_) => "socketpair",
            SpawnError::Exec { .. } => "exec",
        }
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::Path(e) => write!(f, "realm runtime directory: {e}"),
            SpawnError::RuntimeDir { path, detail } => {
                write!(f, "realm runtime directory {}: {detail}", path.display())
            }
            SpawnError::ProgramAudit { path, detail } => write!(
                f,
                "refusing to exec {}: {detail} (re-checked at spawn time, not at config load)",
                path.display()
            ),
            SpawnError::ReservedEnv { name } => write!(
                f,
                "environment name {name:?} is decided by the core, never by config; it \
                 reached the spawn path, which means realm config validation was bypassed"
            ),
            SpawnError::Socketpair(e) => write!(f, "core-to-shim socketpair: {e}"),
            SpawnError::Exec { command, source } => {
                write!(f, "exec {}: {source}", command.display())
            }
        }
    }
}

impl std::error::Error for SpawnError {}

impl From<paths::PathError> for SpawnError {
    fn from(e: paths::PathError) -> Self {
        SpawnError::Path(e)
    }
}

/// Launch a realm's app (PRD Doc 2 §4.1). On success the realm's shim is
/// running with the returned connection's peer end at [`SHIM_CORE_FD`], its
/// private runtime directory exists at mode `0700`, and its environment
/// names that directory's socket and nothing of the host session.
///
/// Every failure is total: no process, no directory, no descriptor left
/// over. See the module docs for the D9 sandboxing deferral -- the child is
/// confined by environment structure only.
pub(crate) fn spawn_realm(
    realm: &Realm,
    paths: &SpawnPaths,
    recorder: &mut Recorder,
) -> Result<SpawnedRealm, SpawnError> {
    spawn_realm_with_env(realm, paths, recorder, |name| std::env::var(name).ok())
}

/// [`spawn_realm`] with the core's environment supplied explicitly, so the
/// allowlist's semantics are testable without mutating process-global state
/// (the same seam [`SpawnConfig::inherited_env`] exists for).
///
/// The [`Recorder`] is a required parameter rather than something a caller
/// may remember: *what the trusted core executed* is the most
/// security-relevant act of a session, and both outcomes are journaled here,
/// in one funnel around [`launch`]. A future error path added inside
/// `launch` is covered structurally instead of by remembering to add a call
/// -- the same reasoning the recorder applies to undelivered resolutions.
pub(crate) fn spawn_realm_with_env<F>(
    realm: &Realm,
    paths: &SpawnPaths,
    recorder: &mut Recorder,
    lookup: F,
) -> Result<SpawnedRealm, SpawnError>
where
    F: Fn(&str) -> Option<String>,
{
    let result = launch(realm, paths, lookup);
    match &result {
        Ok(spawned) => recorder.record(Event::RealmSpawned {
            realm: realm.id(),
            pid: spawned.pid(),
            command: realm.spawn().command(),
            runtime_dir: spawned.runtime_dir(),
            env_allow: realm.spawn().env_allow(),
        }),
        Err(err) => recorder.record(Event::RealmSpawnFailed {
            realm: realm.id(),
            command: realm.spawn().command(),
            cause_class: err.cause_class(),
        }),
    }
    result
}

/// The spawn itself. Separated from the journaling wrapper above so no
/// return path can escape the log.
fn launch<F>(realm: &Realm, paths: &SpawnPaths, lookup: F) -> Result<SpawnedRealm, SpawnError>
where
    F: Fn(&str) -> Option<String>,
{
    let realm_id = realm.id().as_str();
    let runtime_dir = paths.realm_dir(realm_id)?;
    let socket_path = paths.shim_socket(realm_id)?;
    let spawn = realm.spawn();

    // Preconditions first, in the order that leaves the least behind: pure
    // checks before anything is created, so the common refusals never reach
    // the guard below at all.
    reject_reserved_env(spawn)?;
    audit_program_at_spawn(spawn.command())?;

    // From here on the call owns filesystem state; the guard unwinds it on
    // every error path (fail closed).
    let guard = RuntimeDirGuard::create(&runtime_dir)?;

    // The identity pair. `Connection::pair` is the transport's own
    // socketpair primitive: `SOCK_CLOEXEC` from birth, so neither end can
    // leak into an unrelated concurrent spawn.
    let (core_side, shim_side) = Connection::pair().map_err(SpawnError::Socketpair)?;

    // Materialize the child's end as a bare descriptor guaranteed to be
    // >= SHIM_CORE_FD. That guarantee is load-bearing: std places the
    // child's stdio with `dup2` onto 0/1/2 before running our closure, so a
    // source descriptor below 3 could be silently clobbered between fork and
    // our `dup3`. `F_DUPFD_CLOEXEC` gives the lowest free descriptor at or
    // above the floor, still close-on-exec in this process.
    let shim_fd: OwnedFd = rustix::io::fcntl_dupfd_cloexec(shim_side.as_fd(), SHIM_CORE_FD)
        .map_err(|e| SpawnError::Socketpair(e.into()))?;
    drop(shim_side);
    let shim_raw = shim_fd.as_raw_fd();

    let env = child_env(spawn, &socket_path, &runtime_dir, lookup);

    let mut cmd = Command::new(spawn.command());
    cmd.args(spawn.args());
    // Default-deny: the child's environment starts empty and receives only
    // what `child_env` composed. `env_clear` before `envs` is what makes
    // "the core's session environment never reaches the app" structural.
    cmd.env_clear();
    cmd.envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())));
    // The realm's own directory, so a relative path the app resolves lands
    // in its private scratch rather than wherever vitrind was started from.
    cmd.current_dir(&runtime_dir);
    // Module docs: the operator's keystrokes are not ambient authority the
    // app gets to compete for. Diagnostics keep their inherited path.
    cmd.stdin(Stdio::null());

    // SAFETY: this closure runs in the forked child, between `fork` and
    // `execve`, where only async-signal-safe operations are permitted (the
    // core is multi-threaded; any lock held by another thread at fork time
    // -- the allocator's included -- is held forever in the child).
    //
    // It satisfies that requirement by construction:
    //   * it captures `shim_raw`, a `RawFd`, by value -- an integer, no heap
    //     data, no `Drop` type in scope;
    //   * it performs exactly one syscall, `dup3` (or `fcntl` when the
    //     source already sits on the target descriptor, where `dup3` would
    //     return EINVAL), both async-signal-safe per signal-safety(7);
    //   * `io::Error::last_os_error()` is `from_raw_os_error` over `errno`
    //     and allocates nothing;
    //   * it does not allocate, lock, log, format, or call back into Rust
    //     runtime machinery.
    //
    // `shim_raw` is a valid open descriptor in the child: `shim_fd` is alive
    // in the parent until after `spawn()` returns, and the child inherits the
    // fork-time descriptor table. Clearing FD_CLOEXEC happens *only here*, in
    // the child -- doing it in the parent would expose the descriptor to any
    // other thread's concurrent `exec`.
    unsafe {
        cmd.pre_exec(move || {
            let rc = if shim_raw == SHIM_CORE_FD {
                libc::fcntl(SHIM_CORE_FD, libc::F_SETFD, 0)
            } else {
                libc::dup3(shim_raw, SHIM_CORE_FD, 0)
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn().map_err(|source| SpawnError::Exec {
        command: spawn.command().to_path_buf(),
        source,
    })?;

    // The parent's copy of the child's end closes here: from now on exactly
    // one process other than the core holds it, which is what makes "holding
    // this descriptor is being realm N's shim" true rather than aspirational.
    drop(shim_fd);
    guard.keep();

    Ok(SpawnedRealm {
        realm_id: realm.id().clone(),
        child,
        runtime_dir,
        connection: core_side,
    })
}

/// Compose the child's environment: allow-listed inheritance first, then the
/// core's injections, which therefore win any collision. Reserved names are
/// filtered here regardless of what reached this function -- the structural
/// half of the guarantee (module docs); [`reject_reserved_env`] is the alarm.
fn child_env<F>(
    spawn: &SpawnConfig,
    socket_path: &Path,
    runtime_dir: &Path,
    lookup: F,
) -> Vec<(OsString, OsString)>
where
    F: Fn(&str) -> Option<String>,
{
    let mut env: Vec<(OsString, OsString)> = spawn
        .inherited_env(lookup)
        .into_iter()
        .filter(|(name, _)| !is_reserved_env(name))
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    // The confinement itself: an absolute path, so libwayland connects to
    // exactly this socket and never derives one from a runtime directory.
    env.push((
        OsString::from("WAYLAND_DISPLAY"),
        socket_path.as_os_str().to_os_string(),
    ));
    // Not the host session's: that one holds the host compositor socket, the
    // session bus, and agent sockets (the reason `RESERVED_ENV` refuses it).
    env.push((
        OsString::from("XDG_RUNTIME_DIR"),
        runtime_dir.as_os_str().to_os_string(),
    ));
    env
}

fn is_reserved_env(name: &str) -> bool {
    RESERVED_ENV.iter().any(|(reserved, _)| *reserved == name)
}

/// Refuse a spawn whose allowlist names a variable the core decides. The
/// config loader already refuses these, so reaching this check means the
/// loader was bypassed -- and a TCB that silently repairs a bypassed
/// validator hides the bug worth finding (module docs).
fn reject_reserved_env(spawn: &SpawnConfig) -> Result<(), SpawnError> {
    for name in spawn.env_allow() {
        if let Some((reserved, _)) = RESERVED_ENV.iter().find(|(r, _)| *r == name.as_str()) {
            return Err(SpawnError::ReservedEnv { name: reserved });
        }
    }
    Ok(())
}

/// Re-audit the program **at spawn time**, on an opened descriptor, against
/// the same trusted-writer rule `realm.toml` loading applies
/// ([`untrusted_writer`] -- one definition, two call sites).
///
/// This is deliberately not a re-run of the config-load audit: that one
/// proved the operator's configuration was coherent, possibly minutes ago
/// and certainly before this process decided to exec anything. What it could
/// never prove is anything about the instant of `execve`, and
/// `crate::realm`'s docs hand that question here.
///
/// What remains honest about the residual window: this checks an inode
/// through an open descriptor and then `execve`s a *path*, so a swap between
/// the two is not excluded. Closing it fully needs `fexecve` on this
/// descriptor. It is left open because the audit already proves only root or
/// this uid can perform the swap, and same-uid separation is exactly what D9
/// defers -- a `fexecve` here would be the one hardened step in an otherwise
/// unsandboxed spawn. It lands with the powerbox (E2.6/E2.7), which
/// re-opens the exec primitive anyway.
fn audit_program_at_spawn(command: &Path) -> Result<(), SpawnError> {
    let refuse = |detail: String| SpawnError::ProgramAudit {
        path: command.to_path_buf(),
        detail,
    };
    // O_PATH: the audit needs the inode's metadata, not its contents, and
    // an O_PATH descriptor needs no read permission on the program.
    let fd = rustix::fs::open(command, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| refuse(format!("cannot open the program ({e})")))?;
    let st =
        rustix::fs::fstat(&fd).map_err(|e| refuse(format!("cannot stat the program ({e})")))?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(refuse("not a regular file".into()));
    }
    let euid = rustix::process::geteuid().as_raw();
    if let Some(fault) = untrusted_writer(st.st_uid, st.st_mode, euid, false) {
        return Err(refuse(format!(
            "{fault}; whoever can write the program the trusted core execs chooses what it runs"
        )));
    }
    Ok(())
}

/// Owns the realm's private runtime directory for the duration of a spawn
/// attempt: created on construction, removed on drop unless [`keep`] was
/// called. This is what makes a failed spawn leave *nothing* behind, in
/// every error path including ones added later.
///
/// [`keep`]: RuntimeDirGuard::keep
#[derive(Debug)]
struct RuntimeDirGuard {
    path: PathBuf,
    armed: bool,
}

impl RuntimeDirGuard {
    /// Create `$XDG_RUNTIME_DIR/vitrin-0/<realm>` fresh at mode `0700`,
    /// purging a stale directory left by a run that is gone (module docs).
    fn create(path: &Path) -> Result<Self, SpawnError> {
        let refuse = |detail: String| SpawnError::RuntimeDir {
            path: path.to_path_buf(),
            detail,
        };

        // The enclosing `vitrin-0` tree may not exist yet: the listener
        // creates it when it binds, and a spawn can precede that.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| refuse(format!("cannot create {}: {e}", parent.display())))?;
            fs::set_permissions(parent, fs::Permissions::from_mode(RUNTIME_DIR_MODE))
                .map_err(|e| refuse(format!("cannot chmod {}: {e}", parent.display())))?;
        }

        match rustix::fs::mkdir(path, Mode::from_bits_truncate(RUNTIME_DIR_MODE)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => {
                purge_stale_runtime_dir(path).map_err(refuse)?;
                rustix::fs::mkdir(path, Mode::from_bits_truncate(RUNTIME_DIR_MODE)).map_err(
                    |e| refuse(format!("cannot recreate after purging a stale one: {e}")),
                )?;
            }
            Err(e) => return Err(refuse(format!("cannot create: {e}"))),
        }
        // `mkdir`'s mode is masked by the process umask, so state the mode
        // rather than hoping for it: this directory holds the socket that
        // drives the realm.
        fs::set_permissions(path, fs::Permissions::from_mode(RUNTIME_DIR_MODE))
            .map_err(|e| refuse(format!("cannot chmod to {RUNTIME_DIR_MODE:o}: {e}")))?;

        Ok(Self {
            path: path.to_path_buf(),
            armed: true,
        })
    }

    /// The spawn succeeded: the directory now belongs to the running realm,
    /// and its removal is P1.5.3's (#32).
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for RuntimeDirGuard {
    fn drop(&mut self) {
        if self.armed {
            // Best effort by necessity (a Drop cannot report), but the
            // failure mode is benign: a leftover directory is purged by the
            // next spawn's stale check.
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Remove a runtime directory left by a previous run -- after proving it is
/// one. The `O_NOFOLLOW | O_DIRECTORY` open is the whole point: a recursive
/// delete that followed a planted symlink is how a cleanup routine deletes
/// someone's home directory.
fn purge_stale_runtime_dir(path: &Path) -> Result<(), String> {
    let fd = rustix::fs::open(
        path,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| {
        format!("already exists and is not a directory this core may replace ({e}); refusing")
    })?;
    let st =
        rustix::fs::fstat(&fd).map_err(|e| format!("already exists but cannot stat it: {e}"))?;
    // Stated rather than inferred from the open flags. `O_PATH | O_NOFOLLOW`
    // deliberately opens a *symlink itself* instead of failing, and it is
    // only the accompanying `O_DIRECTORY` that rejects one -- an interaction
    // subtle enough that the recursive delete below should not rest on it.
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != FileType::Directory {
        return Err("already exists and is not a directory; refusing to replace it".into());
    }
    let euid = rustix::process::geteuid().as_raw();
    if st.st_uid != euid {
        return Err(format!(
            "already exists and is owned by uid {}, not the core's uid {euid}; refusing to \
             replace a directory this core did not create",
            st.st_uid
        ));
    }
    drop(fd);
    fs::remove_dir_all(path).map_err(|e| format!("cannot purge the stale directory: {e}"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Read;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::capture::tests::fd_lock;
    use crate::realm::tests::realm_with_spawn;
    use crate::recorder::tests::{read_log, scratch_recorder, Json};

    /// How long a test waits for a child to reach a state it must reach
    /// (exec'd, forked a grandchild). Generous: these are process
    /// operations on a loaded CI runner, not a latency assertion.
    const DEADLINE: Duration = Duration::from_secs(10);

    /// A private scratch tree standing in for `$XDG_RUNTIME_DIR`.
    fn scratch() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vitrin-spawn-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// The `vitrin-mock-shim` executable (the permanent fixture crate's
    /// binary target), found beside the test binary in the cargo profile
    /// directory.
    ///
    /// There is no `CARGO_BIN_EXE_*` for it here: that variable exists only
    /// for the *defining* package's integration tests, and `vitrin-core` is
    /// a binary-only crate whose tests are unit tests.
    ///
    /// What guarantees the binary exists at all is
    /// `crates/vitrin-mock-shim/tests/binary_contract.rs`: Cargo builds a
    /// package's binary targets during `cargo test` exactly when that
    /// package has an integration test. Without it `cargo test --workspace`
    /// would compile the mock shim only in test mode and every spawn test
    /// here would fail to find a program to exec -- which is why that file
    /// says, in its own header, not to delete it.
    ///
    /// A bare `cargo test -p vitrin-core` still needs
    /// `cargo build -p vitrin-mock-shim` first, which the panic says.
    fn mock_shim_bin() -> PathBuf {
        let exe = std::env::current_exe().expect("the test binary has a path");
        // .../target/<profile>/deps/<test-bin>
        let deps = exe.parent().expect("test binary has a parent directory");
        let mut candidates = vec![deps.join("vitrin-mock-shim")];
        if let Some(profile) = deps.parent() {
            candidates.push(profile.join("vitrin-mock-shim"));
        }
        for candidate in &candidates {
            if candidate.is_file() {
                return candidate.clone();
            }
        }
        panic!(
            "vitrin-mock-shim binary not found in {candidates:?}; run \
             `cargo build -p vitrin-mock-shim` (CI's `cargo test --workspace` builds it)"
        );
    }

    /// Poll until `f` returns `Some`, or fail with `what` after [`DEADLINE`].
    fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Block until `pid` has actually `execve`d `program`.
    ///
    /// Necessary before reading the child's `/proc` state: between `fork`
    /// and `execve` the child is still a copy of *this* process, so
    /// `/proc/<pid>/environ` would report the test harness's environment and
    /// `/proc/<pid>/fd` its descriptors. `/proc/<pid>/exe` flipping to the
    /// spawned program is the observable moment that copy is gone.
    fn wait_for_exec(pid: u32, program: &Path) {
        let expected = fs::canonicalize(program).expect("the program resolves");
        wait_for(&format!("pid {pid} to exec {}", expected.display()), || {
            let link = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
            (link == expected).then_some(())
        });
    }

    /// A child's environment, read from `/proc/<pid>/environ` -- the
    /// kernel's copy of the `envp` the child was `execve`d with.
    ///
    /// Read from the kernel rather than from a self-report by the child on
    /// purpose: it is the environment the child *actually has*, it cannot be
    /// embellished by a cooperative reporter, and it is emphatically not the
    /// parent's intent (which lives in a `Vec` this test never looks at).
    fn child_env_of(pid: u32) -> BTreeMap<String, String> {
        let mut raw = Vec::new();
        fs::File::open(format!("/proc/{pid}/environ"))
            .expect("child environ is readable")
            .read_to_end(&mut raw)
            .expect("child environ reads");
        raw.split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .map(|entry| {
                let text = String::from_utf8_lossy(entry).into_owned();
                match text.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (text, String::new()),
                }
            })
            .collect()
    }

    /// A child's open descriptors, as `number -> readlink target`.
    fn child_fds_of(pid: u32) -> BTreeMap<i32, String> {
        fs::read_dir(format!("/proc/{pid}/fd"))
            .expect("child fd directory is readable")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let number: i32 = entry.file_name().to_str()?.parse().ok()?;
                let target = fs::read_link(entry.path())
                    .map(|p| p.to_string_lossy().into_owned())
                    // A descriptor can close between listing and readlink;
                    // record it rather than dropping it, so the count check
                    // below can never be weakened by a race.
                    .unwrap_or_else(|_| "<unreadable>".to_string());
                Some((number, target))
            })
            .collect()
    }

    /// The parent of `pid`, from `/proc/<pid>/status`. The exact,
    /// race-free spelling of "is X a child of Y", and the one the
    /// topology assertions use.
    ///
    /// Deliberately *not* `/proc/<pid>/task/<tid>/children`: that file
    /// lists the children of one **thread**, and the test harness is
    /// multi-threaded, so it answers a different question than the one
    /// being asked and silently omits a child forked on another thread.
    fn ppid_of(pid: u32) -> Option<u32> {
        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))?
            .trim()
            .parse()
            .ok()
    }

    /// Direct children of `pid`, by scanning procfs for processes whose
    /// parent is `pid` (see [`ppid_of`] for why the per-thread `children`
    /// file is not used).
    fn children_of(pid: u32) -> BTreeSet<u32> {
        let mut found = BTreeSet::new();
        let Ok(entries) = fs::read_dir("/proc") else {
            return found;
        };
        for entry in entries.flatten() {
            let Some(candidate) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if ppid_of(candidate) == Some(pid) {
                found.insert(candidate);
            }
        }
        found
    }

    /// One spawn test's world: the scratch runtime tree, the run's flight
    /// recorder (a required argument of every spawn), and the log path.
    struct Harness {
        base: PathBuf,
        recorder: Recorder,
        log: PathBuf,
    }

    impl Harness {
        fn new(label: &str) -> Self {
            let (recorder, log) = scratch_recorder(label);
            Self {
                base: scratch(),
                recorder,
                log,
            }
        }

        fn paths(&self) -> SpawnPaths {
            SpawnPaths::under(&self.base)
        }

        /// Every recorded entry, parsed. Closes the log first, because a
        /// footer-less file is not what a reader would ever see.
        fn entries(&mut self) -> Vec<Json> {
            self.recorder.finish();
            read_log(&self.log)
        }

        /// Spawn the mock shim into this harness's runtime tree and bring
        /// its session up, returning the live realm and the [`ShimServer`]
        /// its socketpair terminates in.
        ///
        /// Bring-up is part of the helper rather than each test's preamble
        /// because it is also the **readiness gate** every `/proc`
        /// assertion needs. `wait_for_exec` only proves the kernel has
        /// installed the new program image; the dynamic loader is still
        /// running at that instant, and a descriptor-table snapshot taken
        /// then catches ld.so's transient `libc.so` handle. A `create_surface`
        /// arriving from the child is proof it is executing its *own* code,
        /// which is the earliest moment its `/proc` state means what the
        /// tests read it to mean.
        fn spawn_mock(
            &mut self,
            args: &[&str],
            env_allow: &[&str],
            core_env: &[(&str, &str)],
        ) -> (SpawnedRealm, ShimServer) {
            let bin = mock_shim_bin();
            let realm = realm_with_spawn(
                "realm-0",
                &bin,
                &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                &env_allow.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
            );
            let core_env: BTreeMap<String, String> = core_env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let paths = self.paths();
            let mut spawned = spawn_realm_with_env(&realm, &paths, &mut self.recorder, |name| {
                core_env.get(name).cloned()
            })
            .expect("spawn must succeed against a scratch runtime tree");
            wait_for_exec(spawned.pid(), &bin);

            // The wiring under test: the inherited socketpair becomes a
            // served shim session, and the shim's first request is
            // dispatched through the real `ShimServer` -- so this is an
            // end-to-end proof that identity-at-fork produces exactly the
            // connection P1.3.4's server expects.
            let mut server = spawned
                .start_shim_session(1280, 800)
                .expect("configure must reach the shim over the inherited socketpair");
            let msg = spawned
                .connection_mut()
                .recv_message()
                .expect("the shim's first request must arrive")
                .expect("the shim must not hang up during bring-up");
            let mut scene = crate::scene::Scene::new();
            let conn = spawned.connection_mut();
            server
                .handle_message(msg, &mut scene, None, &mut |bytes: &[u8]| {
                    conn.send_message(bytes, None)
                })
                .expect("the shim's bring-up request must be well-formed");
            (spawned, server)
        }

        /// Terminate the child and reap it, so no test leaves a zombie
        /// behind. (Lifecycle *policy* is #32's; this is test hygiene.)
        fn reap(&self, mut spawned: SpawnedRealm) {
            let _ = spawned.child_mut().kill();
            let _ = spawned.child_mut().wait();
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
            if let Some(dir) = self.log.parent() {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    // -- acceptance: process topology --------------------------------------

    #[test]
    fn the_core_forks_the_shim_which_forks_the_app() {
        // Acceptance criterion: `pstree` shows core -> shim -> app. Asserted
        // through procfs rather than by shelling out: the parent/child edges
        // are exactly what pstree renders.
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-topology");
        let (spawned, _server) = h.spawn_mock(&["--serve", "--spawn-app"], &[], &[]);

        let shim_pid = spawned.pid();
        assert_eq!(spawned.realm_id().as_str(), "realm-0");
        // Edge 1: the core (this process) is the shim's parent.
        assert_eq!(
            ppid_of(shim_pid),
            Some(std::process::id()),
            "the shim must be a direct child of the core"
        );

        // Edge 2: the app is a direct child of the shim. The shim only
        // launches it *after* the session is up (the harness's bring-up),
        // so reaching this point is also the end-to-end proof that the
        // inherited socketpair is a working core connection.
        let app_pid = wait_for("the shim to spawn its app", || {
            children_of(shim_pid).into_iter().next()
        });
        assert_ne!(app_pid, shim_pid);
        assert_eq!(
            ppid_of(app_pid),
            Some(shim_pid),
            "topology must be core -> shim -> app"
        );

        h.reap(spawned);

        // The launch is journaled, with the names of what crossed the fork
        // and never their values.
        let entries = h.entries();
        let spawn_entry = entries
            .iter()
            .find(|e| e.str("kind") == "realm_spawned")
            .expect("a spawn must leave a realm_spawned entry");
        assert_eq!(spawn_entry.str("realm"), "realm-0");
        assert_eq!(spawn_entry.u64("pid"), u64::from(shim_pid));
        assert_eq!(
            spawn_entry.str("command"),
            mock_shim_bin().to_str().unwrap()
        );
        assert!(spawn_entry.strings("env_allow").is_empty());
        assert!(spawn_entry.bool("env_cleared"));
    }

    // -- acceptance: the app's environment names only its shim's socket ----

    #[test]
    fn the_childs_environment_names_only_its_own_socket() {
        // Acceptance criterion, asserted on the child's ACTUAL environment
        // (`/proc/<pid>/environ`), never on the parent's intent.
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-env");
        let (spawned, _server) = h.spawn_mock(
            &["--serve"],
            &["HOME", "LANG", "NEVER_SET"],
            &[
                ("HOME", "/home/agent"),
                ("LANG", "en_US.UTF-8"),
                // Present in the core's environment and deliberately never
                // allow-listed: the allowlist decides, not the lookup.
                ("SSH_AUTH_SOCK", "/run/user/1000/ssh-agent"),
            ],
        );
        let env = child_env_of(spawned.pid());

        // The host display server is unreachable by name, in every spelling.
        for absent in [
            "DISPLAY",
            "WAYLAND_SOCKET",
            "XAUTHORITY",
            "SSH_AUTH_SOCK",
            "DBUS_SESSION_BUS_ADDRESS",
            "NEVER_SET",
        ] {
            assert!(
                !env.contains_key(absent),
                "{absent} must not reach the confined child: {env:?}"
            );
        }

        // WAYLAND_DISPLAY names this realm's private socket, absolutely, and
        // XDG_RUNTIME_DIR its private directory -- never the host session's.
        let expected_socket = h.base.join("vitrin-0/realm-0/wayland-0");
        assert_eq!(
            env.get("WAYLAND_DISPLAY").map(String::as_str),
            Some(expected_socket.to_str().unwrap())
        );
        assert_eq!(
            env.get("XDG_RUNTIME_DIR").map(String::as_str),
            Some(spawned.runtime_dir().to_str().unwrap())
        );

        // Exactly the allow-listed names that the core's environment defines,
        // plus exactly the two the core injects. Nothing else at all.
        let names: BTreeSet<&str> = env.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            BTreeSet::from(["HOME", "LANG", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"]),
            "the child's environment is default-deny: allowlist + injections, nothing more"
        );
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/agent"));

        h.reap(spawned);

        // The journal names what was allowed to cross -- names only, never
        // values, because an allow-listed variable's value is whatever the
        // operator's session holds.
        let entries = h.entries();
        let spawn_entry = entries
            .iter()
            .find(|e| e.str("kind") == "realm_spawned")
            .expect("realm_spawned");
        assert_eq!(
            spawn_entry.strings("env_allow"),
            ["HOME", "LANG", "NEVER_SET"]
        );
        let line = format!("{spawn_entry:?}");
        assert!(
            !line.contains("/home/agent") && !line.contains("en_US"),
            "the log must carry env NAMES, never values: {line}"
        );
    }

    #[test]
    fn an_empty_allowlist_yields_only_the_core_injections() {
        // `env_allow = []` is a real working configuration, not a degenerate
        // one (crate::realm's default-deny decision) -- proven on the child.
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-empty-allowlist");
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[("HOME", "/home/agent")]);
        let env = child_env_of(spawned.pid());
        let names: BTreeSet<&str> = env.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            BTreeSet::from(["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"]),
            "an empty allowlist inherits NOTHING"
        );
        h.reap(spawned);
    }

    #[test]
    fn the_ambient_lookup_path_reads_the_cores_own_environment() {
        // `spawn_realm` is `spawn_realm_with_env` over `std::env::var` --
        // the production call. Asserted on a variable cargo guarantees is
        // set for a test process, so the wiring is real rather than
        // described. (`PATH` is an ordinary allow-listable name: it is not
        // reserved, because it names no display server.)
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-ambient-env");
        let bin = mock_shim_bin();
        let realm = realm_with_spawn(
            "realm-0",
            &bin,
            &["--serve".to_string()],
            &["CARGO_PKG_NAME".to_string()],
        );
        let paths = h.paths();
        let spawned = spawn_realm(&realm, &paths, &mut h.recorder).expect("spawn");
        wait_for_exec(spawned.pid(), &bin);

        let env = child_env_of(spawned.pid());
        // Whatever the core's own environment holds for that name is what
        // the child must hold -- including nothing at all, which is the
        // skip-an-unset-name rule (`crate::realm`: config validity must not
        // depend on which machine loads it).
        assert_eq!(
            env.get("CARGO_PKG_NAME").cloned(),
            std::env::var("CARGO_PKG_NAME").ok(),
            "the ambient path resolves names from the core's real environment"
        );
        assert!(
            std::env::var_os("PATH").is_some() && !env.contains_key("PATH"),
            "and only allow-listed ones: PATH is set for the core and must not cross"
        );
        h.reap(spawned);
    }

    #[test]
    fn a_spawned_realm_becomes_running_without_changing_what_the_wire_answers() {
        // The intended wiring, executable so it cannot silently rot: the
        // spawn produces the pid, and `RealmRegistry` -- the single owner of
        // realm state -- records it. Petition-time addressing is deliberately
        // unchanged by the launch (`Realm::admits_petitions`): a realm is
        // addressable before its app starts and after, and liveness stays
        // the chokepoint's `no_surface` judgement.
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-registry-state");
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);

        let mut registry = crate::realm::tests::registry_with(&["realm-0"]);
        let before = registry.resolve_for_petition("realm-0").cloned();
        assert!(registry.mark_running(spawned.realm_id(), spawned.pid()));
        assert_eq!(
            registry.get("realm-0").map(|r| r.state()),
            Some(crate::realm::RealmState::Running { pid: spawned.pid() })
        );
        assert_eq!(
            registry.resolve_for_petition("realm-0").cloned(),
            before,
            "spawning changes what the realm is doing, never what it answers"
        );

        h.reap(spawned);
    }

    #[test]
    fn the_session_runtime_tree_comes_from_xdg_runtime_dir() {
        // The production constructor agrees with the transport's own
        // convention paths; nothing here mutates process-global state.
        if let (Ok(paths), Ok(dir)) = (SpawnPaths::from_env(), paths::runtime_dir()) {
            assert_eq!(paths.realm_dir("realm-0").unwrap(), dir.join("realm-0"));
        }
    }

    // -- acceptance: nothing else crosses the fork -------------------------

    #[test]
    fn no_unrelated_core_descriptor_reaches_the_child() {
        // The strongest confinement assertion available: the child's whole
        // descriptor table, from procfs. Decoys of every class the core
        // really holds are opened first, so the test would fail if CLOEXEC
        // discipline (P1.2.1) regressed anywhere.
        let _fd = fd_lock();
        let decoy_listener = Connection::pair().expect("decoy socketpair");
        let decoy_memfd =
            rustix::fs::memfd_create("vitrin-spawn-decoy", rustix::fs::MemfdFlags::CLOEXEC)
                .expect("decoy memfd");
        let decoy_file = fs::File::open("/proc/self/status").expect("decoy file");

        let mut h = Harness::new("spawn-fd-table");
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);
        let fds = child_fds_of(spawned.pid());

        assert_eq!(
            fds.keys().copied().collect::<Vec<_>>(),
            vec![0, 1, 2, SHIM_CORE_FD],
            "the child holds exactly stdio and the core socketpair: {fds:?}"
        );
        assert!(
            fds[&SHIM_CORE_FD].starts_with("socket:"),
            "fd {SHIM_CORE_FD} must be the inherited socketpair, not {:?}",
            fds[&SHIM_CORE_FD]
        );
        assert_eq!(
            fds[&0], "/dev/null",
            "stdin is /dev/null: the operator's keystrokes are not ambient authority"
        );

        drop(decoy_listener);
        drop(decoy_memfd);
        drop(decoy_file);
        h.reap(spawned);
    }

    #[test]
    fn the_app_does_not_inherit_the_core_connection() {
        // The shim's obligation (module docs): set FD_CLOEXEC on fd 3 at
        // startup so nothing it spawns can speak to the core. Asserted on
        // the grandchild's real descriptor table -- if the mock shim (or,
        // later, the C shim) forgot, the app would hold a live core
        // connection and this fails.
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-app-fds");
        let (spawned, _server) = h.spawn_mock(&["--serve", "--spawn-app"], &[], &[]);
        let app_pid = wait_for("the shim to spawn its app", || {
            children_of(spawned.pid()).into_iter().next()
        });
        let fds = wait_for("the app's descriptor table", || {
            let fds = child_fds_of(app_pid);
            (!fds.is_empty()).then_some(fds)
        });
        assert!(
            !fds.contains_key(&SHIM_CORE_FD),
            "the app must not inherit the shim's core connection: {fds:?}"
        );
        h.reap(spawned);
    }

    // -- acceptance: killing the shim leaves the core alive ----------------

    #[test]
    fn killing_the_shim_does_not_take_the_core_with_it() {
        // Acceptance criterion, in the half that is true at THIS commit:
        // the core survives, and the loss surfaces as an ordinary
        // end-of-stream on the connection rather than a signal or a panic.
        // (Surface removal is #32's; nothing here claims it.)
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-kill-shim");
        let (mut spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);

        spawned.child_mut().kill().expect("the shim can be killed");
        let status = spawned.child_mut().wait().expect("the shim is reapable");
        assert!(!status.success(), "killed, not exited cleanly");

        // The core is still running -- it is executing this line -- and the
        // dead peer is an ordinary transport condition. SIGPIPE would have
        // killed this process instead of returning; the transport's
        // MSG_NOSIGNAL discipline is what makes that true.
        let conn = spawned.connection_mut();
        loop {
            match conn.recv_message() {
                // create_surface, sent before the kill: drain it.
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => break,
            }
        }
        // Writing to a dead peer is an error value, never a signal: the
        // transport's MSG_NOSIGNAL discipline is what keeps a shim's death
        // from becoming the core's.
        let probe = vitrin_protocol::generated::vitrin_shim_session::events::Configure {
            realm: "realm-0".into(),
            width: 1280,
            height: 800,
        }
        .encode(1);
        let _ = conn.send_message(&probe, None);

        h.reap(spawned);
    }

    // -- fail-closed: every precondition refuses, leaving nothing behind ---

    /// A spawn expected to fail: run it, assert nothing was created, and
    /// hand back the typed error plus the journal's refusal entry.
    fn refused_spawn(label: &str, realm: &Realm) -> (SpawnError, Json) {
        let mut h = Harness::new(label);
        let paths = h.paths();
        let err = spawn_realm_with_env(realm, &paths, &mut h.recorder, |_| Some("hostile".into()))
            .expect_err("this spawn must be refused");
        assert!(
            !h.base.join("vitrin-0/realm-0").exists(),
            "a refused spawn must leave no runtime directory behind"
        );
        let entry = h
            .entries()
            .into_iter()
            .find(|e| e.str("kind") == "realm_spawn_failed")
            .expect("every refused spawn is journaled");
        assert_eq!(entry.str("realm"), "realm-0");
        assert!(entry.is_null("pid"), "a refusal started no process");
        (err, entry)
    }

    #[test]
    fn a_nonexistent_command_is_a_typed_error_leaving_no_state() {
        let _fd = fd_lock();
        let missing = std::env::temp_dir().join("vitrin-no-such-program");
        let realm = realm_with_spawn("realm-0", &missing, &[], &[]);
        let (err, entry) = refused_spawn("spawn-missing", &realm);
        assert!(matches!(err, SpawnError::ProgramAudit { .. }), "{err}");
        assert_eq!(err.cause_class(), "program_audit");
        assert_eq!(entry.str("cause_class"), "program_audit");
    }

    #[test]
    fn a_non_executable_command_is_a_typed_error_at_exec() {
        // Distinct from the audit refusals: the file passes the
        // trusted-writer rule (0644 is not group/other WRITABLE) and fails
        // at `execve` with EACCES. It must still be typed, and must still
        // leave nothing behind -- which is the interesting half, because
        // this is the one failure that happens *after* the runtime
        // directory and the socketpair already exist.
        let _fd = fd_lock();
        let dir = scratch();
        let program = dir.join("not-executable");
        fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o644)).unwrap();

        let realm = realm_with_spawn("realm-0", &program, &[], &[]);
        let (err, entry) = refused_spawn("spawn-noexec", &realm);
        assert!(matches!(err, SpawnError::Exec { .. }), "{err}");
        assert_eq!(err.cause_class(), "exec");
        assert_eq!(entry.str("cause_class"), "exec");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_group_writable_command_is_refused_at_spawn_time() {
        // The re-audit's reason for existing: config load happened earlier,
        // and the mode may have changed since. Nothing is created.
        let _fd = fd_lock();
        let dir = scratch();
        let program = dir.join("app");
        fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o775)).unwrap();

        let realm = realm_with_spawn("realm-0", &program, &[], &[]);
        let (err, _) = refused_spawn("spawn-group-writable", &realm);
        assert!(matches!(err, SpawnError::ProgramAudit { .. }), "{err}");
        assert!(
            err.to_string().contains("writable by group/other"),
            "the refusal must name the fault: {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_runtime_tree_refuses_the_spawn() {
        let _fd = fd_lock();
        let realm = realm_with_spawn("realm-0", &mock_shim_bin(), &[], &[]);
        let mut h = Harness::new("spawn-readonly-tree");
        // A read-only runtime base: the realm's directory cannot be made.
        fs::set_permissions(&h.base, fs::Permissions::from_mode(0o500)).unwrap();
        let paths = h.paths();
        let err = spawn_realm_with_env(&realm, &paths, &mut h.recorder, |_| None)
            .expect_err("an unwritable runtime tree must refuse the spawn");
        assert!(matches!(err, SpawnError::RuntimeDir { .. }), "{err}");
        assert_eq!(err.cause_class(), "runtime_dir");
        fs::set_permissions(&h.base, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn a_reserved_env_name_refuses_the_spawn_even_though_the_loader_refuses_it_first() {
        // Defense in depth (module docs): the filter is the guarantee, this
        // error is the alarm that config validation was bypassed. Built
        // through the test fixture rather than the loader, because the
        // loader cannot produce this state -- which is the point.
        let _fd = fd_lock();
        let bin = mock_shim_bin();
        for (reserved, _) in RESERVED_ENV {
            let realm = realm_with_spawn("realm-0", &bin, &[], &[reserved.to_string()]);
            let (err, _) = refused_spawn(&format!("spawn-reserved-{reserved}"), &realm);
            assert!(
                matches!(err, SpawnError::ReservedEnv { name } if name == reserved),
                "{err}"
            );
            assert_eq!(err.cause_class(), "reserved_env");
        }
    }

    #[test]
    fn the_env_filter_holds_even_if_the_reserved_check_is_bypassed() {
        // The structural half, asserted directly on the composed
        // environment: `child_env` cannot emit a reserved name, whatever the
        // allowlist says and whatever the lookup resolves.
        let spawn_config = crate::realm::tests::spawn_config_with(
            Path::new("/usr/bin/true"),
            &[],
            &RESERVED_ENV.map(|(name, _)| name.to_string()),
        );
        let env = child_env(
            &spawn_config,
            Path::new("/run/user/1000/vitrin-0/realm-0/wayland-0"),
            Path::new("/run/user/1000/vitrin-0/realm-0"),
            |_| Some("host-value".into()),
        );
        let names: BTreeSet<&str> = env
            .iter()
            .map(|(k, _)| k.to_str().expect("ASCII names"))
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"])
        );
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == "WAYLAND_DISPLAY")
                .map(|(_, v)| v.to_str().unwrap()),
            Some("/run/user/1000/vitrin-0/realm-0/wayland-0"),
            "the injected value wins, never the inherited one"
        );
    }

    // -- runtime directory -------------------------------------------------

    #[test]
    fn a_stale_runtime_directory_is_purged_not_reused() {
        // A directory at this path belongs to a run that is gone: reusing it
        // would carry a dead run's socket into a new run's confinement.
        let _fd = fd_lock();
        let base = scratch();
        let realm_dir = base.join("vitrin-0/realm-0");
        fs::create_dir_all(&realm_dir).unwrap();
        let stale = realm_dir.join("wayland-0");
        fs::write(&stale, b"stale socket from a crashed run").unwrap();
        // Deliberately wrong mode, as a crashed run may well have left.
        fs::set_permissions(&realm_dir, fs::Permissions::from_mode(0o755)).unwrap();

        let guard = RuntimeDirGuard::create(&realm_dir).expect("a stale directory is purged");
        assert!(!stale.exists(), "stale contents must not survive");
        let mode = fs::metadata(&realm_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, RUNTIME_DIR_MODE, "the mode is stated, not inherited");
        guard.keep();
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_symlink_where_the_runtime_directory_belongs_is_refused_never_followed() {
        // The reason the purge opens O_NOFOLLOW|O_DIRECTORY: a recursive
        // delete through a planted symlink is how a cleanup routine deletes
        // someone's home directory.
        let _fd = fd_lock();
        let base = scratch();
        let victim = base.join("precious");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keepme"), b"not yours to delete").unwrap();
        fs::create_dir_all(base.join("vitrin-0")).unwrap();
        std::os::unix::fs::symlink(&victim, base.join("vitrin-0/realm-0")).unwrap();

        let err = RuntimeDirGuard::create(&base.join("vitrin-0/realm-0"))
            .expect_err("a symlink must be refused");
        assert!(matches!(err, SpawnError::RuntimeDir { .. }), "{err}");
        assert!(
            victim.join("keepme").exists(),
            "the symlink target must be untouched"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_guard_removes_the_directory_unless_it_is_kept() {
        let _fd = fd_lock();
        let base = scratch();
        let realm_dir = base.join("vitrin-0/realm-0");

        let guard = RuntimeDirGuard::create(&realm_dir).unwrap();
        assert!(realm_dir.is_dir());
        drop(guard);
        assert!(
            !realm_dir.exists(),
            "an unkept guard unwinds the directory it created"
        );

        let guard = RuntimeDirGuard::create(&realm_dir).unwrap();
        guard.keep();
        assert!(realm_dir.is_dir(), "a kept directory survives");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_hostile_realm_id_never_becomes_a_path() {
        // The id is validated by the transport's own rule, so no spawn path
        // can turn a realm id into a traversal.
        let paths = SpawnPaths::under("/run/user/1000");
        assert!(paths.realm_dir("../../etc").is_err());
        assert_eq!(
            paths.realm_dir("realm-0").unwrap(),
            Path::new("/run/user/1000/vitrin-0/realm-0")
        );
        assert_eq!(
            paths.shim_socket("realm-0").unwrap(),
            Path::new("/run/user/1000/vitrin-0/realm-0/wayland-0")
        );
    }
}
