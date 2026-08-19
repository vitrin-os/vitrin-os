// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask isolation-matrix [--check]` -- generate
//! `docs/book/src/isolation-matrix.md` (P2.6.3, issue #187).
//!
//! # What issue #187 asked for, and what this is
//!
//! PRD §20 carries a one-line caveat -- "Landlock coverage is kernel-dependent
//! ... older kernels degrade to namespace-only enforcement" -- and #187's job
//! was to turn it into **a table a reader can check their own kernel
//! against**. Everything that landed before this module was the *ruleset*
//! (`crates/vitrin-realm-init/src/landlock.rs`); the table itself had never
//! been built, and `docs/book/src/limits.md` said so in as many words.
//!
//! The owner's decision of 2026-08-15 narrowed the target before this was
//! written: there is a **declared ABI floor**
//! ([`vitrin_realm_init::LANDLOCK_MIN_ABI`], lowered from 7 to 6 on 2026-08-16)
//! rather than a degradation ladder, so a kernel below it is refused instead of
//! confined at a weaker rung. That makes this **not** a 1..9 degradation ladder.
//! It is a matrix of
//! two things:
//!
//! 1. **what this build requires** -- the floor, the ceiling, and what each
//!    rung of the ABI buys the ruleset on the way; and
//! 2. **what this machine provides** -- which is not in the table at all, and
//!    the next section says why.
//!
//! # This generator probes NOTHING, and that is a decision with a cost
//!
//! `session_matrix.rs` refuses runtime probing so its output stays diff-clean.
//! This module inherits that rule and has a second, harder reason for it: the
//! two machines this repository can run are a development box reporting
//! Landlock **ABI 9** and a CI runner reporting **ABI 7**. A table that
//! recorded the ABI of the machine that generated it could not be
//! byte-identical on both, so `--check` would be red on every pull request --
//! the gate would be measuring the runner, not the repository.
//!
//! So every number on the emitted page is a **build** fact, derived from the
//! shipped source (see [`Ladder::from_source`] and [`Constants::from_source`])
//! rather than typed here. The **machine** half stays a command the reader
//! runs -- `vitrind --print-isolation` prints `landlock.abi=`, `vitrind
//! --print-floor` prints `build.landlock_min_abi=` -- and the page's verdict
//! table says what this build does with each possible answer.
//!
//! The cost, stated rather than buried: this is **not** the acceptance
//! criterion `docs/plan/02-phase-2-semantic-epochs.md` restated for P2.6.3,
//! which asks for "one row per ABI actually reported" on "each kernel in the
//! CI matrix". No such per-kernel row set exists here, and nothing in this
//! module measures a kernel. The plan carries that as Correction 5.
//!
//! # Why a row cannot exist without a claim, and a claim cannot exist without
//! a row
//!
//! #187 is explicit: *"a row with a right and no claim, or a claim with no
//! row, fails the generator."* That is enforced here as two rejections in
//! [`render`], not as a comment:
//!
//! * a [`Rung`] (or [`Denial`]) whose `claims` list is empty is refused;
//! * a [`Claim`] that no row names is refused.
//!
//! And a claim is not a string in this file: every [`Claim`] carries
//! [`Anchor`]s naming a **published surface** (`docs/book/src/limits.md`,
//! `README.md`, `SECURITY.md`) and a substring that must appear there. Delete
//! the published sentence and the generator stops -- so the table cannot
//! outlive the prose it points at, in either direction.
//!
//! # Why the ladder is parsed out of the helper rather than typed here
//!
//! A second copy of the rung ladder is a second thing to keep true. Instead,
//! [`Ladder::from_source`] reads `crates/vitrin-realm-init/src/landlock.rs` --
//! the bit constants, the base mask, and each `if rung >= N { mask |= RIGHT; }`
//! -- and [`Constants::from_source`] reads the floor and ceiling out of
//! `crates/vitrin-realm-init/src/lib.rs`. Move a right to another rung, or
//! re-tune the floor, and the emitted page changes on the next run, so the
//! checked-in copy is stale and CI is red. The parse is deliberately strict:
//! a shape it does not recognise is an error, never a silently skipped rung.
//!
//! The parsed ladder is then cross-checked against the **measured** mask table
//! pinned in `crates/vitrin-realm-init/src/main.rs`
//! (`the_rung_masks_pin_a_measured_table`, whose values were taken from a real
//! kernel rather than derived). Two independent readings of the same ladder
//! must agree, or nothing is emitted.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// The checked-in page this module owns, relative to the workspace root.
pub const PAGE_PATH: &str = "docs/book/src/isolation-matrix.md";

/// Published surfaces an [`Anchor`] may name.
const LIMITS: &str = "docs/book/src/limits.md";
const README: &str = "README.md";
const SECURITY: &str = "SECURITY.md";

/// Sources the ladder and the constants are read out of.
const LANDLOCK_RS: &str = "crates/vitrin-realm-init/src/landlock.rs";
const REALM_INIT_LIB_RS: &str = "crates/vitrin-realm-init/src/lib.rs";
const REALM_INIT_MAIN_RS: &str = "crates/vitrin-realm-init/src/main.rs";
const ISOLATION_RS: &str = "crates/vitrin-core/src/spawn/isolation.rs";
/// The integration harness, which keeps a **Python copy** of both constants
/// so a mock-free gate can assert the rung a real realm obtained against the
/// floor this build declares. Loaded here because that copy is the one thing
/// on the ladder no Rust check could reach.
const HARNESS_PY: &str = "tests/integration/harness.py";

/// The two claims D-043 (2026-08-19) put on the rows below the floor. Exactly
/// one belongs on each such row, and which one is decided by
/// [`Rung::behavioural_tests`] rather than by whoever last edited the corpus --
/// see step (7b) of [`render`].
const EXERCISED_CLAIM: &str = "sub-floor-rungs-hold-the-dial-not-the-floor";
const UNEXERCISED_CLAIM: &str = "sub-floor-rungs-are-not-all-exercised";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Which part of the enforced domain a rung moves.
///
/// The distinction decides whether `--landlock=abi:N` can *simulate* the
/// rung's absence on a modern kernel, which is the difference between a
/// measurable statement and a prose one -- see [`Axis::mask_capped`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// `handled_access_fs`: an access-mask bit.
    HandledAccessFs,
    /// `scoped`, the second field of `struct landlock_ruleset_attr`.
    Scoped,
    /// `landlock_restrict_self` **flags** -- not an access right at all.
    RestrictSelfFlags,
    /// `handled_access_net`, which this build leaves zero.
    HandledAccessNet,
    /// A rung above this build's ceiling. Nothing here knows what it buys, and
    /// this variant refuses to guess.
    NotKnownToThisBuild,
}

impl Axis {
    fn cell(self) -> &'static str {
        match self {
            Axis::HandledAccessFs => "`handled_access_fs`",
            Axis::Scoped => "`scoped`",
            Axis::RestrictSelfFlags => "`landlock_restrict_self` flags",
            Axis::HandledAccessNet => "`handled_access_net`",
            Axis::NotKnownToThisBuild => "not known to this build",
        }
    }

    /// Whether `--landlock=abi:N` can reproduce a kernel without this rung.
    ///
    /// The cap sets `handled_access_fs` and `scoped` at ruleset-creation time,
    /// so those two are reproducible. The flags word and the network field are
    /// **not** capped by it -- and since a shipped session asks for neither,
    /// there is nothing for a cap to take away.
    fn mask_capped(self) -> bool {
        matches!(self, Axis::HandledAccessFs | Axis::Scoped)
    }
}

/// Whether this build asks the kernel for what the rung buys.
#[derive(Clone, Copy)]
pub enum Requested {
    /// It does.
    Yes,
    /// It does not, and this is why -- never left blank, because an
    /// unexplained omission reads as an oversight.
    No(&'static str),
}

impl Requested {
    fn cell(self) -> String {
        match self {
            Requested::Yes => "**yes**".to_string(),
            Requested::No(why) => format!("no — {why}"),
        }
    }
}

/// One published surface, and the substring that must be on it.
///
/// Compared after whitespace normalization (see [`normalize`]), so a needle
/// may span a line wrap in the published prose. The needle is a load-bearing
/// phrase, never a whole paragraph: the three surfaces state the same fact in
/// three registers on purpose.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub surface: &'static str,
    pub needle: &'static str,
}

/// A published claim a table row carries.
pub struct Claim {
    /// Stable id, referenced by rows and used in failure output.
    pub id: &'static str,
    /// The claim in one sentence, as this generator understands it.
    pub says: &'static str,
    /// Where it is published. **Must be non-empty** -- a "claim" nobody
    /// publishes is a sentence in a Rust file.
    pub anchors: &'static [Anchor],
}

/// One rung of the Landlock ABI, as this build meets it.
pub struct Rung {
    pub abi: u32,
    /// The right or facility the rung buys, named exactly.
    pub buys: &'static str,
    pub axis: Axis,
    pub requested: Requested,
    /// The honest half: what a reader would assume the rung buys and it does
    /// not. **Required** -- there is always something to say, and the four
    /// rows this table exists for are all in this column.
    pub not_bought: &'static str,
    /// Claim ids into [`Corpus::claims`]. **Must be non-empty.**
    pub claims: &'static [&'static str],
    /// Tests in `crates/vitrin-realm-init/src/main.rs` that **enter a Landlock
    /// domain at this rung** -- `create_ruleset(N)` then `restrict_self` --
    /// and assert the kernel's own answer: either the outcome of a syscall
    /// made inside the resulting domain, or the kernel's verdict on the
    /// `restrict_self` request itself. A test that builds a rung-N ruleset and
    /// never enters it is **not** one of these; it measures rule acceptance,
    /// which is a weaker fact and is not what the sub-floor claims rest on.
    ///
    /// Empty is a real and common answer, and it is the honest one for every
    /// rung nothing exercises. It is a field rather than a sentence because
    /// the sentence is what went wrong: D-043's first draft published "the
    /// behavioural tests that exercise them" on the rows for rungs 4 and 5,
    /// which no test enters a domain at.
    ///
    /// **Both halves of the binding are resolved, and the second one was
    /// missing at first.** Step (7b) of [`render`] checks that each name here
    /// is a `fn NAME(` in that file *and* that `BEHAVIOURAL_RUNGS` in the same
    /// file declares it entering a domain at **this** rung -- because the
    /// name-only check could not see a name on the wrong row, and a rung-1
    /// test moved onto rung 5's row rendered green. The reverse direction is
    /// checked too, so a rung a test does enter cannot be left off. See
    /// [`behavioural_rungs`].
    pub behavioural_tests: &'static [&'static str],
}

/// One filesystem denial the ruleset contributes that the realm's mount table
/// does **not** already carry.
///
/// This table is short on purpose. Most of what the ruleset refuses, the mount
/// table refuses too; publishing the overlap as though the ruleset earned it
/// would be the flattering direction.
pub struct Denial {
    pub what: &'static str,
    pub why_the_mount_does_not: &'static str,
    /// What has actually been measured about this row -- including "nothing
    /// has", which is the honest answer for the row this table was built for.
    pub measured: &'static str,
    pub claims: &'static [&'static str],
}

/// One statement about a distinct enforced domain, published **verbatim** on
/// the limits page.
///
/// The point of the verbatim rule is item 4 of this task: a later cross-check
/// compares the generated table against the limits page without a human
/// adjudicating a paraphrase. The generator refuses to emit a statement the
/// limits page does not carry byte-for-byte (after whitespace normalization).
pub struct TierStatement {
    /// `T1`, `T2`, ... in ladder order.
    pub id: &'static str,
    pub statement: &'static str,
}

/// A line of shipped source that must still be there for a cell to be true.
///
/// The same device `crates/xtask/src/limits.rs` uses: `means` says what a
/// reader should conclude from the pin holding, so a failure can say what
/// became false rather than only which substring stopped matching.
pub struct CodePin {
    pub path: &'static str,
    pub needle: &'static str,
    pub means: &'static str,
}

/// One answer `vitrind --print-isolation` can give, and what this build does
/// with it.
///
/// **Not a measurement of any kernel.** Which kernels produce which answer is
/// a fact about mainline and about distributions, and this repository has run
/// exactly two machines. Each cell here is a property of this build's own
/// code.
pub struct MachineRow {
    pub print_isolation: &'static str,
    pub what_this_build_does: &'static str,
}

/// Everything the page is rendered from.
pub struct Corpus {
    pub rungs: &'static [Rung],
    pub claims: &'static [Claim],
    pub tiers: &'static [TierStatement],
    pub denials: &'static [Denial],
    pub pins: &'static [CodePin],
    pub machine: &'static [MachineRow],
}

/// The files the generator reads, loaded once.
pub struct Sources {
    limits: String,
    readme: String,
    security: String,
    landlock_rs: String,
    realm_init_lib_rs: String,
    realm_init_main_rs: String,
    isolation_rs: String,
    harness_py: String,
}

impl Sources {
    /// Load every source the generator needs from a workspace root.
    pub fn load(root: &Path) -> Result<Sources> {
        let read = |rel: &str| -> Result<String> {
            let path = root.join(rel);
            fs::read_to_string(&path).with_context(|| {
                format!(
                    "isolation-matrix: reading {} (the generator reads the shipped source and \
                     the published surfaces; it cannot render without them)",
                    path.display()
                )
            })
        };
        Ok(Sources {
            limits: read(LIMITS)?,
            readme: read(README)?,
            security: read(SECURITY)?,
            landlock_rs: read(LANDLOCK_RS)?,
            realm_init_lib_rs: read(REALM_INIT_LIB_RS)?,
            realm_init_main_rs: read(REALM_INIT_MAIN_RS)?,
            isolation_rs: read(ISOLATION_RS)?,
            harness_py: read(HARNESS_PY)?,
        })
    }

    fn surface(&self, path: &str) -> Result<&str> {
        match path {
            LIMITS => Ok(&self.limits),
            README => Ok(&self.readme),
            SECURITY => Ok(&self.security),
            other => bail!(
                "isolation-matrix: {other} is not a published surface this generator knows. Add \
                 it beside LIMITS/README/SECURITY, with a reason -- a surface nobody loads \
                 cannot be checked."
            ),
        }
    }

    fn pinned(&self, path: &str) -> Result<&str> {
        match path {
            LANDLOCK_RS => Ok(&self.landlock_rs),
            REALM_INIT_LIB_RS => Ok(&self.realm_init_lib_rs),
            REALM_INIT_MAIN_RS => Ok(&self.realm_init_main_rs),
            ISOLATION_RS => Ok(&self.isolation_rs),
            other => bail!(
                "isolation-matrix: {other} is not a source this generator loads, so a pin on it \
                 could never fail. Load it in `Sources::load` first."
            ),
        }
    }
}

/// Collapse every run of whitespace to one space, so a needle may span the
/// line wrapping of the prose it is looking for.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Refuse a corpus string that would corrupt the markdown table it lands in.
///
/// A `|` inside a cell is not a rendering blemish: it splits the row into
/// extra columns, so the published table has a column nobody wrote and
/// [`table_rows`] -- which every test here reads the page back with -- sees a
/// different row shape than the one that was emitted.
fn reject_markup(who: &str, field: &str, value: &str) -> Result<()> {
    if value.contains('|') || value.contains('\n') {
        bail!(
            "isolation-matrix: {who}: {field} contains a pipe or a newline, which would corrupt \
             the emitted markdown table (and the parser the tests read it back with). Spell it \
             without the pipe -- `MS_RDONLY`, `MS_NOSUID`, `MS_NODEV` rather than the \
             `|`-joined form. Offending text: {value:?}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading the build's own numbers out of the shipped source
// ---------------------------------------------------------------------------

/// The floor and the ceiling, read from `vitrin-realm-init`'s library.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Constants {
    /// [`vitrin_realm_init::LANDLOCK_MIN_ABI`].
    pub min_abi: u32,
    /// [`vitrin_realm_init::LANDLOCK_BUILD_MAX_RUNG`].
    pub max_rung: u32,
}

impl Constants {
    /// Parse both constants out of the crate that declares them.
    ///
    /// Parsed rather than duplicated here: a second copy of the floor is a
    /// second number to keep true, and the whole posture of this page is that
    /// re-tuning the constant makes the checked-in copy stale.
    pub fn from_source(lib_rs: &str) -> Result<Constants> {
        let min_abi = u32_after(lib_rs, "pub const LANDLOCK_MIN_ABI: u32 = ").ok_or_else(|| {
            anyhow::anyhow!(
                "isolation-matrix: could not read `pub const LANDLOCK_MIN_ABI: u32 = N;` from \
                 {REALM_INIT_LIB_RS}. The floor is what makes every 'below the floor' cell true; \
                 refusing to render rather than guessing it."
            )
        })?;
        let max_rung =
            u32_after(lib_rs, "pub const LANDLOCK_BUILD_MAX_RUNG: u32 = ").ok_or_else(|| {
                anyhow::anyhow!(
                    "isolation-matrix: could not read `pub const LANDLOCK_BUILD_MAX_RUNG: u32 = \
                     N;` from {REALM_INIT_LIB_RS}. The ceiling decides how many rows the ladder \
                     has and which rung a newer kernel is clamped to."
                )
            })?;
        if min_abi == 0 || max_rung == 0 || min_abi > max_rung {
            bail!(
                "isolation-matrix: read floor={min_abi} ceiling={max_rung} from \
                 {REALM_INIT_LIB_RS}, which is not a ladder this page can describe (Landlock ABI \
                 versions start at 1, and a floor above the ceiling would refuse every kernel \
                 including the ones this build can serve)."
            );
        }
        Ok(Constants { min_abi, max_rung })
    }

    /// Hold the integration harness's **Python copy** of these two numbers to
    /// the crate that declares them.
    ///
    /// `tests/integration/harness.py` mirrors both, because
    /// `test_real_confinement.py` asserts a real realm's *obtained* rung
    /// against the declared floor and a Python test cannot read a Rust
    /// `const`. Nothing checked the mirror until now, and the cost was
    /// concrete rather than theoretical: commit e7b5514 lowered the crate's
    /// floor from 7 to 6 for the owner's ABI-6 VPS and left the harness at 7,
    /// so `obtained_rung >= LANDLOCK_MIN_ABI` would have gone RED on exactly
    /// the kernels the change existed to serve -- and green everywhere the
    /// suite is actually run, which is why nobody saw it.
    ///
    /// Checked *here* rather than in a Python test of its own for the reason
    /// this whole module exists: the drift is between two files, so the check
    /// belongs with the thing that already reads one of them and refuses to
    /// publish when the other disagrees.
    pub fn cross_check_harness(&self, harness_py: &str) -> Result<()> {
        for (name, ours) in [
            ("LANDLOCK_MIN_ABI", self.min_abi),
            ("LANDLOCK_BUILD_MAX_RUNG", self.max_rung),
        ] {
            let theirs = u32_after(harness_py, &format!("\n{name} = ")).ok_or_else(|| {
                anyhow::anyhow!(
                    "isolation-matrix: could not read `{name} = N` from {HARNESS_PY}. The \
                     mock-free confinement gate asserts a real realm's obtained rung against \
                     that copy, so a copy this cannot find is a copy nothing holds to \
                     {REALM_INIT_LIB_RS}."
                )
            })?;
            if theirs != ours {
                bail!(
                    "isolation-matrix: {HARNESS_PY} says `{name} = {theirs}` and \
                     {REALM_INIT_LIB_RS} says {ours}. The harness copy is what \
                     `tests/integration/test_real_confinement.py` compares a real realm's \
                     obtained rung against, so this drift does not fail here -- it fails as a \
                     confinement gate going red on whichever kernels sit between the two \
                     numbers, and nowhere else. Update {HARNESS_PY} to {ours}."
                );
            }
        }
        Ok(())
    }
}

/// The declaration in `main.rs` that says which rungs each behavioural test
/// enters a Landlock domain at.
const BEHAVIOURAL_RUNGS_MARKER: &str = "BEHAVIOURAL_RUNGS: &[(&str, &[u32])] = &[";

/// Parse that declaration: test name → the rungs it enters a domain at.
///
/// # Why the rung has to be parsed and not trusted
///
/// [`Rung::behavioural_tests`] used to be checked only for the *existence* of
/// a `fn NAME(` in `main.rs`. That check cannot see a name attached to the
/// wrong row, and the gap was demonstrated rather than imagined: moving
/// `a_realm_can_write_where_it_was_granted_and_nowhere_else` from rung 1's row
/// onto rung 5's left `cargo xtask isolation-matrix` **green** and published
/// "**rung 5** — `a_realm_can_write_where_it_was_granted_and_nowhere_else`" on
/// a page whose own prose says a rung is counted only when a test *enters* a
/// domain at it. The name was a lookup; the rung was still a sentence.
///
/// The other end of the same declaration is held by the tests themselves --
/// each records every rung it hands to `create_ruleset` and asserts the set
/// against its own row before it returns -- so a rung number in that table is
/// wrong only if the test *and* this page are wrong together, which is an edit
/// rather than a drift.
fn behavioural_rungs(main_rs: &str) -> Result<Vec<(String, Vec<u32>)>> {
    let start = main_rs.find(BEHAVIOURAL_RUNGS_MARKER).ok_or_else(|| {
        anyhow::anyhow!(
            "isolation-matrix: could not find `{BEHAVIOURAL_RUNGS_MARKER}` in \
             {REALM_INIT_MAIN_RS}. That table is what binds a behavioural test to the RUNG it \
             enters a domain at; without it this page could only check that a test of some name \
             exists somewhere, which is the check that let a rung-1 test be published on rung \
             5's row."
        )
    })?;
    let rest = &main_rs[start + BEHAVIOURAL_RUNGS_MARKER.len()..];
    let end = rest.find("];").ok_or_else(|| {
        anyhow::anyhow!(
            "isolation-matrix: `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} is not terminated by \
             `];`, so its extent cannot be read. Refusing to parse a table whose end is a guess."
        )
    })?;
    let body = normalize(&rest[..end]);

    let mut out: Vec<(String, Vec<u32>)> = Vec::new();
    let mut cursor = body.as_str();
    while let Some(open) = cursor.find('"') {
        let after = &cursor[open + 1..];
        let close = after.find('"').ok_or_else(|| {
            anyhow::anyhow!(
                "isolation-matrix: a `BEHAVIOURAL_RUNGS` row in {REALM_INIT_MAIN_RS} opens a \
                 test name and never closes it."
            )
        })?;
        let name = after[..close].to_string();
        let tail = &after[close + 1..];
        let list = tail.find("&[").ok_or_else(|| {
            anyhow::anyhow!(
                "isolation-matrix: the `BEHAVIOURAL_RUNGS` row for {name:?} in \
                 {REALM_INIT_MAIN_RS} names no `&[...]` of rungs. A test with no rung says \
                 nothing about which row of this page it belongs on."
            )
        })? + 2;
        let list_end = tail[list..].find(']').ok_or_else(|| {
            anyhow::anyhow!(
                "isolation-matrix: the rung list for {name:?} in {REALM_INIT_MAIN_RS} is not \
                 closed."
            )
        })?;
        let mut rungs = Vec::new();
        for item in tail[list..list + list_end].split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            rungs.push(item.parse::<u32>().with_context(|| {
                format!(
                    "isolation-matrix: the `BEHAVIOURAL_RUNGS` row for {name:?} in \
                     {REALM_INIT_MAIN_RS} lists {item:?}, which is not a rung number"
                )
            })?);
        }
        if rungs.is_empty() {
            bail!(
                "isolation-matrix: the `BEHAVIOURAL_RUNGS` row for {name:?} in \
                 {REALM_INIT_MAIN_RS} lists no rung at all."
            );
        }
        out.push((name, rungs));
        cursor = &tail[list + list_end + 1..];
    }
    if out.is_empty() {
        bail!(
            "isolation-matrix: `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} parsed to no rows. An \
             empty table would make every cross-check below vacuously true, which is the shape \
             this repository calls a check that stopped checking."
        );
    }
    Ok(out)
}

/// The first `u32` literal following `prefix`.
fn u32_after(haystack: &str, prefix: &str) -> Option<u32> {
    let rest = haystack.split_once(prefix)?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The cumulative access masks this build asks for, per rung.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ladder {
    /// `handled_access_fs` at rung `i + 1`.
    pub fs: Vec<u64>,
    /// `scoped` at rung `i + 1`.
    pub scoped: Vec<u64>,
    /// The rung at which each right is first handled, in ladder order.
    pub introduced: Vec<(u32, String)>,
}

impl Ladder {
    /// Parse the ladder out of the helper's own `handled_access_fs`/`scoped`.
    ///
    /// Deliberately strict about the shapes it accepts. A rung guard written
    /// some other way is an **error** here, never a rung silently dropped from
    /// the page: a ladder missing a row reads exactly like a ladder that never
    /// had one.
    pub fn from_source(landlock_rs: &str, max_rung: u32) -> Result<Ladder> {
        let bits = bit_constants(landlock_rs);
        let fs_body = fn_body(landlock_rs, "fn handled_access_fs(rung: u32) -> u64 {")
            .context("isolation-matrix: locating `handled_access_fs` in the helper")?;
        let scoped_body = fn_body(landlock_rs, "fn scoped(rung: u32) -> u64 {")
            .context("isolation-matrix: locating `scoped` in the helper")?;

        // The rung-1 mask: the initializer of `let mut mask = ...;`.
        let base_expr = fs_body
            .split_once("let mut mask = ")
            .and_then(|(_, rest)| rest.split_once(';'))
            .map(|(expr, _)| expr)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "isolation-matrix: `handled_access_fs` no longer starts with `let mut mask = \
                     ...;`, so the rung-1 mask cannot be read. Refusing to render a ladder whose \
                     bottom row is a guess."
                )
            })?;
        let base = or_expression(base_expr, &bits, "the rung-1 mask")?;

        // Each `if rung >= N { mask |= RIGHT; }`.
        let mut movers: Vec<(u32, String, u64)> = Vec::new();
        for chunk in fs_body.split("if rung >= ").skip(1) {
            let rung: u32 = chunk
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .map_err(|_| {
                    anyhow::anyhow!(
                        "isolation-matrix: an `if rung >= ...` guard in `handled_access_fs` is \
                         not followed by a rung number."
                    )
                })?;
            let name = chunk
                .split_once("mask |= ")
                .and_then(|(_, rest)| rest.split_once(';'))
                .map(|(name, _)| name.trim().to_string())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "isolation-matrix: the `if rung >= {rung}` guard in `handled_access_fs` \
                         does not add exactly one named right (`mask |= RIGHT;`). This parser \
                         refuses shapes it cannot read rather than dropping the rung."
                    )
                })?;
            let bit = *bits.get(name.as_str()).ok_or_else(|| {
                anyhow::anyhow!(
                    "isolation-matrix: rung {rung} handles `{name}`, which is not declared as \
                     `const {name}: u64 = 1 << K;` in {LANDLOCK_RS}."
                )
            })?;
            movers.push((rung, name, bit));
        }

        // `scoped` moves at exactly one rung, and the page's domain grouping
        // depends on knowing which.
        let scoped_rung: u32 = scoped_body
            .split_once("if rung >= ")
            .and_then(|(_, rest)| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "isolation-matrix: `scoped` no longer names the rung it starts at with an \
                     `if rung >= N` guard."
                )
            })?;
        let scoped_expr = scoped_body
            .split_once("if rung >= ")
            .and_then(|(_, rest)| rest.split_once('{'))
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(expr, _)| expr)
            .ok_or_else(|| {
                anyhow::anyhow!("isolation-matrix: `scoped`'s guarded expression cannot be read.")
            })?;
        let scoped_mask = or_expression(scoped_expr, &bits, "the `scoped` mask")?;

        let mut fs = Vec::new();
        let mut scoped = Vec::new();
        let mut introduced: Vec<(u32, String)> = Vec::new();
        for rung in 1..=max_rung {
            let mut mask = base;
            for (at, name, bit) in &movers {
                if rung >= *at {
                    mask |= bit;
                    if rung == *at {
                        introduced.push((rung, name.clone()));
                    }
                }
            }
            fs.push(mask);
            scoped.push(if rung >= scoped_rung { scoped_mask } else { 0 });
        }
        if scoped_rung <= max_rung {
            introduced.push((scoped_rung, "the `scoped` field".to_string()));
        }
        introduced.sort_by_key(|(rung, _)| *rung);
        Ok(Ladder {
            fs,
            scoped,
            introduced,
        })
    }

    /// Cross-check against the **measured** table pinned in the helper's own
    /// test suite.
    ///
    /// The values in `the_rung_masks_pin_a_measured_table` came from a kernel
    /// rather than from a derivation, so two independent readings agreeing is
    /// worth more than either alone. Disagreement stops the render.
    pub fn cross_check(&self, main_rs: &str) -> Result<()> {
        let measured = measured_mask_table(main_rs)?;
        if measured != self.fs {
            bail!(
                "isolation-matrix: the ladder parsed from {LANDLOCK_RS} disagrees with the \
                 MEASURED mask table pinned in {REALM_INIT_MAIN_RS} \
                 (`the_rung_masks_pin_a_measured_table`).\n  parsed:   {:x?}\n  measured: \
                 {:x?}\nOne of the two moved without the other. Nothing is emitted until they \
                 agree, because the page prints these numbers as facts about a kernel.",
                self.fs,
                measured
            );
        }
        Ok(())
    }
}

/// Every `const NAME: u64 = 1 << K;` in the helper.
fn bit_constants(src: &str) -> std::collections::BTreeMap<&str, u64> {
    let mut out = std::collections::BTreeMap::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("const ") else {
            continue;
        };
        let Some((name, rest)) = rest.split_once(": u64 = 1 << ") else {
            continue;
        };
        let Some((shift, _)) = rest.split_once(';') else {
            continue;
        };
        if let Ok(shift) = shift.trim().parse::<u32>() {
            out.insert(name.trim(), 1u64 << shift);
        }
    }
    out
}

/// The body of a function, from its opening brace to the matching close.
fn fn_body<'a>(src: &'a str, signature: &str) -> Result<&'a str> {
    let start = src
        .find(signature)
        .ok_or_else(|| anyhow::anyhow!("`{signature}` is no longer in the source"))?
        + signature.len();
    let rest = &src[start..];
    let mut depth = 1usize;
    for (i, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&rest[..i]);
                }
            }
            _ => {}
        }
    }
    bail!("`{signature}` has no matching closing brace")
}

/// Fold `A | B | C` into a mask, refusing any name that is not a declared bit.
fn or_expression(
    expr: &str,
    bits: &std::collections::BTreeMap<&str, u64>,
    what: &str,
) -> Result<u64> {
    let mut mask = 0u64;
    for name in expr.split('|') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let bit = bits.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "isolation-matrix: {what} names `{name}`, which is not declared as `const \
                 {name}: u64 = 1 << K;` in {LANDLOCK_RS}."
            )
        })?;
        mask |= bit;
    }
    Ok(mask)
}

/// The measured `handled_access_fs` values pinned in the helper's test suite.
fn measured_mask_table(main_rs: &str) -> Result<Vec<u64>> {
    let anchor = "fn the_rung_masks_pin_a_measured_table()";
    let body = fn_body(main_rs, &format!("{anchor} {{")).with_context(|| {
        format!("isolation-matrix: locating `{anchor}` in {REALM_INIT_MAIN_RS}")
    })?;
    let vec_body = body
        .split_once("vec![")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(inner, _)| inner)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "isolation-matrix: `{anchor}` no longer pins its masks as a `vec![...]` literal, \
                 so the measured table cannot be read back."
            )
        })?;
    let mut out = Vec::new();
    for item in vec_body.split(',') {
        let literal = item
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("0x"))
            .unwrap_or("");
        if literal.is_empty() {
            continue;
        }
        let hex = literal.trim_start_matches("0x");
        let value = u64::from_str_radix(hex, 16).map_err(|_| {
            anyhow::anyhow!("isolation-matrix: `{literal}` in `{anchor}` is not a hex mask")
        })?;
        out.push(value);
    }
    if out.is_empty() {
        bail!("isolation-matrix: `{anchor}` yielded no masks");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Domains: the grouping is DERIVED, never typed
// ---------------------------------------------------------------------------

/// One distinct enforced domain, and the rungs that produce it.
///
/// "Nine rung numbers name six different domains" is computed here from the
/// parsed ladder rather than asserted, so the sentence cannot go stale against
/// the code that makes it true.
pub struct Domain {
    pub index: usize,
    pub rungs: Vec<u32>,
    pub fs: u64,
    pub scoped: u64,
    /// `landlock_restrict_self` flags in a **shipped** run. Zero at every
    /// rung; see [`SHIPPED_FLAGS_ARE_ZERO`].
    pub flags: u32,
}

/// The Landlock ABI this repository's development box reported, 2026-08-15.
///
/// A **measurement**, deliberately not derived from anything: it is one number
/// one machine answered on one date. It is not the ceiling
/// (`LANDLOCK_BUILD_MAX_RUNG`), which happens to equal it today and is a build
/// decision.
const MEASURED_DEV_BOX_ABI: u32 = 9;

/// The Landlock ABI the runner this repository's CI uses reported, 2026-08-14.
///
/// Also a **measurement**, and deliberately not [`Constants::min_abi`]. Until
/// 2026-08-16 the floor was *chosen* to equal this number; it is now 6, one
/// rung below it, so the two are no longer even numerically related -- and
/// rendering them from one constant would still be wrong for the original
/// reason, that it would turn a choice into a tautology.
///
/// **This line is the only place in the repository the number is written down
/// as a fact about the RUNNER, and that part is a transcription rather than an
/// artefact.** It was read out of a CI job log -- the `What confinement this
/// runner actually grants` diagnostic step, which runs `--print-isolation` on
/// an unmodified runner and archives nothing. GitHub expires job logs, so the
/// runner half cannot be re-derived from the tree; re-running the job is the
/// only way to re-take it.
///
/// What #281 *did* close, on 2026-08-16: the runner's own kernel
/// (`6.17.0-1020-azure`) is now booted here under QEMU, and
/// `tests/kernel-matrix/rows/ubuntu-azure-6.17.row` is a checked-in artefact
/// reporting `landlock.abi=7` from the shipped binary. That corroborates the
/// number without replacing this constant, because it is a fact about the
/// *kernel*: the same boot reads `apparmor_restrict_unprivileged_userns=0`
/// where the runner reads `1`, so the two rows agree on the ABI and disagree
/// on policy. `docs/book/src/isolation-kernels.md` states that distinction as
/// the reason it is a kernel page and not a distribution page.
const MEASURED_CI_RUNNER_ABI: u32 = 7;

/// The flags word every shipped session passes to `landlock_restrict_self`.
///
/// Zero, at every rung. The one thing that moves it is the
/// `VITRIN_LANDLOCK_AUDIT` diagnostic, which the core forwards only when its
/// own environment carries it -- and which changes what the kernel *logs*,
/// never what it permits. Pinned in [`PINS`].
const SHIPPED_FLAGS_ARE_ZERO: u32 = 0;

/// Group the ladder into distinct enforced domains, in rung order.
fn domains(ladder: &Ladder) -> Vec<Domain> {
    let mut out: Vec<Domain> = Vec::new();
    for (i, (&fs, &scoped)) in ladder.fs.iter().zip(ladder.scoped.iter()).enumerate() {
        let rung = i as u32 + 1;
        match out.last_mut() {
            Some(last) if last.fs == fs && last.scoped == scoped => last.rungs.push(rung),
            _ => out.push(Domain {
                index: out.len() + 1,
                rungs: vec![rung],
                fs,
                scoped,
                flags: SHIPPED_FLAGS_ARE_ZERO,
            }),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the page, or refuse.
///
/// The refusals are the deliverable as much as the page is. In order:
///
/// 1. every code pin must still be in the source it names;
/// 2. the floor and ceiling must be readable from the helper's library;
/// 3. the ladder must parse, and must agree with the measured table;
/// 4. the ladder must cover exactly the rungs the corpus has rows for;
/// 5. **every row must name at least one claim**;
/// 6. **every claim must be named by at least one row**;
/// 7. every claim must have at least one anchor, and every anchor's needle
///    must be on its published surface;
/// 8. there must be exactly one tier statement per derived domain, and each
///    must appear **verbatim** on the limits page.
pub fn render(corpus: &Corpus, sources: &Sources) -> Result<String> {
    // (1) The code this page describes.
    for pin in corpus.pins {
        let src = sources.pinned(pin.path)?;
        if !normalize(src).contains(&normalize(pin.needle)) {
            bail!(
                "isolation-matrix: PIN BROKEN in {}: {:?} is no longer there.\n  what it meant: \
                 {}\nThe page is not emitted, because a cell it renders is now describing code \
                 that does not exist.",
                pin.path,
                pin.needle,
                pin.means
            );
        }
    }

    // (2) and (3): the build's own numbers, and the ladder they bound.
    let constants = Constants::from_source(&sources.realm_init_lib_rs)?;
    constants.cross_check_harness(&sources.harness_py)?;
    let ladder = Ladder::from_source(&sources.landlock_rs, constants.max_rung)?;
    ladder.cross_check(&sources.realm_init_main_rs)?;

    // (4) One row per rung this build can ask for, plus exactly one row for
    // the first rung above the ceiling -- the clamp row. A corpus that has
    // drifted from the parsed ladder is refused rather than rendered short.
    let expected: Vec<u32> = (1..=constants.max_rung + 1).collect();
    let actual: Vec<u32> = corpus.rungs.iter().map(|r| r.abi).collect();
    if actual != expected {
        bail!(
            "isolation-matrix: the rung rows are {actual:?} and the ladder parsed from \
             {LANDLOCK_RS} (ceiling {}) needs {expected:?}. A ladder row set that does not match \
             the code is the exact staleness this page exists to prevent, so nothing is emitted.",
            constants.max_rung
        );
    }

    // (4b) No cell may carry a pipe or a newline. A `|` inside a cell splits
    // the row into extra columns, which corrupts the emitted table AND the
    // parser every test here reads it back with -- so it is refused at the
    // corpus rather than shipped as a table that renders wrong. (This is not
    // hypothetical: the first draft of the `/etc` denial row said
    // `MS_RDONLY|MS_NOSUID|MS_NODEV` and split its own row into seven cells.)
    for rung in corpus.rungs {
        let abi = rung.abi;
        for (field, value) in [("buys", rung.buys), ("not_bought", rung.not_bought)] {
            reject_markup(&format!("ABI rung {abi}"), field, value)?;
        }
        if let Requested::No(why) = rung.requested {
            reject_markup(&format!("ABI rung {abi}"), "requested", why)?;
        }
    }
    for denial in corpus.denials {
        for (field, value) in [
            ("what", denial.what),
            ("why_the_mount_does_not", denial.why_the_mount_does_not),
            ("measured", denial.measured),
        ] {
            reject_markup("a denial row", field, value)?;
        }
    }
    for tier in corpus.tiers {
        reject_markup(tier.id, "statement", tier.statement)?;
    }
    for claim in corpus.claims {
        reject_markup(claim.id, "says", claim.says)?;
        for anchor in claim.anchors {
            reject_markup(claim.id, "needle", anchor.needle)?;
        }
    }
    for row in corpus.machine {
        reject_markup("a machine row", "print_isolation", row.print_isolation)?;
        reject_markup(
            "a machine row",
            "what_this_build_does",
            row.what_this_build_does,
        )?;
    }

    // (5) A row with a right and no claim fails the generator (issue #187).
    for rung in corpus.rungs {
        if rung.claims.is_empty() {
            bail!(
                "isolation-matrix: ABI rung {} names the right {:?} and carries NO published \
                 claim. Issue #187: \"a row with a right and no claim, or a claim with no row, \
                 fails the generator.\" Either publish what this rung is worth on \
                 {LIMITS}/{README}/{SECURITY} and add the claim here, or delete the row.",
                rung.abi,
                rung.buys
            );
        }
        if rung.not_bought.trim().is_empty() {
            bail!(
                "isolation-matrix: ABI rung {} has an empty \"what it does not buy\" cell. That \
                 column is the reason this table exists; a blank one publishes a rung as pure \
                 gain.",
                rung.abi
            );
        }
    }

    for denial in corpus.denials {
        if denial.claims.is_empty() {
            bail!(
                "isolation-matrix: the denial {:?} carries no published claim. Same rule as a \
                 rung row.",
                denial.what
            );
        }
    }

    // (6) A claim with no row fails the generator, in the other direction.
    let mut named: Vec<&str> = Vec::new();
    for rung in corpus.rungs {
        named.extend(rung.claims.iter().copied());
    }
    for denial in corpus.denials {
        named.extend(denial.claims.iter().copied());
    }
    for id in &named {
        if !corpus.claims.iter().any(|c| c.id == *id) {
            bail!(
                "isolation-matrix: a row names the claim {id:?}, which is not in the claim \
                 table. A claim id that resolves to nothing renders as a citation to nowhere."
            );
        }
    }
    for claim in corpus.claims {
        if !named.contains(&claim.id) {
            bail!(
                "isolation-matrix: the claim {:?} is carried by NO row. Issue #187: \"a row with \
                 a right and no claim, or a claim with no row, fails the generator.\" Either \
                 give it a row or stop publishing it here.",
                claim.id
            );
        }
    }

    // (7) Every claim is anchored to a published sentence that still exists.
    for claim in corpus.claims {
        if claim.anchors.is_empty() {
            bail!(
                "isolation-matrix: the claim {:?} names no published surface. A claim nobody \
                 publishes is a sentence in a Rust file.",
                claim.id
            );
        }
        for anchor in claim.anchors {
            let surface = sources.surface(anchor.surface)?;
            if !normalize(surface).contains(&normalize(anchor.needle)) {
                bail!(
                    "isolation-matrix: the claim {:?} is anchored to {} at {:?}, and that text \
                     is no longer published there. The table may not cite a sentence that has \
                     been deleted or reworded, so nothing is emitted.",
                    claim.id,
                    anchor.surface,
                    anchor.needle
                );
            }
        }
    }

    // (7b) Which sub-floor rungs a behavioural test actually enters a domain
    // at, held against the two claims that talk about it (D-043). The first
    // draft of that decision published "the behavioural tests that exercise
    // them" on the rows for rungs 4 and 5, where nothing does -- a sentence a
    // human had to remember to keep true, on the rows it was least true of.
    // Here it is a lookup instead: `behavioural_tests` decides which claim a
    // sub-floor row may carry, every name in it is resolved against the file
    // it names, and every combination that would publish a false row refuses
    // to render.
    //
    // And the name is not the whole of it. Resolving `fn NAME(` proves a test
    // of that name exists; it says nothing about the RUNG that test enters a
    // domain at, which is what every sentence on the emitted page is about.
    // That gap was demonstrated: a rung-1 test moved onto rung 5's row
    // rendered green. So the rung is resolved too, against
    // `BEHAVIOURAL_RUNGS` in the same file -- a table the tests themselves
    // assert against at runtime, so neither end of the binding is a sentence.
    let declared = behavioural_rungs(&sources.realm_init_main_rs)?;
    for (name, _) in &declared {
        if !sources.realm_init_main_rs.contains(&format!("fn {name}(")) {
            bail!(
                "isolation-matrix: `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} declares \
                 {name:?}, and there is no `fn {name}(` in that file. A row for a test that no \
                 longer exists is never executed, so nothing else would ever notice."
            );
        }
    }
    for rung in corpus.rungs {
        for name in rung.behavioural_tests {
            if !sources.realm_init_main_rs.contains(&format!("fn {name}(")) {
                bail!(
                    "isolation-matrix: ABI rung {} names the behavioural test {name:?}, and \
                     there is no `fn {name}(` in {REALM_INIT_MAIN_RS}. A renamed or deleted test \
                     leaves the page claiming a rung is exercised by something that is not \
                     there, so nothing is emitted.",
                    rung.abi
                );
            }
            let Some((_, enters)) = declared.iter().find(|(n, _)| n == name) else {
                bail!(
                    "isolation-matrix: ABI rung {} names the behavioural test {name:?}, and \
                     `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} does not declare it. This page \
                     says a rung is counted only when a test ENTERS a domain at it, so a test \
                     that has not declared the rungs it enters cannot be counted on any row.",
                    rung.abi
                );
            };
            if !enters.contains(&rung.abi) {
                bail!(
                    "isolation-matrix: ABI rung {} names the behavioural test {name:?}, which \
                     `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} says enters a Landlock domain \
                     at rung(s) {enters:?} -- not at this one. Moving a test's NAME onto another \
                     row used to render green and publish a rung as exercised by a test that \
                     never enters it; the rung is now resolved as well as the name.",
                    rung.abi
                );
            }
        }
    }
    // And the same binding read the other way, so the page cannot UNDER-report
    // either: a test that enters a domain at a rung this page carries a row
    // for must be listed on that row.
    for (name, enters) in &declared {
        for abi in enters {
            let Some(row) = corpus.rungs.iter().find(|r| r.abi == *abi) else {
                bail!(
                    "isolation-matrix: `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} says \
                     {name:?} enters a Landlock domain at rung {abi}, and this page has no row \
                     for that rung. Either the ladder lost a row or the test is measuring a \
                     rung nothing publishes."
                );
            };
            if !row.behavioural_tests.contains(&name.as_str()) {
                bail!(
                    "isolation-matrix: `BEHAVIOURAL_RUNGS` in {REALM_INIT_MAIN_RS} says \
                     {name:?} enters a Landlock domain at rung {abi}, and rung {abi}'s row does \
                     not list it. The page would report that rung as exercised by less than it \
                     is -- and, below the floor, could report it as exercised by nothing."
                );
            }
        }
    }
    for rung in corpus.rungs {
        let exercised = !rung.behavioural_tests.is_empty();
        let says_exercised = rung.claims.contains(&EXERCISED_CLAIM);
        let says_unexercised = rung.claims.contains(&UNEXERCISED_CLAIM);
        if rung.abi >= constants.min_abi && (says_exercised || says_unexercised) {
            bail!(
                "isolation-matrix: ABI rung {} is at or above the floor of {} and carries a \
                 SUB-FLOOR claim. Both claims in that pair say \"this rung is below the floor\", \
                 which this row would publish as false.",
                rung.abi,
                constants.min_abi
            );
        }
        if rung.abi < constants.min_abi {
            if says_exercised == says_unexercised {
                bail!(
                    "isolation-matrix: ABI rung {} is below the floor of {} and carries {} of \
                     {EXERCISED_CLAIM:?} / {UNEXERCISED_CLAIM:?}. Exactly one is required: \
                     D-043 is the published answer to \"what is a test at an unreachable rung \
                     evidence about\", and a sub-floor row that names neither, or both, either \
                     has no answer or two.",
                    rung.abi,
                    constants.min_abi,
                    if says_exercised { "BOTH" } else { "NEITHER" }
                );
            }
            if says_exercised != exercised {
                bail!(
                    "isolation-matrix: ABI rung {} lists {} behavioural test(s) and carries \
                     {:?}. The claims are not interchangeable -- {EXERCISED_CLAIM:?} says a test \
                     enters a domain at this rung and {UNEXERCISED_CLAIM:?} says none does -- so \
                     the row is refused rather than published against its own corpus.",
                    rung.abi,
                    rung.behavioural_tests.len(),
                    if says_exercised {
                        EXERCISED_CLAIM
                    } else {
                        UNEXERCISED_CLAIM
                    }
                );
            }
        }
    }

    // (7c) And the tally itself, on the surface a security reader meets. The
    // two claims above hold WHICH rows carry an explanation; this holds the
    // NUMBER, which is the half that rotted last time -- a count corrected in
    // one place and left standing in another. It is spelled from the corpus,
    // so adding or dropping a sub-floor test changes the required sentence and
    // the generator names the new one.
    {
        let tally = sub_floor_tally(corpus, &constants);
        if !normalize(&sources.limits).contains(&normalize(&tally)) {
            bail!(
                "isolation-matrix: {LIMITS} does not carry this corpus's sub-floor tally. It \
                 must say, in these words:\n  {tally:?}\nThe count of exercised rungs is \
                 published there and computed here, so the two are held together rather than \
                 kept in step by hand."
            );
        }
    }

    // (7d) The helper's own doc comment counts how many rungs leave
    // `handled_access_fs` untouched, and that count is derivable from the very
    // function this module already parses. It was typed, and it had drifted:
    // "Five of the ten rungs do not move it at all (4, 6, 7, 8, 10)" counted a
    // clamp row above the ceiling as a rung and disagreed with the "nine
    // rungs" every other surface publishes. Held here rather than reworded,
    // because rewording is what it had already had.
    {
        let required = flat_rungs_sentence(&ladder, &constants);
        if !normalize(&sources.landlock_rs).contains(&normalize(&required)) {
            bail!(
                "isolation-matrix: {LANDLOCK_RS}'s `handled_access_fs` doc comment must carry \
                 this sentence, which is computed from the ladder parsed out of that same \
                 function:\n  {required:?}\nA hand-typed count of the flat rungs is a count \
                 nothing holds -- and the one that was there counted the clamp row above the \
                 ceiling as a rung."
            );
        }
    }

    // (8) One tier statement per derived domain, published verbatim.
    let domains = domains(&ladder);
    if corpus.tiers.len() != domains.len() {
        bail!(
            "isolation-matrix: the ladder parsed from {LANDLOCK_RS} has {} distinct enforced \
             domains and this corpus carries {} tier statements. One statement per domain, in \
             ladder order -- a domain with no statement is a tier this page would publish \
             without saying what it is.",
            domains.len(),
            corpus.tiers.len()
        );
    }
    let normalized_limits = normalize(&sources.limits);
    for tier in corpus.tiers {
        if !normalized_limits.contains(&normalize(tier.statement)) {
            bail!(
                "isolation-matrix: tier {} is not published VERBATIM on {LIMITS}:\n  \
                 {:?}\nItem 4 of this task requires the per-tier statement to be comparable \
                 without a human adjudicating a paraphrase, so the two copies must be \
                 byte-identical after whitespace normalization.",
                tier.id,
                tier.statement
            );
        }
    }

    let mut p = String::new();
    render_preamble(&mut p, &constants, &ladder, &domains);
    render_machine_table(&mut p, corpus, &constants);
    render_rung_table(&mut p, corpus, &ladder, &constants);
    render_not_bought_table(&mut p, corpus, &domains);
    render_domain_table(&mut p, corpus, &domains);
    render_denial_table(&mut p, corpus);
    render_claim_table(&mut p, corpus);
    render_not_here(&mut p, &constants);
    render_runbook(&mut p);

    verify_emitted_rows(&p)?;
    Ok(p)
}

/// Words that record nothing. Borrowed verbatim from `session_matrix.rs`, for
/// the same reason: a cell that says "ok" publishes a row's existence and
/// nothing else.
const BARE_PASS_WORDS: &[&str] = &["", "-", "n/a", "na", "pass", "passed", "ok", "works", "yes"];

/// Re-read the emitted bytes and refuse to hand back a page carrying a cell
/// that records nothing.
///
/// Catches the *rendering* bug rather than the *data* bug: a column added to a
/// table but not to every row, a format string that drops a field, an enum
/// variant that renders empty.
fn verify_emitted_rows(page: &str) -> Result<()> {
    for row in table_rows(page) {
        for (i, cell) in row.cells.iter().enumerate() {
            if BARE_PASS_WORDS.contains(&cell.trim().to_ascii_lowercase().as_str()) {
                bail!(
                    "isolation-matrix: rendered a cell that records nothing (column {i} of a row \
                     under {:?}): {}",
                    row.section,
                    row.line
                );
            }
        }
    }
    Ok(())
}

/// One data row of one markdown table on the rendered page.
#[derive(Clone, Debug)]
pub struct TableRow {
    pub section: String,
    pub cells: Vec<String>,
    pub line: String,
}

fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed
            .trim_matches('|')
            .split('|')
            .all(|c| !c.trim().is_empty() && c.trim().chars().all(|ch| ch == '-' || ch == ':'))
}

/// Every **data** row of every markdown table on `page`.
///
/// The tests assert against these -- the bytes the page actually carries --
/// rather than against the corpus that produced them.
pub fn table_rows(page: &str) -> Vec<TableRow> {
    let lines: Vec<&str> = page.lines().collect();
    let mut section = String::new();
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            section = rest.trim().to_string();
            continue;
        }
        if !trimmed.starts_with('|') || is_separator(line) {
            continue;
        }
        if lines.get(i + 1).is_some_and(|next| is_separator(next)) {
            continue;
        }
        rows.push(TableRow {
            section: section.clone(),
            cells: line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect(),
            line: (*line).to_string(),
        });
    }
    rows
}

fn render_preamble(p: &mut String, c: &Constants, ladder: &Ladder, domains: &[Domain]) {
    p.push_str(
        "<!-- GENERATED FILE -- DO NOT EDIT.\n\
         \n\
         Produced by `cargo xtask isolation-matrix` from the corpus in\n\
         crates/xtask/src/isolation_matrix.rs, the Landlock ladder parsed out of\n\
         crates/vitrin-realm-init/src/landlock.rs, and the floor and ceiling read\n\
         from crates/vitrin-realm-init/src/lib.rs. `cargo xtask isolation-matrix\n\
         --check` re-renders and compares byte-for-byte, and CI runs it, so a hand\n\
         edit to this file is a red build -- and so is moving a right to another\n\
         rung, or re-tuning the floor, without regenerating.\n\
         -->\n\n",
    );
    p.push_str("# The Landlock ABI matrix\n\n");
    p.push_str(&format!(
        "PRD §20 says Landlock coverage is kernel-dependent. This page is the table that\n\
         sentence is checkable against: **what this build requires of a kernel's Landlock,\n\
         and what each rung of the ABI buys the ruleset on the way.**\n\n\
         This build's two numbers, both read out of the source that declares them rather\n\
         than typed here:\n\n\
         - **floor — `build.landlock_min_abi` = {min}.** A kernel reporting a lower\n  \
         Landlock ABI is refused at startup. It is not confined at a weaker rung.\n\
         - **ceiling — `build.landlock_max_rung` = {max}.** A kernel reporting a higher\n  \
         ABI gets a rung-{max} ruleset, journaled as\n  \
         `isolation.landlock.clamped_by_build`.\n\n\
         Both are printed by `vitrind --print-floor`.\n\n",
        min = c.min_abi,
        max = c.max_rung,
    ));
    p.push_str(&format!(
        "The ladder below has **{rungs} rung numbers naming {domains} distinct enforced\n\
         domains** — that count is computed from the parsed ladder, not asserted, and the\n\
         rungs that collapse into one domain are named in the domain table.\n\n",
        rungs = ladder.fs.len(),
        domains = domains.len(),
    ));
    p.push_str(&format!(
        "## What this page is a fact about\n\n\
         **This build, not your kernel.** Nothing here probes anything. The generator runs\n\
         on a laptop and on a CI runner and must emit the same bytes on both, so it reads\n\
         the repository and never the machine — and the two machines this repository has\n\
         actually run report different Landlock ABIs (the development box {MEASURED_DEV_BOX_ABI},\n\
         the CI runner {MEASURED_CI_RUNNER_ABI}), which a probing generator could not have\n\
         reconciled into one checked-in page.\n\n\
         The machine half is a command you run:\n\n\
         ```console\n\
         $ vitrind --print-isolation | grep landlock\n\
         $ vitrind --print-floor | grep landlock\n\
         ```\n\n\
         The first prints what your kernel answers; the second prints the two build\n\
         numbers above. The next table says what this build does with each possible\n\
         answer. **Which kernel releases produce which answer is not stated anywhere on\n\
         this page, because it was not measured here** — that mapping is a fact about\n\
         mainline and about distributions, and this page probes neither. It is measured\n\
         on a page of its own: [which kernels this build starts on](isolation-kernels.md),\n\
         from boot rows checked in under `tests/kernel-matrix/rows/`.\n\n",
    ));
}

fn render_machine_table(p: &mut String, corpus: &Corpus, c: &Constants) {
    p.push_str("## Read your own kernel against it\n\n");
    p.push_str(&format!(
        "Every cell below is a property of this build's own code — `spawn::isolation`'s\n\
         `Report::mechanism` for the verdict and `landlock::apply_with` for the second\n\
         refusal — with the floor at {} and the ceiling at {}.\n\n",
        c.min_abi, c.max_rung
    ));
    p.push_str("| `vitrind --print-isolation` says | what this build does |\n|---|---|\n");
    for row in corpus.machine {
        p.push_str(&format!(
            "| {} | {} |\n",
            row.print_isolation, row.what_this_build_does
        ));
    }
    p.push('\n');
}

fn render_rung_table(p: &mut String, corpus: &Corpus, ladder: &Ladder, c: &Constants) {
    p.push_str("## The ladder, one row per ABI rung\n\n");
    p.push_str(
        "`what it buys` is the right or facility the rung adds. `axis` is which field of\n\
         the request it moves, and it decides whether `--landlock=abi:N` can *simulate* a\n\
         kernel without the rung: the cap sets `handled_access_fs` and `scoped`, so it can;\n\
         it does not set the `landlock_restrict_self` flags word or `handled_access_net`,\n\
         so for those rungs there is nothing for a cap to take away.\n\n",
    );
    p.push_str(
        "| ABI | what it buys | axis | capping simulates it | this build asks for it | \
         `handled_access_fs` | `scoped` | vs. this build's floor | published claim |\n\
         |---|---|---|---|---|---|---|---|---|\n",
    );
    for rung in corpus.rungs {
        let idx = rung.abi as usize;
        let (fs, scoped) = if idx <= ladder.fs.len() {
            (
                format!("`0x{:x}`", ladder.fs[idx - 1]),
                format!("`0x{:x}`", ladder.scoped[idx - 1]),
            )
        } else {
            (
                "not requested by this build".to_string(),
                "not requested by this build".to_string(),
            )
        };
        let floor = if rung.abi > c.max_rung {
            format!(
                "**above this build's ladder** — clamped down to rung {}, journaled as \
                 `clamped_by_build`",
                c.max_rung
            )
        } else if rung.abi < c.min_abi {
            format!(
                "**below the floor** — a session refuses to start with \
                 `below-floor(abi={},required={})`; reachable only through `--landlock=abi:{}`, \
                 which warns that no published confinement claim applies",
                rung.abi, c.min_abi, rung.abi
            )
        } else {
            "at or above the floor — a shipped session runs here".to_string()
        };
        p.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            rung.abi,
            rung.buys,
            rung.axis.cell(),
            if rung.axis.mask_capped() {
                "yes — `--landlock=abi:N` reproduces its absence"
            } else {
                "**no** — not an access-mask bit"
            },
            rung.requested.cell(),
            fs,
            scoped,
            floor,
            claim_links(rung.claims),
        ));
    }
    p.push('\n');
    p.push_str(&format!(
        "The `handled_access_fs` column is the cumulative mask this build asks a kernel at\n\
         that rung for. It is parsed out of `handled_access_fs` in\n\
         `crates/vitrin-realm-init/src/landlock.rs` and cross-checked against the measured\n\
         table pinned in that crate's `the_rung_masks_pin_a_measured_table`; the two\n\
         readings disagreeing stops this page being emitted at all. The rights arrive in\n\
         this order: {}.\n\n",
        ladder
            .introduced
            .iter()
            .map(|(rung, name)| format!("rung {rung} → {name}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    render_exercised_rungs(p, corpus, c);
}

/// `1, 2 and 3`, for a list a human reads inside a sentence.
fn join_and(items: &[String]) -> String {
    match items {
        [] => "none".to_string(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `rung 4 is` / `rungs 4 and 5 are` -- the noun phrase with its verb, so a
/// generated sentence agrees with a count the corpus happens to reach. The
/// verb is a parameter because two generated sentences need this and they need
/// different ones.
///
/// This exists because the sentence it replaces hardcoded the plural. With one
/// sub-floor rung on either side, [`sub_floor_tally`] demanded that
/// `docs/book/src/limits.md` carry, word for word, "... and **rungs 4 are
/// not**" -- and because the tally is required verbatim on a published page,
/// the first time a sub-floor rung gained or lost a test the generator would
/// have forced broken prose onto it. Never reachable today; reachable on the
/// next edit, which is the same thing.
fn rungs_verb(items: &[String], singular: &str, plural: &str) -> String {
    match items {
        [one] => format!("rung {one} {singular}"),
        _ => format!("rungs {} {plural}", join_and(items)),
    }
}

/// `one` .. `ten`, so a generated sentence reads like the prose around it.
///
/// Above ten it falls back to digits rather than growing a table nobody
/// checks; the ladder is nine rungs and this is not a general-purpose speller.
fn spell(n: usize) -> String {
    const WORDS: [&str; 11] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    WORDS
        .get(n)
        .map(|w| (*w).to_string())
        .unwrap_or_else(|| n.to_string())
}

/// The sentence `handled_access_fs`'s own doc comment must carry: how many of
/// this build's rungs leave the mask untouched, and which.
///
/// Computed from the ladder parsed out of that same function, so the comment
/// cannot state a count the code contradicts. The count it replaced was typed
/// by hand and had drifted into counting the ABI-10 clamp row -- above this
/// build's ceiling, never passed to that function -- as one of the rungs.
fn flat_rungs_sentence(ladder: &Ladder, c: &Constants) -> String {
    let flat: Vec<String> = (2..=c.max_rung)
        .filter(|rung| {
            let i = *rung as usize - 1;
            ladder.fs.get(i) == ladder.fs.get(i - 1)
        })
        .map(|rung| rung.to_string())
        .collect();
    let mut count = spell(flat.len());
    count[..1].make_ascii_uppercase();
    format!(
        "**{count} of the {} rungs do not move it at all** ({})",
        spell(c.max_rung as usize),
        join_and(&flat)
    )
}

/// The sub-floor tally, in one sentence built from the corpus.
///
/// Rendered onto the page **and** required verbatim on the limits page, so the
/// two cannot disagree and neither can be edited to a number the corpus does
/// not support. Adding or dropping a `Rung::behavioural_tests` entry below the
/// floor changes this string, and the generator then refuses to emit until the
/// limits page carries the new one.
fn sub_floor_tally(corpus: &Corpus, c: &Constants) -> String {
    let numbers = |exercised: bool| -> Vec<String> {
        corpus
            .rungs
            .iter()
            .filter(|r| r.abi < c.min_abi && r.behavioural_tests.is_empty() != exercised)
            .map(|r| r.abi.to_string())
            .collect()
    };
    let yes = numbers(true);
    let no = numbers(false);
    match (yes.as_slice(), no.as_slice()) {
        // A floor of 1 leaves nothing below it. Spelled out rather than left
        // to the mixed arm, which would have said "rungs none are exercised".
        ([], []) => format!(
            "below the floor of {}, there are no rungs at all",
            c.min_abi
        ),
        (yes, []) => format!(
            "below the floor of {}, every rung ({}) is exercised",
            c.min_abi,
            join_and(yes)
        ),
        ([], no) => format!(
            "below the floor of {}, no rung ({}) is exercised",
            c.min_abi,
            join_and(no)
        ),
        (yes, no) => format!(
            "below the floor of {}, {} exercised and {} not",
            c.min_abi,
            rungs_verb(yes, "is", "are"),
            rungs_verb(no, "is", "are")
        ),
    }
}

/// Which rungs a behavioural test enters a domain at — **counted from the
/// corpus, never typed into a sentence.**
///
/// This paragraph exists because the sentence it replaces was wrong. D-043's
/// first draft published "the behavioural tests that exercise them" on every
/// row below the floor, including the two rows nothing exercises. A reader
/// cannot tell those rows apart from the table, so the page now says which is
/// which, and says it from [`Rung::behavioural_tests`] — the same field
/// step (7b) of [`render`] holds the claims against, and the same field whose
/// rung binding [`behavioural_rungs`] resolves rather than trusts.
///
/// The **denominator** is the ladder's height and not the number of rows here:
/// rows above [`Constants::max_rung`] are clamp rows, not rungs this build can
/// ask for, and counting them published "4 of the 10 rungs" against D-043's
/// "the ladder ... has nine rungs" on one branch.
fn render_exercised_rungs(p: &mut String, corpus: &Corpus, c: &Constants) {
    let exercised: Vec<&Rung> = corpus
        .rungs
        .iter()
        .filter(|r| !r.behavioural_tests.is_empty())
        .collect();
    p.push_str(
        "**Which rungs are exercised is counted from this table, not asserted.** A rung is\n\
         counted here when a test in `crates/vitrin-realm-init/src/main.rs` **enters** a\n\
         Landlock domain at it and asserts the kernel's own answer — a syscall's outcome\n\
         inside the domain, or the kernel's verdict on the request. Building a ruleset at a\n\
         rung and never entering it does not count.\n\n",
    );
    for rung in &exercised {
        p.push_str(&format!(
            "- **rung {}** — {}\n",
            rung.abi,
            rung.behavioural_tests
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    p.push('\n');
    // The denominator is the LADDER's height, not the number of rows: the
    // rows above this build's ceiling are clamp rows, and every other surface
    // in this repository counts the ladder as nine rungs. Counting rows here
    // published "4 of the 10 rungs" against "the ladder ... has nine rungs" in
    // D-043, on the same branch -- exactly the count-in-two-places defect this
    // page's tally check exists to stop.
    let above: Vec<String> = corpus
        .rungs
        .iter()
        .filter(|r| r.abi > c.max_rung)
        .map(|r| format!("ABI {}", r.abi))
        .collect();
    let askable = corpus.rungs.len() - above.len();
    let denominator = match above.as_slice() {
        [] => format!(
            "That is {} of the {askable} rungs this build can ask for, which is every row on\n\
             this page.",
            exercised.len()
        ),
        [one] => format!(
            "That is {} of the {askable} rungs this build can ask for. The one further row on\n\
             this page ({one}) is above this build's ceiling of {} — a clamp, not a rung it\n\
             requests — so it is not in that denominator.",
            exercised.len(),
            c.max_rung
        ),
        many => format!(
            "That is {} of the {askable} rungs this build can ask for. The further rows on this\n\
             page ({}) are above this build's ceiling of {} — clamps, not rungs it requests — so\n\
             they are not in that denominator.",
            exercised.len(),
            join_and(many),
            c.max_rung
        ),
    };
    p.push_str(&format!(
        "{denominator} Below the floor the tally is the one\n\
         `docs/book/src/limits.md` has to carry word for word:\n\n\
         > {}.\n\n\
         Every cell on an unexercised row is derived from this build's own source and\n\
         measured against nothing — keeping the sub-floor tests that exist and adding none\n\
         for the rest is decision D-043, not an oversight. **Neither the name nor the rung is\n\
         remembered.** Each name above is resolved against `BEHAVIOURAL_RUNGS` in that same\n\
         file, which declares the rungs that test enters a domain at; a name listed on a rung\n\
         it does not enter refuses to render, a rung it does enter and this page omits refuses\n\
         to render, and the tests themselves assert at runtime that the rungs they entered are\n\
         the rungs they declared. The generator also refuses to emit when the limits page does\n\
         not carry the tally above.\n\n",
        sub_floor_tally(corpus, c),
    ));
}

fn render_not_bought_table(p: &mut String, corpus: &Corpus, domains: &[Domain]) {
    p.push_str("## What each rung does not buy\n\n");
    // Which rows say "not pure gain" is a fact about the parsed ladder: a rung
    // that is not the FIRST of its domain group adds nothing to the domain
    // beneath it. This sentence used to carry a hand-typed "three", which was
    // right and was held by nothing -- move a right between rungs and the
    // grouping changes while the word does not.
    let adds_nothing: Vec<String> = domains
        .iter()
        .flat_map(|d| d.rungs.iter().skip(1))
        .map(|rung| rung.to_string())
        .collect();
    let opening = if adds_nothing.is_empty() {
        "The column this table exists for. A ladder printed without it reads as though\n\
         every rung is pure gain, and on this build's ladder every rung does move the\n\
         enforced domain — but not always in the tightening direction, which is what\n\
         rung 1's row below is about: the *absence* of `REFER` makes its domain\n\
         stricter, not weaker."
            .to_string()
    } else {
        format!(
            "The column this table exists for. A ladder printed without it reads as though\n\
             every rung is pure gain, and the rows below say otherwise on the kernel's own\n\
             terms: {} nothing to the enforced domain of the rung beneath,\n\
             counted from the parsed ladder rather than typed here. Rung 1's row is sharper\n\
             still — the *absence* of `REFER` makes its domain stricter, not weaker.",
            rungs_verb(&adds_nothing, "adds", "add")
        )
    };
    p.push_str(&opening);
    p.push_str("\n\n");
    p.push_str("| ABI | what it buys | what it does **not** buy |\n|---|---|---|\n");
    for rung in corpus.rungs {
        p.push_str(&format!(
            "| {} | {} | {} |\n",
            rung.abi, rung.buys, rung.not_bought
        ));
    }
    p.push('\n');
}

fn render_domain_table(p: &mut String, corpus: &Corpus, domains: &[Domain]) {
    p.push_str("## The enforced domains\n\n");
    p.push_str(
        "Two rungs enforce the same domain when this build's request is byte-identical at\n\
         both — `handled_access_fs`, `scoped` and the `landlock_restrict_self` flags word\n\
         together. The grouping below is computed from the parsed ladder. `applied_profile`\n\
         still spells every rung differently, so read that string as *which rung was\n\
         obtained*, never as *how much confinement*.\n\n\
         Each statement is published **verbatim** on the\n\
         [limits page](limits.md), so the two can be compared without anyone adjudicating\n\
         a paraphrase.\n\n",
    );
    p.push_str(
        "| domain | rungs | `handled_access_fs` | `scoped` | `restrict_self` flags | what \
         this domain is |\n|---|---|---|---|---|---|\n",
    );
    for (domain, tier) in domains.iter().zip(corpus.tiers) {
        p.push_str(&format!(
            "| {} ({}) | {} | `0x{:x}` | `0x{:x}` | `0x{:x}` | {} |\n",
            domain.index,
            tier.id,
            domain
                .rungs
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            domain.fs,
            domain.scoped,
            domain.flags,
            tier.statement,
        ));
    }
    p.push('\n');
    p.push_str(
        "The flags column is zero at every rung because a **shipped** session passes zero.\n\
         The one thing that moves it is `VITRIN_LANDLOCK_AUDIT=1` in vitrind's own\n\
         environment, which sets rung 7's `LOG_NEW_EXEC_ON` so the kernel keeps logging a\n\
         realm's denials past the shim's `execve`. It changes what the kernel writes down,\n\
         never what it permits, and it cannot be reached from `realm.toml` or a command\n\
         line — so under it rungs 6 and 7 stop being one domain **in the log flags only**.\n\n",
    );
}

fn render_denial_table(p: &mut String, corpus: &Corpus) {
    p.push_str("## What the ruleset denies that the realm's mount table does not\n\n");
    p.push_str(
        "A realm is confined by a mount table *and* a Landlock domain, and most of what\n\
         the domain refuses the mount table refuses too. Publishing the overlap as though\n\
         the ruleset earned it would be the flattering direction, so this table is only\n\
         the difference — and it is short.\n\n",
    );
    p.push_str(
        "| the denial | why the mount table does not already carry it | what has been \
         measured | published claim |\n|---|---|---|---|\n",
    );
    for denial in corpus.denials {
        p.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            denial.what,
            denial.why_the_mount_does_not,
            denial.measured,
            claim_links(denial.claims),
        ));
    }
    p.push('\n');
}

fn render_claim_table(p: &mut String, corpus: &Corpus) {
    p.push_str("## Every claim this table carries, and where it is published\n\n");
    p.push_str(
        "A row with a right and no claim, or a claim with no row, stops the generator.\n\
         Each needle below is checked against the surface it names on every run, so a\n\
         published sentence cannot be deleted or reworded while this table still cites it.\n\n",
    );
    p.push_str("| claim | what it says | published at |\n|---|---|---|\n");
    for claim in corpus.claims {
        p.push_str(&format!(
            "| `{}` | {} | {} |\n",
            claim.id,
            claim.says,
            claim
                .anchors
                .iter()
                .map(|a| format!("`{}` — “{}”", a.surface, a.needle))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    p.push('\n');
}

fn render_not_here(p: &mut String, c: &Constants) {
    p.push_str("## What is NOT on this page\n\n");
    p.push_str(&format!(
        "- **A per-kernel measurement.** `docs/plan/02-phase-2-semantic-epochs.md`'s\n  \
         restated criteria for P2.6.3 ask for a table generated \"from `vitrind\n  \
         --print-isolation` output on each kernel in the CI matrix, one row per ABI\n  \
         actually reported\". That is not what this is, and it is not a step towards it\n  \
         that was left half-taken: a table carrying the ABI of the machine that\n  \
         generated it cannot be byte-stable across two machines, so it cannot be the\n  \
         thing CI holds. The plan carries that as Correction 5.\n\
         - **Which kernels clear the floor — measured elsewhere, not here.** This page\n  \
         still probes nothing. Since 2026-08-16 the per-kernel measurement exists as its\n  \
         own artefact: [the kernel page](isolation-kernels.md), generated from boot logs\n  \
         checked in under `tests/kernel-matrix/rows/`. Read the two together and do not\n  \
         confuse them — this page says what the *build* requires, that one says what five\n  \
         *kernels* answered and which of them the floor of {min} admits. Two live machines\n  \
         are also on record: this repository's\n  \
         development box at Landlock ABI {dev} on 2026-08-15, and the runner its CI uses\n  \
         at ABI {ci} on 2026-08-14. The runner's number was read out of a CI job log that\n  \
         archives nothing, and it is corroborated but **not** replaced by the kernel page:\n  \
         booting the runner's own `6.17.0-1020-azure` in a bare initramfs answers ABI {ci}\n  \
         too, which is a fact about that kernel and not about that runner.\n\
         - **Any statement that P2.6.3's criteria were all met as written.** The task\n  \
         (issue #187) closed on 2026-08-19, on its *corrected* criteria and on decision\n  \
         D-043 — not on the row the plan first wrote. What landed with this page is a\n  \
         generated ladder of what this build requires, held by CI. A per-kernel row set\n  \
         landed separately on 2026-08-16 — five kernels, on [the kernel\n  \
         page](isolation-kernels.md) — and it is a row per *kernel*, not the \"one row per\n  \
         ABI actually reported\" the criteria ask for, a clause no byte-stable checked-in\n  \
         page can satisfy (the plan carries that as Correction 5). Four things did not\n  \
         become true when the task closed: five kernels answered five ABIs, and four of\n  \
         the nine rungs are reported by none of them; every row on that page is a\n  \
         **kernel** reading taken in a bare initramfs rather than a distribution; the\n  \
         behavioural per-rung tests this page's numbers rest on still live in\n  \
         `vitrin-realm-init`'s own suite, running on this repository's development box\n  \
         and on the CI runner and on no third machine, with the values they pin recorded\n  \
         on one box on one date; and the sub-floor half of those\n  \
         tests is evidence about the `--landlock=abi:N` dial rather than about any state a\n  \
         stock session reaches.\n\
         - **The realm's grant table.** Which hierarchies get which rights is\n  \
         [the limits page](limits.md)'s two-tier grant list, not a per-rung fact. The\n  \
         only grant-table row here is the one denial the mount table does not carry.\n\n",
        min = c.min_abi,
        dev = MEASURED_DEV_BOX_ABI,
        ci = MEASURED_CI_RUNNER_ABI,
    ));
}

fn render_runbook(p: &mut String) {
    p.push_str("## Runbook\n\n");
    p.push_str(
        "**To change what this page says, change the code or the published prose — never\n\
         this file.**\n\n\
         ```console\n\
         $ cargo xtask isolation-matrix          # regenerate in place, then review `git diff`\n\
         $ cargo xtask isolation-matrix --check  # what CI runs; reads only, writes nothing\n\
         ```\n\n\
         The generator refuses to emit anything when:\n\n\
         1. a pinned line of `crates/vitrin-realm-init/` or `crates/vitrin-core/` source\n   \
         is gone, so a cell here would describe code that no longer exists;\n\
         2. `LANDLOCK_MIN_ABI` or `LANDLOCK_BUILD_MAX_RUNG` cannot be read, or the ladder\n   \
         in `landlock.rs` cannot be parsed — a shape the parser does not recognise is an\n   \
         error, never a rung silently dropped;\n\
         3. the parsed ladder disagrees with the measured mask table pinned in\n   \
         `the_rung_masks_pin_a_measured_table`;\n\
         4. a rung row names no published claim, or a published claim is named by no row;\n\
         5. a claim's needle is no longer on the surface it cites;\n\
         6. a domain has no tier statement, or a tier statement is not on the limits page\n   \
         verbatim.\n\n\
         Adding a rung is therefore not an edit to this page: move the right in\n\
         `landlock.rs`, publish what the rung is worth, add the row and its claim to\n\
         `crates/xtask/src/isolation_matrix.rs`, and regenerate.\n",
    );
}

fn claim_links(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// Lines of shipped source a cell on this page describes.
pub static PINS: &[CodePin] = &[
    CodePin {
        path: LANDLOCK_RS,
        needle: "handled_access_net: 0,",
        means: "rung 4's facility is still unrequested, which is what makes the enforced domain \
                identical at rungs 3 and 4. Ask for a network mask and that row becomes false in \
                the overclaiming direction -- the page would show two rungs as one domain while \
                the kernel enforced two.",
    },
    CodePin {
        path: LANDLOCK_RS,
        needle: "const LOG_FLAGS_RUNG: u32 = 7;",
        means: "the audit log flags still arrive at rung 7, which is the rung the domain table \
                names as sharing a domain with 6 and 8 for every run that is not a measurement.",
    },
    CodePin {
        path: LANDLOCK_RS,
        needle: "fn restrict_self_flags(rung: u32, audit: bool) -> libc::c_uint {",
        means: "the flags word is still computed rather than constant, and still takes the audit \
                diagnostic as its only input. If a shipped session ever passes a nonzero flag, \
                the `restrict_self flags` column of the domain table stops being zero and rungs \
                6, 7 and 8 stop being one domain.",
    },
    CodePin {
        path: LANDLOCK_RS,
        needle: "out.push(grant(\"/etc\", READ_ANY, true)?);",
        means: "`/etc` is still granted read WITHOUT execute, which is the single denial in the \
                'what the ruleset denies that the mount table does not' table. Add EXECUTE here \
                and that table has no rows left.",
    },
    CodePin {
        path: LANDLOCK_RS,
        needle: "READ_FILE | WRITE_FILE | IOCTL_DEV,",
        means: "a bound render node still carries IOCTL_DEV, which is why rung 5 does not close \
                the published render-node limit. Remove it and the 'does not buy' cell for rung \
                5 becomes wrong -- and every realm on a GPU host stops rendering.",
    },
    CodePin {
        path: ISOLATION_RS,
        needle: "Ok(abi) if abi >= vitrin_realm_init::LANDLOCK_MIN_ABI => Support::Available,",
        means: "the floor is still a STARTUP GATE and not merely a printed constant. Relax it \
                and every 'below the floor' cell in the ladder describes a refusal that no \
                longer happens.",
    },
    CodePin {
        path: ISOLATION_RS,
        needle: "write!(f, \"below-floor(abi={found},required={required})\")",
        means: "the refusal string the machine table quotes is still the one the code prints. \
                This page tells an operator what to expect in their terminal, so a reworded \
                Display is a page that misquotes it.",
    },
];

/// The published claims each row carries.
///
/// Every needle here was checked against the surface it names before it was
/// written down, and is checked again on every run.
pub static CLAIMS: &[Claim] = &[
    Claim {
        id: "abi-floor-refuses-below-the-number",
        says: "A kernel reporting a Landlock ABI below this build's floor is refused at startup \
               rather than confined at a weaker rung, and the number is printed as \
               `build.landlock_min_abi`.",
        anchors: &[
            Anchor {
                surface: LIMITS,
                needle: "build.landlock_min_abi",
            },
            Anchor {
                surface: README,
                needle: "build.landlock_min_abi",
            },
            Anchor {
                surface: SECURITY,
                needle: "build.landlock_min_abi",
            },
        ],
    },
    // The lower half of this ladder is unreachable in production, and until
    // D-043 (2026-08-19) nothing published said what its behavioural tests
    // were therefore evidence *about*. A reader met three tests exercising
    // rungs no shipped session can run at and had to guess. These two claims
    // are the machine-held half of that decision, and they are a PAIR because
    // the first draft was a single claim carried by every sub-floor row --
    // including the two rows no test enters a domain at, which turned "the
    // behavioural tests that exercise them" into a published falsehood on
    // exactly the rows it was least true of. Which of the pair a row carries
    // is now decided by `Rung::behavioural_tests` and checked in `render`, so
    // a rung cannot render below the floor without an explanation and cannot
    // render with the wrong one.
    //
    // **Both are anchored to LIMITS only, and that is a decision.** Every
    // other claim here that a security reader would meet is anchored to two or
    // three surfaces, so the single anchor looks like an omission. It is not:
    // README.md and SECURITY.md say what this build refuses and on which
    // kernels, and "which sub-floor rungs a component test enters a domain at"
    // is a statement about this repository's own coverage rather than about
    // what an operator gets. Copying it onto two more surfaces would put the
    // same sentence in two more places that can rot, against a tally check
    // that already forces the limits page to stay current. If either of those
    // pages ever states the sub-floor coverage, add the anchor then -- an
    // anchored sentence is the only kind this generator can keep true.
    Claim {
        id: "sub-floor-rungs-hold-the-dial-not-the-floor",
        says: "Rungs below this build's floor are unreachable in production -- a kernel \
               reporting one is REFUSED at startup rather than confined weakly -- so a \
               behavioural test taken at one of them holds the `--landlock=abi:N` DIAL honest \
               and not the floor. This row is a rung such a test enters a domain at: it \
               describes no state a stock session can reach, and those tests are the only \
               evidence that this part of the table is not fiction (decision D-043, \
               2026-08-19).",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "hold the dial honest, not the floor",
        }],
    },
    Claim {
        id: "sub-floor-rungs-are-not-all-exercised",
        says: "This rung is below the floor AND no behavioural test enters a Landlock domain at \
               it, so every cell on this row is derived from this build's own source and \
               measured against nothing -- the sub-floor half of the ladder is exercised in \
               part, not throughout. D-043 (2026-08-19) kept the sub-floor tests that exist and \
               deliberately added none, so this row's status is a decision rather than an \
               oversight.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "exercised in part, not throughout",
        }],
    },
    Claim {
        id: "refer-makes-the-cap-a-dial",
        says: "A domain denies cross-directory `rename(2)` unless its ruleset HANDLES `REFER`, \
               so rung 1 is stricter about reparenting than rung 2 -- the rung cap is a dial, \
               not a one-way weakening.",
        anchors: &[
            Anchor {
                surface: LIMITS,
                needle: "The cap is a dial, not a one-way weakening",
            },
            Anchor {
                surface: README,
                needle: "dial, not a one-way tightening",
            },
        ],
    },
    Claim {
        id: "truncate-arrives-at-abi-3",
        says: "Below ABI 3 there is no `TRUNCATE` right, so a payload that cannot write a file \
               can still empty it -- measured at rung 2 succeeding and rung 3 refusing.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "Below ABI 3 there is no `TRUNCATE` right",
        }],
    },
    Claim {
        id: "net-scoping-is-carried-by-the-namespace",
        says: "ABI 4 buys network scoping, which this build leaves zero because the realm's own \
               network namespace carries that claim and covers UDP and raw sockets too.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "ABI 4 is network scoping",
        }],
    },
    Claim {
        id: "ioctl-dev-does-not-close-the-render-node",
        says: "ABI 5's `IOCTL_DEV` is one all-or-nothing bit per hierarchy and the app needs the \
               render node's ioctls, so the ruleset grants them there and the published \
               render-node limit survives the rung intact.",
        anchors: &[
            Anchor {
                surface: LIMITS,
                needle: "It does not close the render-node limit below.",
            },
            Anchor {
                surface: README,
                needle: "the ruleset grants it there and this cost is unchanged",
            },
            Anchor {
                surface: SECURITY,
                needle: "the app needs the node, so the ruleset grants it there",
            },
        ],
    },
    Claim {
        id: "scoped-is-defence-in-depth",
        says: "ABI 6's `scoped` field is defence in depth rather than the mechanism behind any \
               published claim: the realm's network namespace already isolates abstract UNIX \
               sockets and its pid namespace already denies signalling outward.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "ABI 6's `scoped` field is defence in depth rather than the mechanism behind \
                     either published claim",
        }],
    },
    Claim {
        id: "restrict-self-flags-are-not-mask-bits",
        says: "ABI 7 and ABI 8 buy `landlock_restrict_self` FLAGS rather than access-mask bits, \
               so `--landlock=abi:N` cannot simulate their absence and those rungs are \
               prose-backed rather than measurable here.",
        anchors: &[
            Anchor {
                surface: LIMITS,
                needle: "`landlock_restrict_self` *flags*",
            },
            Anchor {
                surface: README,
                needle: "byte-identical at rungs 3 and 4 and again at rungs 6, 7 and 8",
            },
        ],
    },
    Claim {
        id: "nine-rungs-are-six-domains",
        says: "Rung numbers and enforced domains are not the same count: rungs that buy nothing \
               this build requests collapse into their predecessor's domain, while \
               `applied_profile` still spells every rung differently.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "Nine rung numbers name six different domains",
        }],
    },
    Claim {
        id: "the-ladder-stops-at-the-build-ceiling",
        says: "This build's ladder stops at its ceiling and a newer kernel is clamped down to \
               it, journaled per realm as `isolation.landlock.clamped_by_build`; nothing here \
               has run on such a kernel.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "This build's ladder stops at rung 9",
        }],
    },
    Claim {
        id: "execute-under-etc-is-the-rulesets-own-denial",
        says: "`/etc` is bound read-only with no `noexec`, so the mount permits execution there \
               and only the Landlock ruleset refuses it -- the one filesystem denial this layer \
               contributes that the mount table does not already carry.",
        anchors: &[Anchor {
            surface: LIMITS,
            needle: "only this ruleset refuses it, and no test in this repository measures that \
                     denial yet",
        }],
    },
];

/// One statement per distinct enforced domain, in ladder order.
///
/// **Published verbatim on `docs/book/src/limits.md`** and checked on every
/// run; see [`TierStatement`].
pub static TIERS: &[TierStatement] = &[
    TierStatement {
        id: "T1",
        statement: "`handled_access_fs=0x1fff`, `scoped=0x0`: no `REFER`, so a realm capped at \
                    rung 1 cannot `rename(2)` across directories inside its own writable \
                    storage — the one rung that is stricter than the rung above it.",
    },
    TierStatement {
        id: "T2",
        statement: "`handled_access_fs=0x3fff`, `scoped=0x0`: `REFER` arrives, and handling it \
                    is what permits cross-directory rename inside the realm's own storage.",
    },
    TierStatement {
        id: "T3",
        statement: "`handled_access_fs=0x7fff`, `scoped=0x0`: `TRUNCATE` arrives at rung 3; rung \
                    4 buys `handled_access_net`, which this build leaves zero, so rungs 3 and 4 \
                    are one domain.",
    },
    TierStatement {
        id: "T4",
        statement: "`handled_access_fs=0xffff`, `scoped=0x0`: `IOCTL_DEV` arrives, and it does \
                    not close the render-node limit — the app needs the node's ioctls, so the \
                    ruleset grants them there.",
    },
    TierStatement {
        id: "T5",
        statement: "`handled_access_fs=0xffff`, `scoped=0x3`: rung 6 adds the `scoped` field; \
                    rungs 7 and 8 buy `landlock_restrict_self` flags rather than access-mask \
                    bits, so a mask cap cannot simulate their absence and rungs 6, 7 and 8 are \
                    one domain.",
    },
    TierStatement {
        id: "T6",
        statement: "`handled_access_fs=0x1ffff`, `scoped=0x3`: `RESOLVE_UNIX` arrives, and this \
                    is the highest rung this build requests — a kernel reporting a higher ABI is \
                    clamped here.",
    },
];

/// One row per rung this build can ask for, plus the clamp row above the
/// ceiling.
pub static RUNGS: &[Rung] = &[
    Rung {
        abi: 1,
        buys: "the base access-mask bits — `EXECUTE`, `WRITE_FILE`, `READ_FILE`, `READ_DIR`, the \
               `REMOVE_*` pair and the seven `MAKE_*` bits",
        axis: Axis::HandledAccessFs,
        requested: Requested::Yes,
        not_bought: "`REFER`, and its absence makes a rung-1 domain **stricter**: it refuses \
                     `rename(2)` and `link(2)` across directories even inside the realm's own \
                     writable storage. `EXDEV` at rung 1 and success at rung 2 are re-taken by \
                     a test on every run; rungs 3–9 succeeded in a hand run on 2026-08-14 that \
                     nothing since repeats.",
        claims: &[
            "refer-makes-the-cap-a-dial",
            "abi-floor-refuses-below-the-number",
            "sub-floor-rungs-hold-the-dial-not-the-floor",
        ],
        behavioural_tests: &[
            "a_realm_can_write_where_it_was_granted_and_nowhere_else",
            "rung_one_forbids_reparenting_that_the_rung_above_permits",
        ],
    },
    Rung {
        abi: 2,
        buys: "`LANDLOCK_ACCESS_FS_REFER`",
        axis: Axis::HandledAccessFs,
        requested: Requested::Yes,
        not_bought: "a tightening of any kind. Handling `REFER` is what **permits** \
                     cross-directory rename, which is how GTK and Firefox write files; a ladder \
                     read as \"higher is tighter\" has this rung backwards.",
        claims: &[
            "refer-makes-the-cap-a-dial",
            "sub-floor-rungs-hold-the-dial-not-the-floor",
        ],
        behavioural_tests: &[
            "rung_one_forbids_reparenting_that_the_rung_above_permits",
            "the_truncate_rung_is_measured_and_its_absence_is_measured_with_it",
        ],
    },
    Rung {
        abi: 3,
        buys: "`LANDLOCK_ACCESS_FS_TRUNCATE`",
        axis: Axis::HandledAccessFs,
        requested: Requested::Yes,
        not_bought: "protection for a path outside every granted write hierarchy, which was \
                     never truncatable at any rung. What it adds is that a path the domain \
                     grants only `READ_FILE` on can no longer be emptied by `truncate(2)`, \
                     `creat(2)` or `O_TRUNC`.",
        claims: &[
            "truncate-arrives-at-abi-3",
            "sub-floor-rungs-hold-the-dial-not-the-floor",
        ],
        behavioural_tests: &["the_truncate_rung_is_measured_and_its_absence_is_measured_with_it"],
    },
    Rung {
        abi: 4,
        buys: "`handled_access_net` — TCP bind/connect scoping by port",
        axis: Axis::HandledAccessNet,
        requested: Requested::No(
            "the realm's own network namespace carries that claim structurally and far more \
             completely, since it covers UDP and raw sockets too",
        ),
        not_bought: "anything this build asks for. `handled_access_net` stays zero, so the \
                     enforced domain at rung 4 is byte-identical to rung 3 — and because the cap \
                     moves `handled_access_fs`, `--landlock=abi:3` cannot simulate a kernel \
                     without rung 4.",
        claims: &[
            "net-scoping-is-carried-by-the-namespace",
            "nine-rungs-are-six-domains",
            "sub-floor-rungs-are-not-all-exercised",
        ],
        behavioural_tests: &[],
    },
    Rung {
        abi: 5,
        buys: "`LANDLOCK_ACCESS_FS_IOCTL_DEV`",
        axis: Axis::HandledAccessFs,
        requested: Requested::Yes,
        not_bought: "closure of the published render-node limit. The bit is all-or-nothing per \
                     hierarchy and the app needs the node's ioctls, so the ruleset **grants** \
                     `IOCTL_DEV` on every bound render node and on `/dev/pts`. What the rung \
                     buys is denying `ioctl` on every *other* device node in the realm.",
        claims: &[
            "ioctl-dev-does-not-close-the-render-node",
            "sub-floor-rungs-are-not-all-exercised",
        ],
        behavioural_tests: &[],
    },
    Rung {
        abi: 6,
        buys: "the `scoped` field — `SCOPE_ABSTRACT_UNIX_SOCKET` and `SCOPE_SIGNAL`",
        axis: Axis::Scoped,
        requested: Requested::Yes,
        not_bought: "a claim that rests on it. Both halves are already carried structurally by \
                     the realm's namespaces — abstract sockets are per-netns, and the pid \
                     namespace already denies signalling outward — so this rung is defence in \
                     depth, and no published sentence would become false without it.",
        claims: &["scoped-is-defence-in-depth"],
        behavioural_tests: &[],
    },
    Rung {
        abi: 7,
        buys: "`landlock_restrict_self` log flags — `LOG_SAME_EXEC_OFF`, `LOG_NEW_EXEC_ON`, \
               `LOG_SUBDOMAINS_OFF`",
        axis: Axis::RestrictSelfFlags,
        requested: Requested::No(
            "the log flags are observability, not confinement, and no published claim depends on \
             them; the one that is reachable at all is reachable only through the \
             `VITRIN_LANDLOCK_AUDIT` diagnostic in vitrind's own environment",
        ),
        not_bought: "any access right — and because it is a **flag** rather than a mask bit, \
                     `--landlock=abi:6` cannot simulate a kernel without it. There is nothing \
                     for the cap to remove from a request that never asked.",
        claims: &[
            "restrict-self-flags-are-not-mask-bits",
            "nine-rungs-are-six-domains",
        ],
        behavioural_tests: &["the_audit_log_flag_is_off_unless_asked_for_and_the_kernel_takes_it"],
    },
    Rung {
        abi: 8,
        buys: "`landlock_restrict_self` `TSYNC` — apply the domain to every thread of the caller",
        axis: Axis::RestrictSelfFlags,
        requested: Requested::No(
            "the helper is single-threaded by design and enforces the domain on the one thread \
             that then `execve`s, so its shape already carries what `TSYNC` would buy",
        ),
        not_bought: "any access right, and — as at rung 7 — nothing a mask cap can take away. \
                     `--landlock=abi:7` and `--landlock=abi:8` request the same domain.",
        claims: &[
            "restrict-self-flags-are-not-mask-bits",
            "nine-rungs-are-six-domains",
        ],
        behavioural_tests: &[],
    },
    Rung {
        abi: 9,
        buys: "`LANDLOCK_ACCESS_FS_IOCTL_DEV`'s ladder successor `RESOLVE_UNIX` — `connect(2)` \
               and addressed `sendmsg(2)` restricted to pathname UNIX sockets",
        axis: Axis::HandledAccessFs,
        requested: Requested::Yes,
        not_bought: "a rung above it that this build knows how to ask for. It travels with every \
                     writable hierarchy, because a socket the realm creates for itself — the \
                     shim's `wayland-0` among them — must stay connectable to it.",
        claims: &["the-ladder-stops-at-the-build-ceiling"],
        behavioural_tests: &[],
    },
    Rung {
        abi: 10,
        buys: "not stated here — this build does not define ABI 10's rights, and nothing in this \
               repository has measured them",
        axis: Axis::NotKnownToThisBuild,
        requested: Requested::No(
            "a build must not name a constant its own headers do not define; a kernel reporting \
             ABI 10 or above is clamped down to this build's ceiling and the clamp is journaled",
        ),
        not_bought: "anything, for this build. The clamp is asserted against a **constructed** \
                     ABI value rather than against a machine that reports one — nothing here has \
                     run on such a kernel.",
        claims: &["the-ladder-stops-at-the-build-ceiling"],
        behavioural_tests: &[],
    },
];

/// The filesystem denials the ruleset contributes over the mount table.
pub static DENIALS: &[Denial] = &[Denial {
    what: "`execve(2)` anywhere under `/etc`",
    why_the_mount_does_not: "`/etc` is bound `MS_RDONLY`, `MS_NOSUID`, `MS_NODEV` and with \
                                 **no** `noexec`, so the mount itself permits execution there. \
                                 Everywhere else the two maps agree: the ruleset grants \
                                 `EXECUTE` exactly where the mount table omits `noexec`.",
    measured: "**Nothing measures it.** No test in this repository exercises this denial, \
                   which makes it the one row here that is prose rather than measurement. Said \
                   plainly rather than left implied.",
    claims: &["execute-under-etc-is-the-rulesets-own-denial"],
}];

/// What `vitrind --print-isolation` can answer, and what this build does.
pub static MACHINE: &[MachineRow] = &[
    MachineRow {
        print_isolation: "`landlock.abi=N` with N at or above `build.landlock_min_abi` and at or \
                          below `build.landlock_max_rung`",
        what_this_build_does: "Starts. The helper asks for rung N and journals the rung it \
                              obtained, the rung it asked for, and the ABI the kernel reported.",
    },
    MachineRow {
        print_isolation: "`landlock.abi=N` with N above `build.landlock_max_rung`",
        what_this_build_does: "Starts, at the ceiling. The request is clamped down and the clamp \
                              is journaled as `isolation.landlock.clamped_by_build`.",
    },
    MachineRow {
        print_isolation: "`landlock.abi=N` with N at or above 1 and below \
                          `build.landlock_min_abi`",
        what_this_build_does: "**Refuses to start** at `--isolation=default`, reporting \
                              `below-floor(abi=N,required=M)`. The remedy is a newer kernel, \
                              explicitly *not* a sysctl, an `lsm=` edit or a `CONFIG_` change — \
                              those are already correct on such a machine.",
    },
    MachineRow {
        print_isolation: "`landlock.abi=absent(errno=E)`",
        what_this_build_does: "**Refuses to start**: the kernel does not implement the syscall. \
                              Check `CONFIG_SECURITY_LANDLOCK` and the kernel version.",
    },
    MachineRow {
        print_isolation: "`landlock.abi=restricted-by-policy(errno=E)`",
        what_this_build_does: "**Refuses to start**: the kernel has Landlock and something above \
                              it said no — most often `landlock` missing from the active `lsm=` \
                              list.",
    },
    MachineRow {
        print_isolation: "any of the above, with `--landlock=off` on the command line",
        what_this_build_does: "Starts with **no ruleset at all**, journaling `namespaces-only`. \
                              It is not a remedy for a kernel that could be upgraded, and no \
                              confinement claim on this page applies to such a session.",
    },
    MachineRow {
        print_isolation: "any of the above, with `--landlock=abi:N` on the command line",
        what_this_build_does: "Pins the request to rung N, **including below the floor**, \
                              because it is the instrument every per-rung measurement in this \
                              repository is taken with. A session pinned below the floor warns \
                              that no published confinement claim applies to its realms.",
    },
];

/// The checked-in corpus.
pub fn corpus() -> Corpus {
    Corpus {
        rungs: RUNGS,
        claims: CLAIMS,
        tiers: TIERS,
        denials: DENIALS,
        pins: PINS,
        machine: MACHINE,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// `cargo xtask isolation-matrix [--check]`.
pub fn isolation_matrix(root: &Path, check: bool) -> Result<()> {
    let sources = Sources::load(root)?;
    let page = render(&corpus(), &sources)?;
    let path = root.join(PAGE_PATH);

    if check {
        let on_disk = fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "isolation-matrix --check: reading {}: {e}. Run `cargo xtask isolation-matrix` \
                 and commit the result.",
                path.display()
            )
        })?;
        if on_disk == page {
            eprintln!(
                "xtask: isolation-matrix --check: no drift -- {PAGE_PATH} matches what the \
                 generator emits"
            );
            return Ok(());
        }
        let (line, disk_line, gen_line) = first_difference(&on_disk, &page);
        bail!(
            "isolation-matrix --check: {PAGE_PATH} has drifted from what the generator \
             emits.\n  first difference at line {line}:\n    on disk:   {disk_line}\n    \
             generator: {gen_line}\n  This page is GENERATED and must not be hand-edited. If the \
             Landlock ladder, the ABI floor or a published claim moved, run `cargo xtask \
             isolation-matrix` and commit the result; if none of them did, revert the hand edit."
        );
    }

    fs::write(&path, &page).map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    eprintln!(
        "xtask: isolation-matrix: wrote {PAGE_PATH} ({} table rows, {} claims)",
        table_rows(&page).len(),
        CLAIMS.len()
    );
    eprintln!("xtask: isolation-matrix complete -- review `git diff` and commit.");
    Ok(())
}

/// One-line-resolution diff report, so a `--check` failure says *where*.
fn first_difference(on_disk: &str, generated: &str) -> (usize, String, String) {
    let a: Vec<&str> = on_disk.lines().collect();
    let b: Vec<&str> = generated.lines().collect();
    for i in 0..a.len().max(b.len()) {
        let l = a.get(i).copied().unwrap_or("<end of file>");
        let r = b.get(i).copied().unwrap_or("<end of file>");
        if l != r {
            return (i + 1, truncate(l), truncate(r));
        }
    }
    (0, "<identical>".into(), "<identical>".into())
}

fn truncate(s: &str) -> String {
    const MAX: usize = 120;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn sources() -> Sources {
        Sources::load(&root()).expect("the checked-in tree must load")
    }

    fn baseline() -> String {
        render(&corpus(), &sources()).expect("the checked-in corpus must render")
    }

    /// Build a corpus with the rung rows replaced.
    fn with_rungs(rungs: Vec<Rung>) -> Corpus {
        Corpus {
            rungs: Box::leak(rungs.into_boxed_slice()),
            claims: CLAIMS,
            tiers: TIERS,
            denials: DENIALS,
            pins: PINS,
            machine: MACHINE,
        }
    }

    fn rung_clone(r: &Rung) -> Rung {
        Rung {
            abi: r.abi,
            buys: r.buys,
            axis: r.axis,
            requested: r.requested,
            not_bought: r.not_bought,
            claims: r.claims,
            behavioural_tests: r.behavioural_tests,
        }
    }

    /// **The non-vacuity test, direction one** (issue #187: "a row with a
    /// right and no claim ... fails the generator").
    #[test]
    fn a_rung_that_names_a_right_and_no_claim_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        // Rung 5 buys IOCTL_DEV. Strip its claim and nothing else.
        rungs[4].claims = &[];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a rung with a right and no published claim must not render");
        let text = format!("{err}");
        assert!(
            text.contains("ABI rung 5") && text.contains("NO published claim"),
            "the refusal must name the rung and say what is missing, got: {text}"
        );
    }

    /// **The non-vacuity test, direction two** ("... or a claim with no row,
    /// fails the generator").
    #[test]
    fn a_claim_no_row_carries_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        // Take `truncate-arrives-at-abi-3` off the only row that carries it,
        // leaving the claim published and orphaned.
        rungs[2].claims = &["abi-floor-refuses-below-the-number"];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a published claim no row carries must not render");
        let text = format!("{err}");
        assert!(
            text.contains("truncate-arrives-at-abi-3") && text.contains("carried by NO row"),
            "the refusal must name the orphaned claim, got: {text}"
        );
    }

    /// A row may not cite a claim id that resolves to nothing.
    #[test]
    fn a_row_citing_an_unknown_claim_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        rungs[0].claims = &["no-such-claim"];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a citation to nowhere must not render");
        assert!(format!("{err}").contains("no-such-claim"), "got: {err:#}");
    }

    /// The anchors are load-bearing: a claim whose published sentence is gone
    /// stops the page.
    #[test]
    fn a_claim_whose_published_sentence_is_gone_refuses_to_render() {
        let mut src = sources();
        // Remove exactly the sentence the render-node claim cites, leaving
        // every other surface untouched.
        src.limits = src
            .limits
            .replace("It does not close the render-node limit below.", "");
        let err = render(&corpus(), &src)
            .expect_err("a claim citing a deleted published sentence must not render");
        let text = format!("{err}");
        assert!(
            text.contains("ioctl-dev-does-not-close-the-render-node")
                && text.contains("no longer published"),
            "got: {text}"
        );
    }

    /// **D-043's own defect, as a gate.** The first draft of that decision
    /// carried one claim -- "the behavioural tests that exercise them" -- on
    /// every sub-floor row, including the two rows nothing enters a domain at,
    /// so the published matrix asserted tests existed where none did. Which
    /// claim a sub-floor row carries is now a function of that row's
    /// `behavioural_tests`, and disagreeing with it is refused.
    #[test]
    fn a_sub_floor_rung_claiming_tests_it_does_not_have_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        // Rung 4 (index 3) is below the floor and no test enters a domain at
        // it. Give it the "exercised" claim and change nothing else -- exactly
        // the edit the first draft made, and the one a reader cannot catch.
        rungs[3].claims = &[
            "net-scoping-is-carried-by-the-namespace",
            "nine-rungs-are-six-domains",
            EXERCISED_CLAIM,
        ];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a rung with no behavioural test may not claim one exercises it");
        let text = format!("{err}");
        assert!(
            text.contains("ABI rung 4") && text.contains(EXERCISED_CLAIM),
            "the refusal must name the rung and the claim it may not carry, got: {text}"
        );
    }

    /// And the other direction: a sub-floor row that carries neither claim has
    /// no answer to "what is a test at an unreachable rung evidence about",
    /// which is the state D-043 exists to end.
    #[test]
    fn a_sub_floor_rung_with_no_sub_floor_claim_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        // Rung 5 keeps a published claim, so this is not step (5)'s check
        // firing, and rung 4 still carries the unexercised claim, so it is not
        // step (6)'s either.
        rungs[4].claims = &["ioctl-dev-does-not-close-the-render-node"];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a sub-floor row must say which kind of sub-floor row it is");
        let text = format!("{err}");
        assert!(
            text.contains("ABI rung 5") && text.contains("NEITHER"),
            "the refusal must name the rung and which way it is wrong, got: {text}"
        );
    }

    /// The named tests are resolved, not trusted. A rename in
    /// `vitrin-realm-init` that this corpus has not followed is a page
    /// claiming a rung is exercised by something that is not there.
    #[test]
    fn a_behavioural_test_name_that_is_not_in_the_helper_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        rungs[0].behavioural_tests = &["a_test_that_was_renamed_and_nobody_followed_it"];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a behavioural test name that resolves to nothing must not render");
        let text = format!("{err}");
        assert!(
            text.contains("a_test_that_was_renamed_and_nobody_followed_it")
                && text.contains(REALM_INIT_MAIN_RS),
            "the refusal must name the missing test and the file it was looked for in, got: \
             {text}"
        );
    }

    /// The "not pure gain" rows are named from the domain grouping, not
    /// counted by hand. The sentence this replaces said "three of the rows
    /// below say otherwise" -- correct today, held by nothing, and silently
    /// wrong the moment a right moves between rungs.
    #[test]
    fn the_not_pure_gain_rows_are_named_from_the_domain_grouping() {
        let page = baseline();
        assert!(
            page.contains(
                "rungs 4, 7 and 8 add nothing to the enforced domain of the rung beneath"
            ),
            "the shipped ladder groups 3 with 4 and 6 with 7 and 8, so those three rungs \
             add nothing"
        );
        assert!(
            !page.contains("three of the rows below say otherwise"),
            "the hand-typed count is the form this replaced"
        );
        // And it follows the grouping: collapse rung 5's IOCTL_DEV into rung 4
        // and rung 5 joins the set.
        let mut src = sources();
        src.landlock_rs = src.landlock_rs.replace(
            "if rung >= 5 {\n        mask |= IOCTL_DEV;",
            "if rung >= 4 {\n        mask |= IOCTL_DEV;",
        );
        let ladder = Ladder::from_source(&src.landlock_rs, 9).expect("the moved ladder");
        let moved: Vec<u32> = domains(&ladder)
            .iter()
            .flat_map(|d| d.rungs.iter().skip(1).copied())
            .collect();
        assert_eq!(
            moved,
            vec![5, 7, 8],
            "moving IOCTL_DEV down a rung must move which rung adds nothing"
        );
    }

    /// The helper's own count of the flat rungs is derived from the same
    /// function this module parses, so it cannot state a number the code
    /// contradicts. The form it replaced was typed and had drifted into
    /// counting the clamp row above the ceiling as a rung -- "Five of the ten"
    /// against the "nine rungs" every other surface publishes.
    #[test]
    fn a_hand_typed_count_of_the_flat_rungs_refuses_to_render() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("the constants");
        let ladder = Ladder::from_source(&src.landlock_rs, c.max_rung).expect("the ladder");
        let required = flat_rungs_sentence(&ladder, &c);
        assert_eq!(
            required, "**Four of the nine rungs do not move it at all** (4, 6, 7 and 8)",
            "the shipped ladder leaves the mask flat at rungs 4, 6, 7 and 8"
        );
        assert!(
            src.landlock_rs.contains(&required),
            "the checked-in helper must already carry {required:?}"
        );

        let mut broken = sources();
        broken.landlock_rs = broken.landlock_rs.replace(
            &required,
            "**Five of the ten rungs do not move it at all** (4, 6, 7, 8 and 10)",
        );
        let err = render(&corpus(), &broken)
            .expect_err("a flat-rung count the ladder does not support must not render");
        let text = format!("{err}");
        assert!(
            text.contains(&required) && text.contains(LANDLOCK_RS),
            "the refusal must quote the sentence the helper has to carry, got: {text}"
        );
    }

    /// And the count follows the ladder rather than the prose: move a right to
    /// another rung and the required sentence changes with it.
    #[test]
    fn the_flat_rung_count_moves_when_the_ladder_does() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("the constants");
        let ladder = Ladder::from_source(&src.landlock_rs, c.max_rung).expect("the ladder");
        // TRUNCATE arrives at rung 3 today. Hand it to rung 4 instead: rung 3
        // goes flat and rung 4 stops being so.
        let moved = Ladder::from_source(
            &src.landlock_rs.replace(
                "if rung >= 3 {\n        mask |= TRUNCATE;",
                "if rung >= 4 {\n        mask |= TRUNCATE;",
            ),
            c.max_rung,
        )
        .expect("the moved ladder");
        assert_ne!(
            ladder.fs, moved.fs,
            "the perturbation must change the ladder"
        );
        assert_eq!(
            flat_rungs_sentence(&moved, &c),
            "**Four of the nine rungs do not move it at all** (3, 6, 7 and 8)"
        );
    }

    /// **The gap the name-only check could not see.** Resolving `fn NAME(`
    /// proves a test exists; it says nothing about the rung it enters. This is
    /// the exact edit that used to render GREEN: a rung-1 test moved onto rung
    /// 5's row, with rung 5's claim swapped so nothing else objected first.
    /// The page then published "**rung 5** —
    /// `a_realm_can_write_where_it_was_granted_and_nowhere_else`" under prose
    /// saying a rung counts only when a test *enters* a domain at it.
    #[test]
    fn a_behavioural_test_listed_on_a_rung_it_does_not_enter_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        rungs[4].behavioural_tests = &["a_realm_can_write_where_it_was_granted_and_nowhere_else"];
        rungs[4].claims = &["ioctl-dev-does-not-close-the-render-node", EXERCISED_CLAIM];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a test may not be published on a rung it never enters");
        let text = format!("{err}");
        assert!(
            text.contains("ABI rung 5")
                && text.contains("a_realm_can_write_where_it_was_granted_and_nowhere_else")
                && text.contains("not at this one"),
            "the refusal must name the rung, the test and the rungs it really enters, got: \
             {text}"
        );
    }

    /// And the same binding read the other way, so the page cannot
    /// UNDER-report: a rung a declared test enters must be listed on that
    /// rung's row. Without this direction, deleting a name from a sub-floor
    /// row would move it to the unexercised claim and change the tally, all
    /// while the test kept entering that rung on every run.
    #[test]
    fn a_rung_a_declared_test_enters_and_this_page_omits_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        // `the_truncate_rung_...` enters rungs 2 AND 3. Drop it from rung 2's
        // row only, which leaves rung 2 still exercised and still claiming so.
        rungs[1].behavioural_tests = &["rung_one_forbids_reparenting_that_the_rung_above_permits"];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a rung a test enters may not be omitted from that rung's row");
        let text = format!("{err}");
        assert!(
            text.contains("the_truncate_rung_is_measured_and_its_absence_is_measured_with_it")
                && text.contains("rung 2's row does not list it"),
            "the refusal must name the test and the row that dropped it, got: {text}"
        );
    }

    /// A `BEHAVIOURAL_RUNGS` this generator cannot find is a binding that has
    /// silently reverted to a name-only lookup, which is the state this check
    /// exists to end. It refuses rather than falling back.
    #[test]
    fn a_missing_behavioural_rungs_table_refuses_to_render() {
        let mut src = sources();
        src.realm_init_main_rs = src.realm_init_main_rs.replace(
            BEHAVIOURAL_RUNGS_MARKER,
            "SOMETHING_ELSE: &[(&str, &[u32])] = &[",
        );
        let err = render(&corpus(), &src)
            .expect_err("a vanished rung declaration must not degrade to a name-only check");
        let text = format!("{err}");
        assert!(
            text.contains("BEHAVIOURAL_RUNGS") && text.contains(REALM_INIT_MAIN_RS),
            "the refusal must name the table and the file, got: {text}"
        );
    }

    /// The third way a sub-floor row can be wrong, and the one the earlier
    /// gate tests left uncovered: carrying **both** claims, which answers
    /// "what is a test at an unreachable rung evidence about" twice and
    /// contradictorily.
    #[test]
    fn a_sub_floor_rung_carrying_both_sub_floor_claims_refuses_to_render() {
        let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        rungs[3].claims = &[
            "net-scoping-is-carried-by-the-namespace",
            "nine-rungs-are-six-domains",
            EXERCISED_CLAIM,
            UNEXERCISED_CLAIM,
        ];
        let err = render(&with_rungs(rungs), &sources())
            .expect_err("a sub-floor row may not carry both halves of the pair");
        let text = format!("{err}");
        assert!(
            text.contains("ABI rung 4") && text.contains("BOTH"),
            "the refusal must name the rung and which way it is wrong, got: {text}"
        );
    }

    /// The tally is required **verbatim** on a published page, so a shape it
    /// renders ungrammatically is a shape that forces broken prose onto
    /// `docs/book/src/limits.md`. The hardcoded plural did exactly that: with
    /// one rung on the unexercised side it demanded "and rungs 4 are not".
    #[test]
    fn the_sub_floor_tally_is_grammatical_for_every_shape_the_corpus_can_reach() {
        let c = Constants {
            min_abi: 6,
            max_rung: 9,
        };
        let some: &[&str] = &["a_test"];
        let tally = |edits: &[(usize, &'static [&'static str])]| -> String {
            let mut rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
            for (index, tests) in edits {
                rungs[*index].behavioural_tests = tests;
            }
            sub_floor_tally(&with_rungs(rungs), &c)
        };

        // As shipped, and byte-identical to the sentence limits.md carries.
        assert_eq!(
            tally(&[]),
            "below the floor of 6, rungs 1, 2 and 3 are exercised and rungs 4 and 5 are not"
        );
        // ONE rung left on the unexercised side -- the shape that used to
        // demand "rungs 4 are not".
        assert_eq!(
            tally(&[(4, some)]),
            "below the floor of 6, rungs 1, 2, 3 and 5 are exercised and rung 4 is not"
        );
        // And one on the exercised side.
        assert_eq!(
            tally(&[(1, &[]), (2, &[])]),
            "below the floor of 6, rung 1 is exercised and rungs 2, 3, 4 and 5 are not"
        );
        // The two ends, which were special-cased from the start.
        assert_eq!(
            tally(&[(3, some), (4, some)]),
            "below the floor of 6, every rung (1, 2, 3, 4 and 5) is exercised"
        );
        assert_eq!(
            tally(&[(0, &[]), (1, &[]), (2, &[])]),
            "below the floor of 6, no rung (1, 2, 3, 4 and 5) is exercised"
        );
        // And a floor of 1, where the sub-floor set is empty on both sides.
        // The mixed arm would have said "rungs none are exercised".
        let ground = Constants {
            min_abi: 1,
            max_rung: 9,
        };
        let rungs: Vec<Rung> = RUNGS.iter().map(rung_clone).collect();
        assert_eq!(
            sub_floor_tally(&with_rungs(rungs), &ground),
            "below the floor of 1, there are no rungs at all"
        );
    }

    /// The denominator on the emitted page is the LADDER's height, not the
    /// number of rows: the clamp row above the ceiling is not a rung this
    /// build can ask for. Counting rows published "4 of the 10 rungs" against
    /// D-043's "the ladder ... has nine rungs" on the same branch.
    #[test]
    fn the_exercised_count_is_out_of_the_rungs_this_build_can_ask_for() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("the constants");
        let page = render(&corpus(), &src).expect("the shipped corpus renders");
        let askable = RUNGS.iter().filter(|r| r.abi <= c.max_rung).count();
        let exercised = RUNGS
            .iter()
            .filter(|r| !r.behavioural_tests.is_empty())
            .count();
        assert!(
            askable < RUNGS.len(),
            "this test is only meaningful while the corpus carries a row above the ceiling"
        );
        assert!(
            page.contains(&format!(
                "That is {exercised} of the {askable} rungs this build can ask for"
            )),
            "the page must count out of the ladder's height, not its row count"
        );
        assert!(
            !page.contains(&format!("of the {} rungs on this page", RUNGS.len())),
            "the row-counting form is the one that contradicted every other surface"
        );
    }

    /// The tally is a **count**, and a count corrected in one place and left
    /// stale in another is this repository's dominant defect. So the limits
    /// page must carry the one this corpus computes, word for word.
    #[test]
    fn a_limits_page_carrying_a_stale_sub_floor_tally_refuses_to_render() {
        let mut src = sources();
        let tally = sub_floor_tally(
            &corpus(),
            &Constants::from_source(&src.realm_init_lib_rs).expect("the constants"),
        );
        assert!(
            src.limits.contains(&tally),
            "the checked-in limits page must already carry {tally:?}"
        );
        // The stale form: the count a reader would have written before rungs 4
        // and 5 were separated out.
        src.limits = src.limits.replace(
            &tally,
            "below the floor of 6, rungs 1, 2, 3, 4 and 5 are exercised",
        );
        let err = render(&corpus(), &src)
            .expect_err("a limits page whose tally no longer matches the corpus must not render");
        let text = format!("{err}");
        assert!(
            text.contains(&tally) && text.contains(LIMITS),
            "the refusal must name the surface and quote the tally it must carry, got: {text}"
        );
    }

    /// Item 4: the per-tier statement must be on the limits page verbatim, so
    /// a later cross-check needs no human to adjudicate a paraphrase.
    ///
    /// The perturbation is a **paraphrase**, not a deletion: one synonym
    /// swapped inside T4, of the kind a human comparing the two pages would
    /// wave through. The gate must not.
    #[test]
    fn a_tier_statement_the_limits_page_only_paraphrases_refuses_to_render() {
        let mut tiers: Vec<TierStatement> = TIERS
            .iter()
            .map(|t| TierStatement {
                id: t.id,
                statement: t.statement,
            })
            .collect();
        tiers[3].statement = "`handled_access_fs=0xffff`, `scoped=0x0`: `IOCTL_DEV` arrives, and \
                              it does not shut the render-node limit — the app needs the node's \
                              ioctls, so the ruleset grants them there.";
        let corpus = Corpus {
            rungs: RUNGS,
            claims: CLAIMS,
            tiers: Box::leak(tiers.into_boxed_slice()),
            denials: DENIALS,
            pins: PINS,
            machine: MACHINE,
        };
        let err =
            render(&corpus, &sources()).expect_err("a paraphrased tier statement must not render");
        let text = format!("{err}");
        assert!(
            text.contains("tier T4") && text.contains("not published VERBATIM"),
            "got: {text}"
        );
    }

    /// The pins describe code, not prose: deleting the pinned line stops the
    /// page even though every published sentence is untouched.
    #[test]
    fn a_broken_code_pin_refuses_to_render() {
        let mut src = sources();
        src.landlock_rs = src.landlock_rs.replace("handled_access_net: 0,", "");
        let err = render(&corpus(), &src)
            .expect_err("a page describing code that is gone must not render");
        assert!(format!("{err}").contains("PIN BROKEN"), "got: {err:#}");
    }

    /// The floor is read from the source, not typed here -- so a re-tune moves
    /// the page.
    #[test]
    fn the_floor_and_ceiling_are_read_from_the_crate_that_declares_them() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("constants must parse");
        assert_eq!(
            c.min_abi,
            vitrin_min_abi_from_tree(),
            "the parser must agree with the checked-in constant"
        );
        assert!(c.min_abi >= 1 && c.min_abi <= c.max_rung);

        let bumped = src.realm_init_lib_rs.replace(
            "pub const LANDLOCK_MIN_ABI: u32 = 6;",
            "pub const LANDLOCK_MIN_ABI: u32 = 8;",
        );
        assert_ne!(
            bumped, src.realm_init_lib_rs,
            "the replacement matched nothing, so the assertion below would be testing the \
             unmodified source against itself"
        );
        let moved = Constants::from_source(&bumped).expect("a bumped floor must still parse");
        assert_eq!(
            moved.min_abi, 8,
            "re-tuning the constant must move the number this page prints"
        );
    }

    /// The harness's Python copy of the floor and the ceiling agrees with the
    /// crate -- and the check that says so can actually fail.
    ///
    /// The first assertion is the live one: it is what would have caught
    /// e7b5514 leaving `tests/integration/harness.py` at 7 while the crate
    /// went to 6. The rest is the lever, because a mirror check that passed on
    /// a mismatch would be indistinguishable from one that passed on a match.
    #[test]
    fn the_harness_copy_of_the_floor_is_held_to_the_crate_and_the_hold_can_fail() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("constants must parse");
        c.cross_check_harness(&src.harness_py)
            .expect("the checked-in harness must mirror the checked-in crate");

        // Lever 1: a harness that drifted on the floor is refused, and the
        // message names both numbers so the reader knows which to move.
        let drifted = src.harness_py.replace(
            &format!("\nLANDLOCK_MIN_ABI = {}", c.min_abi),
            "\nLANDLOCK_MIN_ABI = 7",
        );
        assert_ne!(
            drifted, src.harness_py,
            "the replacement matched nothing, so this lever would be testing the unmodified \
             harness against itself"
        );
        let err = c
            .cross_check_harness(&drifted)
            .expect_err("a drifted floor must refuse")
            .to_string();
        assert!(err.contains("LANDLOCK_MIN_ABI = 7"), "{err}");
        assert!(err.contains(&c.min_abi.to_string()), "{err}");

        // Lever 2: the ceiling is mirrored too, and separately.
        let capped = src.harness_py.replace(
            &format!("\nLANDLOCK_BUILD_MAX_RUNG = {}", c.max_rung),
            "\nLANDLOCK_BUILD_MAX_RUNG = 3",
        );
        assert_ne!(
            capped, src.harness_py,
            "the ceiling replacement matched nothing"
        );
        assert!(
            c.cross_check_harness(&capped).is_err(),
            "a drifted ceiling must refuse"
        );

        // Lever 3: a harness that dropped the constant entirely is refused
        // rather than silently unchecked -- the failure mode a `find`-based
        // mirror check falls into.
        let deleted = src
            .harness_py
            .replace("\nLANDLOCK_MIN_ABI = ", "\nLANDLOCK_MIN_ABI_OLD = ");
        assert!(
            c.cross_check_harness(&deleted).is_err(),
            "a missing copy must refuse, not pass for want of anything to compare"
        );
    }

    /// Read the floor straight out of the file, independently of the parser
    /// under test.
    fn vitrin_min_abi_from_tree() -> u32 {
        let text = fs::read_to_string(root().join(REALM_INIT_LIB_RS)).expect("lib.rs");
        let line = text
            .lines()
            .find(|l| {
                l.trim_start()
                    .starts_with("pub const LANDLOCK_MIN_ABI: u32 = ")
            })
            .expect("the constant must be declared");
        line.trim()
            .trim_start_matches("pub const LANDLOCK_MIN_ABI: u32 = ")
            .trim_end_matches(';')
            .parse()
            .expect("a number")
    }

    /// The ladder is parsed from the helper and cross-checked against the
    /// helper's own measured table. Perturb either and nothing renders.
    #[test]
    fn the_parsed_ladder_must_agree_with_the_measured_mask_table() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("constants");
        let ladder = Ladder::from_source(&src.landlock_rs, c.max_rung).expect("ladder");
        ladder
            .cross_check(&src.realm_init_main_rs)
            .expect("the checked-in tree must agree with itself");

        // Move TRUNCATE up a rung in the parsed source only. The measured
        // table still says rung 3, so the two readings disagree.
        let moved = src.landlock_rs.replace(
            "if rung >= 3 {\n        mask |= TRUNCATE;",
            "if rung >= 4 {\n        mask |= TRUNCATE;",
        );
        assert_ne!(moved, src.landlock_rs, "the replacement must have applied");
        let ladder = Ladder::from_source(&moved, c.max_rung).expect("ladder still parses");
        let err = ladder
            .cross_check(&src.realm_init_main_rs)
            .expect_err("a right that moved rung must fail the cross-check");
        assert!(format!("{err}").contains("disagrees with the MEASURED mask table"));
    }

    /// A rung row set that does not match the parsed ladder is refused rather
    /// than rendered short -- the staleness this page exists to prevent.
    #[test]
    fn a_rung_row_set_that_does_not_match_the_ladder_refuses_to_render() {
        let rungs: Vec<Rung> = RUNGS.iter().take(4).map(rung_clone).collect();
        let err =
            render(&with_rungs(rungs), &sources()).expect_err("a short ladder must not render");
        assert!(
            format!("{err}").contains("does not match the code"),
            "got: {err:#}"
        );
    }

    /// The domain grouping is derived, and the emitted count is the derived
    /// one.
    #[test]
    fn the_domain_count_is_derived_from_the_ladder_and_is_not_the_rung_count() {
        let src = sources();
        let c = Constants::from_source(&src.realm_init_lib_rs).expect("constants");
        let ladder = Ladder::from_source(&src.landlock_rs, c.max_rung).expect("ladder");
        let d = domains(&ladder);
        assert!(
            d.len() < ladder.fs.len(),
            "some rungs must collapse into one domain, or the published 'nine rungs, six \
             domains' sentence is wrong"
        );
        let page = baseline();
        assert!(
            page.contains(&format!(
                "**{} rung numbers naming {} distinct enforced",
                ladder.fs.len(),
                d.len()
            )),
            "the page must print the derived counts"
        );
    }

    /// Every rung row reaches the page, including the clamp row above the
    /// ceiling, and each carries a claim in its own row.
    #[test]
    fn every_rung_reaches_the_page_with_a_claim_beside_it() {
        let page = baseline();
        let rows: Vec<TableRow> = table_rows(&page)
            .into_iter()
            .filter(|r| r.section == "The ladder, one row per ABI rung")
            .collect();
        assert_eq!(rows.len(), RUNGS.len(), "one row per rung");
        for (row, rung) in rows.iter().zip(RUNGS) {
            assert_eq!(row.cells[0], rung.abi.to_string());
            let claims = row.cells.last().expect("a claim cell");
            assert!(
                !claims.is_empty(),
                "rung {} rendered an empty claim cell",
                rung.abi
            );
            for id in rung.claims {
                assert!(
                    claims.contains(id),
                    "rung {} must cite {id} on the page, got {claims:?}",
                    rung.abi
                );
            }
        }
    }

    /// The four rows this task exists for are on the page, in the column that
    /// carries them.
    #[test]
    fn the_four_honest_rows_are_published() {
        let page = baseline();
        // 1. execute under /etc is the ruleset's own denial.
        assert!(page.contains("`execve(2)` anywhere under `/etc`"));
        assert!(page.contains("**no** `noexec`"));
        // 2. ABI 5's IOCTL_DEV does not close the render-node limit.
        assert!(page.contains("closure of the published render-node limit"));
        // 3. ABI 2's REFER means a lower rung is stricter.
        assert!(page.contains("makes a rung-1 domain **stricter**"));
        // 4. Rungs 7 and 8 buy flags, so a mask cap cannot simulate them.
        for rung in [7u32, 8] {
            let row = table_rows(&page)
                .into_iter()
                .find(|r| {
                    r.section == "The ladder, one row per ABI rung"
                        && r.cells[0] == rung.to_string()
                })
                .unwrap_or_else(|| panic!("rung {rung} must have a ladder row"));
            assert_eq!(
                row.cells[3], "**no** — not an access-mask bit",
                "rung {rung} must be published as NOT simulable by a mask cap"
            );
        }
    }

    /// No emitted cell says nothing.
    #[test]
    fn no_emitted_cell_is_empty_or_a_bare_pass() {
        let page = baseline();
        verify_emitted_rows(&page).expect("the checked-in corpus must emit no empty cell");
        assert!(!table_rows(&page).is_empty());
    }
}
