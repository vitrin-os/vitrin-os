// SPDX-License-Identifier: MPL-2.0
//! The wire shape `vitrind` and `vitrin-realm-init` share (P2.6.2, issue
//! #186, D-036), and nothing else.
//!
//! Both sides import these types. A hand-transcribed second copy of a frame
//! layout is exactly the cost D-022 records for the Python SDK's enum, and
//! this channel is the one the realm's confinement is negotiated over, so a
//! silent divergence between the two spellings would be a divergence between
//! what the core believes it asked for and what the helper built.
//!
//! # Why a helper binary exists at all
//!
//! `unshare(2)` mutates the caller. The core may not confine itself as a side
//! effect of confining a realm, so the namespaces have to be created by
//! *some* other process. The choice of **which** other process is D-036(1),
//! and it is the inverse of the shape the plan row implied:
//!
//! - **`execve` first, `unshare` second.** The core `spawn`s this binary with
//!   `std::process::Command` -- the ordinary path, with the existing
//!   `pre_exec` closure unchanged -- and *this* process unshares itself once
//!   it is running. Cloning into namespaces from the core is not implementable
//!   while the core keeps `Command`, and the reason is a deadlock rather than
//!   a preference: D-020(1) requires the parent to write the id maps while the
//!   child blocks, and std's own fork path has the parent blocked reading its
//!   exec-report pipe until `execve` closes it.
//! - **It is also the stronger posture, which is why it is a decision and not
//!   an implementation detail.** `execve` recomputes capabilities. A process
//!   that unshared a user namespace with no map written has an unmapped uid,
//!   so it is not namespace-root and would leave `execve` with an empty
//!   permitted set -- which forces the map to be written *before* that
//!   `execve`, and forces it to be `0 <euid> 1`, handing the confined app
//!   `CAP_SYS_ADMIN` inside its own user namespace. Unsharing *after* this
//!   binary's `execve` avoids both: no `execve` intervenes between `unshare`
//!   and the mount work, so this process keeps its namespace-local
//!   capabilities, and the map can be an **identity** map. The app therefore
//!   ends up with **zero capabilities** and cannot reshape its own
//!   confinement.
//! - **And it deletes a window.** No shape where a child blocks inside the
//!   fork window leaves a copy-on-write image of the core's address space --
//!   the clipboard slot, the grant table, principal keys -- sitting in a
//!   not-yet-`exec`'d process readable through `/proc/<pid>/mem` by any
//!   same-uid process.
//!
//! # Why there are two processes on this side of the fork
//!
//! `unshare(CLONE_NEWPID)` does not move the caller. It sets
//! `pid_ns_for_children`, so the *next* `fork` produces PID 1 -- measured on
//! this kernel, not read: after `unshare(CLONE_NEWPID)` the caller's own
//! `/proc/self/ns/pid` is still the core's. So this binary forks:
//!
//! - the **supervisor** stays in the host PID namespace and is the process
//!   the core's [`std::process::Child`] names;
//! - the **PID-1 child** builds the mount table and `execve`s the shim.
//!
//! That fork is not merely forced, it pays for itself: with
//! `PR_SET_PDEATHSIG = SIGKILL` on PID 1 and the kernel's rule that a PID
//! namespace dies with its init, the orphan residual `vitrin-core`'s
//! `lifecycle` module used to publish -- "rung 3 kills the shim and its app
//! is reparented to init" -- is closed. A fresh `proc` mount also requires
//! the mounter to be in a new PID namespace, so the mount table could not be
//! built before the fork in any case.
//!
//! ## One correction to the record, because a false reason is worse than none
//!
//! The verification the core performs never reads
//! `/proc/<pid>/ns/pid_for_children`, and an earlier draft of this design
//! justified that by citing `proc(5)`'s statement that the file reads back
//! empty until the first child exists. **That is not what this kernel does**
//! (measured on 7.1.8: the link is populated immediately after the
//! `unshare`). The justification is therefore *not* "it would be empty". It
//! is that `pid_for_children` describes a namespace the reader is not in, so
//! a difference there proves only that a namespace was *requested*; the
//! PID-1 child's own `/proc/<host pid>/ns/pid` is the unambiguous fact, and
//! it is the one the core reads.
//!
//! # The invariant this channel exists to serve
//!
//! > `applied` is computed from what the kernel says about the child, never
//! > from the flag that was requested, the path that was resolved, or the
//! > table that was sent.
//!
//! Nothing in the frames below is evidence of confinement. [`Frame::Mounted`]
//! carries a count and a fingerprint the *child* computed, and the core
//! journals both as `child-asserted` for exactly that reason. What licenses
//! the spawn is the core's own reads of `/proc/<pid>/ns/*`,
//! `/proc/<pid>/root` and the canary set -- see `vitrin_core::spawn`.

pub mod seccomp;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the core will hold the compositor waiting for this helper to
/// report a built realm, end to end (D-036(11)).
///
/// **250 ms, and the number is not free.** Since WS-E.1.1 `launch` is
/// reachable from an admitted `realm_launch`, so a wire-reachable verb now
/// blocks the compositor for the length of this handshake. `lifecycle` draws
/// the line explicitly for the *death* path ("nothing in the death-detection
/// path blocks or sleeps, so a dying realm can never park the compositor");
/// the birth path is bounded here instead, at exactly the shortest dead-man
/// hold the off-switch will accept -- so a realm being born can never
/// out-wait the switch that would kill it. `vitrin_core::spawn` holds that
/// equality with a compile-time assertion against `deadman::MIN_HOLD`; this
/// constant may not be raised without moving that floor first.
pub const HELPER_DEADLINE: Duration = Duration::from_millis(250);

/// The realm's private runtime directory, *inside* the realm: a bind of the
/// host directory the core created, purged and `flock`ed before the fork.
///
/// Short and fixed on purpose. It removes the `sun_path` 108-byte pressure a
/// long realm id creates under the host spelling, and it makes the app's
/// `XDG_RUNTIME_DIR` a constant rather than a path that leaks the host tree's
/// shape.
pub const IN_REALM_RUNTIME_DIR: &str = "/run/vitrin";

/// The app-facing Wayland socket, inside the realm. `wl_display_add_socket`
/// takes an absolute name verbatim, so the shim binds exactly this.
pub const IN_REALM_WAYLAND_SOCKET: &str = "/run/vitrin/wayland-0";

/// **Reserved** (D-020(4)): the path P2.1.3's per-realm accessibility bus
/// daemon will bind, and the only thing that may ever be bound there.
///
/// It is a name plus an ordering, not a mount. The directory it sits in is
/// the core-created realm runtime directory bound at
/// [`IN_REALM_RUNTIME_DIR`], so P2.1.3 adds **no bind mount** and re-makes
/// **no** confinement claim -- which is what the reservation was written for.
///
/// The residual is published rather than papered over: within one realm the
/// app can `unlink` and rebind any socket in [`IN_REALM_RUNTIME_DIR`],
/// including `wayland-0` and this path. Mode bits cannot stop it (the shim
/// and the app are one uid inside a single-id map), the blast radius is that
/// one realm, and the closure is P2.6.3's shim-side Landlock stack.
pub const IN_REALM_A11Y_BUS: &str = "/run/vitrin/a11y-bus";

/// The directory the core owns inside the realm's root: the shim binary and
/// the realm's private storage hang off it.
///
/// A single core-chosen prefix rather than two unrelated paths, so "what did
/// the core put in this realm's filesystem" has a one-line answer an operator
/// can check with `ls`.
pub const IN_REALM_PREFIX: &str = "/vitrin";

/// The shim binary, inside the realm -- a read-only, `nosuid`, `nodev` file
/// bind of the host shim the core audited.
///
/// It has to be bound at all because in a development tree the shim lives in
/// `target/debug` under `$HOME`, which is the exact tree this task exists to
/// hide from the realm.
///
/// # Why the basename is `vitrin-shim` and not `shim` (issue #283)
///
/// The kernel takes a process's `comm` from the **basename of the file it
/// `execve`d**, and this constant is that basename for every confined shim.
/// Binding at `/vitrin/shim` therefore made `ps` report `shim` -- a name
/// neither binary owned. Nine integration gates read `comm` to prove a run
/// was mock-free, spelled `comm_of(shim).startswith("vitrin-shim")`, and
/// under confinement that assertion went **red for the real shim**, because
/// `"shim"` does not start with `"vitrin-shim"`. It did not go green for
/// `vitrin-mock-shim` either, confined or not: unconfined the kernel
/// truncates that basename to `vitrin-mock-shi`, which fails the same test.
/// The check had stopped identifying anything at all, which is why those
/// gates now compare `(st_dev, st_ino)` of `/proc/<pid>/exe`.
///
/// **This rename gives `comm` no evidence value back; it takes the last of
/// it away.** `main.rs` binds whatever host binary the core named *to this
/// constant*, so a confined `vitrin-mock-shim` answers to `vitrin-shim` too
/// -- measured 2026-08-19 against a `--isolation=default` core started with
/// the mock: `comm` `vitrin-shim`, `/proc/<pid>/exe -> /vitrin/vitrin-shim`,
/// inode the mock's. A `startswith("vitrin-shim")` check that was red for
/// both binaries is now green for both. What the name buys is `ps`
/// legibility for a human reading a process tree, and nothing else. The test
/// `a_confined_comm_is_decided_by_the_bind_target_not_the_host_binary` below
/// holds that reading to `main.rs` rather than to this paragraph.
///
/// The in-realm path is a **core-chosen constant**, not the host basename, so
/// this is not a claim about what the operator called their binary.
pub const IN_REALM_SHIM: &str = "/vitrin/vitrin-shim";

/// The realm's private storage, inside the realm, and the value of its
/// `HOME`.
///
/// Disk-backed and realm-writable. Two published costs travel with it: it
/// carries **no quota** (a project quota needs `prjquota` on the filesystem
/// and is out of scope), and it is keyed on realm id and never purged, so a
/// realm whose `command` changes inherits the old app's `HOME`.
pub const IN_REALM_HOME: &str = "/vitrin/home";

/// The highest Landlock ABI rung **this build knows how to request**
/// (P2.6.3, issue #187).
///
/// Not the highest rung that exists: ABI 10 is in mainline and this build
/// does not request it, because a build must not name a constant its own
/// headers do not define. A kernel reporting a higher number than this is
/// **clamped down to it and reported as clamped** -- see [`plan_rung`], which
/// is asserted against a constructed probe value rather than left waiting for
/// such a kernel to exist.
///
/// It lives here rather than in the helper's `landlock` module because three
/// sides need it: the helper to clamp, the core to refuse an
/// `--landlock=abi:N` above the ladder it could possibly walk, and
/// `vitrind --print-floor` to print it -- a build constant nothing prints is
/// a clamp an operator cannot see coming.
pub const LANDLOCK_BUILD_MAX_RUNG: u32 = 9;

/// The lowest Landlock ABI this build will **start** on (owner's decision,
/// 2026-08-15). A kernel below it is refused, not degraded.
///
/// # What this replaces, and what it gives up
///
/// P2.6.3 (#187) was written around a *degradation ladder*: ask for the highest
/// rung, fall back a rung at a time, publish a generated per-ABI table so each
/// rung's absence is a measured row rather than a sentence. The ladder's
/// mechanism is still here -- it is what makes `--landlock=abi:N` a measurement
/// instrument -- but it is no longer how a **shipped session** meets a kernel.
/// A session either gets a domain at this number or above, or it does not
/// start.
///
/// **The floor narrowed #187 rather than completing it**, and what completed it
/// was other work plus a dated decision — P2.6.3 was accepted on 2026-08-19; see
/// `docs/plan/02-phase-2-semantic-epochs.md` (P2.6.3, Correction 7) and D-044.
/// PRD §20's "coverage is kernel-dependent" caveat is answered **for the five
/// kernels #281 booted and for no others**: this build targets recent kernels
/// and says so, instead of claiming a spectrum it never measured.
///
/// A generated multi-rung table with a CI staleness gate now exists at the
/// narrowed scope -- `docs/book/src/isolation-matrix.md`, emitted by `cargo
/// xtask isolation-matrix`, which **reads this constant out of this file** and
/// goes red when the checked-in page no longer prints the number declared here.
/// Re-tuning it is therefore a regeneration, not an edit. What that table is
/// not is a per-kernel measurement: it probes nothing. The per-kernel row set
/// landed separately as `docs/book/src/isolation-kernels.md` (#281, five
/// kernels booted under QEMU), and the original criteria's "one row per ABI
/// actually reported, on each kernel in the CI matrix" was found unsatisfiable
/// by any byte-stable checked-in page and replaced rather than met. See
/// `docs/plan/02-phase-2-semantic-epochs.md` (P2.6.3, Corrections 4, 5, 6 and
/// 7) and `docs/book/src/limits.md`.
///
/// # Why 6 and not 7, and why lowering it took no enforcement away
///
/// The floor was **7** from 2026-08-15 until it was lowered to **6** on
/// 2026-08-16 (owner's decision, taken for a VPS running Debian 13). The
/// lowering is not a relaxation of what a realm gets, and the reason is
/// mechanical rather than a judgement call: **the two rungs between 6 and 8 buy
/// `landlock_restrict_self` FLAGS, not `handled_access_fs` mask bits, and this
/// build passes flags = 0 in every shipped run.** See `landlock.rs`'s module
/// docs ("What a 'rung' is, mechanically", point 2) and
/// `landlock::restrict_self_flags`'s own documentation: rung 7 buys the
/// audit-log flags and rung 8 buys `LANDLOCK_RESTRICT_SELF_TSYNC`. The enforced
/// domain -- `handled_access_fs`, `scoped` and the flags word together -- is
/// therefore **byte-identical at rungs 6, 7 and 8**.
///
/// **Rung 9 is NOT in that identity, and the distinction has to be kept.** Rung
/// 9 adds `LANDLOCK_ACCESS_FS_RESOLVE_UNIX` to `handled_access_fs`, so its
/// domain really is a superset. Lowering the floor still costs nothing, because
/// **the floor decides admission, not which rung is applied**: the rung a realm
/// gets is `min(kernel ABI, LANDLOCK_BUILD_MAX_RUNG)` either way, so a machine
/// that could supply rung 9 still gets rung 9, and every machine that started
/// under the old floor gets the identical domain under the new one. What
/// changed is only which machines are refused. The identity, and its
/// non-vacuity -- rung 5's domain differs, which is why 6 and not 5 is the
/// lowest floor that costs nothing -- are asserted in
/// `crates/vitrin-realm-init/src/main.rs`'s
/// `the_floor_costs_nothing_because_the_domain_is_flat_from_six_to_eight`.
///
/// # Which kernels that admits and refuses, measured
///
/// Since 2026-08-16 this is a **measurement** rather than a bound this
/// repository declined to state. Five distribution kernels were booted under
/// QEMU with the shipped `vitrind` in a minimal initramfs
/// (`tests/kernel-matrix/`, rows in `tests/kernel-matrix/rows/`, published as
/// `docs/book/src/isolation-kernels.md`):
///
/// | kernel | `landlock.abi` | at this floor |
/// |---|---|---|
/// | 5.15.0-191-generic (Ubuntu 22.04) | 1 | refused |
/// | 6.1.0-50-amd64 (Debian 12) | 2 | refused |
/// | 6.8.0-139-generic (Ubuntu 24.04 GA) | 4 | refused |
/// | 6.12.101+deb13-amd64 (Debian 13) | 6 | **admitted** |
/// | 6.17.0-1020-azure (the CI runner's kernel) | 7 | **admitted** |
///
/// Debian 13 is the row the decision was taken for. Those are **kernel** rows
/// and not distribution rows -- the same vmlinuz under a distribution's own
/// policy answers differently on the policy cells, which is why the page keeps
/// the two vocabularies apart.
///
/// What is stated is the rule itself: below this number `vitrind` refuses to
/// start and names the requirement. `--landlock=off` starts a session whose
/// realms get no ruleset at all, and is not a remedy for a kernel that could be
/// upgraded.
pub const LANDLOCK_MIN_ABI: u32 = 6;

/// The **diagnostic** that asks the kernel to keep logging a realm's Landlock
/// denials **after** the shim's `execve` (P2.6.3 follow-up).
///
/// Landlock ABI 7 added a `flags` word to `landlock_restrict_self`, and its
/// default is the confinement-correct one and the observability-hostile one:
/// denials are audited for the *current* execution and go silent across
/// `execve`. Every denial worth measuring in this project happens on the far
/// side of that `execve` -- it is the app's denials that decide whether the
/// enumerated read set is complete -- so with the default flags word the one
/// question P2.6.9 has to answer is unanswerable from the kernel's own record.
/// `LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON` turns it back on.
///
/// # Why an environment variable, and why it is not a `vitrind` flag
///
/// A `--landlock-audit` flag would be a *shipped* switch over what the kernel
/// records about a realm, on a binary whose whole argument is that confinement
/// does not have modes an operator can be talked into. This is an instrument
/// for the person measuring the read set, and it is spelled like one: the core
/// forwards it into the helper's environment **only** when its own environment
/// carries it (see `vitrin_core::spawn`), and the helper acts on it only for
/// the exact value [`landlock_audit_requested`] accepts. Nothing in
/// `realm.toml`, no command line and no default can reach it.
///
/// What it does **not** change: the ruleset. `handled_access_fs`, `scoped` and
/// every rule are identical whether or not it is set -- the flag decides what
/// the kernel *writes down*, never what it permits.
pub const LANDLOCK_AUDIT_ENV: &str = "VITRIN_LANDLOCK_AUDIT";

/// Is the [`LANDLOCK_AUDIT_ENV`] diagnostic asked for?
///
/// **Exactly `"1"`, and nothing else.** Not "non-empty", not "anything but
/// 0", not a case-insensitive `true`: a diagnostic that changes what the
/// kernel logs about a confined process must be impossible to switch on by
/// accident, and a permissive parse is how an inherited `...AUDIT=false` ends
/// up meaning yes. One predicate, used by both the core (deciding whether to
/// forward it) and the helper (deciding whether to set the flag), so the two
/// cannot drift into disagreeing about what "on" is.
pub fn landlock_audit_requested(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Which Landlock rung this session asks the helper to apply, and the
/// **only** knob that can weaken it (P2.6.3, issue #187).
///
/// # Why a shipped flag rather than a cargo feature
///
/// The rungs above the floor are the ones whose absence has to be
/// *measured*: a rung whose weakening can only be described in prose is a
/// rung nobody can check. Simulating an older kernel is exactly what a cap on
/// `handled_access_fs` does -- a Landlock rung *is* which bits are legal in
/// that mask -- so capping the requested mask to rung N reproduces an ABI-N
/// kernel on a newer one.
///
/// A cargo feature was available and is refused: CI's helper would then not
/// be the shipped helper, and this repo's acceptance rule says a milestone
/// closes on the shipped binaries. So the knob ships, and pays for itself by
/// being the thing the per-rung tests drive.
///
/// **The cap is never a floor.** A bit above the kernel's own ABI is refused
/// `EINVAL` at ruleset creation, so [`LandlockRequest::CappedAt`] above what
/// the kernel grants cannot raise anything -- the helper still walks down to
/// what the kernel accepts and journals what it obtained.
///
/// # It is a dial, not a one-way weakening, and the difference is measured
///
/// An earlier draft of this doc claimed "the cap can only ever weaken". That
/// is **false**, and the counterexample is `REFER` (rung 2): a Landlock
/// domain denies reparenting -- `rename(2)`/`link(2)` across directories --
/// whenever the ruleset does not *handle* `REFER`, so a rung-1 domain forbids
/// it even inside the realm's own writable storage while rungs 2 and above
/// permit it. Measured on this repo's own box (kernel `7.1.8-arch1-3`,
/// Landlock ABI 9, 2026-08-14), granting the realm's whole writable set on
/// one hierarchy and renaming a file from one subdirectory to another:
/// **rung 1 answers `EXDEV`; rungs 2 through 9 succeed**. A same-directory
/// rename succeeds at every rung, so the denial is reparenting specifically.
/// `crates/vitrin-realm-init/src/main.rs`'s
/// `rung_one_forbids_reparenting_that_the_rung_above_permits` re-measures
/// exactly that.
///
/// So `--landlock=abi:1` is **stricter** than `--landlock=highest` for
/// reparenting and weaker for everything else (no `TRUNCATE`, no `IOCTL_DEV`,
/// no scoping, no `RESOLVE_UNIX`). What the flag guarantees is narrower than
/// "weaker" and is what the rest of this type is built on: the rung requested
/// is the rung whose `handled_access_fs` mask is used, which reproduces an
/// ABI-N kernel exactly -- including its stricter reparenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandlockRequest {
    /// The highest rung this build knows that this kernel will accept.
    Highest,
    /// No higher than this ABI rung. A **cap**, never a floor.
    CappedAt(u32),
    /// Apply no ruleset at all.
    ///
    /// Strictly weaker than [`LandlockRequest::Highest`] and named on the
    /// command line, on the `--isolation=off` precedent: the binary already
    /// ships a switch that turns *all* confinement off, so a switch that
    /// turns one mechanism off adds no new worst case. It exists so the
    /// per-rung gates have their positive control -- an absence is only
    /// evidence if the same run proves the thing was present somewhere.
    Off,
}

impl LandlockRequest {
    /// Parse the `--landlock` value.
    pub fn parse(value: &str) -> Result<LandlockRequest, String> {
        if value == "off" {
            return Ok(LandlockRequest::Off);
        }
        if value == "highest" {
            return Ok(LandlockRequest::Highest);
        }
        if let Some(n) = value.strip_prefix("abi:") {
            let rung: u32 = n
                .parse()
                .map_err(|_| format!("`--landlock=abi:{n}` is not a number"))?;
            if rung == 0 {
                return Err(
                    "`--landlock=abi:0` names no rung: Landlock ABI versions start at 1, and \
                     `--landlock=off` is how a session asks for no ruleset at all"
                        .to_string(),
                );
            }
            if rung > LANDLOCK_BUILD_MAX_RUNG {
                return Err(format!(
                    "`--landlock=abi:{rung}` is above the highest rung this build knows how to \
                     request ({LANDLOCK_BUILD_MAX_RUNG}). The flag is a CAP, so a number above \
                     the build's own ladder could only ever be a no-op wearing the look of an \
                     upgrade"
                ));
            }
            return Ok(LandlockRequest::CappedAt(rung));
        }
        Err(format!(
            "unknown `--landlock` value {value:?} (expected `highest`, `off`, or `abi:N` for \
             N in 1..={LANDLOCK_BUILD_MAX_RUNG})"
        ))
    }

    /// The cap, if there is one. `None` means "as high as this build and this
    /// kernel agree on".
    pub fn cap(self) -> Option<u32> {
        match self {
            LandlockRequest::CappedAt(n) => Some(n),
            LandlockRequest::Highest | LandlockRequest::Off => None,
        }
    }
}

impl fmt::Display for LandlockRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LandlockRequest::Highest => write!(f, "highest"),
            LandlockRequest::CappedAt(n) => write!(f, "abi:{n}"),
            LandlockRequest::Off => write!(f, "off"),
        }
    }
}

/// Which rung the helper asks for **first**, and whether the answer was cut
/// down by this **build** rather than by the kernel or the operator.
///
/// Pure, so the clamp is asserted against a *constructed* kernel ABI rather
/// than by waiting for a kernel that reports one. A kernel newer than this
/// build is not a hypothetical -- rung 10 is already in mainline and absent
/// from this build's headers.
///
/// # Why it lives in the shared library rather than in the helper
///
/// Two processes need the same answer and must not compute it twice. The
/// **helper** needs [`Plan::rung`] to open the ladder at. The **core** needs
/// the same number to decide whether the rung the helper came back with is
/// *below what this session asked for* -- which is the difference between a
/// ladder fallback and a working request, and is the thing #187 forbids
/// masking. A second hand-kept copy of this arithmetic in the core would be a
/// second opinion about one session.
pub fn plan_rung(kernel_abi: u32, request: LandlockRequest) -> Plan {
    let clamped_by_build = kernel_abi > LANDLOCK_BUILD_MAX_RUNG;
    let mut rung = kernel_abi.min(LANDLOCK_BUILD_MAX_RUNG);
    if let Some(cap) = request.cap() {
        // `min`, never assignment: the flag is a cap, so asking for a rung
        // above what the kernel grants is a no-op and not a floor.
        rung = rung.min(cap);
    }
    Plan {
        rung,
        clamped_by_build,
    }
}

/// The opening rung, and why it is not simply the kernel's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The rung to request first. The ladder may still walk below it, and the
    /// core warns when it did.
    pub rung: u32,
    /// The kernel reported an ABI above this build's highest known rung, so
    /// the request was cut down to [`LANDLOCK_BUILD_MAX_RUNG`].
    ///
    /// **Reported, not merely computed.** The core reads this field for every
    /// confined spawn and writes it into the realm's journal entry as
    /// `isolation.landlock.clamped_by_build`, and `vitrind --print-floor`
    /// prints the constant it is measured against. A build confining a newer
    /// kernel at an older rung is a build whose confinement claim is one rung
    /// narrower than its kernel would allow, and nobody should have to diff
    /// two numbers to notice.
    pub clamped_by_build: bool,
}

/// Fixed `size=` caps for the four tmpfs mounts a realm gets.
///
/// # The basis, written down because an unexplained cap is a cap nobody can
/// revise
///
/// Every tmpfs in the table carries an explicit `size=`. This is not
/// tidiness: an unsized tmpfs defaults to **half of RAM**, and OOM-killing
/// `vitrind` under session mode takes the display with it. The four numbers
/// below are *reasoned*, not measured, and they are flagged here as revisable
/// the moment a measurement exists.
///
/// - **`root` = 4 MiB.** The new root holds mount points (empty directories)
///   and a handful of symlinks, and it is remounted read-only before the app
///   exists, so nothing can grow it after bring-up. The number is deliberately
///   ~1000x what the table needs rather than as small as possible, so adding a
///   row later cannot turn into an `ENOSPC` nobody can read.
/// - **`dev` = 1 MiB.** Bind targets (empty files) and one symlink. Device
///   nodes are file binds, not `mknod` -- a `mknod` of a device node is
///   impossible in a non-initial user namespace -- so nothing here consumes
///   data blocks.
/// - **`shm` = 64 MiB.** The only cap an app legitimately fills, and therefore
///   the one a measurement should move first. Sized from one full-screen
///   double-buffered `wl_shm` pool at 4K (3840x2160x4 = 31.6 MiB, twice), which
///   is an upper bound on the shape rather than an observation of any real
///   client.
/// - **`tmp` = 64 MiB.** App scratch. Chosen equal to `shm` so an operator has
///   two numbers to reason about rather than four.
///
/// **The number that matters is the product.** 133 MiB of pageable memory per
/// realm, and `MAX_REALMS` is 16, so a fully populated session can pin 2.1 GiB
/// in tmpfs. That total, not any single row, is what an operator should read
/// before raising a cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmpfsCaps {
    pub root: u64,
    pub dev: u64,
    pub shm: u64,
    pub tmp: u64,
}

impl TmpfsCaps {
    /// The caps argued in this type's docs.
    pub const DEFAULT: TmpfsCaps = TmpfsCaps {
        root: 4 * 1024 * 1024,
        dev: 1024 * 1024,
        shm: 64 * 1024 * 1024,
        tmp: 64 * 1024 * 1024,
    };
}

/// The largest `CONFIG` blob the helper will accept, and the receive buffer
/// both sides size to.
///
/// Bounded rather than streamed because the helper reads this before it has
/// unshared anything: a peer that could make it allocate without limit would
/// be a denial of service against the core's own spawn path. 64 KiB is far
/// past the largest plausible table (a few dozen paths).
pub const CONFIG_MAX: usize = 64 * 1024;

/// The fixed size of every non-`CONFIG` frame.
pub const FRAME_LEN: usize = 32;

/// The exit status this binary uses for a failure that happened **before**
/// the shim's `execve`.
///
/// Reserved rather than arbitrary: after `execve` succeeds the supervisor
/// mirrors the shim's exit exactly and invents no code of its own, so a `125`
/// can only ever be observed on a spawn the core already refused. `125` is
/// the `env(1)`/`nice(1)` convention for "the wrapper failed, not the
/// program".
pub const PRE_EXEC_EXIT: i32 = 125;

// Tags. Two disjoint spaces, so a frame decoded on the wrong side is a
// protocol error rather than a plausible-looking value.
const TAG_CONFIG: u8 = 0x10;
const TAG_MAP_DONE: u8 = 0x11;
const TAG_PROCEED: u8 = 0x12;
const TAG_UNSHARED: u8 = 0x20;
const TAG_CHILD: u8 = 0x21;
const TAG_MOUNTED: u8 = 0x22;
const TAG_FAIL: u8 = 0x23;
const TAG_LANDLOCKED: u8 = 0x24;
const TAG_FILTERED: u8 = 0x25;

/// Where in the sequence the helper gave up.
///
/// A closed vocabulary, drawn on the `cause_class` convention
/// `vitrin_core::spawn` already holds: the core maps each of these to a fixed
/// label a reader can switch on, never to free-form text the helper composed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// The `CONFIG` blob did not decode, or its `schema_version` did not
    /// match this binary's. A stale helper beside a new `vitrind` is refused,
    /// never silently executed.
    Version = 1,
    /// The single six-flag `unshare` failed.
    Unshare = 2,
    /// Reserved, and **unreachable in this build**.
    ///
    /// It was to be `setgroups(0, NULL)`'s refusal. That call turns out to be
    /// impossible for an unprivileged realm in *either* window -- see
    /// `main.rs`'s C3 comment for the two kernel predicates and the
    /// measurement -- so nothing sends this stage. The variant is kept
    /// rather than deleted so the wire numbering is stable and so a future
    /// per-uid tier, where the drop does become reachable, has its label
    /// already agreed.
    Setgroups = 3,
    /// The child's own read-back of `uid_map` / `gid_map` / `setgroups`
    /// disagreed with what the core said it wrote.
    MapVerify = 4,
    /// The supervisor/PID-1 fork failed.
    Fork = 5,
    /// A mount-table entry failed.
    Mount = 6,
    /// `pivot_root` (or its `umount2` tail) failed.
    PivotRoot = 7,
    /// The shim's `execve` failed. This frame is the *only* way the core
    /// learns that, because a successful `execve` is reported by EOF.
    Exec = 8,
    /// Anything else this binary refuses to continue past.
    Internal = 9,
    /// The Landlock ruleset could not be built, granted or enforced
    /// (P2.6.3, issue #187).
    ///
    /// Its own stage rather than [`Stage::Internal`], because the operator's
    /// remedy is specific and different: a kernel without
    /// `CONFIG_SECURITY_LANDLOCK`, or with `landlock` missing from the `lsm=`
    /// boot parameter, answers here and at no other step. **Reaching it is a
    /// refusal, never a downgrade** -- the helper does not fall back to "no
    /// ruleset" when it cannot build the one it was asked for.
    Landlock = 10,
    /// The seccomp-bpf filter could not be compiled or installed
    /// (P2.6.4, issue #188).
    ///
    /// Its own stage rather than [`Stage::Internal`] for the same reason
    /// [`Stage::Landlock`] is: the operator's remedy is specific. A kernel
    /// built without `CONFIG_SECCOMP_FILTER` answers here and nowhere else,
    /// and so does a realm that somehow reached K12c without
    /// `PR_SET_NO_NEW_PRIVS` -- which are two different repairs.
    /// **Reaching it is a refusal, never a downgrade**: there is no
    /// `--seccomp=off`, so the helper does not fall back to "no filter" when
    /// it cannot install the one the table describes.
    Seccomp = 11,
}

impl Stage {
    fn from_u8(v: u8) -> Option<Stage> {
        Some(match v {
            1 => Stage::Version,
            2 => Stage::Unshare,
            3 => Stage::Setgroups,
            4 => Stage::MapVerify,
            5 => Stage::Fork,
            6 => Stage::Mount,
            7 => Stage::PivotRoot,
            8 => Stage::Exec,
            9 => Stage::Internal,
            10 => Stage::Landlock,
            11 => Stage::Seccomp,
            _ => return None,
        })
    }
}

/// One message on the config channel.
///
/// `SOCK_SEQPACKET`, so a frame is a datagram and there is no framing to get
/// wrong. **There is no `READY` frame**: EOF on this channel is the success
/// signal for the shim's `execve`, because the channel descriptor is
/// `FD_CLOEXEC` and `FD_CLOEXEC` takes effect only on a *successful* `execve`
/// -- the same trick std uses for its own exec-report pipe. A `READY` sent
/// before `execve` would report success for a shim that then failed to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// core -> helper. The whole realm description, once.
    Config(Box<Config>),
    /// core -> helper. `setgroups`, `uid_map` and `gid_map` are written *and
    /// read back*; the child may proceed to verify them itself.
    MapDone,
    /// core -> helper. The core's own reads of `/proc/<pid>/ns/*`,
    /// `/proc/<pid>/root` and the canary set all passed. Nothing else
    /// licenses the second `execve`.
    Proceed,
    /// helper -> core. The six-flag `unshare` and the `setgroups(0, NULL)`
    /// both succeeded; the helper is now blocked waiting for its maps.
    Unshared,
    /// helper -> core. The PID-1 child exists, at this **host** pid -- the
    /// number the core addresses `/proc` by.
    Child { host_pid: i32 },
    /// helper -> core, **child-asserted**. What the PID-1 child's own
    /// post-pivot `/proc/self/mountinfo` said. Journaled as an assertion, not
    /// as evidence: a substituted helper could send anything, which is why the
    /// core's own root-view check exists.
    Mounted { count: u32, fingerprint: u64 },
    /// helper -> core, **child-asserted**, sent by PID 1 after the ruleset is
    /// enforced and before the shim's `execve` (P2.6.3, issue #187).
    ///
    /// Two numbers, because one of them cannot be read off the other. `rung`
    /// is the ABI rung the ruleset was actually built at -- `0` when the
    /// session asked for none -- and `kernel_abi` is what
    /// `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`
    /// reported inside the realm. A session pinned low is then visible as
    /// `rung < kernel_abi` in one journal entry, which is the whole point of
    /// sending the second number: without it a capped session reads exactly
    /// like a session on a weak kernel.
    ///
    /// `kernel_abi == 0` means **not measured**, and there is exactly one
    /// such path: `--landlock=off`, where the helper returns before issuing
    /// any syscall so that a kernel with no Landlock at all can still run a
    /// session that asked for no ruleset. Zero is not an ABI version -- they
    /// start at 1 -- and the core journals it as `null`.
    ///
    /// Asserted by the child and journaled as such, on the same terms as
    /// [`Frame::Mounted`]. There is no `/proc` file that reports a process's
    /// Landlock domain, so the parent cannot corroborate it; what a
    /// substituted helper cannot forge is the *behaviour*, which is what
    /// `tests/integration/test_real_confinement.py` measures from inside the
    /// realm.
    Landlocked { rung: u32, kernel_abi: u32 },
    /// helper -> core, **child-asserted**, sent by PID 1 after the seccomp
    /// filter is installed and before the shim's `execve` (P2.6.4, issue
    /// #188).
    ///
    /// Two numbers on the same terms as [`Frame::Landlocked`], and for the
    /// same reason neither is derivable from the other: `rows` is how many
    /// entries of the deny-list table this build compiled, and `instructions`
    /// is what that came to in classic BPF. A build whose table grew and whose
    /// program did not is a build whose assembler dropped something.
    ///
    /// **Asserted, not evidence.** There is no `/proc` file that lists a
    /// process's seccomp *rules* -- `/proc/<pid>/status` reports only
    /// `Seccomp: 2`, the mode -- so the parent can corroborate that a filter
    /// exists and never which one. What a substituted helper cannot forge is
    /// the realm's *behaviour*, which is what
    /// `tests/integration/test_real_seccomp.py` measures from inside.
    Filtered { rows: u32, instructions: u32 },
    /// helper -> core. A refusal, at a named stage, with the kernel's errno.
    Fail { stage: Stage, errno: i32 },
}

/// Why a frame or a config blob did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// The datagram was not the length this frame kind requires.
    Length { got: usize },
    /// The tag byte names no frame in this direction.
    Tag(u8),
    /// A length-prefixed field ran past the end of the blob.
    Truncated,
    /// A `Fail` frame named a stage this build does not know.
    Stage(u8),
    /// The config blob's Landlock selection is not one this build has a
    /// reading for. Refused rather than defaulted: see the decoder.
    Landlock { tag: u32, value: u32 },
    /// The blob is larger than [`CONFIG_MAX`].
    TooLarge(usize),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::Length { got } => write!(f, "frame is {got} bytes, not {FRAME_LEN}"),
            CodecError::Tag(t) => write!(f, "unknown frame tag {t:#04x} for this direction"),
            CodecError::Truncated => write!(f, "a length-prefixed field ran past the end"),
            CodecError::Stage(s) => write!(f, "unknown failure stage {s}"),
            CodecError::Landlock { tag, value } => {
                write!(f, "no Landlock selection has tag {tag} and value {value}")
            }
            CodecError::TooLarge(n) => write!(f, "config blob is {n} bytes, over {CONFIG_MAX}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Everything the helper needs to build one realm, and nothing it could use
/// to decide anything.
///
/// Every path here is one the **core** canonicalized and audited before the
/// fork; the helper resolves no name of its own and reads no environment
/// variable, ever. Environment composition stays at one site in
/// `vitrin_core::spawn::child_env`, and this binary passes its own `environ`
/// through to the shim verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The core's `CARGO_PKG_VERSION`. Compared for **exact** equality
    /// against this binary's own before anything else happens: a `--shim`
    /// from `/usr/lib/vitrin` beside a helper from `target/debug` is a
    /// refusal, not undefined behaviour.
    pub schema_version: String,
    /// The realm's id -- used as the realm's hostname, so `uname -n` inside
    /// the realm names the realm rather than the operator's machine.
    pub realm_id: String,
    /// The identity map's single line: `<inner_uid> <inner_uid> 1`.
    ///
    /// Identity, one fixed shape for every realm (D-036(3)). Namespace-root
    /// (`0 <euid> 1`) was available and is refused: it buys nothing and grants
    /// the app `CAP_SYS_ADMIN` in the realm's own user namespace, from which
    /// it could bind-mount over the read-only view for every process in the
    /// realm.
    pub inner_uid: u32,
    pub inner_gid: u32,
    /// Host source for [`IN_REALM_RUNTIME_DIR`].
    pub runtime_dir: PathBuf,
    /// Host source for [`IN_REALM_HOME`].
    pub storage_dir: PathBuf,
    /// Host source for [`IN_REALM_SHIM`].
    pub shim_source: PathBuf,
    /// The directory holding the app binary, bound at the **same** in-realm
    /// path, when the app is not already covered by `/usr` or `/etc`.
    pub app_dir: Option<PathBuf>,
    /// Render nodes (`/dev/dri/renderD*`) the core enumerated and audited at
    /// startup. Never `card*`, never `controlD*`.
    pub render_nodes: Vec<PathBuf>,
    /// Operator-declared read-only binds from `realm.toml`, canonicalized and
    /// audited at spawn exactly as `command` is.
    pub binds: Vec<PathBuf>,
    /// The full argv of the second `execve`: the in-realm shim path, the
    /// shim's own leading arguments, `--`, then the app command and its args.
    pub argv: Vec<OsString>,
    pub caps: TmpfsCaps,
    /// Which Landlock rung the helper may build (P2.6.3, issue #187), from
    /// the session's `--landlock` flag.
    ///
    /// A **session** input like every other field here: the helper decides
    /// nothing, it walks the ladder down from this cap to what the kernel
    /// accepts and reports what it got. This channel is a private codec
    /// between two halves of one build, not the wire protocol, so carrying it
    /// here owes no IDL edit.
    pub landlock: LandlockRequest,
}

impl Frame {
    /// Encode into a datagram. `CONFIG` is variable-length and bounded;
    /// everything else is exactly [`FRAME_LEN`].
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let fixed = |tag: u8, a: i64, b: u64, stage: u8| {
            let mut buf = vec![0u8; FRAME_LEN];
            buf[0] = tag;
            buf[1] = stage;
            buf[4..8].copy_from_slice(&(a as i32).to_le_bytes());
            buf[8..16].copy_from_slice(&b.to_le_bytes());
            buf
        };
        Ok(match self {
            Frame::Config(cfg) => {
                let mut buf = vec![TAG_CONFIG];
                cfg.encode_into(&mut buf);
                if buf.len() > CONFIG_MAX {
                    return Err(CodecError::TooLarge(buf.len()));
                }
                buf
            }
            Frame::MapDone => fixed(TAG_MAP_DONE, 0, 0, 0),
            Frame::Proceed => fixed(TAG_PROCEED, 0, 0, 0),
            Frame::Unshared => fixed(TAG_UNSHARED, 0, 0, 0),
            Frame::Child { host_pid } => fixed(TAG_CHILD, i64::from(*host_pid), 0, 0),
            Frame::Mounted { count, fingerprint } => {
                fixed(TAG_MOUNTED, i64::from(*count), *fingerprint, 0)
            }
            Frame::Landlocked { rung, kernel_abi } => {
                fixed(TAG_LANDLOCKED, i64::from(*rung), u64::from(*kernel_abi), 0)
            }
            Frame::Filtered { rows, instructions } => {
                fixed(TAG_FILTERED, i64::from(*rows), u64::from(*instructions), 0)
            }
            Frame::Fail { stage, errno } => fixed(TAG_FAIL, i64::from(*errno), 0, *stage as u8),
        })
    }

    /// Decode one datagram.
    pub fn decode(bytes: &[u8]) -> Result<Frame, CodecError> {
        let tag = *bytes.first().ok_or(CodecError::Length { got: 0 })?;
        if tag == TAG_CONFIG {
            if bytes.len() > CONFIG_MAX {
                return Err(CodecError::TooLarge(bytes.len()));
            }
            return Ok(Frame::Config(Box::new(Config::decode(&bytes[1..])?)));
        }
        if bytes.len() != FRAME_LEN {
            return Err(CodecError::Length { got: bytes.len() });
        }
        let a = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let b = u64::from_le_bytes(bytes[8..16].try_into().expect("eight bytes"));
        Ok(match tag {
            TAG_MAP_DONE => Frame::MapDone,
            TAG_PROCEED => Frame::Proceed,
            TAG_UNSHARED => Frame::Unshared,
            TAG_CHILD => Frame::Child { host_pid: a },
            TAG_MOUNTED => Frame::Mounted {
                count: a as u32,
                fingerprint: b,
            },
            TAG_LANDLOCKED => Frame::Landlocked {
                rung: a as u32,
                kernel_abi: b as u32,
            },
            TAG_FILTERED => Frame::Filtered {
                rows: a as u32,
                instructions: b as u32,
            },
            TAG_FAIL => Frame::Fail {
                stage: Stage::from_u8(bytes[1]).ok_or(CodecError::Stage(bytes[1]))?,
                errno: a,
            },
            other => return Err(CodecError::Tag(other)),
        })
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, v: &[u8]) {
    put_u32(out, v.len() as u32);
    out.extend_from_slice(v);
}

fn put_path(out: &mut Vec<u8>, p: &Path) {
    put_bytes(out, p.as_os_str().as_bytes());
}

/// A bounds-checked cursor. Every read either yields a value inside the blob
/// or [`CodecError::Truncated`]; there is no arithmetic on a length the blob
/// did not supply.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Result<u32, CodecError> {
        let end = self.at.checked_add(4).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(u32::from_le_bytes(slice.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        let end = self.at.checked_add(8).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(u64::from_le_bytes(slice.try_into().expect("eight bytes")))
    }

    fn bytes(&mut self) -> Result<&'a [u8], CodecError> {
        let len = self.u32()? as usize;
        let end = self.at.checked_add(len).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn string(&mut self) -> Result<String, CodecError> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    fn os_string(&mut self) -> Result<OsString, CodecError> {
        Ok(OsString::from_vec(self.bytes()?.to_vec()))
    }

    fn path(&mut self) -> Result<PathBuf, CodecError> {
        Ok(PathBuf::from(OsStr::from_bytes(self.bytes()?)))
    }

    fn paths(&mut self) -> Result<Vec<PathBuf>, CodecError> {
        let n = self.u32()? as usize;
        // Bounded by the blob's own remaining length: an attacker-supplied
        // count cannot make this allocate more than the bytes that exist.
        let mut out = Vec::with_capacity(n.min(self.bytes.len()));
        for _ in 0..n {
            out.push(self.path()?);
        }
        Ok(out)
    }
}

impl Config {
    fn encode_into(&self, out: &mut Vec<u8>) {
        put_bytes(out, self.schema_version.as_bytes());
        put_bytes(out, self.realm_id.as_bytes());
        put_u32(out, self.inner_uid);
        put_u32(out, self.inner_gid);
        put_path(out, &self.runtime_dir);
        put_path(out, &self.storage_dir);
        put_path(out, &self.shim_source);
        match &self.app_dir {
            Some(p) => {
                put_u32(out, 1);
                put_path(out, p);
            }
            None => put_u32(out, 0),
        }
        put_u32(out, self.render_nodes.len() as u32);
        for p in &self.render_nodes {
            put_path(out, p);
        }
        put_u32(out, self.binds.len() as u32);
        for p in &self.binds {
            put_path(out, p);
        }
        put_u32(out, self.argv.len() as u32);
        for a in &self.argv {
            put_bytes(out, a.as_bytes());
        }
        put_u64(out, self.caps.root);
        put_u64(out, self.caps.dev);
        put_u64(out, self.caps.shm);
        put_u64(out, self.caps.tmp);
        // Tag plus value rather than one number with reserved sentinels: a
        // sentinel is a value somebody eventually types.
        let (tag, value) = match self.landlock {
            LandlockRequest::Highest => (0u32, 0u32),
            LandlockRequest::CappedAt(n) => (1, n),
            LandlockRequest::Off => (2, 0),
        };
        put_u32(out, tag);
        put_u32(out, value);
    }

    fn decode(bytes: &[u8]) -> Result<Config, CodecError> {
        let mut c = Cursor { bytes, at: 0 };
        let schema_version = c.string()?;
        let realm_id = c.string()?;
        let inner_uid = c.u32()?;
        let inner_gid = c.u32()?;
        let runtime_dir = c.path()?;
        let storage_dir = c.path()?;
        let shim_source = c.path()?;
        let app_dir = match c.u32()? {
            0 => None,
            _ => Some(c.path()?),
        };
        let render_nodes = c.paths()?;
        let binds = c.paths()?;
        let argv_len = c.u32()? as usize;
        let mut argv = Vec::with_capacity(argv_len.min(bytes.len()));
        for _ in 0..argv_len {
            argv.push(c.os_string()?);
        }
        let caps = TmpfsCaps {
            root: c.u64()?,
            dev: c.u64()?,
            shm: c.u64()?,
            tmp: c.u64()?,
        };
        // An unknown tag is a refusal and never a default. The default this
        // arm would otherwise pick is `Highest`, which is the *safe*
        // direction and still wrong: a helper that silently reinterprets a
        // confinement request it did not understand is a helper whose journal
        // entry describes a session nobody selected.
        let landlock = match (c.u32()?, c.u32()?) {
            (0, _) => LandlockRequest::Highest,
            (1, n) if (1..=LANDLOCK_BUILD_MAX_RUNG).contains(&n) => LandlockRequest::CappedAt(n),
            (2, _) => LandlockRequest::Off,
            (tag, value) => return Err(CodecError::Landlock { tag, value }),
        };
        Ok(Config {
            schema_version,
            realm_id,
            inner_uid,
            inner_gid,
            runtime_dir,
            storage_dir,
            shim_source,
            app_dir,
            render_nodes,
            binds,
            argv,
            caps,
            landlock,
        })
    }
}

/// FNV-1a, 64-bit, over the child's own post-pivot `mountinfo`.
///
/// **Deliberately not a cryptographic digest, and named as one would not be.**
/// The value is child-asserted: a substituted helper can send any number it
/// likes, so collision resistance would buy nothing that the core's own
/// root-view check does not already provide. What it *is* for is noticing that
/// the realm's mount table changed shape between two runs of the same build --
/// a job a checksum does, and calling it a "digest" beside the recorder's
/// blake3 capture digests would overclaim.
pub fn mount_fingerprint(mountinfo: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in mountinfo {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The algorithm name the flight recorder prints beside a fingerprint, so a
/// reader never has to guess which family the number is from.
pub const MOUNT_FINGERPRINT_ALG: &str = "fnv1a-64";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Config {
        Config {
            schema_version: "0.1.0".into(),
            realm_id: "realm-0".into(),
            inner_uid: 1000,
            inner_gid: 1000,
            runtime_dir: PathBuf::from("/run/user/1000/vitrin-0/realm-0"),
            storage_dir: PathBuf::from("/home/op/.local/share/vitrin/realms/realm-0"),
            shim_source: PathBuf::from("/usr/lib/vitrin/vitrin-shim"),
            app_dir: Some(PathBuf::from("/opt/app/bin")),
            render_nodes: vec![PathBuf::from("/dev/dri/renderD128")],
            binds: vec![PathBuf::from("/nix"), PathBuf::from("/opt/data")],
            argv: vec![
                OsString::from(IN_REALM_SHIM),
                OsString::from("--"),
                OsString::from("/opt/app/bin/app"),
                OsString::from("--flag"),
            ],
            caps: TmpfsCaps::DEFAULT,
            landlock: LandlockRequest::Highest,
        }
    }

    #[test]
    fn the_config_blob_round_trips() {
        // Both sides import this type, so a divergent hand-transcribed copy
        // cannot exist -- but the codec itself still has to be an identity.
        let cfg = sample_config();
        let frame = Frame::Config(Box::new(cfg.clone()));
        let bytes = frame.encode().expect("encodes");
        assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
    }

    #[test]
    fn a_config_with_no_app_dir_and_no_binds_round_trips() {
        // The `Option`/empty-vector edges, which is where a hand-rolled codec
        // gets its off-by-one.
        let mut cfg = sample_config();
        cfg.app_dir = None;
        cfg.binds.clear();
        cfg.render_nodes.clear();
        let frame = Frame::Config(Box::new(cfg));
        let bytes = frame.encode().expect("encodes");
        assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
    }

    #[test]
    fn every_fixed_frame_round_trips_at_exactly_one_length() {
        for frame in [
            Frame::MapDone,
            Frame::Proceed,
            Frame::Unshared,
            Frame::Child { host_pid: 424_242 },
            Frame::Mounted {
                count: 21,
                fingerprint: 0xdead_beef_cafe_f00d,
            },
            Frame::Landlocked {
                rung: 3,
                kernel_abi: 9,
            },
            Frame::Filtered {
                rows: 13,
                instructions: 38,
            },
            Frame::Fail {
                stage: Stage::Unshare,
                errno: libc::EPERM,
            },
        ] {
            let bytes = frame.encode().expect("encodes");
            assert_eq!(bytes.len(), FRAME_LEN, "{frame:?} is not a fixed frame");
            assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
        }
        // The two fields are DIFFERENT numbers on the wire, which one
        // round-trip of equal values could not show: `Filtered { 13, 38 }` and
        // a codec that wrote one field twice both decode to themselves when
        // the two are equal. Held here rather than trusted, because the core
        // journals both and a swapped pair would publish a plausible wrong
        // size.
        let bytes = Frame::Filtered {
            rows: 1,
            instructions: 2,
        }
        .encode()
        .expect("encodes");
        assert_eq!(
            Frame::decode(&bytes).expect("decodes"),
            Frame::Filtered {
                rows: 1,
                instructions: 2
            },
            "the row and instruction counts crossed on the wire"
        );
    }

    #[test]
    fn every_stage_survives_the_wire() {
        // A `Fail` whose stage did not decode would be reported to the core as
        // a protocol error instead of the refusal it is, which would hide the
        // real cause behind a generic one.
        for stage in [
            Stage::Version,
            Stage::Unshare,
            Stage::Setgroups,
            Stage::MapVerify,
            Stage::Fork,
            Stage::Mount,
            Stage::PivotRoot,
            Stage::Exec,
            Stage::Internal,
            Stage::Landlock,
            Stage::Seccomp,
        ] {
            let frame = Frame::Fail { stage, errno: 13 };
            let bytes = frame.encode().expect("encodes");
            assert_eq!(Frame::decode(&bytes).expect("decodes"), frame);
        }
        // **The list above is written by hand, so it can silently stop being
        // every stage** -- which is the failure this test exists to prevent,
        // one level up. A stage added to the enum and forgotten here would
        // decode to `CodecError::Stage` in the field and be reported to the
        // core as a protocol error rather than as the refusal it is. So the
        // discriminants are walked instead of trusted: every `u8` the enum
        // could occupy either decodes or is genuinely unassigned, and the
        // highest assigned one is named.
        assert!(
            Stage::from_u8(Stage::Seccomp as u8).is_some(),
            "#188's stage does not decode"
        );
        let assigned = (0u8..=32).filter(|v| Stage::from_u8(*v).is_some()).count();
        assert_eq!(
            assigned, 11,
            "the wire vocabulary changed size. Add the new stage to the loop above as well as \
             to `from_u8`, or a realm's real refusal reaches the core as `helper_protocol`"
        );
    }

    #[test]
    fn a_truncated_blob_is_an_error_and_never_a_partial_config() {
        let bytes = Frame::Config(Box::new(sample_config()))
            .encode()
            .expect("encodes");
        for cut in [1usize, 5, 17, bytes.len() - 1] {
            assert!(
                matches!(Frame::decode(&bytes[..cut]), Err(CodecError::Truncated)),
                "a blob cut at {cut} decoded to something"
            );
        }
    }

    #[test]
    fn a_wrong_length_fixed_frame_is_refused() {
        let mut bytes = Frame::Unshared.encode().expect("encodes");
        bytes.push(0);
        assert_eq!(
            Frame::decode(&bytes),
            Err(CodecError::Length { got: FRAME_LEN + 1 })
        );
        assert_eq!(Frame::decode(&[]), Err(CodecError::Length { got: 0 }));
    }

    #[test]
    fn an_unknown_tag_is_refused_rather_than_guessed() {
        let mut bytes = vec![0u8; FRAME_LEN];
        bytes[0] = 0x77;
        assert_eq!(Frame::decode(&bytes), Err(CodecError::Tag(0x77)));
    }

    #[test]
    fn an_oversized_config_is_refused_at_both_ends() {
        let mut cfg = sample_config();
        // One path far past the cap: the encoder must refuse rather than hand
        // the helper a datagram `sendmsg` would truncate.
        cfg.binds = vec![PathBuf::from("/".to_string() + &"a".repeat(CONFIG_MAX))];
        assert!(matches!(
            Frame::Config(Box::new(cfg)).encode(),
            Err(CodecError::TooLarge(_))
        ));
        let oversized = vec![TAG_CONFIG; CONFIG_MAX + 1];
        assert!(matches!(
            Frame::decode(&oversized),
            Err(CodecError::TooLarge(_))
        ));
    }

    #[test]
    fn the_helper_deadline_is_the_shortest_dead_man_hold() {
        // The core holds the *equality* with a compile-time assertion against
        // `deadman::MIN_HOLD`, which is private to that crate. This is the
        // half that can be stated here: the number, so a change to it shows up
        // as a failing test in this crate too rather than only in the core's.
        assert_eq!(HELPER_DEADLINE, Duration::from_millis(250));
    }

    #[test]
    fn the_fingerprint_is_a_function_of_the_bytes_and_changes_with_them() {
        assert_eq!(mount_fingerprint(b"a b c"), mount_fingerprint(b"a b c"));
        assert_ne!(mount_fingerprint(b"a b c"), mount_fingerprint(b"a b d"));
        // Non-vacuity for the loop body: an implementation that ignored its
        // input would pass the equality above and fail this.
        assert_ne!(mount_fingerprint(b""), mount_fingerprint(b"\0"));
    }

    #[test]
    fn the_reserved_bus_path_is_inside_the_realms_runtime_directory() {
        // D-020(4)'s reservation is a name plus an ordering: it is only "no
        // new mount for P2.1.3" if the path lives in a mount that already
        // exists.
        assert!(IN_REALM_A11Y_BUS.starts_with(IN_REALM_RUNTIME_DIR));
        assert!(IN_REALM_WAYLAND_SOCKET.starts_with(IN_REALM_RUNTIME_DIR));
        assert_ne!(IN_REALM_A11Y_BUS, IN_REALM_WAYLAND_SOCKET);
    }

    #[test]
    fn every_landlock_selection_survives_the_config_blob() {
        // The field the *confinement* is negotiated over. A selection that
        // decoded to a different rung than the one the operator typed would
        // be the exact divergence this shared type exists to make impossible.
        for request in [
            LandlockRequest::Highest,
            LandlockRequest::Off,
            LandlockRequest::CappedAt(1),
            LandlockRequest::CappedAt(LANDLOCK_BUILD_MAX_RUNG),
        ] {
            let mut cfg = sample_config();
            cfg.landlock = request;
            let frame = Frame::Config(Box::new(cfg));
            let bytes = frame.encode().expect("encodes");
            assert_eq!(Frame::decode(&bytes).expect("decodes"), frame, "{request}");
        }
    }

    #[test]
    fn an_unknown_landlock_tag_is_refused_rather_than_defaulted() {
        // Non-vacuity for the arm above: the decoder really is reading those
        // two trailing words, and it refuses rather than picking the safe
        // default. A helper that silently reinterprets a confinement request
        // journals a session nobody selected.
        let mut bytes = Frame::Config(Box::new(sample_config()))
            .encode()
            .expect("encodes");
        let len = bytes.len();
        bytes[len - 8..len - 4].copy_from_slice(&7u32.to_le_bytes());
        assert!(matches!(
            Frame::decode(&bytes),
            Err(CodecError::Landlock { tag: 7, .. })
        ));
        // And a cap outside the build's ladder is refused too, so a blob from
        // a build with a longer ladder cannot ask for a rung this one would
        // silently round down.
        let mut bytes = Frame::Config(Box::new(sample_config()))
            .encode()
            .expect("encodes");
        let len = bytes.len();
        bytes[len - 8..len - 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[len - 4..].copy_from_slice(&(LANDLOCK_BUILD_MAX_RUNG + 1).to_le_bytes());
        assert!(matches!(
            Frame::decode(&bytes),
            Err(CodecError::Landlock { tag: 1, .. })
        ));
    }

    /// The audit diagnostic answers to **one** spelling.
    ///
    /// This is the whole of "it cannot be on by default": the switch has no
    /// default-on path because there is no value but `"1"` it says yes to, and
    /// the two sides that read it -- the core, deciding whether to forward the
    /// name into the helper's environment, and the helper, deciding whether to
    /// set the flag -- ask this one function rather than each parsing a string.
    /// The values enumerated below are the ones an inherited environment
    /// actually carries in the wild; every one of them means no.
    #[test]
    fn the_landlock_audit_diagnostic_answers_to_exactly_one_value() {
        assert!(landlock_audit_requested(Some("1")));
        for no in [
            "", "0", "true", "TRUE", "True", "yes", "on", "1 ", " 1", "01", "2", "-1", "false",
        ] {
            assert!(
                !landlock_audit_requested(Some(no)),
                "{no:?} switched the Landlock audit diagnostic on. It must answer to \"1\" and \
                 nothing else -- a permissive parse is how an inherited `...=false` comes to \
                 mean yes"
            );
        }
        assert!(
            !landlock_audit_requested(None),
            "an unset variable is the shipped path and must never be on"
        );
        // The name is part of the contract: the core forwards it and the
        // helper reads it, in two different processes with no shared parse.
        assert_eq!(LANDLOCK_AUDIT_ENV, "VITRIN_LANDLOCK_AUDIT");
    }

    #[test]
    fn the_landlock_flag_parses_exactly_the_values_it_documents() {
        assert_eq!(
            LandlockRequest::parse("highest"),
            Ok(LandlockRequest::Highest)
        );
        assert_eq!(LandlockRequest::parse("off"), Ok(LandlockRequest::Off));
        assert_eq!(
            LandlockRequest::parse("abi:3"),
            Ok(LandlockRequest::CappedAt(3))
        );
        // `abi:0` names no rung -- ABI versions start at 1 -- and the message
        // has to point at `off` rather than read as a typo, because somebody
        // who means "no ruleset" will type it.
        let zero = LandlockRequest::parse("abi:0").expect_err("abi:0 is not a rung");
        assert!(zero.contains("--landlock=off"), "{zero}");
        // A cap above this build's ladder is refused rather than clamped: a
        // silent clamp would read as an upgrade that did nothing.
        let high = LandlockRequest::parse(&format!("abi:{}", LANDLOCK_BUILD_MAX_RUNG + 1))
            .expect_err("above the build's ladder");
        assert!(high.contains("CAP"), "{high}");
        assert!(LandlockRequest::parse("abi:x").is_err());
        assert!(LandlockRequest::parse("on").is_err());
        // Round-trips through its own rendering, which is what the journal
        // and `--print-floor` both print.
        for request in [
            LandlockRequest::Highest,
            LandlockRequest::Off,
            LandlockRequest::CappedAt(4),
        ] {
            assert_eq!(LandlockRequest::parse(&request.to_string()), Ok(request));
        }
    }

    #[test]
    fn the_core_owned_prefix_holds_the_shim_and_the_storage() {
        assert!(IN_REALM_SHIM.starts_with(IN_REALM_PREFIX));
        assert!(IN_REALM_HOME.starts_with(IN_REALM_PREFIX));
    }

    /// `TASK_COMM_LEN - 1`: the kernel copies at most this many bytes of the
    /// `execve`d file's basename into `comm` and NUL-terminates.
    const COMM_MAX: usize = 15;

    /// What `ps` will show for a process that `execve`d `path`: the component
    /// after the last `/`, truncated the way `__set_task_comm` truncates it.
    fn comm_of(path: &str) -> &str {
        let base = path.rsplit('/').next().unwrap_or(path);
        &base[..base.len().min(COMM_MAX)]
    }

    /// Issue #283. The kernel derives `comm` from the basename of the
    /// `execve`d file, so this constant's LAST COMPONENT is what `ps` shows
    /// for every confined shim.
    ///
    /// Two properties, and the pair is the whole of what the name is worth:
    /// it is the shim's own name rather than the bare `shim` the old bind
    /// target produced, and it survives `TASK_COMM_LEN` truncation whole, so
    /// what `ps` prints is the name and not a prefix of it.
    #[test]
    fn the_confined_shims_comm_is_this_constants_basename() {
        assert_eq!(
            comm_of(IN_REALM_SHIM),
            "vitrin-shim",
            "the bind target's basename IS the confined shim's comm; \
             `/vitrin/shim` made it `shim`, which is issue #283's second half"
        );
        assert_eq!(
            IN_REALM_SHIM.rsplit('/').next(),
            Some(comm_of(IN_REALM_SHIM)),
            "the basename must survive the kernel's {COMM_MAX}-byte truncation \
             whole, or `ps` shows a prefix and the legibility this rename \
             bought is spent"
        );
    }

    /// Issue #283, and the half a sentence in a doc comment kept getting
    /// wrong. A confined process's `comm` is decided by **this constant**,
    /// not by the binary the core was handed: `main.rs` binds whatever host
    /// path arrived in `sources.shim` *to this target*, so a confined
    /// `vitrin-mock-shim` answers to `vitrin-shim` exactly as the real shim
    /// does. Measured 2026-08-19 against a `--isolation=default` core started
    /// with the mock: `comm` `vitrin-shim`, `/proc/<pid>/exe ->
    /// /vitrin/vitrin-shim`, and the inode the mock's.
    ///
    /// So `comm` is **not** a mock-freeness check and no rename can make it
    /// one -- the integration gates compare `(st_dev, st_ino)` of
    /// `/proc/<pid>/exe`. This test asserts the mechanism rather than
    /// restating the conclusion, because the conclusion is the part that
    /// rots: if the bind ever stops taking `sources.shim`, or a second bind
    /// starts writing this target, the reasoning above is void and this goes
    /// red instead of a comment quietly becoming false.
    #[test]
    fn a_confined_comm_is_decided_by_the_bind_target_not_the_host_binary() {
        const MAIN_RS: &str = include_str!("main.rs");
        const TARGET: &str = "strip_leading_slash(IN_REALM_SHIM)";

        // Comments removed FIRST. What this test counts is bind sites, and
        // the most natural place to explain a bind site is a comment beside
        // it -- so counting raw text would turn "somebody documented this
        // line" into a red build complaining about binds. That was raised as
        // brittleness in #283's third review round, and it was right.
        let code = code_without_comments(MAIN_RS);

        let sites: Vec<usize> = code.match_indices(TARGET).map(|(at, _)| at).collect();
        assert_eq!(
            sites.len(),
            1,
            "main.rs's CODE mentions `{TARGET}` {} times, not once (comments \
             are stripped before counting, so prose naming this call is free). \
             This test reads the single bind that creates the shim's in-realm \
             path; with more than one it is reading an arbitrary one of them, \
             and with none the path is created somewhere this check cannot see.",
            sites.len()
        );

        // Back up to a char boundary: `main.rs` is not pure ASCII, and
        // slicing a `&str` mid-codepoint panics with a message about UTF-8
        // rather than about the bind this test is here to check.
        let at = sites[0];
        let mut from = at.saturating_sub(120);
        while from < at && !code.is_char_boundary(from) {
            from += 1;
        }
        let call = &code[from..at];
        assert!(
            call.contains("sources.shim"),
            "the bind that creates `{IN_REALM_SHIM}` no longer takes \
             `sources.shim`, so `comm` may no longer be a function of the \
             bind target alone -- the doc comment on IN_REALM_SHIM, \
             tests/integration/README.md and D-044 all reason from that and \
             have to be re-checked. What precedes the target is: {call:?}"
        );
    }

    /// Rust source with its `//` comments removed, for the test above.
    ///
    /// Deliberately **not** a lexer, and it refuses rather than guesses. It
    /// tracks double-quoted strings — backslash escapes included, and across
    /// newlines, because `main.rs` really does continue a string onto the next
    /// line — and it panics if the file ever grows a raw string or a `/* */`
    /// comment outside a string, the two constructs a scanner this size would
    /// silently get wrong. A parser that quietly mis-reads its input is the
    /// exact failure the test above exists to prevent, so it may not be one.
    fn code_without_comments(src: &str) -> String {
        // Byte-wise is safe here: every byte of a multi-byte UTF-8 sequence is
        // >= 0x80, so none can equal `/`, `"`, `\`, `r` or `\n`, and nothing
        // below can split a character.
        let bytes = src.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0usize;
        let mut in_string = false;
        while i < bytes.len() {
            let b = bytes[i];
            if in_string {
                if b == b'\\' {
                    out.push(b);
                    if i + 1 < bytes.len() {
                        out.push(bytes[i + 1]);
                    }
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    in_string = false;
                }
                out.push(b);
                i += 1;
                continue;
            }
            let next = bytes.get(i + 1).copied();
            if b == b'"' {
                in_string = true;
                out.push(b);
                i += 1;
                continue;
            }
            if b == b'/' && next == Some(b'/') {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue; // the `\n` itself is kept, so line structure survives
            }
            assert!(
                !(b == b'/' && next == Some(b'*')),
                "main.rs has grown a `/* */` block comment outside a string, \
                 which this stripper does not handle -- it would leave the \
                 comment's text in the code it hands to the bind-site count. \
                 Teach it block comments, or use `//` here."
            );
            let prev_is_ident = out
                .last()
                .is_some_and(|p| p.is_ascii_alphanumeric() || *p == b'_');
            assert!(
                !(b == b'r' && !prev_is_ident && matches!(next, Some(b'"') | Some(b'#'))),
                "main.rs has grown a raw string literal, whose `\"` this \
                 stripper would treat as an ordinary quote and so lose track \
                 of where strings end. Teach it raw strings, or keep the \
                 literal ordinary."
            );
            out.push(b);
            i += 1;
        }
        assert!(
            !in_string,
            "main.rs ended inside a string literal, so this stripper lost \
             track of quoting somewhere and its output cannot be trusted to \
             be code."
        );
        String::from_utf8(out).expect("stripping whole `//` runs cannot split a UTF-8 character")
    }
}
