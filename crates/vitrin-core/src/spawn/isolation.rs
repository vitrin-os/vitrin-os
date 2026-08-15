// SPDX-License-Identifier: MPL-2.0
//! The runtime isolation preflight (P2.6.1, issue #185): what confinement
//! *this kernel* will actually grant, measured rather than assumed.
//!
//! # Why this module exists before the confinement it describes
//!
//! Phase 2 risk **R2.9** is the one risk that can invalidate two whole epics:
//! unprivileged user namespaces are restricted or disabled outright on several
//! major distributions, and without a user namespace there is no mount, PID,
//! IPC or network namespace either — so E2.6 and E2.7 have nothing to build
//! on. The mitigation the plan states is not "document the requirement": it is
//! that the core **measures its tier and refuses to start below a floor**
//! rather than degrading silently. A quiet degradation is the failure mode
//! that matters here, because every confinement claim downstream would still
//! read as true while being false.
//!
//! That is why this lands first, ahead of [`super`]'s spawn path actually
//! using any of it: the measurement is what tells us whether the rest of the
//! track is buildable on real machines, and it retires R2.9 only by running on
//! real kernels.
//!
//! # Probing rules this module holds itself to
//!
//! 1. **Attempt, never infer.** No probe parses a kernel version string, and
//!    no probe reads a config symbol and calls it support. A version tells you
//!    what upstream shipped; it tells you nothing about what a distribution
//!    disabled, what a security module restricted, or what a container runtime
//!    already spent. Every row below is the kernel's own answer to the exact
//!    request the spawn path will later make.
//!
//! 2. **Namespaces are probed in the combination the spawn will use, not one
//!    flag at a time.** This is the subtle one. An unprivileged process cannot
//!    `unshare(CLONE_NEWNS)` on its own — that needs `CAP_SYS_ADMIN`, and the
//!    call fails `EPERM` on a perfectly healthy machine. It becomes possible
//!    only *inside* a new user namespace, which is exactly how P2.6.2 will
//!    issue it: one `CLONE_NEWUSER | CLONE_NEWNS | …` call, not six. Probing
//!    `CLONE_NEWNS` alone would therefore report "restricted" almost
//!    everywhere and be worse than not probing at all — a measurement that is
//!    reliably wrong in the safe direction still gets a machine written off.
//!    Each namespace row below is `CLONE_NEWUSER | <that flag>`, and the
//!    `ns.all` row is the full set in one call: the request that actually has
//!    to succeed.
//!
//! 3. **A probe may not change the process that runs it.** `unshare` mutates
//!    the caller. Measuring it in-process would mean the core confines itself
//!    as a side effect of asking a question. Every namespace probe therefore
//!    runs in a forked child that calls `unshare` and `_exit` and nothing
//!    else — both syscalls, so the async-signal-safety discipline
//!    [`super`] documents for `pre_exec` is honored here too, for the same
//!    reason: the core is multi-threaded and the child inherits locks it can
//!    never release. The Landlock, seccomp and `no_new_privs` probes need no
//!    fork because each has a query form with no side effect.
//!
//! 4. **"Absent" and "restricted" are different answers.** They are separated
//!    throughout because the operator's remedy differs: a kernel built without
//!    `CONFIG_USER_NS` needs a different kernel, while
//!    `apparmor_restrict_unprivileged_userns=1` needs one sysctl. Collapsing
//!    them into a boolean would produce a matrix that says "no" in two cells
//!    that mean entirely different things.
//!
//! # The floor and the tier are two vocabularies, and they may never merge
//!
//! P2.6.2 (#186, D-036(6)) wired the refusal this module was written for, and
//! it did **not** wire it to [`Tier`]. The distinction is the load-bearing one
//! in this file:
//!
//! - **[`Tier`] measures the machine.** It is the strongest rung this kernel
//!   and this provisioning would permit, and it may never read a build
//!   constant.
//! - **[`FLOOR`] is a property of the *build*.** It is the set of mechanisms
//!   *this binary actually applies*, so it grows one entry per task: `#186
//!   {Namespaces}`, `#187 +{Landlock}` (**landed**), `#188 +{Seccomp,
//!   NoNewPrivs}`. After #188 it coincides exactly with [`Report::tier`]'s
//!   base predicate, and
//!   [`the_floor_is_a_subset_of_the_intra_user_predicate`] asserts
//!   subset-until-then so the day they coincide is a checked fact rather than
//!   a coincidence somebody notices later.
//!
//! Gating `--isolation=default` on `Tier::meets(Tier::IntraUser)` was
//! available and is refused, and #187 is exactly why the distinction earns
//! its keep. At #186 that gate would have refused a pre-5.13 kernel while
//! applying no Landlock at all — a new failure mode bought for nothing.
//! At #187 the *same machine* is refused, and now the refusal is honest:
//! the build applies a ruleset there, so a kernel that cannot supply one is
//! a kernel this build cannot confine as its own documentation says. The
//! floor moved when the applier landed, not before it, and that ordering is
//! the whole rule.
//!
//! [`Tier::meets`] does get its first non-test caller, as the **forecast**
//! rather than the gate ([`forecast`]): on a machine that meets this build's
//! floor but not `Tier::IntraUser`, the core warns and starts, naming the
//! build that will refuse there. That is the only real answer to "how does
//! the floor move without a silent behaviour change" — every floor move is
//! pre-announced on the machines it will break, by the build that still works
//! on them.
//!
//! It does not generate the per-kernel matrix. [`Report::render`] emits
//! the deterministic one-machine rows the matrix is *built from*; collecting
//! rows across a ≥ 6.12, a 6.1-class LTS and a 5.15-class LTS is a separate,
//! multi-machine job, and no amount of code here substitutes for running on
//! those kernels.

use std::fmt;
use std::fs;
use std::path::Path;

use vitrin_realm_init::LandlockRequest;

/// `landlock_create_ruleset(2)`.
///
/// Hard-coded rather than taken from `libc` so the build does not gain a
/// minimum-`libc`-version constraint for one integer. The Landlock syscalls
/// arrived in 5.13 through the *generic* syscall table, so unlike `seccomp`
/// (3.17, per-architecture numbering) the number is uniform across every
/// architecture that has them. On an architecture that does not, the call
/// returns `ENOSYS` and is reported [`Support::Absent`] — which is the
/// truthful answer, so the fallback fails safe rather than silently.
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;

/// `LANDLOCK_CREATE_RULESET_VERSION`. With a null attribute pointer and a zero
/// size this flag turns the create call into a pure ABI query.
const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;

/// `SECCOMP_MODE_FILTER`, for the `prctl` probe described on
/// [`probe_seccomp_filter`].
const SECCOMP_MODE_FILTER: libc::c_int = 2;

/// What one probe found.
///
/// The three failing variants are distinct on purpose; see rule 4 in the
/// module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// The kernel granted the exact request the spawn path will make.
    Available,
    /// The kernel implements the feature; something above it refused —
    /// a sysctl, a security module, or an exhausted per-user limit.
    RestrictedByPolicy(i32),
    /// The kernel does not implement the feature at all.
    Absent(i32),
    /// The probe itself could not be run, so nothing was learned. Never
    /// reported as either success or failure, because it is neither.
    Unmeasured(&'static str),
    /// The kernel implements the feature, at a version **below this build's
    /// declared floor** (owner's decision, 2026-08-15).
    ///
    /// Distinct from every variant above it, and the distinction is the whole
    /// point: nothing is wrong with the machine, no sysctl will change the
    /// answer, and the remedy is a newer kernel. Telling such an operator
    /// `absent` or `restricted-by-policy` would send them looking for a
    /// configuration problem that is not there.
    ///
    /// Carries what was found and what was required, in that order, because a
    /// refusal that names only one of the two cannot be acted on.
    BelowFloor { found: u32, required: u32 },
}

impl Support {
    /// True only for [`Support::Available`]. Written as a method rather than a
    /// `matches!` at each call site so that adding a variant forces a decision
    /// here instead of silently joining the falsy set.
    pub fn is_available(self) -> bool {
        matches!(self, Support::Available)
    }

    /// Classify a failed syscall into [`Support::Absent`] versus
    /// [`Support::RestrictedByPolicy`].
    ///
    /// `EINVAL`/`ENOSYS` mean the kernel does not know the request — an
    /// unsupported clone flag and a missing syscall both land here.
    /// `EPERM`/`EACCES` mean it knows and said no, which is policy.
    /// `ENOSPC` is the nesting-limit answer (`max_user_namespaces`, or an
    /// already-deep namespace stack), which is likewise a limit rather than an
    /// absence — a machine that reports it *can* be fixed without a reboot.
    fn from_errno(errno: i32) -> Self {
        match errno {
            libc::EINVAL | libc::ENOSYS => Support::Absent(errno),
            _ => Support::RestrictedByPolicy(errno),
        }
    }
}

impl fmt::Display for Support {
    /// The rendering used by `vitrind --print-isolation`, and therefore by the
    /// checked-in matrix. Stable by contract: changing a string here changes
    /// every committed matrix cell.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Support::Available => write!(f, "available"),
            Support::RestrictedByPolicy(e) => write!(f, "restricted-by-policy(errno={e})"),
            Support::Absent(e) => write!(f, "absent(errno={e})"),
            Support::Unmeasured(why) => write!(f, "unmeasured({why})"),
            Support::BelowFloor { found, required } => {
                write!(f, "below-floor(abi={found},required={required})")
            }
        }
    }
}

/// The isolation tier a machine can actually deliver.
///
/// This is *not* [`super`]'s eventual `--isolation` selector and it is not
/// D-010's three-rung dial (default / hardened / paranoid). It is the measured
/// ceiling: the strongest rung this kernel and this machine's provisioning
/// would permit. The selector chooses at or below it; D-010's upper two rungs
/// (gVisor-class, microVM) ship in neither Phase 2 nor this enum, and their
/// absence here is deliberate — an enum that named them would imply they exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Nothing measurable. A realm would run with the core's own authority,
    /// which is what `spawn.rs`'s D9 section describes today.
    None,
    /// The full namespace set plus Landlock plus seccomp filter mode: D-020's
    /// intra-user default. Per-realm uid/gid exist inside the realm's own user
    /// namespace; on the host every realm shares one uid.
    IntraUser,
    /// [`Tier::IntraUser`] plus provisioned subordinate ids, so a realm can be
    /// mapped to a *distinct host uid*. D-020 makes this an explicit upgrade,
    /// never an inference: reaching it needs `/etc/subuid` and a
    /// `newuidmap`-class helper, and no packaging in this tree provides either.
    PerUid,
}

impl Tier {
    /// Whether this measured ceiling satisfies a required tier.
    ///
    /// **This is the forecast, not the gate.** P2.6.2 gave it its first
    /// non-test caller ([`forecast`]) and deliberately did not make it the
    /// `--isolation=default` admission test: a tier is a claim about the
    /// machine, and refusing to start because a machine cannot reach a tier
    /// this build does not yet apply would add a failure mode with no
    /// matching safety. What admits a session is [`admit`] against [`FLOOR`],
    /// which is a property of the build.
    ///
    /// The ordering it rests on is [`Tier`]'s own `Ord`, which is why
    /// [`Isolation`] deliberately has none: a *selector* that could be
    /// compared to a *measurement* is a selector that can be silently clamped
    /// down to it.
    pub fn meets(self, floor: Tier) -> bool {
        self >= floor
    }
}

/// One confinement mechanism this build knows how to *apply*.
///
/// Distinct from a [`Report`] row on purpose: a row is a question the kernel
/// answered, a `Mechanism` is something `vitrind` does.
///
/// # `FLOOR` and [`APPLIED`] are two different lists, and the difference is
/// not bookkeeping
///
/// [`APPLIED`] is what this build *does* to a confined realm. [`FLOOR`] is
/// what it *refuses to start without*, checked at startup against a probe.
/// `FLOOR` is a subset of `APPLIED` and may be a proper one: a mechanism can
/// be applied without being a startup gate, and `PR_SET_NO_NEW_PRIVS` is
/// exactly that case. The helper sets it on every confined spawn (a spawn
/// where it failed is refused), but it is not in `FLOOR`, because D-036(6)
/// schedules it to arrive as a *gate* with the seccomp filter it protects at
/// #188 -- and adding it early would refuse whole sessions on a probe for a
/// mechanism whose reason for being checked does not exist yet.
///
/// Until an adversarial review caught it, `--print-floor` rendered its
/// `applies.*` rows straight off `FLOOR`, so it printed
/// `applies.no-new-privs=not-yet` for a mechanism the build applies to every
/// realm. Understating is the safe direction and it was still a false row.
///
/// Every member of `FLOOR` must have an applier, and
/// [`every_floor_mechanism_refuses_when_its_probe_fails`] asserts both
/// directions of the gate relation, because a one-directional check would
/// pass for a `FLOOR` that refused on everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// The six-flag `unshare` `vitrin-realm-init` issues (P2.6.2, #186).
    Namespaces,
    /// The Landlock ruleset `vitrin-realm-init` enforces immediately before
    /// the shim's `execve` (P2.6.3, #187).
    ///
    /// In [`FLOOR`] since #187, which is a **startup behaviour change by
    /// design**: a kernel with no Landlock now refuses `--isolation=default`
    /// rather than starting a session whose realms are path-confined by the
    /// mount table alone. Every machine this breaks was warned by
    /// [`forecast`] in the build before it.
    Landlock,
    /// The seccomp filter (P2.6.4, #188) -- **not applied yet**.
    Seccomp,
    /// `PR_SET_NO_NEW_PRIVS`, which arrives with the filter it protects
    /// (P2.6.4, #188).
    NoNewPrivs,
}

impl fmt::Display for Mechanism {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mechanism::Namespaces => write!(f, "namespaces"),
            Mechanism::Landlock => write!(f, "landlock"),
            Mechanism::Seccomp => write!(f, "seccomp"),
            Mechanism::NoNewPrivs => write!(f, "no-new-privs"),
        }
    }
}

/// What **this build** applies at `--isolation=default`, and therefore what
/// it refuses to start without.
///
/// Two entries. It grows with the tasks that add the appliers, never ahead
/// of them: a floor naming a mechanism nothing applies would refuse sessions
/// in exchange for nothing, which is exactly the trade this module declined
/// at #185.
///
/// `Landlock` joined at #187, on the schedule this module's docs published at
/// #186 and that [`forecast`] has been pre-announcing on every machine the
/// move would break. **It changes startup behaviour**: a kernel that answers
/// the ABI query with `ENOSYS` used to start and now refuses. That is the
/// trade D-020(6) makes explicitly -- the alternative is a session whose
/// realms are confined one mechanism less than its own documentation says.
pub const FLOOR: &[Mechanism] = &[Mechanism::Namespaces, Mechanism::Landlock];

/// What **this build** actually applies to a confined realm, gate or not.
///
/// A superset of [`FLOOR`] (asserted by
/// [`the_floor_is_a_subset_of_what_the_build_applies`]). The two differ by
/// `PR_SET_NO_NEW_PRIVS` today: `vitrin-realm-init` sets it before the shim's
/// `execve` on every confined spawn -- `/proc/<shim>/status` reads
/// `NoNewPrivs: 1`, and the confinement suite asserts it -- but it is not a
/// startup gate until #188 brings the seccomp filter it exists to protect.
///
/// This list is what `--print-floor`'s `applies.*` rows are rendered from. A
/// build that applies a mechanism and prints `not-yet` for it is a build
/// whose own published description is wrong, which is the same class of
/// defect as overclaiming even though it errs the safe way.
pub const APPLIED: &[Mechanism] = &[
    Mechanism::Namespaces,
    Mechanism::Landlock,
    Mechanism::NoNewPrivs,
];

/// The operator's **selection**: which confinement this session applies.
///
/// # Why this is a separate type from [`Tier`], with no bridge to it
///
/// `Tier` is a measurement of the machine; this is a choice about the
/// session. They are related by exactly one function, [`admit`], which
/// returns the request unchanged or a refusal -- never a value in between.
///
/// So this type has, deliberately, **no `From` to or from [`Tier`], no `Ord`
/// or `PartialOrd`, and no `Default`**. Each absence closes one spelling of
/// the same accident: with an ordering, `min(selected, measured)` typechecks
/// and an operator who asked for confinement silently gets less; with a
/// `From`, a measurement can be laundered into a selection; with a `Default`,
/// an unconfined session becomes reachable without anybody typing the word.
/// The rendering sets are disjoint at the bottom for the same reason --
/// `Tier::None` is also what `tier()` returns for `Support::Unmeasured`, so a
/// selector spelled `none` would conflate "the operator chose no confinement"
/// with "nothing was measured".
///
/// The rule is *not* "no shared token". When D-020(3)'s per-uid upgrade
/// becomes selectable, `--isolation=per-uid` may share `per-uid` with the
/// tier, because there the relation is required equality rather than a clamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Namespaces (and, in later builds, more): the confinement `FLOOR`
    /// names. Its meaning grows across upgrades on purpose -- a pinned
    /// spelling like `namespaces+landlock` would make an upgrade
    /// behaviourally inert and push mechanism bookkeeping onto every
    /// operator. Between "refuses loudly after an upgrade" and "silently
    /// confines less after an upgrade", D-020(6) picks the former.
    Default,
    /// No confinement at all: byte for byte the spawn path that shipped
    /// before #186. It exists because D-020(6) needs it as the positive
    /// control for the acceptance gate -- an absence is only evidence if the
    /// same run proves the thing was present somewhere.
    ///
    /// It is **not** a rung of D-010's default/hardened/paranoid dial. It is
    /// the dial being switched out, which has to be said in `--help` or it
    /// reads as an oversight.
    Off,
}

impl Isolation {
    /// Parse the selector's value.
    ///
    /// `none` is rejected **by name** for at least one release rather than
    /// falling into the generic unknown-value message: D-020(6) is an
    /// accepted decision that quotes `--isolation=none`, so everybody who
    /// read it will type that word, and the copy has to tell them the token
    /// moved rather than that they mistyped. Same precedent as
    /// `parse_consent`'s retired spelling.
    pub fn parse(value: &str) -> Result<Isolation, String> {
        match value {
            "default" => Ok(Isolation::Default),
            "off" => Ok(Isolation::Off),
            "none" => Err(
                "`--isolation=none` was renamed to `--isolation=off` before it shipped. \
                 D-020(6) minted `none`, but `none` is also what the *measured tier* reports \
                 when nothing could be measured at all (`--print-isolation`, `tier=none`), and \
                 one token for \"the operator chose no confinement\" and \"this machine was \
                 never measured\" is one token too few"
                    .to_string(),
            ),
            other => Err(format!(
                "unknown `--isolation` value {other:?} (expected `default` or `off`)"
            )),
        }
    }
}

impl fmt::Display for Isolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Isolation::Default => write!(f, "default"),
            Isolation::Off => write!(f, "off"),
        }
    }
}

/// The selections [`admit`] let through.
///
/// **It carries no profile string**, and the absence is deliberate. A
/// profile names what a realm *got*, and at admission time no realm exists:
/// the kernel's ABI is known, the ladder's landing is not. [`profile_for`]
/// is therefore called from the spawn path, once per realm, with the rung
/// that realm's PID 1 reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    isolation: Isolation,
    /// The session's `--landlock` selection, carried here because the spawn
    /// path has to send it to the helper and the journal has to record it
    /// beside the obtained rung: "asked for the highest and got rung 1" and
    /// "asked for rung 1" are different sessions.
    landlock: LandlockRequest,
}

impl Applied {
    pub fn isolation(self) -> Isolation {
        self.isolation
    }

    /// The session's Landlock selection, for the spawn path that has to send
    /// it to the helper.
    pub fn landlock(self) -> LandlockRequest {
        self.landlock
    }
}

/// The value the flight recorder's `applied_profile` field carries, as a
/// function of the session's isolation selection and **the rung the realm
/// actually obtained**.
///
/// # The second argument is the obtained rung, and that is the whole point
///
/// The first version of this function took the session's *request*, which is
/// the one thing that cannot describe what happened. `--landlock=abi:9` on an
/// ABI-3 kernel rendered `namespaces+landlock-abi9`; `--landlock=highest`
/// rendered the same string whether the ladder settled on rung 9 or fell all
/// the way to rung 1. The field is named `applied_profile`, so it reports
/// what was applied: `0` (no ruleset) renders `namespaces-only`, and every
/// other rung renders itself. A session's *request* is journaled beside it as
/// `landlock.requested`, because the request and the grant are two facts and
/// neither is derivable from the other.
///
/// **It may not overclaim upward either**, which is why it is still not the
/// tier's spelling: `Tier::IntraUser` is *defined* as namespaces plus
/// Landlock plus seccomp, #187 applies two of the three, and #188 owns the
/// filter. An entry printing `intra-user` would assert confinement that does
/// not exist.
pub fn profile_for(isolation: Isolation, obtained_rung: u32) -> &'static str {
    match (isolation, obtained_rung) {
        (Isolation::Off, _) => "none",
        // No ruleset at all: `--landlock=off`, and the one word the pre-#187
        // build used for exactly this state.
        (Isolation::Default, 0) => "namespaces-only",
        (Isolation::Default, 1) => "namespaces+landlock-abi1",
        (Isolation::Default, 2) => "namespaces+landlock-abi2",
        (Isolation::Default, 3) => "namespaces+landlock-abi3",
        (Isolation::Default, 4) => "namespaces+landlock-abi4",
        (Isolation::Default, 5) => "namespaces+landlock-abi5",
        (Isolation::Default, 6) => "namespaces+landlock-abi6",
        (Isolation::Default, 7) => "namespaces+landlock-abi7",
        (Isolation::Default, 8) => "namespaces+landlock-abi8",
        (Isolation::Default, 9) => "namespaces+landlock-abi9",
        // Unreachable: the helper's ladder cannot climb above
        // `LANDLOCK_BUILD_MAX_RUNG`, so a rung above it means the helper is
        // not this build. Spelled as a refusal to name a rung rather than as
        // a rung, because a profile string that guessed would be the one
        // field in the journal nobody could check.
        (Isolation::Default, _) => "namespaces+landlock-unknown-rung",
    }
}

/// Why a session may not start with the confinement it asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub mechanism: Mechanism,
    pub support: Support,
    /// What the operator can actually do, derived from
    /// [`read_policy_knobs`]' real readings -- never from a distro guess.
    /// Where no known knob explains the failure, this says so and invents
    /// nothing.
    pub remedy: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this build's isolation floor requires `{}` and this machine reports `{}`. {}",
            self.mechanism, self.support, self.remedy
        )
    }
}

/// Relate a request to a measurement. **The only function that does.**
///
/// It returns the request unchanged or a refusal; there is no third answer,
/// which is what makes "the session silently confined less than it was told
/// to" unrepresentable rather than merely unlikely.
///
/// `Off` is admitted on every machine -- it asks for nothing, so nothing can
/// be missing. The caller owes it a standing warning and a journal entry;
/// this function does not warn, because a pure relation that logged would be
/// a relation with a side effect.
pub fn admit(
    requested: Isolation,
    landlock: LandlockRequest,
    report: &Report,
) -> Result<Applied, Refusal> {
    let applied = Applied {
        isolation: requested,
        landlock,
    };
    if requested == Isolation::Off {
        return Ok(applied);
    }
    for mechanism in FLOOR {
        // `--landlock=off` is the operator switching one floor mechanism off
        // by name, exactly as `--isolation=off` switches all of them off. It
        // does **not** license skipping the probe silently: the caller owes
        // the session a standing warning, the same way `Isolation::Off` does,
        // and every realm it spawns reports rung 0, which [`profile_for`]
        // renders `namespaces-only` -- so no journal entry from such a
        // session can read as a confined one.
        if *mechanism == Mechanism::Landlock && landlock == LandlockRequest::Off {
            continue;
        }
        let support = report.mechanism(*mechanism);
        if support.is_available() {
            continue;
        }
        return Err(Refusal {
            mechanism: *mechanism,
            support,
            remedy: remedy_for(*mechanism, support, report),
        });
    }
    Ok(applied)
}

/// The remedy paragraph, composed from what was actually read.
///
/// `Support::Unmeasured` **refuses** and gets its own copy: treating "unknown"
/// as "fine" is exactly the silent degradation D-020(6) forbids, and an
/// operator told "restricted" when the truth is "the probe could not run"
/// would go looking for a sysctl that is not the problem.
fn remedy_for(mechanism: Mechanism, support: Support, report: &Report) -> String {
    if let Support::Unmeasured(why) = support {
        return format!(
            "The probe for `{mechanism}` could not be run at all ({why}), so this machine's \
             support is unknown. An unknown answer is refused rather than assumed good: a \
             session that started here would make every confinement claim downstream read as \
             true while being unverified. Re-run `vitrind --print-isolation` from a plain \
             shell to see the same probe outside whatever launched this core."
        );
    }
    // A real remedy, because there is one: unlike seccomp, a missing Landlock
    // has exactly three causes and an operator can check all three. It is
    // stated as three things to *look at* rather than one thing to do,
    // because two of them need a reboot and the module's own rule is that a
    // fabricated remedy is worse than silence.
    // A kernel that HAS Landlock, below this build's declared floor. Its own
    // paragraph, because none of the three causes below applies: the LSM is
    // present, enabled and answering, and the only thing that moves the number
    // is a newer kernel. Handing this operator the `lsm=` boot parameter would
    // send them to reconfigure something that is already correct.
    if let Support::BelowFloor { found, required } = support {
        return format!(
            "This kernel has Landlock and reports ABI {found}; this build's floor is ABI \
             {required} (owner's decisions of 2026-08-15 and 2026-08-16: declare a floor rather \
             than publish a multi-rung ladder nothing measures, and set it at the lowest rung \
             that gives up no enforcement). Nothing is misconfigured here and \
             no sysctl, LSM list or boot parameter will change the number -- the remedy is a \
             newer kernel. `uname -r` says {release}, and `vitrind --print-floor` prints the \
             required number as `build.landlock_min_abi`. This build will not fall back to a \
             lower rung: a realm confined by a weaker domain than the session's own journal \
             names is the silent degradation D-020(6) exists to forbid. `--landlock=off` \
             starts a session whose realms get NO ruleset at all -- it is the positive control \
             this repository's confinement gates run against, not a way to run on an older \
             kernel.",
            release = report.kernel_release,
        );
    }
    if mechanism == Mechanism::Landlock {
        return format!(
            "Landlock is a startup requirement since P2.6.3 (#187): the realm's filesystem \
             confinement is a ruleset `vitrin-realm-init` enforces before the shim's execve, \
             and a session that could not build one would confine realms by mount table alone \
             while its own journal said otherwise. This kernel answered `{support}`. Three \
             things produce that, in the order worth checking: (1) the kernel predates 5.13, \
             which is where Landlock arrived -- `uname -r` says {release}; (2) it was built \
             without `CONFIG_SECURITY_LANDLOCK` (check `zgrep CONFIG_SECURITY_LANDLOCK \
             /proc/config.gz`, or the matching file under /boot); (3) it has the code but \
             `landlock` is missing from the active LSM list, which several distributions do \
             by default -- read `/sys/kernel/security/lsm`, and add `landlock` to the `lsm=` \
             boot parameter (keeping every name already there) if it is absent. \
             `--landlock=off` starts a session whose realms get NO ruleset and is the wrong \
             answer to a kernel that could be configured; `--isolation=off` is weaker still.",
            release = report.kernel_release,
        );
    }
    if mechanism != Mechanism::Namespaces {
        return format!(
            "No knob in this build's list explains a `{mechanism}` failure, so no remedy is \
             offered rather than one being guessed at. `vitrind --print-isolation` prints \
             every row this core measured."
        );
    }
    // Only the knobs that were actually read, and only the ones whose value
    // explains the failure. A guess here is worse than silence: an operator
    // who follows a fabricated remedy and sees no change concludes the
    // diagnosis is broken rather than that the cause is elsewhere.
    let mut lines = Vec::new();
    // Which half failed changes what the operator should go looking at, so it
    // is said before the knobs. Without this, a machine that grants the
    // namespace and denies the mount gets handed three userns sysctls that all
    // already read fine -- a remedy that sends someone to verify the one thing
    // that is not broken.
    if report.namespaces_combined.is_available() && !report.mount_in_userns.is_available() {
        lines.push(format!(
            "The user and mount namespaces were granted; what failed is the first mount \
             *inside* them (`mount(NULL, \"/\", NULL, MS_REC|MS_PRIVATE, NULL)` answered \
             {}). That combination means something stripped the capabilities the new user \
             namespace was supposed to confer -- on Ubuntu 24.04+ the usual cause is \
             AppArmor's unprivileged-userns restriction, which permits the unshare and then \
             confines the process to a profile that denies CAP_SYS_ADMIN inside it. This \
             core's own label is the `apparmor.label` row of `vitrind --print-isolation`, \
             and it is what separates `unconfined` (nothing attached) from a named profile \
             that attached and conferred nothing -- those two are indistinguishable in this \
             errno alone.",
            report.mount_in_userns
        ));
    }
    for (key, value) in &report.policy {
        match (*key, value.as_deref()) {
            ("max_user_namespaces", Some("0")) => lines.push(
                "/proc/sys/user/max_user_namespaces is 0, which disables unprivileged user \
                 namespaces outright; raise it (sysctl user.max_user_namespaces=15000)."
                    .to_string(),
            ),
            ("unprivileged_userns_clone", Some("0")) => lines.push(
                "/proc/sys/kernel/unprivileged_userns_clone is 0 (Debian's downstream switch); \
                 set it to 1."
                    .to_string(),
            ),
            ("apparmor_restrict_unprivileged_userns", Some("1")) => lines.push(
                "/proc/sys/kernel/apparmor_restrict_unprivileged_userns is 1 (Ubuntu 24.04+); \
                 either set it to 0 or ship an AppArmor profile for vitrind that grants \
                 userns creation. This repository carries one at \
                 `packaging/apparmor/vitrind` -- read its header before installing it: it \
                 has not been verified to work anywhere, and it names the path the binaries \
                 have to be installed at for it to attach at all."
                    .to_string(),
            ),
            _ => {}
        }
    }
    if lines.is_empty() {
        format!(
            "None of the three knobs that could explain this \
             (user.max_user_namespaces, kernel.unprivileged_userns_clone, \
             kernel.apparmor_restrict_unprivileged_userns) does, so no remedy is \
             offered rather than one being guessed at. (This core reads a fourth, \
             kernel.apparmor_restrict_unprivileged_unconfined, and it is deliberately not in \
             that list: it sizes what an AppArmor profile for vitrind would COST, and \
             explains no denial. A knob in a remedy that cannot move the failure is a \
             fabricated remedy.) The kernel answered `{support}`; \
             `vitrind --print-isolation` prints every row behind that answer. \
             `--isolation=off` starts an UNCONFINED session and is the wrong answer to a \
             machine that could be fixed."
        )
    } else {
        format!(
            "{} `--isolation=off` starts an UNCONFINED session and is the wrong answer to a \
             machine that could be fixed.",
            lines.join(" ")
        )
    }
}

/// The `vitrind --print-floor` payload.
///
/// A **separate verb from `--print-isolation`**, and the separation is the
/// point: a build constant among kernel facts would contradict this module's
/// own first rule, and it would invalidate #185's four-kernel matrix over a
/// row that is not a kernel fact.
pub fn render_floor() -> String {
    let mut out = String::from("vitrin-floor 1\n");
    out.push_str(&format!("build.version={}\n", env!("CARGO_PKG_VERSION")));
    for mechanism in FLOOR {
        out.push_str(&format!("floor.mechanism={mechanism}\n"));
    }
    // Stated as rows rather than left to inference, so an operator can see
    // which mechanisms this build knows about but does not yet apply.
    //
    // Rendered from [`APPLIED`] and **not** from `FLOOR`. They are not the
    // same list: `no-new-privs` is set on every confined spawn and is not a
    // startup gate until #188, so reading the row off `FLOOR` printed
    // `applies.no-new-privs=not-yet` about a mechanism this build applies.
    // `floor.mechanism=` above is the gate; these rows are the behaviour.
    for mechanism in [
        Mechanism::Namespaces,
        Mechanism::Landlock,
        Mechanism::Seccomp,
        Mechanism::NoNewPrivs,
    ] {
        let applied = APPLIED.contains(&mechanism);
        out.push_str(&format!(
            "applies.{mechanism}={}\n",
            if applied { "yes" } else { "not-yet" }
        ));
    }
    // The highest Landlock ABI rung this build knows how to ask for, and the
    // number a kernel newer than this build gets clamped down to (P2.6.3,
    // #187). A **build** constant, so this verb and not `--print-isolation`
    // is its home -- the kernel's own ABI is a measured row over there, and
    // the two are only interesting side by side.
    //
    // Printed rather than left implicit because the clamp is otherwise
    // invisible in advance: an operator on an ABI-10 kernel gets a rung-9
    // ruleset, which is correct and is also one rung narrower than their
    // kernel would allow. The per-realm journal reports the clamp after the
    // fact (`isolation.landlock.clamped_by_build`); this row is how it can be
    // seen before a realm exists.
    out.push_str(&format!(
        "build.landlock_max_rung={}\n",
        vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG
    ));
    // The **floor**, and the row that tells an operator on an older kernel why
    // a session with a working Landlock refused to start (owner's decision,
    // 2026-08-15). It is a build constant like the row above it, so it lives
    // here and not in `--print-isolation`, which prints the kernel's own
    // `landlock.abi=` -- the two are only interesting side by side.
    out.push_str(&format!(
        "build.landlock_min_abi={}\n",
        vitrin_realm_init::LANDLOCK_MIN_ABI
    ));
    out
}

/// The forward warning [`Tier::meets`] exists for: this machine meets the
/// floor, but a build that adds the next mechanism will refuse here.
///
/// `None` when there is nothing to forecast -- the machine already reaches
/// `Tier::IntraUser`, so no scheduled floor move will break it.
pub fn forecast(report: &Report) -> Option<String> {
    if report.tier().meets(Tier::IntraUser) {
        return None;
    }
    let mut missing = Vec::new();
    for mechanism in [
        Mechanism::Namespaces,
        Mechanism::Landlock,
        Mechanism::Seccomp,
        Mechanism::NoNewPrivs,
    ] {
        if FLOOR.contains(&mechanism) {
            continue;
        }
        let support = report.mechanism(mechanism);
        if !support.is_available() {
            missing.push(format!("{mechanism}={support}"));
        }
    }
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "this build's isolation floor is [{}] and this machine meets it; the machine does not \
         meet the `{}` tier ({}), so a build that adds those mechanisms will REFUSE to start \
         here. Every floor move is announced by the build that still works on the machines it \
         will break -- this is that announcement",
        FLOOR
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        Tier::IntraUser,
        missing.join(", "),
    ))
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tier::None => write!(f, "none"),
            Tier::IntraUser => write!(f, "intra-user"),
            Tier::PerUid => write!(f, "per-uid"),
        }
    }
}

/// One namespace row: the flag, its stable output key, and what was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamespaceProbe {
    pub key: &'static str,
    pub support: Support,
}

/// Everything one machine reported, in the fixed order it is rendered.
///
/// Field order *is* output order: [`Report::render`] walks these explicitly
/// rather than iterating a map, because a matrix whose row order depends on
/// hash iteration cannot be byte-compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// `uname -r`. A probed value like any other; the matrix keys its rows on
    /// this rather than embedding it in a cell, so a CI image bump moves one
    /// row instead of dirtying every cell.
    pub kernel_release: String,
    /// Per-namespace results, each measured as `CLONE_NEWUSER | <flag>`.
    pub namespaces: Vec<NamespaceProbe>,
    /// The full set in one call — the first half of the request P2.6.2 issues.
    pub namespaces_combined: Support,
    /// Whether a mount succeeds inside a fresh user+mount namespace — the
    /// second half. Separate from [`Report::namespaces_combined`] because the
    /// two answers differ on real machines (see [`probe_mount_in_userns`]) and
    /// the operator's remedy differs with them: a denied `unshare` is a
    /// namespace policy, a denied mount inside a granted namespace is an LSM
    /// stripping capabilities the namespace was supposed to confer.
    pub mount_in_userns: Support,
    /// The integer `landlock_create_ruleset(NULL, 0,
    /// LANDLOCK_CREATE_RULESET_VERSION)` returned. Reported as the ABI number
    /// rather than a boolean because P2.6.3's degradation ladder consumes the
    /// number directly: ABI 3 is where `LANDLOCK_ACCESS_FS_TRUNCATE` arrives,
    /// and below it a designated read-only fd can still be truncated to zero.
    pub landlock_abi: Result<u32, Support>,
    pub seccomp_filter: Support,
    pub no_new_privs: Support,
    /// Distro-level knobs, each as the raw file contents or absence. These are
    /// *explanatory*, never authoritative: the namespace probes above already
    /// hold the answer, and these say why.
    pub policy: Vec<(&'static str, Option<String>)>,
    /// This process's own AppArmor label, or `absent` where AppArmor is not
    /// the LSM answering (see [`read_apparmor_label`]).
    ///
    /// **The one row here that describes the PROCESS rather than the
    /// machine**, and it is here because on an Ubuntu 24.04+ host it is the
    /// only thing that separates two outcomes with identical errnos. Where
    /// `apparmor_restrict_unprivileged_userns=1`, a task labelled
    /// `unconfined` gets its `unshare` permitted and the capabilities inside
    /// stripped, so [`Report::mount_in_userns`] answers
    /// `restricted-by-policy(errno=13)`. A task under a profile that grants
    /// `userns` should get both — but a profile that loaded and did NOT
    /// attach, and a profile that attached and did not grant, produce *the
    /// same* `errno=13`. Only the label tells them apart, and the refusal
    /// text this module already prints sends operators to read exactly this
    /// file. Carrying it in the matrix means the diagnosis and its evidence
    /// arrive together instead of one command apart.
    pub apparmor_label: Option<String>,
    /// Whether the per-uid upgrade is provisioned on this machine.
    pub subuid_range: bool,
    pub newuidmap: bool,
}

impl Report {
    /// Run every probe against the running kernel.
    ///
    /// # Safety-adjacent constraint
    ///
    /// This forks. Call it from a single-threaded context — the
    /// `--print-isolation` path is one, being upstream of `init_tracing` and
    /// of every backend. The forked children touch only `unshare` and
    /// `_exit`, so a multi-threaded caller is not *unsound*, but the
    /// discipline is stated so nobody later adds an allocation to the child.
    pub fn probe() -> Self {
        // Every namespace row below reads its answer out of a child's exit
        // status, so the probes are only measurable while this process
        // actually reaps its own children. See [`SigchldDefault`] for the
        // launcher state that would otherwise void all seven rows at once.
        let _sigchld = SigchldDefault::install();

        let namespaces = vec![
            NamespaceProbe {
                key: "ns.user",
                support: probe_unshare(libc::CLONE_NEWUSER),
            },
            NamespaceProbe {
                key: "ns.mount",
                support: probe_unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS),
            },
            NamespaceProbe {
                key: "ns.pid",
                support: probe_unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID),
            },
            NamespaceProbe {
                key: "ns.ipc",
                support: probe_unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWIPC),
            },
            NamespaceProbe {
                key: "ns.uts",
                support: probe_unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWUTS),
            },
            NamespaceProbe {
                key: "ns.net",
                support: probe_unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET),
            },
        ];

        let combined = probe_unshare(
            libc::CLONE_NEWUSER
                | libc::CLONE_NEWNS
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWUTS
                | libc::CLONE_NEWNET,
        );

        Report {
            kernel_release: kernel_release(),
            namespaces,
            namespaces_combined: combined,
            mount_in_userns: probe_mount_in_userns(),
            landlock_abi: probe_landlock_abi(),
            seccomp_filter: probe_seccomp_filter(),
            no_new_privs: probe_no_new_privs(),
            policy: read_policy_knobs(),
            apparmor_label: read_apparmor_label(),
            subuid_range: has_subuid_range(),
            newuidmap: has_newuidmap(),
        }
    }

    /// What this machine said about one [`Mechanism`].
    ///
    /// The seam between "a row the kernel answered" and "a thing this build
    /// applies", written once so [`admit`], [`forecast`] and [`Report::tier`]
    /// cannot disagree about which row backs which mechanism. The
    /// `Namespaces` row is [`Report::namespaces_combined`] and not any
    /// individual namespace: six probes that each pass separately do not
    /// prove the one six-flag call `vitrin-realm-init` issues will pass, and
    /// it is that call the mechanism is.
    pub fn mechanism(&self, mechanism: Mechanism) -> Support {
        match mechanism {
            // Both halves of the spawn's request, and the *failing* half is
            // what gets reported: an operator handed "restricted-by-policy"
            // needs the errno of the call that actually said no. The unshare
            // is checked first because a mount row measured after a failed
            // unshare says nothing about mounting.
            Mechanism::Namespaces => {
                if self.namespaces_combined.is_available() {
                    self.mount_in_userns
                } else {
                    self.namespaces_combined
                }
            }
            // **The floor is an ABI number, not a boolean** (owner's decision,
            // 2026-08-15). A kernel that has Landlock at a rung below
            // `LANDLOCK_MIN_ABI` cannot supply the domain this build applies,
            // so it is refused here rather than degraded at spawn -- the same
            // D-020(6) trade the mechanism itself was added under.
            Mechanism::Landlock => match self.landlock_abi {
                Ok(abi) if abi >= vitrin_realm_init::LANDLOCK_MIN_ABI => Support::Available,
                Ok(abi) if abi >= 1 => Support::BelowFloor {
                    found: abi,
                    required: vitrin_realm_init::LANDLOCK_MIN_ABI,
                },
                Ok(_) => Support::Unmeasured("landlock returned ABI 0"),
                Err(support) => support,
            },
            Mechanism::Seccomp => self.seccomp_filter,
            Mechanism::NoNewPrivs => self.no_new_privs,
        }
    }

    /// The measured ceiling.
    ///
    /// Keyed on [`Report::mechanism`] for the namespace rung, not on the
    /// individual rows: six probes that each pass separately do not prove the
    /// one call P2.6.2 makes will pass, and that call passing does not prove
    /// the mount after it will — which is why this reads the mechanism rather
    /// than [`Report::namespaces_combined`] directly. A tier that counted the
    /// unshare alone called a GitHub runner `IntraUser` on a machine where no
    /// realm could start.
    ///
    /// # The Landlock rung here is `>= 1`, and it deliberately is **not** the
    /// build's ABI floor
    ///
    /// Since 2026-08-15 this build refuses to start below
    /// [`vitrin_realm_init::LANDLOCK_MIN_ABI`], which is a **build** decision.
    /// A kernel at ABI 4 has Landlock, would carry an intra-user tier for a
    /// build that asked for a rung it can supply, and is refused by *this* one.
    /// Both statements are true at once, and this module's first rule is that
    /// the floor and the tier are separate vocabularies -- so `tier` keeps
    /// answering the machine question and [`Report::mechanism`] answers the
    /// build question. The consequence, stated because it looks like a
    /// contradiction in a terminal: on such a machine `--print-isolation` can
    /// print `tier=intra-user` while `vitrind` refuses to start, and the two
    /// rows that explain it are `landlock.abi=` here and
    /// `build.landlock_min_abi=` under `--print-floor`.
    pub fn tier(&self) -> Tier {
        let base = self.mechanism(Mechanism::Namespaces).is_available()
            && matches!(self.landlock_abi, Ok(abi) if abi >= 1)
            && self.seccomp_filter.is_available()
            && self.no_new_privs.is_available();

        if !base {
            return Tier::None;
        }
        if self.subuid_range && self.newuidmap {
            Tier::PerUid
        } else {
            Tier::IntraUser
        }
    }

    /// The `vitrind --print-isolation` payload.
    ///
    /// Deterministic by contract, on the same rule the generated wire code
    /// holds: free of timestamps, hostnames, pids and any other per-run
    /// varying content. The checked-in matrix is byte-compared against this,
    /// so anything that varies between two runs on one machine is a bug here,
    /// not flake in the checker.
    pub fn render(&self) -> String {
        let mut out = String::new();
        // A schema version, so a matrix committed against an older row set is
        // recognizable as stale rather than merely different. Bumped to 2 when
        // `mount.in_userns` was added: a version-1 matrix is not merely
        // missing a row, its `tier` cell was computed without that row and can
        // read a rung too high, so it has to be recollected rather than
        // patched. That is the whole reason this number is here.
        //
        // Bumped to 3 for `apparmor.label` and
        // `policy.apparmor_restrict_unprivileged_unconfined` (issue #286), and
        // the *reason* differs from the 1 -> 2 bump in a way worth stating:
        // neither new row feeds [`Report::tier`], so a version-2 matrix's
        // `tier` cell is still correct. It is missing explanation, not
        // measurement. A version-2 row set is therefore readable as-is and
        // only needs recollecting if you want to know *why* its
        // `mount.in_userns` cell says what it says.
        out.push_str("vitrin-isolation 3\n");
        out.push_str(&format!("kernel.release={}\n", self.kernel_release));
        for probe in &self.namespaces {
            out.push_str(&format!("{}={}\n", probe.key, probe.support));
        }
        out.push_str(&format!("ns.all={}\n", self.namespaces_combined));
        out.push_str(&format!("mount.in_userns={}\n", self.mount_in_userns));
        match self.landlock_abi {
            Ok(abi) => out.push_str(&format!("landlock.abi={abi}\n")),
            Err(support) => out.push_str(&format!("landlock.abi={support}\n")),
        }
        out.push_str(&format!("seccomp.filter={}\n", self.seccomp_filter));
        out.push_str(&format!("no_new_privs={}\n", self.no_new_privs));
        for (key, value) in &self.policy {
            match value {
                Some(v) => out.push_str(&format!("policy.{key}={v}\n")),
                None => out.push_str(&format!("policy.{key}=unset\n")),
            }
        }
        // Rendered beside the policy knobs because it explains the same cell
        // they do, and after them because it is the narrower answer: the knobs
        // say what the machine's policy is, this says what it decided about
        // *this* process.
        out.push_str(&format!(
            "apparmor.label={}\n",
            self.apparmor_label.as_deref().unwrap_or("absent")
        ));
        out.push_str(&format!(
            "provisioning.subuid={}\n",
            if self.subuid_range {
                "present"
            } else {
                "absent"
            }
        ));
        out.push_str(&format!(
            "provisioning.newuidmap={}\n",
            if self.newuidmap { "present" } else { "absent" }
        ));
        out.push_str(&format!("tier={}\n", self.tier()));
        out
    }
}

/// Hold `SIGCHLD` at `SIG_DFL` for as long as the probes run, restoring
/// whatever was there on drop.
///
/// # The failure this exists to stop
///
/// Under `SIGCHLD = SIG_IGN` the kernel reaps children itself, and a
/// subsequent `waitpid` on the child returns `-1`/`ECHILD` instead of its exit
/// status. Every namespace row is *encoded in* that exit status, so the whole
/// set would come back `unmeasured` and [`Report::tier`] would report
/// [`Tier::None`] — on a machine that grants every namespace it was asked for.
/// Once P2.6.2 wires D-020(6)'s floor onto [`Tier::meets`], the same launcher
/// state would make `vitrind` *refuse to start* on a fully capable kernel.
///
/// This is not a hypothetical disposition. `execve(2)` resets *caught* signals
/// to `SIG_DFL` but deliberately **preserves `SIG_IGN`** — a rule
/// [`super`]'s own module docs already state, for the mirror-image reason
/// (every disposition the core inherits survives into the shim). And
/// `signal(SIGCHLD, SIG_IGN)` is the standard zombie-avoidance idiom in
/// supervisors, service wrappers, shells (`trap "" CHLD`) and CI runners, so
/// the core can plausibly be launched under it.
///
/// Resetting is the only available fix rather than the tidy one: under
/// `SIG_IGN` the exit status is *destroyed*, not merely hard to read, so no
/// amount of care in the parent can recover a measurement taken under it.
struct SigchldDefault {
    previous: libc::sigaction,
    /// False when the save itself failed, in which case restoring would write
    /// a zeroed disposition over a live one — worse than leaving it alone.
    restore: bool,
}

impl SigchldDefault {
    fn install() -> Self {
        // SAFETY: both structs are fully initialized before use and outlive
        // the calls; `sigaction` writes the previous disposition into
        // `previous` and reads `desired`.
        unsafe {
            let mut previous: libc::sigaction = std::mem::zeroed();
            let mut desired: libc::sigaction = std::mem::zeroed();
            desired.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut desired.sa_mask);
            let rc = libc::sigaction(libc::SIGCHLD, &desired, &mut previous);
            SigchldDefault {
                previous,
                restore: rc == 0,
            }
        }
    }
}

impl Drop for SigchldDefault {
    fn drop(&mut self) {
        if !self.restore {
            return;
        }
        // SAFETY: `previous` was written by a successful `sigaction` above and
        // is still a valid disposition for this process.
        unsafe {
            libc::sigaction(libc::SIGCHLD, &self.previous, std::ptr::null_mut());
        }
    }
}

/// Measure one `unshare` request in a forked child.
///
/// The child calls exactly two syscalls. Success is encoded as exit status 0
/// and failure as the errno, which is unambiguous because no errno is zero.
fn probe_unshare(flags: libc::c_int) -> Support {
    probe_unshare_reporting_pid(flags).1
}

/// Measure whether a mount actually succeeds *inside* a fresh user+mount
/// namespace — the question [`probe_unshare`] does not ask.
///
/// # Why this row exists, and what it cost to learn
///
/// Rule 1 of this module is *attempt, never infer*, and it is stated as
/// "every row is the kernel's own answer to the exact request the spawn path
/// will later make". The namespace rows honored the letter of that and missed
/// its point: `vitrin-realm-init`'s request is not `unshare` alone, it is
/// `unshare` **and then** `mount(NULL, "/", NULL, MS_REC|MS_PRIVATE, NULL)`,
/// the propagation change every later bind depends on. Probing only the first
/// half infers the second, which is precisely what rule 1 forbids.
///
/// P2.6.2's first CI run is what exposed the gap. On a GitHub `ubuntu-latest`
/// runner the `unshare` returned 0, this preflight therefore reported the
/// machine fit, `vitrind` booted — and then every realm spawn was refused
/// because that first mount returned `EPERM`. The refusal was correct and came
/// from #186's own post-spawn verification; the point is that **this gate,
/// written to catch exactly that machine, passed it**. A preflight that
/// certifies a box which cannot confine is worse than no preflight, because
/// the tier it prints is then read as evidence.
///
/// # Why the fork is still enough isolation
///
/// Rule 3 says a probe may not change the process that runs it, which is what
/// kept the earlier probes to a bare `unshare`. It does not forbid this: by
/// the time the child mounts, it is already in a mount namespace of its own
/// that nothing else shares, so the mount is unobservable outside the child
/// and dies with its `_exit`. `mount` is a plain syscall, so the child stays
/// async-signal-safe, and the path argument is a `static` NUL-terminated
/// literal — no allocation is reachable after the fork.
fn probe_mount_in_userns() -> Support {
    // A `static` rather than a `CString`: this pointer is dereferenced in a
    // forked child, where an allocation would be a deadlock waiting for a
    // malloc lock the child can never see released.
    static ROOT: &[u8] = b"/\0";
    probe_in_forked_child(|| {
        let rc = unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) };
        if rc != 0 {
            // Reported rather than masked. The row then reads the same as
            // `ns.mount`, which is the honest answer: the mount was never
            // reached, so nothing was learned about it.
            return unsafe { *libc::__errno_location() };
        }
        let rc = unsafe {
            libc::mount(
                std::ptr::null(),
                ROOT.as_ptr().cast(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            )
        };
        if rc != 0 {
            return unsafe { *libc::__errno_location() };
        }
        0
    })
    .1
}

/// [`probe_unshare`], additionally reporting the pid it forked and reaped
/// (`-1` when the fork itself failed).
///
/// The pid exists for one reason: it lets a test prove the reap happened
/// against *that exact child* rather than against the process-wide child set.
/// The process-wide check is unusable here — this binary's test suite spawns
/// real realms in other tests, and they run concurrently, so a
/// `waitpid(-1, WNOHANG)` would be answering a question about whichever test
/// happened to be mid-spawn.
fn probe_unshare_reporting_pid(flags: libc::c_int) -> (libc::pid_t, Support) {
    probe_in_forked_child(move || {
        let rc = unsafe { libc::unshare(flags) };
        if rc == 0 {
            0
        } else {
            unsafe { *libc::__errno_location() }
        }
    })
}

/// The fork / measure / reap harness every probe in this module shares.
///
/// `body` runs in the child and returns `0` for success or an errno for
/// failure; the harness encodes that as the exit status and decodes it back
/// into a [`Support`]. Written once rather than per probe because the
/// interesting part is not the fork, it is the `waitpid` accounting below —
/// two copies of that would mean a fix to one silently not reaching the other.
///
/// # Contract on `body`
///
/// It runs **after `fork` in a possibly multi-threaded process**, so it may
/// call nothing but async-signal-safe functions: no allocation, no locking, no
/// `std` I/O, no Rust destructor in scope. Every current caller is syscalls
/// and `__errno_location` (a thread-local address, read directly so that no
/// `std` machinery sits between the fork and the exit at all). `Fn` rather
/// than `FnOnce` so no closure state needs dropping in the child.
fn probe_in_forked_child<F>(body: F) -> (libc::pid_t, Support)
where
    F: Fn() -> libc::c_int,
{
    // SAFETY: `fork` in a multi-threaded process is safe as long as the child
    // reaches `_exit` through async-signal-safe calls only, which is the
    // contract `body` carries above.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => (-1, Support::Unmeasured("fork failed")),
        0 => {
            // Child. Nothing here may allocate or unwind.
            let errno = body();
            // Errnos are small positive integers; clamp defensively rather
            // than truncating silently into a different errno.
            let code = if errno == 0 { 0 } else { errno.clamp(1, 255) };
            unsafe { libc::_exit(code as libc::c_int) };
        }
        _ => {
            let mut status: libc::c_int = 0;
            // SAFETY: `pid` is our own child and `status` is a valid pointer.
            let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
            if waited != pid {
                // `ECHILD` here means the child was auto-reaped before we could
                // read it, which happens only under `SIGCHLD = SIG_IGN`.
                // [`SigchldDefault`] is installed for exactly this reason, so
                // reaching this arm means something re-ignored `SIGCHLD`
                // underneath the probe. Named separately from a generic
                // failure because the exit status is *destroyed* in that case
                // and no retry can recover it — the operator needs the cause,
                // not a shrug.
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                let why = if errno == libc::ECHILD {
                    "probe child auto-reaped (SIGCHLD ignored); exit status unrecoverable"
                } else {
                    "waitpid failed"
                };
                return (pid, Support::Unmeasured(why));
            }
            let support = if libc::WIFEXITED(status) {
                match libc::WEXITSTATUS(status) {
                    0 => Support::Available,
                    errno => Support::from_errno(errno),
                }
            } else {
                Support::Unmeasured("probe child did not exit normally")
            };
            (pid, support)
        }
    }
}

/// Query the Landlock ABI version.
///
/// A pure query: with a null attribute and zero size, the
/// `LANDLOCK_CREATE_RULESET_VERSION` flag makes the call return the ABI
/// number without creating anything, so no fork is needed and no state
/// changes.
fn probe_landlock_abi() -> Result<u32, Support> {
    // SAFETY: the null pointer and zero size are exactly what the version
    // query requires; the kernel reads neither.
    let rc = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<libc::c_void>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if rc >= 1 {
        Ok(rc as u32)
    } else if rc == 0 {
        // Documented as impossible — ABI versions start at 1 — so treat a zero
        // as an unmeasured oddity rather than quietly reporting ABI 0.
        Err(Support::Unmeasured("landlock returned ABI 0"))
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        Err(Support::from_errno(errno))
    }
}

/// Probe seccomp *filter* mode without installing a filter.
///
/// The trick is deliberate and worth naming: `prctl(PR_SET_SECCOMP,
/// SECCOMP_MODE_FILTER, NULL)` cannot succeed, because the filter pointer is
/// null — but *which way* it fails answers the question. A kernel that
/// supports filter mode gets far enough to dereference the argument and
/// returns `EFAULT`; one that does not rejects the mode itself with `EINVAL`.
/// So the probe reads a failure as success, and nothing is installed either
/// way.
fn probe_seccomp_filter() -> Support {
    // SAFETY: a null filter pointer is the point — the call is guaranteed to
    // fail and cannot install anything.
    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            std::ptr::null::<libc::c_void>(),
        )
    };
    if rc == 0 {
        // Would mean a filter was installed on the calling process, which this
        // call cannot do. Report it rather than claiming support.
        return Support::Unmeasured("PR_SET_SECCOMP unexpectedly succeeded");
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    match errno {
        libc::EFAULT => Support::Available,
        other => Support::from_errno(other),
    }
}

/// Whether `no_new_privs` can be read, which is whether it exists.
///
/// Queried, never set: setting it is a one-way door for the calling process,
/// and the caller here is the core.
fn probe_no_new_privs() -> Support {
    // SAFETY: a pure query with no out-parameters.
    let rc = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
    if rc >= 0 {
        Support::Available
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        Support::from_errno(errno)
    }
}

/// The distro knobs that make R2.9 concrete, read raw.
///
/// Explanatory only. The namespace probes above are authoritative; these say
/// *why* a probe failed, which is the difference between an operator who can
/// fix their machine and one who files a bug.
fn read_policy_knobs() -> Vec<(&'static str, Option<String>)> {
    const KNOBS: &[(&str, &str)] = &[
        // Upstream nesting limit; 0 disables user namespaces outright.
        ("max_user_namespaces", "/proc/sys/user/max_user_namespaces"),
        // Debian's long-standing downstream switch.
        (
            "unprivileged_userns_clone",
            "/proc/sys/kernel/unprivileged_userns_clone",
        ),
        // Ubuntu 24.04+: AppArmor refuses unprivileged userns to unconfined
        // programs. The single most likely reason a modern machine fails the
        // `ns.user` probe with EPERM while every other signal looks healthy.
        (
            "apparmor_restrict_unprivileged_userns",
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
        ),
        // The knob that decides what the remedy for the one above COSTS
        // (issue #286). The supported remedy is a per-binary AppArmor profile
        // -- `packaging/apparmor/vitrind` -- and such a profile is borrowable:
        // where this is 0, any local unprivileged user can `aa-exec -p
        // vitrind` into it and obtain the user namespace it grants. Where it
        // is 1 -- which is what Ubuntu 24.04's `apparmor` package sets, via
        // /usr/lib/sysctl.d/10-apparmor.conf -- they cannot: AppArmor stacks
        // the borrowed profile with `unconfined` rather than transitioning to
        // it, so the restriction is retained. Borrowing the chrome, firefox or
        // flatpak profiles fails the same way, for the same reason.
        //
        // It explains nothing about whether a realm can spawn; it sizes what
        // shipping the profile hands over, which is a published cost and
        // therefore worth measuring rather than assuming. That is not a
        // rhetorical preference: the published version of this cost was once
        // stated with this knob's default inverted, and it was measuring it
        // here -- and recording it in CI -- that made the contradiction
        // findable.
        (
            "apparmor_restrict_unprivileged_unconfined",
            "/proc/sys/kernel/apparmor_restrict_unprivileged_unconfined",
        ),
    ];
    KNOBS
        .iter()
        .map(|(key, path)| {
            let value = fs::read_to_string(path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (*key, value)
        })
        .collect()
}

/// This process's AppArmor label, or `None` where AppArmor is not the LSM
/// answering.
///
/// # Why this reads two paths in a fixed order, and why the order matters
///
/// `/proc/self/attr/current` is **not** an AppArmor file. It is the *active
/// LSM's* file, and on a SELinux machine it returns an SELinux context. A
/// reader that took it unconditionally would render an SELinux context under
/// an `apparmor.` key on every Fedora and RHEL box — a row that is not merely
/// unhelpful but affirmatively wrong, and wrong in the direction that invents
/// an AppArmor confinement nobody has.
///
/// So the LSM-qualified path is tried first: `/proc/self/attr/apparmor/current`
/// exists only where AppArmor is stacked in, and its answer is unambiguous.
/// The unqualified path is consulted **only** as a fallback and **only** when
/// `/sys/module/apparmor/parameters/enabled` says `Y`, which is the same file
/// the refusal text and this repository's CI diagnostics already read.
///
/// Absence is reported as absence. On the machine this was written on
/// (AppArmor compiled out) every one of these reads fails and the row renders
/// `apparmor.label=absent`, which is the truthful answer and is distinguishable
/// from `apparmor.label=unconfined` — a machine that *has* AppArmor and left
/// this process unlabelled. Those are different situations with different
/// remedies, and collapsing them would repeat the mistake this module's own
/// rule 4 was written about.
fn read_apparmor_label() -> Option<String> {
    fn clean(raw: String) -> Option<String> {
        // The kernel writes a trailing newline, and older kernels a trailing
        // NUL. A label is a profile name plus an optional ` (mode)` suffix, so
        // anything else non-printable would be a kernel this code does not
        // understand; drop such a read rather than render it.
        let trimmed = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
        if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
            return None;
        }
        Some(trimmed.to_string())
    }

    if let Some(label) = fs::read_to_string("/proc/self/attr/apparmor/current")
        .ok()
        .and_then(clean)
    {
        return Some(label);
    }

    let enabled = fs::read_to_string("/sys/module/apparmor/parameters/enabled")
        .map(|s| s.trim() == "Y")
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    fs::read_to_string("/proc/self/attr/current")
        .ok()
        .and_then(clean)
}

fn kernel_release() -> String {
    // SAFETY: `uname` fills a caller-provided, fully-initialized struct.
    let mut buf: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut buf) } != 0 {
        return "unknown".to_string();
    }
    let bytes: Vec<u8> = buf
        .release
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Whether `/etc/subuid` provisions a subordinate range for this user.
///
/// Matched on both the numeric uid and the login name, because `shadow`
/// accepts either spelling and a machine provisioned one way must not read as
/// unprovisioned.
fn has_subuid_range() -> bool {
    let Ok(contents) = fs::read_to_string("/etc/subuid") else {
        return false;
    };
    let uid = unsafe { libc::getuid() };
    let uid_str = uid.to_string();
    let name = current_user_name();
    contents.lines().any(|line| {
        let mut fields = line.trim().splitn(3, ':');
        let Some(owner) = fields.next() else {
            return false;
        };
        // A range with a zero count provisions nothing; treat it as absent
        // rather than letting a placeholder line read as capability.
        let count_ok = fields
            .nth(1)
            .and_then(|c| c.trim().parse::<u64>().ok())
            .is_some_and(|c| c > 0);
        count_ok && (owner == uid_str || name.as_deref() == Some(owner))
    })
}

/// The login name for the current uid, or `None` if it cannot be resolved.
fn current_user_name() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    // `libc::c_char`, never a literal `i8`: `c_char` is *unsigned* on aarch64,
    // arm, riscv64, powerpc64 and s390x, so `Vec<i8>` here is a hard build
    // break on every one of them. CI runs x86_64 only, which is exactly why
    // this has to be right by construction rather than by test.
    let mut buf = vec![0 as libc::c_char; 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: `getpwuid_r` writes into the caller's `pwd` and `buf`; `result`
    // is set to `&pwd` on success and left null when no entry exists. The
    // reentrant form is used rather than `getpwuid` because this function has
    // no thread-safety contract of its own.
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: on success `pw_name` points into `buf`, which is still alive.
    let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    name.to_str().ok().map(|s| s.to_string())
}

/// Whether a `newuidmap`-class helper exists on `PATH`.
///
/// Presence is necessary and *not* sufficient: the helper must also be setuid
/// or hold `CAP_SETUID`, which this does not check. The per-uid tier is an
/// explicit opt-in for exactly this reason — D-020 makes it a provisioned
/// upgrade, never an inference — so an over-optimistic reading here cannot
/// silently weaken anything.
fn has_newuidmap() -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| Path::new(dir).join("newuidmap").exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_restricted_are_different_answers() {
        // Rule 4 in the module docs, as a test rather than a comment: the two
        // failing classes must never collapse, because the operator's remedy
        // differs. EINVAL is "this kernel cannot"; EPERM is "something said
        // no"; ENOSPC is an exhausted limit, which is a policy answer because
        // it is fixable without a new kernel.
        assert_eq!(
            Support::from_errno(libc::EINVAL),
            Support::Absent(libc::EINVAL)
        );
        assert_eq!(
            Support::from_errno(libc::ENOSYS),
            Support::Absent(libc::ENOSYS)
        );
        assert_eq!(
            Support::from_errno(libc::EPERM),
            Support::RestrictedByPolicy(libc::EPERM)
        );
        assert_eq!(
            Support::from_errno(libc::ENOSPC),
            Support::RestrictedByPolicy(libc::ENOSPC)
        );
    }

    #[test]
    fn only_available_counts_as_available() {
        assert!(Support::Available.is_available());
        assert!(!Support::Absent(libc::EINVAL).is_available());
        assert!(!Support::RestrictedByPolicy(libc::EPERM).is_available());
        assert!(!Support::Unmeasured("x").is_available());
    }

    #[test]
    fn rendered_support_strings_are_stable() {
        // These strings are matrix cells. A change here is a change to every
        // committed matrix, so the test exists to make that deliberate.
        assert_eq!(Support::Available.to_string(), "available");
        assert_eq!(Support::Absent(22).to_string(), "absent(errno=22)");
        assert_eq!(
            Support::RestrictedByPolicy(1).to_string(),
            "restricted-by-policy(errno=1)"
        );
        assert_eq!(
            Support::Unmeasured("fork failed").to_string(),
            "unmeasured(fork failed)"
        );
    }

    /// A report with everything available, as the base for the tier tests.
    ///
    /// Its `landlock_abi` is **this build's floor**, not a literal: the number
    /// stopped being arbitrary when the floor landed (2026-08-15), and a
    /// hard-coded 6 quietly turned every `admit(...).is_ok()` in this module
    /// into a refusal. Spelled from the constant so raising the floor moves
    /// this with it rather than reddening a dozen unrelated tests.
    fn full_report() -> Report {
        Report {
            kernel_release: "0.0.0-test".to_string(),
            namespaces: vec![NamespaceProbe {
                key: "ns.user",
                support: Support::Available,
            }],
            namespaces_combined: Support::Available,
            mount_in_userns: Support::Available,
            landlock_abi: Ok(vitrin_realm_init::LANDLOCK_MIN_ABI),
            seccomp_filter: Support::Available,
            no_new_privs: Support::Available,
            policy: vec![("max_user_namespaces", Some("1000".to_string()))],
            apparmor_label: None,
            subuid_range: false,
            newuidmap: false,
        }
    }

    #[test]
    fn tier_is_keyed_on_the_combined_call_not_the_individual_probes() {
        // The property the doc comment on `tier` claims: six passing probes do
        // not license a tier if the one call P2.6.2 actually makes failed.
        let mut report = full_report();
        report.namespaces = vec![
            NamespaceProbe {
                key: "ns.user",
                support: Support::Available,
            },
            NamespaceProbe {
                key: "ns.mount",
                support: Support::Available,
            },
        ];
        report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM);
        assert_eq!(report.tier(), Tier::None);
    }

    /// The machine P2.6.2's first CI run actually met: every namespace granted,
    /// and the first mount inside them denied.
    ///
    /// Before `mount.in_userns` existed this report was `Tier::IntraUser` and
    /// [`admit`] returned `Ok`, so `vitrind` booted and every realm spawn was
    /// then refused by the post-spawn verification instead. The preflight is
    /// the thing that is supposed to catch this, so the assertion is on the
    /// preflight.
    #[test]
    fn a_granted_namespace_whose_mount_is_denied_is_not_a_tier() {
        let mut report = full_report();
        report.mount_in_userns = Support::RestrictedByPolicy(libc::EPERM);

        assert_eq!(
            report.tier(),
            Tier::None,
            "an unshare that succeeds does not license a tier when the mount after it fails"
        );
        assert!(
            !report.mechanism(Mechanism::Namespaces).is_available(),
            "the Namespaces mechanism is both halves of the call, not the unshare alone"
        );
        let refusal = admit(Isolation::Default, LandlockRequest::Highest, &report)
            .expect_err("a machine that cannot mount must be refused, not admitted");
        assert_eq!(refusal.mechanism, Mechanism::Namespaces);
    }

    /// The failing half is reported, not the passing one. An operator handed
    /// `available` for a mechanism that just refused the spawn cannot act.
    #[test]
    fn the_mechanism_reports_whichever_half_said_no() {
        let mut report = full_report();

        report.mount_in_userns = Support::RestrictedByPolicy(libc::EACCES);
        assert_eq!(
            report.mechanism(Mechanism::Namespaces),
            Support::RestrictedByPolicy(libc::EACCES),
            "the mount's errno reaches the operator"
        );

        // When the unshare itself failed, the mount row is meaningless — it was
        // measured after a namespace that was never granted — so the unshare is
        // what gets reported.
        report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM);
        assert_eq!(
            report.mechanism(Mechanism::Namespaces),
            Support::RestrictedByPolicy(libc::EPERM),
            "a failed unshare outranks a mount row that could not mean anything"
        );
    }

    /// The remedy for a denied mount must not be three userns sysctls that all
    /// already read fine.
    #[test]
    fn a_denied_mount_gets_a_remedy_about_the_mount() {
        let mut report = full_report();
        report.mount_in_userns = Support::RestrictedByPolicy(libc::EPERM);
        // Exactly the knob set a GitHub runner presents: nothing here explains
        // the failure, which is the case the old remedy path had no answer for.
        report.policy = vec![
            ("max_user_namespaces", Some("15000".to_string())),
            (
                "apparmor_restrict_unprivileged_userns",
                Some("0".to_string()),
            ),
        ];

        let refusal =
            admit(Isolation::Default, LandlockRequest::Highest, &report).expect_err("must refuse");
        assert!(
            refusal.remedy.contains("inside"),
            "the remedy must say the mount inside the namespace is what failed: {}",
            refusal.remedy
        );
        // The pointer to the LSM context, which is the only thing left that
        // explains this state. It used to be `cat /proc/self/attr/current`,
        // and moved to the matrix row when `apparmor.label` landed (issue
        // #286): a shell's label is not necessarily this core's, and the row
        // is measured by the process that hit the denial. What the assertion
        // holds is unchanged -- the remedy must send the operator to a label
        // and not leave them with an errno.
        assert!(
            refusal.remedy.contains("apparmor.label"),
            "and must point at the LSM context, the only thing left that explains it: {}",
            refusal.remedy
        );
    }

    /// The row is measured on this machine, whatever the answer is. A probe
    /// that silently reports `Unmeasured` everywhere would make every assertion
    /// above vacuous in production while passing in this module.
    #[test]
    fn the_mount_probe_actually_runs_here() {
        let _sigchld = SigchldDefault::install();
        let support = probe_mount_in_userns();
        assert!(
            !matches!(support, Support::Unmeasured(_)),
            "the mount probe must reach a real answer on the test machine, got {support}"
        );
    }

    /// The rendered matrix carries the new row, since a matrix collected
    /// without it cannot be told apart from one where the mount passed.
    #[test]
    fn the_matrix_carries_the_mount_row() {
        let report = full_report();
        assert!(
            report.render().contains("mount.in_userns=available\n"),
            "{}",
            report.render()
        );
    }

    #[test]
    fn per_uid_needs_both_halves_of_its_provisioning() {
        let mut report = full_report();
        assert_eq!(report.tier(), Tier::IntraUser);

        report.subuid_range = true;
        assert_eq!(
            report.tier(),
            Tier::IntraUser,
            "a range with no helper is not the tier"
        );

        report.subuid_range = false;
        report.newuidmap = true;
        assert_eq!(
            report.tier(),
            Tier::IntraUser,
            "a helper with no range is not the tier"
        );

        report.subuid_range = true;
        assert_eq!(report.tier(), Tier::PerUid);
    }

    #[test]
    fn landlock_abi_zero_is_not_a_tier() {
        let mut report = full_report();
        report.landlock_abi = Err(Support::Absent(libc::ENOSYS));
        assert_eq!(report.tier(), Tier::None);
    }

    #[test]
    fn seccomp_and_nnp_are_both_required_for_the_base_tier() {
        let mut report = full_report();
        report.seccomp_filter = Support::Absent(libc::EINVAL);
        assert_eq!(report.tier(), Tier::None);

        let mut report = full_report();
        report.no_new_privs = Support::Absent(libc::EINVAL);
        assert_eq!(report.tier(), Tier::None);
    }

    #[test]
    fn the_floor_comparison_orders_the_tiers() {
        assert!(Tier::PerUid.meets(Tier::IntraUser));
        assert!(Tier::IntraUser.meets(Tier::IntraUser));
        assert!(!Tier::None.meets(Tier::IntraUser));
        assert!(Tier::None.meets(Tier::None));
    }

    #[test]
    fn render_is_deterministic_and_carries_no_per_run_content() {
        let report = full_report();
        let first = report.render();
        let second = report.render();
        assert_eq!(first, second);
        assert!(first.starts_with("vitrin-isolation 3\n"));
        assert!(first.contains("tier=intra-user\n"));
        assert!(first.contains(&format!(
            "landlock.abi={}\n",
            vitrin_realm_init::LANDLOCK_MIN_ABI
        )));
        assert!(first.contains("policy.max_user_namespaces=1000\n"));
        assert!(first.contains("provisioning.subuid=absent\n"));
    }

    #[test]
    fn an_unmeasured_landlock_renders_as_a_support_string_not_a_number() {
        let mut report = full_report();
        report.landlock_abi = Err(Support::Absent(libc::ENOSYS));
        let rendered = report.render();
        assert!(rendered.contains(&format!("landlock.abi=absent(errno={})\n", libc::ENOSYS)));
        assert!(rendered.contains("tier=none\n"));
    }

    /// `absent` and `unconfined` are different answers, and the row has to
    /// keep them different.
    ///
    /// This is the module's rule 4 applied to issue #286's row. "AppArmor is
    /// not here" and "AppArmor is here and gave this process the label
    /// `unconfined`" have opposite remedies: the first machine needs nothing,
    /// the second needs `packaging/apparmor/vitrind` loaded and attached. A
    /// row that printed one token for both would send an operator on the wrong
    /// errand from a matrix that looked complete.
    #[test]
    fn an_absent_apparmor_is_not_rendered_as_an_unconfined_one() {
        let mut report = full_report();

        report.apparmor_label = None;
        assert!(report.render().contains("apparmor.label=absent\n"));

        report.apparmor_label = Some("unconfined".to_string());
        assert!(report.render().contains("apparmor.label=unconfined\n"));

        // The value this file exists to make observable: the profile attached.
        report.apparmor_label = Some("vitrind (unconfined)".to_string());
        let rendered = report.render();
        assert!(rendered.contains("apparmor.label=vitrind (unconfined)\n"));
        // ...and it must not be confusable with the row above by a `grep`
        // that reads the whole line, which is how the CI job reads it.
        assert!(!rendered.contains("apparmor.label=unconfined\n"));
    }

    /// What this machine actually answered, asserted as a SHAPE and never as a
    /// value.
    ///
    /// The two states this repository can reach are a development box with
    /// AppArmor compiled out (`absent`) and a GitHub runner with AppArmor
    /// enforcing (`unconfined`, or `vitrind` once #286's profile is loaded).
    /// Demanding either would be asserting a machine's configuration. What
    /// *is* assertable everywhere: the row exists, is non-empty, and — the
    /// part that matters — reports `absent` rather than some other LSM's
    /// context when AppArmor is not the LSM answering.
    #[test]
    fn the_apparmor_label_row_never_reports_another_lsms_context() {
        let apparmor_enabled = fs::read_to_string("/sys/module/apparmor/parameters/enabled")
            .map(|s| s.trim() == "Y")
            .unwrap_or(false);
        let qualified = Path::new("/proc/self/attr/apparmor/current").exists();
        let label = read_apparmor_label();

        if !apparmor_enabled && !qualified {
            assert_eq!(
                label, None,
                "AppArmor is not the LSM here, so no label may be reported; \
                 /proc/self/attr/current on such a machine belongs to whatever LSM IS active"
            );
        }
        // The other direction is not assertable: a machine with AppArmor may
        // legitimately answer anything, including nothing.
        if let Some(label) = label {
            assert!(!label.is_empty());
            assert!(!label.chars().any(|c| c.is_control()));
        }
    }

    #[test]
    fn probing_this_machine_reports_every_row_and_changes_nothing() {
        // Runs the real probes. It asserts shape, never a specific answer:
        // CI kernels, developer machines and container runtimes legitimately
        // differ, and a test that demanded `available` would be asserting the
        // runner's configuration rather than this code.
        let before = std::fs::read_to_string("/proc/self/uid_map").ok();
        let report = Report::probe();
        let after = std::fs::read_to_string("/proc/self/uid_map").ok();

        // The probe must not have confined the process that ran it.
        assert_eq!(before, after, "probing changed the caller's user namespace");

        assert_eq!(report.namespaces.len(), 6);
        let rendered = report.render();
        for key in [
            "ns.user",
            "ns.mount",
            "ns.pid",
            "ns.ipc",
            "ns.uts",
            "ns.net",
            "ns.all",
            "landlock.abi",
            "seccomp.filter",
            "no_new_privs",
            "apparmor.label",
            "policy.apparmor_restrict_unprivileged_unconfined",
            "tier",
        ] {
            assert!(rendered.contains(&format!("{key}=")), "missing row {key}");
        }
        // No probe may report success and failure at once, and none may be
        // left unset: every row parses to a non-empty value.
        for line in rendered.lines().skip(1) {
            let (key, value) = line.split_once('=').expect("every row is key=value");
            assert!(!key.is_empty() && !value.is_empty(), "empty row: {line}");
        }
    }

    /// The child half of [`a_launcher_that_ignores_sigchld_still_measures`].
    ///
    /// `#[ignore]`d so a normal run never picks it up; the parent invokes it
    /// by exact name.
    #[test]
    #[ignore]
    fn probe_under_ignored_sigchld() {
        assert_eq!(
            std::env::var("VITRIN_TEST_SIGCHLD_IGNORED").as_deref(),
            Ok("1"),
            "this test is only meaningful when its parent set up the disposition"
        );
        // Confirm the premise before asserting anything about the fix: we must
        // really have inherited SIG_IGN across exec, or the test proves nothing.
        let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
        unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut current) };
        assert_eq!(
            current.sa_sigaction,
            libc::SIG_IGN,
            "SIG_IGN was not inherited across exec, so this run tests nothing"
        );

        let report = Report::probe();
        for probe in &report.namespaces {
            assert!(
                !matches!(probe.support, Support::Unmeasured(_)),
                "{} came back {} under an ignored SIGCHLD",
                probe.key,
                probe.support
            );
        }
        assert!(
            !matches!(report.namespaces_combined, Support::Unmeasured(_)),
            "ns.all came back {} under an ignored SIGCHLD",
            report.namespaces_combined
        );

        // And the guard must put back what it found rather than leaving the
        // process on SIG_DFL.
        let mut after: libc::sigaction = unsafe { std::mem::zeroed() };
        unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut after) };
        assert_eq!(
            after.sa_sigaction,
            libc::SIG_IGN,
            "the probe left SIGCHLD changed instead of restoring it"
        );
    }

    #[test]
    fn a_launcher_that_ignores_sigchld_still_measures() {
        // Under `SIGCHLD = SIG_IGN` the kernel auto-reaps, `waitpid` returns
        // ECHILD, and every namespace row would come back `unmeasured` on a
        // machine that grants all of them -- with `tier=none` following. The
        // idiom is ordinary (`trap "" CHLD`, `signal(SIGCHLD, SIG_IGN)` in
        // supervisors) and `execve` preserves SIG_IGN, so the core can be
        // launched into it.
        //
        // Run in a SUBPROCESS on purpose. Setting SIG_IGN in this process
        // would change a disposition the whole test binary shares, and other
        // tests here spawn real realms and reap them with `Child::wait` -- so
        // an in-process version of this test would flake them.
        use std::os::unix::process::CommandExt;

        let exe = std::env::current_exe().expect("test binary path");
        let mut cmd = std::process::Command::new(exe);
        cmd.args([
            "--exact",
            "spawn::isolation::tests::probe_under_ignored_sigchld",
            "--ignored",
            "--nocapture",
        ])
        .env("VITRIN_TEST_SIGCHLD_IGNORED", "1");

        // SAFETY: `signal` is async-signal-safe and is the only thing between
        // fork and exec; the disposition is set in the CHILD, so this parent's
        // own reaping (`output()` below) is untouched.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGCHLD, libc::SIG_IGN);
                Ok(())
            });
        }

        let out = cmd.output().expect("re-exec of the test binary");
        assert!(
            out.status.success(),
            "probe under ignored SIGCHLD failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    #[test]
    fn the_selector_and_the_tier_share_no_bottom_token() {
        // Collision 1, as a test rather than a comment. `Tier::None` is also
        // what `tier()` returns for `Support::Unmeasured`, so a selector
        // spelled `none` would conflate "the operator chose no confinement"
        // with "nothing was measured".
        //
        // Both sets are enumerated through an exhaustive `match` with no
        // catch-all, so a new variant on either side does not compile until
        // somebody classifies it.
        let tiers: Vec<String> = [Tier::None, Tier::IntraUser, Tier::PerUid]
            .into_iter()
            .map(|t| match t {
                Tier::None | Tier::IntraUser | Tier::PerUid => t.to_string(),
            })
            .collect();
        let selectors: Vec<String> = [Isolation::Default, Isolation::Off]
            .into_iter()
            .map(|i| match i {
                Isolation::Default | Isolation::Off => i.to_string(),
            })
            .collect();
        assert_eq!(tiers, ["none", "intra-user", "per-uid"]);
        assert_eq!(selectors, ["default", "off"]);
        assert!(
            !selectors.contains(&Tier::None.to_string()),
            "the tier's bottom token became a selector value"
        );
    }

    #[test]
    fn the_retired_none_spelling_is_refused_by_name() {
        // Everybody who read D-020(6) will type `none`. The copy has to say
        // the token moved, not that they mistyped.
        let err = Isolation::parse("none").expect_err("`none` is retired");
        assert!(err.contains("--isolation=off"), "{err}");
        assert!(err.contains("D-020(6)"), "{err}");
        // Non-vacuity for the arm above: the two live spellings parse, and an
        // unrelated word gets the generic message rather than this one.
        assert_eq!(Isolation::parse("default"), Ok(Isolation::Default));
        assert_eq!(Isolation::parse("off"), Ok(Isolation::Off));
        let other = Isolation::parse("hardened").expect_err("not a value");
        assert!(!other.contains("D-020(6)"), "{other}");
    }

    #[test]
    fn isolation_cannot_be_clamped_defaulted_or_converted() {
        // The three accidents `Isolation`'s docs name, checked against this
        // module's own source because the alternative -- a `trybuild` case --
        // would put a proc-macro-adjacent dev-dependency in the TCB for one
        // assertion. The needles are the *declarations*, so a derive added
        // later fails here even though it compiles.
        // **The shipped half only.** A source-reading test that scans its own
        // module finds its own prose: the needles below name the impls they
        // forbid, and a comment explaining why is a literal occurrence of the
        // thing being forbidden. Measured, on the first run of the
        // hand-written-impl arm added below.
        let whole = include_str!("isolation.rs");
        let source = whole
            .split("#[cfg(test)]")
            .next()
            .expect("this file has a test module");
        assert!(
            source.len() < whole.len(),
            "the test module was not split off, so every needle below can find itself"
        );
        let decl = source
            .split("pub enum Isolation {")
            .next()
            .expect("the enum is declared in this file");
        let derive = decl
            .rsplit_once("#[derive(")
            .expect("the enum carries a derive")
            .1;
        let derive = derive.split(')').next().expect("a closed derive list");
        for forbidden in ["Ord", "Default"] {
            assert!(
                !derive.contains(forbidden),
                "`Isolation` derives {forbidden}: {derive:?}. An ordering lets \
                 min(selected, measured) typecheck and an operator who asked for confinement \
                 silently gets less; a Default makes an unconfined session reachable without \
                 anybody typing the word"
            );
        }
        // Non-vacuity: the derive list is really being read, not an empty
        // string that trivially contains nothing.
        assert!(derive.contains("Copy"), "read the wrong derive: {derive:?}");
        // Assembled from fragments rather than written out, because a
        // source-reading test whose needle is a literal in its own body finds
        // itself and fails on the first run.
        for (from, to) in [("Tier", "Isolation"), ("Isolation", "Tier")] {
            let bridge = format!("impl From<{from}> for {to}");
            assert!(
                !source.contains(&bridge),
                "`{bridge}` exists; a measurement must not be launderable into a selection"
            );
        }
        // Reading the derive list is not enough, and an adversarial review
        // said so: a hand-written `impl Default for Isolation` or `impl
        // PartialOrd for Isolation` compiles, does exactly the damage the
        // derive ban exists to prevent, and appears nowhere in the derive
        // list. Same fragment assembly, same reason.
        for trait_name in ["Default", "PartialOrd", "Ord"] {
            let hand_written = format!("impl {trait_name} for Isolation");
            assert!(
                !source.contains(&hand_written),
                "`{hand_written}` exists. Written out by hand it does precisely what the \
                 missing derive would: an ordering lets min(selected, measured) typecheck, and \
                 a Default makes an unconfined session reachable without anybody typing the word"
            );
        }
        // And the one legal relation is present, so a rename cannot make the
        // absences above true by deleting the whole mechanism.
        assert!(source.contains("pub fn admit(\n    requested: Isolation,"));
    }

    #[test]
    fn the_floor_is_a_subset_of_the_intra_user_predicate() {
        // Clause 6's schedule, as a checked fact. `FLOOR` grows one entry per
        // task (#186 namespaces, #187 landlock, #188 seccomp + nnp); at #188
        // it coincides with `tier()`'s base predicate and the assertion below
        // flips from subset to equality. Two of the four are in it now, so
        // the subset is still proper -- and the two that are missing are
        // named below rather than left to be inferred from a length.
        let base = [
            Mechanism::Namespaces,
            Mechanism::Landlock,
            Mechanism::Seccomp,
            Mechanism::NoNewPrivs,
        ];
        for mechanism in FLOOR {
            assert!(
                base.contains(mechanism),
                "{mechanism} is in FLOOR but is not part of the intra-user predicate"
            );
        }
        assert!(
            FLOOR.len() <= base.len(),
            "FLOOR has outgrown the tier predicate it must stay inside"
        );
        // The two entries #187 leaves for #188, named. A `len()` comparison
        // alone would pass for a `FLOOR` that had picked up the wrong two.
        assert!(FLOOR.contains(&Mechanism::Namespaces), "#186's entry");
        assert!(FLOOR.contains(&Mechanism::Landlock), "#187's entry");
        assert!(
            !FLOOR.contains(&Mechanism::Seccomp) && !FLOOR.contains(&Mechanism::NoNewPrivs),
            "seccomp and no-new-privs are #188's move: the filter has to arrive with the gate \
             that requires it, or whole sessions are refused for a mechanism nothing applies"
        );

        // A truth table over synthetic reports, one row per mechanism absent:
        // a machine missing a mechanism the tier needs is never `IntraUser`,
        // whether or not that mechanism is in this build's floor.
        for mechanism in base {
            let mut report = full_report();
            match mechanism {
                Mechanism::Namespaces => {
                    report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM)
                }
                Mechanism::Landlock => report.landlock_abi = Err(Support::Absent(libc::ENOSYS)),
                Mechanism::Seccomp => report.seccomp_filter = Support::Absent(libc::EINVAL),
                Mechanism::NoNewPrivs => report.no_new_privs = Support::Absent(libc::EINVAL),
            }
            assert_eq!(
                report.tier(),
                Tier::None,
                "a machine missing {mechanism} still reported a tier"
            );
            assert!(!report.mechanism(mechanism).is_available());
        }
    }

    #[test]
    fn the_floor_is_a_subset_of_what_the_build_applies() {
        // The relation `--print-floor` renders two different row families
        // from, so a build cannot describe itself wrongly in either
        // direction. A gate for something nothing applies would refuse
        // sessions for nothing; a mechanism applied but printed as `not-yet`
        // is a false published row, which is what an adversarial review found
        // when `applies.*` was rendered off `FLOOR`.
        for mechanism in FLOOR {
            assert!(
                APPLIED.contains(mechanism),
                "{mechanism} is a startup gate but nothing in this build applies it"
            );
        }
        // Non-vacuous in the other direction too: the two lists are not
        // required to be equal today and this states which entry differs, so
        // #188 moving `no-new-privs` into `FLOOR` has to come here and say so.
        assert!(
            APPLIED.contains(&Mechanism::NoNewPrivs),
            "the helper sets PR_SET_NO_NEW_PRIVS on every confined spawn"
        );
        assert!(
            !FLOOR.contains(&Mechanism::NoNewPrivs),
            "no-new-privs became a startup gate: that is #188's move, and it needs the seccomp \
             filter it protects to arrive with it"
        );
        // #187's entry is in BOTH, which is what makes it a floor rather than
        // a behaviour: the helper enforces a ruleset before every shim's
        // execve, and a kernel that cannot supply one refuses the session.
        assert!(
            APPLIED.contains(&Mechanism::Landlock) && FLOOR.contains(&Mechanism::Landlock),
            "Landlock is applied by the helper and gated at startup; a build with one and not \
             the other either refuses for nothing or applies something it does not require"
        );
        // And the one this build still does not apply, so `--print-floor`
        // cannot print `applies.seccomp=yes` before #188 writes the filter.
        assert!(
            !APPLIED.contains(&Mechanism::Seccomp),
            "nothing in this build installs a seccomp filter"
        );
    }

    #[test]
    fn every_floor_mechanism_refuses_when_its_probe_fails() {
        // Both directions, and the second half is what makes the first
        // non-vacuous: a `FLOOR` that refused on everything would pass a
        // one-directional check.
        let all = [
            Mechanism::Namespaces,
            Mechanism::Landlock,
            Mechanism::Seccomp,
            Mechanism::NoNewPrivs,
        ];
        for mechanism in all {
            let mut report = full_report();
            match mechanism {
                Mechanism::Namespaces => {
                    report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM)
                }
                Mechanism::Landlock => report.landlock_abi = Err(Support::Absent(libc::ENOSYS)),
                Mechanism::Seccomp => report.seccomp_filter = Support::Absent(libc::EINVAL),
                Mechanism::NoNewPrivs => report.no_new_privs = Support::Absent(libc::EINVAL),
            }
            let outcome = admit(Isolation::Default, LandlockRequest::Highest, &report);
            if FLOOR.contains(&mechanism) {
                let refusal = outcome.expect_err("a floor mechanism failed and the spawn started");
                assert_eq!(refusal.mechanism, mechanism);
            } else {
                assert!(
                    outcome.is_ok(),
                    "{mechanism} is not in FLOOR, so this build must start without it -- \
                     refusing here would buy a failure mode with no matching safety"
                );
            }
            // `off` asks for nothing, so nothing can be missing.
            assert!(admit(Isolation::Off, LandlockRequest::Highest, &report).is_ok());
            // `--landlock=off` switches ONE floor mechanism off by name, so a
            // machine missing only Landlock starts under it -- and a machine
            // missing anything else still refuses. Without the second half
            // this would pass for a `--landlock=off` that skipped the whole
            // floor check.
            let with_landlock_off = admit(Isolation::Default, LandlockRequest::Off, &report);
            if mechanism == Mechanism::Landlock {
                assert!(
                    with_landlock_off.is_ok(),
                    "`--landlock=off` names the one mechanism it turns off; a machine missing \
                     only that one must start, or the flag is unusable on the machines it is \
                     for"
                );
                assert_eq!(
                    with_landlock_off.expect("admitted").landlock(),
                    LandlockRequest::Off
                );
                // And the realms such a session spawns report rung 0, which
                // is the one profile that says no ruleset was applied.
                assert_eq!(
                    profile_for(Isolation::Default, 0),
                    "namespaces-only",
                    "the session may not journal a Landlock it does not apply"
                );
            } else if FLOOR.contains(&mechanism) {
                assert!(
                    with_landlock_off.is_err(),
                    "`--landlock=off` waived {mechanism}, which it does not name -- one flag \
                     turning off a mechanism it was not asked about is a silent degradation"
                );
            }
        }
    }

    #[test]
    fn an_unmeasured_floor_mechanism_refuses_with_its_own_copy() {
        // M3: treating "unknown" as "fine" is the silent degradation D-020(6)
        // forbids, and an operator told "restricted" would go looking for a
        // sysctl that is not the problem.
        let mut report = full_report();
        report.namespaces_combined = Support::Unmeasured("fork failed");
        let refusal = admit(Isolation::Default, LandlockRequest::Highest, &report)
            .expect_err("unmeasured must refuse");
        assert!(refusal.remedy.contains("could not be run"), "{refusal}");
        assert!(refusal.remedy.contains("fork failed"), "{refusal}");
        // Non-vacuity: the restricted case gets *different* copy.
        report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM);
        let other = admit(Isolation::Default, LandlockRequest::Highest, &report)
            .expect_err("restricted must refuse");
        assert!(!other.remedy.contains("could not be run"), "{other}");
    }

    #[test]
    fn the_remedy_names_only_knobs_that_were_actually_read() {
        // A fabricated remedy is worse than silence: an operator who follows
        // one and sees no change concludes the diagnosis is broken rather
        // than that the cause is elsewhere.
        let mut report = full_report();
        report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM);
        report.policy = vec![(
            "apparmor_restrict_unprivileged_userns",
            Some("1".to_string()),
        )];
        let refusal =
            admit(Isolation::Default, LandlockRequest::Highest, &report).expect_err("refuses");
        assert!(refusal.remedy.contains("apparmor_restrict"), "{refusal}");
        assert!(
            !refusal.remedy.contains("unprivileged_userns_clone"),
            "a knob that was not read was named anyway: {refusal}"
        );

        // And when nothing explains it, the copy says so and invents nothing.
        report.policy = vec![("max_user_namespaces", Some("15000".to_string()))];
        let silent =
            admit(Isolation::Default, LandlockRequest::Highest, &report).expect_err("refuses");
        assert!(silent.remedy.contains("no remedy is offered"), "{silent}");
    }

    #[test]
    fn applied_profile_never_claims_a_tier_this_build_does_not_apply() {
        // Clause 10: `Tier::IntraUser` is *defined* as namespaces plus
        // Landlock plus seccomp. #187 applies two thirds, so the profile
        // names the two and is still not the tier.
        let full = profile_for(
            Isolation::Default,
            vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG,
        );
        assert_eq!(full, "namespaces+landlock-abi9");
        assert_ne!(full, Tier::IntraUser.to_string());
        assert!(
            !full.contains("seccomp"),
            "the filter is #188's; a profile naming it would be this build describing itself \
             wrongly in the dangerous direction"
        );
        assert_eq!(profile_for(Isolation::Off, 0), "none");
        assert_eq!(
            profile_for(Isolation::Off, 9),
            "none",
            "at --isolation=off no helper runs, so no rung can qualify the profile"
        );
    }

    #[test]
    fn the_profile_names_the_rung_obtained_and_not_the_one_requested() {
        // **The defect this pins.** `applied_profile` was derived from the
        // session's `--landlock` flag, so `--landlock=abi:9` on an ABI-3
        // kernel journaled `namespaces+landlock-abi9`, and `--landlock=
        // highest` journaled one string whether the ladder settled on rung 9
        // or fell to rung 1. The field is named `applied`, and it now takes
        // the number the realm's PID 1 reported.
        //
        // The signature is the assertion: `profile_for` takes a rung, so
        // there is no request to pass it by mistake.
        let mut seen = std::collections::HashSet::new();
        for rung in 0..=vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG {
            let profile = profile_for(Isolation::Default, rung);
            assert!(
                seen.insert(profile),
                "rung {rung} renders exactly like a rung already seen: {profile}. A ladder \
                 fallback from 9 to 1 would then be invisible in the field named for what was \
                 applied"
            );
            if rung == 0 {
                assert_eq!(
                    profile, "namespaces-only",
                    "rung 0 is `--landlock=off`, and it may not read as a Landlocked session"
                );
            } else {
                assert!(
                    profile.ends_with(&format!("abi{rung}")),
                    "the rung must be IN the profile string, because that string is what a \
                     human greps a journal for: {profile}"
                );
            }
        }
        // A rung above the build's ladder cannot come from this build's
        // helper, and the profile refuses to name one rather than guessing.
        let impossible = profile_for(
            Isolation::Default,
            vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG + 1,
        );
        assert_eq!(impossible, "namespaces+landlock-unknown-rung");
        assert!(!impossible.contains("abi10"));
    }

    #[test]
    fn a_landlock_refusal_names_the_three_things_that_cause_it() {
        // The remedy path had one real arm (namespaces) and a generic "no
        // knob explains this" for everything else. A missing Landlock has
        // exactly three causes and an operator can check all three, so
        // offering silence here would have been the module's own rule
        // (a fabricated remedy is worse than none) applied where a real one
        // exists.
        let mut report = full_report();
        report.landlock_abi = Err(Support::Absent(libc::ENOSYS));
        let refusal = admit(Isolation::Default, LandlockRequest::Highest, &report)
            .expect_err("a kernel with no Landlock must refuse since #187");
        assert_eq!(refusal.mechanism, Mechanism::Landlock);
        for needle in ["5.13", "CONFIG_SECURITY_LANDLOCK", "lsm=", "--landlock=off"] {
            assert!(
                refusal.remedy.contains(needle),
                "the Landlock remedy must name {needle}: {}",
                refusal.remedy
            );
        }
        // It names the kernel it was measured against, so an operator reading
        // a log does not have to correlate two lines to know which machine
        // answered.
        assert!(
            refusal.remedy.contains(&report.kernel_release),
            "the remedy must quote the release it measured: {}",
            refusal.remedy
        );
        // **Non-vacuity, and it has to call `remedy_for` to be one.** The
        // comment here once claimed this control while the assertion below it
        // only called `admit` -- which, for a mechanism outside `FLOOR`,
        // returns `Ok` without ever reaching `remedy_for`. That asserted
        // nothing about the copy at all. So the generic arm is exercised
        // directly: `remedy_for` must still produce the "no knob explains
        // this" paragraph for an un-remedied mechanism, and must NOT leak any
        // of the Landlock needles into it. Were the Landlock branch ever to
        // swallow the whole function, the loop above would keep passing and
        // this is what would go red.
        let generic = remedy_for(
            Mechanism::Seccomp,
            Support::Absent(libc::EINVAL),
            &full_report(),
        );
        assert!(
            generic.contains("No knob in this build's list"),
            "an un-remedied mechanism must get the generic copy: {generic}"
        );
        for needle in ["5.13", "CONFIG_SECURITY_LANDLOCK", "lsm="] {
            assert!(
                !generic.contains(needle),
                "the Landlock remedy is one branch, not the whole function; {needle} must not \
                 appear in another mechanism's copy: {generic}"
            );
        }
        // And the separate fact the assertion below actually holds, stated as
        // what it is: seccomp is not in `FLOOR`, so its absence does not
        // refuse a session. This is about `admit`'s membership test, not
        // about the remedy text.
        let mut seccomp_gone = full_report();
        seccomp_gone.seccomp_filter = Support::Absent(libc::EINVAL);
        assert!(
            admit(Isolation::Default, LandlockRequest::Highest, &seccomp_gone).is_ok(),
            "seccomp is not in FLOOR yet, so its absence may not refuse a session"
        );
    }

    /// The **ABI floor** (owner's decision, 2026-08-15): a kernel that has
    /// Landlock, below the number this build declares, is refused -- and the
    /// refusal says which number it found, which one it needed, and that no
    /// knob will change either.
    ///
    /// Three arms, and the middle one is what stops this being vacuous: at the
    /// floor exactly, `admit` returns `Ok`. Without it every assertion here
    /// would still pass for a build that refused every kernel.
    #[test]
    fn a_kernel_below_the_abi_floor_is_refused_and_the_remedy_names_the_number() {
        let floor = vitrin_realm_init::LANDLOCK_MIN_ABI;
        assert!(
            floor >= 1,
            "a floor of 0 is not a rung: Landlock ABI versions start at 1"
        );

        // Below the floor, with Landlock present and working.
        let mut low = full_report();
        low.landlock_abi = Ok(floor - 1);
        let refusal = admit(Isolation::Default, LandlockRequest::Highest, &low)
            .expect_err("a kernel below this build's Landlock floor must not start a session");
        assert_eq!(refusal.mechanism, Mechanism::Landlock);
        assert_eq!(
            refusal.support,
            Support::BelowFloor {
                found: floor - 1,
                required: floor,
            },
            "a kernel that HAS Landlock, one rung too low, is neither `absent` nor \
             `restricted-by-policy`: nothing is misconfigured and no sysctl moves it"
        );
        // The rendering is what an operator reads, and it must carry both
        // numbers -- a refusal naming only what it wanted cannot be acted on.
        let rendered = refusal.to_string();
        for needle in [
            format!("abi={}", floor - 1),
            format!("required={floor}"),
            "below-floor".to_string(),
        ] {
            assert!(
                rendered.contains(&needle),
                "the refusal must carry {needle}: {rendered}"
            );
        }
        // And the remedy must be the newer-kernel one, NOT the three-causes
        // paragraph: `lsm=` and `CONFIG_SECURITY_LANDLOCK` are already correct
        // on this machine, and sending an operator to check them is sending
        // them to verify the one thing that is not broken.
        assert!(
            refusal.remedy.contains("the remedy is a newer kernel"),
            "{}",
            refusal.remedy
        );
        for needle in ["CONFIG_SECURITY_LANDLOCK", "lsm="] {
            assert!(
                !refusal.remedy.contains(needle),
                "a below-floor kernel must not be handed {needle}, which is already correct \
                 there: {}",
                refusal.remedy
            );
        }

        // At the floor exactly: admitted. The non-vacuity control.
        let mut at = full_report();
        at.landlock_abi = Ok(floor);
        assert!(
            admit(Isolation::Default, LandlockRequest::Highest, &at).is_ok(),
            "the floor is a floor, not a strict inequality: a kernel reporting exactly the \
             declared number must start"
        );

        // And `--landlock=off` still waives it, on the same rule that waives
        // every floor mechanism the operator switched off by name.
        assert!(
            admit(Isolation::Default, LandlockRequest::Off, &low).is_ok(),
            "`--landlock=off` asks for no ruleset, so a kernel that cannot supply one is not a \
             reason to refuse the session"
        );
    }

    #[test]
    fn the_floor_is_published_by_its_own_verb_and_not_among_kernel_facts() {
        // Putting a build constant in `--print-isolation` would invalidate
        // #185's four-kernel matrix over a row that is not a kernel fact.
        let floor = render_floor();
        assert!(floor.starts_with("vitrin-floor 1\n"));
        assert!(floor.contains("floor.mechanism=namespaces\n"));
        // #187's move, in both row families: Landlock is a gate AND a
        // behaviour. It printed `not-yet` for the whole of #186, and the day
        // it stopped being true is the day this line had to change.
        assert!(floor.contains("floor.mechanism=landlock\n"), "{floor}");
        assert!(floor.contains("applies.landlock=yes\n"), "{floor}");
        // The one still owed, so the verb cannot start claiming #188 early.
        assert!(floor.contains("applies.seccomp=not-yet\n"), "{floor}");
        assert!(!floor.contains("floor.mechanism=seccomp\n"), "{floor}");
        // #187's build constant. It is the number a kernel newer than this
        // build gets clamped down to, and until it was printed the clamp was
        // computed for every realm and read by nobody -- so an operator on an
        // ABI-10 kernel had no way to see, in advance, that their realms
        // would be confined one rung below what their kernel offers.
        assert!(
            floor.contains(&format!(
                "build.landlock_max_rung={}\n",
                vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG
            )),
            "{floor}"
        );
        // The floor's own number (owner's decision, 2026-08-15). Without this
        // row an operator whose kernel has a working Landlock, one rung too
        // low, has no way to find out in advance which number this build
        // wanted -- only a refusal after the fact.
        assert!(
            floor.contains(&format!(
                "build.landlock_min_abi={}\n",
                vitrin_realm_init::LANDLOCK_MIN_ABI
            )),
            "{floor}"
        );
        // The gate rows and the behaviour rows are two families, and the
        // difference is visible: `no-new-privs` is applied to every confined
        // realm and is not a startup gate until #188. It printed `not-yet`
        // while the helper was setting it, which is a false row about this
        // build even though it errs the safe way.
        assert!(floor.contains("applies.no-new-privs=yes\n"), "{floor}");
        assert!(!floor.contains("floor.mechanism=no-new-privs\n"), "{floor}");
        let isolation = full_report().render();
        assert!(!isolation.contains("floor"), "{isolation}");
        assert!(!isolation.contains("applies."), "{isolation}");
    }

    #[test]
    fn the_forecast_warns_only_where_a_later_build_will_refuse() {
        // A machine that already reaches the tier has nothing to be warned
        // about; one that meets today's floor and misses a scheduled one is
        // exactly who the announcement is for.
        let mut report = full_report();
        assert_eq!(forecast(&report), None, "a full machine needs no forecast");

        // The forecast is now #188's, because #187 promoted Landlock out of
        // it: a machine with no Landlock is refused outright by `admit`, so
        // warning about it would be describing a session that cannot exist.
        // This is what "every floor move is announced by the build that still
        // works on the machines it will break" looks like on the far side of
        // a move -- the announcement retires when the refusal lands.
        report.landlock_abi = Err(Support::Absent(libc::ENOSYS));
        assert!(
            admit(Isolation::Default, LandlockRequest::Highest, &report).is_err(),
            "a machine with no Landlock is refused since #187, not warned"
        );
        assert!(
            forecast(&report).is_none_or(|w| !w.contains("landlock=")),
            "a mechanism in FLOOR must not also be forecast; it is already a hard refusal"
        );

        let mut report = full_report();
        report.seccomp_filter = Support::Absent(libc::EINVAL);
        let warning = forecast(&report).expect("a machine that #188 will break");
        assert!(warning.contains("seccomp"), "{warning}");
        assert!(warning.contains("REFUSE"), "{warning}");
        // The mechanisms this build *does* gate are not in the forecast: they
        // are already hard refusals, not warnings.
        assert!(!warning.contains("namespaces="), "{warning}");
        assert!(!warning.contains("landlock="), "{warning}");
    }

    #[test]
    fn a_forked_probe_reaps_its_own_child() {
        // A zombie per probe would be a slow leak in a long-running core.
        // Asserted against the probe's OWN pid, never against `waitpid(-1)`:
        // other tests in this binary spawn real realms concurrently, so the
        // process-wide child set answers a different question than this one.
        let (pid, support) = probe_unshare_reporting_pid(libc::CLONE_NEWUSER);
        assert!(pid > 0, "probe did not fork at all: {support}");

        let mut status: libc::c_int = 0;
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        assert_eq!(
            rc, -1,
            "child {pid} was still waitable, so the probe leaked it"
        );
        assert_eq!(
            errno,
            libc::ECHILD,
            "child {pid} was not reaped by the probe"
        );
    }
}
