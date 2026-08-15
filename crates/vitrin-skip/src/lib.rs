// SPDX-License-Identifier: Apache-2.0
//! The sanctioned test skip (issue #288).
//!
//! # The failure this exists against
//!
//! `cargo test` **captures stdout and stderr for tests that pass** and
//! prints them only for tests that fail. Every runtime skip in this
//! repository used to announce itself with `eprintln!`, so a suite that
//! skipped its entire confinement half printed `993 passed` and nothing
//! else. That is not a hypothetical: it was true of this repository's
//! `rust` job from the merge of #186 until #287, and it surfaced only
//! because a tenth test forgot its guard and therefore failed loudly
//! instead of skipping quietly.
//!
//! Two prior issues in this repository are the same shape -- #229 (a
//! harness exiting 0 while silently skipping the whole real-app ladder)
//! and #186 (nine gates whose mock-freeness check would have passed for
//! the mock). The class is *a green count standing in for absent
//! evidence*.
//!
//! # The mechanism: a capability answer cannot be observed silently
//!
//! The first version of this crate was a pair of macros plus a source
//! scan, and a scan is a rule about how code is *written*. Three
//! idiomatic ways of writing the same silent skip evade any such rule --
//! inverting the guard and wrapping the whole body, moving the guard into
//! the closure that *is* the measurement, or hoisting it into a helper --
//! and two of the three were demonstrated end to end against that first
//! version.
//!
//! So the rule is now about **types**, not about text. A capability probe
//! does not return an answer: it returns a [`Verdict`], which is opaque.
//! It has no accessor, no `Debug`, no `PartialEq`, no `Deref`, no public
//! field and no public variant, so there is nothing to test, match,
//! print, compare or destructure. The one operation the public API offers
//! on a `Verdict` is [`decide`] -- which **prints the marker line and
//! applies the require-variable before it answers**.
//!
//! That gives a property a scan cannot: *you cannot learn whether this
//! machine is capable without the census learning it too*. Branching on
//! the answer yourself is still possible, and it is still loud -- the
//! marker was already printed and the require-check already panicked --
//! so the class of failure that remains is "a skip nobody justified",
//! which the census fails on, rather than "a skip nobody saw".
//!
//! # What this does NOT close -- read this before citing it
//!
//! This section is the point of the change, not a caveat appended to it. A
//! mechanism whose subject is "a green claim must not exceed the evidence"
//! may not overclaim about itself, so here is the exact bound.
//!
//! **Closed by the type system** (these stop compiling; each is asserted by a
//! `compile_fail` doctest -- four on [`Verdict`], one on [`Measured`] -- with
//! a **passing twin beside it**, and each pair is pinned by
//! `each_sealed_operation_fails_with_the_error_code_that_names_the_seal`
//! below, which compiles the twin, then the twin plus the sealed line, and
//! requires the exact error code AND the compiler's own wording for the
//! refusal. All three parts are load-bearing: rustdoc's `,E0599` annotation
//! is nightly-only and checks nothing on this repository's pinned stable
//! toolchain, and a code alone cannot tell an intact seal from a typo in the
//! fixture -- `Verdict::incapabel(..)` is `E0599` too):
//!
//! * branching on a sanctioned probe's answer at all -- `if probe().is_none()
//!   { ... }`, `let Some(x) = probe() else { return }`, `probe() == ...`,
//!   `format!("{:?}", probe())`, destructuring it. There is no accessor.
//! * a bare `return;` inside a closure a measurement helper runs, where
//!   that helper takes `-> Measured` (today: `with_confined_realm` in
//!   `crates/vitrin-core/src/spawn.rs`). `E0069`, whose wording names the
//!   shape: "`return;` in a function whose return type is not `()`".
//!
//! **Closed by `cargo xtask skip-scan`**, a text scan and therefore
//! second-line: a bare `return` at a `#[test]`'s own level -- including
//! behind an `if` whose condition takes a closure, which is what most
//! conditions in this tree look like and which the classifier missed until
//! it started comparing the pipe's parenthesis depth with the brace's -- a
//! `process::exit` in a test body, a sanctioned skip with no `INVENTORY`
//! entry, an `INVENTORY` entry with no call site, a require-variable
//! `.github/workflows/ci.yml` stopped declaring **in the job that needs it**
//! or that a `run:` line assigns in shell (where it would apply to one
//! command and be invisible to the job-scoped table), and -- the failure one
//! level below a skip -- a `#[test]` that no CI step compiles and selects,
//! `#[ignore]`d or `#[cfg_attr(<cond>, ignore)]`d out of every build,
//! which emits no output for any census to read.
//!
//! **Closed by `cargo xtask skip-census`**: a skip nobody justified, and an
//! affirmative claim over a run that executed fewer tests than the
//! invocation's floor -- a floor every invocation must declare, and which
//! may not be zero.
//!
//! **Closed by nothing here**, and these are real. Each one is a way to write
//! a test that measures nothing while every gate above stays green; they are
//! listed so that "the scan is clean" is never read as more than it is:
//!
//! * a test that **re-implements a probe inline** -- reading the sysctl, the
//!   ABI, `/proc` or the environment variable itself -- and branches on that.
//!   Two spellings, both invisible: the read in the test body
//!   (`if std::fs::read_to_string("/proc/...").is_err() { return; }`, which
//!   the scan catches only because of the bare `return`, so the same read
//!   wrapped in `if ok { ...whole body... }` is caught by nothing), and the
//!   read hoisted into a private helper returning `bool`. The sealed probes
//!   are unobservable; nothing stops a new one being written. Review is the
//!   only check.
//! * a bare `return;` inside a closure whose helper does **not** take
//!   `-> Measured`. The scan deliberately ignores closure returns (this
//!   tree has eight legitimate ones: watchdogs, fork children, event-loop
//!   callbacks), so adopting the token is per-helper and opt-in.
//! * an early `return vitrin_skip::measured();` -- a written claim to have
//!   measured, which review can see and a type cannot.
//! * **every assertion inside a conditional that is false at run time.** The
//!   test executes, passes, prints nothing and is counted by every census
//!   here as a test that ran. Neither a scan over `return`s nor a count of
//!   markers can see it; only reading the test can.
//! * a **home-grown `macro_rules!` that expands to the `return`.** The scan's
//!   decidability argument is "a `#[test]` body may not contain a bare
//!   `return` at all", and it reads bodies before expansion, so a local macro
//!   hiding the return is a skip it cannot see -- the same hole as an inline
//!   probe, reached by a different route.
//! * a test that runs and asserts nothing. Nothing here reads assertions,
//!   and no amount of counting will. (Its sibling -- a `#[cfg]`-gated test
//!   no job compiles -- IS now closed, by `cargo xtask skip-scan`'s test
//!   census; twenty-four were in that state, and what is left is published
//!   in `docs/book/src/limits.md`. It needed a different mechanism for the
//!   reason stated above: such a test produces no output at all.) One
//!   spelling deserves its own mention because it looks like a normal test
//!   and passes: an assertion inside a `thread::spawn` the test never
//!   joins. The thread's panic never reaches the harness, so the test is
//!   green whatever the assertion said.
//! * a **`#[cfg_attr(<pred>, test)]`** -- a test whose `#[test]` attribute is
//!   itself conditional. Both scans here find `#[test]` syntactically, so a
//!   test that becomes one only under some predicate is invisible to the
//!   return scan AND to the coverage census, while still running wherever
//!   the predicate holds. This is the one shape that evades both halves at
//!   once, and it is closed by neither.
//! * **macro-generated tests.** The census models a `macro_rules!` that
//!   expands to `#[test]` functions as a single nameless template, not as
//!   the tests it produces. This tree has such a macro -- `roundtrip_test!`
//!   generates on the order of forty tests -- so the census's coverage claim
//!   is per-*template* there, not per-test. Read the per-test claim below as
//!   holding for hand-written `#[test]` functions.
//!
//! # The three parts, and why no two of them suffice
//!
//! 1. **this crate** -- one marker line per skip, and a per-class
//!    require-variable that turns the skip into a **panic** on a machine
//!    that declared it could run. This is the enforcement.
//! 2. **`cargo xtask skip-census`** -- runs a suite, filters the marker
//!    lines out of the noise, itemises them, fails on a marker whose test
//!    is not in a checked-in inventory of justified skips, and refuses to
//!    make any affirmative claim over a run that executed fewer tests
//!    than the invocation's own floor.
//! 3. **`cargo xtask skip-scan`** -- walks every `#[test]` body in the
//!    tree and fails on an early `return` that did not go through this
//!    crate. Second line of defence, not the primary one: it catches the
//!    bare-`return` shape and the stale inventory, it holds
//!    `.github/workflows/ci.yml` to the declarations this crate's classes
//!    say it makes (per job, not merely somewhere in the file, and in an
//!    `env:` block rather than as a shell prefix on one command), and it
//!    holds every `#[test]` in the tree to a CI step that compiles and
//!    selects it.
//!
//! # The asymmetry, which is the design
//!
//! A developer whose kernel cannot confine must still be able to run the
//! suite: on their machine a skip prints its marker and returns, exactly
//! as before. **CI is the asymmetric half** -- it sets the require
//! variable for every class its runner is asserted to satisfy, and there a
//! skip is a failure. Nothing here makes local development harder, and
//! nothing here claims a green CI job that measured nothing has become
//! impossible -- "What this does NOT close", above, is the exact bound.
//! What it claims is narrower and checkable: no *sanctioned* capability
//! answer can be learned without the marker being printed and the
//! require-check applied.
//!
//! # Why not `#[ignore]`
//!
//! `#[ignore]` is the one skip form libtest already reports by default
//! (`test X ... ignored, <reason>`, plus `N ignored` in the summary), so
//! it looks like the free answer. It is a **compile-time** decision, and
//! every site this crate serves is a **runtime** probe of the machine --
//! whether this kernel grants six namespaces, what Landlock ABI it
//! answers, whether a C shim was built. Converting them would mean
//! deciding at build time what only the kernel can answer at run time.
//!
//! # Using it
//!
//! A probe returns a [`Verdict`]; the test hands it straight to
//! [`skip_unless!`] and never sees it again:
//!
//! ```text
//! /// Whether this kernel grants the six namespaces a confined realm needs.
//! fn namespaces() -> vitrin_skip::Verdict {
//!     let report = isolation::Report::probe();
//!     vitrin_skip::Verdict::capable_if(
//!         report.mechanism(Namespaces).is_available(),
//!         format!("this kernel reports ns.all={}", report.namespaces_combined),
//!     )
//! }
//!
//! #[test]
//! fn a_confined_realm_cannot_reach_the_canary() {
//!     vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
//!     // ...the measurement...
//! }
//! ```
//!
//! [`skip_unless!`] **contains the `return`**. That is deliberate and it
//! is what makes `cargo xtask skip-scan` a decidable rule rather than a
//! heuristic: a `#[test]` body may not contain a bare `return` at all, so
//! the scan does not have to guess whether one is a skip.

use std::fmt::Write as _;

/// The one string the emitters print and the census greps for.
///
/// It is a **single exported constant** on purpose. The census
/// (`crates/xtask/src/skip_census.rs`) imports this rather than spelling
/// the literal again, so renaming the marker cannot silently blind the
/// census -- the two cannot desynchronise, because there is only one of
/// them. (Contrast `tests/integration/run.sh`, which greps for a message
/// written out longhand in nineteen separate Python files.)
pub const MARKER: &str = "VITRIN-SKIP";

/// A capability probe's answer, **sealed**.
///
/// This type is the whole mechanism, and every absence in it is
/// deliberate. There is no accessor, no `Debug`, no `Clone`, no `Copy`,
/// no `PartialEq`, no `Deref`, no public field and no public variant, so
/// a caller holding one can do exactly one thing with it: pass it to
/// [`decide`] (which [`skip_unless!`] does for them). Learning the answer
/// therefore *requires* emitting the marker line and applying the
/// require-variable.
///
/// That is what closes the three shapes a source scan cannot see. Each of
/// these is a silent skip against a probe that returns `bool` or
/// `Option<T>`, and each fails to compile against a probe that returns a
/// `Verdict`:
///
/// ```text
/// if probe().is_none() { measure(); }                 // guard inverted
/// with_realm(|facts| { let Some(x) = probe() else { return; }; ... })
/// if helper_says_yes() { measure(); }                 // guard hoisted
/// ```
///
/// Each `compile_fail` below carries the error code it expects, and **the
/// annotation is not what checks it**. A bare `compile_fail` passes for *any*
/// compile error, so a typo in the fixture -- a misspelt path, a missing
/// semicolon -- would keep it green while asserting nothing about the sealing;
/// and rustdoc's `,E0599` error-index check is nightly-only, so on this
/// repository's pinned stable toolchain it is decorative (measured on 1.94.1:
/// a doctest annotated `E0277` that really fails with `E0599` still passes).
/// Writing the code and calling it enforcement would be exactly the defect
/// this crate exists against, so the codes below are pinned by a real test --
/// [`each_sealed_operation_fails_with_the_error_code_that_names_the_seal`],
/// which compiles each fixture against a real build of this crate and asserts
/// **three** things, because the code alone is not enough:
///
/// 1. the **passing twin compiles**. It is the fixture minus its last line,
///    so every name in the fixture is proved real by a compiler before the
///    refusal is credited to the seal. Without this, `Verdict::incapabel(..)`
///    -- a typo -- fails with `E0599` too, and an intact seal and a broken
///    fixture are indistinguishable;
/// 2. the fixture then fails with that ONE error code;
/// 3. the compiler's message **names the operation** (`no method named
///    `is_skip``, `field `reason` ... is private`), so a typo *inside* the
///    sealed line cannot pass as the seal either.
///
/// [`each_sealed_operation_fails_with_the_error_code_that_names_the_seal`]:
///     https://github.com/vitrin-os/vitrin-os/issues/288
///
/// The no-accessor rule, asserted. Owning a verdict is fine:
///
/// ```
/// let verdict = vitrin_skip::Verdict::incapable("no namespaces here");
/// let _held = verdict;
/// ```
///
/// ...and asking it anything is not -- `E0599`, no such method:
///
/// ```compile_fail,E0599
/// let verdict = vitrin_skip::Verdict::incapable("no namespaces here");
/// if verdict.is_skip() {}
/// ```
///
/// The no-destructuring rule, the same way -- constructing one is fine:
///
/// ```
/// let verdict = vitrin_skip::Verdict::capable();
/// let _held = verdict;
/// ```
///
/// ...and taking it apart is not -- `E0451`, the field is private:
///
/// ```compile_fail,E0451
/// let verdict = vitrin_skip::Verdict::capable();
/// let vitrin_skip::Verdict { reason, .. } = verdict;
/// ```
///
/// No `PartialEq`, so two verdicts cannot be compared into a `bool`. Holding
/// two of them is fine:
///
/// ```
/// let verdict = vitrin_skip::Verdict::capable();
/// let _second = vitrin_skip::Verdict::capable();
/// ```
///
/// ...and comparing them is not -- `E0369`, the operator does not apply:
///
/// ```compile_fail,E0369
/// let verdict = vitrin_skip::Verdict::capable();
/// let _ = verdict == vitrin_skip::Verdict::capable();
/// ```
///
/// ...and no `Debug`, so the answer cannot be recovered from a rendering.
/// Formatting a type that HAS one still works, which is what makes the
/// refusal below a fact about `Verdict` rather than about `format!`:
///
/// ```
/// let verdict = vitrin_skip::Verdict::capable();
/// let _ = format!("{:?}", "a str, which does implement Debug");
/// ```
///
/// ```compile_fail,E0277
/// let verdict = vitrin_skip::Verdict::capable();
/// let _ = format!("{:?}", verdict);
/// ```
#[must_use = "a Verdict that is never handed to skip_unless! has measured nothing"]
pub struct Verdict {
    /// `None` when the machine is capable. Private, and there is no method
    /// that returns it or anything derived from it.
    reason: Option<String>,
    /// The [`Require::Floor`] rung this measurement needed, when the class
    /// is a ladder. Carried by the probe rather than transcribed at the
    /// call site, so the number the macro enforces and the number the
    /// probe compared against cannot drift apart.
    needed: Option<u32>,
}

impl Verdict {
    /// The machine can run the measurement; nothing is skipped and nothing
    /// is printed.
    pub fn capable() -> Self {
        Self {
            reason: None,
            needed: None,
        }
    }

    /// The machine cannot, for this reason -- which becomes the marker
    /// line's tail and the panic message's, so it must name the machine
    /// state rather than the test.
    pub fn incapable(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            needed: None,
        }
    }

    /// The machine cannot because it is below rung `needed` of a ladder
    /// class ([`LANDLOCK_ABI`]).
    ///
    /// The rung travels with the verdict because it is the probe that knows
    /// it: `VITRIN_REQUIRE_LANDLOCK_ABI=7` requires every rung at or below
    /// 7 and honestly forgives the ones above.
    pub fn incapable_below_rung(needed: u32, reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            needed: Some(needed),
        }
    }

    /// [`capable`](Verdict::capable) when `condition` holds, and
    /// [`incapable`](Verdict::incapable) otherwise.
    ///
    /// `reason` is built even on the capable path, which is the price of
    /// taking it by value; every reason in this tree is a short `format!`
    /// over numbers a probe already had.
    pub fn capable_if(condition: bool, reason: impl Into<String>) -> Self {
        if condition {
            Self::capable()
        } else {
            Self::incapable(reason)
        }
    }

    /// [`capable`](Verdict::capable) when `condition` holds, and
    /// [`incapable_below_rung`](Verdict::incapable_below_rung) otherwise.
    pub fn capable_if_at_rung(condition: bool, needed: u32, reason: impl Into<String>) -> Self {
        if condition {
            Self::capable()
        } else {
            Self::incapable_below_rung(needed, reason)
        }
    }
}

/// Proof that a measurement closure reached its end.
///
/// The second half of the sealing, for the second shape a source scan
/// cannot see. A helper that *runs* a measurement takes a closure, and a
/// bare `return;` inside that closure leaves the closure rather than the
/// test -- so the helper completes, the test reports `ok`, and nothing was
/// asserted. `cargo xtask skip-scan` deliberately does not count returns
/// inside closures (this tree has eight legitimate ones: watchdogs, fork
/// children, event-loop callbacks), so that shape is invisible to it, and
/// it was demonstrated end to end against the first version of this crate.
///
/// A measurement helper therefore declares its closure as `-> Measured`:
///
/// ```text
/// fn with_confined_realm(label: &str, f: impl FnOnce(i32, &Facts) -> vitrin_skip::Measured)
/// ```
///
/// `return;` in such a closure yields `()` where `Measured` is expected and
/// **does not compile**. The closure must end in [`measured()`], which is
/// the only way one exists.
///
/// That claim used to be prose, and prose is what this crate exists against,
/// so it is a pair of doctests pinned by the same test as [`Verdict`]'s four
/// -- passing twin first, so the refusal below cannot be a typo:
///
/// ```
/// fn with_realm(f: impl FnOnce() -> vitrin_skip::Measured) { let _ = f(); }
/// with_realm(|| vitrin_skip::measured());
/// ```
///
/// ...and the same closure with a bare `return;` in it does not compile --
/// `E0069`, whose wording names the shape exactly ("`return;` in a function
/// whose return type is not `()`"):
///
/// ```compile_fail,E0069
/// fn with_realm(f: impl FnOnce() -> vitrin_skip::Measured) { let _ = f(); }
/// with_realm(|| { if false { return; } vitrin_skip::measured() });
/// ```
///
/// What this does *not* do is stop somebody writing `return
/// vitrin_skip::measured();` early. That is deliberate and it is the
/// honest bound: the goal is to make the *accidental, idiomatic* silent
/// skip impossible, not to make a determined lie impossible. An early
/// `measured()` is a written claim to have measured, in a diff, which is a
/// thing review can see -- unlike `return;`, which reads like nothing.
#[must_use = "a Measured that is dropped rather than returned proves nothing"]
pub struct Measured(());

/// Make the [`Measured`] token a measurement closure ends with.
///
/// Deliberately trivial. It is a shape, not a check: what it buys is that
/// the closure cannot end -- or leave -- without saying something.
pub fn measured() -> Measured {
    Measured(())
}

/// How a class decides whether *this* machine was supposed to be able to
/// run the skipped test.
///
/// Three shapes rather than one flag, because a single flat
/// `VITRIN_REQUIRE_EVERYTHING=1` would be red on day one for an honest
/// machine limitation and would be reverted within a week. A require
/// variable must be able to say "this runner grants namespaces" without
/// also claiming "this runner answers Landlock ABI 9".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Require {
    /// Required when [`Class::require_var`] is set to `1`: a plain
    /// declaration that this machine can do the thing.
    WhenSet,
    /// Required when [`Class::require_var`] parses as a number **at or
    /// above the rung the skipping test needed**. `VITRIN_REQUIRE_LANDLOCK_ABI=7`
    /// says "this kernel answers at least 7", so a test that wanted rung 2
    /// and skipped is a misconfiguration, while one that wanted rung 9 is
    /// an honest limit and still skips.
    Floor,
    /// Required whenever `CI` is set, **unless** [`Class::require_var`] is
    /// set to `1` -- a job that genuinely cannot satisfy the class declares
    /// so in its own `env:` block, and every other job is required by
    /// default.
    ///
    /// This is the one pattern that already worked here before #288
    /// (`c_shim_conforms_to_the_real_core`'s `assert!`), generalised. Its
    /// good half is that an *undeclared* skip under CI is red; its bad half
    /// was that a *declared* one printed nothing a log could show, which
    /// the marker line now fixes.
    UnderCiUnlessDeclared,
}

/// One class of machine capability a test can need.
///
/// A class is a claim about the *host*, never about the test: "this
/// machine grants unprivileged user namespaces", "this machine answers
/// Landlock ABI N". Tests naming the same class skip together and are
/// required together.
#[derive(Debug, Clone, Copy)]
pub struct Class {
    /// Printed in the marker line, and the key the census inventory uses.
    pub name: &'static str,
    /// The environment variable CI sets to declare this machine capable.
    pub require_var: &'static str,
    /// How [`require_var`](Class::require_var) is read.
    pub require: Require,
}

/// The six namespaces a confined realm spawn asks the kernel for.
///
/// CI sets `VITRIN_REQUIRE_CONFINEMENT=1` in the `rust` job, which first
/// takes the remedy `vitrind`'s own preflight prints
/// (`kernel.apparmor_restrict_unprivileged_userns=0`). A runner that can
/// no longer confine is then RED rather than green-and-empty, which is
/// the entire point -- and it means that job depends on that sysctl
/// staying permitted. That is intended: the alternative is the job going
/// green having exercised nothing.
pub const CONFINEMENT: Class = Class {
    name: "confinement",
    require_var: "VITRIN_REQUIRE_CONFINEMENT",
    require: Require::WhenSet,
};

/// A Landlock ruleset rung the running kernel must actually support.
///
/// Floor semantics, because the rungs are a ladder: a machine can honestly
/// be able to run the ABI-2 measurement and honestly not the ABI-9 one.
pub const LANDLOCK_ABI: Class = Class {
    name: "landlock-abi",
    require_var: "VITRIN_REQUIRE_LANDLOCK_ABI",
    require: Require::Floor,
};

/// A file or directory layout the test needs the host to already have --
/// `/usr/bin/env`, a bindable ancestor of the build directory.
///
/// These look like the least interesting class and are the reason it
/// exists: they are the skips that fire after an unrelated change (a
/// `CARGO_TARGET_DIR` move, a slimmer container image) with no other
/// symptom at all.
pub const HOST_TOOLING: Class = Class {
    name: "host-tooling",
    require_var: "VITRIN_REQUIRE_HOST_TOOLING",
    require: Require::WhenSet,
};

/// A built C shim (`VITRIN_C_SHIM_BIN`) for the cross-track checks.
///
/// [`Require::UnderCiUnlessDeclared`], so the `rust` job -- which has no C
/// toolchain -- must keep declaring `VITRIN_C_SHIM_CONFORMANCE_SKIP=1`,
/// and the `conformance` job, which builds the shim for real, is required
/// without having to say anything.
pub const C_SHIM: Class = Class {
    name: "c-shim",
    require_var: "VITRIN_C_SHIM_CONFORMANCE_SKIP",
    require: Require::UnderCiUnlessDeclared,
};

/// A real GPU: an EGL device with a working GBM pipeline, and a renderer
/// that imports the format under test.
///
/// No CI job runs these today (they are `#[ignore]`d, behind the
/// `gpu-tests` feature, behind `VITRIN_GPU_TESTS=1`), so nothing sets the
/// variable -- and `cargo xtask skip-scan` asserts that nothing does, so
/// "no job requires this" stays a checked fact rather than an assumption.
/// They emit markers anyway: the census is how a developer's GPU box says
/// which half of the acceptance its driver actually reached.
pub const GPU: Class = Class {
    name: "gpu",
    require_var: "VITRIN_REQUIRE_GPU",
    require: Require::WhenSet,
};

/// Every class, for the census's inventory validation and for
/// documentation. Keep in sync by adding to this list -- `skip-census`
/// rejects a marker naming a class that is not here, and `skip-scan`
/// refuses a class that `.github/workflows/ci.yml` has no stated position
/// on.
pub const CLASSES: &[Class] = &[CONFINEMENT, LANDLOCK_ABI, HOST_TOOLING, C_SHIM, GPU];

/// What a require-variable's value says, once validated.
///
/// The validation is the point. Before it, enforcement rested on the magic
/// string `"1"` and everything else -- `"true"`, `"yes"`, `"01"`, a
/// trailing space, a one-character typo in the workflow -- silently meant
/// "not required", so the mechanism could be disabled by an edit that
/// looked like a no-op. An unrecognised value now panics.
enum Declaration {
    /// The variable is unset.
    Absent,
    /// Set to `0`: this machine explicitly does *not* declare the class.
    Off,
    /// Set to `1`: this machine declares it can satisfy the class.
    On,
    /// A [`Require::Floor`] class's declared rung.
    Floor(u32),
}

/// Read and validate `class`'s require-variable.
///
/// # Panics
///
/// If the variable is set to anything the class does not recognise. That
/// is deliberately fatal rather than false: a value nobody validates is a
/// mechanism that can be switched off by a typo, which is the failure
/// class this whole crate exists to close.
fn declaration(class: &Class) -> Declaration {
    let Some(raw) = std::env::var_os(class.require_var) else {
        return Declaration::Absent;
    };
    let Some(text) = raw.to_str() else {
        panic!(
            "{} is set to a value that is not UTF-8. It must be {}.",
            class.require_var,
            expected(class)
        );
    };
    match class.require {
        Require::Floor => match text.trim().parse::<u32>() {
            Ok(rung) => Declaration::Floor(rung),
            Err(_) => panic!(
                "{}={text:?} is not a Landlock ABI rung. It must be {}. A value nobody validates \
                 is an enforcement mechanism a typo can switch off, which is issue #288 in the \
                 code that closes issue #288.",
                class.require_var,
                expected(class)
            ),
        },
        Require::WhenSet | Require::UnderCiUnlessDeclared => match text.trim() {
            "1" => Declaration::On,
            "0" => Declaration::Off,
            _ => panic!(
                "{}={text:?} is not a recognised value. It must be {}. A value nobody validates \
                 is an enforcement mechanism a typo can switch off, which is issue #288 in the \
                 code that closes issue #288.",
                class.require_var,
                expected(class)
            ),
        },
    }
}

/// The human half of every rejection message above.
fn expected(class: &Class) -> &'static str {
    match class.require {
        Require::WhenSet => {
            "`1` (this machine can satisfy the class, so a skip is a failure) or \
                             `0` (it cannot, so a skip is honest), or unset"
        }
        Require::Floor => {
            "a decimal Landlock ABI rung, e.g. `7` (every rung at or below 7 must \
                           run), or unset"
        }
        Require::UnderCiUnlessDeclared => {
            "`1` (this job cannot satisfy the class, so its skip is excused) or `0` (it can, same \
             as unset), or unset"
        }
    }
}

/// Whether this machine was declared able to run a test skipping for
/// `class`, where `needed` is the rung a [`Require::Floor`] class wanted.
///
/// Reads the environment and nothing else, so a test can assert both
/// answers by setting the variable around it. It says nothing about
/// whether the machine *is* capable -- only about what it claimed -- so it
/// is not a way to learn a [`Verdict`]'s answer.
///
/// # Panics
///
/// If `class`'s require-variable holds a value the class does not
/// recognise; see [`Declaration`].
#[must_use]
pub fn is_required(class: &Class, needed: Option<u32>) -> bool {
    let declared = declaration(class);
    match class.require {
        Require::WhenSet => matches!(declared, Declaration::On),
        Require::Floor => match declared {
            Declaration::Floor(floor) => floor >= needed.unwrap_or(0),
            _ => false,
        },
        // The variable EXCUSES rather than requires, so `0` reads exactly
        // like unset: a job that does not declare an inability is required.
        Require::UnderCiUnlessDeclared => {
            std::env::var_os("CI").is_some() && !matches!(declared, Declaration::On)
        }
    }
}

/// The line a skip prints, verbatim -- built here rather than at the call
/// site so the census has exactly one format to parse.
///
/// `VITRIN-SKIP <class> <crate>::<test path> <reason>`, on one line: the
/// reason's newlines and carriage returns are collapsed to spaces, because
/// a marker that can span lines is a marker a `grep` census would report
/// half of.
#[must_use]
pub fn line(class: &Class, test: &str, reason: &str) -> String {
    let mut out = String::with_capacity(reason.len() + 64);
    let _ = write!(out, "{MARKER} {} {test} ", class.name);
    for ch in reason.chars() {
        out.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
    }
    out
}

/// The test's own path, as libtest knows it.
///
/// libtest names each test's thread after the test, so the running
/// thread's name *is* the path (`spawn::tests::a_confined_realm_...`) --
/// derived, never transcribed, so it cannot drift from a rename. Under
/// `--test-threads=1` libtest runs on the main thread instead, and the
/// fallback is the emitting module's path with the crate prefix stripped
/// (the crate is printed separately). Attribution survives either way:
/// `--show-output` prints each marker under its own
/// `---- <path> stdout ----` heading regardless.
#[must_use]
pub fn test_path(krate: &str, module: &str) -> String {
    let thread = std::thread::current();
    match thread.name() {
        Some(name) if !name.is_empty() && name != "main" => format!("{krate}::{name}"),
        _ => {
            let module = module.strip_prefix(krate).unwrap_or(module);
            let module = module.strip_prefix("::").unwrap_or(module);
            format!("{krate}::{module}")
        }
    }
}

/// Consume a [`Verdict`] and answer whether the caller must stop --
/// **after** printing the marker line and applying the require-variable.
///
/// This is the only operation the public API offers on a `Verdict`, and
/// that is the mechanism: there is no path from a probe to its answer that
/// does not pass through here, so a capability answer cannot be learned
/// without the census learning it too. [`skip_unless!`] wraps it with the
/// `return` a macro can write and a function cannot.
///
/// Calling it directly and ignoring the answer is possible and is not a
/// silent skip: by the time it returns, the marker is on stdout and a
/// required class has already panicked. What such a call would evade is
/// the *scan*, which is why [`skip_unless!`] carries the return and why
/// `cargo xtask skip-scan` reads test bodies for bare ones.
///
/// # Panics
///
/// If the verdict is a skip and [`is_required`] says this machine declared
/// it could run the test. That panic is the enforcement.
#[must_use = "the caller must return when this answers true, or the test continues into a \
              measurement the machine cannot make"]
pub fn decide(class: &Class, krate: &str, module: &str, verdict: Verdict) -> bool {
    let Some(reason) = verdict.reason else {
        return false;
    };
    let needed = verdict.needed;
    let test = test_path(krate, module);
    let line = line(class, &test, &reason);
    // stdout, not stderr: `--show-output` attributes a passing test's
    // stdout under its own heading, which is what keeps a marker readable
    // in a parallel run.
    println!("{line}");
    if is_required(class, needed) {
        let why = match class.require {
            Require::WhenSet => format!(
                "{}=1 declares that this machine can run the `{}` class",
                class.require_var, class.name
            ),
            Require::Floor => format!(
                "{}={} declares a Landlock ABI floor at or above the rung {} this measurement \
                 needed",
                class.require_var,
                std::env::var(class.require_var).unwrap_or_default(),
                needed.unwrap_or(0),
            ),
            Require::UnderCiUnlessDeclared => format!(
                "CI is set and {} is not, so this job is required to satisfy the `{}` class -- a \
                 job that genuinely cannot declares so by setting {}=1 in its own env block",
                class.require_var, class.name, class.require_var,
            ),
        };
        panic!(
            "{line}\n\nThis skip is a FAILURE here, not a skip: {why}. Either the machine \
             regressed (fix it, or stop declaring it), or this test is skipping for a reason \
             that has nothing to do with the class it named. See crates/vitrin-skip and issue \
             #288."
        );
    }
    true
}

/// Run the enclosing test only if the probe's [`Verdict`] says this
/// machine can; otherwise print the marker and **return**.
///
/// The `return` lives inside the macro. That is what lets `cargo xtask
/// skip-scan` state its rule as "a `#[test]` body contains no bare
/// `return`", and it is why this is a macro rather than a function.
///
/// The second argument is a `Verdict` **expression**, so the probe is
/// evaluated here and its answer is never bound to a name the test can
/// branch on:
///
/// ```text
/// vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, namespaces());
/// ```
#[macro_export]
macro_rules! skip_unless {
    ($class:expr, $verdict:expr $(,)?) => {
        if $crate::decide(
            &$class,
            ::core::env!("CARGO_CRATE_NAME"),
            ::core::module_path!(),
            $verdict,
        ) {
            return;
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Every test below sets and restores process-wide environment
    /// variables, and libtest runs them in parallel in one process. Without
    /// this they would race each other into a flake that looks like a bug in
    /// the enforcement -- which is the worst possible flake for a mechanism
    /// whose entire job is to be believed.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Take [`ENV_LOCK`], ignoring poisoning: a poisoned lock here means a
    /// sibling test already failed, and blocking on it would turn one red
    /// test into a hung suite.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The marker is one line whatever the reason did, because the census
    /// is line-oriented and half a marker is worse than none.
    #[test]
    fn a_multi_line_reason_still_produces_exactly_one_marker_line() {
        let line = line(
            &CONFINEMENT,
            "vitrind::spawn::tests::x",
            "first\nsecond\r\nthird",
        );
        assert_eq!(line.lines().count(), 1, "the marker must not span lines");
        assert!(line.starts_with(MARKER));
        assert!(
            line.contains("first second"),
            "newlines become spaces: {line}"
        );
    }

    /// Set `var` to `value` (or unset it), read `is_required`, put back
    /// whatever was there. Serialised by the caller, never in parallel:
    /// the process environment is shared by every test in this binary.
    fn with(var: &str, value: Option<&str>, class: &Class, needed: Option<u32>) -> bool {
        let saved = std::env::var_os(var);
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        let answer = is_required(class, needed);
        match saved {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        answer
    }

    /// The three require shapes, each asserted in **both** directions --
    /// a require rule that can only say yes is a rule that never fires.
    #[test]
    fn each_require_shape_answers_both_ways() {
        let _lock = env_lock();
        assert!(!with(CONFINEMENT.require_var, None, &CONFINEMENT, None));
        assert!(with(CONFINEMENT.require_var, Some("1"), &CONFINEMENT, None));
        assert!(
            !with(CONFINEMENT.require_var, Some("0"), &CONFINEMENT, None),
            "only `1` declares a machine capable; `0` must not"
        );

        // Floor: a declared floor of 7 requires rung 2 and forgives rung 9.
        assert!(with(
            LANDLOCK_ABI.require_var,
            Some("7"),
            &LANDLOCK_ABI,
            Some(2)
        ));
        assert!(with(
            LANDLOCK_ABI.require_var,
            Some("7"),
            &LANDLOCK_ABI,
            Some(7)
        ));
        assert!(
            !with(LANDLOCK_ABI.require_var, Some("7"), &LANDLOCK_ABI, Some(9)),
            "a rung above the declared floor is an honest kernel limit, not a misconfiguration"
        );
        assert!(!with(
            LANDLOCK_ABI.require_var,
            None,
            &LANDLOCK_ABI,
            Some(1)
        ));
    }

    /// `UnderCiUnlessDeclared` is inverted -- the variable *excuses* rather
    /// than requires -- so it gets its own both-ways assertion.
    #[test]
    fn the_declared_shape_requires_under_ci_and_is_excused_by_its_own_variable() {
        let _lock = env_lock();
        let saved_ci = std::env::var_os("CI");
        let saved_declared = std::env::var_os(C_SHIM.require_var);

        std::env::remove_var("CI");
        std::env::remove_var(C_SHIM.require_var);
        assert!(
            !is_required(&C_SHIM, None),
            "off CI a developer without a built shim must still be able to skip"
        );

        std::env::set_var("CI", "true");
        assert!(
            is_required(&C_SHIM, None),
            "under CI an UNDECLARED skip is a failure"
        );

        std::env::set_var(C_SHIM.require_var, "0");
        assert!(
            is_required(&C_SHIM, None),
            "...and `0` is a job saying nothing, so it stays required"
        );

        std::env::set_var(C_SHIM.require_var, "1");
        assert!(
            !is_required(&C_SHIM, None),
            "...and a job that declared it cannot build C is excused"
        );

        match saved_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match saved_declared {
            Some(v) => std::env::set_var(C_SHIM.require_var, v),
            None => std::env::remove_var(C_SHIM.require_var),
        }
    }

    /// A value nobody recognises is REJECTED rather than read as "off".
    ///
    /// This is the whole point of validating: before it, a one-character
    /// typo in `.github/workflows/ci.yml` (`VITRIN_REQUIRE_CONFINEMENT:
    /// "true"`, or a trailing character) turned the enforcement off and
    /// left the job green. Asserted for every class, in the shape each one
    /// would actually be mistyped.
    #[test]
    fn an_unrecognised_require_value_is_fatal_rather_than_silently_off() {
        let _lock = env_lock();
        for (class, bad) in [
            (&CONFINEMENT, "true"),
            (&HOST_TOOLING, "yes"),
            (&C_SHIM, ""),
            (&GPU, "on"),
            (&LANDLOCK_ABI, "seven"),
        ] {
            let saved = std::env::var_os(class.require_var);
            std::env::set_var(class.require_var, bad);
            let outcome = std::panic::catch_unwind(|| is_required(class, Some(1)));
            match saved {
                Some(v) => std::env::set_var(class.require_var, v),
                None => std::env::remove_var(class.require_var),
            }
            assert!(
                outcome.is_err(),
                "{}={bad:?} was accepted; an unvalidated value is an enforcement mechanism a \
                 typo can switch off",
                class.require_var
            );
        }
    }

    /// The valid values stay valid, so the rejection above is a rule about
    /// bad input rather than a rule that rejects everything.
    #[test]
    fn every_recognised_require_value_is_accepted() {
        let _lock = env_lock();
        for (class, good) in [
            (&CONFINEMENT, "1"),
            (&CONFINEMENT, "0"),
            (&HOST_TOOLING, "1"),
            (&C_SHIM, "1"),
            (&GPU, "0"),
            (&LANDLOCK_ABI, "7"),
            (&LANDLOCK_ABI, " 7 "),
        ] {
            let saved = std::env::var_os(class.require_var);
            std::env::set_var(class.require_var, good);
            let outcome = std::panic::catch_unwind(|| is_required(class, Some(1)));
            match saved {
                Some(v) => std::env::set_var(class.require_var, v),
                None => std::env::remove_var(class.require_var),
            }
            assert!(
                outcome.is_ok(),
                "{}={good:?} was rejected, and it is a documented value",
                class.require_var
            );
        }
    }

    /// A capable verdict decides "run" and prints nothing; an incapable one
    /// decides "stop". Both directions, because a decision that can only
    /// say one thing is not a decision.
    #[test]
    fn a_capable_verdict_runs_and_an_incapable_one_stops() {
        let _lock = env_lock();
        let saved = std::env::var_os(GPU.require_var);
        std::env::remove_var(GPU.require_var);
        assert!(
            !decide(
                &GPU,
                "vitrin_skip",
                "vitrin_skip::tests",
                Verdict::capable()
            ),
            "a capable verdict must not stop the test"
        );
        assert!(
            decide(
                &GPU,
                "vitrin_skip",
                "vitrin_skip::tests",
                Verdict::incapable("no GPU on this machine, for the unit test")
            ),
            "an incapable verdict must stop the test"
        );
        if let Some(v) = saved {
            std::env::set_var(GPU.require_var, v);
        }
    }

    /// ...and an incapable verdict PANICS where the machine declared it
    /// could run, which is the enforcement half.
    #[test]
    fn an_incapable_verdict_panics_where_the_class_is_required() {
        let _lock = env_lock();
        let saved = std::env::var_os(GPU.require_var);
        std::env::set_var(GPU.require_var, "1");
        let outcome = std::panic::catch_unwind(|| {
            decide(
                &GPU,
                "vitrin_skip",
                "vitrin_skip::tests",
                Verdict::incapable("no GPU, on a machine that claimed one"),
            )
        });
        match saved {
            Some(v) => std::env::set_var(GPU.require_var, v),
            None => std::env::remove_var(GPU.require_var),
        }
        assert!(
            outcome.is_err(),
            "a skip in a declared class must be a failure, not a skip"
        );
    }

    /// One sealed operation: the code the compiler must refuse it with, the
    /// snippet that must compile, and the one line that must not.
    ///
    /// The split into `prelude` and `sealed` is the whole design. The fixture
    /// is `prelude + sealed`, and the **twin is `prelude` alone**, so the two
    /// programs differ by exactly the sealed operation and nothing else --
    /// which is what makes "this did not compile" evidence about the seal
    /// rather than about the fixture.
    struct Seal {
        /// The error code that names this seal.
        code: &'static str,
        /// Items above `fn main`, where the seal needs a helper.
        items: &'static str,
        /// Statements that must compile ON THEIR OWN: the positive control.
        prelude: &'static str,
        /// The one operation the seal forbids.
        sealed: &'static str,
        /// Text the compiler's message must contain, naming what it refused.
        names: &'static str,
    }

    /// The `compile_fail` doctests above assert only that a snippet does not
    /// compile. **Any** compile error satisfies that -- a misspelt path, a
    /// missing semicolon, a renamed constructor -- so each of them would stay
    /// green while proving nothing about the sealing, and the `,E0599`
    /// annotation beside them does not help: rustdoc's error-index check is
    /// nightly-only, and this repository pins stable 1.94.1. Measured before
    /// this test existed: the `is_skip()` doctest annotated `E0277` (the wrong
    /// code entirely) still passed.
    ///
    /// So the codes are pinned here, and pinning the code turned out not to be
    /// enough either. `Verdict::incapabel("x")` -- one transposed letter --
    /// fails with `E0599` exactly like the sealed `verdict.is_skip()` does, so
    /// a fixture that had rotted into nonsense would go on reporting that the
    /// seal held. Three assertions per seal close that:
    ///
    /// 1. the **twin compiles**: `prelude` alone, which is the fixture minus
    ///    its last line. Every name the fixture uses is proved real by a
    ///    compiler before any refusal is credited to the seal;
    /// 2. the fixture -- `prelude + sealed` -- fails with that ONE code;
    /// 3. the message **names the operation**, so a typo inside the sealed
    ///    line cannot pass as the seal either.
    ///
    /// The doctest blocks are re-read from `src/lib.rs` so the two copies of
    /// each snippet cannot drift apart silently.
    #[test]
    fn each_sealed_operation_fails_with_the_error_code_that_names_the_seal() {
        use std::path::PathBuf;
        use std::process::Command;

        const SEALS: &[Seal] = &[
            Seal {
                code: "E0599",
                items: "",
                prelude: "let verdict = vitrin_skip::Verdict::incapable(\"no namespaces here\");",
                sealed: "if verdict.is_skip() {}",
                names: "no method named `is_skip`",
            },
            Seal {
                code: "E0451",
                items: "",
                prelude: "let verdict = vitrin_skip::Verdict::capable();",
                sealed: "let vitrin_skip::Verdict { reason, .. } = verdict;",
                names: "field `reason` of struct `Verdict` is private",
            },
            Seal {
                code: "E0369",
                items: "",
                prelude: "let verdict = vitrin_skip::Verdict::capable();",
                sealed: "let _ = verdict == vitrin_skip::Verdict::capable();",
                names: "binary operation `==` cannot be applied to type `Verdict`",
            },
            Seal {
                code: "E0277",
                items: "",
                prelude: "let verdict = vitrin_skip::Verdict::capable();",
                sealed: "let _ = format!(\"{:?}\", verdict);",
                names: "`Verdict` doesn't implement `Debug`",
            },
            // The second half of the sealing, and until this row it was a
            // sentence rather than a check: nothing compiled a bare `return;`
            // inside a `-> Measured` closure to see what the compiler did with
            // it. `with_confined_realm` in crates/vitrin-core/src/spawn.rs is
            // the real helper this models.
            Seal {
                code: "E0069",
                items: "fn with_realm(f: impl FnOnce() -> vitrin_skip::Measured) { let _ = f(); }",
                prelude: "with_realm(|| vitrin_skip::measured());",
                sealed: "with_realm(|| { if false { return; } vitrin_skip::measured() });",
                names: "`return;` in a function whose return type is not `()`",
            },
        ];

        // The doctests and the snippets above are two copies of the same five
        // pairs. Hold them together, so editing one and not the other is a red
        // test rather than a silent divergence.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(manifest.join("src/lib.rs")).expect("this crate");
        for seal in SEALS {
            let Seal {
                code,
                prelude,
                sealed,
                ..
            } = seal;
            assert!(
                source.contains(&format!("```compile_fail,{code}")),
                "no ```compile_fail,{code} block remains in src/lib.rs, and this test pins that \
                 code to a fixture"
            );
            assert!(
                source.contains(sealed),
                "the {code} doctest no longer contains {sealed:?}; the fixture below it has \
                 drifted from the block it is supposed to be pinning"
            );
            assert!(
                source.contains(prelude),
                "the {code} seal's passing twin ({prelude:?}) is in no doctest any more. The twin \
                 is what tells an intact seal from a fixture that stopped compiling for reasons \
                 of its own"
            );
        }

        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let dir = std::env::temp_dir().join(format!("vitrin-skip-seals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");

        // A real build of this crate, so the fixtures meet the same type the
        // doctests do rather than a stand-in. Zero dependencies is what makes
        // one `rustc` invocation enough.
        let rlib = dir.join("libvitrin_skip.rlib");
        let built = Command::new(&rustc)
            .args(["--edition", "2021", "--crate-type", "rlib", "--crate-name"])
            .arg("vitrin_skip")
            .arg(manifest.join("src/lib.rs"))
            .arg("-o")
            .arg(&rlib)
            .output()
            .expect("running rustc");
        assert!(
            built.status.success(),
            "this crate did not build under a plain rustc:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );

        // Compile one program against that build, and say what happened.
        let compile = |name: &str, program: &str| -> (bool, String) {
            let path = dir.join(format!("{name}.rs"));
            std::fs::write(&path, program).expect("writing the fixture");
            let out = Command::new(&rustc)
                .args(["--edition", "2021", "--emit", "metadata", "--extern"])
                .arg(format!("vitrin_skip={}", rlib.display()))
                .arg(&path)
                .arg("-o")
                .arg(dir.join(format!("{name}.meta")))
                .output()
                .expect("running rustc");
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        };

        for seal in SEALS {
            let Seal {
                code,
                items,
                prelude,
                sealed,
                names,
            } = seal;
            let twin = format!("{items}\nfn main() {{\n    {prelude}\n}}\n");
            let fixture = format!("{items}\nfn main() {{\n    {prelude}\n    {sealed}\n}}\n");

            // (1) The twin, which shares every name with the fixture.
            let (ok, stderr) = compile(&format!("{code}-twin"), &twin);
            assert!(
                ok,
                "the {code} seal's PASSING twin did not compile, so the refusal below proves \
                 nothing about the seal -- the fixture itself is broken:\n{twin}\n{stderr}"
            );

            // (2) and (3): the fixture, refused by name.
            let (ok, stderr) = compile(code, &fixture);
            assert!(
                !ok,
                "the {code} fixture COMPILED. The seal it asserts is gone:\n{fixture}"
            );
            assert!(
                stderr.contains(&format!("error[{code}]")),
                "the {code} fixture failed for a different reason, so its doctest is green over \
                 evidence it does not have:\n{stderr}"
            );
            assert!(
                stderr.contains(names),
                "the {code} fixture failed with that code but the compiler did not say \
                 {names:?} -- so what it refused is not the operation this seal is about:\n\
                 {stderr}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every class's `require_var` is distinct, and every class name is
    /// distinct -- the census keys its inventory on the name, and two
    /// classes sharing one would let a declaration for one silently
    /// require the other.
    #[test]
    fn class_names_and_variables_are_unique() {
        for (i, a) in CLASSES.iter().enumerate() {
            for b in &CLASSES[i + 1..] {
                assert_ne!(a.name, b.name, "two classes share a name");
                assert_ne!(
                    a.require_var, b.require_var,
                    "two classes share a require variable"
                );
            }
        }
    }
}
