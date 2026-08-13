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
//!   {Namespaces}`, `#187 +{Landlock}`, `#188 +{Seccomp, NoNewPrivs}`. After
//!   #188 it coincides exactly with [`Report::tier`]'s base predicate, and
//!   [`the_floor_is_a_subset_of_the_intra_user_predicate`] asserts
//!   subset-until-then so the day they coincide is a checked fact rather than
//!   a coincidence somebody notices later.
//!
//! Gating `--isolation=default` on `Tier::meets(Tier::IntraUser)` was
//! available and is refused: it would make `vitrind` refuse to start on a
//! pre-5.13 kernel while applying no Landlock at all — a new failure mode on
//! machines where today's build runs fine, which is verbatim what this module
//! declined to buy at #185.
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
/// answered, a `Mechanism` is something `vitrind` does. Every member of
/// [`FLOOR`] must have an applier, and every applier must be in `FLOOR` --
/// [`every_floor_mechanism_refuses_when_its_probe_fails`] asserts both
/// directions, because a one-directional check would pass for a `FLOOR` that
/// refused on everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// The six-flag `unshare` `vitrin-realm-init` issues (P2.6.2, #186).
    Namespaces,
    /// The shim-side Landlock stack (P2.6.3, #187) -- **not applied yet**.
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
/// One entry today. It grows with the tasks that add the appliers, never
/// ahead of them: a floor naming a mechanism nothing applies would refuse
/// sessions in exchange for nothing, which is exactly the trade this module
/// declined at #185.
pub const FLOOR: &[Mechanism] = &[Mechanism::Namespaces];

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

/// The selection [`admit`] let through, together with the *honest* name for
/// what a realm spawned under it actually gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied(Isolation);

impl Applied {
    pub fn isolation(self) -> Isolation {
        self.0
    }

    /// The value the flight recorder's `applied_profile` field carries.
    ///
    /// **It may not overclaim, and that is why it is not the tier's
    /// spelling.** `Tier::IntraUser` is *defined* as namespaces plus Landlock
    /// plus seccomp; #186 applies the namespace third. An entry printing
    /// `intra-user` would assert confinement that does not exist, so at the
    /// end of #186 the only legal value for a confined realm is
    /// `namespaces-only`.
    pub fn profile(self) -> &'static str {
        match self.0 {
            Isolation::Default => "namespaces-only",
            Isolation::Off => "none",
        }
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
pub fn admit(requested: Isolation, report: &Report) -> Result<Applied, Refusal> {
    if requested == Isolation::Off {
        return Ok(Applied(requested));
    }
    for mechanism in FLOOR {
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
    Ok(Applied(requested))
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
                 userns creation."
                    .to_string(),
            ),
            _ => {}
        }
    }
    if lines.is_empty() {
        format!(
            "None of the three knobs this core reads \
             (user.max_user_namespaces, kernel.unprivileged_userns_clone, \
             kernel.apparmor_restrict_unprivileged_userns) explains it, so no remedy is \
             offered rather than one being guessed at. The kernel answered `{support}`; \
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
    for mechanism in [
        Mechanism::Namespaces,
        Mechanism::Landlock,
        Mechanism::Seccomp,
        Mechanism::NoNewPrivs,
    ] {
        let applied = FLOOR.contains(&mechanism);
        out.push_str(&format!(
            "applies.{mechanism}={}\n",
            if applied { "yes" } else { "not-yet" }
        ));
    }
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
    /// The full set in one call — the request P2.6.2 actually issues.
    pub namespaces_combined: Support,
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
            landlock_abi: probe_landlock_abi(),
            seccomp_filter: probe_seccomp_filter(),
            no_new_privs: probe_no_new_privs(),
            policy: read_policy_knobs(),
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
            Mechanism::Namespaces => self.namespaces_combined,
            Mechanism::Landlock => match self.landlock_abi {
                Ok(abi) if abi >= 1 => Support::Available,
                Ok(_) => Support::Unmeasured("landlock returned ABI 0"),
                Err(support) => support,
            },
            Mechanism::Seccomp => self.seccomp_filter,
            Mechanism::NoNewPrivs => self.no_new_privs,
        }
    }

    /// The measured ceiling.
    ///
    /// Keyed on [`Report::namespaces_combined`], not on the individual rows:
    /// six probes that each pass separately do not prove the one call P2.6.2
    /// makes will pass, and it is that call whose success the tier claims.
    pub fn tier(&self) -> Tier {
        let base = self.namespaces_combined.is_available()
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
        // recognizable as stale rather than merely different.
        out.push_str("vitrin-isolation 1\n");
        out.push_str(&format!("kernel.release={}\n", self.kernel_release));
        for probe in &self.namespaces {
            out.push_str(&format!("{}={}\n", probe.key, probe.support));
        }
        out.push_str(&format!("ns.all={}\n", self.namespaces_combined));
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
    // SAFETY: `fork` in a multi-threaded process is safe as long as the child
    // reaches `_exit` through async-signal-safe calls only. The child below
    // calls exactly three things: `unshare`, `__errno_location` (a
    // thread-local address, read directly rather than through
    // `io::Error::last_os_error` so that no `std` machinery sits between the
    // fork and the exit at all), and `_exit`. No allocation, no locks, no
    // Rust destructor in scope.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => (-1, Support::Unmeasured("fork failed")),
        0 => {
            // Child. Nothing here may allocate or unwind.
            let rc = unsafe { libc::unshare(flags) };
            let code = if rc == 0 {
                0
            } else {
                let errno = unsafe { *libc::__errno_location() };
                // Errnos are small positive integers; clamp defensively rather
                // than truncating silently into a different errno.
                errno.clamp(1, 255)
            };
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
    fn full_report() -> Report {
        Report {
            kernel_release: "0.0.0-test".to_string(),
            namespaces: vec![NamespaceProbe {
                key: "ns.user",
                support: Support::Available,
            }],
            namespaces_combined: Support::Available,
            landlock_abi: Ok(6),
            seccomp_filter: Support::Available,
            no_new_privs: Support::Available,
            policy: vec![("max_user_namespaces", Some("1000".to_string()))],
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
        assert!(first.starts_with("vitrin-isolation 1\n"));
        assert!(first.contains("tier=intra-user\n"));
        assert!(first.contains("landlock.abi=6\n"));
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
        let source = include_str!("isolation.rs");
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
        // And the one legal relation is present, so a rename cannot make the
        // absences above true by deleting the whole mechanism.
        assert!(source.contains("pub fn admit(requested: Isolation, report: &Report)"));
    }

    #[test]
    fn the_floor_is_a_subset_of_the_intra_user_predicate() {
        // Clause 6's schedule, as a checked fact. `FLOOR` grows one entry per
        // task (#186 namespaces, #187 landlock, #188 seccomp + nnp); at #188
        // it coincides with `tier()`'s base predicate and the assertion below
        // flips from subset to equality.
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
            let outcome = admit(Isolation::Default, &report);
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
            assert!(admit(Isolation::Off, &report).is_ok());
        }
    }

    #[test]
    fn an_unmeasured_floor_mechanism_refuses_with_its_own_copy() {
        // M3: treating "unknown" as "fine" is the silent degradation D-020(6)
        // forbids, and an operator told "restricted" would go looking for a
        // sysctl that is not the problem.
        let mut report = full_report();
        report.namespaces_combined = Support::Unmeasured("fork failed");
        let refusal = admit(Isolation::Default, &report).expect_err("unmeasured must refuse");
        assert!(refusal.remedy.contains("could not be run"), "{refusal}");
        assert!(refusal.remedy.contains("fork failed"), "{refusal}");
        // Non-vacuity: the restricted case gets *different* copy.
        report.namespaces_combined = Support::RestrictedByPolicy(libc::EPERM);
        let other = admit(Isolation::Default, &report).expect_err("restricted must refuse");
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
        let refusal = admit(Isolation::Default, &report).expect_err("refuses");
        assert!(refusal.remedy.contains("apparmor_restrict"), "{refusal}");
        assert!(
            !refusal.remedy.contains("unprivileged_userns_clone"),
            "a knob that was not read was named anyway: {refusal}"
        );

        // And when nothing explains it, the copy says so and invents nothing.
        report.policy = vec![("max_user_namespaces", Some("15000".to_string()))];
        let silent = admit(Isolation::Default, &report).expect_err("refuses");
        assert!(silent.remedy.contains("no remedy is offered"), "{silent}");
    }

    #[test]
    fn applied_profile_never_claims_a_tier_this_build_does_not_apply() {
        // Clause 10: `Tier::IntraUser` is *defined* as namespaces plus
        // Landlock plus seccomp, and #186 applies the namespace third.
        let report = full_report();
        let applied = admit(Isolation::Default, &report).expect("a full machine admits");
        assert_eq!(applied.profile(), "namespaces-only");
        assert_ne!(applied.profile(), Tier::IntraUser.to_string());
        assert_eq!(
            admit(Isolation::Off, &report).expect("off always admits").profile(),
            "none"
        );
    }

    #[test]
    fn the_floor_is_published_by_its_own_verb_and_not_among_kernel_facts() {
        // Putting a build constant in `--print-isolation` would invalidate
        // #185's four-kernel matrix over a row that is not a kernel fact.
        let floor = render_floor();
        assert!(floor.starts_with("vitrin-floor 1\n"));
        assert!(floor.contains("floor.mechanism=namespaces\n"));
        assert!(floor.contains("applies.landlock=not-yet\n"));
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

        report.landlock_abi = Err(Support::Absent(libc::ENOSYS));
        let warning = forecast(&report).expect("a machine that #187 will break");
        assert!(warning.contains("landlock"), "{warning}");
        assert!(warning.contains("REFUSE"), "{warning}");
        // The mechanism this build *does* apply is not in the forecast: it is
        // already a hard refusal, not a warning.
        assert!(!warning.contains("namespaces="), "{warning}");
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
