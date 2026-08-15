// SPDX-License-Identifier: MPL-2.0
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
//!   live run's socket directory out from under it.
//!
//!   **There are now two locks, and this one is not redundant.** Since the
//!   M1.1 runtime wiring the core really does bind a `Listener`, so the
//!   tree-level `core.sock.lock` is finally held by somebody -- which
//!   retires the old note that it was a justification resting on a lock
//!   nobody took. The tempting inverse conclusion is wrong: the tree lock
//!   cannot replace this one. Their **lifetimes are independent**: the
//!   listener lives inside the event loop, and realm teardown -- the
//!   shutdown ladder's hangup, `SIGTERM`, `SIGKILL`, directory removal --
//!   is a separate step ordered against it only by where the calls happen
//!   to sit. Today `session::shutdown_realm` runs after `event_loop.run`
//!   returns but while the loop object is still alive, so the tree lock
//!   does outlast the teardown -- by arrangement, not by construction, and
//!   nothing would fail loudly if that arrangement changed. A realm
//!   directory being recursively deleted must not depend on an unrelated
//!   lock's scope for its safety, which is why this lock exists and why
//!   `into_parts` releases it last of all. Their
//!   **objects differ**: the tree lock guards a socket path and its worst
//!   unlicensed act is unlinking one socket, whereas this one licenses a
//!   *recursive delete*, and a lock should never be coarser than the
//!   destruction it authorizes. And their **granularity differs**: one core
//!   with N realms is the intended shape, which a per-realm lock survives
//!   and a per-tree lock does not.
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
//! What the injected `XDG_RUNTIME_DIR` is, and what it stopped being
//! (P2.6.2, #186):
//!
//! - **At `--isolation=off` it is not a jail.** Its value is
//!   `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>`, a subdirectory of the tree that
//!   also holds the core's own principal-facing `core.sock` and this run's
//!   flight-recorder log -- so the value names the core's control plane one
//!   level up (`../core.sock`). Relocating the tree would not help: the child
//!   runs as the core's uid and can derive `/run/user/<uid>` from `getuid()`
//!   whether or not any variable points at it, so moving it would change what
//!   the app is *told*, not what it can reach. The variable is redirected so
//!   a well-behaved client finds its own realm's socket, which is a
//!   confinement of the well-behaved.
//! - **At `--isolation=default` the same value becomes structural, and it
//!   does so by a different mechanism than the one this paragraph used to
//!   reach for.** The value is now the fixed in-realm `/run/vitrin`, a bind
//!   of that same core-created directory -- and `..` from a bind-mount target
//!   resolves to the target's parent *in the realm's own tree*, where there
//!   is no `core.sock` and no recorder log. The closure is the mount
//!   namespace's, not the path's; that is also why the private-tmpfs
//!   alternative was refused, since it would have handed the child creation
//!   of the directory holding its own app-facing socket. And it is checked
//!   rather than argued: `core.sock` and the recorder path are two of the
//!   canaries every confined spawn probes through `/proc/<shim>/root`.
//!
//! Stdio: **stdin is `/dev/null` in both modes.** A child sharing the
//! operator's terminal *stdin* would be competing for the operator's
//! keystrokes, which is precisely the ambient input authority this display
//! server exists to mediate.
//!
//! Stdout and stderr differ, and this one is not hygiene:
//!
//! - **At `off`, inherited**, with the residual that a hostile child can
//!   write terminal escape sequences to the operator's terminal.
//! - **At `default`, a per-realm log file** inside the realm's own runtime
//!   directory, named in a startup log line and tailed into the core's stderr
//!   when a realm dies during bring-up. The reason is that `close_range`
//!   starts at 3 by construction, so descriptors 1 and 2 cross *both*
//!   `execve`s untouched -- and on a bare-DRM session they are open
//!   descriptors on `/dev/ttyN`. **A mount table has no say over a device
//!   that arrived as a descriptor**, so the `/dev` closure could not be
//!   claimed honestly with them inherited. (The same argument applies to
//!   `/dev/tty`, which names the *controlling terminal of the opener*; the
//!   helper's PID 1 calls `setsid` so the realm has none.) The objection this
//!   trades against -- "a shim that cannot say why it failed to start is
//!   undebuggable" -- is answered rather than accepted: the helper writes the
//!   failing mount entry and its errno to that file, and a bring-up failure
//!   puts the tail of it both in the core's log and in the refusal itself.
//!
//! # SECURITY POSTURE -- READ THIS BEFORE BELIEVING ANY OF THE ABOVE
//!
//! ## Which of the two spawn paths ran decides everything below
//!
//! `--isolation` selects one of two code paths in this file, and they make
//! genuinely different claims. **Two spawn paths inside the TCB is a real
//! cost**, stated because it is exactly how a confinement claim rots: the
//! only thing keeping `default` from silently degrading into `off` is that
//! the confined arm proves its confinement from outside before it commits.
//!
//! ### `--isolation=off`: D9's posture, unchanged and still accurate
//!
//! Confinement is **environment-structural only**: a private socket, a
//! scrubbed environment, a private runtime directory, and a closed descriptor
//! table. That is the complete list. There are no namespaces, no seccomp
//! filter and no Landlock policy; the shim and its app run as the core's own
//! uid with the core's full view of the filesystem, the network and every
//! socket on the machine. An app that *ignores* `WAYLAND_DISPLAY` and
//! connects to a path it already knows is not stopped by anything here. This
//! is a *confinement of the well-behaved*, not a containment of the hostile,
//! and the session warns about it every sixty seconds for its whole life.
//!
//! ### `--isolation=default`: namespaces and Landlock, and not seccomp
//!
//! The realm gets its own user, mount, PID, IPC, UTS and network namespaces,
//! an identity uid/gid map, zero capabilities, `no_new_privs`, a read-only
//! root whose entire writable set is `{/run/vitrin, /vitrin/home, /tmp,
//! /dev/shm}`, and an enumerated `/dev` with no `/dev/input` in it. Since
//! P2.6.3 (#187) it also gets a **Landlock ruleset**, enforced by
//! `vitrin-realm-init` immediately before the shim's `execve` and therefore
//! inherited by every descendant including the app the shim forks: the four
//! writable hierarchies above, an *enumerated* read+exec set for the
//! read-only runtime paths, and nothing else.
//!
//! PRD Doc 2 §4.1 describes the child as spawned "in an unprivileged sandbox
//! (namespaces/seccomp)"; **this build does two thirds of that.** Seccomp is
//! P2.6.4 (#188), so the journal says `applied_profile=namespaces+landlock-abiN`
//! rather than any tier name -- `intra-user` is *defined* as all three.
//!
//! **The `N` is the rung the realm obtained, not the rung it asked for**, so a
//! ladder that fell from 9 to 1 is visible in the field named for what was
//! applied rather than only inside a JSON blob. [`warn_on_landlock_shortfall`]
//! additionally logs a WARN per spawn whenever the obtained rung is below the
//! request or below the kernel's own ABI, naming both numbers and the rights
//! that moved between them.
//!
//! The rung the ruleset was built at is journaled beside the ABI the kernel
//! reported, both **child-asserted**: there is no `/proc` file naming a
//! process's Landlock domain, so unlike the namespace inodes the parent
//! cannot corroborate them. What cannot be forged is the realm's behaviour,
//! which `tests/integration/test_real_confinement.py` measures from inside --
//! it drives one probe the mount table leaves reachable and the ruleset
//! denies, with `--landlock=off` in the same run as the positive control.
//!
//! Three residuals travel with it, published rather than papered over:
//!
//! - **Supplementary groups survive.** `setgroups=deny` blocks the *call* and
//!   drops nothing, and an unprivileged realm has no window in which to drop
//!   them itself -- see [`IsolationFacts::supplementary_groups_retained`] for
//!   the two kernel predicates that make the windows disjoint. Every
//!   `realm_spawned` entry carries the count.
//! - **Within one realm, the app can unlink and rebind any socket in
//!   `/run/vitrin`**, including `wayland-0` and the reserved a11y bus path.
//!   The shim and the app are one uid inside a single-id map, so mode bits
//!   cannot separate them. Blast radius is that one realm; the closure is
//!   P2.6.3's shim-side Landlock stack.
//! - **Realm private storage has no quota**, and the GPU render node is bound
//!   read-write with its whole ioctl surface.
//!
//! ## The session D-Bus hole: open at `off`, closed at `default`
//!
//! The session bus stayed reachable in the MVP because Firefox -- the P1
//! acceptance app -- wants it. At `--isolation=off` that is still exactly
//! true, and the distinction that mattered still matters:
//!
//! - The core injects no `DBUS_SESSION_BUS_ADDRESS` and points
//!   `XDG_RUNTIME_DIR` at the realm's private directory, so the bus is not
//!   *advertised*, and a well-behaved client finds nothing.
//! - That is advertisement, not reachability. `/run/user/<uid>/bus` is still
//!   on the filesystem and still connectable by any process of this uid, and
//!   the abstract-socket namespace is still shared.
//!
//! At `--isolation=default` it is **closed twice over**, and neither closure
//! is this file's cleverness -- both are the kernel's. The mount namespace
//! removes `/run/user/<uid>/bus` as a path (the realm's `/run` holds one
//! entry, `vitrin`), and the network namespace removes the abstract-socket
//! namespace the bus also listens on, because abstract sockets are scoped to
//! a network namespace. An operator who allow-lists
//! `DBUS_SESSION_BUS_ADDRESS` in `realm.toml` at `default` gets a variable
//! naming something that is not there.
//!
//! PRD Doc 2 §15 catalogues this shape (D-Bus activation of a privileged
//! helper) as a lateral-escape path, and P13 is where the *network* half of
//! the answer is finished; the mount half is here.
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

/// The runtime isolation preflight (P2.6.1, #185) and the selector that acts
/// on it (P2.6.2, #186): what confinement *this kernel* will grant, measured
/// rather than assumed, and what *this build* refuses to start without.
///
/// It sits under `spawn` because it measures precisely the requests the spawn
/// path makes -- the six-flag `unshare` its `ns.all` row probes is the exact
/// call `vitrin-realm-init` issues, one constant shared by both, so the
/// preflight cannot measure something weaker than what runs.
///
/// It landed a task ahead of the confinement because Phase 2's R2.9 --
/// unprivileged user namespaces restricted on major distros -- is the one
/// risk that can invalidate two whole epics, and it retires only by measuring
/// real kernels.
pub mod isolation;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustix::fs::{FileType, Mode, OFlags};
use vitrin_ipc::paths;
use vitrin_ipc::{Connection, TransportError};
use vitrin_realm_init::{
    landlock_audit_requested, Config as RealmInitConfig, Frame, LandlockRequest, Stage, TmpfsCaps,
    CONFIG_MAX, HELPER_DEADLINE, IN_REALM_HOME, IN_REALM_RUNTIME_DIR, IN_REALM_SHIM,
    IN_REALM_WAYLAND_SOCKET, LANDLOCK_AUDIT_ENV, MOUNT_FINGERPRINT_ALG,
};

use crate::grants::{GrantId, RealmId};
use crate::identity::PrincipalIdentity;
use crate::realm::{untrusted_writer, Realm, SpawnConfig, RESERVED_ENV};
use crate::recorder::{Event, Recorder};
use crate::shim::{ShimConfig, ShimServer};

pub(crate) use isolation::Isolation;

/// D-036(11), held by the compiler rather than by a comment.
///
/// Since WS-E.1.1 `launch` is reachable from an admitted `realm_launch`, so
/// the birth path is now a wire-reachable way to park the compositor.
/// [`crate::lifecycle`] draws the line for the *death* path in prose
/// ("nothing in the death-detection path blocks or sleeps"); the birth path
/// is bounded here, at exactly the shortest dead-man hold the off-switch will
/// accept -- so a realm being born can never out-wait the switch that would
/// kill it. Raising [`HELPER_DEADLINE`] without first moving
/// `deadman::MIN_HOLD` does not compile.
const _: () = assert!(
    HELPER_DEADLINE.as_millis() <= crate::deadman::MIN_HOLD.as_millis(),
    "the realm helper's handshake deadline exceeds the shortest dead-man hold: a realm being \
     born could out-wait the off-switch that would kill it"
);

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

/// Where a session's runtime tree lives, and which shim binary the core
/// execs to hold fd 3. Held explicitly rather than read from the environment
/// at each use so the spawn path is deterministic and testable against a
/// scratch base -- the same reason [`vitrin_ipc::paths`] ships a `*_in` form
/// of every helper.
///
/// # The shim is a *core* input, not a realm one
///
/// [`crate::realm`]'s `command` names the **app** (`/usr/bin/foot`); the core
/// never execs it directly. It execs a core-known **shim** binary, placing
/// the shim's end of the identity socketpair at [`SHIM_CORE_FD`], and conveys
/// the app command (`realm.command` + `realm.args`) to the shim in argv after
/// a `--` separator (PRD Doc 2 §4.1; issue #103). The shim then serves the
/// app-facing Wayland socket and `exec`s the app -- so no binary is ever both
/// an fd-3 core peer *and* a Wayland app, which is why the shim is inserted
/// rather than conflated with the app.
///
/// The shim is held here as an argv, not a bare path: its first element is
/// the program the core execs (audited transitively at spawn, exactly like
/// `command`), and any further elements are the shim's own leading arguments,
/// placed *before* the `--`/app tail. Production supplies a single element
/// (`--shim PATH`, defaulting to a sibling `vitrin-shim` beside `vitrind`);
/// the multi-element form exists for the lifecycle ladder tests, which spawn
/// `/bin/sh -c <script>` as the realm's direct child and need `-c <script>`
/// to be shim arguments rather than app ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnPaths {
    xdg_runtime_dir: PathBuf,
    /// The shim's argv: `[program, leading-args...]`. Never empty; the app
    /// command follows a `--` after these (module docs).
    shim: Vec<OsString>,
    /// `None` at `--isolation=off`, which is byte for byte the pre-#186
    /// spawn. `Some` at `--isolation=default`.
    ///
    /// An `Option` rather than an [`Isolation`] field plus a bag of maybe-set
    /// paths, so "confined" and "we have everything a confined spawn needs"
    /// are the same fact and the unconfined arm cannot accidentally read half
    /// a confinement.
    confinement: Option<Confinement>,
}

/// Everything the confined spawn path needs that the unconfined one has no
/// use for (P2.6.2, #186).
///
/// Held on [`SpawnPaths`] for the reason that type's docs already give about
/// the shim: these are **core** inputs, not realm ones. The realm names its
/// app; the core decides which helper binary confines it, which render nodes
/// exist, and which paths the confinement is proved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Confinement {
    /// The `vitrin-realm-init` binary, audited at spawn exactly like the shim
    /// and the app -- three programs, one rule (issue #103).
    pub realm_init: PathBuf,
    /// `$XDG_DATA_HOME/vitrin/realms`, the parent of every realm's private
    /// storage.
    pub storage_root: PathBuf,
    /// Absolute host paths that must be **unreachable** from inside the
    /// realm, checked through `/proc/<pid>/root` on every single spawn.
    ///
    /// This is the behavioural half of D-036(4): six differing namespace
    /// inodes prove a helper unshared, they prove nothing about what it
    /// mounted. A substituted helper that unshares and mounts nothing passes
    /// the inode check and fails this one.
    pub canaries: Vec<PathBuf>,
    /// `/dev/dri/renderD*`, enumerated and audited by the core at startup.
    /// Never `card*`, never `controlD*`.
    pub render_nodes: Vec<PathBuf>,
    /// The `size=` caps for the realm's four tmpfs mounts. A field rather
    /// than a constant read at the mount site so a test can shrink them.
    pub tmpfs: TmpfsCaps,
    /// Which Landlock rung this session's `--landlock` flag allows the helper
    /// to build (P2.6.3, #187).
    ///
    /// Session-wide, like `--isolation` and for the same reason (D-036(7)):
    /// whether a realm is confined -- and how far -- is not per-app
    /// information, so it does not live in `realm.toml` where a copied
    /// `[[realm]]` block could weaken one realm silently.
    pub landlock: LandlockRequest,
}

impl SpawnPaths {
    /// The session's real runtime tree, from `$XDG_RUNTIME_DIR`, with the
    /// core-known shim binary the CLI resolved (`--shim`, or its default).
    pub fn from_env(shim: impl Into<PathBuf>) -> Result<Self, paths::PathError> {
        Ok(Self {
            xdg_runtime_dir: paths::xdg_runtime_dir()?,
            shim: vec![shim.into().into_os_string()],
            confinement: None,
        })
    }

    /// Select `--isolation=default` for every realm this session spawns.
    ///
    /// Session-wide, deliberately (D-036(7)): one flag, one shape, and the
    /// confinement switch stays out of `realm.toml`, where a copied
    /// `[[realm]]` block could weaken a realm silently. `--realm-bind`-style
    /// per-app *paths* do live in `realm.toml` -- they are per-app
    /// information and the app is named there -- but whether a realm is
    /// confined at all is not per-app information.
    pub fn confined(mut self, confinement: Confinement) -> Self {
        self.confinement = Some(confinement);
        self
    }

    /// What this session applies. Derived rather than stored, so the two can
    /// never disagree.
    ///
    /// Read only by tests today, on the same terms as [`Self::under`]: the
    /// production code paths branch on the `Option` directly, because a
    /// branch on the derived enum would have to handle a case the type system
    /// already excluded. The attribute is verified rather than assumed --
    /// deleting it warns.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn isolation(&self) -> Isolation {
        match self.confinement {
            Some(_) => Isolation::Default,
            None => Isolation::Off,
        }
    }

    /// A runtime tree rooted at an explicit base with an explicit shim binary
    /// (tests, and any future caller that must not depend on ambient
    /// environment).
    ///
    /// The runtime uses [`Self::from_env`]; this exists so a spawn can be
    /// driven against a scratch tree without mutating the process
    /// environment, which a single-process test suite cannot do safely.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn under(xdg_runtime_dir: impl Into<PathBuf>, shim: impl Into<PathBuf>) -> Self {
        Self {
            xdg_runtime_dir: xdg_runtime_dir.into(),
            shim: vec![shim.into().into_os_string()],
            confinement: None,
        }
    }

    /// [`Self::under`] with a multi-element shim argv, for the lifecycle
    /// ladder tests that spawn `/bin/sh -c <script>` as the realm's direct
    /// child: the script is a *shim* argument, placed before the `--`/app
    /// tail, not an app one.
    #[cfg(test)]
    pub fn under_with_shim_argv(
        xdg_runtime_dir: impl Into<PathBuf>,
        shim_argv: Vec<OsString>,
    ) -> Self {
        assert!(
            !shim_argv.is_empty(),
            "a shim argv names at least a program"
        );
        Self {
            xdg_runtime_dir: xdg_runtime_dir.into(),
            shim: shim_argv,
            confinement: None,
        }
    }

    /// The shim binary the core execs -- argv[0] of the child, audited like
    /// `command`.
    fn shim_program(&self) -> &OsStr {
        self.shim
            .first()
            .expect("a shim argv names at least a program")
    }

    /// The shim's own leading arguments, before the `--`/app tail.
    fn shim_leading_args(&self) -> &[OsString] {
        &self.shim[1..]
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

    /// Test-only: the runtime reaches the child through
    /// [`SpawnedRealm::into_parts`] and `RealmLifecycle`, never directly.
    #[cfg_attr(not(test), allow(dead_code))]
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
/// The realm's writable set, and *how the journal came to know it*.
///
/// Three cases rather than a string, because "the core measured an empty-ish
/// list", "there was nothing to measure" and "the core could not tell" are
/// three different things and a reader auditing one entry has to be able to
/// separate them. Collapsing them into prose is how a confinement claim comes
/// to rest on a sentence nobody checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WritableSet {
    /// `--isolation=off`: there is no mount namespace, so the realm's
    /// writable set is the operator's own and there is nothing to measure.
    Unconfined,
    /// Mount points carrying the per-mount `rw` option in
    /// `/proc/<shim>/mountinfo`, sorted. This is the short list P2.6.9's
    /// measured-write-set gate will assert against.
    Measured(Vec<String>),
    /// The read failed, and the journal says so rather than substituting the
    /// list the core intended to build.
    Unreadable(String),
}

/// What the flight recorder is told about one realm's confinement, split
/// into what the **parent read from the kernel** and what the **child said**.
///
/// The split is the whole design, restated as a struct layout:
///
/// > `applied` is computed from what the kernel says about the child, never
/// > from the flag that was requested, the path that was resolved, or the
/// > table that was sent.
///
/// Everything above [`Self::mount_count`] is a value this process read out of
/// `/proc` *after* the child existed. Everything from there down is a number
/// the child sent, and the journal labels it as such -- a substituted helper
/// can put anything it likes in those two fields, which is exactly why they
/// are not what licenses the spawn.
///
/// # The one number that arrives from the child and stays above the line
///
/// [`Self::shim_host_pid`] is *reported* by the helper in `Frame::Child`, so
/// on its own it would belong below. It stays above because the parent does
/// not take it on trust: [`verify_root_view`] refuses unless
/// `/proc/<pid>/status` shows `PPid:` equal to the supervisor this core
/// spawned **and** an `NSpid:` line of exactly two fields whose second is
/// `1`, and the post-`execve` check refuses unless `/proc/<pid>/exe` names
/// the in-realm shim path. A decoy pid -- a second child the helper forked
/// into an unmounted namespace and reported instead of the real one -- fails
/// all three. What is journaled is therefore a pid this process verified,
/// not a pid it was told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IsolationFacts {
    // --- parent-observed -------------------------------------------------
    /// `namespaces+landlock-abiN` for the rung the realm **obtained**,
    /// `namespaces-only` (rung 0, i.e. `--landlock=off`), or `none` (at
    /// `--isolation=off`). **Never a tier name**: `intra-user` is *defined*
    /// as namespaces plus Landlock plus seccomp, and the filter is #188's.
    ///
    /// The rung in it is the one PID 1 reported, never the one the session
    /// asked for -- otherwise a ladder that fell from rung 9 to rung 1 would
    /// journal the same string as one that did not. `landlock.requested`
    /// carries the ask.
    pub applied_profile: &'static str,
    /// The namespace kinds whose `/proc/<pid>/ns/*` inode the core read and
    /// found different from its own. Empty at `--isolation=off`.
    pub namespaces_verified: Vec<&'static str>,
    /// Whether `stat("/proc/<shim>/root")`'s `st_dev` differed from `/`'s.
    pub root_dev_differs: Option<bool>,
    /// How many canary paths were probed through `/proc/<shim>/root`, and
    /// whether every one of them was unreachable. The count is journaled so a
    /// shrinking canary list is visible in the log rather than only in a test.
    pub canaries_probed: usize,
    pub canaries_unreachable: Option<bool>,
    /// Whether `/proc/<pid>/setgroups` reads back `deny`, read by the parent.
    pub setgroups_denied: Option<bool>,
    /// **How many of the operator's supplementary groups the realm still
    /// holds**, read by the parent from `/proc/<pid>/status`.
    ///
    /// This field exists because a published residual that lives only in
    /// prose is a residual nobody rediscovers. The design called for
    /// `setgroups(0, NULL)` in the child, and that call is impossible for an
    /// unprivileged realm in either window -- `userns_may_setgroups` wants a
    /// non-empty `gid_map`, and `new_idmap_permitted` will not admit an
    /// unprivileged single-id `gid_map` until `setgroups=deny` has already
    /// cleared the flag that predicate reads. Measured, not read: `EPERM`
    /// both times, and `getgroups()` inside the finished realm still reports
    /// the operator's entries.
    ///
    /// So `setgroups_denied` being `true` does **not** mean the groups were
    /// dropped; it means the *call* is blocked. This number is what was
    /// actually retained, and it is journaled on every confined spawn so an
    /// auditor reading one entry can see the gap without reading `limits.md`.
    pub supplementary_groups_retained: Option<usize>,
    /// The id maps exactly as the kernel rendered them back to the core.
    pub uid_map: Option<String>,
    pub gid_map: Option<String>,
    pub realm_uid: u32,
    pub realm_gid: u32,
    pub host_uid: u32,
    /// The process the core's `Child` names. At `--isolation=default` this is
    /// the **supervisor**, not the shim.
    pub supervisor_pid: u32,
    /// The shim's host pid: PID 1 inside the realm, some other number here.
    /// Reported by the helper, then **verified** by the parent -- see this
    /// type's own docs for the three checks that make it a parent fact.
    pub shim_host_pid: Option<i32>,
    /// How long the handshake took, so the 250 ms margin is observable rather
    /// than assumed.
    pub handshake_ms: u64,
    /// The realm's entire writable set, **measured** from
    /// `/proc/<shim>/mountinfo` by this process on this spawn.
    ///
    /// It was a hardcoded sentence until an adversarial review pointed out
    /// what that meant: a fixed `&'static str` describing a mount table
    /// nobody had looked at, printed under `parent_observed`. The parent has
    /// permission for the read (the confined child is its own descendant and
    /// the same kuid), so there is no reason for the field to be a claim
    /// rather than a measurement.
    pub writable: WritableSet,
    /// Where the realm's `stdout`/`stderr` went.
    pub stdio: &'static str,
    /// Whether this realm's private storage already existed from an earlier
    /// run. Warned and journaled, never refused: refusing would make a
    /// routine `/usr/bin` -> `/usr/local/bin` move a hard failure for no
    /// security gain, since the data was always this operator's.
    pub storage_reused: bool,
    /// The session's `--landlock` selection, which is a **parent** fact: it
    /// is what this core asked for, not what the realm got. `None` at
    /// `--isolation=off`, where nothing was asked for at all.
    pub landlock_requested: Option<LandlockRequest>,
    /// The kernel reported an ABI **above this build's own ladder**, so the
    /// request was cut down to `LANDLOCK_BUILD_MAX_RUNG` by the build rather
    /// than by the kernel or the operator (P2.6.3, #187).
    ///
    /// Derived here, from `vitrin_realm_init::plan_rung`, over the ABI the
    /// child reported and this build's own constant -- so it is a *parent*
    /// conclusion about a child-asserted number, which is why it sits in this
    /// half of the struct. `None` when nothing measured the ABI, which is
    /// only `--landlock=off` and `--isolation=off`.
    ///
    /// It is reported rather than merely computed: a build confining a newer
    /// kernel at an older rung has a confinement claim one rung narrower than
    /// its kernel would allow, and `vitrind --print-floor` prints the
    /// constant it was measured against.
    pub landlock_clamped_by_build: Option<bool>,
    // --- child-asserted --------------------------------------------------
    /// What the PID-1 child's own post-pivot `/proc/self/mountinfo` said.
    pub mount_count: Option<u32>,
    pub mount_fingerprint: Option<u64>,
    /// The ABI rung the ruleset was actually built at, as reported by the
    /// child that built it. `Some(0)` means "the session asked for none".
    ///
    /// Journaled beside [`Self::landlock_kernel_abi`] rather than alone,
    /// because one number cannot distinguish a session pinned low by
    /// `--landlock=abi:2` from a session on a kernel that offers no more. Two
    /// numbers can, and an auditor should not have to read the command line
    /// to tell those apart.
    pub landlock_rung: Option<u32>,
    /// What `landlock_create_ruleset(NULL, 0,
    /// LANDLOCK_CREATE_RULESET_VERSION)` answered *inside the realm*.
    pub landlock_kernel_abi: Option<u32>,
}

impl IsolationFacts {
    /// The `--isolation=off` shape: nothing was verified because nothing was
    /// applied, and every parent-observed field is `None` rather than a
    /// hopeful default.
    fn unconfined(pid: u32) -> IsolationFacts {
        IsolationFacts {
            applied_profile: "none",
            namespaces_verified: Vec::new(),
            root_dev_differs: None,
            canaries_probed: 0,
            canaries_unreachable: None,
            setgroups_denied: None,
            supplementary_groups_retained: None,
            uid_map: None,
            gid_map: None,
            realm_uid: rustix::process::geteuid().as_raw(),
            realm_gid: rustix::process::getegid().as_raw(),
            host_uid: rustix::process::geteuid().as_raw(),
            supervisor_pid: pid,
            shim_host_pid: None,
            handshake_ms: 0,
            writable: WritableSet::Unconfined,
            stdio: "inherited",
            storage_reused: false,
            landlock_requested: None,
            landlock_clamped_by_build: None,
            mount_count: None,
            mount_fingerprint: None,
            landlock_rung: None,
            landlock_kernel_abi: None,
        }
    }

    /// The algorithm name printed beside [`Self::mount_fingerprint`], so a
    /// reader never has to guess which family the number is from -- and, in
    /// particular, never reads it as one of the recorder's blake3 digests.
    pub fn fingerprint_alg(&self) -> &'static str {
        MOUNT_FINGERPRINT_ALG
    }
}

#[derive(Debug)]
pub(crate) struct SpawnedRealm {
    realm_id: RealmId,
    /// The shim process, reaped on drop if the realm is never adopted
    /// ([`GuardedChild`]).
    ///
    /// At `--isolation=default` this is the **supervisor**, which is the
    /// realm's process from the core's point of view: it is what the
    /// termination ladder signals, what `waitpid` reports on, and what
    /// PDEATHSIG hangs the shim's life off.
    child: GuardedChild,
    runtime_dir: PathBuf,
    connection: Connection,
    /// What the journal is allowed to say about this realm's confinement.
    isolation: IsolationFacts,
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

    /// The realm's process id.
    ///
    /// At `--isolation=off` this is the shim. At `--isolation=default` it is
    /// the **supervisor**, and the shim's own host pid is
    /// [`IsolationFacts::shim_host_pid`]. The distinction matters to anything
    /// walking `/proc/<pid>/task/*/children`: the confined tree is
    /// `vitrind -> supervisor -> shim (PID 1) -> app`, one level deeper.
    pub fn pid(&self) -> u32 {
        self.child.get().id()
    }

    /// What the journal may say about this realm's confinement.
    pub fn isolation(&self) -> &IsolationFacts {
        &self.isolation
    }

    /// The realm's private runtime directory (mode `0700`), which the shim
    /// binds its app-facing socket inside.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// The core's end of the identity socketpair.
    ///
    /// Test-only: the runtime never touches the connection here. It sends
    /// `configure` through [`Self::start_shim_session`] and then moves the
    /// whole realm on with [`Self::into_parts`], so the connection reaches
    /// the event loop by ownership rather than by borrow — which is what
    /// keeps exactly one core-side descriptor in existence.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// The process handle, unreaped. Exposed so this module's tests can
    /// terminate and wait deterministically; this module implements no
    /// lifecycle policy of its own, and at runtime the handle goes to
    /// `RealmLifecycle` through [`Self::into_parts`] instead.
    #[cfg_attr(not(test), allow(dead_code))]
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
    /// The **confined** spawn path refused (P2.6.2, #186). Every one of these
    /// is reachable only at `--isolation=default`.
    ///
    /// One variant carrying a closed-vocabulary discriminant rather than
    /// seventeen sibling variants: the property that matters is that
    /// [`SpawnError::cause_class`] draws from a fixed set a reader can switch
    /// on, and [`ConfinementFault`] *is* that set, exhaustively matched in
    /// one place. Seventeen variants would spread the same mapping across
    /// seventeen arms of three different `match`es.
    Confinement {
        class: ConfinementFault,
        detail: String,
    },
}

/// Why a confined spawn was refused, as a closed vocabulary.
///
/// Each value maps to exactly one `cause_class` token, and the mapping is one
/// exhaustive `match`, so a new fault cannot be added without being given a
/// label the flight recorder can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfinementFault {
    /// `vitrin-realm-init` failed the trusted-writer audit. Named apart from
    /// [`SpawnError::ProgramAudit`] because the remedy differs: this is the
    /// core's own sibling binary, not the operator's app.
    RealmInitAudit,
    /// The realm's private storage directory could not be prepared.
    StorageDir,
    /// The helper's `schema_version` is not this core's. A `--shim` from
    /// `/usr/lib/vitrin` beside a helper from `target/debug`.
    HelperVersion,
    /// The helper said something the protocol has no reading for.
    HelperProtocol,
    /// The 250 ms handshake deadline expired.
    HelperTimeout,
    /// The helper exited or closed the channel before the handshake finished.
    HelperDied,
    /// `unshare` returned `EPERM`/`EACCES`: the kernel knows the request and
    /// something above it said no. This is R2.9 arriving at run time.
    UnshareDenied,
    /// `unshare` returned `EINVAL`/`ENOSYS`: the kernel does not implement it.
    UnshareAbsent,
    /// The core's own read of `/proc/<supervisor>/ns/*` did not show five
    /// different namespaces. **Never retried** -- a helper that reported
    /// `UNSHARED` and did not unshare is a substitution or a bug, and neither
    /// gets better on a second attempt.
    NsVerify,
    SetgroupsWrite,
    UidMapWrite,
    GidMapWrite,
    /// The maps did not read back as what the core wrote, on either side.
    MapVerify,
    /// A mount-table entry failed inside the realm.
    MountTable,
    /// `pivot_root` or its detach failed.
    PivotRoot,
    /// The core's own behavioural probe of the realm's filesystem view
    /// failed: the root device matched the host's, or a canary was reachable.
    /// **This is the check that catches a helper which unshared and mounted
    /// nothing**, and it is the only one that can.
    RootView,
    /// The shim's `execve` inside the realm failed.
    ExecShim,
    /// The Landlock ruleset could not be built, granted or enforced inside
    /// the realm (P2.6.3, #187) -- or the helper reported enforcing none on a
    /// session that asked for one.
    ///
    /// Its own class rather than folded into [`ConfinementFault::MountTable`]
    /// or the generic protocol fault, because the operator's remedy is
    /// specific: a kernel without `CONFIG_SECURITY_LANDLOCK`, or with
    /// `landlock` missing from the active LSM list, answers here and nowhere
    /// else. The startup preflight normally catches that before any realm
    /// exists; this class is what catches the case where the *realm's* view
    /// differs from the core's.
    Landlock,
}

impl ConfinementFault {
    fn cause_class(self) -> &'static str {
        match self {
            ConfinementFault::RealmInitAudit => "realm_init_audit",
            ConfinementFault::StorageDir => "storage_dir",
            ConfinementFault::HelperVersion => "helper_version",
            ConfinementFault::HelperProtocol => "helper_protocol",
            ConfinementFault::HelperTimeout => "helper_timeout",
            ConfinementFault::HelperDied => "helper_died",
            ConfinementFault::UnshareDenied => "unshare_denied",
            ConfinementFault::UnshareAbsent => "unshare_absent",
            ConfinementFault::NsVerify => "ns_verify",
            ConfinementFault::SetgroupsWrite => "setgroups_write",
            ConfinementFault::UidMapWrite => "uid_map_write",
            ConfinementFault::GidMapWrite => "gid_map_write",
            ConfinementFault::MapVerify => "map_verify",
            ConfinementFault::MountTable => "mount_table",
            ConfinementFault::PivotRoot => "pivot_root",
            ConfinementFault::RootView => "root_view",
            ConfinementFault::ExecShim => "exec_shim",
            ConfinementFault::Landlock => "landlock",
        }
    }
}

/// Build a confinement refusal. A free function rather than a constructor on
/// the enum so call sites read as one line.
fn refuse(class: ConfinementFault, detail: impl Into<String>) -> SpawnError {
    SpawnError::Confinement {
        class,
        detail: detail.into(),
    }
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
            SpawnError::Confinement { class, .. } => class.cause_class(),
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
            SpawnError::Confinement { class, detail } => write!(
                f,
                "refusing to spawn a realm that is not confined ({}): {detail}. \
                 A half-confined child is worse than no child, so this is a refusal and \
                 nothing was left behind",
                class.cause_class()
            ),
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
    origin: SpawnOrigin<'_>,
) -> Result<SpawnedRealm, SpawnError> {
    spawn_realm_with_env(realm, paths, recorder, origin, |name| {
        std::env::var(name).ok()
    })
}

/// **Why the trusted core is forking** -- a required argument of every
/// spawn, and a required field of both of the flight recorder's spawn
/// entries (WS-E.1.1, issue #207).
///
/// Until `realm_launch` was served there was one answer and it did not have
/// to be written down: startup read a file the operator had hardened, and
/// nothing reachable from the wire could make `vitrind` create a process.
/// That property is gone. What replaces it is consent, a cap, a token
/// bucket, revocation and this -- and "the journal names who asked" is only
/// true if the asker cannot be omitted, so it is a parameter with no
/// default rather than an `Option` a caller may leave `None`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SpawnOrigin<'a> {
    /// Session startup, from `realm.toml`. No principal and no grant: the
    /// authority was the operator's write access to the file, which the
    /// loader audited.
    Startup,
    /// An admitted `vitrin_launcher.launch` -- the verifier-canonical
    /// identity bound at `hello`, and the grant row the chokepoint judged
    /// the use against.
    Launch {
        principal: &'a PrincipalIdentity,
        grant: GrantId,
    },
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
    origin: SpawnOrigin<'_>,
    lookup: F,
) -> Result<SpawnedRealm, SpawnError>
where
    F: Fn(&str) -> Option<String>,
{
    let (result, record) = spawn_realm_deferring_journal(realm, paths, origin, lookup);
    record.journal(recorder);
    result
}

/// [`spawn_realm_with_env`] for the one caller that **cannot lend a
/// `&mut Recorder` at the moment it forks**: the enforcement chokepoint's
/// launch sink (WS-E.1.1, issue #207).
///
/// A principal-connection dispatch already holds the recorder mutably
/// through its `ServerCtx`, so a sink that also took one would alias the
/// same field. The alternative -- handing the chokepoint a recorder -- is
/// the thing [`crate::enforcement`] is written not to do: the recorder
/// observes the chokepoint through its return value and never appears
/// inside it.
///
/// **The log is still inescapable**, which is the property the recorder
/// parameter above exists for. The `Result` and the [`SpawnRecord`] come
/// out together, the record is `#[must_use]`, and `journal` is the only
/// thing that consumes it -- so a caller can drop the obligation only by
/// writing a line the compiler warns about. A future error path added
/// inside [`launch`] is still covered structurally, because the record is
/// built from the result rather than at each return.
pub(crate) fn spawn_realm_deferring_journal<F>(
    realm: &Realm,
    paths: &SpawnPaths,
    origin: SpawnOrigin<'_>,
    lookup: F,
) -> (Result<SpawnedRealm, SpawnError>, SpawnRecord)
where
    F: Fn(&str) -> Option<String>,
{
    let result = launch(realm, paths, lookup);
    let record = SpawnRecord {
        realm: realm.id().clone(),
        command: realm.spawn().command().to_path_buf(),
        env_allow: realm.spawn().env_allow().to_vec(),
        origin: match origin {
            SpawnOrigin::Startup => OwnedSpawnOrigin::Startup,
            SpawnOrigin::Launch { principal, grant } => OwnedSpawnOrigin::Launch {
                principal: principal.clone(),
                grant,
            },
        },
        outcome: match &result {
            Ok(spawned) => Ok((
                spawned.pid(),
                spawned.runtime_dir().to_path_buf(),
                spawned.isolation().clone(),
            )),
            Err(err) => Err(err.cause_class()),
        },
        journaled: false,
    };
    (result, record)
}

/// [`SpawnOrigin`] owned, for the moment a [`SpawnRecord`] outlives the
/// borrow its spawn was made under.
#[derive(Debug, Clone)]
enum OwnedSpawnOrigin {
    Startup,
    Launch {
        principal: PrincipalIdentity,
        grant: GrantId,
    },
}

/// **The journal entry one spawn owes**, when the spawn happened somewhere
/// the recorder could not be borrowed. See
/// [`spawn_realm_deferring_journal`].
/// # Why a `Drop` guard and not only `#[must_use]`
///
/// `#[must_use]` is kept, but it is the weaker half and it is why this hole
/// existed: the attribute fires on an *expression* whose value is discarded,
/// and says nothing at all once the value is moved into a struct field inside
/// a `Vec`. That is exactly where this record lives (`session::PendingLaunch`),
/// and a dispatch turn that ended on a transport fault returned before the
/// launches were applied, dropping the whole vector -- forked processes,
/// unjournaled, with the compiler perfectly happy. Issue #207's review found
/// it; the doc on this type had claimed the log was "inescapable" and it was
/// not.
///
/// [`Drop`] closes it for good, because a drop is the one thing every escape
/// path has in common. Debug builds abort on the spot; release builds log at
/// `error` and carry on, because losing a journal line is bad but killing a
/// live session over it is worse.
#[must_use = "a spawn that is never journaled is exactly the gap the recorder parameter on \
              spawn_realm_with_env exists to close: what the trusted core executed is the \
              most security-relevant act of a session"]
pub(crate) struct SpawnRecord {
    realm: RealmId,
    command: PathBuf,
    env_allow: Vec<String>,
    origin: OwnedSpawnOrigin,
    /// `Ok((pid, runtime_dir, isolation))` or `Err(cause_class)` -- exactly
    /// what the two entries below need, and nothing that could be used to
    /// reconstruct a spawn that did not happen.
    ///
    /// The confinement facts ride here rather than being re-read at journal
    /// time, and that is not an optimisation: they were read *while the
    /// handshake held the child*, and re-reading `/proc` afterwards would be
    /// asking a question about a different instant.
    outcome: Result<(u32, PathBuf, IsolationFacts), &'static str>,
    /// Set by [`Self::journal`] just before the entry is written, and read
    /// only by [`Drop`]. The obligation is discharged, not merely intended.
    journaled: bool,
}

impl Drop for SpawnRecord {
    fn drop(&mut self) {
        if self.journaled {
            return;
        }
        // A forked process with no journal entry is the one outcome the
        // launch verb's whole accountability story rules out: "who started
        // this app" must always have an answer.
        tracing::error!(
            realm = %self.realm,
            command = %self.command.display(),
            "a spawn record was dropped without being journaled -- the flight recorder \
             cannot say who started this realm"
        );
        debug_assert!(
            false,
            "a spawn was dropped without being journaled ({}): every escape path from a \
             fork must write its entry",
            self.realm
        );
    }
}

impl SpawnRecord {
    /// Write this spawn's entry. Consumes the record, so it cannot be
    /// written twice and cannot be kept for later.
    pub fn journal(mut self, recorder: &mut Recorder) {
        // Before the write, so the `Drop` guard below cannot fire for a record
        // that is in the middle of discharging its obligation.
        self.journaled = true;
        let origin = match &self.origin {
            OwnedSpawnOrigin::Startup => SpawnOrigin::Startup,
            OwnedSpawnOrigin::Launch { principal, grant } => SpawnOrigin::Launch {
                principal,
                grant: *grant,
            },
        };
        match &self.outcome {
            Ok((pid, runtime_dir, isolation)) => recorder.record(Event::RealmSpawned {
                realm: &self.realm,
                pid: *pid,
                origin,
                command: &self.command,
                runtime_dir,
                env_allow: &self.env_allow,
                isolation,
            }),
            Err(cause_class) => recorder.record(Event::RealmSpawnFailed {
                realm: &self.realm,
                origin,
                command: &self.command,
                cause_class,
            }),
        }
    }
}

/// The spawn itself. Separated from the journaling wrapper above so no
/// return path can escape the log.
///
/// **The one branch, and why it is only one.** `--isolation=off` runs
/// [`launch_unconfined`], which is byte for byte the path that shipped before
/// #186; `--isolation=default` runs [`launch_confined`]. Two spawn paths
/// inside the TCB is a real cost and is stated as one: it is exactly how a
/// confinement claim rots, and the only thing that keeps `default` from
/// silently degrading into `off` is that the confined arm proves its
/// confinement from outside before it commits.
fn launch<F>(realm: &Realm, paths: &SpawnPaths, lookup: F) -> Result<SpawnedRealm, SpawnError>
where
    F: Fn(&str) -> Option<String>,
{
    match &paths.confinement {
        None => launch_unconfined(realm, paths, lookup),
        Some(confinement) => launch_confined(realm, paths, confinement, lookup),
    }
}

fn launch_unconfined<F>(
    realm: &Realm,
    paths: &SpawnPaths,
    lookup: F,
) -> Result<SpawnedRealm, SpawnError>
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
    // the guard below at all. Both the shim (the binary the core actually
    // execs) and the app (`command`, which the shim will exec) pass the same
    // transitive trusted-writer audit -- whoever can write either chooses
    // what the trusted core, or its shim, runs (issue #103).
    reject_reserved_env(spawn)?;
    audit_program_at_spawn(Path::new(paths.shim_program()))?;
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

    // `HOME` joined `RESERVED_ENV` at #186 because a confined realm's
    // filesystem does not contain the operator's home directory. That is a
    // load-time rule and it cannot be conditional on a CLI flag -- a
    // `realm.toml` whose validity depended on `--isolation` would load on one
    // invocation and fail on the next. But the *consequence* had leaked onto
    // this path: with the name reserved and no injection here, an
    // `--isolation=off` realm got no `HOME` at all, where before #186 an
    // allow-listing config gave it the operator's own. `off` is supposed to
    // be the path that shipped before #186 and the acceptance gate's positive
    // control; a variable that is present in one arm and missing in the other
    // is neither.
    //
    // So the core decides `HOME` in **both** arms, which is what putting a
    // name in `RESERVED_ENV` means: here it decides on the operator's own,
    // because at `--isolation=off` that directory is exactly as reachable as
    // it always was; on the confined path it decides on the realm's private
    // storage.
    let host_home = lookup("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let env = child_env(
        spawn,
        &socket_path,
        &runtime_dir,
        host_home.as_deref(),
        lookup,
    );

    // The core execs the SHIM, never the app directly: the shim holds fd 3
    // and, in turn, execs the app. The app command (`command` + `args`)
    // rides the shim's argv after a `--` separator, where a conformant shim
    // reads it (PRD Doc 2 §4.1; issue #103). Any leading shim arguments come
    // before the `--`; production supplies none.
    let mut cmd = Command::new(paths.shim_program());
    cmd.args(paths.shim_leading_args());
    cmd.arg("--");
    cmd.arg(spawn.command());
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
        // The shim is what the core execs, so an exec failure is the shim's:
        // the app is exec'd later by the shim, out of this module's reach.
        command: Path::new(paths.shim_program()).to_path_buf(),
        source,
    })?;

    // The parent's copy of the child's end closes here: from now on exactly
    // one process other than the core holds it, which is what makes "holding
    // this descriptor is being realm N's shim" true rather than aspirational.
    drop(shim_fd);

    let isolation = IsolationFacts::unconfined(child.id());
    Ok(SpawnedRealm {
        realm_id: realm.id().clone(),
        child: GuardedChild(Some(child)),
        runtime_dir,
        connection: core_side,
        isolation,
        // The realm is now live, and the lock that proved it was free
        // becomes the lock that says it is taken -- held until this struct
        // drops (or the process dies, which the kernel handles).
        _realm_lock: guard.keep(),
    })
}

// ---------------------------------------------------------------------------
// The confined spawn path (P2.6.2, #186, D-036)
// ---------------------------------------------------------------------------

/// P0-P23: the whole confined bring-up.
///
/// The shape, so the body below reads as a sequence rather than as a wall:
///
/// - **P0-P11, parent, before the fork.** Audit three programs and every bind
///   source, take the realm lock, prepare private storage, open the per-realm
///   log, make two socketpairs, compose the config blob, and `spawn` the
///   helper with the **existing** `pre_exec` closure -- unchanged, because
///   the config channel is stdin and needs no second `dup3`.
/// - **P12-P18, the map protocol.** Wait for `UNSHARED`, read the
///   supervisor's namespace inodes, write `setgroups`/`uid_map`/`gid_map`,
///   read all three back, and only then release the helper.
/// - **P19-P21, the checkpoint that licenses the whole inversion.** Wait for
///   `CHILD` and `MOUNTED`, then probe the realm's filesystem *from out here*
///   through `/proc/<shim>/root`. This is the only step that catches a helper
///   which unshared six namespaces and mounted nothing.
/// - **P22-P23.** EOF means the shim is running; anything else means it is
///   not.
///
/// Every failure is total. The [`RuntimeDirGuard`] removes the directory,
/// [`GuardedChild`] kills and reaps the helper, and the private storage
/// deliberately survives -- it is the realm's data, not this attempt's.
fn launch_confined<F>(
    realm: &Realm,
    paths: &SpawnPaths,
    conf: &Confinement,
    lookup: F,
) -> Result<SpawnedRealm, SpawnError>
where
    F: Fn(&str) -> Option<String>,
{
    let realm_id = realm.id().as_str();
    let runtime_dir = paths.realm_dir(realm_id)?;
    let lock_path = paths.realm_lock(realm_id)?;
    let spawn = realm.spawn();

    // P0/P1. Same preconditions as the unconfined path plus one program:
    // three binaries now stand between the operator and the app, and whoever
    // can write any of them chooses what the trusted core runs.
    reject_reserved_env(spawn)?;
    audit_program_at_spawn(&conf.realm_init).map_err(|e| {
        refuse(
            ConfinementFault::RealmInitAudit,
            format!("{e}; this is the core's own confinement helper"),
        )
    })?;
    audit_program_at_spawn(Path::new(paths.shim_program()))?;
    audit_program_at_spawn(spawn.command())?;

    // P2. Bind sources are audited exactly as `command` is, and for exactly
    // the same reason. Since the owner's decision put them in `realm.toml`
    // beside the app, that file now carries confinement-relevant
    // configuration -- so whoever can write a bind source, or any directory
    // on its path, chooses part of what the realm can read.
    let mut binds = Vec::new();
    for source in spawn.binds() {
        binds.push(audit_bind_source_at_spawn(source)?);
    }

    // The app has to exist *inside* the realm, which the operator's spelling
    // cannot promise: `/usr` and `/etc` are bound at their own paths, so an
    // app under either is already covered, and anything else needs its
    // directory bound.
    let app = fs::canonicalize(spawn.command()).map_err(|e| SpawnError::ProgramAudit {
        path: spawn.command().to_path_buf(),
        detail: format!("does not resolve to a program ({e})"),
    })?;
    let app_dir = app_dir_to_bind(&app, &binds, lookup("HOME"))?;

    // P3. Unchanged.
    let guard = RuntimeDirGuard::create(&runtime_dir, &lock_path, realm_id)?;

    // P4. Private storage, through the same verified-parent chain as the
    // runtime tree -- never `create_dir_all` + `set_permissions`, which is
    // the symlink-following bug this module already records.
    let (storage_dir, storage_reused) = prepare_realm_storage(&conf.storage_root, realm_id)?;
    if storage_reused {
        tracing::warn!(
            realm = realm_id,
            storage = %storage_dir.display(),
            command = %app.display(),
            "this realm's private storage already existed and is being reused as the app's \
             HOME. Storage is keyed on realm id and never purged, so if this realm's `command` \
             changed, the new app inherits the old app's home directory. Not refused: the data \
             was always this operator's, and refusing would make a routine binary move a hard \
             failure. It also has NO QUOTA -- filling this partition is a host denial of \
             service with no gate in front of it"
        );
    }

    // P5. The realm's diagnostics stop being the operator's terminal.
    //
    // `close_range(3, ...)` starts at 3 by construction, so fds 1 and 2 cross
    // both `execve`s untouched -- and on a bare-DRM session they are open
    // descriptors on `/dev/ttyN`. **A mount table has no say over a device
    // that arrived as a descriptor**, so the `/dev` closure could not be
    // claimed honestly with them inherited.
    let log_path = runtime_dir.join(REALM_LOG_NAME);
    let log = fs::File::create(&log_path).map_err(|e| SpawnError::RuntimeDir {
        path: log_path.clone(),
        detail: format!("cannot open the realm's log file: {e}"),
    })?;
    let log_err = log.try_clone().map_err(|e| SpawnError::RuntimeDir {
        path: log_path.clone(),
        detail: format!("cannot duplicate the realm's log file: {e}"),
    })?;
    tracing::info!(
        realm = realm_id,
        log = %log_path.display(),
        "the realm's stdout and stderr go to this file, not to this terminal: at \
         --isolation=default they would otherwise be inherited descriptors on the operator's \
         tty, which no mount flag can revoke. The core tails it if the realm dies during \
         bring-up"
    );

    // P6. Unchanged from the unconfined path.
    let (core_side, shim_side) = Connection::pair().map_err(SpawnError::Socketpair)?;
    let shim_fd: OwnedFd = rustix::io::fcntl_dupfd_cloexec(shim_side.as_fd(), SHIM_CORE_FD)
        .map_err(|e| SpawnError::Socketpair(e.into()))?;
    drop(shim_side);
    let shim_raw = shim_fd.as_raw_fd();

    // P7. The config channel. `SOCK_SEQPACKET`, so a frame is a datagram.
    let (cfg_core, cfg_child) = rustix::net::socketpair(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::SEQPACKET,
        rustix::net::SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|e| SpawnError::Socketpair(e.into()))?;

    // P8. The blob, built where allocating is free of consequence. Every path
    // in it is one *this* function canonicalized and audited; the helper
    // resolves nothing.
    let euid = rustix::process::geteuid().as_raw();
    let egid = rustix::process::getegid().as_raw();
    let mut argv: Vec<OsString> = vec![OsString::from(IN_REALM_SHIM)];
    argv.extend(paths.shim_leading_args().iter().cloned());
    argv.push(OsString::from("--"));
    // The **canonical** app path, not the operator's spelling. The operator's
    // spelling may route through a symlink the realm's tree does not
    // reproduce, and a realm whose app fails to exec because of a symlink
    // would be a confinement bug wearing an ENOENT. `argv[0]` is observable
    // to the program, so this is a real (small) difference from the
    // unconfined path and is stated rather than hidden.
    argv.push(app.clone().into_os_string());
    argv.extend(spawn.args().iter().map(OsString::from));

    let config = RealmInitConfig {
        schema_version: env!("CARGO_PKG_VERSION").to_string(),
        realm_id: realm_id.to_string(),
        inner_uid: euid,
        inner_gid: egid,
        runtime_dir: runtime_dir.clone(),
        storage_dir: storage_dir.clone(),
        shim_source: fs::canonicalize(paths.shim_program()).map_err(|e| {
            SpawnError::ProgramAudit {
                path: PathBuf::from(paths.shim_program()),
                detail: format!("does not resolve to a program ({e})"),
            }
        })?,
        app_dir,
        render_nodes: conf.render_nodes.clone(),
        binds,
        argv,
        caps: conf.tmpfs,
        landlock: conf.landlock,
    };
    let blob = Frame::Config(Box::new(config)).encode().map_err(|e| {
        refuse(
            ConfinementFault::HelperProtocol,
            format!("cannot encode the realm's confinement description: {e}"),
        )
    })?;

    // P9. The helper, not the shim, is what the core execs now.
    //
    // Read before `child_env` consumes `lookup`. See [`landlock_audit_env`]
    // for why the diagnostic is added here and not inside that function.
    let landlock_audit = landlock_audit_env(&lookup);
    let mut cmd = Command::new(&conf.realm_init);
    cmd.env_clear();
    cmd.envs(
        child_env(
            spawn,
            Path::new(IN_REALM_WAYLAND_SOCKET),
            Path::new(IN_REALM_RUNTIME_DIR),
            Some(Path::new(IN_REALM_HOME)),
            lookup,
        )
        .iter()
        .map(|(k, v)| (k.as_os_str(), v.as_os_str())),
    );
    if let Some((name, value)) = &landlock_audit {
        cmd.env(name, value);
    }
    cmd.current_dir(&runtime_dir);
    cmd.stdin(Stdio::from(cfg_child));
    cmd.stdout(Stdio::from(log));
    cmd.stderr(Stdio::from(log_err));

    // P10. The existing closure, verbatim. It is the single reason the config
    // channel is stdin: with no second fixed descriptor to place there is no
    // second `dup3`, and nothing here can collide with std's exec-report pipe.
    let sig_max = libc::SIGRTMAX();
    // SAFETY: identical to the unconfined path's closure -- see its comment
    // for the full argument. Nothing about confinement changes it.
    unsafe {
        cmd.pre_exec(move || {
            if libc::close_range(
                SHIM_CORE_FD as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut sig = 1;
            while sig <= sig_max {
                if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                    libc::signal(sig, libc::SIG_DFL);
                }
                sig += 1;
            }
            let mut empty = core::mem::MaybeUninit::<libc::sigset_t>::uninit();
            if libc::sigemptyset(empty.as_mut_ptr()) < 0
                || libc::sigprocmask(libc::SIG_SETMASK, empty.as_ptr(), core::ptr::null_mut()) < 0
            {
                return Err(io::Error::last_os_error());
            }
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

    // P11.
    let child = cmd.spawn().map_err(|source| SpawnError::Exec {
        command: conf.realm_init.clone(),
        source,
    })?;
    drop(shim_fd);
    // **The `Command` has to die here, and this is not tidiness either.**
    // `Stdio::from(cfg_child)` moved the child's end of the config channel
    // into `cmd`, which holds it until `cmd` drops -- so a `cmd` still alive
    // at the end of this function is a third holder of a socket whose closure
    // is how the core learns the shim `execve`'d. Measured, after exactly
    // that: the handshake reached `PROCEED` in 2 ms and then waited out its
    // whole 250 ms deadline on a realm that had started perfectly.
    //
    // Two holders were found this way, the other being the helper's own fd 0.
    // The pattern is worth stating once: **EOF-as-a-signal is only as good as
    // the census of who holds the descriptor**, and `Stdio` is easy to forget
    // because it never appears as a variable of its own.
    drop(cmd);
    // **The clock starts here, at P11, and not at the top of this function.**
    // Everything above -- three transitive `audit_program_at_spawn` walks, a
    // `canonicalize` per bind source, `RuntimeDirGuard::create` (which takes
    // an `flock` and may recursively delete a stale realm directory),
    // `prepare_realm_storage`, the log `create`, two socketpairs and the blob
    // encode -- is this core's own work and has nothing to do with whether
    // the helper is answering. Charging it to the handshake budget would
    // refuse perfectly good spawns on a cold or loaded filesystem, and would
    // do it while the journal recorded a `handshake_ms` of 3.
    let deadline = Instant::now() + HELPER_DEADLINE;
    // Armed immediately: from here every error path must kill and reap the
    // helper, and the guard is what makes that true on paths added later.
    let mut child = GuardedChild(Some(child));
    let supervisor_pid = child.get().id();

    let handshake = handshake(
        cfg_core.as_fd(),
        &blob,
        supervisor_pid,
        conf,
        euid,
        egid,
        deadline,
    );
    let handshake = match handshake {
        Ok(handshake) => handshake,
        Err(err) => {
            // The refusal must leave no process: `GuardedChild::drop` kills
            // and reaps the supervisor, PDEATHSIG takes PID 1 with it, and the
            // pid namespace dying takes the app.
            return Err(tail_realm_log(&log_path, realm_id, err));
        }
    };

    // Every one of these was a literal `Some(true)` until an adversarial
    // review deleted the checks behind them and watched the suite stay green.
    // They now carry what the check *returned*: a boolean that cannot be
    // produced without running the comparison is a boolean a deleted
    // comparison cannot forge.
    // **What the ladder actually landed on, compared with what was asked for
    // and with what this kernel offers** (P2.6.3, #187). Three numbers, and
    // the comparison between them is the thing #187 forbids masking: a realm
    // that fell from rung 9 to rung 1 has no `TRUNCATE`, no `IOCTL_DEV` and
    // no scoping, and must be distinguishable without diffing a per-realm
    // JSON blob.
    //
    // The plan is recomputed through the SAME pure function the helper
    // planned with (`vitrin_realm_init::plan_rung`), rather than by a second
    // copy of the arithmetic here: two opinions about one session is exactly
    // the shape this warning exists to catch.
    let plan = vitrin_realm_init::plan_rung(handshake.landlock_kernel_abi, conf.landlock);
    warn_on_landlock_shortfall(
        realm_id,
        conf.landlock,
        handshake.landlock_rung,
        handshake.landlock_kernel_abi,
        plan,
    );

    let isolation = IsolationFacts {
        // **The rung OBTAINED, never the rung requested.** The field is named
        // `applied_profile`; deriving it from `conf.landlock` rendered
        // `--landlock=abi:9` on an ABI-3 kernel as `namespaces+landlock-abi9`
        // and rendered `--landlock=highest` identically at rung 1 and rung 9.
        // The request is journaled separately as `landlock.requested`.
        applied_profile: isolation::profile_for(Isolation::Default, handshake.landlock_rung),
        namespaces_verified: handshake.namespaces_verified,
        root_dev_differs: Some(handshake.root_view.root_dev_differs),
        canaries_probed: handshake.root_view.canaries_probed,
        canaries_unreachable: Some(handshake.root_view.canaries_unreachable),
        setgroups_denied: Some(handshake.setgroups_denied),
        supplementary_groups_retained: handshake.supplementary_groups,
        uid_map: Some(handshake.uid_map),
        gid_map: Some(handshake.gid_map),
        realm_uid: euid,
        realm_gid: egid,
        host_uid: euid,
        supervisor_pid,
        shim_host_pid: Some(handshake.shim_host_pid),
        handshake_ms: handshake.elapsed.as_millis() as u64,
        writable: handshake.root_view.writable,
        stdio: "per-realm log file",
        storage_reused,
        landlock_requested: Some(conf.landlock),
        // `0` is the helper's "not measured", which happens on exactly one
        // path: `--landlock=off`, where nothing asked the kernel anything.
        // Rendered `null` rather than as rung 0, because 0 is not an ABI
        // version.
        landlock_clamped_by_build: (handshake.landlock_kernel_abi > 0)
            .then_some(plan.clamped_by_build),
        mount_count: Some(handshake.mount_count),
        mount_fingerprint: Some(handshake.mount_fingerprint),
        landlock_rung: Some(handshake.landlock_rung),
        landlock_kernel_abi: (handshake.landlock_kernel_abi > 0)
            .then_some(handshake.landlock_kernel_abi),
    };

    Ok(SpawnedRealm {
        realm_id: realm.id().clone(),
        child: GuardedChild(child.0.take()),
        runtime_dir,
        connection: core_side,
        isolation,
        _realm_lock: guard.keep(),
    })
}

/// The realm's `stdout`/`stderr` file, inside its own runtime directory.
const REALM_LOG_NAME: &str = "realm.log";

/// How many trailing bytes of a realm's log the core repeats into its own
/// stderr when the realm dies during bring-up.
///
/// Bounded rather than unbounded: the log is written by the confined child,
/// so it is attacker-controlled content being echoed into the operator's
/// terminal. 4 KiB is enough for a dynamic-linker error and a backtrace, and
/// small enough that a realm cannot flood the terminal by failing to start.
const REALM_LOG_TAIL: u64 = 4096;

/// "A shim that cannot say why it failed to start is undebuggable" -- the
/// objection the module docs raise against redirecting diagnostics, answered
/// rather than accepted. On a bring-up failure the core repeats the tail of
/// the realm's log into its own stderr, so the operator sees the reason in
/// the place they were already looking.
fn tail_realm_log(path: &Path, realm_id: &str, err: SpawnError) -> SpawnError {
    let Ok(text) = fs::read(path) else {
        return err;
    };
    let from = text.len().saturating_sub(REALM_LOG_TAIL as usize);
    let tail = String::from_utf8_lossy(&text[from..]);
    let tail = sanitise_for_terminal(tail.trim_end());
    let tail = tail.trim_end();
    if tail.trim().is_empty() {
        return err;
    }
    tracing::error!(
        realm = realm_id,
        log = %path.display(),
        "the realm died during bring-up; the last {} bytes of its log follow (written by the \
         confined child, so treat the content as untrusted):\n{tail}",
        text.len() - from,
    );
    // And onto the refusal itself, not only into the log. The runtime
    // directory -- and with it this file -- is removed by `RuntimeDirGuard`
    // on the way out, so a caller that only ever sees the `SpawnError` would
    // otherwise be reading about a file that no longer exists.
    match err {
        SpawnError::Confinement { class, detail } => SpawnError::Confinement {
            class,
            detail: format!("{detail}; the realm's own last words were: {tail}"),
        },
        other => other,
    }
}

/// Strip the control characters out of realm-written text before it reaches
/// the operator's terminal.
///
/// `realm.log` lives inside the runtime directory the realm has bound
/// read-write at `/run/vitrin`, so the confined app can truncate it and write
/// whatever it likes -- and [`tail_realm_log`] echoes the last 4 KiB of it
/// into `vitrind`'s stderr. Bounded and labelled untrusted was not enough on
/// its own: an escape sequence can move the cursor, repaint earlier lines, or
/// set the title, so a realm that failed to start could forge output
/// attributed to the core that killed it.
///
/// Newlines and tabs survive, because a linker error is unreadable without
/// them and neither can reposition a cursor. Everything else below `0x20`,
/// plus `DEL` and the C1 range, becomes a visible escape. `\u{...}` rather
/// than a dot so the text stays reversible: an operator debugging a genuinely
/// binary crash dump can still see what was there.
fn sanitise_for_terminal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            c if c.is_control() || ('\u{80}'..='\u{9f}').contains(&c) => {
                out.push_str(&format!("\\u{{{:02x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// **The ladder fallback, said out loud** (P2.6.3, #187).
///
/// `create_ruleset` walks 9 -> 8 -> ... -> 1 on `EINVAL`/`E2BIG`, and the
/// core accepts any rung at or above 1. Without this, a realm that fell from
/// rung 9 to rung 1 -- no `TRUNCATE`, no `IOCTL_DEV`, no scoping, no
/// `RESOLVE_UNIX` -- was indistinguishable from a full-strength one unless
/// somebody diffed two numbers inside a per-realm JSON blob. #187's rule is
/// "never mask the fallback", and a rung that only a JSON reader can see is
/// masked in every sense that matters.
///
/// Two different shortfalls, warned separately because their causes and
/// remedies are different:
///
/// 1. **Below the request.** The helper asked for the planned rung and the
///    kernel refused it. Nobody chose this, and it is the one that means
///    something is wrong with the kernel or with the build's idea of it.
/// 2. **Below the kernel's own ABI.** The operator's `--landlock=abi:N`, or
///    this build's own ladder being shorter than the kernel's, cut it down.
///    Both are choices, and both still leave a realm confined less than this
///    machine could confine it.
fn warn_on_landlock_shortfall(
    realm_id: &str,
    requested: LandlockRequest,
    obtained: u32,
    kernel: u32,
    plan: vitrin_realm_init::Plan,
) {
    if requested == LandlockRequest::Off {
        // Rung 0 by the operator's own instruction, warned about once per
        // session at startup rather than once per realm. Nothing here is a
        // shortfall against a request.
        return;
    }
    if obtained < plan.rung {
        tracing::warn!(
            realm = realm_id,
            obtained_rung = obtained,
            requested_rung = plan.rung,
            kernel_abi = kernel,
            "LANDLOCK LADDER FELL BELOW THE REQUEST: this realm's ruleset was built at rung \
             {obtained} after the kernel refused rung {requested}, on a kernel that reports \
             ABI {kernel}. Nobody selected this. What a rung-{obtained} domain does not \
             police, that rung {requested} would: {lost}",
            requested = plan.rung,
            lost = landlock_rights_between(obtained, plan.rung),
        );
    }
    if obtained < kernel {
        let max = vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG;
        let cause = if obtained < plan.rung {
            "the ladder fell below the request; see the warning above".to_string()
        } else if requested.cap().is_some() {
            format!("the session's own `--landlock={requested}`")
        } else if plan.clamped_by_build {
            format!(
                "this BUILD's ladder stops at rung {max} (`vitrind --print-floor`, row \
                 `build.landlock_max_rung`)"
            )
        } else {
            "unaccounted for -- the request, the build's ladder and the kernel all allow more \
             than this realm obtained, which should be impossible"
                .to_string()
        };
        tracing::warn!(
            realm = realm_id,
            obtained_rung = obtained,
            kernel_abi = kernel,
            requested = %requested,
            clamped_by_build = plan.clamped_by_build,
            "LANDLOCK IS BELOW THIS KERNEL: this realm's ruleset is at rung {obtained} on a \
             kernel that offers ABI {kernel}. Cause: {cause}. What this machine could police \
             and this realm does not: {lost}",
            lost = landlock_rights_between(obtained, kernel),
        );
    }
}

/// The access rights a domain at `obtained` does not police that a domain at
/// `target` would, named rather than left as two numbers.
///
/// **Rung 2 is called out as a loosening, not a tightening**, because it is
/// one: a domain that does not handle `REFER` denies reparenting outright, so
/// rung 1 is *stricter* about `rename(2)` across directories than every rung
/// above it. A message that listed it beside `TRUNCATE` as a thing "lost"
/// would be this build describing its own confinement wrongly.
fn landlock_rights_between(obtained: u32, target: u32) -> String {
    let mut lost: Vec<&str> = Vec::new();
    for (rung, what) in [
        (
            2u32,
            "rung 2 REFER -- and this one goes the OTHER way: a rung-1 domain forbids \
             rename/link across directories entirely, even inside the realm's own writable \
             storage, so the lower rung is STRICTER here and apps that write by \
             rename-into-place (GTK, Firefox) break",
        ),
        (
            3,
            "rung 3 TRUNCATE -- without it a payload that cannot write a file can still \
             destroy it (truncate(2), creat(2), O_TRUNC are ungated)",
        ),
        (
            5,
            "rung 5 IOCTL_DEV -- without it every device node the realm can open accepts \
             every ioctl",
        ),
        (
            6,
            "rung 6 scoping -- without it the domain does not scope abstract UNIX sockets or \
             signals (the realm's namespaces still do)",
        ),
        (
            9,
            "rung 9 RESOLVE_UNIX -- without it connect(2) to pathname UNIX sockets is ungated",
        ),
    ] {
        if obtained < rung && rung <= target {
            lost.push(what);
        }
    }
    if lost.is_empty() {
        "nothing -- no access right this build requests moves between those rungs".to_string()
    } else {
        lost.join("; ")
    }
}

/// What the handshake established, all of it read by this process.
struct Handshake {
    namespaces_verified: Vec<&'static str>,
    /// Whether `/proc/<pid>/setgroups` read back `deny`. Not a constant: it
    /// is what the read-back comparison returned, and a spawn that did not
    /// perform the comparison cannot produce it.
    setgroups_denied: bool,
    supplementary_groups: Option<usize>,
    uid_map: String,
    gid_map: String,
    shim_host_pid: i32,
    /// What P20 measured, not what P20 was hoping for.
    root_view: RootView,
    mount_count: u32,
    mount_fingerprint: u64,
    /// The Landlock rung PID 1 said it enforced, and the ABI the kernel
    /// reported to it. **Child-asserted, both of them** -- there is no
    /// `/proc` file naming a process's Landlock domain, so unlike the
    /// namespace inodes the parent cannot corroborate this pair. What cannot
    /// be forged is the realm's *behaviour*, which is what
    /// `tests/integration/test_real_confinement.py` measures from inside.
    landlock_rung: u32,
    landlock_kernel_abi: u32,
    elapsed: Duration,
}

/// What the root-view checkpoint (P20) actually observed.
///
/// Returned rather than reduced to `Ok(())` on purpose: the journal's
/// `root_dev_differs` and `canaries_unreachable` used to be literal
/// `Some(true)` beside a check whose deletion nothing noticed. A value that
/// only this function can produce is a value the journal cannot state without
/// the check having run.
struct RootView {
    root_dev_differs: bool,
    canaries_probed: usize,
    canaries_unreachable: bool,
    writable: WritableSet,
}

/// The five namespace kinds the *supervisor* is checked on, plus the one the
/// PID-1 child is checked on.
///
/// `pid` is deliberately absent from the first list and present in the
/// second. `unshare(CLONE_NEWPID)` does not move the caller -- it sets
/// `pid_ns_for_children` -- so the supervisor's own `ns/pid` is still the
/// core's and always will be. The pid namespace is proved from the PID-1
/// child's `ns/pid`, which is unambiguous.
///
/// `ns/pid_for_children` is never read. **Not** because it reads back empty
/// before the first child -- measured on this kernel it is populated
/// immediately, so that reason would be false -- but because it describes a
/// namespace the reader is not in, so a difference there proves only that one
/// was *requested*.
const SUPERVISOR_NAMESPACES: [&str; 5] = ["user", "mnt", "ipc", "uts", "net"];

#[allow(clippy::too_many_arguments)]
fn handshake(
    cfg: BorrowedFd<'_>,
    blob: &[u8],
    supervisor_pid: u32,
    conf: &Confinement,
    euid: u32,
    egid: u32,
    deadline: Instant,
) -> Result<Handshake, SpawnError> {
    let started = Instant::now();
    send_frame(cfg, blob)?;

    // P12.
    match recv_frame(cfg, deadline)? {
        Frame::Unshared => {}
        other => return Err(unexpected(other)),
    }

    // P13. The core's own read, and the first of the two checkpoints. A
    // failure here is a substitution or a bug, never a transient: it is not
    // retried.
    let mut namespaces_verified = Vec::new();
    for kind in SUPERVISOR_NAMESPACES {
        let ours = read_ns(std::process::id() as i32, kind)?;
        let theirs = read_ns(supervisor_pid as i32, kind)?;
        if ours == theirs {
            return Err(refuse(
                ConfinementFault::NsVerify,
                format!(
                    "the helper reported UNSHARED but its {kind} namespace is still this \
                     core's ({}). A helper that says it unshared and did not is a substituted \
                     binary or a bug in this build; either way a second attempt would produce \
                     the same unconfined realm",
                    ours.display()
                ),
            ));
        }
        namespaces_verified.push(match kind {
            "user" => "user",
            "mnt" => "mnt",
            "ipc" => "ipc",
            "uts" => "uts",
            _ => "net",
        });
    }

    // P14-P16. Order is kernel-forced, not ours: `setgroups=deny` must
    // precede `gid_map` (user_namespaces(7), 3.19), and the helper's
    // `setgroups(0, NULL)` must already have happened -- which is why it is
    // the child's second syscall and not something written here.
    //
    // `uid_map` before `gid_map` is **ours**. There is no kernel edge; it is
    // fixed so the code has one shape.
    write_proc(
        supervisor_pid,
        "setgroups",
        "deny",
        ConfinementFault::SetgroupsWrite,
    )?;
    write_proc(
        supervisor_pid,
        "uid_map",
        &format!("{euid} {euid} 1\n"),
        ConfinementFault::UidMapWrite,
    )?;
    write_proc(
        supervisor_pid,
        "gid_map",
        &format!("{egid} {egid} 1\n"),
        ConfinementFault::GidMapWrite,
    )?;

    // P17. Read back, and this is a proof rather than a formality: a second
    // write to an id map fails `EPERM`, so what is in the file now is what
    // will be in it forever.
    //
    // The comparison is on the parsed triple, not on literal bytes: the
    // kernel renders an id map back as `%10u %10u %10u`, so a byte comparison
    // against what was written fails on every kernel.
    let setgroups = read_proc(supervisor_pid, "setgroups")?;
    let setgroups_denied = setgroups.trim() == "deny";
    if !setgroups_denied {
        return Err(refuse(
            ConfinementFault::MapVerify,
            format!("setgroups reads back {setgroups:?}, not \"deny\""),
        ));
    }
    let uid_map = read_map(supervisor_pid, "uid_map", euid)?;
    let gid_map = read_map(supervisor_pid, "gid_map", egid)?;
    // Read, not assumed, and read for a residual rather than for a guarantee.
    // `deny` blocks the setgroups *call*; it drops nothing, and the kernel
    // leaves no window in which an unprivileged realm could drop them itself
    // (see `IsolationFacts::supplementary_groups_retained`). What the realm
    // kept is therefore a fact worth journaling on every spawn.
    let supplementary_groups = read_supplementary_group_count(supervisor_pid);

    // P18.
    send_frame(cfg, &encode(&Frame::MapDone)?)?;

    // P19. Two writers, concurrently: the supervisor sends `CHILD` and the
    // PID-1 child sends `MOUNTED`. `SOCK_SEQPACKET` makes the order
    // irrelevant, so the loop takes whichever arrives first.
    let mut shim_host_pid: Option<i32> = None;
    let mut mounted: Option<(u32, u64)> = None;
    while shim_host_pid.is_none() || mounted.is_none() {
        match recv_frame(cfg, deadline)? {
            Frame::Child { host_pid } => shim_host_pid = Some(host_pid),
            Frame::Mounted { count, fingerprint } => mounted = Some((count, fingerprint)),
            other => return Err(unexpected(other)),
        }
    }
    let shim_host_pid = shim_host_pid.expect("the loop exits only with both");
    let (mount_count, mount_fingerprint) = mounted.expect("the loop exits only with both");

    // P20. **The clause-4 line.** Everything above proves a helper unshared;
    // only this proves it built a filesystem. A substituted helper that
    // unshares six namespaces and mounts nothing passes every earlier check
    // and fails here.
    //
    // The pid is safe to address by number throughout because the core holds
    // an unreaped `Child`, which pins the supervisor's pid against reuse for
    // the whole handshake -- and PID 1 cannot outlive it.
    let root_view = verify_root_view(shim_host_pid, supervisor_pid, conf)?;

    // P21.
    send_frame(cfg, &encode(&Frame::Proceed)?)?;

    // P22. **EOF is the success signal.** The config channel is `FD_CLOEXEC`
    // in the child, and `FD_CLOEXEC` only takes effect on a *successful*
    // `execve`, so the channel closing means the shim is running. A frame
    // instead of an EOF means it is not.
    //
    // `Ok(None)` and not an error class: EOF-as-a-signal is only sound if
    // EOF can be told apart from failure-to-read. This match used to accept
    // any `HelperDied`, which `recv_frame` also returned for a `recv(2)`
    // *error* -- so `ECONNRESET`, `ENOMEM` or `EFAULT` read as "the execve
    // succeeded". That is the one fail-open shape this whole handshake exists
    // to avoid, and the codec now keeps the two apart at the type level.
    // P21b. **One frame may arrive between `PROCEED` and the EOF**, and only
    // one: PID 1 reports the Landlock rung it enforced (P2.6.3, #187) just
    // before its `execve`, because after the `execve` there is nobody left to
    // report it -- the channel's closing *is* the success signal.
    //
    // It is expected rather than optional, and the loop below refuses a spawn
    // that skipped it. A session that asked for a ruleset and got an EOF with
    // no rung reported is a session with no evidence that a ruleset was ever
    // built, and "no evidence" is not the same as "it worked": that is the
    // silent degradation the whole floor exists to forbid. At
    // `--landlock=off` the helper still sends the frame, carrying rung 0.
    let mut landlock: Option<(u32, u32)> = None;
    loop {
        match recv_frame_or_eof(cfg, deadline)? {
            None => break,
            Some(Frame::Landlocked { rung, kernel_abi }) if landlock.is_none() => {
                landlock = Some((rung, kernel_abi));
            }
            Some(frame) => return Err(unexpected(frame)),
        }
    }
    let (landlock_rung, landlock_kernel_abi) = landlock.ok_or_else(|| {
        refuse(
            ConfinementFault::HelperProtocol,
            "the confinement helper exec'd the shim without reporting which Landlock rung it \
             enforced. The rung is child-asserted and is not what licenses the spawn, but its \
             ABSENCE is this core's only signal that the helper never reached K12b at all -- \
             and a realm whose ruleset may not exist is refused rather than journaled as \
             confined",
        )
    })?;
    if conf.landlock != LandlockRequest::Off && landlock_rung == 0 {
        return Err(refuse(
            ConfinementFault::Landlock,
            format!(
                "this session selected `--landlock={}` and the helper reported that it \
                 enforced no ruleset at all (rung 0). The spawn is refused rather than \
                 started: the difference between the confinement a session asked for and the \
                 confinement it got is exactly what D-020(6) forbids leaving to a log line",
                conf.landlock
            ),
        ));
    }

    // The last thing that binds the *verified* pid to the process that
    // actually `execve`'d. Everything above proved that `shim_host_pid` is
    // the supervisor's child and is PID 1 of a nested namespace whose root is
    // not the host's; this proves the program now running there is the shim
    // the core bound into the realm, and not something else the helper
    // arranged after the checkpoint passed.
    verify_shim_exe(shim_host_pid)?;

    Ok(Handshake {
        namespaces_verified,
        setgroups_denied,
        supplementary_groups,
        uid_map,
        gid_map,
        shim_host_pid,
        root_view,
        mount_count,
        mount_fingerprint,
        landlock_rung,
        landlock_kernel_abi,
        elapsed: started.elapsed(),
    })
}

fn encode(frame: &Frame) -> Result<Vec<u8>, SpawnError> {
    frame.encode().map_err(|e| {
        refuse(
            ConfinementFault::HelperProtocol,
            format!("cannot encode {frame:?}: {e}"),
        )
    })
}

fn send_frame(cfg: BorrowedFd<'_>, bytes: &[u8]) -> Result<(), SpawnError> {
    match rustix::net::send(cfg, bytes, rustix::net::SendFlags::NOSIGNAL) {
        Ok(_) => Ok(()),
        Err(e) => Err(refuse(
            ConfinementFault::HelperDied,
            format!("the confinement helper's channel is gone ({e})"),
        )),
    }
}

/// Receive one frame, or refuse. Never blocks past `deadline`.
///
/// The EOF-expecting caller is [`recv_frame_or_eof`]; every other call site
/// wants a frame, and an EOF there is a helper that died mid-handshake.
fn recv_frame(cfg: BorrowedFd<'_>, deadline: Instant) -> Result<Frame, SpawnError> {
    match recv_frame_or_eof(cfg, deadline)? {
        Some(frame) => Ok(frame),
        None => Err(refuse(
            ConfinementFault::HelperDied,
            "the confinement helper closed its channel without reporting",
        )),
    }
}

/// Receive one frame, `Ok(None)` for a genuine end of stream, or refuse.
///
/// **`Ok(None)` is exactly `recv(2)` returning zero, and nothing else.** The
/// distinction is load bearing rather than tidy: P22 reads EOF as "the shim's
/// `execve` succeeded, so `FD_CLOEXEC` closed the channel". While a `recv`
/// *error* and an end of stream shared one error class, every errno the
/// syscall can return -- `ECONNRESET`, `ENOMEM`, `EFAULT` -- was read as
/// proof that a program started. An adversarial review found that and it was
/// right: an inference from silence is only as good as the reader's ability
/// to tell silence from deafness.
///
/// A helper `FAIL` frame is translated here, once, into this module's own
/// vocabulary -- so the mapping from "where the child gave up" to a
/// `cause_class` lives at one site instead of at each call.
fn recv_frame_or_eof(cfg: BorrowedFd<'_>, deadline: Instant) -> Result<Option<Frame>, SpawnError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(timed_out());
    }
    let mut poll_fd = libc::pollfd {
        fd: cfg.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid `pollfd` for the count given.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, remaining.as_millis() as libc::c_int) };
    if ready == 0 {
        return Err(timed_out());
    }
    if ready < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return recv_frame_or_eof(cfg, deadline);
        }
        return Err(refuse(
            ConfinementFault::HelperProtocol,
            format!("cannot wait on the confinement helper's channel: {err}"),
        ));
    }

    let mut buf = vec![0u8; CONFIG_MAX];
    let read = rustix::net::recv(cfg, &mut buf, rustix::net::RecvFlags::empty()).map_err(|e| {
        // A read that *failed* is not a read that returned nothing. This arm
        // must never reach P22's success path.
        refuse(
            ConfinementFault::HelperProtocol,
            format!(
                "the confinement helper's channel faulted ({e}); this is a failure to read, \
                 which is not the same fact as an end of stream and must never be mistaken \
                 for one -- the shim's execve is reported by EOF and by nothing else"
            ),
        )
    })?;
    let n = read.0;
    if n == 0 {
        return Ok(None);
    }
    let frame = Frame::decode(&buf[..n]).map_err(|e| {
        refuse(
            ConfinementFault::HelperProtocol,
            format!("the confinement helper sent an undecodable frame ({e})"),
        )
    })?;
    if let Frame::Fail { stage, errno } = frame {
        return Err(from_helper_stage(stage, errno));
    }
    Ok(Some(frame))
}

fn timed_out() -> SpawnError {
    refuse(
        ConfinementFault::HelperTimeout,
        format!(
            "the confinement helper did not finish within {} ms. That bound is not a tuning \
             knob: it equals the shortest dead-man hold, so a realm being born can never \
             out-wait the off-switch that would kill it",
            HELPER_DEADLINE.as_millis()
        ),
    )
}

fn unexpected(frame: Frame) -> SpawnError {
    refuse(
        ConfinementFault::HelperProtocol,
        format!("the confinement helper sent {frame:?} out of sequence"),
    )
}

/// Translate the helper's refusal into this module's vocabulary.
///
/// `unshare` splits two ways on the errno, on exactly the rule
/// [`isolation::Support::from_errno`] holds, because the operator's remedy
/// differs: a kernel built without `CONFIG_USER_NS` needs a different kernel,
/// while `apparmor_restrict_unprivileged_userns=1` needs one sysctl.
fn from_helper_stage(stage: Stage, errno: i32) -> SpawnError {
    let os = io::Error::from_raw_os_error(errno);
    match stage {
        Stage::Version => refuse(
            ConfinementFault::HelperVersion,
            format!(
                "the confinement helper is not this core's build ({os}). A helper from one \
                 install beside a vitrind from another is refused rather than run: the two \
                 negotiate a mount table, and a divergence there is a divergence in what got \
                 confined"
            ),
        ),
        Stage::Unshare => {
            let class = match errno {
                libc::EINVAL | libc::ENOSYS => ConfinementFault::UnshareAbsent,
                _ => ConfinementFault::UnshareDenied,
            };
            let hint = if class == ConfinementFault::UnshareDenied {
                " Something above the kernel said no -- most often \
                 `kernel.apparmor_restrict_unprivileged_userns` on Ubuntu 24.04+, or \
                 `user.max_user_namespaces=0`. `vitrind --print-isolation` reads all three \
                 knobs this core knows about."
            } else {
                " This kernel does not implement the request at all; no sysctl will change it."
            };
            refuse(class, format!("the six-flag unshare failed ({os}).{hint}"))
        }
        // Unreachable in this build: the helper no longer attempts the drop,
        // because the kernel leaves no window in which an unprivileged realm
        // could perform it. Mapped anyway rather than folded into the
        // catch-all, so a future per-uid tier that *can* drop them reports
        // its failure under its own label from day one.
        Stage::Setgroups => refuse(
            ConfinementFault::SetgroupsWrite,
            format!("the helper could not drop the operator's supplementary groups ({os})"),
        ),
        Stage::MapVerify => refuse(
            ConfinementFault::MapVerify,
            format!("the helper's own read-back of its id maps disagreed with this core ({os})"),
        ),
        Stage::Fork => refuse(
            ConfinementFault::HelperProtocol,
            format!("the helper could not fork the realm's PID 1 ({os})"),
        ),
        Stage::Mount => refuse(
            ConfinementFault::MountTable,
            format!("a mount-table entry failed inside the realm ({os})"),
        ),
        Stage::PivotRoot => refuse(
            ConfinementFault::PivotRoot,
            format!("pivot_root into the realm's new root failed ({os})"),
        ),
        Stage::Exec => refuse(
            ConfinementFault::ExecShim,
            format!("the shim could not be exec'd inside the realm ({os})"),
        ),
        Stage::Internal => refuse(
            ConfinementFault::HelperProtocol,
            format!("the confinement helper refused at an internal step ({os})"),
        ),
        // Reached only when the *realm's* answer differs from the core's own
        // preflight, which measured the same three syscalls before any realm
        // existed. That gap is worth its own sentence rather than a shrug: it
        // means something changed between startup and this spawn, or the
        // helper is not the binary the preflight described.
        Stage::Landlock => refuse(
            ConfinementFault::Landlock,
            format!(
                "the realm's Landlock ruleset could not be built, granted or enforced ({os}). \
                 A session that reaches this stage asked for a ruleset -- `--landlock=off` \
                 returns from the helper's K12b before any syscall -- and its startup \
                 preflight already found Landlock available, so the two answers disagree: \
                 check `vitrind --print-isolation` against the same kernel, and note that a \
                 grant on an in-realm path the mount table did not create fails here as \
                 ENOENT rather than as a mount-table error"
            ),
        ),
    }
}

/// How many supplementary groups the realm still holds, from
/// `/proc/<pid>/status`'s `Groups:` line.
///
/// `None` on any read or parse failure rather than a refusal: this is a
/// *residual being measured*, not a confinement being verified, and refusing
/// a spawn because a diagnostic could not be read would be fail-closed
/// applied to the wrong thing.
fn read_supplementary_group_count(pid: u32) -> Option<usize> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("Groups:"))?;
    Some(line["Groups:".len()..].split_whitespace().count())
}

fn read_ns(pid: i32, kind: &str) -> Result<PathBuf, SpawnError> {
    let path = format!("/proc/{pid}/ns/{kind}");
    fs::read_link(&path).map_err(|e| {
        refuse(
            ConfinementFault::NsVerify,
            format!(
                "cannot read {path} ({e}); the confinement cannot be verified, so the spawn is \
                 refused rather than taken on trust"
            ),
        )
    })
}

fn read_proc(pid: u32, name: &str) -> Result<String, SpawnError> {
    let path = format!("/proc/{pid}/{name}");
    fs::read_to_string(&path).map_err(|e| {
        refuse(
            ConfinementFault::MapVerify,
            format!("cannot read back {path} ({e})"),
        )
    })
}

fn write_proc(
    pid: u32,
    name: &str,
    contents: &str,
    class: ConfinementFault,
) -> Result<(), SpawnError> {
    let path = format!("/proc/{pid}/{name}");
    fs::write(&path, contents)
        .map_err(|e| refuse(class, format!("cannot write {contents:?} to {path} ({e})")))
}

/// Read an id map back and prove it is the single identity line that was
/// written.
fn read_map(pid: u32, name: &str, expected: u32) -> Result<String, SpawnError> {
    let text = read_proc(pid, name)?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let fields: Vec<u32> = match lines.as_slice() {
        [only] => only
            .split_whitespace()
            .filter_map(|f| f.parse().ok())
            .collect(),
        // Multi-line is not a shape an unprivileged writer can produce, so it
        // means something other than this core wrote the map.
        _ => Vec::new(),
    };
    if fields != vec![expected, expected, 1] {
        return Err(refuse(
            ConfinementFault::MapVerify,
            format!(
                "{name} reads back {text:?}, not the single identity line \
                 `{expected} {expected} 1` this core wrote. A single-id identity map is the \
                 only shape an unprivileged writer may produce, and it is what makes the app \
                 hold zero capabilities: namespace-root (`0 {expected} 1`) would grant it \
                 CAP_SYS_ADMIN inside its own user namespace"
            ),
        ));
    }
    Ok(text.trim().to_string())
}

/// P20. The behavioural probe: what can the realm actually reach?
///
/// Everything here is read by this process through `/proc/<pid>/…`, which
/// needs no `mountinfo` permission and is readable across the realm's user
/// namespace because the core is the same kuid and an ancestor. **Fail closed
/// on any read error**: "I could not tell" is not "it is confined".
///
/// # Step 0 exists because the pid is the helper's word
///
/// `shim_host_pid` arrives in `Frame::Child`. Until an adversarial review
/// pointed it out, nothing checked that the number named the supervisor's
/// child rather than a decoy: a substituted helper could genuinely unshare
/// six namespaces (it must -- P13 reads the *supervisor*, which is the
/// process this core spawned), fork a decoy that pivots into an empty tmpfs
/// and sleeps, report the decoy's pid, and then `execve` the shim in a second
/// child that never left the host filesystem. Every check below would pass on
/// the decoy and the journal would record a confined realm.
///
/// Two `/proc/<pid>/status` fields close it, and both are rendered in **this
/// process's** pid namespace by the kernel rather than supplied by anyone:
///
/// - `PPid:` must be the supervisor the core spawned and holds an unreaped
///   `Child` for. A second child of the supervisor would pass this alone,
///   which is why the second field is not optional.
/// - `NSpid:` must be exactly two fields ending in `1` -- this pid, then its
///   pid *in the namespace one level down*. Exactly two means one level of
///   nesting; ending in `1` means it is that namespace's init, which is what
///   makes killing it take the namespace down.
///
/// And after the `execve`, [`verify_shim_exe`] confirms the program actually
/// running under that pid.
fn verify_root_view(
    shim_host_pid: i32,
    supervisor_pid: u32,
    conf: &Confinement,
) -> Result<RootView, SpawnError> {
    let root = format!("/proc/{shim_host_pid}/root");

    // 0. The pid is the supervisor's child, and it is init of exactly one
    //    nested pid namespace.
    let status = fs::read_to_string(format!("/proc/{shim_host_pid}/status")).map_err(|e| {
        refuse(
            ConfinementFault::RootView,
            format!(
                "cannot read /proc/{shim_host_pid}/status ({e}); the pid the helper reported \
                 cannot be verified, and an unverified pid is not evidence about anything"
            ),
        )
    })?;
    let field = |name: &str| -> Option<String> {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_string()))
    };
    let ppid = field("PPid:").and_then(|v| v.parse::<u32>().ok());
    if ppid != Some(supervisor_pid) {
        return Err(refuse(
            ConfinementFault::RootView,
            format!(
                "the helper reported pid {shim_host_pid} as the realm's PID 1, but its parent \
                 is {ppid:?} and not the supervisor this core spawned ({supervisor_pid}). A \
                 helper is free to fork a decoy, confine the decoy and exec the shim somewhere \
                 else entirely; this check is what makes the pid a fact rather than a claim"
            ),
        ));
    }
    let nspid: Vec<String> = field("NSpid:")
        .map(|v| v.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    if nspid.len() != 2 || nspid[1] != "1" {
        return Err(refuse(
            ConfinementFault::RootView,
            format!(
                "pid {shim_host_pid} has NSpid {nspid:?}, not exactly two entries ending in 1. \
                 Two entries is one level of pid-namespace nesting, and the trailing 1 is what \
                 makes this process that namespace's init -- which is the property the whole \
                 termination ladder rests on"
            ),
        ));
    }

    // 1. The realm's pid namespace, proved from the child that is *in* it.
    let ours = read_ns(std::process::id() as i32, "pid")?;
    let theirs = read_ns(shim_host_pid, "pid")?;
    if ours == theirs {
        return Err(refuse(
            ConfinementFault::RootView,
            format!(
                "the realm's PID 1 is in this core's own pid namespace ({}), so the shim is \
                 not init of anything and killing it would orphan its app rather than take \
                 the namespace down",
                ours.display()
            ),
        ));
    }

    // 2. A different root filesystem, not merely a different view of the same
    //    one.
    let realm_root = fs::metadata(&root).map_err(|e| {
        refuse(
            ConfinementFault::RootView,
            format!("cannot stat {root} ({e}); the realm's filesystem view cannot be verified"),
        )
    })?;
    let host_root = fs::metadata("/")
        .map_err(|e| refuse(ConfinementFault::RootView, format!("cannot stat / ({e})")))?;
    if realm_root.dev() == host_root.dev() {
        return Err(refuse(
            ConfinementFault::RootView,
            format!(
                "the realm's root is on the same device as the host's (st_dev {}), so it never \
                 pivoted onto its own tree. Six differing namespace inodes prove a helper \
                 unshared; they prove nothing about what it mounted, and this is the check \
                 that knows the difference",
                host_root.dev()
            ),
        ));
    }

    // 3. The canaries. This is the same property the acceptance gate asserts,
    //    run on every single spawn rather than once in CI.
    //
    // **The question is "can the realm reach this host inode", not "does this
    // name exist inside the realm".** Those came apart the moment the mount
    // table grew: `app_dir_to_bind` binds the app's containing directory at
    // the same in-realm path, and creating that target `mkdir -p`s every
    // ancestor onto the realm's own root tmpfs. So an app anywhere under
    // `$HOME` -- which is every development checkout, and every realm the
    // integration harness spawns -- materialises `$HOME` inside the realm as
    // an empty stub, and a pure-presence test refused the spawn. Fail-closed,
    // but refusing every spawn is not a confinement property, it is an outage.
    //
    // `(st_dev, st_ino)` separates the two cases exactly: the stub is a fresh
    // inode on a tmpfs this core's helper mounted, the breach is the host's
    // own inode reachable by its own name. The same comparison also *narrows*
    // the check, because a mount table that shadowed a canary with a decoy of
    // the same name no longer passes by accident.
    let mut probed = 0usize;
    for canary in &conf.canaries {
        // Resolved now rather than at startup: this is the identity the realm
        // must not be able to reach *at this instant*, and the whole point of
        // running the probe on every spawn is that startup was a while ago.
        let host = fs::symlink_metadata(canary).map_err(|e| {
            refuse(
                ConfinementFault::RootView,
                format!(
                    "cannot stat the canary {} on the host ({e}). A canary that does not exist \
                     out here proves nothing when it is absent in there, and a vacuous check is \
                     worse than no check because it reads as a pass",
                    canary.display()
                ),
            )
        })?;
        let probe = format!("{root}{}", canary.display());
        probed += 1;
        match fs::symlink_metadata(&probe) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(refuse(
                    ConfinementFault::RootView,
                    format!(
                        "probing {} through the realm's root failed with {e} rather than \
                         `not found`. Fail closed: an error is not evidence of absence",
                        canary.display()
                    ),
                ))
            }
            Ok(inside) if (inside.dev(), inside.ino()) == (host.dev(), host.ino()) => {
                return Err(refuse(
                    ConfinementFault::RootView,
                    format!(
                        "{} is REACHABLE from inside the realm -- same device {} and same inode \
                         {}, so it is the host's own file and not a same-named stub. This is the \
                         check that catches a helper which unshared its namespaces and mounted \
                         nothing",
                        canary.display(),
                        host.dev(),
                        host.ino()
                    ),
                ))
            }
            // Present, but a different inode: the mount table's own stub
            // directory, which holds nothing and leads nowhere the rest of
            // the table does not already permit.
            Ok(_) => {}
        }
    }

    // 4. And what the realm can *write*, measured rather than asserted. Not a
    //    gate here -- turning the write set into a refusal is P2.6.9's
    //    measured-write-set task -- but a parent-observed fact, so the
    //    journal stops printing a sentence about a mount table nobody read.
    let writable = match measure_writable_set(shim_host_pid) {
        Ok(set) => WritableSet::Measured(set),
        Err(detail) => WritableSet::Unreadable(detail),
    };

    Ok(RootView {
        root_dev_differs: true,
        canaries_probed: probed,
        canaries_unreachable: true,
        writable,
    })
}

/// The mount points the realm can write, from the child's own `mountinfo`.
///
/// mountinfo fields are `id parent dev root MOUNTPOINT OPTIONS`, then optional
/// fields, then ` - `. The per-mount options are field 5; searching the whole
/// line for `rw` would match the **superblock** options after the separator,
/// which say nothing about this mount.
fn measure_writable_set(pid: i32) -> Result<Vec<String>, String> {
    let path = format!("/proc/{pid}/mountinfo");
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut set: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let mountpoint = f.nth(4)?;
            let options = f.next()?;
            options
                .split(',')
                .any(|o| o == "rw")
                .then(|| mountpoint.to_string())
        })
        .collect();
    set.sort();
    set.dedup();
    Ok(set)
}

/// The last link in the chain that makes `shim_host_pid` a parent fact: the
/// program running under that pid is the shim the core bound into the realm.
///
/// Read **after** the EOF that reports the second `execve`, because before it
/// the pid is still running `vitrin-realm-init` and the answer would be the
/// helper's path. `/proc/<pid>/exe` is rendered by the kernel against this
/// reader's root, so a path inside a mount namespace this core cannot reach
/// comes back with an `(unreachable)` prefix -- hence a suffix match on the
/// in-realm path rather than an equality against it.
fn verify_shim_exe(shim_host_pid: i32) -> Result<(), SpawnError> {
    let link = format!("/proc/{shim_host_pid}/exe");
    let target = fs::read_link(&link).map_err(|e| {
        refuse(
            ConfinementFault::ExecShim,
            format!(
                "the config channel closed, which should mean the shim `execve`d, but {link} \
                 cannot be read ({e}); fail closed rather than record a shim this core never \
                 saw"
            ),
        )
    })?;
    // Measured on this kernel: the link reads back as exactly `/vitrin/shim`.
    // Matched as a suffix rather than for equality anyway, because `d_path`
    // is documented to prefix a path it cannot reach from the reader's root
    // with `(unreachable)`, and a hardening change that started doing so here
    // must not turn every confined spawn into a refusal.
    let in_realm_shim = Path::new(IN_REALM_SHIM.trim_start_matches('/'));
    if !target.ends_with(in_realm_shim) {
        return Err(refuse(
            ConfinementFault::ExecShim,
            format!(
                "the verified realm pid {shim_host_pid} is running {} and not the shim this \
                 core bound at {IN_REALM_SHIM}. The handshake's EOF says *something* exec'd; \
                 this says what, so a helper cannot pass the checkpoint with one child and run \
                 the shim in another",
                target.display()
            ),
        ));
    }
    Ok(())
}

/// Which directory, if any, has to be bound so the realm can `execve` its app.
///
/// `/usr` and `/etc` are in the table at their own paths, so an app under
/// either needs nothing. Anything else needs its containing directory, bound
/// at the same in-realm path -- and three of those are refused outright,
/// because binding them would undo the table rather than extend it.
fn app_dir_to_bind(
    app: &Path,
    binds: &[PathBuf],
    home: Option<String>,
) -> Result<Option<PathBuf>, SpawnError> {
    if app.starts_with("/usr") || app.starts_with("/etc") {
        return Ok(None);
    }
    let Some(dir) = app.parent() else {
        return Err(refuse(
            ConfinementFault::MountTable,
            format!("{} has no containing directory", app.display()),
        ));
    };
    // Already covered by something the operator declared.
    if binds.iter().any(|b| dir.starts_with(b)) {
        return Ok(None);
    }
    let forbidden: Vec<PathBuf> = ["/", "/home"]
        .iter()
        .map(PathBuf::from)
        .chain(home.map(PathBuf::from))
        .collect();
    if forbidden.iter().any(|f| f == dir) {
        return Err(refuse(
            ConfinementFault::MountTable,
            format!(
                "the app {} lives directly in {}, so confining it would mean binding that \
                 whole directory into the realm -- which is the tree this confinement exists \
                 to hide. Move the binary, or declare a narrower `binds` entry in realm.toml",
                app.display(),
                dir.display()
            ),
        ));
    }
    Ok(Some(dir.to_path_buf()))
}

/// P4. `$XDG_DATA_HOME/vitrin/realms/<realm>/`, through the same
/// verified-parent, `O_NOFOLLOW`, euid-ownership chain as the runtime tree.
///
/// Never `create_dir_all` + `set_permissions`: both follow symlinks, which is
/// the exact bug this module's runtime-directory section records. Returns the
/// directory and whether it already existed.
fn prepare_realm_storage(root: &Path, realm_id: &str) -> Result<(PathBuf, bool), SpawnError> {
    let dir = root.join(realm_id);
    let fault = |detail: String| refuse(ConfinementFault::StorageDir, detail);

    // Every component of the root is ours to make -- unlike `$XDG_RUNTIME_DIR`,
    // `$XDG_DATA_HOME/vitrin/realms` is a path this project owns end to end,
    // so building it is honest rather than papering over a misconfiguration.
    let mut walked = PathBuf::new();
    for part in root.components() {
        walked.push(part);
        match rustix::fs::mkdir(&walked, Mode::from_bits_truncate(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(fault(format!("cannot create {}: {e}", walked.display()))),
        }
    }
    let root_fd = open_owned_dir(root).map_err(fault)?;

    let existed = match rustix::fs::mkdirat(
        &root_fd,
        realm_id,
        Mode::from_bits_truncate(RUNTIME_DIR_MODE),
    ) {
        Ok(()) => false,
        Err(rustix::io::Errno::EXIST) => true,
        Err(e) => return Err(fault(format!("cannot create {}: {e}", dir.display()))),
    };

    // Reopened through the verified parent, `O_NOFOLLOW`, and confirmed to be
    // a directory this euid owns -- so a symlink planted at the realm's name
    // cannot redirect the realm's HOME somewhere the operator did not choose.
    let dir_fd = rustix::fs::openat(
        &root_fd,
        realm_id,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| fault(format!("cannot open {} ({e})", dir.display())))?;
    let st = rustix::fs::fstat(&dir_fd)
        .map_err(|e| fault(format!("cannot stat {} ({e})", dir.display())))?;
    let euid = rustix::process::geteuid().as_raw();
    if st.st_uid != euid {
        return Err(fault(format!(
            "{} is owned by uid {}, not the core's uid {euid}; refusing to hand a realm a HOME \
             this core does not own",
            dir.display(),
            st.st_uid
        )));
    }
    rustix::fs::fchmod(&dir_fd, Mode::from_bits_truncate(RUNTIME_DIR_MODE))
        .map_err(|e| fault(format!("cannot chmod {} ({e})", dir.display())))?;
    Ok((dir, existed))
}

/// P2. `audit_program_at_spawn`'s rule, applied to a **bind source**.
///
/// Not the same function, because a bind source is legitimately a directory
/// and a program is legitimately not -- but the same policy, from the same
/// [`untrusted_writer`] definition, over the path *and every ancestor*. This
/// exists because the owner's decision put `binds` in `realm.toml` beside the
/// app: that file now carries confinement-relevant configuration, so its
/// paths get the same treatment `command` gets.
fn audit_bind_source_at_spawn(source: &Path) -> Result<PathBuf, SpawnError> {
    let reject = |detail: String| SpawnError::ProgramAudit {
        path: source.to_path_buf(),
        detail,
    };
    if !source.is_absolute() {
        return Err(SpawnError::RelativeCommand {
            path: source.to_path_buf(),
        });
    }
    let resolved = fs::canonicalize(source)
        .map_err(|e| reject(format!("bind source does not resolve ({e})")))?;
    let euid = rustix::process::geteuid().as_raw();
    let st = rustix::fs::stat(&resolved)
        .map_err(|e| reject(format!("cannot stat the bind source ({e})")))?;
    let is_dir = FileType::from_raw_mode(st.st_mode) == FileType::Directory;
    if !is_dir && FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(reject(
            "a bind source must be a directory or a regular file".into(),
        ));
    }
    if let Some(fault) = untrusted_writer(st.st_uid, st.st_mode, euid, is_dir) {
        return Err(reject(format!(
            "{fault}; a bind source is mounted read-only INTO the realm, so whoever can write \
             it chooses part of what the confined app reads -- including, if it holds a \
             library or an interpreter, what the app runs"
        )));
    }
    for dir in resolved.ancestors().skip(1) {
        let st = rustix::fs::stat(dir)
            .map_err(|e| reject(format!("cannot stat directory {} ({e})", dir.display())))?;
        if let Some(fault) = untrusted_writer(st.st_uid, st.st_mode, euid, true) {
            return Err(reject(format!(
                "directory {} is {fault}; whoever can write a directory on the path can swap \
                 what gets bound into the realm",
                dir.display()
            )));
        }
    }
    Ok(resolved)
}

/// Compose the child's environment: allow-listed inheritance first, then the
/// core's injections, which therefore win any collision. Reserved names are
/// filtered here regardless of what reached this function -- the structural
/// half of the guarantee (module docs); [`reject_reserved_env`] is the alarm.
///
/// `home` is `Some` only on the confined path. `HOME` joins the injections
/// there because a host `$HOME` that does not exist inside the realm breaks
/// every app that reads it -- which is why `HOME` also joined
/// [`RESERVED_ENV`], and why a configuration that allow-listed it now fails
/// to load.
fn child_env<F>(
    spawn: &SpawnConfig,
    socket_path: &Path,
    runtime_dir: &Path,
    home: Option<&Path>,
    lookup: F,
) -> Vec<(OsString, OsString)>
where
    F: Fn(&str) -> Option<String>,
{
    let mut env: Vec<(OsString, OsString)> = spawn
        .inherited_env(lookup)
        .into_iter()
        .filter(|(name, _)| !is_reserved_env(name))
        // The Landlock audit diagnostic is **not** a realm's variable to
        // allow-list. It is filtered here rather than added to `RESERVED_ENV`
        // because those six names are *decided by the core* -- the core
        // supplies a value for each -- while this one is decided by the
        // operator's own environment and consumed by the helper, never by the
        // app. Allow-listing it in `realm.toml` could not escalate anything
        // (only the core's own environment can carry the value, and only the
        // helper acts on it), but it would put a second route to an
        // instrument's switch inside a realm's configuration, and one route is
        // the number that stays reviewable. See [`landlock_audit_env`].
        //
        // What this does not claim: that the name is invisible inside a realm
        // when the diagnostic IS on. It rides the helper's own environment
        // through `execve`, so the shim and the app can read it there. It
        // authorises nothing -- the domain is already enforced by then.
        .filter(|(name, _)| name != LANDLOCK_AUDIT_ENV)
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
    if let Some(home) = home {
        env.push((OsString::from("HOME"), home.as_os_str().to_os_string()));
    }
    env
}

fn is_reserved_env(name: &str) -> bool {
    RESERVED_ENV.iter().any(|(reserved, _)| *reserved == name)
}

/// The one name the **confined** path adds on top of [`child_env`], and only
/// when the core's own environment asked for it: the Landlock audit-log
/// diagnostic (P2.6.3 follow-up, `vitrin_realm_init::LANDLOCK_AUDIT_ENV`).
///
/// It is deliberately **not** part of [`child_env`]. That function composes
/// the realm's environment, which is a confinement surface with a
/// default-deny rule and an allowlist; this is an instrument the operator
/// running a measurement turns on for the *helper*, and folding it in would
/// mean the unconfined path -- which has no helper and no ruleset -- also
/// carried a variable that could not possibly do anything there.
///
/// The value forwarded is the literal `"1"` rather than whatever the core's
/// own environment held, so there is exactly one spelling of "on" anywhere in
/// the system and the helper's own
/// [`vitrin_realm_init::landlock_audit_requested`] cannot be handed a second
/// one. Every other value -- including `"0"`, `"true"` and the empty string --
/// forwards nothing at all, so an inherited variable cannot switch a realm's
/// audit logging on by accident.
fn landlock_audit_env<F>(lookup: F) -> Option<(OsString, OsString)>
where
    F: Fn(&str) -> Option<String>,
{
    landlock_audit_requested(lookup(LANDLOCK_AUDIT_ENV).as_deref())
        .then(|| (OsString::from(LANDLOCK_AUDIT_ENV), OsString::from("1")))
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
///
/// **P2.6.2 narrowed it rather than closing it**, and the narrowing is worth
/// stating precisely because it is easy to overclaim. At
/// `--isolation=default` the shim and the app are `execve`'d from *read-only,
/// `nosuid`, `nodev` bind mounts inside the realm*, so nothing in the realm
/// can write over either binary after the fork. What that does not cover is
/// the window this paragraph is about, which is on the **host** side and
/// before the helper exists: between this audit and the helper's `open`, a
/// same-uid process outside the realm can still swap the source. The realm's
/// read-only view protects the realm from itself; only `fexecve` -- or the
/// per-uid tier that makes "same uid" stop meaning "the same authority" --
/// closes the host-side race.
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

    /// Capture the log lines a block emits, on this thread only.
    ///
    /// `session.rs`'s own capture is private to its test module and this one
    /// is deliberately not shared with it: the thing under test here is a
    /// sentence a human reads in a log, and a sentence is only asserted by
    /// reading what was emitted. Thread-local
    /// ([`tracing::subscriber::set_default`]) rather than global, because
    /// `main.rs`'s auto-approve banner test owns the one `set_global_default`
    /// this binary may install.
    struct LogCapture {
        lines: std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>,
        _guard: tracing::subscriber::DefaultGuard,
    }

    impl LogCapture {
        fn install() -> Self {
            use tracing_subscriber::layer::SubscriberExt;
            let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let subscriber =
                tracing_subscriber::registry().with(CaptureLayer(std::sync::Arc::clone(&lines)));
            let guard = tracing::subscriber::set_default(subscriber);
            LogCapture {
                lines,
                _guard: guard,
            }
        }

        fn take(&self) -> Vec<(tracing::Level, String)> {
            std::mem::take(&mut *self.lines.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    struct CaptureLayer(std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut message = MessageField(String::new());
            event.record(&mut message);
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((*event.metadata().level(), message.0));
        }
    }

    struct MessageField(String);

    impl tracing::field::Visit for MessageField {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    /// **The ladder fallback is never masked** (P2.6.3, #187).
    ///
    /// The core accepts any rung at or above 1, so before this warning a realm
    /// that fell from rung 9 to rung 1 -- no `TRUNCATE`, no `IOCTL_DEV`, no
    /// scoping, no `RESOLVE_UNIX` -- looked exactly like a full-strength one
    /// unless somebody read two numbers inside a per-realm JSON blob.
    ///
    /// Driven with constructed numbers rather than by finding a kernel that
    /// falls back: the fallback is a kernel behaviour this box does not
    /// exhibit, and waiting for a machine that does is how a warning ships
    /// untested.
    #[test]
    fn a_landlock_rung_below_the_request_or_below_the_kernel_is_warned_about() {
        let plan_for = |kernel, request| vitrin_realm_init::plan_rung(kernel, request);

        // 1. The ladder fell: asked for 9 (the kernel offers 9), got 1.
        let capture = LogCapture::install();
        warn_on_landlock_shortfall(
            "realm-0",
            LandlockRequest::Highest,
            1,
            9,
            plan_for(9, LandlockRequest::Highest),
        );
        let lines = capture.take();
        let fell = lines
            .iter()
            .find(|(_, m)| m.contains("FELL BELOW THE REQUEST"))
            .unwrap_or_else(|| panic!("no fallback warning was emitted; lines were {lines:?}"));
        assert_eq!(fell.0, tracing::Level::WARN, "the fallback is not an INFO");
        for needle in [
            "rung 1",
            "rung 9",
            "ABI 9",
            // What was lost, named. Two numbers alone are what this warning
            // exists to replace.
            "TRUNCATE",
            "IOCTL_DEV",
            "RESOLVE_UNIX",
            "scoping",
        ] {
            assert!(fell.1.contains(needle), "missing {needle:?} in {}", fell.1);
        }
        // And REFER is named as the rung that goes the OTHER way, because a
        // message listing it beside TRUNCATE as a thing "lost" would be this
        // build describing its own confinement wrongly.
        assert!(fell.1.contains("REFER"), "{}", fell.1);
        assert!(fell.1.contains("STRICTER"), "{}", fell.1);
        // The same run also warns that the realm is below the kernel, and the
        // second warning points at the first rather than inventing a cause.
        assert!(
            lines
                .iter()
                .any(|(_, m)| m.contains("BELOW THIS KERNEL") && m.contains("see the warning")),
            "{lines:?}"
        );

        // 2. The operator's cap: the request was honoured, and the realm is
        //    still below the kernel. One warning, naming the flag.
        let capture = LogCapture::install();
        warn_on_landlock_shortfall(
            "realm-0",
            LandlockRequest::CappedAt(2),
            2,
            9,
            plan_for(9, LandlockRequest::CappedAt(2)),
        );
        let lines = capture.take();
        assert!(
            !lines
                .iter()
                .any(|(_, m)| m.contains("FELL BELOW THE REQUEST")),
            "a cap that was honoured is not a fallback: {lines:?}"
        );
        let capped = lines
            .iter()
            .find(|(_, m)| m.contains("BELOW THIS KERNEL"))
            .unwrap_or_else(|| panic!("a pinned realm was not warned about: {lines:?}"));
        assert!(capped.1.contains("--landlock=abi:2"), "{}", capped.1);

        // 3. This build's ladder is shorter than the kernel's -- the clamp,
        //    driven by a CONSTRUCTED ABI rather than by waiting for such a
        //    kernel to exist.
        let over = vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG + 3;
        let plan = plan_for(over, LandlockRequest::Highest);
        assert!(plan.clamped_by_build, "the fixture must be a clamp");
        let capture = LogCapture::install();
        warn_on_landlock_shortfall("realm-0", LandlockRequest::Highest, plan.rung, over, plan);
        let lines = capture.take();
        let clamped = lines
            .iter()
            .find(|(_, m)| m.contains("BELOW THIS KERNEL"))
            .unwrap_or_else(|| panic!("a build-clamped realm was not warned about: {lines:?}"));
        assert!(
            clamped.1.contains("build.landlock_max_rung"),
            "{}",
            clamped.1
        );

        // 4. **Non-vacuity**: the case that must say nothing. A realm that got
        //    exactly what the kernel offers and what the session asked for is
        //    silent, or every session warns and the warning means nothing.
        let capture = LogCapture::install();
        warn_on_landlock_shortfall(
            "realm-0",
            LandlockRequest::Highest,
            9,
            9,
            plan_for(9, LandlockRequest::Highest),
        );
        assert!(
            capture.take().is_empty(),
            "a full-strength realm warned; a warning that fires always is a warning nobody reads"
        );
        // And `--landlock=off` is not a shortfall: it is the operator's own
        // instruction, warned about once at startup rather than once a realm.
        let capture = LogCapture::install();
        warn_on_landlock_shortfall(
            "realm-0",
            LandlockRequest::Off,
            0,
            0,
            plan_for(0, LandlockRequest::Off),
        );
        assert!(
            capture.take().is_empty(),
            "--landlock=off is not a fallback"
        );
    }

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

    /// The **real** `vitrin-realm-init`, resolved exactly as
    /// [`mock_shim_bin`] resolves the mock shim.
    ///
    /// The real binary rather than a stub, deliberately: a confinement test
    /// against a stub helper would prove that this module can talk to a stub.
    pub(crate) fn realm_init_bin() -> PathBuf {
        let exe = std::env::current_exe().expect("the test binary has a path");
        let deps = exe.parent().expect("test binary has a parent directory");
        let mut candidates = vec![deps.join("vitrin-realm-init")];
        if let Some(profile) = deps.parent() {
            candidates.push(profile.join("vitrin-realm-init"));
        }
        for candidate in &candidates {
            if candidate.is_file() {
                return candidate.clone();
            }
        }
        panic!(
            "vitrin-realm-init binary not found in {candidates:?}; run \
             `cargo build -p vitrin-realm-init` (CI's `cargo test --workspace` builds it)"
        );
    }

    /// A binary from `crates/vitrin-realm-init-fixtures`, resolved exactly as
    /// [`mock_shim_bin`] resolves the mock shim.
    ///
    /// Each fixture defeats exactly one of the core's confinement
    /// checkpoints, so a test can prove that checkpoint fires. They exist
    /// because an adversarial review deleted two of the checks and watched
    /// the whole suite stay green: the tests against the *real* helper cannot
    /// exercise a guard a correct helper never trips.
    ///
    /// `crates/vitrin-realm-init-fixtures/tests/binary_contract.rs` is what
    /// makes `cargo test --workspace` build them.
    pub(crate) fn fixture_bin(name: &str) -> PathBuf {
        let exe = std::env::current_exe().expect("the test binary has a path");
        let deps = exe.parent().expect("test binary has a parent directory");
        let mut candidates = vec![deps.join(name)];
        if let Some(profile) = deps.parent() {
            candidates.push(profile.join(name));
        }
        for candidate in &candidates {
            if candidate.is_file() {
                return candidate.clone();
            }
        }
        panic!(
            "{name} fixture binary not found in {candidates:?}; run \
             `cargo build -p vitrin-realm-init-fixtures` (CI's `cargo test --workspace` builds \
             it, via that crate's tests/binary_contract.rs)"
        );
    }

    /// Whether this machine grants the six namespaces the confined spawn
    /// asks for.
    ///
    /// Measured with the same probe the startup floor uses, never inferred
    /// from a kernel version. What changed in issue #288 is the **type**:
    /// the answer is a [`vitrin_skip::Verdict`], which is opaque. A caller
    /// cannot test it, match it, print it or destructure it -- the only
    /// thing it can do is hand it to
    /// [`skip_unless!`](vitrin_skip::skip_unless), and that prints one
    /// machine-readable marker line and, under
    /// `VITRIN_REQUIRE_CONFINEMENT=1` (which CI sets), **panics instead of
    /// returning**.
    ///
    /// That is deliberate, and it is what a `bool` could not do. With a
    /// `bool` (or an `Option<String>`, which this briefly was) the guard
    /// can be inverted and the whole body wrapped in `if
    /// namespaces_available() { ... }`, or moved inside the closure that
    /// *is* the measurement -- two silent skips that no source scan sees
    /// and that were both demonstrated against the first version of this
    /// mechanism. Neither compiles now.
    ///
    /// This function used to `eprintln!` the reason itself and the doc
    /// comment here used to call that "loudly". It was not loud: `cargo
    /// test` captures stdout AND stderr for tests that PASS and prints them
    /// only for failures, so the announcement was visible on a terminal
    /// somebody was watching and invisible in every CI log. The `rust` job
    /// printed `993 passed` while exercising none of the confinement below
    /// from the merge of #186 until #287.
    pub(crate) fn namespaces() -> vitrin_skip::Verdict {
        let report = isolation::Report::probe();
        vitrin_skip::Verdict::capable_if(
            report
                .mechanism(isolation::Mechanism::Namespaces)
                .is_available(),
            format!(
                "this kernel reports ns.all={} -- the confinement tests cannot run here. This is \
                 a real gap in this run's coverage, not a pass.",
                report.namespaces_combined
            ),
        )
    }

    /// Whether this host has the `/usr/bin/env` the reachable-canary
    /// measurement binds.
    ///
    /// A [`vitrin_skip::Verdict`] rather than a `bool` for the same reason
    /// as [`namespaces`]: the caller must not be able to ask, only to hand
    /// the answer over. The path is re-derived after the guard by
    /// [`reachable_canary`], which is a constant either way.
    pub(crate) fn reachable_canary_present() -> vitrin_skip::Verdict {
        let path = reachable_canary();
        vitrin_skip::Verdict::capable_if(
            path.exists(),
            format!(
                "{} is missing on this host, so there is no canary the realm really can reach",
                path.display()
            ),
        )
    }

    /// The canary the reachable-canary measurement binds into the realm.
    pub(crate) fn reachable_canary() -> PathBuf {
        PathBuf::from("/usr/bin/env")
    }

    /// The first ancestor of `dir` the mount table may bind, if any.
    ///
    /// Split out of the test so the same expression answers the guard and
    /// the measurement: the guard turns it into a [`vitrin_skip::Verdict`]
    /// and the measurement re-derives it, rather than the guard handing a
    /// value the test could have got by branching on the probe itself.
    pub(crate) fn bindable_ancestor(dir: &Path) -> Option<PathBuf> {
        dir.parent()
            .filter(|p| p.parent().is_some() && !p.starts_with("/usr") && !p.starts_with("/etc"))
            .map(Path::to_path_buf)
    }

    /// Whether the build directory has an ancestor the mount table can bind.
    pub(crate) fn bindable_ancestor_present(dir: &Path) -> vitrin_skip::Verdict {
        vitrin_skip::Verdict::capable_if(
            bindable_ancestor(dir).is_some(),
            format!(
                "the mock shim's directory ({}) has no bindable ancestor, so there is no stub \
                 the mount table would have had to create",
                dir.display()
            ),
        )
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

    /// Bring a freshly-spawned shim's session up and return the server plus
    /// the shim's first request. This is the **affirmative-liveness gate**
    /// every `/proc/<pid>/{environ,fd}` read in these tests depends on, so it
    /// lives in one place both [`Harness::spawn_mock`] and the ambient-env
    /// test share — no future `spawn_realm`-based test can re-open the #97
    /// gap by hand-rolling a weaker gate.
    ///
    /// [`wait_for_exec`] only proves the kernel installed the new program
    /// image (`begin_new_exec` set `mm->exe_file`). At that instant the
    /// dynamic loader is still running: `create_elf_tables` may not yet have
    /// populated the child's `env_start..env_end` region, so
    /// `/proc/<pid>/environ` can read empty; and nothing there proves the
    /// child is still alive rather than already a zombie when the read
    /// lands. A `create_surface` arriving is proof from the child's *own*
    /// post-`execve` code that its env region is populated — and because the
    /// caller still holds the core side of a **blocking** transport open
    /// (vitrin-ipc constructors set no `O_NONBLOCK`, and these unit tests run
    /// no event loop to toggle it), the child that sent it is parked in its
    /// blocking `recv` and cannot have exited. Both empty-`environ`
    /// preconditions — mid-`execve`, or dead — are thereby excluded by
    /// construction, not merely made less likely.
    fn bring_up_shim(spawned: &mut SpawnedRealm) -> (ShimServer, vitrin_ipc::Message) {
        let server = spawned
            .start_shim_session(1280, 800)
            .expect("configure must reach the shim over the inherited socketpair");
        let msg = spawned
            .connection_mut()
            .recv_message()
            .expect("the shim's first request must arrive")
            .expect("the shim must not hang up during bring-up");
        // Round-trip on the shim's own create_surface, not a spurious byte:
        // this makes the gate meaningful even for callers (the ambient-env
        // test) that do not go on to dispatch the message.
        assert_eq!(
            msg.header.opcode,
            vitrin_protocol::generated::vitrin_shim_session::requests::CreateSurface::OPCODE,
            "bring-up must round-trip on the shim's create_surface"
        );
        (server, msg)
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
            // The mock shim is the core-inserted `--shim` binary in tests: a
            // valid, audit-passing fd-3 holder. The realm's `command` (the
            // app) is whatever a given test configures.
            SpawnPaths::under(&self.base, mock_shim_bin())
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
        /// because it is also the readiness gate every `/proc` assertion
        /// needs — see [`bring_up_shim`], which owns that gate and its
        /// rationale.
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
            let mut spawned = spawn_realm_with_env(
                &realm,
                &paths,
                &mut self.recorder,
                SpawnOrigin::Startup,
                |name| core_env.get(name).cloned(),
            )
            .expect("spawn must succeed against a scratch runtime tree");
            wait_for_exec(spawned.pid(), &bin);

            // The wiring under test: the inherited socketpair becomes a
            // served shim session, and the shim's first request is
            // dispatched through the real `ShimServer` -- so this is an
            // end-to-end proof that identity-at-fork produces exactly the
            // connection P1.3.4's server expects. `bring_up_shim` is also the
            // readiness gate the `/proc` reads below depend on.
            let (mut server, msg) = bring_up_shim(&mut spawned);
            let mut scene = crate::scene::Scene::new();
            let conn = spawned.connection_mut();
            server
                .handle_message(msg, &mut scene, None, &mut |bytes: &[u8]| {
                    conn.send_message(bytes, None)
                })
                .expect("the shim's bring-up request must be well-formed");
            (spawned, server)
        }

        /// The confinement inputs a test spawn uses: the **real**
        /// `vitrin-realm-init` binary, a scratch storage root, and one canary
        /// that lives outside every path the realm's table binds.
        fn confinement(&self) -> Confinement {
            Confinement {
                realm_init: realm_init_bin(),
                storage_root: self.base.join("storage"),
                canaries: vec![self.canary()],
                // Empty rather than the machine's real nodes: a test must not
                // depend on this runner having a GPU, and the node bind is
                // the one row whose absence changes nothing about what the
                // canary check proves.
                render_nodes: Vec::new(),
                // Small on purpose. The shipped caps are sized for a 4K
                // double-buffered `wl_shm` pool; a test that reserved 133 MiB
                // of tmpfs per spawn would be charging CI for a number it
                // does not exercise.
                tmpfs: TmpfsCaps {
                    root: 1024 * 1024,
                    dev: 1024 * 1024,
                    shm: 4 * 1024 * 1024,
                    tmp: 4 * 1024 * 1024,
                },
                // The shipped default, so a component test exercises the
                // rung the operator gets rather than a weaker one nobody
                // runs. The per-rung measurements live in
                // `vitrin-realm-init`'s own suite, where a forked child can
                // enforce a capped domain without confining this binary.
                landlock: LandlockRequest::Highest,
            }
        }

        /// A host file that must be unreachable from inside the realm.
        ///
        /// **Never under `/tmp`, and that is the whole design of this
        /// function.** The realm gets a fresh `/tmp` tmpfs unconditionally,
        /// so a canary there is shadowed by one line of the mount table: a
        /// helper that mounted *only* that one tmpfs and nothing else would
        /// pass, which makes the canary vacuous in exactly the way D-036's
        /// own open question warned against. It has to live somewhere the
        /// realm's absence of it is a consequence of the pivot, not of a
        /// tmpfs the core happens to stack on top.
        ///
        /// `$XDG_RUNTIME_DIR` first: it is a real host directory, no mount in
        /// the table shadows it, and writing there is what that directory is
        /// for. `$HOME` second, on the same terms the acceptance gate uses --
        /// created per run and removed in the teardown. If neither is set
        /// this **panics** rather than falling back to `/tmp`: a test that
        /// silently substitutes a vacuous canary is worse than one that does
        /// not run, because it reports a pass.
        fn canary(&self) -> PathBuf {
            let name = format!(
                "vitrin-confinement-canary-{}-{}",
                std::process::id(),
                self.base.file_name().unwrap().to_string_lossy()
            );
            let dir = ["XDG_RUNTIME_DIR", "HOME"]
                .iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from)
                .find(|d| d.is_dir())
                .unwrap_or_else(|| {
                    panic!(
                        "neither $XDG_RUNTIME_DIR nor $HOME names a directory, so this test has \
                         nowhere to put a canary that the realm's own /tmp tmpfs does not \
                         shadow. Refusing to substitute a /tmp path: it would be unreachable \
                         inside every realm whether or not the mount table was built, and the \
                         test would pass on a helper that did nothing"
                    )
                });
            let path = dir.join(name);
            fs::write(&path, b"the realm must not be able to read this").unwrap();
            path
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

    // -- acceptance: the core inserts a shim; the app rides its argv --------

    #[test]
    fn the_core_execs_the_shim_and_conveys_the_app_command_in_argv() {
        // The #103 acceptance: the core execs the core-inserted SHIM binary
        // (which holds fd 3), never the realm's `command` directly, and
        // conveys the app command to the shim in argv after a `--` separator.
        let _fd = fd_lock();
        let mut h = Harness::new("spawn-shim-insertion");
        let shim = mock_shim_bin();
        // A real, always-present app the mock shim ignores (#103 does not exec
        // it) -- deliberately DISTINCT from the shim, so "the core execs the
        // shim, not the app" is a real assertion rather than a tautology.
        // `--serve` rides the app-argument tail, which the mock scans, so the
        // fixture brings its session up.
        let app = PathBuf::from("/bin/true");
        let realm = realm_with_spawn("realm-0", &app, &["--serve".to_string()], &[]);
        let paths = SpawnPaths::under(&h.base, &shim);
        let mut spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("spawn must succeed against a scratch runtime tree");

        // The direct child is the SHIM, not `/bin/true`: had the core exec'd
        // `command` directly, `/proc/<pid>/exe` would never flip to the shim
        // and this would time out.
        wait_for_exec(spawned.pid(), &shim);
        // Readiness gate before any /proc read (see `bring_up_shim`).
        let (_server, _msg) = bring_up_shim(&mut spawned);

        // The app command is conveyed in the shim's argv after `--`.
        let raw = fs::read(format!("/proc/{}/cmdline", spawned.pid())).expect("cmdline reads");
        let argv: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        assert_eq!(
            argv.first().map(String::as_str),
            shim.to_str(),
            "argv[0] is the shim the core execs: {argv:?}"
        );
        let sep = argv
            .iter()
            .position(|a| a == "--")
            .expect("the app command is conveyed after a `--` separator");
        assert_eq!(
            argv.get(sep + 1).map(String::as_str),
            Some("/bin/true"),
            "the app command follows `--` in the shim's argv: {argv:?}"
        );

        // And fd 3 is the inherited core socketpair, held by the shim.
        let fds = child_fds_of(spawned.pid());
        assert!(
            fds.get(&SHIM_CORE_FD)
                .is_some_and(|t| t.starts_with("socket:")),
            "the shim must hold the core socketpair at fd {SHIM_CORE_FD}: {fds:?}"
        );

        h.reap(spawned);
    }

    #[test]
    fn a_group_writable_shim_is_refused_at_spawn_time() {
        // The shim is audited transitively, exactly like `command`: whoever
        // can write the binary the trusted core execs chooses what it runs. A
        // group-writable shim is refused with a typed `ProgramAudit` naming
        // the shim -- the app here is the beyond-reproach mock, so the refusal
        // is unambiguously the shim's, not the app's.
        let _fd = fd_lock();
        let dir = scratch();
        let shim = dir.join("shim");
        fs::write(&shim, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o775)).unwrap();

        let realm = realm_with_spawn("realm-0", &mock_shim_bin(), &[], &[]);
        let (err, _) = refused_spawn_with_shim("spawn-group-writable-shim", &realm, &shim);
        assert!(matches!(err, SpawnError::ProgramAudit { .. }), "{err}");
        assert!(
            err.to_string().contains("writable by group/other"),
            "the refusal must name the fault: {err}"
        );
        if let SpawnError::ProgramAudit { path, .. } = &err {
            assert_eq!(
                path, &shim,
                "the audit refusal must name the shim, not the app"
            );
        }
        let _ = fs::remove_dir_all(&dir);
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
            // `TERM` and not `HOME`: since P2.6.2 `HOME` is reserved (the
            // core injects the realm's private storage), so a config that
            // allow-listed it would be refused before the fork and this test
            // would be asserting the refusal instead of the environment.
            &["TERM", "LANG", "NEVER_SET"],
            &[
                ("TERM", "foot"),
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
            BTreeSet::from(["TERM", "LANG", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"]),
            "the child's environment is default-deny: allowlist + injections, nothing more"
        );
        assert_eq!(env.get("TERM").map(String::as_str), Some("foot"));

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
            ["TERM", "LANG", "NEVER_SET"]
        );
        let line = format!("{spawn_entry:?}");
        assert!(
            !line.contains("en_US"),
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
            BTreeSet::from(["HOME", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"]),
            "an empty allowlist inherits NOTHING"
        );
        // `HOME` is in that set as a core INJECTION, not as inheritance --
        // the same status `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` have. It is
        // in `RESERVED_ENV`, so `env_allow` could not have passed it through
        // whatever the operator wrote; the core decides it in both isolation
        // modes and here decides on the host's own.
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/agent"));
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
        // `spawn_realm` (not `spawn_realm_with_env`/`spawn_mock`) is kept on
        // purpose: this test exists to exercise the production
        // `std::env::var` ambient path that the *_with_env variants bypass.
        let mut spawned =
            spawn_realm(&realm, &paths, &mut h.recorder, SpawnOrigin::Startup).expect("spawn");
        wait_for_exec(spawned.pid(), &bin);

        // Affirmative-liveness gate before any /proc read: only after the
        // child's own post-execve code has sent create_surface is its env
        // region provably populated and the child provably still alive (see
        // bring_up_shim). wait_for_exec alone leaves the #97 window where
        // /proc/<pid>/environ can read empty.
        let (_server, _msg) = bring_up_shim(&mut spawned);

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
        if let (Ok(paths), Ok(dir)) = (SpawnPaths::from_env(mock_shim_bin()), paths::runtime_dir())
        {
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
        // The common case: a valid shim (the mock) and a realm whose *app*
        // (`command`) is what the test makes hostile.
        refused_spawn_with_shim(label, realm, &mock_shim_bin())
    }

    /// [`refused_spawn`] with an explicit `--shim` binary, so a test can make
    /// the *shim* the hostile half (a group-writable or non-executable shim)
    /// while the app is beyond reproach.
    fn refused_spawn_with_shim(label: &str, realm: &Realm, shim: &Path) -> (SpawnError, Json) {
        let mut h = Harness::new(label);
        let paths = SpawnPaths::under(&h.base, shim);
        let err =
            spawn_realm_with_env(realm, &paths, &mut h.recorder, SpawnOrigin::Startup, |_| {
                Some("hostile".into())
            })
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
    fn a_non_executable_shim_is_a_typed_error_at_exec() {
        // Distinct from the audit refusals: the file passes the
        // trusted-writer rule (0644 is not group/other WRITABLE) and fails
        // at `execve` with EACCES. It must still be typed, and must still
        // leave nothing behind -- which is the interesting half, because
        // this is the one failure that happens *after* the runtime
        // directory and the socketpair already exist.
        //
        // The non-executable file is the **shim** here, not the app: since
        // issue #103 the shim is the binary the core actually execs, so an
        // `execve` failure is the shim's. The app (`command`) is the valid
        // mock, which the shim would exec later, out of this module's reach.
        let _fd = fd_lock();
        let dir = scratch();
        let shim = dir.join("not-executable");
        fs::write(&shim, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o644)).unwrap();

        let realm = realm_with_spawn("realm-0", &mock_shim_bin(), &[], &[]);
        let (err, entry) = refused_spawn_with_shim("spawn-noexec", &realm, &shim);
        assert!(matches!(err, SpawnError::Exec { .. }), "{err}");
        assert_eq!(err.cause_class(), "exec");
        assert_eq!(entry.str("cause_class"), "exec");
        // The exec failure names the shim, the program the core actually ran.
        if let SpawnError::Exec { command, .. } = &err {
            assert_eq!(command, &shim, "the exec error must name the shim");
        }
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
        let err = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
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
            None,
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
        // The shim is irrelevant to directory/lock derivation; a placeholder
        // keeps these pure runtime-tree tests off the mock-binary lookup.
        let paths = SpawnPaths::under(base, Path::new("/vitrin-shim-placeholder"));
        (
            paths.realm_dir("realm-0").unwrap(),
            paths.realm_lock("realm-0").unwrap(),
        )
    }

    /// **A spawn record that is dropped without being journaled is caught.**
    ///
    /// The structural half of issue #207's accountability claim, and the half
    /// that was missing. `#[must_use]` is on this type and did nothing: it
    /// fires when an *expression's* value is discarded and says nothing once
    /// the value is moved into a struct field inside a `Vec` — which is
    /// exactly where a pending launch keeps it. A dispatch turn ending on a
    /// transport fault returned early, dropped the vector, and left a forked
    /// process with no journal entry naming who asked for it.
    ///
    /// This pins the [`Drop`] guard rather than that one call path, because a
    /// drop is what every escape path has in common: a future early return
    /// nobody has written yet is caught by the same assertion.
    ///
    /// **Debug-only, and that is the guard's design rather than a gap in the
    /// test.** `Drop` aborts under `debug_assert!` and merely logs at `error`
    /// in release, because losing a journal line is bad and killing a live
    /// desktop session over it is worse. So this test cannot run in release —
    /// and `cargo test --release` is exactly where it first went red, because
    /// a local `cargo test --workspace` is debug only while CI runs both.
    /// Do not "fix" this by deleting the `cfg`; the release half of the guard
    /// is a log line, and asserting on one would cost more machinery than it
    /// is worth.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "dropped without being journaled")]
    fn a_spawn_record_dropped_without_journaling_is_caught() {
        let _fd = fd_lock();
        let base = scratch();
        // A command that cannot possibly spawn: the record's outcome is `Err`,
        // which still owes a `realm_spawn_failed` entry. A failed spawn is
        // exactly as accountable as a successful one — "the core tried to run
        // this" is the fact, and it is the one an attacker would most like
        // missing from the log.
        let realm = crate::realm::tests::realm_with_spawn(
            "realm-0",
            Path::new("/nonexistent/vitrin-spawn-record-drop-test"),
            &[],
            &[],
        );
        let paths = SpawnPaths::under(&base, Path::new("/vitrin-shim-placeholder"));
        let (_result, record) =
            spawn_realm_deferring_journal(&realm, &paths, SpawnOrigin::Startup, |_| None);
        drop(record);
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
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
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
        let paths = SpawnPaths::under("/run/user/1000", Path::new("/vitrin-shim-placeholder"));
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

    // -- P2.6.2 (#186): the confined spawn path ----------------------------

    #[test]
    fn the_selector_is_derived_from_the_inputs_and_cannot_disagree_with_them() {
        // `SpawnPaths::isolation()` is computed from whether a `Confinement`
        // is present rather than stored beside it, so "this session says it
        // confines" and "this session has what confining needs" are one fact.
        let h = Harness::new("isolation-derived");
        let off = h.paths();
        assert_eq!(off.isolation(), Isolation::Off);
        let on = h.paths().confined(h.confinement());
        assert_eq!(on.isolation(), Isolation::Default);
    }

    #[test]
    fn an_unconfined_spawn_journals_no_confinement_it_did_not_perform() {
        // The half of clause 4 that is easy to get wrong in the other
        // direction: at `--isolation=off` the parent-observed fields must be
        // `null`, not `false` and not a hopeful default. A reader has to be
        // able to tell "the core looked and found nothing" from "the core
        // never looked".
        let _fd = fd_lock();
        let mut h = Harness::new("unconfined-journal");
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[]);
        let facts = spawned.isolation().clone();
        assert_eq!(facts.applied_profile, "none");
        assert!(facts.namespaces_verified.is_empty());
        assert_eq!(facts.root_dev_differs, None);
        assert_eq!(facts.canaries_unreachable, None);
        assert_eq!(facts.setgroups_denied, None);
        assert_eq!(facts.shim_host_pid, None);
        assert_eq!(facts.mount_count, None);
        h.reap(spawned);

        let entries = h.entries();
        let entry = entries
            .iter()
            .find(|e| e.str("kind") == "realm_spawned")
            .expect("realm_spawned");
        let line = format!("{entry:?}");
        assert!(line.contains("\"applied_profile\": \"none\"") || line.contains("none"));
        // The Landlock object is written with every field null rather than
        // omitted -- this file's rule that absent information is an explicit
        // value. Until #187 the same statement was the fixed string
        // "not-applied (P2.6.3)"; a null `requested` now says the same thing
        // more precisely, because at `--isolation=off` no helper runs and
        // nothing was asked for OR obtained.
        //
        // Asserted against the PARSED entry rather than by substring. A
        // `line.contains("landlock")` stood here briefly and could not fail:
        // `write_isolation` emits that key unconditionally on every path, so
        // the assertion held whatever the object contained -- including a
        // rung. `Json::at` panics on a missing member, so dropping the object
        // or any field turns these red, and `is_null` turns them red if a
        // number appears where "nothing was measured" is the only honest
        // answer.
        for field in [
            "requested",
            "obtained_rung",
            "kernel_abi",
            "clamped_by_build",
        ] {
            assert!(
                entry.is_null(&format!("isolation.landlock.{field}")),
                "at --isolation=off no helper runs, so isolation.landlock.{field} must be \
                 null rather than a value: {line}"
            );
        }
        // And the provenance label goes null with them. It used to read
        // `child-asserted` here, which is a claim about numbers made where
        // there are none -- no helper ran at `--isolation=off`, so no child
        // asserted anything. The key is still written (this recorder's rule:
        // absent information is an explicit value), only its value moves.
        assert!(
            entry.is_null("isolation.landlock.rung_evidence"),
            "at --isolation=off nothing was asked and nothing was measured, so the object \
             may not label an absence with the provenance of a number: {line}"
        );
        assert!(
            line.contains("rung_evidence"),
            "the key must still be present, null rather than omitted: {line}"
        );
        assert_eq!(facts.landlock_requested, None);
        assert_eq!(facts.landlock_rung, None, "nothing enforced it, so no rung");
        assert_eq!(facts.landlock_kernel_abi, None, "and nothing read the ABI");
    }

    /// The confined bring-up, end to end, against the **real**
    /// `vitrin-realm-init` and the **real** mock shim.
    ///
    /// This is a component test, not P2.6.2's acceptance gate: the shim is
    /// `vitrin-mock-shim`, so it proves the mechanism rather than the
    /// milestone. The mock-free gate is `tests/integration/`'s, and this
    /// test's value is that it runs on every `cargo test` and catches a
    /// broken mount table in seconds instead of in CI's Python suite.
    #[test]
    fn a_confined_realm_cannot_reach_the_canary_and_the_core_proves_it_from_outside() {
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let mut h = Harness::new("confined-spawn");
        let confinement = h.confinement();
        let canary = confinement.canaries[0].clone();
        // Non-vacuity, first and unconditionally: the canary has to be a path
        // that really exists and really is readable out here, or its absence
        // inside the realm would be an absence over nothing.
        assert!(
            fs::read(&canary).is_ok(),
            "the canary must be readable on the host, or its unreachability proves nothing"
        );

        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("the confined spawn must succeed on a kernel that grants the namespaces");

        let facts = spawned.isolation().clone();
        // The rung is what the helper reported, and it is at least the floor:
        // rung 0 is `--landlock=off`, which this spawn did not select.
        assert_eq!(facts.landlock_requested, Some(LandlockRequest::Highest));
        let rung = facts
            .landlock_rung
            .expect("a confined spawn reports its rung");
        assert!(
            rung >= 1,
            "the helper enforced no ruleset on a spawn that asked for the highest rung"
        );
        let kernel_abi = facts
            .landlock_kernel_abi
            .expect("and the kernel's own ABI beside it");
        assert!(
            rung <= kernel_abi,
            "the reported rung is above the ABI the same child read from the same kernel"
        );
        // **The profile is the rung that was OBTAINED**, so it moves with the
        // ladder. Derived here from the number the helper reported, which is
        // the whole assertion: a profile computed from the *request* would
        // read `namespaces+landlock` on a kernel that granted rung 1.
        assert_eq!(
            facts.applied_profile,
            isolation::profile_for(Isolation::Default, rung)
        );
        assert!(
            facts.applied_profile.ends_with(&format!("abi{rung}")),
            "the obtained rung must be in the profile: {}",
            facts.applied_profile
        );
        // The build clamp, reported rather than merely computed. This box's
        // kernel is inside the build's ladder, so `false` is the measured
        // answer; the constructed-probe assertion lives in
        // `vitrin-realm-init`'s own suite, which does not need such a kernel
        // to exist.
        assert_eq!(
            facts.landlock_clamped_by_build,
            Some(kernel_abi > vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG)
        );
        assert_eq!(
            facts.namespaces_verified,
            ["user", "mnt", "ipc", "uts", "net"],
            "all five supervisor namespaces must have been READ, not assumed"
        );
        assert_eq!(facts.root_dev_differs, Some(true));
        assert_eq!(facts.canaries_unreachable, Some(true));
        assert_eq!(facts.canaries_probed, 1);
        assert_eq!(facts.setgroups_denied, Some(true));
        assert!(facts.mount_count.unwrap_or(0) > 5, "{facts:?}");
        let shim_pid = facts.shim_host_pid.expect("the shim's host pid");
        assert_ne!(
            shim_pid as u32, facts.supervisor_pid,
            "the supervisor and the shim must be two processes: unshare(CLONE_NEWPID) does not \
             move the caller, so something has to fork to produce PID 1"
        );
        assert!(
            facts.handshake_ms <= HELPER_DEADLINE.as_millis() as u64,
            "the handshake took {} ms against a {} ms bound",
            facts.handshake_ms,
            HELPER_DEADLINE.as_millis()
        );

        // The properties again, read a second time from out here rather than
        // taken from the struct the spawn built -- so a bug that filled the
        // struct in without doing the work cannot pass.
        let root = format!("/proc/{shim_pid}/root");
        assert!(
            fs::symlink_metadata(format!("{root}{}", canary.display()))
                .err()
                .map(|e| e.kind() == io::ErrorKind::NotFound)
                .unwrap_or(false),
            "the canary is reachable through the realm's root"
        );
        assert!(
            fs::symlink_metadata(format!("{root}/usr/bin")).is_ok(),
            "the realm has no /usr, so it is not confined, it is broken"
        );
        assert_ne!(
            fs::read_link(format!("/proc/{shim_pid}/ns/pid")).unwrap(),
            fs::read_link("/proc/self/ns/pid").unwrap(),
            "the shim must be PID 1 of its own namespace, or killing it orphans its app"
        );

        h.reap(spawned);
        let _ = fs::remove_file(&canary);
    }

    #[test]
    fn a_confined_spawn_journals_the_split_between_what_it_read_and_what_it_was_told() {
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let mut h = Harness::new("confined-journal");
        let confinement = h.confinement();
        let canary = confinement.canaries[0].clone();
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("the confined spawn must succeed");
        h.reap(spawned);
        let _ = fs::remove_file(&canary);

        let entries = h.entries();
        let entry = entries
            .iter()
            .find(|e| e.str("kind") == "realm_spawned")
            .expect("realm_spawned");
        let line = format!("{entry:?}");
        for needle in [
            "parent_observed",
            "child_asserted",
            "namespaces+landlock",
            "mount_fingerprint",
            "fnv1a-64",
            // P2.6.3's pair, and both halves are needed: one number cannot
            // separate a session pinned low by `--landlock=abi:N` from a
            // session on a kernel that offers no more.
            "obtained_rung",
            "kernel_abi",
            // The third: whether this BUILD's ladder, rather than the kernel
            // or the operator, is what held the rung down. Computed for every
            // realm since #187 and, until this needle, read by nobody.
            "clamped_by_build",
            // The label that keeps the pair out of the parent's column.
            "child-asserted",
        ] {
            assert!(line.contains(needle), "missing {needle} in {line}");
        }
        // The claim the entry may never make. Still true at #187: the tier
        // means namespaces plus Landlock plus SECCOMP, and the filter is
        // #188's.
        assert!(
            !line.contains("intra-user"),
            "a build that applies namespaces and Landlock may not journal a tier that also \
             means seccomp: {line}"
        );
        // And the rung is a number that was reported, never a hopeful
        // default: rung 0 is what `--landlock=off` journals, and this spawn
        // did not ask for that.
        assert!(
            !line.contains("\"obtained_rung\":0"),
            "a confined spawn journaled rung 0, which is the `--landlock=off` value: {line}"
        );
    }

    /// Spawn one confined realm and hand it to `f`, then tear it down.
    ///
    /// Factored out so each property below is one assertion against a real
    /// realm rather than forty lines of bring-up: the bring-up is the same
    /// every time, and a copy of it per test is a copy that drifts.
    ///
    /// **The closure returns [`vitrin_skip::Measured`]** (#288). A bare
    /// `return;` inside it would leave the closure rather than the test, so
    /// this helper would complete, the test would report `ok`, and nothing
    /// would have been asserted -- a silent skip that `cargo xtask
    /// skip-scan` cannot see, because it deliberately does not count
    /// returns inside closures. Requiring the token makes `return;` a type
    /// error; the closure has to end by saying `vitrin_skip::measured()`.
    fn with_confined_realm(
        label: &str,
        f: impl FnOnce(i32, &IsolationFacts) -> vitrin_skip::Measured,
    ) {
        let mut h = Harness::new(label);
        let confinement = h.confinement();
        let canary = confinement.canaries[0].clone();
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("the confined spawn must succeed");
        let facts = spawned.isolation().clone();
        let _measured: vitrin_skip::Measured =
            f(facts.shim_host_pid.expect("the shim's host pid"), &facts);
        h.reap(spawned);
        let _ = fs::remove_file(&canary);
    }

    /// The `--landlock` cap, end to end: core -> config blob -> helper ->
    /// ladder -> journal (P2.6.3, #187).
    ///
    /// **The point is that the cap is a shipped flag rather than a build
    /// feature**, so this test drives the same binary an operator runs. What
    /// it proves is narrow and specific: the rung the helper enforced is the
    /// rung this core asked for, and the journal carries both it and the
    /// kernel's own ABI so a pinned-low session cannot be mistaken for a
    /// full-strength one. What each rung *buys* is measured in
    /// `vitrin-realm-init`'s own suite, where a forked child can enforce a
    /// capped domain without confining this test binary for good.
    #[test]
    fn a_capped_session_enforces_the_rung_it_asked_for_and_journals_both_numbers() {
        // This one spawns a REAL confined realm, so it needs the same guard the
        // other nine confinement tests carry: a host that refuses the mount
        // inside a user namespace (Ubuntu 24.04+'s AppArmor default, and the
        // GitHub runner) cannot run it at all. CI takes the sysctl remedy in
        // the `rust` job so this runs there rather than skipping.
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let mut h = Harness::new("landlock-cap");
        let mut confinement = h.confinement();
        confinement.landlock = LandlockRequest::CappedAt(2);
        let canary = confinement.canaries[0].clone();
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("a capped session still spawns");
        let facts = spawned.isolation().clone();
        h.reap(spawned);
        let _ = fs::remove_file(&canary);

        let kernel = facts.landlock_kernel_abi.expect("the realm read the ABI");
        // Worse than a guard at the top of a test: this one fires AFTER the
        // spawn, so before #288 the test reported `ok` having asserted
        // nothing at all about the cap. The rung travels inside the verdict,
        // so the number the require-floor is compared against and the number
        // this compared against are one number rather than two.
        vitrin_skip::skip_unless!(
            vitrin_skip::LANDLOCK_ABI,
            vitrin_skip::Verdict::capable_if_at_rung(
                kernel >= 2,
                2,
                format!(
                    "the cap assertion: this kernel reports Landlock ABI {kernel}, so rung 2 is \
                     not below it and the cap could not have weakened anything."
                )
            )
        );
        assert_eq!(
            facts.landlock_rung,
            Some(2),
            "the helper enforced a rung other than the one the core capped it to"
        );
        assert!(
            facts.landlock_rung < facts.landlock_kernel_abi,
            "on a kernel that offers more, a capped session must journal a rung BELOW the \
             kernel's own -- that inequality is the whole signal that the session is pinned"
        );
        assert_eq!(
            facts.applied_profile, "namespaces+landlock-abi2",
            "a capped session must not render like the default one"
        );

        // Non-vacuity, in the same run: the uncapped spawn on this machine
        // reaches the kernel's own rung, so the numbers above are the cap's
        // doing and not a ceiling this box happens to have.
        let mut h = Harness::new("landlock-uncapped");
        let confinement = h.confinement();
        let canary = confinement.canaries[0].clone();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("the uncapped control spawns");
        let control = spawned.isolation().clone();
        h.reap(spawned);
        let _ = fs::remove_file(&canary);
        assert_eq!(
            control.landlock_rung,
            Some(kernel.min(vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG)),
            "the uncapped control did not reach this kernel's rung, so the capped run above \
             proves nothing about the cap"
        );
        assert_ne!(
            control.applied_profile, facts.applied_profile,
            "the capped and uncapped runs journal the same profile string, so a reader \
             greping the journal cannot tell them apart"
        );
    }

    /// One field of `/proc/<pid>/status`, or `None`.
    fn proc_status(pid: i32, field: &str) -> Option<String> {
        let text = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        text.lines()
            .find(|l| l.starts_with(field))
            .map(|l| l[field.len()..].trim().to_string())
    }

    #[test]
    fn the_two_paths_diverge_on_stdio_and_on_home_and_nowhere_else_observable() {
        // `off` must stay the path that shipped before #186, and the *whole*
        // reason `default` is a second path is a short list of differences.
        // Asserting the list on the child's own `/proc` state -- rather than
        // snapshotting a `Command` struct -- is what makes an accidental
        // change to the unconfined arm visible: a struct snapshot goes stale
        // the moment a field is added for the other arm.
        let _fd = fd_lock();
        let mut h = Harness::new("off-unchanged");
        let host_home = "/home/operator-under-test";
        let (spawned, _server) = h.spawn_mock(&["--serve"], &[], &[("HOME", host_home)]);
        let pid = spawned.pid();

        // 1. stdout and stderr are INHERITED. This is the property clause 9
        //    changes, and on a bare-DRM session these are descriptors on
        //    /dev/ttyN that no mount flag can revoke.
        for fd in [1, 2] {
            let child = fs::read_link(format!("/proc/{pid}/fd/{fd}")).expect("the child's fd");
            let ours = fs::read_link(format!("/proc/self/fd/{fd}")).expect("our fd");
            assert_eq!(
                child, ours,
                "at --isolation=off fd {fd} must still be the core's own; it is what                  --isolation=default replaces with a per-realm log file"
            );
        }
        // 2. stdin is /dev/null in both modes.
        assert_eq!(
            fs::read_link(format!("/proc/{pid}/fd/0")).expect("fd 0"),
            Path::new("/dev/null")
        );
        // 3. HOME is the HOST's, not a realm-private directory. `HOME` is
        //    reserved at config-load time in both modes -- a `realm.toml`
        //    whose validity depended on `--isolation` would load on one
        //    invocation and fail on the next -- so the core has to decide it
        //    here too, and at `--isolation=off` the honest decision is the
        //    operator's own home. The two arms differ in the VALUE, never in
        //    whether the app has one; an `off` realm with no `HOME` at all is
        //    neither the path that shipped before #186 nor a positive control
        //    for the confined path's `/vitrin/home`.
        assert_eq!(
            child_env_of(pid).get("HOME").map(String::as_str),
            Some(host_home),
            "at --isolation=off the realm must get the operator's own HOME"
        );
        // 4. The cwd is the realm's HOST runtime directory, not /run/vitrin.
        let cwd = fs::read_link(format!("/proc/{pid}/cwd")).expect("cwd");
        assert!(
            cwd.ends_with("vitrin-0/realm-0"),
            "the unconfined child's cwd moved: {cwd:?}"
        );
        h.reap(spawned);
    }

    #[test]
    fn the_confined_shim_holds_no_authority_it_was_not_handed() {
        // Everything here is read by the parent from `/proc`, which is the
        // whole point: a confinement claim is only as good as the reads
        // behind it, and these are the same reads the acceptance gate makes
        // from *inside* the realm.
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        with_confined_realm("confined-authority", |shim, facts| {
            // Zero capabilities. `execve` recomputes them, and with an
            // identity map there is no valid `make_kuid(ns, 0)` -- so the
            // root special case never fires and the drop in the helper makes
            // the property a step rather than a derivation.
            assert_eq!(
                proc_status(shim, "CapEff:").as_deref(),
                Some("0000000000000000"),
                "the confined shim holds effective capabilities"
            );
            assert_eq!(
                proc_status(shim, "CapPrm:").as_deref(),
                Some("0000000000000000")
            );
            assert_eq!(
                proc_status(shim, "CapBnd:").as_deref(),
                Some("0000000000000000"),
                "an empty bounding set is what stops a later execve regaining anything"
            );
            assert_eq!(proc_status(shim, "NoNewPrivs:").as_deref(), Some("1"));

            // The descriptor table, and the property is **not** "nothing above
            // fd 3": a running shim opens descriptors of its own, so a count
            // is a race rather than an assertion. What must hold is that no
            // descriptor the shim holds names a file outside the realm --
            // which is the actual escape, because after the `MNT_DETACH` a
            // leaked `O_DIRECTORY` handle on the old root is a complete pivot
            // escape (`openat(fd, "../..")` and `fchdir(fd)` both work
            // through one) and it is the only remaining handle on the host
            // tree.
            let entries: Vec<PathBuf> = fs::read_dir(format!("/proc/{shim}/fd"))
                .expect("the shim's fd table is readable")
                .flatten()
                .map(|e| e.path())
                .collect();
            assert!(
                entries
                    .iter()
                    .any(|e| e.file_name() == Some(OsStr::new("3"))),
                "fd 3 (the core connection) is gone: {entries:?}"
            );
            // fds 1 and 2 are the realm's log file, and asserting *that* is
            // the positive half of clause 9: on a bare-DRM session they would
            // otherwise be open descriptors on `/dev/ttyN`, which no mount
            // flag can revoke, and the `/dev` closure could not be claimed
            // honestly with them inherited. They are a host-tree descriptor
            // by design -- a regular file, so not a pivot escape -- and the
            // loop below therefore excludes stdio rather than pretending
            // nothing crosses.
            for fd in [1, 2] {
                let target = fs::read_link(format!("/proc/{shim}/fd/{fd}")).expect("the fd");
                assert!(
                    target.ends_with(REALM_LOG_NAME),
                    "fd {fd} is {target:?}, not the realm's log file"
                );
            }

            for entry in &entries {
                // stdio is core-decided and asserted above.
                if matches!(
                    entry.file_name().and_then(OsStr::to_str),
                    Some("0") | Some("1") | Some("2")
                ) {
                    continue;
                }
                let Ok(target) = fs::read_link(entry) else {
                    continue;
                };
                let target = target.to_string_lossy();
                // Anonymous descriptors name no path in the filesystem and
                // cannot be a pivot escape:
                //   * `socket:[N]`, `anon_inode:...`, `pipe:[N]` -- not
                //     absolute, so the first test catches them;
                //   * an unlinked file, which the kernel renders as
                //     `<path> (deleted)`. A `memfd` is exactly this -- it
                //     shows as `/memfd:wl_shm (deleted)` -- and a Wayland
                //     shim allocates them routinely, so this arm is load
                //     bearing rather than defensive.
                if !target.starts_with('/') || target.ends_with(" (deleted)") {
                    continue;
                }
                assert!(
                    fs::symlink_metadata(format!("/proc/{shim}/root{target}")).is_ok(),
                    "the shim holds {} on {target}, which does not exist inside the realm -- \
                     that is a descriptor that survived the pivot, and one is enough",
                    entry.display(),
                );
            }
            // And fd 0 is the realm's own /dev/null, not the config channel.
            let stdin = fs::read_link(format!("/proc/{shim}/fd/0")).expect("fd 0");
            assert_eq!(stdin, Path::new("/dev/null"), "fd 0 is {stdin:?}");

            // The writable set, from the child's own mountinfo read by the
            // parent -- the short sentence P2.6.9's gate will assert against.
            // mountinfo fields: id, parent, dev, root, MOUNTPOINT, OPTIONS,
            // then optional fields, then " - ". The per-mount options are
            // field 5 and the mountpoint field 4; reading the whole line for
            // the word "rw" would match the *superblock* options after the
            // separator, which say nothing about this mount.
            let mountinfo = fs::read_to_string(format!("/proc/{shim}/mountinfo"))
                .expect("the shim's mountinfo is readable");
            let mut writable: Vec<&str> = mountinfo
                .lines()
                .filter_map(|l| {
                    let mut f = l.split_whitespace();
                    let mountpoint = f.nth(4)?;
                    let options = f.next()?;
                    options.split(',').any(|o| o == "rw").then_some(mountpoint)
                })
                .collect();
            writable.sort_unstable();
            // The exact list, not a subset. An exhaustive assertion is what
            // makes a *new* writable mount fail this test instead of joining
            // it silently -- and the published claim
            // (`{/run/vitrin, /vitrin/home, /tmp, /dev/shm}`) is only honest
            // if the rest of this list is things that store nothing: procfs
            // and devpts are kernel filesystems, and the six device nodes are
            // borrowed inodes whose "writability" is `write(2)` to a driver.
            assert_eq!(
                writable,
                [
                    "/dev/full",
                    "/dev/null",
                    "/dev/pts",
                    "/dev/random",
                    "/dev/shm",
                    "/dev/tty",
                    "/dev/urandom",
                    "/dev/zero",
                    "/proc",
                    "/run/vitrin",
                    "/tmp",
                    "/vitrin/home",
                ],
                "the realm's writable set changed:\n{mountinfo}"
            );
            // The two that would be quietly catastrophic, called out by name.
            // `/` writable would let the app -- which runs as the mapped uid
            // -- replace the shim binary's own bind target; `/dev` writable
            // would falsify the published writable set through a mount nobody
            // thinks of as storage.
            assert!(!writable.contains(&"/"), "the realm's root is writable");
            assert!(!writable.contains(&"/dev"), "the realm's /dev is writable");

            // And what the JOURNAL will say is the same list -- because the
            // core measured it, from this same file, on this same spawn. It
            // was a hardcoded `&'static str` printed under `parent_observed`
            // until an adversarial review named that for what it was: a
            // sentence about a mount table nobody had read. Asserting the
            // equality here is what stops it drifting back into one.
            assert_eq!(
                facts.writable,
                WritableSet::Measured(writable.iter().map(|m| m.to_string()).collect()),
                "the journal's writable set is not the one the parent can measure right now"
            );

            // The device closure the limits page is about.
            for absent in ["/dev/input", "/dev/dri/card0", "/dev/kmsg"] {
                assert!(
                    fs::symlink_metadata(format!("/proc/{shim}/root{absent}")).is_err(),
                    "{absent} is reachable inside the realm"
                );
            }
            // Says "this closure reached its end". Nothing else can be
            // returned from here, so an early `return;` -- the silent skip
            // shape a closure hides from `skip-scan` -- is a type error.
            vitrin_skip::measured()
        });
    }

    #[test]
    fn killing_the_supervisor_takes_the_realm_down_with_it() {
        // The residual `lifecycle` used to publish, asserted closed: rung 3
        // SIGKILLs the process the core's `Child` names, PDEATHSIG takes PID
        // 1 with it, and the kernel's rule that a pid namespace dies with its
        // init takes everything else. No /proc walk, no supervision policy.
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let mut h = Harness::new("pdeathsig");
        let confinement = h.confinement();
        let canary = confinement.canaries[0].clone();
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let mut spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("the confined spawn must succeed");
        let shim = spawned
            .isolation()
            .shim_host_pid
            .expect("the shim's host pid");

        // Non-vacuity first: the shim is alive right now, so its later
        // absence is a death and not a process that never existed.
        assert!(
            fs::metadata(format!("/proc/{shim}")).is_ok(),
            "the shim was never running, so this test proves nothing"
        );

        let _ = spawned.child_mut().kill();
        let _ = spawned.child_mut().wait();

        // PDEATHSIG is delivered asynchronously, so poll rather than assume.
        wait_for("the shim to die with its supervisor", || {
            // `wait_for` panics with this description on timeout, which is the
            // assertion: a shim still present after the deadline is a shim
            // PDEATHSIG did not reach, and the orphan residual is back.
            (!Path::new(&format!("/proc/{shim}")).exists()).then_some(true)
        });
        let _ = fs::remove_file(&canary);
    }

    #[test]
    fn a_helper_that_does_not_speak_the_protocol_is_refused_and_leaves_no_process() {
        // A substituted `--realm-init`: a real, audit-passing, perfectly
        // working binary that is simply not this one. The refusal has to be a
        // refusal -- no realm, no directory, no surviving process -- rather
        // than a spawn that quietly produced an unconfined realm.
        let _fd = fd_lock();
        let mut h = Harness::new("substituted-helper");
        let mut confinement = h.confinement();
        // Non-vacuity: `/bin/cat` really does exec and really does pass the
        // trusted-writer audit, so a refusal here is the protocol's and not
        // the auditor's.
        confinement.realm_init = fs::canonicalize("/bin/cat").expect("/bin/cat exists");
        assert!(audit_program_at_spawn(&confinement.realm_init).is_ok());
        let canary = confinement.canaries[0].clone();

        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let err = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect_err("a helper that cannot speak the protocol must be refused");
        assert!(
            matches!(
                err.cause_class(),
                "helper_protocol" | "helper_died" | "helper_timeout" | "helper_version"
            ),
            "unexpected refusal class {}: {err}",
            err.cause_class()
        );
        // Fail closed: nothing left behind.
        assert!(
            !paths.realm_dir("realm-0").unwrap().exists(),
            "a refused spawn left its runtime directory behind"
        );
        let _ = fs::remove_file(&canary);
    }

    // -- the two clause-4 checkpoints, proved non-vacuous -------------------
    //
    // Both guards below used to be deletable with the suite staying green. An
    // adversarial review measured exactly that: replacing the canary loop
    // with `.iter().take(0)` and the `st_dev` comparison with `if false &&
    // ...` each left 5 passed, 0 failed. Two reasons, both now closed:
    // the journal's booleans were literal `Some(true)` rather than the
    // checks' results, and no test could put the core in a state where a
    // guard had anything to catch. What follows is the state.

    #[test]
    fn the_realms_last_words_cannot_repaint_the_operators_terminal() {
        // `realm.log` is inside the runtime directory the realm has bound
        // read-write at `/run/vitrin`, so the confined app can truncate it and
        // write anything; the core echoes its tail into its own stderr on a
        // bring-up failure. Bounded and labelled untrusted was not enough: an
        // escape sequence can clear the screen, reposition the cursor over
        // lines the core already printed, or set the window title, so a realm
        // that failed to start could forge output attributed to the core that
        // refused it.
        let hostile = "ld.so: not found\x1b[2J\x1b[1;1H\x1b]0;owned\x07vitrind: all fine\r\x07";
        let safe = sanitise_for_terminal(hostile);
        assert!(!safe.contains('\x1b'), "an ESC survived: {safe:?}");
        assert!(!safe.contains('\x07'), "a BEL survived: {safe:?}");
        assert!(!safe.contains('\r'), "a CR survived: {safe:?}");
        // Reversible, not redacted: an operator debugging a genuinely binary
        // crash dump can still see what the bytes were.
        assert!(safe.contains("\\u{1b}"), "{safe:?}");
        // And the two characters a linker error is unreadable without survive
        // untouched -- neither can reposition a cursor.
        assert_eq!(sanitise_for_terminal("a\nb\tc"), "a\nb\tc");
        assert!(safe.starts_with("ld.so: not found"), "{safe:?}");
    }

    #[test]
    fn a_helper_that_unshares_but_mounts_nothing_is_refused() {
        // The exact hole clause 4 is written against, as a running program:
        // `unshare-only-init` genuinely unshares all six namespaces, so every
        // P13 inode read passes and the maps are written -- and then it
        // mounts nothing at all. Six differing namespace inodes prove a
        // helper unshared; they prove nothing about what it built, and this
        // is the check that knows the difference.
        //
        // NON-VACUITY: the refusal must be `root_view` and not `ns_verify`.
        // A fixture that failed to unshare would be refused too, one
        // checkpoint earlier, and the test would pass for the wrong reason --
        // so the class is asserted by name rather than the mere failure.
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let mut h = Harness::new("unshare-only");
        let mut confinement = h.confinement();
        confinement.realm_init = fixture_bin("unshare-only-init");
        let canary = confinement.canaries[0].clone();
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let err = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect_err("a helper that mounted nothing must be refused");
        assert_eq!(
            err.cause_class(),
            "root_view",
            "the refusal came from the wrong checkpoint: {err}"
        );
        // The message, not just the class: with the `st_dev` comparison
        // removed this fixture is caught one step later by the canary loop,
        // which is also a `root_view`. A needle unique to the root-device
        // refusal is what keeps the two checkpoints' tests from covering for
        // each other -- measured, after exactly that.
        assert!(
            err.to_string().contains("never pivoted onto its own tree"),
            "the refusal came from the canary loop, not from the root-device check: {err}"
        );
        assert!(
            !paths.realm_dir("realm-0").unwrap().exists(),
            "a refused spawn left its runtime directory behind"
        );
        let _ = fs::remove_file(&canary);
    }

    #[test]
    fn a_canary_the_realm_really_can_reach_is_refused() {
        // The canary loop's own non-vacuity, and it needs no fixture: point a
        // canary at a path the realm's mount table *does* reproduce, with the
        // host's own inode behind it, and the core must refuse. `/usr` is
        // bound read-only at `/usr`, so any file under it is the same
        // (st_dev, st_ino) inside and out -- which is exactly the condition
        // the check is looking for.
        //
        // NON-VACUITY: delete the loop and this spawn succeeds.
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        vitrin_skip::skip_unless!(vitrin_skip::HOST_TOOLING, reachable_canary_present());
        let reachable = reachable_canary();
        let mut h = Harness::new("reachable-canary");
        let mut confinement = h.confinement();
        let scratch = confinement.canaries[0].clone();
        confinement.canaries.push(reachable.clone());
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let err = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect_err("a canary inside the realm's own /usr bind must refuse the spawn");
        assert_eq!(err.cause_class(), "root_view", "{err}");
        assert!(
            err.to_string().contains("/usr/bin/env") && err.to_string().contains("same inode"),
            "the refusal must name the path and the evidence: {err}"
        );
        let _ = fs::remove_file(&scratch);
    }

    #[test]
    fn a_stub_the_mount_table_had_to_create_is_not_a_reachable_canary() {
        // The other direction, and the reason the check compares inodes
        // rather than presence. `app_dir_to_bind` binds the app's containing
        // directory at the same in-realm path, and creating that target
        // `mkdir -p`s every ancestor onto the realm's own root tmpfs. So an
        // app under `$HOME` -- every development checkout, and every realm
        // the integration harness spawns -- materialises those ancestors
        // inside the realm as empty stubs.
        //
        // A pure-presence canary over any of them refused the spawn. That is
        // fail-closed and it is still an outage: `--isolation=default` is the
        // default, so it refused everything. The stub is a fresh inode on a
        // tmpfs this core's own helper mounted; the breach would be the
        // host's inode under the host's name, and `(st_dev, st_ino)` is what
        // separates them.
        //
        // NON-VACUITY: the canary is asserted to actually EXIST inside the
        // realm before the spawn is called a pass, so this cannot degrade
        // into "the path was absent anyway".
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let bin = mock_shim_bin();
        let app_dir = bin.parent().expect("the mock shim has a directory");
        vitrin_skip::skip_unless!(
            vitrin_skip::HOST_TOOLING,
            bindable_ancestor_present(app_dir)
        );
        // Re-derived rather than carried out of the guard: a guard that hands
        // back a value is a guard whose answer the test has observed, which
        // is the shape #288 closed.
        let stub = bindable_ancestor(app_dir).expect("the guard above said there is one");

        let mut h = Harness::new("stub-canary");
        let mut confinement = h.confinement();
        let scratch = confinement.canaries[0].clone();
        confinement.canaries.push(stub.clone());
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .unwrap_or_else(|e| {
            panic!(
                "a stub directory the mount table itself had to create ({}) must not be read \
                 as a reachable canary: {e}",
                stub.display()
            )
        });
        let shim = spawned
            .isolation()
            .shim_host_pid
            .expect("the shim's host pid");
        let inside = fs::symlink_metadata(format!("/proc/{shim}/root{}", stub.display()))
            .expect("the stub really is present inside the realm, or this test proves nothing");
        let outside = fs::symlink_metadata(&stub).expect("the stub exists on the host");
        assert_ne!(
            (inside.dev(), inside.ino()),
            (outside.dev(), outside.ino()),
            "the stub and the host directory are the same inode, so the realm really can reach \
             it and the spawn should have been refused"
        );
        assert_eq!(spawned.isolation().canaries_probed, 2);
        assert_eq!(spawned.isolation().canaries_unreachable, Some(true));
        h.reap(spawned);
        let _ = fs::remove_file(&scratch);
    }

    #[test]
    fn a_leaked_directory_descriptor_does_not_survive_the_second_execve() {
        // K13's own test. `close_range(4, ~0, CLOSE_RANGE_CLOEXEC)` is
        // defence in depth only for as long as every `open` in the helper
        // remembers `O_CLOEXEC` -- which is precisely the invariant the
        // syscall exists so that nothing has to depend on. Deleting the call
        // left the suite green, because nothing in the tree leaks.
        //
        // `leaks-a-dirfd-init` leaks one: an `O_DIRECTORY` handle on the
        // HOST ROOT, without `O_CLOEXEC`, and then `execve`s the real,
        // unmodified helper in its place. `fchdir` on that descriptor is a
        // complete pivot escape, and after the `MNT_DETACH` it is the only
        // remaining handle on the host tree.
        //
        // It is also the case a name-based assertion cannot catch: the
        // descriptor reads back as `/`, and `/` exists inside the realm too.
        // So the comparison is `(st_dev, st_ino)` against the host root --
        // the realm's root is a tmpfs this helper mounted, so the two can
        // never legitimately coincide.
        //
        // NON-VACUITY: the host root's identity is read first and the shim's
        // fd table is asserted non-empty, so "no descriptor matched" cannot
        // mean "there was nothing to look at".
        let _fd = fd_lock();
        vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
        let host_root = fs::metadata("/").expect("the host root stats");
        let host_root = (host_root.dev(), host_root.ino());

        let mut h = Harness::new("leaked-dirfd");
        let mut confinement = h.confinement();
        confinement.realm_init = fixture_bin("leaks-a-dirfd-init");
        let canary = confinement.canaries[0].clone();
        let bin = mock_shim_bin();
        let realm = realm_with_spawn("realm-0", &bin, &["--serve".to_string()], &[]);
        let paths = h.paths().confined(confinement);
        let spawned = spawn_realm_with_env(
            &realm,
            &paths,
            &mut h.recorder,
            SpawnOrigin::Startup,
            |_| None,
        )
        .expect("the wrapper execs the real helper, so the realm must still come up");
        let shim = spawned
            .isolation()
            .shim_host_pid
            .expect("the shim's host pid");

        let entries: Vec<PathBuf> = fs::read_dir(format!("/proc/{shim}/fd"))
            .expect("the shim's fd table is readable")
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(
            entries.len() >= 4,
            "the shim's fd table is empty, so this test looked at nothing: {entries:?}"
        );
        for entry in &entries {
            let Ok(st) = fs::metadata(entry) else {
                continue;
            };
            assert_ne!(
                (st.dev(), st.ino()),
                host_root,
                "{} is a descriptor on the HOST ROOT inside the confined shim. fchdir on it \
                 walks straight out of the realm; K13's close_range is what is supposed to \
                 have closed it",
                entry.display()
            );
        }
        h.reap(spawned);
        let _ = fs::remove_file(&canary);
    }

    #[test]
    fn the_app_directory_bind_refuses_the_trees_it_exists_to_hide() {
        // Binding `/`, `/home` or the operator's own home would undo the
        // table rather than extend it, so the refusal is at the point the
        // bind would be chosen.
        let home = Some("/home/op".to_string());
        for app in ["/init", "/home/thing", "/home/op/app"] {
            let err = app_dir_to_bind(Path::new(app), &[], home.clone())
                .expect_err("{app} must be refused");
            assert_eq!(err.cause_class(), "mount_table", "{app}: {err}");
        }
        // Non-vacuity in both directions: something under /usr needs no bind
        // at all, something elsewhere gets its own directory, and something
        // already covered by an operator bind is skipped.
        assert_eq!(
            app_dir_to_bind(Path::new("/usr/bin/foot"), &[], home.clone()).ok(),
            Some(None)
        );
        assert_eq!(
            app_dir_to_bind(Path::new("/opt/app/bin/app"), &[], home.clone()).ok(),
            Some(Some(PathBuf::from("/opt/app/bin")))
        );
        assert_eq!(
            app_dir_to_bind(
                Path::new("/opt/app/bin/app"),
                &[PathBuf::from("/opt/app")],
                home
            )
            .ok(),
            Some(None)
        );
    }

    #[test]
    fn a_bind_source_is_audited_exactly_as_the_program_is() {
        // The owner's decision put `binds` in realm.toml beside the app, so
        // that file now carries confinement-relevant configuration -- and a
        // bind source holding a library decides what the confined app RUNS,
        // not merely what it reads.
        let dir = scratch();
        let good = dir.join("closure");
        fs::create_dir(&good).unwrap();
        fs::set_permissions(&good, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            audit_bind_source_at_spawn(&good).ok(),
            Some(fs::canonicalize(&good).unwrap())
        );

        // World-writable: whoever can write it chooses what the realm reads.
        let bad = dir.join("world-writable");
        fs::create_dir(&bad).unwrap();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            audit_bind_source_at_spawn(&bad)
                .map_err(|e| e.cause_class())
                .unwrap_err(),
            "program_audit"
        );

        // Relative, and absent, each with their own answer.
        assert_eq!(
            audit_bind_source_at_spawn(Path::new("closure"))
                .map_err(|e| e.cause_class())
                .unwrap_err(),
            "relative_command"
        );
        assert_eq!(
            audit_bind_source_at_spawn(&dir.join("absent"))
                .map_err(|e| e.cause_class())
                .unwrap_err(),
            "program_audit"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_confinement_fault_has_its_own_label() {
        // The recorder's `cause_class` contract: a closed vocabulary a reader
        // can switch on, with no two faults sharing a token and none of them
        // colliding with the pre-existing set.
        let faults = [
            ConfinementFault::RealmInitAudit,
            ConfinementFault::StorageDir,
            ConfinementFault::HelperVersion,
            ConfinementFault::HelperProtocol,
            ConfinementFault::HelperTimeout,
            ConfinementFault::HelperDied,
            ConfinementFault::UnshareDenied,
            ConfinementFault::UnshareAbsent,
            ConfinementFault::NsVerify,
            ConfinementFault::SetgroupsWrite,
            ConfinementFault::UidMapWrite,
            ConfinementFault::GidMapWrite,
            ConfinementFault::MapVerify,
            ConfinementFault::MountTable,
            ConfinementFault::PivotRoot,
            ConfinementFault::RootView,
            ConfinementFault::ExecShim,
        ];
        let labels: BTreeSet<&str> = faults.iter().map(|f| f.cause_class()).collect();
        assert_eq!(labels.len(), faults.len(), "two faults share a label");
        for pre_existing in [
            "invalid_realm_id",
            "runtime_dir",
            "realm_busy",
            "program_audit",
            "relative_command",
            "reserved_env",
            "socketpair",
            "exec",
        ] {
            assert!(
                !labels.contains(pre_existing),
                "{pre_existing} is already a spawn cause_class"
            );
        }
    }

    #[test]
    fn a_helper_stage_maps_to_a_cause_class_that_tells_the_two_unshare_answers_apart() {
        // "absent" and "restricted" are different answers because the
        // operator's remedy differs: a missing CONFIG_USER_NS needs a
        // different kernel, an AppArmor restriction needs one sysctl.
        assert_eq!(
            from_helper_stage(Stage::Unshare, libc::EPERM).cause_class(),
            "unshare_denied"
        );
        assert_eq!(
            from_helper_stage(Stage::Unshare, libc::EINVAL).cause_class(),
            "unshare_absent"
        );
        assert_eq!(
            from_helper_stage(Stage::Version, libc::EPROTO).cause_class(),
            "helper_version"
        );
        assert_eq!(
            from_helper_stage(Stage::Mount, libc::EACCES).cause_class(),
            "mount_table"
        );
        assert_eq!(
            from_helper_stage(Stage::Exec, libc::ENOENT).cause_class(),
            "exec_shim"
        );
        // And the remedy copy for a denied unshare names the knob an operator
        // would actually change, rather than shrugging.
        let denied = from_helper_stage(Stage::Unshare, libc::EPERM).to_string();
        assert!(denied.contains("apparmor_restrict"), "{denied}");
    }

    #[test]
    fn realm_storage_is_reused_and_says_so_rather_than_being_purged() {
        // Keyed on realm id and never purged: a realm whose `command` changed
        // inherits the old app's HOME. Refusing would make a routine
        // /usr/bin -> /usr/local/bin move a hard failure for no security
        // gain, so it is warned and journaled instead.
        let dir = scratch();
        let root = dir.join("storage");
        let (first, reused) = prepare_realm_storage(&root, "realm-0").expect("first");
        assert!(!reused, "a fresh storage directory is not a reused one");
        assert!(first.is_dir());
        fs::write(first.join("state"), b"the app's data").unwrap();

        let (second, reused) = prepare_realm_storage(&root, "realm-0").expect("second");
        assert!(reused, "the second spawn must see the reuse");
        assert_eq!(first, second);
        assert!(
            second.join("state").exists(),
            "storage is the realm's data, not the attempt's: a refused or repeated spawn must \
             not delete it"
        );
        assert_eq!(
            fs::metadata(&second).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_confined_environment_names_the_in_realm_paths_and_nothing_of_the_host() {
        // `child_env`'s confined shape: the injected values are the fixed
        // in-realm paths, which is also what removes the `sun_path` 108-byte
        // pressure a long realm id creates under the host spelling.
        let spawn_config = crate::realm::tests::spawn_config_with(
            Path::new("/usr/bin/true"),
            &[],
            &["LANG".to_string()],
        );
        let env = child_env(
            &spawn_config,
            Path::new(IN_REALM_WAYLAND_SOCKET),
            Path::new(IN_REALM_RUNTIME_DIR),
            Some(Path::new(IN_REALM_HOME)),
            |name| (name == "LANG").then(|| "en_US.UTF-8".to_string()),
        );
        let map: BTreeMap<String, String> = env
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(map.get("WAYLAND_DISPLAY").unwrap(), "/run/vitrin/wayland-0");
        assert_eq!(map.get("XDG_RUNTIME_DIR").unwrap(), "/run/vitrin");
        assert_eq!(map.get("HOME").unwrap(), "/vitrin/home");
        assert_eq!(map.get("LANG").unwrap(), "en_US.UTF-8");
        assert!(map.len() < 108, "sanity: this is a map, not a path");
        // The unconfined shape injects the HOST's home, and the two paths
        // therefore differ in the *value* of `HOME` rather than in whether
        // the app has one. `HOME` is reserved at load time in both modes
        // (config validity must not depend on a CLI flag), so if the
        // unconfined arm injected nothing an `--isolation=off` realm would
        // get no `HOME` at all -- which is not the path that shipped before
        // #186 and not a positive control for anything.
        let unconfined = child_env(
            &spawn_config,
            Path::new("/run/user/1000/vitrin-0/realm-0/wayland-0"),
            Path::new("/run/user/1000/vitrin-0/realm-0"),
            Some(Path::new("/home/operator")),
            |_| None,
        );
        let unconfined: BTreeMap<String, String> = unconfined
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(unconfined.get("HOME").unwrap(), "/home/operator");
        // And a core with no `HOME` of its own passes none on: the injection
        // is "whatever the core decided", never a fabricated default.
        let homeless = child_env(
            &spawn_config,
            Path::new("/run/user/1000/vitrin-0/realm-0/wayland-0"),
            Path::new("/run/user/1000/vitrin-0/realm-0"),
            None,
            |_| None,
        );
        assert!(homeless.iter().all(|(k, _)| k != "HOME"));
    }

    /// The Landlock audit diagnostic is forwarded **only** when the core's own
    /// environment asked for it, and it is never part of a realm's composed
    /// environment.
    ///
    /// Two properties, and the second is the one a reviewer should look for.
    /// It is not in [`child_env`] at all -- a realm's environment is a
    /// default-deny allowlist and this is an operator's instrument, so it goes
    /// on the helper's `Command` at the confined call site and nowhere else.
    /// And no `realm.toml` allowlist can conjure it: `child_env` copies
    /// `env_allow` names out of the core's environment, so the negative half
    /// below runs with the name *present in the core's environment* and a
    /// realm that explicitly allow-lists it, and still asserts that
    /// `child_env` does not carry it.
    #[test]
    fn the_landlock_audit_diagnostic_is_forwarded_only_when_the_core_was_asked() {
        assert_eq!(
            landlock_audit_env(|name| (name == LANDLOCK_AUDIT_ENV).then(|| "1".to_string())),
            Some((OsString::from(LANDLOCK_AUDIT_ENV), OsString::from("1"))),
        );
        // Anything else is nothing forwarded. The helper would refuse these
        // too, but the core must not hand a second spelling down to be parsed.
        for value in ["", "0", "true", "yes", "on", "2"] {
            assert_eq!(
                landlock_audit_env(|name| (name == LANDLOCK_AUDIT_ENV).then(|| value.to_string())),
                None,
                "{value:?} in the core's environment forwarded the diagnostic into a realm"
            );
        }
        assert_eq!(
            landlock_audit_env(|_| None),
            None,
            "the shipped path: unset in the core's environment, unset in the helper's"
        );

        // And the composed realm environment never carries it, even when the
        // core has it set AND the realm allow-lists the name.
        let spawn_config = crate::realm::tests::spawn_config_with(
            Path::new("/usr/bin/true"),
            &[],
            &[LANDLOCK_AUDIT_ENV.to_string()],
        );
        let env = child_env(
            &spawn_config,
            Path::new(IN_REALM_WAYLAND_SOCKET),
            Path::new(IN_REALM_RUNTIME_DIR),
            Some(Path::new(IN_REALM_HOME)),
            |name| (name == LANDLOCK_AUDIT_ENV).then(|| "1".to_string()),
        );
        assert!(
            env.iter().all(|(k, _)| k != LANDLOCK_AUDIT_ENV),
            "the diagnostic reached a realm's composed environment through `env_allow`; it \
             belongs to the helper's Command at the confined call site, and nowhere a \
             realm's own configuration can name"
        );
    }
}
