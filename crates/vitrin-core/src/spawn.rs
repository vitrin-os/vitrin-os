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
//! - **The registered closure is syscalls and no Rust.** It captures three
//!   integers by value (a `RawFd` and two signal numbers) and calls only
//!   `close_range`, `signal`, and `dup3`/`fcntl` -- every one of them on
//!   signal-safety(7)'s async-signal-safe list. Its errors come from
//!   `io::Error::last_os_error()`, which is `from_raw_os_error` over
//!   `errno` and allocates nothing. There is no `String`, no `Vec`, no
//!   `PathBuf`, no formatting, no logging, and no `Drop` impl in scope that
//!   could take a lock. Everything it needs that *would* allocate or read
//!   global state -- the descriptor number, `SIGRTMAX()` -- is computed in
//!   the parent and captured.
//! - **Order is exploited, not assumed.** std runs its stdio `dup2`s
//!   *before* the closures, so the closure's `dup3` onto fd 3 cannot race
//!   or be undone by stdio setup. The parent independently guarantees the
//!   source descriptor is `>= 3` (see [`spawn_realm`]), so stdio placement
//!   can never clobber it either. Inside the closure the order is equally
//!   load-bearing and equally deliberate: the descriptor sweep runs
//!   *before* the `dup3`, because a sweep afterwards would mark the shim's
//!   own connection close-on-exec and hand the child an empty fd 3.
//!
//! That is the entire `unsafe` surface of this module: one closure, four
//! possible syscalls, no memory operations.
//!
//! # What the child does *not* inherit, made structural
//!
//! Two things cross `execve` by default that a confined child must not get,
//! and neither is prevented by the discipline the rest of the core keeps.
//! Both are closed inside the closure, because that is the only place they
//! *can* be closed:
//!
//! - **Descriptors.** `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` marks every
//!   descriptor above stdio close-on-exec. Until this existed, "no unrelated
//!   descriptor of the core's crosses the fork" rested entirely on every
//!   other module remembering `O_CLOEXEC` -- true today (P1.2.1's discipline
//!   holds, and the tests assert it), but a property of code this file does
//!   not own, and one the DRM/udev/libinput/EGL backends (E4/E7) will put
//!   third-party fd creation inside. One syscall converts a convention into
//!   a guarantee. `CLOSE_RANGE_CLOEXEC` rather than an outright
//!   `close_range(..., 0)`: std reports `execve` failure to the parent
//!   through a pipe it opened before the fork, and *closing* that pipe would
//!   turn "no such program" into a spawn that reports success and dies --
//!   marking it close-on-exec keeps the report working right up to a
//!   successful `execve`. The kernel does the closing, atomically, at the
//!   only instant it is correct. On a kernel too old to have the flag
//!   (before 5.11) the call fails, the closure returns that error, and the
//!   spawn is refused with [`SpawnError::Exec`] carrying the errno: fail
//!   closed, because a child confined only on the assumption that every
//!   other module remembered `O_CLOEXEC` is not confined, it is trusted.
//! - **Ignored signals.** `execve` resets *caught* signals to `SIG_DFL` but
//!   deliberately preserves `SIG_IGN`, and std resets only `SIGPIPE`. So
//!   every disposition the core inherited from its own launch context
//!   survives into the shim and, transitively, into the app: a POSIX shell
//!   sets `SIGINT`/`SIGQUIT` to `SIG_IGN` for a background job, which is the
//!   ordinary `vitrind &` development and demo path, and a service manager
//!   may add others. Measured on this toolchain, an ignored `SIGINT`,
//!   `SIGQUIT` **and `SIGTERM` all reach the app.** That last one is not
//!   hygiene: a display server whose whole promise is revocable authority
//!   cannot spawn a child that is immune to `SIGTERM` because of how the
//!   operator happened to start the core, and P1.5.3 (#32) builds
//!   termination on exactly that signal. The closure resets every
//!   disposition to `SIG_DFL`, so a realm's process tree starts from a
//!   defined state instead of an inherited one.
//! - **The blocked signal *mask*** -- the same hazard through the other
//!   mechanism, and the one P1.5.3 (#32) found by building on the promise
//!   above. A disposition reset is not a mask reset: a *blocked* signal is
//!   never delivered to any disposition at all, so `SIG_DFL` on a blocked
//!   `SIGTERM` is still an unkillable realm. The blocked set crosses
//!   `fork` and `execve` untouched and `std::process::Command` does not
//!   clear it (measured on this toolchain: a child spawned with
//!   `SIGTERM|SIGCHLD` blocked reports exactly that `SigBlk`). This is not
//!   hypothetical for *this* core: **both backends block `SIGINT` and
//!   `SIGTERM`** the moment they install calloop's `signalfd` source
//!   (`backend::headless`, `backend::winit`), and P1.5.3 blocks `SIGCHLD`
//!   on top of them so its reaper can see child deaths -- so every realm
//!   a real `vitrind` spawns would inherit a blocked `SIGTERM`, its
//!   termination ladder's polite rung would be a silent no-op, and every
//!   realm would die by `SIGKILL`. A confined app would also be unable to
//!   reap *its own* children by `SIGCHLD`. The closure clears the mask
//!   outright; `crate::lifecycle` owns the termination ladder that depends
//!   on it, and `spawn::tests::the_child_starts_with_an_empty_signal_mask`
//!   is the assertion that keeps it true.
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
//! - **The enclosing `vitrin-0` tree is verified, not just created.** It is
//!   `mkdir`ed, then opened `O_NOFOLLOW | O_DIRECTORY`, confirmed to be a
//!   directory owned by this euid, and chmodded *through that descriptor*.
//!   The path-based `create_dir_all` + `set_permissions` this replaced both
//!   follow symlinks: a symlink planted at `vitrin-0` was silently accepted,
//!   its target chmodded to `0700`, and every realm directory created inside
//!   a tree the core did not choose -- so `WAYLAND_DISPLAY` would name a
//!   socket somewhere else entirely. The realm directory one level down was
//!   already careful about exactly this; the parent now matches it.
//! - **A pre-existing directory is stale garbage, but only a *proven* stale
//!   one is purged.** The purge is a recursive delete, so what licenses it
//!   has to be a fact, not an assumption. The fact is an `flock` on
//!   `<realm_id>.lock`, taken non-blocking before the purge and held for the
//!   realm's whole life ([`RealmLock`]): winning it proves no live core owns
//!   this realm, so anything at the path belongs to a run that is gone.
//!   Losing it means a second `vitrind` is serving this realm right now, and
//!   the spawn is refused ([`SpawnError::RealmBusy`]) instead of deleting a
//!   live run's socket directory out from under it -- which is precisely
//!   what the previous "the listener's `flock` guard enforces one core per
//!   tree" justification permitted, because **nothing in the core
//!   constructs a `Listener`**, so that lock was never held by anybody.
//!   Reusing a stale directory instead of purging is not an option either:
//!   it would carry a dead run's socket file and scratch into a new run's
//!   confinement.
//! - **The purge is bound to what it verified as tightly as std allows.**
//!   The realm directory is reached with `openat` *through the verified
//!   parent descriptor*, `O_NOFOLLOW | O_DIRECTORY`, and confirmed to be a
//!   directory owned by this euid -- a blind recursive delete through a
//!   planted symlink is how a cleanup routine deletes a home directory. The
//!   honest residual: std has no `remove_dir_all` rooted at a descriptor, so
//!   the delete itself re-resolves the final component by path. What closes
//!   that gap is not the check but the chain around it -- the parent is held
//!   open and proven `0700` and ours, so no *other* uid can substitute the
//!   name, and the realm lock excludes the only same-uid process that has
//!   business here. A same-uid process outside that chain still could, and
//!   that is D9's territory, not a mode bit's.
//! - **Removal at exit belonged to P1.5.3 (#32)**, which owns lifecycle,
//!   and now exists: [`remove_runtime_dir`] is the seam
//!   [`RuntimeDirGuard::keep`] left open, and [`crate::lifecycle`] calls it
//!   on an *orderly* shutdown only (a crash deliberately keeps the tree as
//!   evidence -- that module's docs argue it). It reuses this module's
//!   verified-parent + `O_NOFOLLOW` purge rather than a second delete path,
//!   so "the core recursively deletes a directory" stays one routine with
//!   one set of proofs. What this task already guaranteed is unchanged: a
//!   crashed run is self-healing at the next start (the purge above), and a
//!   *failed* spawn leaves nothing behind ([`RuntimeDirGuard`]) -- fail-closed
//!   means no half-prepared realm.
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
//! bypassed validator hides the bug that matters. The `command` path gets
//! the same treatment for the same reason: [`launch`] refuses a relative one
//! even though [`crate::realm`] already does, because a relative program
//! resolves against the child's `current_dir` -- which this module points at
//! the realm's own runtime directory, a directory the confined app can
//! write. Auditing one inode and `exec`ing whatever the app dropped at that
//! name is the one divergence the audit must not have.
//!
//! What the injected `XDG_RUNTIME_DIR` is *not*: a jail. Its value is
//! `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>`, a subdirectory of the tree that
//! also holds the core's own principal-facing `core.sock` and this run's
//! flight-recorder log -- so the value names the core's control plane one
//! level up (`../core.sock`). That is worth stating plainly, but it is not
//! worth relocating: under D9 the child runs as the core's uid and can
//! derive `/run/user/<uid>` from `getuid()` whether or not any variable
//! points at it, so moving the realm tree elsewhere would change what the
//! app is *told*, not what it can reach. The variable is redirected so a
//! well-behaved client finds its own realm's socket instead of the host
//! session's, which is a confinement of the well-behaved -- the same
//! qualifier the whole D9 section below carries.
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
//! # Not in this module
//!
//! Crash detection, `SIGCHLD` reaping, exit propagation, and shutdown
//! ordering are all [`crate::lifecycle`] (P1.5.3, #32). This module keeps
//! the [`std::process::Child`] handle alive inside [`SpawnedRealm`] and
//! never waits on it, so lifecycle adopts an unreaped, unlost process
//! handle ([`SpawnedRealm::into_parts`]) rather than re-deriving one from a
//! pid -- which is exactly the pid-reuse race a `Child` exists to avoid.
//! Nothing here decides when a realm dies or what that means.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
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

    /// `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>.lock` -- the realm-ownership
    /// lock. Deliberately a *sibling* of the realm directory rather than a
    /// file inside it: the purge this lock authorizes is a recursive delete
    /// of that directory, and a lock file the purge could delete would be a
    /// lock that stops meaning anything at the moment it matters most.
    fn realm_lock(&self, realm_id: &str) -> Result<PathBuf, paths::PathError> {
        paths::realm_lock_path_in(&self.xdg_runtime_dir, realm_id)
    }
}

/// An unreaped [`Child`] that terminates and waits on itself if it is
/// dropped instead of being handed on.
///
/// `std::process::Child` has no `Drop`: dropping one abandons the process,
/// which keeps running and, once it exits, becomes a zombie for the core's
/// entire session because nothing will ever `waitpid` it. That is the
/// correct default for a general-purpose handle and exactly the wrong one
/// here, because [`spawn_realm`] documents every one of its errors as a
/// *refusal* -- "never a partially-launched realm" -- and the window
/// between it returning and [`crate::lifecycle::RealmLifecycle::adopt`]
/// taking ownership is not error-free. Realm bring-up
/// ([`SpawnedRealm::start_shim_session`]) sends `configure` over the
/// inherited socketpair and fails with `EPIPE` against a shim that exec'd
/// successfully and then died at once -- a wrapper that returns nonzero, a
/// binary missing a shared library, a shim that rejects its environment --
/// and a `?` there drops the [`SpawnedRealm`] mid-unwind.
///
/// So the guarantee is made structural rather than left to every caller
/// remembering it: the process is killed and reaped by the type system, on
/// every path out including a panic, until the moment ownership genuinely
/// moves to [`crate::lifecycle`] (which has a `Drop` of its own from
/// `adopt` onwards, so the two are contiguous with no gap between them).
#[derive(Debug)]
struct GuardedChild(Option<Child>);

impl GuardedChild {
    fn get(&self) -> &Child {
        // Unreachable: `release` consumes `self`, so the `None` state is
        // observable only from inside `Drop`.
        self.0
            .as_ref()
            .expect("the child is present until released")
    }

    fn get_mut(&mut self) -> &mut Child {
        self.0
            .as_mut()
            .expect("the child is present until released")
    }

    /// Hand the process on to an owner that will reap it, disarming the
    /// guard. Consumes `self`, so it cannot be released twice.
    fn release(mut self) -> Child {
        self.0.take().expect("the child is present until released")
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        tracing::warn!(
            pid = child.id(),
            "a spawned shim was dropped before the realm came up; SIGKILLing and reaping it \
             so the refusal leaves no runaway process and no zombie"
        );
        // `SIGKILL` and a blocking wait, matching `RealmLifecycle`'s own
        // last-resort drop: there is nowhere left to report a polite
        // failure to, and this realm never became live enough to owe its
        // app an orderly teardown.
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A realm whose app has been launched: the core's half of the identity
/// pair (the socketpair end the shim does *not* hold), the process handle,
/// and the private directory the realm was given.
///
/// The [`Child`] is retained rather than waited on: reaping, crash
/// detection, and exit propagation are P1.5.3 (#32), and losing the handle
/// here would force that task to re-derive one from a pid -- which is
/// exactly the pid-reuse race a `Child` exists to avoid. The single
/// exception is a [`SpawnedRealm`] that is *dropped* before
/// [`Self::into_parts`] hands it on, which kills and reaps
/// ([`GuardedChild`]): a realm that never came up is a refusal, and a
/// refusal must not leave a process behind.
#[derive(Debug)]
pub(crate) struct SpawnedRealm {
    realm_id: RealmId,
    /// The shim process, reaped on drop if the realm is never adopted
    /// ([`GuardedChild`]).
    child: GuardedChild,
    runtime_dir: PathBuf,
    connection: Connection,
    /// This realm's exclusive `flock`, held for as long as the realm is
    /// live so no second core can decide this realm's runtime directory is
    /// stale and delete it (module docs). Never read -- its existence *is*
    /// the effect, and the kernel releases it even if the core is killed
    /// without unwinding.
    _realm_lock: fs::File,
}

impl SpawnedRealm {
    /// The realm this process serves -- the same id the wire addresses and
    /// grant rows key on.
    pub fn realm_id(&self) -> &RealmId {
        &self.realm_id
    }

    /// The shim's process id.
    pub fn pid(&self) -> u32 {
        self.child.get().id()
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

    /// The process handle, unreaped. Exposed so [`crate::lifecycle`] --
    /// and, today, this module's tests -- can terminate and wait
    /// deterministically. This module implements no lifecycle policy of its
    /// own.
    pub fn child_mut(&mut self) -> &mut Child {
        self.child.get_mut()
    }

    /// Hand the whole live realm to [`crate::lifecycle`], which owns it
    /// from birth to death.
    ///
    /// Deliberately a move rather than a set of borrows: every resource
    /// here has to die together and in an order (connection, then process,
    /// then directory, and the realm lock last of all -- releasing it
    /// earlier would let a second core purge a tree this one is still
    /// tearing down). Split ownership is how one of them gets forgotten.
    pub fn into_parts(self) -> SpawnedParts {
        SpawnedParts {
            realm_id: self.realm_id,
            // The one place the reap guard is disarmed: `lifecycle` owns
            // the process from here, and its own `Drop` takes over with no
            // window in between.
            child: self.child.release(),
            runtime_dir: self.runtime_dir,
            connection: self.connection,
            realm_lock: self._realm_lock,
        }
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

/// A [`SpawnedRealm`] taken apart for [`crate::lifecycle`] to adopt: every
/// resource one live realm owns, moved out together
/// ([`SpawnedRealm::into_parts`]).
#[derive(Debug)]
pub(crate) struct SpawnedParts {
    pub realm_id: RealmId,
    pub child: Child,
    pub runtime_dir: PathBuf,
    pub connection: Connection,
    /// The realm's exclusive `flock`, held until the realm is fully torn
    /// down. Never read; its existence is the effect (see [`SpawnedRealm`]).
    pub realm_lock: fs::File,
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
    /// Another live process holds this realm's lock: a second core is
    /// already serving it. Refused rather than purging its runtime
    /// directory out from under it (module docs).
    RealmBusy { realm: String, lock: PathBuf },
    /// The program failed the trusted-writer audit *at spawn time* -- the
    /// re-check on the descriptor, not a re-reading of the config.
    ProgramAudit { path: PathBuf, detail: String },
    /// The program is not an absolute path. Like [`Self::ReservedEnv`] this
    /// is an alarm rather than a filter: [`crate::realm`] already refuses
    /// it at load, so reaching here means the validator was bypassed
    /// (module docs).
    RelativeCommand { path: PathBuf },
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
            SpawnError::RealmBusy { .. } => "realm_busy",
            SpawnError::ProgramAudit { .. } => "program_audit",
            SpawnError::RelativeCommand { .. } => "relative_command",
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
            SpawnError::RealmBusy { realm, lock } => write!(
                f,
                "realm {realm} is already served by another live vitrind (its lock {} is \
                 held); refusing to spawn a second one, because preparing this realm's \
                 runtime directory would delete the running one's socket",
                lock.display()
            ),
            SpawnError::ProgramAudit { path, detail } => write!(
                f,
                "refusing to exec {}: {detail} (re-checked at spawn time, not at config load)",
                path.display()
            ),
            SpawnError::RelativeCommand { path } => write!(
                f,
                "`command` {path:?} is not an absolute path; it would resolve against the \
                 child's working directory -- the realm's own runtime directory, which the \
                 confined app can write -- so the core would audit one program and exec \
                 another. Realm config validation refuses this already; it reached the \
                 spawn path, which means validation was bypassed"
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
    let lock_path = paths.realm_lock(realm_id)?;
    let spawn = realm.spawn();

    // Preconditions first, in the order that leaves the least behind: pure
    // checks before anything is created, so the common refusals never reach
    // the guard below at all.
    reject_reserved_env(spawn)?;
    audit_program_at_spawn(spawn.command())?;

    // From here on the call owns filesystem state; the guard unwinds it on
    // every error path (fail closed).
    let guard = RuntimeDirGuard::create(&runtime_dir, &lock_path, realm_id)?;

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

    // Read in the parent, where reading a libc global is free of consequence,
    // and captured as a plain integer: `SIGRTMAX()` is the last signal number
    // this platform's C library admits to having.
    let sig_max = libc::SIGRTMAX();

    // SAFETY: this closure runs in the forked child, between `fork` and
    // `execve`, where only async-signal-safe operations are permitted (the
    // core is multi-threaded; any lock held by another thread at fork time
    // -- the allocator's included -- is held forever in the child).
    //
    // It satisfies that requirement by construction:
    //   * it captures `shim_raw` (a `RawFd`) and `sig_max` (a `c_int`) by
    //     value -- integers, no heap data, no `Drop` type in scope;
    //   * every syscall it makes is on signal-safety(7)'s async-signal-safe
    //     list: `close_range`, `signal`, `sigemptyset`, `sigprocmask`, and
    //     `dup3` (or `fcntl` when the source already sits on the target
    //     descriptor, where `dup3` would return EINVAL);
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
            // (1) Descriptor sweep, FIRST: everything above stdio becomes
            // close-on-exec, `shim_raw` included. Doing this after the
            // `dup3` below would mark the shim's own connection and hand
            // the child an empty fd 3. Marking rather than closing keeps
            // std's exec-failure pipe alive until `execve` succeeds
            // (module docs); the kernel closes the marked set atomically.
            if libc::close_range(
                SHIM_CORE_FD as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }

            // (2) Signal dispositions to a defined state. `execve` preserves
            // SIG_IGN, so without this the app inherits whatever the
            // operator's launch context ignored -- measurably including
            // SIGTERM, which would make the realm unkillable by the very
            // signal P1.5.3 terminates it with. Errors are ignored on
            // purpose: SIGKILL/SIGSTOP cannot be reset and the real-time
            // range is sparse, so the only reportable outcome would be
            // "this signal never needed resetting".
            let mut sig = 1;
            while sig <= sig_max {
                if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                    libc::signal(sig, libc::SIG_DFL);
                }
                sig += 1;
            }

            // (3) The signal MASK to a defined state, for the same reason
            // and against a different mechanism. `execve` resets nothing
            // here: the blocked set crosses `fork` *and* `execve`
            // untouched, and `std::process::Command` does not clear it
            // (measured -- see the module docs' companion note). The core
            // blocks SIGINT/SIGTERM the moment either backend installs
            // calloop's signalfd source, and P1.5.3 blocks SIGCHLD on top,
            // so without this line every realm this core spawns inherits a
            // *blocked* SIGTERM and the termination ladder's polite rung
            // silently degrades to SIGKILL every time. Step (2) above
            // cannot help: SIG_DFL is a disposition, and a blocked signal
            // is never delivered to any disposition at all.
            //
            // `sigprocmask`, not `pthread_sigmask`: only the former is on
            // signal-safety(7)'s async-signal-safe list, and POSIX's
            // "unspecified in a multi-threaded process" caveat does not
            // apply -- a forked child has exactly one thread.
            let mut empty = core::mem::MaybeUninit::<libc::sigset_t>::uninit();
            if libc::sigemptyset(empty.as_mut_ptr()) < 0
                || libc::sigprocmask(libc::SIG_SETMASK, empty.as_ptr(), core::ptr::null_mut()) < 0
            {
                return Err(io::Error::last_os_error());
            }

            // (4) The one descriptor that must survive, placed after the
            // sweep and therefore without FD_CLOEXEC.
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

    Ok(SpawnedRealm {
        realm_id: realm.id().clone(),
        child: GuardedChild(Some(child)),
        runtime_dir,
        connection: core_side,
        // The realm is now live, and the lock that proved it was free
        // becomes the lock that says it is taken -- held until this struct
        // drops (or the process dies, which the kernel handles).
        _realm_lock: guard.keep(),
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

/// Re-audit the program **at spawn time** against the same trusted-writer
/// rule `realm.toml` loading applies ([`untrusted_writer`] -- one
/// definition, two call sites) -- the program *and every directory on its
/// canonical path*, because a writable directory anywhere on the way is a
/// swap of the program by another name.
///
/// This is deliberately not a re-run of the config-load audit: that one
/// proved the operator's configuration was coherent, possibly minutes ago
/// and certainly before this process decided to exec anything. What it could
/// never prove is anything about the instant of `execve`, and
/// `crate::realm`'s docs hand that question here. Which is exactly why the
/// ancestor walk has to be here too: the window this check exists to cover
/// is a window in which a directory's mode can change as easily as a file's,
/// and a re-check that skipped half the policy would be most confident
/// precisely where it was least entitled to be.
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
    // Before anything else: a relative program would be resolved by
    // `execvp` against the child's working directory, which `launch` sets
    // to the realm's own runtime directory -- writable by the confined app.
    // The audit below would then describe a different file than the one
    // that runs. `crate::realm` refuses this at load; reaching here means
    // validation was bypassed (module docs).
    if !command.is_absolute() {
        return Err(SpawnError::RelativeCommand {
            path: command.to_path_buf(),
        });
    }

    let refuse = |detail: String| SpawnError::ProgramAudit {
        path: command.to_path_buf(),
        detail,
    };
    // Resolved first, for the same reason `crate::realm` resolves: a lexical
    // walk of `/opt/app` would never look at the directory a symlinked
    // `/opt` actually points into. The realm still execs the path the
    // operator wrote -- `argv[0]` is observable to the program.
    let resolved = fs::canonicalize(command)
        .map_err(|e| refuse(format!("does not resolve to a program ({e})")))?;

    // O_PATH: the audit needs the inode's metadata, not its contents, and
    // an O_PATH descriptor needs no read permission on the program.
    let fd = rustix::fs::open(&resolved, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
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

    // Skip(1): `ancestors` yields the program itself first, then each
    // enclosing directory up to `/`. `sticky_tolerated` matches the load
    // audit -- a 1777 directory only lets a writer touch entries it owns.
    for dir in resolved.ancestors().skip(1) {
        let st = rustix::fs::stat(dir)
            .map_err(|e| refuse(format!("cannot stat directory {} ({e})", dir.display())))?;
        if let Some(fault) = untrusted_writer(st.st_uid, st.st_mode, euid, true) {
            return Err(refuse(format!(
                "directory {} is {fault}; whoever can write a directory on the path can \
                 swap the program the trusted core execs",
                dir.display()
            )));
        }
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
    /// The realm lock, held from before the purge until either the spawn
    /// commits (moved into [`SpawnedRealm`] by [`RuntimeDirGuard::keep`])
    /// or this guard drops and releases it. `Option` only so `keep` can
    /// move it out of a type that implements `Drop`.
    lock: Option<fs::File>,
}

impl RuntimeDirGuard {
    /// Take the realm's lock, then create
    /// `$XDG_RUNTIME_DIR/vitrin-0/<realm>` fresh at mode `0700`, purging a
    /// directory the lock has just proven stale (module docs).
    fn create(path: &Path, lock_path: &Path, realm_id: &str) -> Result<Self, SpawnError> {
        let refuse = |detail: String| SpawnError::RuntimeDir {
            path: path.to_path_buf(),
            detail,
        };

        // The enclosing `vitrin-0` tree may not exist yet: the listener
        // creates it when it binds, and a spawn can precede that.
        let parent = path
            .parent()
            .ok_or_else(|| refuse("has no parent directory".into()))?;
        let parent_fd = prepare_runtime_tree(parent).map_err(refuse)?;

        // Named relative to the verified parent from here on, so every
        // component these calls act on is a component that was checked.
        let name = path
            .file_name()
            .ok_or_else(|| refuse("has no final path component".into()))?;
        let lock_name = lock_path
            .file_name()
            .ok_or_else(|| refuse("lock path has no final component".into()))?;

        // Before the destructive part, and it is what licenses it: winning
        // this proves no other live core owns this realm (module docs).
        let lock = lock_realm(&parent_fd, lock_name, lock_path, realm_id)?;

        match rustix::fs::mkdirat(&parent_fd, name, Mode::from_bits_truncate(RUNTIME_DIR_MODE)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => {
                purge_stale_runtime_dir(&parent_fd, name, path).map_err(refuse)?;
                rustix::fs::mkdirat(&parent_fd, name, Mode::from_bits_truncate(RUNTIME_DIR_MODE))
                    .map_err(|e| {
                        refuse(format!("cannot recreate after purging a stale one: {e}"))
                    })?;
            }
            Err(e) => return Err(refuse(format!("cannot create: {e}"))),
        }

        // `mkdir`'s mode is masked by the process umask, so state the mode
        // rather than hoping for it: this directory holds the socket that
        // drives the realm. Through a descriptor opened `O_NOFOLLOW` on the
        // directory just created, so the chmod cannot land on a substituted
        // target the way a path-based one could.
        let dir_fd = rustix::fs::openat(
            &parent_fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| refuse(format!("cannot reopen the directory just created: {e}")))?;
        rustix::fs::fchmod(&dir_fd, Mode::from_bits_truncate(RUNTIME_DIR_MODE))
            .map_err(|e| refuse(format!("cannot chmod to {RUNTIME_DIR_MODE:o}: {e}")))?;

        Ok(Self {
            path: path.to_path_buf(),
            armed: true,
            lock: Some(lock),
        })
    }

    /// The spawn succeeded: the directory now belongs to the running realm,
    /// and its removal is P1.5.3's (#32). Yields the realm lock, which the
    /// live [`SpawnedRealm`] holds from here on -- the guard disarms, the
    /// lock does not.
    fn keep(mut self) -> fs::File {
        self.armed = false;
        self.lock
            .take()
            .expect("the guard owns its lock until keep() consumes it, exactly once")
    }
}

/// Take this realm's exclusive, non-blocking `flock`.
///
/// The lock file is created if absent and **never unlinked**. That is the
/// whole reason this is simpler than [`vitrin_ipc::Listener`]'s equivalent:
/// the listener unlinks its lock file on drop, which opens a race where a
/// binder can win the lock on an orphaned inode, and it pays for that with
/// an fstat/stat re-verification retry loop. A lock file that is never
/// removed has no such race -- every process that ever opens this path
/// opens the same inode -- and the file costs one inode on a tmpfs that the
/// session's end removes wholesale.
///
/// Opened `openat`-relative to the verified parent and `O_NOFOLLOW`, like
/// every other name this module touches: a lock taken on a symlink's target
/// is a lock on a file nobody else will consult, which is worse than no lock
/// because it looks like one. `lock_path` is carried only to name the file
/// in errors.
fn lock_realm(
    parent_fd: &OwnedFd,
    lock_name: &OsStr,
    lock_path: &Path,
    realm_id: &str,
) -> Result<fs::File, SpawnError> {
    let fd = rustix::fs::openat(
        parent_fd,
        lock_name,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|e| SpawnError::RuntimeDir {
        path: lock_path.to_path_buf(),
        detail: format!("cannot open the realm lock: {e}"),
    })?;
    let file = fs::File::from(fd);
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
        |_| SpawnError::RealmBusy {
            realm: realm_id.to_string(),
            lock: lock_path.to_path_buf(),
        },
    )?;
    Ok(file)
}

/// Create (if needed) and verify the enclosing `vitrin-0` tree, returning a
/// descriptor for it.
///
/// Every step is descriptor-bound on purpose. The path-based
/// `create_dir_all` + `set_permissions` this replaced both follow symlinks,
/// so a symlink planted at `vitrin-0` was accepted silently, its target
/// chmodded to `0700`, and every realm directory then created inside a tree
/// the core did not choose (module docs).
fn prepare_runtime_tree(parent: &Path) -> Result<OwnedFd, String> {
    // Only the final `vitrin-0` component is ours to make: `$XDG_RUNTIME_DIR`
    // is the session manager's, and silently building a *path* that should
    // already exist would paper over a misconfigured environment.
    match rustix::fs::mkdir(parent, Mode::from_bits_truncate(RUNTIME_DIR_MODE)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(e) => return Err(format!("cannot create {}: {e}", parent.display())),
    }
    let fd = open_owned_dir(parent)?;
    // Through the descriptor, so this cannot be redirected at a target the
    // check above did not see.
    rustix::fs::fchmod(&fd, Mode::from_bits_truncate(RUNTIME_DIR_MODE)).map_err(|e| {
        format!(
            "cannot chmod {} to {RUNTIME_DIR_MODE:o}: {e}",
            parent.display()
        )
    })?;
    Ok(fd)
}

/// Open `dir` as a directory this core owns: `O_NOFOLLOW | O_DIRECTORY`, and
/// then `fstat`-confirmed to belong to this euid.
///
/// Factored out of [`prepare_runtime_tree`] so [`remove_runtime_dir`] reaches
/// the realm tree through the *same* verified descriptor the spawn path used
/// rather than a second, weaker resolution of the same name -- the delete
/// below is recursive, and a cleanup routine that re-derives its own root is
/// how one ends up deleting somebody's home directory.
fn open_owned_dir(dir: &Path) -> Result<OwnedFd, String> {
    let fd = rustix::fs::open(
        dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| {
        format!(
            "{} is not a directory this core may use ({e}); refusing",
            dir.display()
        )
    })?;
    let st = rustix::fs::fstat(&fd).map_err(|e| format!("cannot stat {}: {e}", dir.display()))?;
    let euid = rustix::process::geteuid().as_raw();
    if st.st_uid != euid {
        return Err(format!(
            "{} is owned by uid {}, not the core's uid {euid}; refusing to place realm \
             runtime directories in a tree this core does not own",
            dir.display(),
            st.st_uid
        ));
    }
    Ok(fd)
}

/// Remove a realm's private runtime directory at an **orderly** exit -- the
/// seam [`RuntimeDirGuard::keep`] deliberately left open for P1.5.3 (#32).
///
/// Called by [`crate::lifecycle`] only after that module has confirmed the
/// realm's shim is reaped, and only on an orderly shutdown: a crash keeps
/// the tree (its docs argue why, and the next spawn's stale-purge collects
/// it either way). Two properties this spelling buys that a plain
/// `remove_dir_all` would not:
///
/// - It goes through [`purge_stale_runtime_dir`], so the *one* recursive
///   delete in this crate is the one whose `O_NOFOLLOW | O_DIRECTORY` open,
///   directory-type check and ownership check are already argued and
///   already tested. A second delete path would be a second set of proofs
///   to keep true.
/// - It is reached through the verified parent descriptor, so the only
///   component still resolved by name is the last one -- and the caller
///   still holds the realm `flock`, which excludes the only same-uid
///   process with any business at this path.
///
/// Errors are returned rather than swallowed so the caller can log them; a
/// failure here is untidy, never unsafe (the next spawn purges what is
/// left).
pub(crate) fn remove_runtime_dir(runtime_dir: &Path) -> Result<(), String> {
    let parent = runtime_dir
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", runtime_dir.display()))?;
    let name = runtime_dir
        .file_name()
        .ok_or_else(|| format!("{} has no final path component", runtime_dir.display()))?;
    // Deliberately not `prepare_runtime_tree`: at shutdown there is nothing
    // to create and nothing to chmod, and a cleanup routine that would
    // *mkdir* the tree it is about to delete from is one refactor away from
    // creating the very thing it then removes.
    let parent_fd = open_owned_dir(parent)?;
    purge_stale_runtime_dir(&parent_fd, name, runtime_dir)
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

/// Recursively remove a realm runtime directory -- after proving it *is* a
/// directory this core owns. The crate's single recursive delete, with two
/// callers: [`RuntimeDirGuard::create`], purging a tree this realm's `flock`
/// has just proven belongs to a run that is gone, and [`remove_runtime_dir`],
/// clearing this run's own tree at an orderly exit. Both hold the realm lock
/// across the call, which is what excludes the only same-uid process with
/// business at this path.
///
/// The `O_NOFOLLOW | O_DIRECTORY` open is the whole point: a recursive
/// delete that followed a planted symlink is how a cleanup routine deletes
/// someone's home directory. It is an `openat` through the caller's verified
/// parent descriptor, so the only component still resolved by name is the
/// last one.
///
/// The residual, stated rather than implied: std offers no `remove_dir_all`
/// rooted at a descriptor, so the delete re-resolves that last component. It
/// is `parent_fd` and the realm lock -- not this check -- that make the
/// substitution window uninteresting: the parent is proven `0700` and ours,
/// so no other uid can rename entries in it, and the lock excludes the only
/// same-uid process with any business here. (std's own `remove_dir_all`
/// walks with `openat`/`O_NOFOLLOW` internally, so the *traversal* below the
/// top is not the exposure.)
fn purge_stale_runtime_dir(parent_fd: &OwnedFd, name: &OsStr, path: &Path) -> Result<(), String> {
    let fd = rustix::fs::openat(
        parent_fd,
        name,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("is not a directory this core may remove ({e}); refusing"))?;
    let st = rustix::fs::fstat(&fd).map_err(|e| format!("exists but cannot be stat'ed: {e}"))?;
    // Stated rather than inferred from the open flags. `O_PATH | O_NOFOLLOW`
    // deliberately opens a *symlink itself* instead of failing, and it is
    // only the accompanying `O_DIRECTORY` that rejects one -- an interaction
    // subtle enough that the recursive delete below should not rest on it.
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != FileType::Directory {
        return Err("is not a directory; refusing to remove it".into());
    }
    let euid = rustix::process::geteuid().as_raw();
    if st.st_uid != euid {
        return Err(format!(
            "is owned by uid {}, not the core's uid {euid}; refusing to remove a directory \
             this core did not create",
            st.st_uid
        ));
    }
    drop(fd);
    fs::remove_dir_all(path).map_err(|e| format!("cannot remove the directory: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::capture::tests::fd_lock;
    use crate::realm::tests::realm_with_spawn;
    use crate::recorder::tests::{read_log, scratch_recorder, Json};

    /// How long a test waits for a child to reach a state it must reach
    /// (exec'd, forked a grandchild). Generous: these are process
    /// operations on a loaded CI runner, not a latency assertion.
    pub(crate) const DEADLINE: Duration = Duration::from_secs(10);

    /// A private scratch tree standing in for `$XDG_RUNTIME_DIR`.
    pub(crate) fn scratch() -> PathBuf {
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
    pub(crate) fn mock_shim_bin() -> PathBuf {
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
    pub(crate) fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
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
    pub(crate) fn wait_for_exec(pid: u32, program: &Path) {
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
    pub(crate) fn child_fds_of(pid: u32) -> BTreeMap<i32, String> {
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
    pub(crate) fn ppid_of(pid: u32) -> Option<u32> {
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
    pub(crate) fn children_of(pid: u32) -> BTreeSet<u32> {
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
    fn a_descriptor_the_core_left_inheritable_still_does_not_cross_the_fork() {
        // The decoys in the test above are all CLOEXEC from birth, so they
        // prove the rest of the core keeps P1.2.1's discipline -- they
        // cannot prove this module does anything. These are opened
        // *deliberately without* CLOEXEC, the only kind of descriptor the
        // closure's `close_range` sweep is the difference for. Remove the
        // sweep and this test fails; that is its whole job.
        let _fd = fd_lock();

        // Several, because one is not enough to be sure of catching
        // anything: the lowest free descriptor may well *be* `SHIM_CORE_FD`,
        // and the closure's `dup3` closes whatever sits there as a side
        // effect -- so a lone decoy can vanish for a reason that has nothing
        // to do with descriptor hygiene, and the test would pass vacuously.
        // Opening four guarantees at least one lands above the target.
        let decoys: Vec<OwnedFd> = (0..4)
            .map(|_| {
                rustix::fs::open("/proc/self/status", OFlags::RDONLY, Mode::empty())
                    .expect("inheritable decoy")
            })
            .collect();
        let above: Vec<i32> = decoys
            .iter()
            .map(|d| d.as_raw_fd())
            .filter(|n| *n > SHIM_CORE_FD)
            .collect();
        assert!(
            !above.is_empty(),
            "the decoys must be able to detect a leak, or this test proves nothing"
        );

        let mut h = Harness::new("spawn-inheritable-fd");
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);
        let fds = child_fds_of(spawned.pid());

        assert_eq!(
            fds.keys().copied().collect::<Vec<_>>(),
            vec![0, 1, 2, SHIM_CORE_FD],
            "descriptors the core left inheritable ({above:?}) must still not reach \
             the child: {fds:?}"
        );

        drop(decoys);
        h.reap(spawned);
    }

    // -- acceptance: the child starts from a defined signal state ----------

    /// The bits of `/proc/<pid>/status`'s `SigIgn` mask, as signal numbers.
    fn ignored_signals_of(pid: u32) -> BTreeSet<i32> {
        let status = fs::read_to_string(format!("/proc/{pid}/status")).expect("status is readable");
        let line = status
            .lines()
            .find(|l| l.starts_with("SigIgn:"))
            .expect("status reports SigIgn");
        let mask = u64::from_str_radix(line.trim_start_matches("SigIgn:").trim(), 16)
            .expect("SigIgn is hexadecimal");
        (0..64)
            .filter(|b| mask >> b & 1 == 1)
            .map(|b| b + 1)
            .collect()
    }

    #[test]
    fn an_ignored_signal_in_the_cores_launch_context_never_reaches_the_child() {
        // `execve` preserves SIG_IGN and std resets only SIGPIPE, so without
        // the closure's reset loop the operator's launch context decides
        // which signals the confined app is immune to -- SIGTERM included,
        // which is the signal P1.5.3 terminates a realm with.
        //
        // Made deterministic without any `unsafe` in this crate: `sh`'s
        // `trap "" SIG` is SIG_IGN, so re-running this one test under a
        // trapping shell gives the *core* an inherited SigIgn to launder.
        // The inner run does the spawning and asserting; the outer run only
        // arranges the launch context and reports the inner verdict.
        const INNER: &str = "VITRIN_SPAWN_SIGTEST_INNER";
        let _fd = fd_lock();

        if std::env::var_os(INNER).is_some() {
            // Inner: this process was exec'd with SIGINT/SIGQUIT/SIGTERM
            // ignored, exactly as `vitrind &` from a shell would be.
            let inherited = ignored_signals_of(std::process::id());
            for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
                assert!(
                    inherited.contains(&sig),
                    "the wrapper must hand this run signal {sig} ignored, else the \
                     assertion below proves nothing: {inherited:?}"
                );
            }

            let mut h = Harness::new("spawn-signal-reset");
            let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);
            let child = ignored_signals_of(spawned.pid());
            h.reap(spawned);

            for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
                assert!(
                    !child.contains(&sig),
                    "signal {sig} crossed execve as SIG_IGN: the realm's app inherited \
                     the operator's launch context, and SIGTERM immunity would make the \
                     realm unkillable by the signal lifecycle terminates it with: {child:?}"
                );
            }
            return;
        }

        // Outer: re-run *this test only* under a shell that ignores the
        // three signals. `$0` is the test binary; `--exact` keeps the inner
        // run to this one test, and `--test-threads=1` keeps its harness
        // from interleaving anything else with the spawn.
        let exe = std::env::current_exe().expect("the test binary has a path");
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(
                r#"trap "" INT QUIT TERM; exec "$0" --exact \
                   spawn::tests::an_ignored_signal_in_the_cores_launch_context_never_reaches_the_child \
                   --test-threads=1 --nocapture"#,
            )
            .arg(&exe)
            .env(INNER, "1")
            .stdin(Stdio::null())
            .status()
            .expect("re-running this test under a trapping shell");
        assert!(
            status.success(),
            "the inner run (launched with INT/QUIT/TERM ignored) failed: {status}"
        );
    }

    /// The bits of a procfs `status` file's `SigBlk` mask, as signal
    /// numbers.
    ///
    /// The path is a parameter because the blocked mask is **per thread**:
    /// `/proc/<pid>/status` reports the thread-group leader's, which is
    /// the right answer for a freshly `exec`ed child (single-threaded, so
    /// pid == tid) and the wrong one for this multi-threaded test harness,
    /// where `pthread_sigmask` affects only the calling thread and
    /// `/proc/thread-self/status` is the honest reading.
    fn blocked_signals_at(status_path: &str) -> BTreeSet<i32> {
        let status = fs::read_to_string(status_path).expect("status is readable");
        let line = status
            .lines()
            .find(|l| l.starts_with("SigBlk:"))
            .expect("status reports SigBlk");
        let mask = u64::from_str_radix(line.trim_start_matches("SigBlk:").trim(), 16)
            .expect("SigBlk is hexadecimal");
        (0..64)
            .filter(|b| mask >> b & 1 == 1)
            .map(|b| b + 1)
            .collect()
    }

    #[test]
    fn the_child_starts_with_an_empty_signal_mask() {
        // The mask half of "a realm's process tree starts from a defined
        // state" (module docs). `execve` does not clear the blocked set and
        // `std::process::Command` does not either, so without the closure's
        // `sigprocmask` the child inherits whatever the core had blocked --
        // and this core blocks SIGINT/SIGTERM the moment either backend
        // installs calloop's signalfd source, plus SIGCHLD for
        // `crate::lifecycle`'s reaper.
        //
        // A *blocked* SIGTERM is not the same failure as an *ignored* one
        // and is not fixed by the disposition reset the sibling test
        // covers: SIG_DFL on a blocked signal is still an unkillable realm,
        // because a blocked signal is never delivered to any disposition at
        // all. It would silently degrade every realm's shutdown to SIGKILL
        // and leave the confined app unable to reap its own children.
        //
        // Asserted on the child's ACTUAL mask, from the kernel, and made
        // able to fail: this run blocks three signals across the fork, so
        // deleting the closure's reset makes them show up below.
        let _fd = fd_lock();

        let mut blocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        let mut previous = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: sigset manipulation and `pthread_sigmask` on this thread
        // only, with two stack sigsets this call initializes. Restored
        // before the test returns, so no other test inherits the mask.
        unsafe {
            libc::sigemptyset(blocked.as_mut_ptr());
            libc::sigaddset(blocked.as_mut_ptr(), libc::SIGTERM);
            libc::sigaddset(blocked.as_mut_ptr(), libc::SIGCHLD);
            libc::sigaddset(blocked.as_mut_ptr(), libc::SIGUSR1);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, blocked.as_ptr(), previous.as_mut_ptr()),
                0
            );
        }
        let ours = blocked_signals_at("/proc/thread-self/status");
        for sig in [libc::SIGTERM, libc::SIGCHLD, libc::SIGUSR1] {
            assert!(
                ours.contains(&sig),
                "the core must really have signal {sig} blocked, else the assertion \
                 below proves nothing: {ours:?}"
            );
        }

        let mut h = Harness::new("spawn-signal-mask");
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);
        let child = blocked_signals_at(&format!("/proc/{}/status", spawned.pid()));
        h.reap(spawned);

        // SAFETY: restoring the mask this test installed, same thread.
        unsafe {
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_SETMASK, previous.as_ptr(), std::ptr::null_mut()),
                0
            );
        }

        assert!(
            child.is_empty(),
            "the child's signal mask must be empty: the core's blocked set crosses fork \
             AND execve, so an inherited block would make the realm immune to the very \
             signal crate::lifecycle terminates it with -- got {child:?}"
        );
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

    /// The (realm directory, realm lock) pair [`RuntimeDirGuard::create`]
    /// takes, derived from a scratch base exactly the way [`launch`] does.
    fn dir_and_lock(base: &Path) -> (PathBuf, PathBuf) {
        let paths = SpawnPaths::under(base);
        (
            paths.realm_dir("realm-0").unwrap(),
            paths.realm_lock("realm-0").unwrap(),
        )
    }

    #[test]
    fn a_stale_runtime_directory_is_purged_not_reused() {
        // A directory at this path belongs to a run that is gone: reusing it
        // would carry a dead run's socket into a new run's confinement.
        let _fd = fd_lock();
        let base = scratch();
        let (realm_dir, lock) = dir_and_lock(&base);
        fs::create_dir_all(&realm_dir).unwrap();
        let stale = realm_dir.join("wayland-0");
        fs::write(&stale, b"stale socket from a crashed run").unwrap();
        // Deliberately wrong mode, as a crashed run may well have left.
        fs::set_permissions(&realm_dir, fs::Permissions::from_mode(0o755)).unwrap();

        let guard = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0")
            .expect("a stale directory is purged");
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
        let (realm_dir, lock) = dir_and_lock(&base);
        std::os::unix::fs::symlink(&victim, &realm_dir).unwrap();

        let err = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0")
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
        let (realm_dir, lock) = dir_and_lock(&base);

        let guard = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0").unwrap();
        assert!(realm_dir.is_dir());
        drop(guard);
        assert!(
            !realm_dir.exists(),
            "an unkept guard unwinds the directory it created"
        );

        let guard = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0").unwrap();
        let held = guard.keep();
        assert!(realm_dir.is_dir(), "a kept directory survives");
        drop(held);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_second_core_is_refused_rather_than_purging_a_live_realms_directory() {
        // The purge is a recursive delete licensed by "this realm belongs to
        // a run that is gone". Nothing used to establish that: the module
        // cited the listener's flock, but the core constructs no listener,
        // so two vitrind processes against one $XDG_RUNTIME_DIR would have
        // had the second delete the first's live socket directory. The lock
        // is what makes the claim true, so this asserts the collision is
        // refused *and* that the live directory survives the refusal.
        let _fd = fd_lock();
        let base = scratch();
        let (realm_dir, lock) = dir_and_lock(&base);

        let first = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0")
            .expect("the first core prepares the realm");
        let live_socket = realm_dir.join("wayland-0");
        fs::write(&live_socket, b"a running realm's socket").unwrap();

        let err = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0")
            .expect_err("a second core must not take over a live realm");
        assert!(matches!(err, SpawnError::RealmBusy { .. }), "{err}");
        assert_eq!(err.cause_class(), "realm_busy");
        assert!(
            live_socket.exists(),
            "the live realm's socket must survive a second core's spawn attempt"
        );

        // Once the first core is gone the kernel drops the lock, and the
        // next run self-heals exactly as the module promises.
        drop(first);
        fs::create_dir_all(&realm_dir).unwrap();
        fs::write(&live_socket, b"left by a run that crashed").unwrap();
        let third = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0")
            .expect("a genuinely stale realm is purged");
        assert!(
            !live_socket.exists(),
            "a stale directory's contents must not survive into the new run"
        );
        drop(third);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_symlinked_runtime_tree_is_refused_never_chmodded_through() {
        // One level up from the realm directory, and the same rule: the
        // path-based create_dir_all + set_permissions this replaced would
        // both follow the link, chmod the *target* to 0700, and put every
        // realm directory inside a tree the core never chose.
        let _fd = fd_lock();
        let base = scratch();
        let victim = base.join("someone-elses");
        fs::create_dir_all(&victim).unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&victim, base.join("vitrin-0")).unwrap();

        let (realm_dir, lock) = dir_and_lock(&base);
        let err = RuntimeDirGuard::create(&realm_dir, &lock, "realm-0")
            .expect_err("a symlinked runtime tree must be refused");
        assert!(matches!(err, SpawnError::RuntimeDir { .. }), "{err}");
        assert_eq!(
            fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o755,
            "the symlink target's mode must be untouched"
        );
        let _ = fs::remove_dir_all(&base);
    }

    // -- the spawn-time audit is the load-time audit, not a weaker one -----

    #[test]
    fn a_relative_command_is_refused_before_anything_is_created() {
        // The alarm half of the same doctrine `ReservedEnv` serves: the
        // loader refuses a relative `command` already, so reaching here
        // means validation was bypassed. It matters more than the env case
        // because `launch` chdirs the child into the realm's runtime
        // directory -- writable by the confined app -- so a relative
        // program would let the audit describe one file and `execve`
        // another.
        let _fd = fd_lock();
        let realm = realm_with_spawn("realm-0", Path::new("./app"), &[], &[]);
        let (err, entry) = refused_spawn("spawn-relative-command", &realm);
        assert!(matches!(err, SpawnError::RelativeCommand { .. }), "{err}");
        assert_eq!(err.cause_class(), "relative_command");
        assert_eq!(entry.str("cause_class"), "relative_command");
    }

    #[test]
    fn a_group_writable_ancestor_is_refused_at_spawn_time() {
        // The load-time audit walks every ancestor of the resolved program,
        // because whoever can write a directory on the path can swap the
        // program. The spawn-time re-audit claimed to re-apply "exactly this
        // rule" while checking only the program's own inode -- so in the one
        // window the re-audit exists to cover, half the policy went
        // unenforced.
        let _fd = fd_lock();
        let dir = scratch();
        let vendor = dir.join("vendor");
        fs::create_dir_all(&vendor).unwrap();
        let program = vendor.join("app");
        fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        // The program itself is beyond reproach; its directory is not.
        fs::set_permissions(&vendor, fs::Permissions::from_mode(0o775)).unwrap();

        let realm = realm_with_spawn("realm-0", &program, &[], &[]);
        let (err, _) = refused_spawn("spawn-writable-ancestor", &realm);
        assert!(matches!(err, SpawnError::ProgramAudit { .. }), "{err}");
        assert!(
            err.to_string().contains("directory") && err.to_string().contains("swap"),
            "the refusal must name the directory as the fault: {err}"
        );
        fs::set_permissions(&vendor, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_spawned_realm_dropped_before_adoption_leaves_no_process_and_no_zombie() {
        // The gap between `spawn_realm` returning and
        // `RealmLifecycle::adopt` taking over. `std::process::Child` has no
        // `Drop`, so without the guard this window abandons the shim: it
        // keeps running, and once it exits it is a zombie for the core's
        // whole session because nothing will ever wait on it.
        //
        // The window is real, not theoretical: `start_shim_session` sits
        // inside it and is fallible (a blocking `configure` that fails with
        // EPIPE against a shim that exec'd and died at once), so a plain `?`
        // there drops a `SpawnedRealm` mid-unwind. Every other zombie test
        // in this repo reaches `adopt` first and cannot see it.
        /// A pid's procfs state character, or `None` if it is gone
        /// entirely. Reading the state is the point: a zombie's `/proc`
        /// entry still exists, so "the directory is gone" would pass for
        /// exactly the failure under test.
        fn proc_state(pid: u32) -> Option<char> {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // The comm field may contain ')' and spaces, so the state is
            // the first token after the LAST ')'.
            let after = stat.rsplit_once(')')?.1;
            after.split_whitespace().next()?.chars().next()
        }

        let _fd = fd_lock();
        let mut h = Harness::new("spawn-drop-before-adopt");
        // Spawned but NOT brought up: the shim is parked on its
        // socketpair, exactly where it sits when a caller bails out
        // between `spawn_realm` and `adopt`.
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths();
        let spawned = spawn_realm_with_env(&realm, &paths, &mut h.recorder, |_| None)
            .expect("spawn must succeed against a scratch runtime tree");
        let pid = spawned.pid();
        wait_for_exec(pid, &bin);
        assert!(
            proc_state(pid).is_some_and(|s| s != 'Z'),
            "the shim is running before the drop"
        );

        drop(spawned);

        assert!(
            proc_state(pid) != Some('Z'),
            "a dropped SpawnedRealm must reap its shim, not merely abandon it as a zombie"
        );
        assert!(
            !children_of(std::process::id()).contains(&pid),
            "the shim must not remain an unreaped child of the core"
        );
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
