// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask limits-check` -- the published-claim drift gate (WS-E.4.4,
//! issue #224, acceptance criterion 1).
//!
//! # #172 HAS LANDED, AND THIS IS NO LONGER TEMPORARY
//!
//! This module was written under #224's first task -- *"Reuse #172's chosen
//! drift mechanism rather than inventing a second convention. If #172 has not
//! landed, add an explicitly temporary claim-string check and label it as such
//! in its own comments, so it is replaced rather than entrenched."* -- at a
//! time when #172 was open and had chosen nothing. Its header said so at
//! length, and enumerated what would happen under each of #172's three
//! candidate shapes.
//!
//! **The owner took option (b): a drift check that fails when one surface
//! disagrees, implemented by extending this module rather than by building a
//! parallel tool.** Option (a), a single generated source, was rejected because
//! the three published surfaces legitimately want different registers for the
//! same fact -- `README.md` a contributor's summary, `docs/book/src/limits.md`
//! an argued page, `site/index.html` a landing-page warning box. Option (c), a
//! checklist convention, is the one #172's own body calls weakest, and it is
//! what #224's acceptance criterion refuses on its own: *"Changing a WS-E claim
//! on one surface and not the others **fails something in CI**."*
//!
//! So the four claims #172 names as known to drift -- the fuzz soak, the wlcs
//! counts, OIN, REUSE -- are now rows in [`CLAIMS`], and the site's undated
//! `3/180` has been normalised to the full four-number form with its date and
//! its wlcs version, which is what made gating them possible. Alongside them
//! #172 added three things this module did not have, each closing a hole that
//! was live on `main`:
//!
//! * [`DERIVED`] -- one value, one canonical definition, several renderings.
//!   [`Anchor`] cannot express this and the gap was not theoretical: six
//!   surfaces stated one AppArmor measurement, three said kernel `1022` and
//!   three said `1020`, and `apparmor-profile-is-one-image-and-uninstalled`
//!   passed green over all six because it anchored the artefact path and the
//!   bound sentence and never the kernel release.
//! * [`MIRRORS`] -- the same machinery over code-to-code duplicates, the class
//!   `tests/integration/harness.py`'s `LANDLOCK_MIN_ABI = 7` belonged to.
//! * [`uncited_issues`] and [`tracker_report`] -- the two halves of #172's
//!   third acceptance criterion, split because only one of them can honestly be
//!   a pull-request gate. Read [`uncited_issues`]'s own docs before treating
//!   either as closing that criterion; neither does, and both say so.
//!
//! An adversarial pass over the first cut of all three found two holes that are
//! now closed here, and they are worth stating as properties rather than as
//! history, because both are shapes this kind of tool acquires by default:
//!
//! * **A surface is held at every occurrence, not at the first one.** A
//!   `contains` test stops at the first match, so a page that states a value
//!   twice was held only at one of them and could contradict *itself* with the
//!   build green. That is not a hypothetical: `site/index.html` said the
//!   per-kernel matrix "has not been built" in one paragraph and cited its
//!   measurements ninety lines below. [`Rendering::context`] and
//!   [`scan_surface`] hold all of them, and a second, disagreeing occurrence
//!   fails with both line numbers.
//! * **The covered set cannot shrink quietly.** Deleting a whole [`Claim`] row
//!   used to leave every check green and move only a tally printed at the end of
//!   a passing step. [`COVERED_CLAIMS`], [`COVERED_DERIVED`] and
//!   [`COVERED_MIRRORS`] name every row, so a deletion is red and the failure
//!   says which id left.
//!
//! # What it checks, which is two different things
//!
//! Each [`Claim`] carries two lists, and a failure in either is a red build:
//!
//! 1. **[`Claim::surfaces`] -- the surfaces agree.** An anchor string that must
//!    appear in each named published surface. Deleting a claim from `README.md`
//!    while leaving it on the limits page fails here. This is #224's acceptance
//!    criterion 1 and #172's five-surface complaint.
//! 2. **[`Claim::evidence`] -- the claim is still true of the code.** An
//!    assertion about the tree that must hold for the published sentence to be
//!    honest: a constant that must still read `61440`, an interface name that
//!    must still appear nowhere in `shim/src/`, a CI job comment that must still
//!    say the job compiles and does not exercise. Changing the code without
//!    changing the page fails here.
//!
//! The second half is the one #172's option (b) does not describe and #224
//! needs, because #224's entire history is a body whose own items were false of
//! `main`: on 2026-08-12 an audit found item (6) claimed "no cross-realm
//! clipboard of any kind" after WS-E.2.1 shipped one, and item (8) claimed "no
//! lock screen" after WS-E.2.2 shipped one. A cross-surface check would have
//! passed both -- the surfaces would have agreed with each other and disagreed
//! with the code. **A page that overstates a gap is as dishonest as one that
//! hides it**, and only the evidence half can see that direction.
//!
//! # And a third thing, which is a set and not a string
//!
//! [`cross_check_limit_sets`] holds #224's acceptance criterion 5 -- *"the WS-E
//! plan document's honesty section and `limits.md` are cross-checked
//! **mechanically, not by reading**"* -- and it is deliberately a different
//! instrument from the two above.
//!
//! **What it compares is the SET of limits, never their wording.** A plan
//! document's enumeration and `docs/book/src/limits.md` are written in two
//! registers that must not be forced to converge: the plan side is a body of
//! work's internal enumeration, addressed to whoever maintains this project, and
//! says things like *"TCB growth for zero differentiator, exactly as #223
//! predicted"*. The limits page is addressed to a stranger deciding
//! whether to run this, and says *"anyone who walks up to your dark laptop and
//! touches a key is inside your session."* Those are the same project being
//! honest twice, not one sentence duplicated. Anchoring a phrase across them --
//! the [`Claim::surfaces`] instrument -- would turn every honest rewording of
//! either register into a red build, and #224's own risk list names that
//! outcome by name: *"brittle... which trains people to weaken the check."*
//!
//! What the criterion actually protects against is narrower and is not about
//! prose at all: **a limit that exists in one document and not the other.** §6
//! enumerating a gap the limits page never published is a gap the project knows
//! about and does not tell anyone; the limits page publishing a WS-E limit §6
//! does not carry means the workstream's own enumeration -- the thing
//! `CLAUDE.md`'s `known-limit` rule sends you to -- is no longer the set. Both
//! directions are drift and neither is visible to a check that reads wording.
//!
//! So every limit gets an **identity that survives rewriting**: a marker
//! comment, invisible in both rendered documents, carrying a kebab-case id.
//!
//! ```text
//! <!-- limit-set: begin -->                       (in §6, once)
//! - <!-- limit: no-key-repeat-on-drm -->          this §6 limit IS on the page
//!   **A held key does not repeat on `--drm`** ...
//! - <!-- limit-not-on-page: kdf-in-the-tcb -- no surface carries it: ... -->
//!   **A KDF is now in the TCB** ...             this §6 limit is NOT, and why
//! <!-- limit-set: end -->
//!
//! <!-- limit: no-key-repeat-on-drm -->            (in limits.md, at the entry)
//! **And on the daily-driver backend a held key does not repeat at all.** ...
//! ```
//!
//! The id is not a phrase and is not read by anyone: rewrite either document's
//! sentence however the register needs and the check does not move. Delete the
//! limit from one document and it fails. Five rules make that non-vacuous:
//!
//! 1. Every published id in §6 must appear on the limits page, and every id on
//!    the limits page must be a published id in §6. That is set equality, and it
//!    is the pair of directions the criterion names.
//! 2. **Every top-level list item in §6's limit set must carry at least one
//!    marker.** Without this the first direction is decoration: a new §6 limit
//!    with no marker at all would be invisible rather than unpublished, which is
//!    exactly the "silently dropped from the list" failure §6 records itself
//!    having committed once already.
//! 3. `limit-not-on-page` is the escape hatch and it **costs a written reason**,
//!    which is checked for existence and read by a human. An unpublished limit is
//!    a real state -- a closed one, a cost to this project rather than to a
//!    reader, a limit whose home is the recovery runbook -- and pretending
//!    otherwise would just push people to delete markers.
//! 4. An id declared `limit-not-on-page` must **not** appear on the limits page.
//!    Publishing something the plan says is unpublished is drift in the other
//!    direction, and it is the one that leaves a stale "no surface carries it"
//!    behind.
//! 5. **The comparison must not be between two empty sets.** Rules 1 to 4 are
//!    all comparisons, and every comparison between nothing and nothing holds.
//!    If §6 yields no published ids *and* the limits page yields no markers --
//!    because a restructure left the delimiters wrapped around a stub, because
//!    the marker spelling was renamed on both sides at once, because somebody
//!    converted every marker to `limit-not-on-page` and stripped the page --
//!    then a green build would print the words *"enumerates the same limit
//!    set"* while holding nothing at all. That is refused explicitly, by
//!    [`cross_check_limit_set`]'s vacuity guard, and the green line prints the
//!    number of limits it matched so a shrinking set is visible in a CI log
//!    rather than only in a failure that never comes. This repository has a
//!    cautionary example of the other choice in `.github/vkms/run-advisory.sh`,
//!    which exits 0 on a declared skip exactly as on a real probe.
//!
//! Each region delimiter must also appear exactly once **in its own document**,
//! which is a rule this check earned the hard way: §6's own prose describing the
//! mechanism spelled the delimiters out in full, which moved where the region
//! began and reported all 39 published ids as missing. See [`limit_set_region`].
//!
//! # More than one enumerating document, and why that is not a hole
//!
//! The comparison runs against the **union** of every region in [`ENUMERATORS`],
//! because a limit's enumerating home is the plan document that owns the work
//! which created it -- WS-E's §6 for WS-E's limits, the Phase-2 plan's §7 for
//! Phase-2 confinement's. Writing one workstream's limit into another's
//! enumeration would send the next sweep to the wrong document's surface table,
//! which is the failure the enumeration exists to prevent. [`ENUMERATORS`]
//! carries the full argument, including what would have been a carve-out here
//! and is not, and the three rules the multi-home shape adds: a home must
//! enumerate something, an id is declared by exactly one home, and rule 2 runs
//! over every home.
//!
//! # What the set cross-check deliberately does NOT hold
//!
//! * **It cannot see an unmarked paragraph on the limits page.** A WS-E limit
//!   published there with no marker comment is not held in either direction --
//!   the gate has no way to tell it apart from the page's many Phase-1 entries,
//!   which are inherited rather than created by WS-E and carry no marker on
//!   purpose. Rule 2 covers the §6 side of this; there is no equivalent for the
//!   page, because "which paragraphs are WS-E's" is not derivable from the text.
//! * **It says nothing about `README.md` or `site/index.html`.** Those are held
//!   by [`Claim::surfaces`], claim by claim, and the plan document's own surface
//!   table records that the site carries a stated subset. Set equality against a
//!   deliberate subset would be wrong.
//! * **It does not check that a marker sits next to the right paragraph.** An id
//!   moved to a neighbouring entry still satisfies every rule here. What it holds
//!   is that the two documents enumerate the same limits, not that each pair of
//!   entries says the same thing -- which is the [`Claim`] table's job, for the
//!   claims that have a row there.
//! * **One id is one anchor.** Where the limits page splits a §6 limit across
//!   several paragraphs, the marker goes on the primary one; the others are
//!   unheld. Where a §6 bullet covers two limits it carries two markers, which
//!   is the shape to reach for rather than widening an id's meaning.
//! * **A marker whose prefix is split across two source lines fails loudly.**
//!   Matching runs over whitespace-[`normalize`]d text, so a wrapped marker is
//!   still found for the set comparison; rule 2's structural scan is line-based
//!   and would report the item as unmarked. That is a false red rather than a
//!   false green, which is the direction to be wrong in, and it is stated here
//!   rather than discovered.
//! * **Rule 2 sees list items, not paragraphs.** A limit written as a bare
//!   paragraph inside the region, or as a nested sub-item under a marked one, is
//!   not a top-level item and is not required to carry a marker. Nesting is
//!   deliberate -- `drm-has-no-ci-gate` has eight sub-bullets under one marker,
//!   and demanding a marker on each would either widen an id's meaning or invent
//!   eight ids for one limit. A limit added as a bare paragraph is the residual
//!   hole, and the only thing standing in front of it is review.
//!
//! Two false greens that were in the first version of this scan and are not in
//! this one, recorded because the fix is invisible once made and the temptation
//! to simplify it back is real. Rule 2's structural scan used to match the
//! region delimiters by whole-line equality, so a delimiter line that gained any
//! other text -- a trailing comment, an anchor -- left `inside` false for the
//! whole document: the set comparison still ran (it works on normalised text and
//! does not care), rule 2 held nothing, and nothing said so. It now matches by
//! containment and reports a REGION failure if the structural scan never finds
//! the region that [`limit_set_region`] did. And it used to recognise only `- `
//! as a list item, so a limit added with Markdown's equally valid `* ` was
//! invisible to it; [`is_top_level_item`] now takes every list marker Markdown
//! has.
//!
//! # The uncovered set, written down rather than assumed empty
//!
//! #172's audit enumerated the duplicated claims in this repository. Most are
//! now held. These are not, and each one is here so the uncovered set is a
//! **list** rather than an absence somebody infers from a green build.
//!
//! **This list is itself a published claim, and it has already gone stale
//! once**: it opened with a count of uncited tracker issues that the same
//! branch's own edits falsified before the branch was committed. So every
//! number below is now dated as the reading it is, and the list is restated for
//! readers who will never open this file, under
//! `docs/book/src/limits.md`'s heading *"What holds this page to the others,
//! and what it does not"* -- a gap written down only in Rust source is written
//! down only for the person who already knows.
//!
//! * **A surface the tables do not NAME is unheld entirely, however many
//!   claims it repeats.** Coverage is per-(row, path): a row lists the paths it
//!   holds, so a page nobody listed is invisible to the gate rather than
//!   partially checked. Two live instances at the time of writing:
//!   `docs/ARCHITECTURE.md` and `docs/plan/02-phase-2-semantic-epochs.md` both
//!   restate the five-kernel figure in the `kernels-measured` register, and
//!   neither is a `Rendering`; both can be edited to "three" with a green
//!   build. A NEW published page inherits the same hole on the day it is added,
//!   which is the likelier future case. What would close it is a rule that
//!   every path under the published roots must appear in some row -- the
//!   `test_census` shape from #288 -- and that is not built here.
//! * **Text a reader never sees still satisfies an anchor.** The scan reads
//!   bytes, not rendered output, so wrapping a block in `<!-- -->` on
//!   `site/index.html`, or fencing it on a Markdown page, leaves every anchor
//!   and every rendering green while the published page says nothing. The
//!   failure direction is the one #172 calls worst: the gate reports agreement
//!   across surfaces that no longer publish the claim at all.
//! * **Whether the wlcs number is still true of the shim.** The `wlcs-counts`
//!   row holds five prose surfaces to each other and to
//!   `shim/wlcs/README.md`'s canonical block, and `wlcs-version` now holds the
//!   release those counts were taken against -- the component that README's own
//!   conclusion calls load-bearing, and the one nothing held until it was
//!   pointed out. Neither re-runs wlcs: the advisory job uploads its summary and
//!   commits nothing, so unlike `tests/kernel-matrix/rows/` there is no in-tree
//!   artefact to compare against. A checked-in row file is the stronger fix and
//!   it is #157's.
//! * **The 8/49-on-1.7.0 comparison.** Three surfaces carry it as the reason a
//!   bare ratio means nothing, and it comes from the same canonical block, but
//!   the block states it as prose rather than as a counts line -- there is no
//!   `total=`-shaped literal to read it out of. Adding one would mean editing
//!   `shim/wlcs/README.md`'s report of a run to suit this gate, which is the
//!   wrong direction of fix.
//! * **The hardware evidence's dates and kernel strings.** The runs themselves
//!   are claims about the world and no runner can hold them -- see the bullet
//!   below, which is correct about the runs. It is over-broad about the TEXT:
//!   `7.1.5-arch1-2` and `2026-08-09` are strings, they are restated on four
//!   surfaces, and they COULD be derived from an artefact under
//!   `docs/book/src/`. They are not, because no single file in the tree is
//!   their canonical home yet, and inventing one as a side effect of #172 would
//!   be worse than naming the gap.
//! * **"The suite has only ever run on two machines."** Nothing derives it, and
//!   it is the claim most likely to drift silently the day a third machine runs
//!   anything -- there is no artefact that counts machines. Note also the
//!   near-collision it creates for any future anchor: `limits.md` says *"still
//!   exactly two"* about the SUITE and *"this repository's two machines"* about
//!   BYTE-STABILITY, which are two claims sharing a phrase.
//! * **`_toml_string_array`'s behavioural mirror.** See [`MIRRORS`], which
//!   lists it and says why a string comparison cannot hold it.
//! * **Most open `known-limit` issues are cited on no published surface** --
//!   six of thirteen when the tracker was last read, on 2026-08-16 (#282, #253,
//!   #252, #172, #171, #167). That is a reading of a tracker on a date, not a
//!   property of this tree, and nothing offline can keep it true: the number
//!   moves when somebody who is not touching this repository opens an issue.
//!   Re-read it with `cargo xtask limits-check --tracker` rather than trusting
//!   this line. [`uncited_issues`] holds the other direction only, and the
//!   report is [`tracker_report`], which is advisory by design.
//! * **A surface can still state a value in a register no `Rendering` names.**
//!   [`scan_surface`] holds every occurrence of a register the table knows
//!   about; it cannot hold a paragraph that invents a new one. That is the
//!   residual half of the self-drift hole and it is closed by adding a
//!   `Rendering`, not by widening a context until it matches prose it was never
//!   about.
//!
//! # What it deliberately does NOT check
//!
//! * **It is not a docs linter.** #172's own scope note: *"the claim set is
//!   small, known, and slow-moving."* [`CLAIMS`] is a hand-written table and it
//!   is meant to stay one.
//! * **It cannot check a claim about hardware.** "One machine, one GPU, one
//!   panel, one kernel" and "the runbook has been executed twice" are claims
//!   about the world, and no runner can hold them. Those claims are published
//!   with dates and a named human run, and this gate is silent about them --
//!   stated here rather than left to be inferred from an absence.
//! * **It matches substrings, not meaning.** Rewording an anchor for clarity
//!   trips it, which #224's own risk list names: *"a claim-string drift check is
//!   brittle... which trains people to weaken the check."* Two mitigations, both
//!   aimed at that specific failure: every anchor is a short noun phrase or an
//!   interface name rather than a sentence, and matching runs over
//!   whitespace-[`normalize`]d text so a prose reflow -- which changes nothing a
//!   reader sees, and moves a newline into the middle of every anchor on a
//!   76-column surface -- cannot turn the build red. What still trips it is
//!   deleting or rewording the claim, which is the event worth catching.
//! * **[`normalize`] collapses whitespace and decodes nothing.** No case
//!   folding, no entity decoding, no regex. `site/index.html` is full of
//!   `&mdash;` and `&rsquo;`, and a [`Rendering`] that happened to straddle a
//!   tag boundary would produce a false RED -- the safe direction, but one that
//!   gets blamed on the gate rather than on the render function. Every rendered
//!   form here is deliberately short and free of markup it does not own.
//!
//! # Why this is not `--check` on a generator
//!
//! `session-matrix` generates its page and compares byte-for-byte, which is a
//! stronger gate. It works there because the page has one register and one
//! author. It does not transfer here: `README.md`, `docs/book/src/limits.md` and
//! `site/index.html` want the *same fact* in three different registers -- a
//! contributor's summary, an argued limits page, a landing page's warning box --
//! and #172 names that as precisely why option (a) is the biggest change of the
//! three. Anchoring a phrase in each register is what can be done today without
//! taking that decision on #172's behalf.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::build_output::BuildOutput;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One published surface that must carry a claim, and the string that anchors
/// it there.
///
/// `needle` is deliberately short. It is the load-bearing noun phrase or the
/// interface name -- never the whole published sentence, because the three
/// surfaces state the same fact in three registers and a sentence common to all
/// three would have to be the blandest of the three.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub path: &'static str,
    pub needle: &'static str,
}

/// An assertion about the tree that must hold for the published claim to be
/// true. Every variant names `means`: what a reader should conclude from the
/// check passing, so a failure message can say what became false rather than
/// only which grep stopped matching.
#[derive(Clone, Copy)]
pub enum Evidence {
    /// `needle` must appear in `path`.
    Contains {
        path: &'static str,
        needle: &'static str,
        means: &'static str,
    },
    /// `needle` must appear in **no** file under any of `roots` (recursively).
    /// `roots` may name files as well as directories.
    AbsentFrom {
        roots: &'static [&'static str],
        needle: &'static str,
        means: &'static str,
    },
}

/// One published claim: where it is said, and what makes it true.
pub struct Claim {
    /// Stable id, used in failure output and nowhere else.
    pub id: &'static str,
    /// The claim in one sentence, as the gate understands it.
    pub says: &'static str,
    /// The issue this claim came from, or an explicit statement that it has
    /// none. #224's task 6 requires every published WS-E bullet to name an
    /// issue or say plainly that it has none; carrying it here means the gate
    /// cannot hold a claim whose provenance nobody wrote down.
    ///
    /// Since #172 this field is **checked**, not only printed: every `#N` it
    /// names must be cited on at least one of the claim's own surfaces. See
    /// [`uncited_issues`] for what that property is and, more importantly,
    /// what it is not.
    pub issue: &'static str,
    pub surfaces: &'static [Anchor],
    pub evidence: &'static [Evidence],
}

// ---------------------------------------------------------------------------
// Derived values -- #172's one new concept
// ---------------------------------------------------------------------------

/// Where a derived value is read from.
pub enum Source {
    /// Read one or more values out of a single file, each following a literal.
    ///
    /// Matching here is **raw**, not [`normalize`]d, and that is the opposite
    /// choice from [`Anchor`] on purpose: a canonical value lives in a
    /// constant declaration, a fenced code block or a `#define`, none of which
    /// reflow. Reading it raw is what lets a `\n`-prefixed literal disambiguate
    /// `\ntotal=` (the counts line) from `` `total=/passed=` `` (prose about
    /// the format) in the same file.
    File {
        path: &'static str,
        reads: &'static [Read],
    },
    /// Count the files directly under `dir` whose name ends with `suffix`.
    ///
    /// The value is that count in decimal. This exists for exactly one shape:
    /// a published number that is the size of a checked-in set
    /// (`tests/kernel-matrix/rows/`), where the honest canonical source is the
    /// set itself and not a second copy of its size.
    FileCount {
        dir: &'static str,
        suffix: &'static str,
    },
}

/// One value to read out of a [`Source::File`].
pub struct Read {
    /// The literal the value immediately follows. Must occur **exactly once**
    /// in the file: a second occurrence means the first one wins silently, and
    /// a canonical value that silently picks a side is not canonical.
    pub after: &'static str,
    pub shape: Shape,
}

/// How a value's end is found.
pub enum Shape {
    /// The run of ASCII digits immediately following `after`.
    Digits,
    /// Everything from `after` up to (not including) the next occurrence of
    /// this terminator.
    UpTo(&'static str),
}

/// One surface, and the form in which it prints the derived value(s).
pub struct Rendering {
    pub path: &'static str,
    /// The values, in `reads` order, rendered the way this surface prints
    /// them.
    ///
    /// Keep every rendered form **short and tag-free**. [`normalize`] collapses
    /// whitespace and decodes nothing, so a rendering that straddles an HTML
    /// tag boundary or an `&mdash;` on `site/index.html` produces a false RED
    /// -- the safe direction, but one that gets blamed on the gate.
    pub render: fn(&[String]) -> String,
    /// **The value-free part of `render`'s output, and the reason a surface
    /// cannot drift from itself behind a green build.**
    ///
    /// A `contains` test asks *"does this value appear somewhere on the
    /// page"*, and answers yes as soon as one occurrence is right. That is not
    /// the property this table claims. `docs/book/src/limits.md` states the
    /// AppArmor run's kernel in two places several hundred lines apart and the
    /// Landlock floor in two places six hundred lines apart; `site/index.html`
    /// asserted that the per-kernel matrix *"has not been built"* in one
    /// paragraph and cited its measurements ninety lines later. A first-hit
    /// check is blind to exactly the failure this gate exists for.
    ///
    /// So a rendering names the literal that identifies **every** place on this
    /// surface where the value belongs -- the register, not the value -- and
    /// [`scan_surface`] holds all of them:
    ///
    /// * it must occur **exactly once** inside the rendered form, which is what
    ///   fixes the value's offset relative to it (refused at run time
    ///   otherwise);
    /// * it must be **value-free**, so that rendering a different value leaves
    ///   it unmoved (`every_context_is_value_free`);
    /// * every occurrence of it on the surface must sit inside a full,
    ///   canonical rendering. A second, disagreeing occurrence is a failure
    ///   naming both line numbers.
    ///
    /// **Choosing it is a judgement, and both ways of getting it wrong are
    /// visible.** Too narrow -- a context so specific that only the one correct
    /// occurrence carries it -- and a stale sibling stays invisible; that is the
    /// residual hole, and it is the same class as an [`Anchor`] needle that is
    /// too specific. Too broad -- `` ` of them` `` on `site/index.html`, `` `**
    /// ` `` in any Markdown -- and honest sentences that were never about this
    /// value go red. A false RED is the safe direction, but it is also the one
    /// that gets the gate deleted, so prefer the narrowest literal that still
    /// covers the whole register: `"total="` for the wlcs counts (every counts
    /// line on the page), `"** in this"` for the floor (both of the limits
    /// page's statements of it), not `"6"` and not the whole sentence.
    ///
    /// Where one surface legitimately carries the value in two different
    /// registers -- the limits page reports the AppArmor run once in a table
    /// header and once in the `unconfined_knob` paragraph -- **model the second
    /// register as its own `Rendering` with its own context**. Two rows for one
    /// path is the intended shape, not a workaround.
    pub context: &'static str,
}

/// One value with **one** canonical definition and several renderings of it.
///
/// This is the concept [`Anchor`] cannot express and #172 needs. An anchor is a
/// literal: where every surface shares one string (`60 KiB`, `16 realms`),
/// editing the table's needle cascades to all of them and the mechanism works.
/// Where the surfaces legitimately render the same value differently --
/// `passed=3` against `3/180`, `pub const LANDLOCK_MIN_ABI: u32 = 6;` against
/// four published literal **6**s -- the cascade breaks, and the value can go
/// stale on one surface with the gate green. That hole was live on the floor
/// claim, and it is why the 1020/1022 attribution survived a commit whose whole
/// purpose was fixing that attribution.
///
/// A `Derived` row closes it: change the canonical definition alone and
/// **every** rendering fails at once, so the fix is to move the surfaces rather
/// than to move a needle in this table.
///
/// The shape is not an invention. `crates/xtask/src/isolation_matrix.rs`'s
/// `Constants::cross_check_harness` already does exactly this for one pair of
/// files, and `crates/xtask/src/test_census.rs`'s `published_count_sentence`
/// renders a number-word into a sentence and matches it against the page. This
/// generalises the two.
pub struct Derived {
    pub id: &'static str,
    pub says: &'static str,
    /// Same contract as [`Claim::issue`] — but NOT checked the same way, and
    /// the difference is worth stating rather than discovering. The citation
    /// half (`uncited_issues`) runs over [`CLAIMS`] only; a `DERIVED` row's
    /// `issue` is read out in failure text and nowhere else, so a wrong number
    /// here is latent rather than red. Every shipped row happens to be correct
    /// today; nothing keeps it that way.
    pub issue: &'static str,
    pub source: Source,
    pub renderings: &'static [Rendering],
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

const LIMITS: &str = "docs/book/src/limits.md";
const README: &str = "README.md";
const SITE: &str = "site/index.html";
/// The reporter-facing surface. It carries a limit only where a reader could
/// otherwise spend a weekend proving something this project already publishes
/// -- a refusal to start being designed behaviour is exactly that shape.
const SECURITY: &str = "SECURITY.md";

// The surfaces #172 added, and the reason each one is here rather than being
// left to care. Every one of them carries a claim #172 names, and before #172
// none of them was a surface at all.
//
// A WARNING that has to be read before the next row is added, because the
// module docs above argue at length (see "And a third thing, which is a set and
// not a string") that the plan documents and the limits page are written in two
// registers which must NOT be forced to converge, and that anchoring a phrase
// across them is exactly what that passage refuses:
//
//   The two plan-document anchors below are safe because they pin CHECKBOX
//   STATE and STATUS WORDS -- `- [ ] **Join the Open Invention Network**`,
//   `- [x] **DCO, not CLA**` -- and not argued prose. A checkbox is a fact
//   about the world with two values; a paragraph is a register. Anchoring the
//   first is a drift gate. Anchoring the second is the brittleness #224's risk
//   list names by name, and this precedent is not licence for it.
/// The licensing map. Normative for the path->license question, and the only
/// place the SPDX-coverage caveat is stated to a packager.
const NOTICE: &str = "NOTICE";
/// The community workstream's checklist. Its ticks are the project's own record
/// of what is done, and until #172 nothing held a single one of them to the
/// tree -- which is how it came to say, for months, that there was no
/// `CONTRIBUTING.md` and that nothing enforced sign-off while both existed.
const COMMUNITY: &str = "docs/plan/12-workstream-community.md";
/// The fuzzing harness's own README, which is where the "not scheduled yet"
/// half of the soak claim is stated in the first person.
const FUZZ_README: &str = "fuzz/README.md";
/// The wlcs harness's README. It is the CANONICAL statement of the conformance
/// counts -- the only surface carrying all four numbers plus the wlcs version
/// plus the date -- and everything else quotes it.
const WLCS_README: &str = "shim/wlcs/README.md";
/// The AppArmor profile. Its header is the canonical statement of which kernel
/// the profile's one green run was on, and the header itself claims that
/// `limits-check` anchors that bound on four pages.
const PROFILE: &str = "packaging/apparmor/vitrind";
/// The decision log. A `**Status:**` line is a status word, not a register --
/// see the warning above.
const DECISIONS: &str = "docs/plan/20-decision-log.md";

/// Directories whose contents are third-party and must never satisfy or break
/// an [`Evidence::AbsentFrom`] check. `shim/subprojects/` is vendored wlroots,
/// pixman, libxkbcommon and v4l-utils; every one of them mentions X11, touch
/// and accessibility, and none of them is this project's code.
///
/// **This list is about provenance, not about tracking.** Every path in it
/// happens also to be gitignored today, and [`BuildOutput`] would therefore
/// skip it too -- but that is a coincidence of this tree, not the rule. A
/// vendored tree that somebody checks in tomorrow is still not this project's
/// code, and this list is what says so. The converse hole is the one #295
/// reports: a build directory is not named here, cannot be, and needs git to
/// find. Both skips, for two different reasons.
const VENDORED: &[&str] = &["shim/subprojects", "target", "docs/book/book"];

// ---------------------------------------------------------------------------
// The claim table
// ---------------------------------------------------------------------------

/// The WS-E claims this gate holds.
///
/// **This is a subset of what WS-E publishes, on purpose.** A claim earns a row
/// here when it is checkable against the tree by a machine with no display
/// controller. The rest of the limit set -- the hardware runs, the counts, the
/// dated measurements -- is published with its date and its single machine
/// named, and is checkable only by a human repeating the run.
pub const CLAIMS: &[Claim] = &[
    Claim {
        id: "accessibility-absent",
        says: "There is no accessibility of any kind; no AT-SPI2 bus is ADVERTISED to a realm \
               (advertisement, not reachability); and the agent-facing semantic tree is not a \
               substitute for AT-SPI.",
        issue: "No issue, deliberately: #224 decided this is an exclusion rather than a \
                deferral, and a deferral is what an issue would imply. #175 (E2.1, the \
                AccessKit/AT-SPI2 semantic bridge) is the thing it must not be read as.",
        // TWO anchors per surface, and the second one is not decoration. The
        // obvious anchor, `AT-SPI`, ALREADY MATCHED `README.md` before this
        // claim was published there -- the "Why" section names AT-SPI2 as an
        // unauthorized backdoor, which is a different sentence about a
        // different thing. An anchor that a surface satisfies by accident is a
        // check that cannot fail, so the claim is pinned to the two phrases
        // that exist only because this limit is published: the exclusion
        // itself, and the non-substitution sentence #224 calls load-bearing.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "No accessibility of any kind",
            },
            Anchor {
                path: LIMITS,
                needle: "does not make Orca work",
            },
            Anchor {
                path: README,
                needle: "No accessibility of any kind",
            },
            Anchor {
                path: README,
                needle: "does not make Orca work",
            },
            Anchor {
                path: SITE,
                needle: "No accessibility of any kind",
            },
            Anchor {
                path: SITE,
                needle: "does not make Orca work",
            },
        ],
        evidence: &[
            Evidence::AbsentFrom {
                roots: &[
                    "crates/vitrin-core/src",
                    "shim/src",
                    "shim/include",
                    "protocol/vitrin-v0.xml",
                    "sdk/python/src/vitrin_os",
                ],
                needle: "AT-SPI",
                means: "no AT-SPI bridge, bus or client exists in the core, the shim, the wire \
                        protocol or the SDK. If this fires, something now speaks AT-SPI and the \
                        published 'no accessibility of any kind' has become false. NOTE the \
                        roots: `crates/xtask` is deliberately outside them, because this table \
                        is prose about the absence and not an implementation of it -- which is \
                        why the limits page's own grep instruction names these roots rather \
                        than saying 'this repository'.",
            },
            // THIS ROW IS THE CORRECTION THAT MATTERS MOST IN THIS TABLE.
            // All three surfaces once published "no AT-SPI bus REACHABLE inside
            // a realm", which is false in exactly the way the portals claim
            // below gets right, and false against the same file: `spawn.rs`
            // says the session bus is unadvertised and NOT unreachable, and
            // `org.a11y.Bus` is activated ON the session bus. `RESERVED_ENV`
            // holds six names (P2.6.2 added HOME) and neither
            // DBUS_SESSION_BUS_ADDRESS nor AT_SPI_BUS_ADDRESS is among them,
            // so both are allow-listable in `realm.toml`. Pinning the same sentence here as `no-portals` does
            // means the accessibility claim cannot silently re-acquire the
            // stronger word: if `spawn.rs` ever earns it, this evidence row
            // fails and the page has to be re-read before the wording moves.
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn.rs",
                needle: "That is advertisement, not reachability",
                means: "the core's own spawn docs still record that an unadvertised session bus \
                        is NOT an unreachable one. The published accessibility claim says \
                        'advertised' rather than 'reachable' because of this sentence; if it \
                        goes away, re-read the page before changing the word.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/realm.rs",
                needle: "pub(crate) const RESERVED_ENV: [(&str, &str); 6]",
                means: "RESERVED_ENV still holds exactly six names, none of them a bus \
                        address, so DBUS_SESSION_BUS_ADDRESS and AT_SPI_BUS_ADDRESS remain \
                        allow-listable in realm.toml. This is why the claim is a missing \
                        service and never a confinement. The count moved from five to six at \
                        P2.6.2 (#186), which reserved HOME because a confined realm's \
                        filesystem does not contain the operator's home directory -- a name \
                        the core decides, not a bus address, so the claim above is unchanged.",
            },
        ],
    },
    Claim {
        id: "no-x-server",
        says: "There is no X server anywhere in this stack, and a realm's app is handed no \
               DISPLAY at all.",
        issue: "#221 (WS-E.4.1, closed) measured it; Phase 3 E3.2 would close it and has no \
                issue of its own yet.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "no X server anywhere in this stack",
            },
            Anchor {
                path: README,
                needle: "no X server anywhere in this stack",
            },
            Anchor {
                path: SITE,
                needle: "no X server anywhere in this stack",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "shim/meson.build",
                needle: "xwayland=disabled",
                means: "the shim's wlroots subproject is built with XWayland disabled, so the \
                        shim cannot start an X server even if asked.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/realm.rs",
                needle: "\"XAUTHORITY\"",
                means: "XAUTHORITY is in RESERVED_ENV, so it cannot be allow-listed into a \
                        realm and cannot reach the app with a host value.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/realm.rs",
                needle: "\"DISPLAY\"",
                means: "DISPLAY is in RESERVED_ENV for the same reason. Both together are why \
                        xterm in a realm dies before it draws.",
            },
        ],
    },
    Claim {
        id: "no-layer-shell",
        says: "There is no zwlr_layer_shell_v1, so no client bar, launcher, notification or \
               OSD can map.",
        issue: "#215 (WS-E.2.3, closed) shipped the core-owned status strip as the \
                replacement; the missing realm-view inset it names is still open inside that \
                closed issue's text and has no issue of its own.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "zwlr_layer_shell_v1",
            },
            Anchor {
                path: README,
                needle: "zwlr_layer_shell_v1",
            },
            Anchor {
                path: SITE,
                needle: "zwlr_layer_shell_v1",
            },
        ],
        evidence: &[Evidence::AbsentFrom {
            roots: &["shim/src"],
            needle: "layer_shell",
            means: "the shim advertises no layer-shell global. If this fires, a bar can map \
                    and the published claim is stale in the pessimistic direction.",
        }],
    },
    Claim {
        id: "no-idle-inhibit",
        says: "Idle inhibition is not served, so full-screen video will blank the screen.",
        issue: "#223 (WS-E.4.3, open) owns it; it reopens on a paired IDL + prose edit on \
                track:protocol.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "zwp_idle_inhibit_manager_v1",
            },
            Anchor {
                path: README,
                needle: "zwp_idle_inhibit_manager_v1",
            },
            Anchor {
                path: SITE,
                needle: "zwp_idle_inhibit_manager_v1",
            },
        ],
        evidence: &[Evidence::AbsentFrom {
            roots: &["shim/src", "protocol/vitrin-v0.xml"],
            needle: "idle_inhibit",
            means: "neither the shim global nor the shim->core wire verb exists. Both are \
                    needed, so either one appearing means this claim needs re-reading.",
        }],
    },
    Claim {
        id: "clipboard-bound",
        says: "The cross-realm clipboard EXISTS and is published as a bound: one core-held \
               slot, text/plain;charset=utf-8 only, 60 KiB, two human gestures.",
        issue: "#213 (WS-E.2.1, closed). This is the claim #224's own body got wrong -- it \
                asked for 'no cross-realm clipboard of any kind' and was corrected on \
                2026-08-12.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "60 KiB",
            },
            Anchor {
                path: README,
                needle: "60 KiB",
            },
            Anchor {
                path: SITE,
                needle: "60 KiB",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/clipboard.rs",
                needle: "MAX_CLIPBOARD_BYTES: usize = 61440",
                means: "the published 60 KiB is the constant the core enforces, not a rounded \
                        recollection of it.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/clipboard.rs",
                needle: "CLIPBOARD_MIME: &str = \"text/plain;charset=utf-8\"",
                means: "the one-type allow-list is still one type. A second MIME here widens a \
                        published cross-realm channel.",
            },
        ],
    },
    Claim {
        id: "realm-cardinality",
        says: "A session holds at most 16 SIMULTANEOUSLY LIVE realms: no principal can end \
               one, and a slot returns only when the realm's own app exits.",
        issue: "#208 (WS-E.1.2, closed) raised the cap; #234 (open) is why no principal can \
                end a realm -- revocation, disconnect and the dead-man switch all leave it \
                running. #234 is NOT about the slot never returning.",
        // The second anchor is the correction. This row originally held only
        // "16 realms", and under that anchor README.md and site/index.html both
        // published that sixteen launches spend the cap "for good" / "for the
        // life of the session" -- false of `main`, and invisible to a gate that
        // only looked for the number. The qualifier is now anchored on every
        // surface, so the cap and the condition under which it is spent cannot
        // drift apart again.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "16 realms",
            },
            Anchor {
                path: LIMITS,
                needle: "own app exits",
            },
            Anchor {
                path: README,
                needle: "16 realms",
            },
            Anchor {
                path: README,
                needle: "own app exits",
            },
            Anchor {
                path: SITE,
                needle: "16 realms",
            },
            Anchor {
                path: SITE,
                needle: "own app exits",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/realm.rs",
                needle: "MAX_REALMS: usize = 16",
                means: "the published cardinality is the constant. Raising the constant without \
                        the page is what this catches.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/realm.rs",
                needle: "fn occupies_capacity",
                means: "an exited realm stops costing capacity, so the cap counts LIVE realms \
                        rather than launches. If this predicate goes away the cap becomes \
                        per-session-spent and all three surfaces are then wrong in the other \
                        direction.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/session.rs",
                needle: "capacity_used() + queued.len() >= crate::realm::MAX_REALMS",
                means: "the launch refusal reads capacity_used(), NOT len(). This is the line \
                        that makes 'sixteen simultaneously live' the true statement and \
                        'sixteen launches' the false one.",
            },
        ],
    },
    Claim {
        id: "one-output",
        says: "The core models exactly one output, and a second connected display is a \
               startup refusal.",
        issue: "#218 (WS-E.3.2, closed) built the refusal; the hot-plug gap it names has no \
                issue.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "exactly one output",
            },
            Anchor {
                path: README,
                needle: "exactly one output",
            },
            Anchor {
                path: SITE,
                needle: "exactly one output",
            },
        ],
        evidence: &[Evidence::Contains {
            path: "crates/vitrin-core/src/backend/drm.rs",
            needle: "this core models exactly one",
            means: "the one-output refusal is still in the DRM backend, in those words.",
        }],
    },
    Claim {
        id: "no-touch-no-tablet",
        says: "Touch and tablet are not served on the wire, and wl_touch is deliberately not \
               in the shim's seat capabilities.",
        issue: "#222 (WS-E.4.2, open). Both are deferrals with named reopening evidence, not \
                refusals.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "no touch and no tablet",
            },
            Anchor {
                path: README,
                needle: "no touch and no tablet",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "shim/src/globals.c",
                needle: "TOUCH IS NOT YET SERVED",
                means: "the shim still states the absence at the seat, in its own words.",
            },
            Evidence::AbsentFrom {
                roots: &["shim/src"],
                needle: "WL_SEAT_CAPABILITY_TOUCH",
                means: "the seat still advertises pointer and keyboard only. Advertising TOUCH \
                        makes a toolkit drop its pointer fallbacks, so this one matters more \
                        than its size suggests.",
            },
        ],
    },
    Claim {
        id: "band-witness-headless-only",
        says: "The trusted band's automated witness covers the headless backend and no other, \
               so the daily-driver backend's band has no machine check behind it.",
        issue: "#173 (open) covers the human half nobody has evidence for. The DRM half has \
                no issue: no runner can hold a seat, so there is nothing to file that CI \
                could ever close.",
        // Anchored on ALL THREE surfaces, and that was a correction: this row
        // originally named `band_witness` on the limits page alone, which made
        // it a check that could not fail for the thing it exists to check --
        // deleting the README bullet and the site row left the gate green,
        // verified by doing exactly that. A cross-surface claim anchored on one
        // surface is a one-surface claim. `band_witness` stays as the second
        // limits-page anchor because it is the identifier a reader greps for;
        // `asserted, not checked` is the phrase all three registers share, and
        // the site's wording was aligned to it rather than the anchor being
        // loosened to accommodate three different phrasings.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "band_witness",
            },
            Anchor {
                path: LIMITS,
                needle: "asserted, not checked",
            },
            Anchor {
                path: README,
                needle: "asserted, not checked",
            },
            Anchor {
                path: SITE,
                needle: "asserted, not checked",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/backend/headless.rs",
                needle: "band_witness",
                means: "the witness is wired into the headless backend, which is what CI runs.",
            },
            Evidence::AbsentFrom {
                roots: &["crates/vitrin-core/src/backend/drm.rs"],
                needle: "band_witness",
                means: "the witness is NOT wired into the DRM backend. If this fires, the \
                        published 'no automated witness on the trusted band's daily-driver \
                        backend' has become false and should be corrected upward -- the good \
                        direction, and still a drift.",
            },
        ],
    },
    Claim {
        id: "drm-ci-compile-only",
        says: "CI compiles the DRM backend and structurally cannot exercise it: no DRM \
               device, no seat, no GPU on a runner.",
        issue: "#218 (WS-E.3.2, closed) landed the compile rung. There is no issue for the \
                functional gate because no CI change can produce one.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "COMPILE ONLY",
            },
            Anchor {
                path: README,
                needle: "COMPILE ONLY",
            },
            Anchor {
                path: SITE,
                needle: "COMPILE ONLY",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "COMPILE ONLY - no display controller is touched",
                means: "the CI job's own name still says it compiles and does not exercise. \
                        The published sentence quotes this job, so the quote must still exist.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "--features drm-backend",
                means: "the compile rung exists. The limits page said for one release that it \
                        did not, which is the correction this gate is here to make un-repeatable.",
            },
        ],
    },
    Claim {
        id: "no-portals",
        says: "No portals: a realm is advertised no session bus, so no file chooser beyond \
               the toolkit's own, no screen share, no notifications -- and that is an absence \
               of a service, NOT a confinement.",
        issue: "#160 (E2.6/E2.7, open) is what makes the unadvertised bus actually \
                unreachable. Serving portals has no issue and is not scheduled anywhere.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "xdg-desktop-portal",
            },
            Anchor {
                path: README,
                needle: "xdg-desktop-portal",
            },
            Anchor {
                path: SITE,
                needle: "xdg-desktop-portal",
            },
        ],
        evidence: &[
            Evidence::AbsentFrom {
                roots: &["crates/vitrin-core/src", "shim/src"],
                needle: "xdg-desktop-portal",
                means: "nothing in the core or the shim talks to a portal, starts one, or \
                        advertises one.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn.rs",
                needle: "DBUS_SESSION_BUS_ADDRESS",
                means: "the core's own spawn docs still record that the bus is unadvertised \
                        and NOT unreachable. This is the half a 'no session bus' claim drops, \
                        and dropping it would contradict the sandbox entry at the top of the \
                        limits page.",
            },
        ],
    },
    // The four rows below were absent from the first draft of this table, and
    // each of them was published on every surface -- so each was a claim the
    // three surfaces could disagree about with the gate still green. Two were
    // found by deleting the README bullet and the site row and watching this
    // pass. They are in scope by this module's own criterion (checkable against
    // the tree by a machine with no display controller); nothing about them is
    // hardware-shaped, which is the only stated reason for leaving a published
    // claim out.
    Claim {
        id: "shell-crash-loses-window-management",
        says: "The shell is an unprivileged client, so killing it loses window management while \
               the core, the realms and their apps survive.",
        issue: "#211 (WS-E.1.5, closed) shipped the switcher. The gap has no issue: it is the \
                price of the shell-is-a-client invariant (D-018/D-021(4)) and nobody intends \
                to close it.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "the ability to re-aim it",
            },
            Anchor {
                path: README,
                needle: "the ability to re-aim it",
            },
            Anchor {
                path: SITE,
                needle: "the ability to re-aim it",
            },
        ],
        evidence: &[Evidence::Contains {
            path: "tests/integration/test_shell.py",
            needle: "def test_killing_the_shell_keeps_the_session_and_a_denial_moves_nothing",
            means: "the surviving-session half is asserted by a real, mock-free integration \
                    test against the shipped binaries. If this test is renamed or deleted, the \
                    published sentence has nothing behind it and says so on three surfaces.",
        }],
    },
    Claim {
        id: "lock-and-blank-do-not-stop-an-agent",
        says: "A lock and an idle blank take away the HUMAN's input and view and touch no \
               agent's authority; and a blank stops every realm's frame clock, so an observer \
               is served the pre-blank frame with no staleness signal.",
        issue: "#214 (WS-E.2.2, closed) and D-025 for the lock; #223 (WS-E.4.3, open) and \
                D-033 for the blank. Neither has an issue to close, because neither is going \
                to change: they are decisions rather than gaps.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "keeps capturing",
            },
            Anchor {
                path: LIMITS,
                needle: "no staleness signal",
            },
            Anchor {
                path: README,
                needle: "keeps capturing",
            },
            Anchor {
                path: README,
                needle: "no staleness signal",
            },
            Anchor {
                path: SITE,
                needle: "keeps capturing",
            },
            Anchor {
                path: SITE,
                needle: "no staleness signal",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/main.rs",
                needle: "\"--blank-idle\"",
                means: "the flag the published sentence names still exists and is still spelled \
                        this way. `--lock-idle` and `--blank-idle` being SEPARATE flags is the \
                        whole of 'blanking and locking are deliberately not coupled'.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/main.rs",
                needle: "\"--lock-idle\"",
                means: "the lock's own idle flag is separate from the blank's. If these ever \
                        merge, 'a dark laptop is an unlocked session' stops being true and \
                        three surfaces are stale in the pessimistic direction.",
            },
        ],
    },
    Claim {
        id: "media-keys-reach-an-app-that-cannot-act",
        says: "The brightness and volume keys are delivered to the focused realm rather than \
               dropped at intake, and a confined app cannot write /sys/class/backlight -- so \
               the human presses brightness and nothing happens.",
        issue: "No issue. It is a residual named in the input router's own comments and in \
                docs/plan/14-workstream-session-mode.md §6; the decision it waits on (a shell \
                verb, or letting the core write /sys/class/backlight, which D-030 notes DRM \
                master does not gate) has not been filed.",
        // Not on site/index.html: the site carries a stated subset and the plan
        // document's surface table records this omission by name. That is a
        // decision about a landing page's length, not a claim the site is
        // allowed to contradict.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "/sys/class/backlight",
            },
            Anchor {
                path: README,
                needle: "/sys/class/backlight",
            },
        ],
        evidence: &[Evidence::Contains {
            path: "crates/vitrin-core/src/input/mod.rs",
            needle: "the key is delivered to an app that cannot act on it",
            means: "the router still records the honest residual in its own words: delivery \
                    changed WHERE the key stops, not what it does. If this comment goes away \
                    because actuation landed, the published half-fix has become a fix and both \
                    surfaces are stale in the pessimistic direction.",
        }],
    },
    Claim {
        id: "no-key-repeat-on-drm",
        says: "A held key does not repeat on the bare-metal backend: the shim's repeat timer \
               is zero by decision and the compensating core-side repeat that decision assumes \
               was never written.",
        issue: "No issue. Found by reading the tree during the WS-E.4.4 sweep (#224) rather \
                than by using the session, and unconfirmed on hardware because CI cannot hold \
                a seat. D-028(5)/#217 is the decision whose second half is missing.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "does not repeat",
            },
            Anchor {
                path: README,
                needle: "does not repeat",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "shim/src/seat.c",
                needle: "#define VITRIN_REPEAT_RATE_HZ 0",
                means: "the shim still disables client-side repeat, so no app in a realm runs \
                        its own repeat timer.",
            },
            Evidence::Contains {
                path: "shim/src/seat.c",
                needle: "libinput synthesizes none",
                means: "the shim still states, in its own words, that off a host there is no \
                        auto-repeat to forward. Together with the zero above, that is the \
                        whole mechanism behind the published sentence. What this gate CANNOT \
                        hold is the other half -- that no core-side repeat has since been \
                        written -- because a future implementation could be named anything; \
                        the honest statement of this row's reach is that it pins the shim's \
                        two facts and leaves the core's absence to a reader.",
            },
        ],
    },
    Claim {
        id: "host-must-permit-unprivileged-userns",
        says: "`vitrind --isolation=default` refuses to start on a host that permits an \
               unprivileged user namespace and then strips the capabilities it should confer; \
               the evidence is ONE measured runner, not a distribution survey.",
        issue: "#286 owns the packaging that makes the host grant routine; #281 owns the \
                cross-kernel matrix that would turn one data point into a survey.",
        // TWO anchors per surface, and the second one is the load-bearing one.
        // The first pins the measured CAUSE; the second pins the BOUND on the
        // evidence, which is the half a later editor is most likely to drop
        // while tightening the prose -- and dropping it turns one CI runner
        // into an implied survey, which is the overclaim this row exists to
        // stop. `--isolation=off` is deliberately NOT anchored: it is one of
        // two remedies and the page must not have to be rewritten when the
        // AppArmor profile #286 attempts lands.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "apparmor_restrict_unprivileged_userns",
            },
            Anchor {
                path: LIMITS,
                needle: "not a distribution survey",
            },
            Anchor {
                path: README,
                needle: "apparmor_restrict_unprivileged_userns",
            },
            Anchor {
                path: README,
                needle: "not a distribution survey",
            },
            Anchor {
                path: SECURITY,
                needle: "apparmor_restrict_unprivileged_userns",
            },
            Anchor {
                path: SECURITY,
                needle: "not a distribution survey",
            },
            Anchor {
                path: SITE,
                needle: "apparmor_restrict_unprivileged_userns",
            },
            Anchor {
                path: SITE,
                needle: "not a distribution survey",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn/isolation.rs",
                needle: "apparmor_restrict_unprivileged_userns is 1 (Ubuntu 24.04+)",
                means: "the refusal still names this sysctl AND offers shipping an AppArmor \
                        profile beside it. If this text goes, either the diagnosis moved or the \
                        remedy became a single blessed one -- and the published pages, which \
                        deliberately describe the REQUIREMENT rather than a remedy, need \
                        re-reading before that ships.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0",
                means: "CI still modifies the runner before it exercises a single confinement \
                        gate, which is exactly why the published measurement is read from the \
                        diagnostic step BEFORE it and why every page bounds the evidence at one \
                        runner. When #286's packaging removes this step, the machine CI measures \
                        changes and every one of those sentences has to be re-derived rather \
                        than inherited.",
            },
        ],
    },
    Claim {
        id: "apparmor-profile-is-one-image-and-uninstalled",
        says: "an AppArmor profile for the requirement above EXISTS in the tree \
               (`packaging/apparmor/vitrind`) and is MEASURED WORKING on exactly one kernel on \
               one CI image. Every surface says both halves: that the profile is there and \
               measured, and that NOBODY HAS LOADED IT ON AN INSTALLED UBUNTU SYSTEM and \
               nothing in this repository installs it outside the job that measures it.",
        issue: "#286 closed the profile's own axis -- it shipped the profile AND the job that \
                measures it, and the job reported green on a stock runner. #293 owns what \
                remains: nothing installs the profile or the binaries whose paths it attaches \
                to, so `we ship a profile` is true of the repository and false of any \
                installation of it.",
        // TWO anchors per surface, and the second one is again the
        // load-bearing half. The first pins the ARTEFACT (a page that stopped
        // naming the profile's path has stopped telling a reader where to
        // look). The second pins the BOUND.
        //
        // The bound moved on 2026-08-16 and the move is the shape this row is
        // for. It used to be "has never been loaded", and its own comment said
        // that sentence was the one a later editor would delete the first time
        // the job ran green. The job HAS run green -- so the pages were
        // rewritten against what it reported and this row's anchors changed in
        // the same commit, which is exactly what that comment instructed and
        // NOT the same act as deleting the anchor to make a gate quiet.
        //
        // The new bound is the boundary that survived the measurement, and it
        // is now the sentence most likely to be tidied away: one green CI job
        // reads like general availability, and it is not. A page that keeps
        // "measured" and drops "nobody has loaded it on an installed Ubuntu
        // system" is claiming the profile works for its readers, which no run
        // in this repository has ever shown.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "packaging/apparmor/vitrind",
            },
            Anchor {
                path: LIMITS,
                needle: "on an installed Ubuntu system",
            },
            Anchor {
                path: README,
                needle: "packaging/apparmor/vitrind",
            },
            Anchor {
                path: README,
                needle: "on an installed Ubuntu system",
            },
            Anchor {
                path: SECURITY,
                needle: "packaging/apparmor/vitrind",
            },
            Anchor {
                path: SECURITY,
                needle: "on an installed Ubuntu system",
            },
            Anchor {
                path: SITE,
                needle: "packaging/apparmor/vitrind",
            },
            Anchor {
                path: SITE,
                needle: "on an installed Ubuntu system",
            },
        ],
        evidence: &[
            // This row was VACUOUS when first written, and the fix is worth
            // recording because the same trap is available to every future
            // row here. The needle was `userns,`; commenting the rule out of
            // the profile left the gate GREEN, because the profile's own
            // header comment quoted the rule while explaining it and
            // `normalize` collapses whitespace, so no amount of surrounding
            // indentation could separate the rule from its documentation.
            // The check was being satisfied by the PROSE ABOUT the grant.
            //
            // The first fix added the opening brace of the profile body to the
            // needle -- `{ userns,` -- on the theory that a structural feature
            // of the rule's POSITION is something prose has no occasion to
            // write. That theory was wrong within one edit. The profile's
            // header was rewritten to document the anchor, the new paragraph
            // named the needle verbatim, and the gate went green again with
            // the rule commented out. Prose about an anchor is prose that
            // contains the anchor.
            //
            // So the needle is now the whole of the declaration's last line
            // PLUS the rule: attachment glob, flags, brace and grant, as one
            // contiguous string. A comment cannot reproduce it by accident,
            // because reproducing a multi-line construct in a comment puts a
            // `#` between the lines and `#` survives `normalize`. Verified the
            // only way this can be verified -- comment the rule out, watch
            // this row go RED, restore -- and the profile's own header now
            // instructs the next editor to re-run that lever after touching
            // the HEADER, not only after touching the rule, because the header
            // is what broke it last time.
            // ONE needle where there were two, and the merge is the point
            // rather than tidying. The old pair was `{ userns,` for the grant
            // and `vitrin-realm-init} flags=(unconfined)` for the attachment;
            // the second is a substring of what the first should always have
            // been, so it constrained nothing the first did not, while reading
            // like a second independent check. Anchoring the declaration and
            // the rule as ONE contiguous string is strictly stronger than
            // either: it fails if the grant goes, if the helper leaves the
            // glob, if the install path moves, if the flag changes, or if
            // anything is inserted between the brace and the rule.
            //
            // The install path is inside the needle for a reason worth
            // stating: AppArmor attaches to a resolved absolute pathname, so
            // the path in this line is not a detail of the profile, it IS the
            // profile's binding to the binaries. The `apparmor-profile` CI job
            // installs to exactly this path, and a change here that the job
            // does not follow produces a profile that loads, attaches to
            // nothing, and fails with the errno it was written to remove.
            Evidence::Contains {
                path: "packaging/apparmor/vitrind",
                needle: "profile vitrind /usr/lib/vitrin/{vitrind,vitrin-realm-init} \
                         flags=(unconfined) { userns,",
                means: "the profile still carries the one rule that makes it more than a name, \
                        still attaches at the path CI installs to, and still covers the HELPER \
                        and not only the core. Each half fails silently and differently. Lose \
                        the rule and the profile loads, attaches, and confers nothing -- the \
                        SAME errno as having no profile at all, so nothing downstream notices. \
                        Narrow the glob to `vitrind` alone and the core still starts and \
                        `--print-isolation` still clears, while every realm spawn fails one \
                        execve later at `vitrin-realm-init`, which is the process that actually \
                        issues the `unshare`. That second outcome is WORSE than shipping no \
                        profile, because the refusal moves somewhere less legible, and it is \
                        invisible to every check that only starts the core.",
            },
            Evidence::Contains {
                path: "packaging/apparmor/vitrind",
                needle: "NOBODY HAS LOADED THIS PROFILE ON AN INSTALLED UBUNTU SYSTEM",
                means: "the profile's own header still states the bound its measurement did NOT \
                        clear. This is the forcing function for the four pages above, and it \
                        has already fired once: the header used to say the profile had never \
                        been loaded at all, and the day the `apparmor-profile` job reported \
                        green that sentence had to be deleted -- which failed this gate until \
                        all four pages were rewritten against what the job actually reported. \
                        The bound this needle now holds is the next one an editor will want to \
                        tidy: one green CI job on one image reads like general availability. A \
                        file that quietly became authoritative for every host while the pages \
                        still cited one runner is drift in the direction nobody notices.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "apparmor-profile:",
                means: "the job the pages point at still exists. Every one of those four \
                        surfaces cites what it reported, and a page citing a measurement whose \
                        instrument was deleted is worse than a page citing nothing -- the \
                        numbers stay readable and stop being re-checkable on the next run.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "THE LEVER: break it, confirm FAIL, restore",
                means: "the job still removes the profile and requires the failure to come back. \
                        Without that step every green result it produces is equally explained by \
                        a runner whose policy drifted, and the job would be measuring the image \
                        rather than the profile. This is the one step that makes the whole job \
                        non-vacuous, so it is anchored rather than trusted.",
            },
        ],
    },
    Claim {
        id: "host-must-have-landlock",
        says: "`vitrind --isolation=default` refuses to start on a host whose kernel has no \
               usable Landlock -- pre-5.13, `CONFIG_SECURITY_LANDLOCK=n`, or `landlock` absent \
               from the active LSM list -- because the ruleset is in the confinement FLOOR and \
               not an optimisation on top of it. It is a SECOND host requirement, independent \
               of the userns one above, and the two remedies do not substitute for each other. \
               Since 2026-08-15 it carries a FOURTH condition with a fourth remedy: the \
               reported ABI must be at or above this build's declared floor \
               (`build.landlock_min_abi`), and a working Landlock on an older kernel is \
               refused rather than confined at a weaker rung.",
        issue: "No issue: the refusal is the intended behaviour, and unlike the userns \
                requirement there is nothing to package -- a kernel build is not something this \
                project can arrange for an operator. #281 owns the survey of which \
                distributions ship the LSM list without `landlock`, and has not run.",
        // TWO anchors per surface, and the second is the load-bearing one for
        // the same reason as the claim above: the requirement itself is the
        // easy half to keep, and the half an editor drops while tightening is
        // the warning that these are TWO requirements whose remedies must not
        // be crossed. Dropping that turns a page that tells an operator which
        // knob to reach for into one that hands them both.
        //
        // `CONFIG_SECURITY_LANDLOCK` is the requirement anchor rather than the
        // word "Landlock", which every one of these surfaces already said many
        // times over before this requirement existed -- an anchor a surface
        // satisfies by accident is a check that cannot fail.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "CONFIG_SECURITY_LANDLOCK",
            },
            Anchor {
                path: LIMITS,
                needle: "their remedies must not be",
            },
            Anchor {
                path: README,
                needle: "CONFIG_SECURITY_LANDLOCK",
            },
            Anchor {
                path: README,
                needle: "Do not cross the two remedies",
            },
            Anchor {
                path: SECURITY,
                needle: "CONFIG_SECURITY_LANDLOCK",
            },
            Anchor {
                path: SECURITY,
                needle: "The two refusals must not be confused",
            },
            Anchor {
                path: SITE,
                needle: "CONFIG_SECURITY_LANDLOCK",
            },
            Anchor {
                path: SITE,
                needle: "Do not cross the two remedies",
            },
            // The ABI floor, on every surface. A THIRD anchor per surface,
            // added when the floor landed (2026-08-15), because it is a
            // condition none of the other two anchors covers: a host can
            // satisfy every word of them and still be refused. `landlock_min_abi`
            // is the needle rather than the word "floor", which these pages
            // already used for `Mechanism::Landlock`'s membership in `FLOOR` --
            // an anchor a surface satisfies by accident is a check that cannot
            // fail.
            Anchor {
                path: LIMITS,
                needle: "build.landlock_min_abi",
            },
            Anchor {
                path: README,
                needle: "build.landlock_min_abi",
            },
            Anchor {
                path: SECURITY,
                needle: "build.landlock_min_abi",
            },
            Anchor {
                path: SITE,
                needle: "build.landlock_min_abi",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-realm-init/src/lib.rs",
                needle: "pub const LANDLOCK_MIN_ABI: u32 = 6;",
                means: "the floor is still 6, which is the number every surface above prints. \
                        It was 7 until 2026-08-16, when the owner lowered it a rung so a \
                        Debian 13 machine measured at ABI 6 stops being refused; that move cost \
                        no enforcement, because rungs 7 and 8 buy `landlock_restrict_self` \
                        flags rather than mask bits and this build passes flags = 0 (pinned by \
                        `the_floor_costs_nothing_because_the_domain_is_flat_from_six_to_eight` \
                        in crates/vitrin-realm-init/src/main.rs). Raising or lowering it \
                        changes which hosts this build refuses, so it may not move without the \
                        four pages moving with it. This row's own comment used to claim that \
                        pinning the constant as a whole line was what stopped a silent re-tune \
                        from leaving four published numbers stale. IT WAS NOT, and the \
                        correction matters more than the claim did: a re-tune fails THIS row, \
                        whoever fixes it updates the needle here, and the gate goes green with \
                        four pages still printing the old number -- nothing ever compared them. \
                        What actually holds them is the `landlock-abi-floor` row in DERIVED, \
                        which reads this constant and requires each surface's own rendering of \
                        it. Keep both: this row holds that the floor is still a startup gate, \
                        that row holds that the pages print the floor this build declares.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn/isolation.rs",
                needle: "Ok(abi) if abi >= vitrin_realm_init::LANDLOCK_MIN_ABI => \
                         Support::Available,",
                means: "the floor is still a STARTUP GATE and not merely a printed constant. \
                        Relax this comparison back to `abi >= 1` and every page above describes \
                        a refusal that no longer happens, while `--print-floor` keeps printing \
                        the number -- the overclaiming direction, and the one a reader cannot \
                        detect from the output alone.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn/isolation.rs",
                needle: "pub const FLOOR: &[Mechanism] = &[\n    Mechanism::Namespaces,\n    \
                         Mechanism::Landlock,\n    Mechanism::Seccomp,\n    \
                         Mechanism::NoNewPrivs,\n];",
                means: "Landlock is still a STARTUP GATE and not merely applied. This is the \
                        one row that decides whether the published requirement is true at all: \
                        take `Mechanism::Landlock` back out of `FLOOR` and every page above \
                        describes a refusal that no longer happens, which is the overclaiming \
                        direction. It is pinned as the whole DECLARATION rather than as \
                        `Mechanism::Landlock`, which also appears in `APPLIED` -- a mechanism \
                        can be applied without gating startup, and that distinction is exactly \
                        what this claim is about. The declaration grew from two entries to four \
                        at P2.6.4 (#188), which is why this needle spans four lines: `FLOOR` now \
                        equals `Report::tier`'s base predicate, and a needle that still named \
                        two entries would have gone red on a move that made the published claim \
                        MORE true rather than less. That is the correct behaviour -- the gate \
                        asks for a re-read, and this comment is the re-read.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn/isolation.rs",
                needle: "Landlock is a startup requirement since P2.6.3 (#187)",
                means: "the refusal still explains itself in the operator's own terms. The \
                        published pages deliberately state the REQUIREMENT rather than transcribe \
                        this text, so they do not go stale when it is reworded -- but if the \
                        remedy paragraph disappears entirely, an operator meets a bare errno and \
                        every page's `--print-isolation` advice is the only thing left.",
            },
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn/isolation.rs",
                needle: "add `landlock` to the `lsm=`",
                means: "the third condition -- a kernel that HAS Landlock and does not enable \
                        it -- is still diagnosed. It is the one of the three an operator cannot \
                        guess at from an ENOSYS, it is the reason the published requirement is \
                        three items rather than one, and it is the half of the diagnosis that a \
                        simplification of this remedy would drop first.",
            },
        ],
    },
    // -----------------------------------------------------------------------
    // #172's four named claims. They were deliberately absent until now: the
    // module header above records that they were #172's to normalise first,
    // "and gating a number before it is normalised would freeze the wrong
    // wording into CI". #172 has now taken option (b) and normalised the wlcs
    // figure, so they land here.
    // -----------------------------------------------------------------------
    Claim {
        id: "fuzz-soak-never-run",
        says: "the 24-hour fuzz soak the plan asks for has never been executed end to end and is \
               not a scheduled job. What CI runs is a corpus replay plus a short per-PR burst, \
               which is a different and much weaker statement.",
        issue: "#156.",
        // Three registers of one fact, plus the harness's own first-person
        // statement. `fuzz/README.md` is a surface rather than only evidence
        // because it is the document a contributor reaches for when wiring the
        // job, and it is the one that would go stale first the day somebody
        // does.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "24-hour fuzz soak has never been run",
            },
            Anchor {
                path: LIMITS,
                needle: "nobody has executed it end to end",
            },
            Anchor {
                path: README,
                needle: "24-hour fuzz soak has not been run",
            },
            Anchor {
                path: README,
                needle: "Nobody has run it end to end",
            },
            Anchor {
                path: SITE,
                needle: "24-hour fuzz soak that has never been run",
            },
            Anchor {
                path: FUZZ_README,
                needle: "Not scheduled yet",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: FUZZ_README,
                needle: "presently a manual, documented, reproducible procedure",
                means: "the harness still describes the soak as a hand procedure. This is the \
                        sentence somebody deletes on the day they wire the job, which is exactly \
                        the day all three published surfaces stop being true.",
            },
            Evidence::AbsentFrom {
                roots: &[".github/workflows"],
                needle: "-max_total_time=86400",
                means: "no workflow asks libFuzzer for a 24-hour run. THE BOUND ON THIS NEEDLE, \
                        stated rather than left to be discovered: it catches the soak spelled the \
                        way fuzz/README.md's own documented procedure spells it, and it would \
                        NOT catch `-max_total_time=$((24*3600))` or a matrix that sums to a day. \
                        A bare `schedule:` needle would have been the general check and is \
                        refused: .github/workflows/kernel-matrix.yml has one, so it would fail \
                        today for an unrelated reason, and a gate that is red for the wrong \
                        cause is a gate people learn to edit.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "fuzz-smoke:",
                means: "the per-PR burst all three surfaces credit still exists. This is the \
                        overclaim-in-the-other-direction half: a page saying CI replays the \
                        corpus on every PR while the soak has never run becomes false if the \
                        smoke job is deleted, and it becomes false in the direction that \
                        flatters the project.",
            },
        ],
    },
    Claim {
        id: "wlcs-advisory-and-mostly-red",
        says: "the wlcs conformance run is ADVISORY -- it never gates a pull request and the \
               shim is never built by default -- and its counts are a dated, version-pinned \
               snapshot that has not been re-measured. The four numbers themselves are held by \
               the `wlcs-counts` row in DERIVED, because the surfaces render them differently.",
        issue: "#157 asks for the re-measure. It was cited on no published surface until #172.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "wlcs conformance is advisory and mostly red",
            },
            Anchor {
                path: LIMITS,
                needle: "2026-07-25 run",
            },
            Anchor {
                path: README,
                needle: "wlcs conformance is advisory and mostly red",
            },
            Anchor {
                path: README,
                needle: "2026-07-25 run",
            },
            Anchor {
                path: SITE,
                needle: "advisory wlcs conformance at",
            },
            Anchor {
                path: SITE,
                needle: "2026-07-25 run",
            },
            Anchor {
                path: WLCS_README,
                needle: "2026-07-25 run",
            },
            // The fifth number on the canonical line, and the one that decides
            // whether the other four are a tally or a floor. `run-advisory.sh`
            // prints `status=aborted` when the runner dies part-way, and a
            // partial run's `failed=` reads exactly like a clean sweep's.
            Anchor {
                path: WLCS_README,
                needle: "status=complete",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: WLCS_README,
                needle: "have NOT been re-measured",
                means: "the canonical statement still carries its own staleness caveat. Every \
                        quoting surface says the counts were not re-measured; this is the \
                        sentence they are quoting, and it is the one that would go first if \
                        somebody tidied the canonical block into a bare table.",
            },
            Evidence::Contains {
                path: WLCS_README,
                needle: "wlcs 1.6.1-1 — the version in Ubuntu 24.04",
                means: "the canonical statement still pins the wlcs VERSION. This file's own \
                        conclusion is that a number from this harness means nothing without the \
                        wlcs version beside it -- the same shim scores 8/49 against 1.7.0 with \
                        no shim change -- so a count published without it is not a weaker claim, \
                        it is an uninterpretable one. That is why #172 made the site quote the \
                        version rather than making the canonical statement quote less.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "ADVISORY: never blocks a PR",
                means: "the job still declares itself advisory in its own name. `advisory` is a \
                        word every surface uses about this number, and the day it starts gating \
                        merges every one of them is describing a different instrument.",
            },
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "shim/wlcs/run-advisory.sh itself always exits 0",
                means: "the second half of `advisory`, and the load-bearing one: the runner \
                        script cannot fail the job even if wlcs itself does. WHAT THIS GATE \
                        CANNOT DO, and it must be read narrowly: nothing here re-runs wlcs. That \
                        job uploads its summary and commits nothing, so there is no in-tree \
                        artefact to compare against -- unlike tests/kernel-matrix/rows/. The \
                        gate holds five prose surfaces to each other and to \
                        shim/wlcs/README.md's canonical block; it can never tell you the number \
                        is still TRUE of the shim. A checked-in row file is the stronger fix and \
                        it is #157's, not this row's.",
            },
        ],
    },
    Claim {
        id: "no-oin-membership-yet",
        says: "joining the Open Invention Network is decided (D-015) and NOT yet done. The other \
               two legs of that decision -- defensive publication and the licenses' own patent \
               grants -- are in force today, and the published sentences must keep the three \
               apart.",
        issue: "#159. Cited on no published surface until #172.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "Joining the Open Invention Network is decided and",
            },
            Anchor {
                path: README,
                needle: "Open Invention Network",
            },
            Anchor {
                path: README,
                needle: "is decided but **not yet done**",
            },
        ],
        evidence: &[
            // The checkbox is the EVIDENCE, not a surface, and the distinction
            // is the whole mechanism. docs/plan/12-workstream-community.md's
            // tick is this project's own record of whether the thing has
            // happened; the two published pages are what it tells a reader. So
            // the day somebody joins OIN and ticks the box, this row goes RED
            // and stays red until both published pages have moved -- which is
            // precisely the failure #172 was opened for, closed in the one
            // direction it is known to travel.
            Evidence::Contains {
                path: COMMUNITY,
                needle: "- [ ] **Join the Open Invention Network**",
                means: "the project's own checklist still records OIN membership as NOT done. \
                        Tick that box without editing docs/book/src/limits.md and README.md and \
                        this row fails -- which is the point of putting it here rather than \
                        among the surfaces.",
            },
            Evidence::Contains {
                path: "docs/plan/20-decision-log.md",
                needle: "**Open Invention Network membership**, to be joined",
                means: "D-015 still records membership as decided-and-pending rather than as \
                        done or as abandoned. If the decision itself is reversed, `no OIN \
                        membership yet` stops being a gap and becomes a position, and both \
                        published pages need rewriting rather than deleting.",
            },
        ],
    },
    Claim {
        id: "spdx-coverage-not-machine-checked",
        says: "SPDX header coverage is asserted by convention and by review, NOT by a machine. \
               There is no REUSE-style CI gate, so a first-party source file added without a \
               header is not caught.",
        issue: "#155. Cited on no published surface until #172.",
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "SPDX header coverage is not machine-checked",
            },
            Anchor {
                path: NOTICE,
                needle: "Header coverage is not machine-checked",
            },
            // A status caveat inside a `[x]` bullet, not argued prose -- see
            // the warning beside COMMUNITY's declaration.
            Anchor {
                path: COMMUNITY,
                needle: "coverage is *not machine-checked*",
            },
        ],
        evidence: &[
            // THE TRAP THIS ROW WALKED AROUND, recorded because the same one
            // is available to every future row and this module has fallen into
            // its twin once already (see
            // `apparmor-profile-is-one-image-and-uninstalled`). The obvious
            // needle is the bare word. It is VACUOUS: .github/workflows/ci.yml
            // contains the English word "reuses" twice, in prose, so an
            // AbsentFrom on it would fail TODAY for a reason that has nothing
            // to do with licensing -- and the fix somebody would reach for is
            // deleting the check. The needles are therefore the two spellings
            // a real gate would actually have: the action, and the command.
            Evidence::AbsentFrom {
                roots: &[".github/workflows"],
                needle: "fsfe/reuse-action",
                means: "no workflow runs the REUSE action. The day one does, this gap is closed \
                        and three surfaces are lying in the direction that understates the \
                        project -- the direction #172 names as the one that erodes the trust \
                        these pages exist to earn.",
            },
            Evidence::AbsentFrom {
                roots: &[".github/workflows"],
                needle: "reuse lint",
                means: "no workflow runs the command either, whether or not it uses the action. \
                        Two needles rather than one because a hand-rolled step and a marketplace \
                        action are different spellings of the same closure, and either of them \
                        makes all three published sentences false.",
            },
        ],
    },
    Claim {
        id: "dco-is-executed",
        says: "D-012 is EXECUTED, not proposed: CONTRIBUTING.md states the sign-off policy and \
               .github/workflows/dco.yml enforces a Signed-off-by trailer per commit on every \
               pull request.",
        issue: "No issue: this is not a gap, it is the correction of one. \
                docs/plan/12-workstream-community.md carried three false statements about it in \
                one bullet -- no CONTRIBUTING.md, nothing enforcing sign-off, D-012 still \
                proposed -- for months after all three became false, and #172 is why it was \
                found. The row exists so the tick cannot silently go stale in either direction.",
        // The only claim here whose direction is the OPPOSITE of a gap, and it
        // is in this table on purpose. #224's argument -- "a page that
        // OVERSTATES a gap is as dishonest as one that hides it" -- has a
        // planning half nothing was holding: a checklist that under-reports its
        // own project is a document that sends the next reader to do work that
        // is already done.
        surfaces: &[
            Anchor {
                path: COMMUNITY,
                needle: "- [x] **DCO, not CLA**",
            },
            Anchor {
                path: README,
                needle: "Developer Certificate of Origin",
            },
            Anchor {
                path: DECISIONS,
                needle: "### D-012 — DCO, not CLA **Status:** accepted, executed",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: ".github/workflows/dco.yml",
                needle: "Executes decision D-012",
                means: "the workflow that makes the policy true rather than aspirational still \
                        exists and still says which decision it executes. Delete it and the \
                        tick above becomes the false statement the untick used to be.",
            },
            Evidence::Contains {
                path: ".github/workflows/dco.yml",
                needle: "pull_request:",
                means: "it still runs on pull requests, which is the only trigger under which \
                        `nothing enforces sign-off` stops being true. A workflow reduced to \
                        workflow_dispatch would keep the file, keep the name, and enforce \
                        nothing.",
            },
            Evidence::Contains {
                path: "CONTRIBUTING.md",
                needle: "Sign your commits (DCO, not a CLA)",
                means: "the document the workflow's own header says states the policy still \
                        states it. The plan bullet's specific false claim was that this file did \
                        not exist at all.",
            },
        ],
    },
    Claim {
        id: "per-kernel-isolation-matrix-exists",
        says: "the per-kernel half of P2.6.3's criteria EXISTS and is measured -- five \
               distribution kernels booted under QEMU with the shipped binary -- and the \
               readings are KERNEL rows taken with no distribution policy loaded, so the number \
               of distributions measured as such is still one.",
        issue: "#281 delivered it. The seccomp filter that used to be named here as P2.6.3's \
                remaining gap is #188, not #187 -- #187 is Landlock -- and it LANDED, so the \
                sentence it belonged to is now the `seccomp-is-a-deny-list` claim below rather \
                than a gap. Kept as a correction rather than deleted: this field is the \
                provenance a reader follows, and a wrong issue number in it sends them to the \
                wrong task.",
        // This row exists because of a drift that was live on `main` when #172
        // was implemented, and it is the cleanest instance of #172's thesis in
        // the repository. PR #294's doc sweep rewrote README.md,
        // docs/book/src/limits.md, SECURITY.md, the Phase-2 plan and both
        // generated pages to say the per-kernel matrix now exists -- and left
        // site/index.html's first warn paragraph saying it "has not been
        // built", ninety lines above a paragraph on the same page citing the
        // very measurement it denied. The landing page understated the project
        // to itself, in the direction #172 names by name, in a commit whose
        // purpose was removing exactly that.
        //
        // Per-surface needles rather than one shared string: `vitrind` is
        // backticked on three surfaces and wrapped in <code> on the fourth, and
        // forcing one spelling on all four would mean editing the site's markup
        // to suit a gate.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "booted under QEMU with the shipped `vitrind`",
            },
            Anchor {
                path: LIMITS,
                needle: "not a statement about the distribution that ships that kernel",
            },
            Anchor {
                path: README,
                needle: "under QEMU with the shipped `vitrind`",
            },
            Anchor {
                path: README,
                needle: "the number of *distributions* measured as such is still",
            },
            Anchor {
                path: SECURITY,
                needle: "booted under QEMU with the shipped `vitrind`",
            },
            Anchor {
                path: SECURITY,
                needle: "the number of *distributions* whose policy this repository has measured",
            },
            Anchor {
                path: SITE,
                needle: "booted under QEMU with the shipped <code>vitrind</code>",
            },
            Anchor {
                path: SITE,
                needle: "taken with no distribution policy loaded",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: ".github/workflows/ci.yml",
                needle: "cargo xtask kernel-matrix --check",
                means: "the generated per-kernel page is still held to the checked-in rows on \
                        every pull request. Without it the four sentences above cite a page \
                        nothing keeps in step with its own measurements.",
            },
            Evidence::Contains {
                path: ".github/workflows/kernel-matrix.yml",
                needle: "collect.sh --check",
                means: "the rows are still held to the KERNELS by a scheduled job. This is the \
                        other half and the surfaces depend on it: `measured` in four published \
                        registers means a boot happened, and a page held only to stale rows \
                        would satisfy every other check here while measuring nothing.",
            },
            // The claim's opposite direction, and the one that actually
            // drifted. A surface that says the per-kernel matrix has NOT been
            // built is now a red build rather than a paragraph nobody re-read.
            Evidence::AbsentFrom {
                roots: &[SITE, README, LIMITS, SECURITY],
                needle: "per-kernel one its criteria ask for has not been built",
                means: "no published surface still says the per-kernel matrix does not exist. \
                        That sentence was true until #281 landed and false afterwards, and it \
                        survived on site/index.html through the sweep that corrected every other \
                        surface. It is pinned as an absence rather than trusted to review, \
                        because review is what missed it.",
            },
        ],
    },
    Claim {
        id: "seccomp-is-a-deny-list",
        says: "The seccomp filter P2.6.4 installs is a DENY-LIST -- a named-class claim, never \
               a completeness one. It closes the rows `vitrind --print-seccomp` prints and \
               leaves the rest of the kernel's syscall surface UNENUMERATED, so a realm is \
               filtered against a named list and is NOT syscall-confined.",
        issue: "#188 (P2.6.4) landed it. The gap it closes -- `no seccomp filter` / `a realm is \
                path-confined, not syscall-confined` -- was published on six surfaces before \
                this row existed, which is exactly the shape #172 was written for: the sentence \
                had to change on all six in one commit or two of them would still be saying a \
                filter does not exist.",
        // The anchor is the WORD, not the sentence. Every surface says
        // `deny-list` because that word is the whole claim -- a reader who
        // takes "seccomp filter" for "syscall-confined" has read a deny-list
        // as an allow-list -- and pinning the word rather than a paragraph
        // leaves each surface free to keep its own register, which the module
        // docs above argue at length must not be forced to converge.
        //
        // TWO anchors per surface, and the second is not decoration: the first
        // could be satisfied by a page that says `deny-list` while still
        // claiming the realm is syscall-confined, so the second pins the
        // NEGATION that makes the claim honest.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "it is a DENY-LIST",
            },
            Anchor {
                path: LIMITS,
                needle: "the residual surface is the kernel's whole surface minus",
            },
            Anchor {
                path: README,
                needle: "it is a **deny-list**",
            },
            Anchor {
                path: README,
                needle: "it is **not** syscall-confined",
            },
            Anchor {
                path: SECURITY,
                needle: "The syscall boundary is a DENY-LIST",
            },
            Anchor {
                path: SECURITY,
                needle: "but **not** syscall-confined",
            },
            Anchor {
                path: SITE,
                needle: "it is a\n      <strong>deny-list</strong>",
            },
            Anchor {
                path: SITE,
                needle: "filtered against a named list but not\n      syscall-confined",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/spawn/isolation.rs",
                needle: "Mechanism::Seccomp,\n    Mechanism::NoNewPrivs,\n];",
                means: "seccomp and no-new-privs are still STARTUP GATES and not merely \
                        applied -- the tail of `FLOOR`'s declaration. Take them out and every \
                        surface above describes a refusal that no longer happens, in the \
                        overclaiming direction. Pinned as the tail of the declaration rather \
                        than as `Mechanism::Seccomp`, which also appears in `APPLIED`: a \
                        mechanism can be applied without gating startup, and that distinction \
                        is what `--print-floor` prints two row families for.",
            },
            Evidence::Contains {
                path: "crates/vitrin-realm-init/src/main.rs",
                needle: "let filtered = seccomp::apply()",
                means: "the helper still INSTALLS the filter before the shim's `execve`. This \
                        is the row that decides whether the published sentence is true at all: \
                        a `FLOOR` naming seccomp beside a helper that installs nothing would \
                        refuse machines and confine none of them. The call site is inside \
                        `exec_shim`, after Landlock and after `close_range`, because a filter \
                        cannot be removed once installed.",
            },
            Evidence::Contains {
                path: "crates/vitrin-realm-init/src/seccomp.rs",
                needle: "pub enum EscapeClass {",
                means: "every row still names its escape class through a CLOSED vocabulary \
                        rather than a free-text field. #188 asks that `a lint or test asserts \
                        no row has an empty class field`, and a non-empty string is trivially \
                        satisfied by a row naming nothing real; the enum makes an unnamed class \
                        unrepresentable and `every_class_label_is_in_the_prd` holds all eight \
                        labels to `docs/PRD.md` verbatim.",
            },
            Evidence::Contains {
                path: "tests/integration/test_real_seccomp.py",
                needle: "reported NOT DEMONSTRATED rather than counted",
                means: "the gate still runs a POSITIVE CONTROL per row and refuses to count a \
                        row whose syscall already fails outside a realm. Without it the \
                        published `13 rows` would be a count of denials rather than of \
                        confinement, which is the overclaim `docs/plan/02-phase-2-semantic-\
                        epochs.md` §4 names by name for the whole of M2.5.",
            },
            Evidence::AbsentFrom {
                roots: &["crates/vitrin-realm-init/src", "crates/vitrin-core/src"],
                // `SeccompRequest` and not the flag spelling `--seccomp=off`:
                // the flag spelling appears in the PROSE of four files here
                // that say the switch does not exist, so an absence check on it
                // fires on its own documentation. `LandlockRequest` is exactly
                // how this codebase spells a per-session confinement selector
                // -- parsed from a flag, carried in `Config`, journaled as
                // `requested` -- so the absence of the seccomp-shaped twin is
                // the CODE form of "there is no off-switch", and it is a needle
                // no comment has a reason to write.
                needle: "SeccompRequest",
                means: "there is still no way to run a session whose realms are unfiltered \
                        while the journal reads as confined. Landlock has `--landlock=off` \
                        because a kernel can lack Landlock entirely and an operator on such a \
                        machine still needs a session; seccomp filter mode is present in every \
                        kernel this build's floor admits, so the same flag here would only ever \
                        be a way to publish confinement that was not applied. If this fires, a \
                        selector was added and every surface above has to say what it does \
                        before it ships.",
            },
        ],
    },
    Claim {
        id: "kernel-matrix-rows-are-held-to-this-builds-floor",
        says: "the per-kernel boot rows record the BUILD they were taken with as well as the \
               kernel's answers, and `cargo xtask kernel-matrix --check` holds that half to this \
               tree: it goes RED the day this build's floor moves out from under the rows, so a \
               row cannot go on describing an older binary in silence. That check RE-BOOTS \
               NOTHING -- it says the rows describe this build and says nothing about whether \
               these kernels still answer this way, which only \
               `tests/kernel-matrix/collect.sh --check` re-takes.",
        issue: "#281 built the row set and #188 moved the floor out from under it. The rows were \
                re-collected on 2026-08-16, which retired the staleness admission this row \
                replaces -- what survives the payment is the guarantee, not the freshness.",
        // **This row is deliberately NOT the inverse of the one it replaces.**
        // Until 2026-08-16 four surfaces carried
        // `kernel-matrix-rows-are-stale-in-their-build-half`: an admission that
        // the rows predated P2.6.4's floor. `collect.sh` re-booted all five
        // kernels against the current binary, the acknowledgement constants in
        // crates/xtask/src/kernel_matrix.rs went to `&[]`, and that claim went
        // red exactly as it was designed to -- so it was retired rather than
        // reworded.
        //
        // What replaced it is NOT "the rows are current". That would be a claim
        // with a shelf life, true on the day it was written and decaying from
        // then on, which is the failure mode this repository has been bitten by
        // often enough to have a gate for it. A freshness assertion also cannot
        // be held by evidence: no needle in the source can witness that a QEMU
        // boot happened recently. So what is published instead is what the
        // MECHANISM guarantees -- a floor move reddens the gate -- which is
        // durable, checkable against code, and the half a reader actually needs
        // in order to know how much a green build is worth.
        //
        // Two anchors per surface, on this file's standing rule. Here the pair
        // is GUARANTEE and SCOPE: the first pins what the gate does when the
        // floor moves, and the second pins what it deliberately does not do, so
        // a surface cannot keep the reassuring half while dropping the sentence
        // that stops a green pull request being read as a re-measurement.
        //
        // One needle spelling for all four surfaces, which is unusual here and
        // is why the wording avoids an apostrophe: site/index.html writes them
        // `&rsquo;`, so "the floor" rather than "this build's floor" is what
        // lets one string cascade across Markdown and HTML alike.
        surfaces: &[
            Anchor {
                path: LIMITS,
                needle: "red\n  the day the floor moves out from under them",
            },
            Anchor {
                path: LIMITS,
                needle: "re-boots nothing",
            },
            Anchor {
                path: README,
                needle: "red the day the floor moves\n  out from under them",
            },
            Anchor {
                path: README,
                needle: "re-boots nothing",
            },
            Anchor {
                path: SECURITY,
                needle: "red the day the\n  floor moves out from under them",
            },
            Anchor {
                path: SECURITY,
                needle: "re-boots nothing",
            },
            Anchor {
                path: SITE,
                needle: "red the day the floor moves out from under\n      them",
            },
            Anchor {
                path: SITE,
                needle: "re-boots nothing",
            },
        ],
        evidence: &[
            Evidence::Contains {
                path: "crates/xtask/src/kernel_matrix.rs",
                needle: "fn staleness(rows: &[Row], build: &BuildMechanisms) -> Result<Staleness>",
                means: "the comparison still HAPPENS. Without it the guarantee above is a \
                        sentence somebody typed, and the next floor move would republish the \
                        rows as current exactly as P2.6.4's did -- `kernel-matrix --check` \
                        otherwise compares the page against the rows, so it is blind to the two \
                        being stale together.",
            },
            Evidence::Contains {
                path: "crates/xtask/src/kernel_matrix.rs",
                needle: "THE CHECKED-IN ROWS MUST BE RE-COLLECTED",
                means: "the comparison is a REFUSAL and not a warning. This is the difference \
                        between the published sentence and a weaker true one: a gate that \
                        printed a note and rendered the page anyway would satisfy the needle \
                        above while letting exactly the drift this claim promises against reach \
                        a reader.",
            },
            Evidence::Contains {
                path: "docs/book/src/isolation-kernels.md",
                needle: "goes RED",
                means: "the page the four surfaces send a reader to publishes the same \
                        guarantee, generated rather than typed. A limits entry describing a gate \
                        while the cited page describes something else is the drift this whole \
                        file exists for, one link further out. The banner it appears in is \
                        rendered from the measured delta, so the day that delta is non-empty \
                        this needle moves to the stale paragraph and the claim has to be \
                        rewritten deliberately.",
            },
        ],
    },
];

// ---------------------------------------------------------------------------
// The derived tables
// ---------------------------------------------------------------------------

/// Values with one canonical definition, published in several renderings.
///
/// See [`Derived`] for why this table exists and what [`Anchor`] cannot do. A
/// row belongs here rather than in [`CLAIMS`] when the surfaces print the *same
/// value* in *different forms* -- which is precisely when a shared literal
/// needle stops cascading and a number can go stale with the gate green.
pub const DERIVED: &[Derived] = &[
    Derived {
        id: "landlock-abi-floor",
        says: "the Landlock ABI floor this build declares. One constant, four published \
               renderings of it, and until #172 the four were held by nothing.",
        issue: "#187 owns what P2.6.3 still does not do. The floor's own value is an owner's \
                decision (2026-08-15, lowered a rung on 2026-08-16) rather than an issue.",
        source: Source::File {
            path: "crates/vitrin-realm-init/src/lib.rs",
            reads: &[Read {
                after: "pub const LANDLOCK_MIN_ABI: u32 = ",
                shape: Shape::Digits,
            }],
        },
        // docs/book/src/isolation-kernels.md renders the same number ("at or
        // above the floor of 6") and is deliberately NOT here: it is generated
        // by `cargo xtask isolation-matrix`, which already reads this constant,
        // so a row would hold the generator to itself. The four below are
        // hand-written and are the ones that can rot.
        renderings: &[
            Rendering {
                path: README,
                render: floor_bold_here,
                context: "** here",
            },
            Rendering {
                path: SECURITY,
                render: floor_bold_here,
                context: "** here",
            },
            // TWO statements of the floor, six hundred lines apart -- the
            // startup-refusal bullet and condition 4 of the four-condition
            // block -- and one context covers both. This is the shape a
            // first-hit `contains` was blind to: either could have gone stale
            // behind the other.
            Rendering {
                path: LIMITS,
                render: floor_bold_in_this,
                context: "** in this",
            },
            Rendering {
                path: SITE,
                render: floor_strong_in_this_build,
                context: "</strong> in this build",
            },
            // A FIFTH surface, added at P2.6.4 because it had already rotted.
            // Commit e7b5514 lowered the floor from 7 to 6 and moved the four
            // renderings above; this one -- condition (4) of the Phase-2 plan's
            // own `host-must-have-landlock` bullet -- kept saying **7** for a
            // day, on the document `CLAUDE.md`'s `known-limit` rule sends
            // whoever closes a limit to read.
            //
            // It is a NUMBER, not argued prose, which is why adding it here
            // does not cross the line this module's docs draw against
            // anchoring the two registers to each other: the plan document and
            // the limits page may say the same thing in different words, and
            // may not say different NUMBERS. The `Anchor` warning above is
            // about phrases; `Derived` exists for exactly this.
            Rendering {
                path: "docs/plan/02-phase-2-semantic-epochs.md",
                render: floor_bold_paren,
                context: "`build.landlock_min_abi` (**",
            },
        ],
    },
    Derived {
        id: "seccomp-deny-list-rows",
        says: "how many rows the shipped seccomp deny-list holds. `deny-list` is a claim whose \
               SIZE is half its meaning -- a reader is being told what the filter does NOT \
               cover -- so six surfaces state the number, and nothing but this row holds them \
               to the table.",
        issue: "#188 (P2.6.4). #172 is why it is a `Derived` and not six literals: adding a \
                fourteenth row would otherwise leave six pages saying thirteen and no build \
                would be red.",
        // The canonical source is a LITERAL in the crate that owns the table,
        // and `the_published_row_count_is_the_tables_own` holds that literal to
        // `ROWS.len()`. Two steps rather than one because this gate reads text
        // and cannot evaluate `len()`; the in-crate test is what stops the
        // literal from being the lie instead.
        source: Source::File {
            path: "crates/vitrin-realm-init/src/seccomp.rs",
            reads: &[Read {
                after: "pub const ROW_COUNT: usize = ",
                shape: Shape::Digits,
            }],
        },
        // One register on all six, which is unusual here and is a deliberate
        // choice rather than a shortcut: `13 denied syscall rows` is a phrase
        // no page had a reason to write before this row existed, so a single
        // `context` identifies it everywhere without forcing any page to give
        // up its own voice around it. The limits page and README each say it
        // more than once; `scan_surface` holds every occurrence, so a page
        // cannot update one mention and leave the other behind.
        renderings: &[
            Rendering {
                path: LIMITS,
                render: seccomp_rows,
                context: "denied syscall rows",
            },
            Rendering {
                path: README,
                render: seccomp_rows,
                context: "denied syscall rows",
            },
            Rendering {
                path: SECURITY,
                render: seccomp_rows,
                context: "denied syscall rows",
            },
            Rendering {
                path: SITE,
                render: seccomp_rows,
                context: "denied syscall rows",
            },
            Rendering {
                path: "examples/realm.toml",
                render: seccomp_rows,
                context: "denied syscall rows",
            },
            Rendering {
                path: "docs/plan/02-phase-2-semantic-epochs.md",
                render: seccomp_rows,
                context: "denied syscall rows",
            },
        ],
    },
    Derived {
        id: "seccomp-rows-demonstrated",
        says: "how many of the deny-list's rows were DEMONSTRATED against a positive control on \
               the kernel the published measurement was taken on. The row count says how much \
               the filter denies; this says how much of that denial is confinement the filter \
               ADDS on a real machine, which is the honest half and the one three surfaces \
               state in the same breath.",
        issue: "#188 (P2.6.4) measured it. It is a `Derived` because it was a word -- `Eleven` \
                -- on three surfaces and a literal nowhere, so it could rot in silence while \
                `seccomp-deny-list-rows` held the 13 beside it to the table.",
        // **The canonical source is the file that PRODUCED the measurement.**
        // Not the filter table: which rows have a working positive control is
        // a property of the kernel's sysctls, not of the table, so a build
        // constant would be the wrong kind of thing. Not one of the three
        // published pages either -- deriving a page from a page is circular.
        // `tests/integration/test_real_seccomp.py` is the gate that runs the
        // control, and the constant there carries the machine and the date.
        //
        // Two checks stand behind it, and neither alone is enough. This one
        // holds three pages to the constant; the gate's own
        // `test_the_published_demonstration_figure_still_describes_this_table`
        // holds the constant to the SHIPPED TABLE -- eleven demonstrated plus
        // two named as shadowed must be the whole row set -- so a fourteenth
        // row makes the figure red instead of leaving three pages saying
        // `11 of the 13` beside a sentence saying `14 denied syscall rows`.
        source: Source::File {
            path: "tests/integration/test_real_seccomp.py",
            reads: &[Read {
                after: "\nPUBLISHED_DEMONSTRATED = ",
                shape: Shape::Digits,
            }],
        },
        // The `13` inside each context is deliberate and is NOT a second copy
        // of the row count getting in through the back door: it is part of the
        // register that identifies this sentence, and if the table ever grows
        // the context stops matching and this row goes red -- which is the
        // correct outcome, because a changed table means the measurement has
        // to be re-taken rather than re-worded.
        renderings: &[
            Rendering {
                path: README,
                render: rows_demonstrated,
                context: " of the 13 denied syscall rows are demonstrated",
            },
            Rendering {
                path: SECURITY,
                render: rows_demonstrated,
                context: " of the 13 denied syscall rows are demonstrated",
            },
            // The limits page shouts the verb, and the case is what tells its
            // sentence apart from the other two rather than a style choice --
            // `normalize` does not fold case, so one shared context would
            // simply not match here.
            Rendering {
                path: LIMITS,
                render: rows_demonstrated_shouted,
                context: " of the 13 denied syscall rows are DEMONSTRATED",
            },
        ],
    },
    Derived {
        id: "wlcs-counts",
        says: "the four advisory wlcs conformance counts. shim/wlcs/README.md's fenced block is \
               the canonical statement; three surfaces quote all four numbers and one renders \
               two of them as a ratio.",
        issue: "#157.",
        source: Source::File {
            path: WLCS_README,
            // Each `after` is leading-delimited so it matches the counts LINE
            // and not the prose about the format. shim/wlcs/README.md also
            // contains "Prints a `total=/passed=/failed=/skipped=/status=`
            // summary", and a bare `total=` would match both -- at which point
            // the first occurrence wins silently, which is what the
            // exactly-once rule is here to refuse.
            reads: &[
                Read {
                    after: "\ntotal=",
                    shape: Shape::Digits,
                },
                Read {
                    after: " passed=",
                    shape: Shape::Digits,
                },
                Read {
                    after: " failed=",
                    shape: Shape::Digits,
                },
                Read {
                    after: " skipped=",
                    shape: Shape::Digits,
                },
            ],
        },
        renderings: &[
            // The context is `total=`, which is the whole counts register:
            // every counts line on the page has to be THIS run's four numbers,
            // not merely one of them somewhere.
            Rendering {
                path: LIMITS,
                render: wlcs_full_counts,
                context: "total=",
            },
            Rendering {
                path: README,
                render: wlcs_full_counts,
                context: "total=",
            },
            // #172's task 2, and the reason this row can exist at all. The site
            // used to quote an undated 3/180 derived from counts it did not
            // show -- a ratio with no failed, no skipped, no date and no wlcs
            // version, which shim/wlcs/README.md's own conclusion says means
            // nothing. It now carries the full four-number form. Note the
            // direction of the fix: the weaker surface was raised to the
            // canonical statement, never the canonical statement lowered to
            // match the weaker one.
            Rendering {
                path: SITE,
                render: wlcs_full_counts,
                context: "total=",
            },
            // The one legitimately different register, and the reason a shared
            // literal could not have held this claim. shim/wlcs/README.md
            // states the ratio a second time to compare it against 8/49 on
            // wlcs 1.7.0, where the four-number form would be noise.
            //
            // `wlcs_ratio` renders the two words in front of the ratio as well,
            // and that is not decoration: a bare `3/180` has no value-free part
            // except `/`, and a context of `/` would scan every path and every
            // date on the page. The register is "the same shim SCORES x/y", so
            // that is what the rendering says.
            Rendering {
                path: WLCS_README,
                render: wlcs_ratio,
                context: "shim scores ",
            },
        ],
    },
    Derived {
        id: "wlcs-version",
        says: "the wlcs release the advisory counts were measured against. shim/wlcs/README.md's \
               own conclusion is that a number from this harness means nothing without it, so \
               the version is load-bearing wherever the counts are quoted.",
        // #157 asks for the re-measure; the version is what makes the CURRENT
        // numbers interpretable in the meantime.
        issue: "#157.",
        // THE COMPONENT THE COUNTS ROW ABOVE DID NOT HOLD. `wlcs-counts` pins
        // four numbers on four surfaces, and shim/wlcs/README.md:728 says in
        // its own words that those numbers "mean nothing" without the version
        // beside them -- the same shim scores 8/49 against 1.7.0 with no shim
        // change in between. Until this row the most load-bearing component of
        // the claim was the one component nothing anchored: every surface could
        // have kept saying 1.6.1-1 after the runner image moved past it, which
        // is not a hypothetical (shim/wlcs/README.md predicts that exact day)
        // and would have left three published pages attributing one run's
        // numbers to another run's harness.
        source: Source::File {
            path: WLCS_README,
            reads: &[Read {
                after: "**Provenance.** wlcs ",
                shape: Shape::UpTo(" —"),
            }],
        },
        renderings: &[
            // One rendering shared by all three surfaces, deliberately: the
            // version is a provenance stamp rather than an argued sentence, so
            // there is no register to preserve here, and one spelling means the
            // context covers the whole family. The narrower `run, against wlcs`
            // rather than `against wlcs` is required -- every one of these
            // surfaces also says "against wlcs 1.7.0" in the very next clause,
            // and that sentence is true and must not go red.
            Rendering {
                path: LIMITS,
                render: wlcs_against_version,
                context: "run, against wlcs ",
            },
            Rendering {
                path: README,
                render: wlcs_against_version,
                context: "run, against wlcs ",
            },
            Rendering {
                path: SITE,
                render: wlcs_against_version,
                context: "run, against wlcs ",
            },
        ],
    },
    Derived {
        id: "apparmor-green-run-kernel",
        says: "the kernel release the AppArmor profile's one green run was on. Six surfaces \
               state one measurement; on 2026-08-16 three of them said 1022 and three said \
               1020, and the commit that corrected three was itself under-enumerating.",
        issue: "#286 shipped the profile and the job. #293 owns installing it.",
        // THE ROW THIS WHOLE MECHANISM WAS BUILT FOR, and the evidence that
        // literal anchors were not enough.
        // `apparmor-profile-is-one-image-and-uninstalled` anchors the artefact
        // path and the bound sentence on four surfaces and passed green over a
        // six-way disagreement about which kernel the measurement was taken on,
        // because the kernel release was never anchored at all. The value is
        // now read from the profile's own header -- the file that carries the
        // run's report -- and every surface has to render it.
        source: Source::File {
            path: PROFILE,
            reads: &[Read {
                after: "#  What it reported on kernel ",
                shape: Shape::UpTo(","),
            }],
        },
        renderings: &[
            Rendering {
                path: README,
                render: kernel_backticked_it_reported,
                context: "` it reported",
            },
            Rendering {
                path: SECURITY,
                render: kernel_backticked_it_took,
                context: "` it took",
            },
            Rendering {
                path: SITE,
                render: kernel_coded_that_took,
                context: "</code> that took",
            },
            // `on kernel \`` and not `kernel \``. The limits page also reports a
            // DIFFERENT, equally real run on `6.17.0-1020-azure` -- the
            // namespace refusal, 2026-08-14 -- and a context broad enough to
            // catch both would turn a true sentence red. Two measurements on
            // two runner images are not drift; the check has to be able to tell
            // them apart, and the register is what does it.
            Rendering {
                path: LIMITS,
                render: kernel_on_kernel_with,
                context: "on kernel `",
            },
            // The SECOND reading on the limits page, and the one the 2026-08-16
            // correction missed: the `unconfined_knob=0` paragraph reports the
            // same job, the same run and the same machine, several hundred
            // lines further down. A surface can carry a value twice and drift
            // from itself.
            Rendering {
                path: LIMITS,
                render: kernel_knob_paragraph,
                context: "`ubuntu-latest` (kernel `",
            },
        ],
    },
    Derived {
        id: "kernels-measured",
        says: "how many distribution kernels have actually been booted with the shipped binary. \
               The canonical source is the set of checked-in row files, not a number written \
               down beside it.",
        issue: "#281.",
        // Deliberately counted from tests/kernel-matrix/rows/ rather than from
        // tests/kernel-matrix/kernels.manifest: the manifest says which kernels
        // are IN THE SET, the rows say which were MEASURED, and every published
        // sentence here is about the second. A manifest entry with no row is a
        // kernel nobody booted.
        source: Source::FileCount {
            dir: "tests/kernel-matrix/rows",
            suffix: ".row",
        },
        renderings: &[
            // Two statements on the limits page, both inside this one context:
            // the page's opening orientation and the floor-exclusion section
            // six score lines later. The second used to open a sentence with a
            // capitalised "Five", which normalize does not case-fold and which
            // therefore rendered it invisible to this scan; it was lowered into
            // the sentence so BOTH are held rather than one.
            Rendering {
                path: LIMITS,
                render: kernels_distribution_kernels,
                context: " distribution kernels",
            },
            Rendering {
                path: README,
                render: kernels_distribution_kernels,
                context: " distribution kernels",
            },
            Rendering {
                path: SECURITY,
                render: kernels_distribution_kernels,
                context: " distribution kernels",
            },
            Rendering {
                path: SITE,
                render: kernels_distribution_kernels,
                context: " distribution kernels",
            },
            // The site's second register for the same count. The trailing colon
            // is load-bearing: the site says "one of them" twice about other
            // things, and ` of them` alone would redden two true sentences.
            Rendering {
                path: SITE,
                render: kernels_on_n_of_them,
                context: " of them:",
            },
        ],
    },
];

/// Code-to-code mirrors: a value duplicated in a second file, with a comment
/// promising the duplicate follows the original.
///
/// **Why this is a separate table from [`DERIVED`] and not a separate
/// mechanism.** [`CLAIMS`] and [`DERIVED`] are about text this project
/// PUBLISHES; a mirror between two source files is a different property with a
/// different reader, and collapsing them would make the green line's "published
/// claims" count mean two things. The machinery underneath is identical because
/// the shape is identical -- one canonical definition, N renderings of it -- and
/// duplicating the runner to make the tables feel different would be the worse
/// error.
///
/// **Why it is worth having at all.** On 2026-08-16
/// `tests/integration/harness.py` carried `LANDLOCK_MIN_ABI = 7` for a day
/// after the crate moved to 6, under a docstring saying it mirrors the crate,
/// while `test_real_confinement.py` asserts `obtained_rung >=
/// LANDLOCK_MIN_ABI` -- so the confinement gate would have gone red on exactly
/// the ABI-6 machines the lowering existed to serve, and green everywhere the
/// suite is actually run. A value duplicated with a comment promising it
/// mirrors another is a drift bug waiting to happen, and nothing checked it.
///
/// **What is NOT here, and why the uncovered set is written down rather than
/// left to be assumed empty:**
///
/// * `LANDLOCK_MIN_ABI` / `LANDLOCK_BUILD_MAX_RUNG` -- already held, by
///   `crates/xtask/src/isolation_matrix.rs`'s `Constants::cross_check_harness`.
///   Deliberately NOT moved here: that module's own comment argues correctly
///   that the check belongs with the tool that already reads one of the files,
///   and moving it would buy tidiness at the cost of a second reader.
/// * `PROTOCOL_DECODE_CLAIMS` -- held by a completeness assertion inside
///   `fuzz/tests/seed_corpus_reachability.rs`.
/// * `PROPERTY_GATES` -- held in line, in `.github/workflows/ci.yml` itself.
/// * `crates/xtask/src/main.rs`'s `_toml_string_array`, which says it "mirrors
///   `tests/integration/harness.py`'s". That is a **behavioural** mirror -- two
///   parsers that must accept the same language -- and no string comparison can
///   hold it. It is the one known unheld mirror in this list, it can diverge
///   silently, and closing it means a shared fixture rather than a needle.
pub const MIRRORS: &[Derived] = &[
    Derived {
        id: "shim-core-fd",
        says: "the file descriptor the core places the shim's end of the identity socketpair on. \
               shim/include/wire.h names it as a #define and says to keep it in sync with \
               spawn::SHIM_CORE_FD.",
        issue: "No issue: this is a mirror the C header asks for in its own comment.",
        source: Source::File {
            path: "crates/vitrin-core/src/spawn.rs",
            reads: &[Read {
                after: "pub(crate) const SHIM_CORE_FD: RawFd = ",
                shape: Shape::Digits,
            }],
        },
        renderings: &[Rendering {
            path: "shim/include/wire.h",
            render: core_fd_define,
            context: "#define VITRIN_CORE_FD ",
        }],
    },
    Derived {
        id: "demo-identity",
        says: "the demo agent's principal identity. Four files carry it: the launcher that \
               writes the registry, the shipped example registry, the integration harness and \
               the demo agent itself.",
        issue: "No issue: crates/xtask/src/main.rs's comment says it must match \
                examples/principals.toml and run_demo.py, and nothing checked that.",
        source: Source::File {
            path: "crates/xtask/src/main.rs",
            reads: &[Read {
                after: "const DEMO_IDENTITY: &str = \"",
                shape: Shape::UpTo("\""),
            }],
        },
        renderings: &[
            Rendering {
                path: "tests/integration/harness.py",
                render: py_demo_identity,
                context: "DEMO_IDENTITY = \"",
            },
            Rendering {
                path: "examples/agent-demo/run_demo.py",
                render: py_demo_identity,
                context: "DEMO_IDENTITY = \"",
            },
            Rendering {
                path: "examples/principals.toml",
                render: toml_identity,
                context: "identity = \"",
            },
        ],
    },
    Derived {
        id: "demo-token",
        says: "the demo agent's pre-shared token, whose length and repeated character the two \
               Python copies spell as a repetition rather than as the literal.",
        issue: "No issue: same comment as the identity above.",
        // THE HONEST BOUND ON THIS ROW, because it is weaker than the others
        // and must not be read as equal to them. The Python spelling is a
        // repetition expression, so what can be held is the repeated character
        // and the length -- not the whole alphabet. Change the Rust token to 64
        // characters that merely START with the same one and this row stays
        // green while the copies diverge. It is here anyway because the two
        // failures it DOES catch (a length change, a different fill character)
        // are the two ways this constant has any reason to move, and because
        // the residual hole fails loudly: a mismatched token refuses the
        // handshake in every real-app gate rather than passing quietly.
        source: Source::File {
            path: "crates/xtask/src/main.rs",
            reads: &[Read {
                after: "const DEMO_TOKEN: &str = \"",
                shape: Shape::UpTo("\""),
            }],
        },
        renderings: &[
            Rendering {
                path: "tests/integration/harness.py",
                render: py_demo_token,
                context: "DEMO_TOKEN = \"",
            },
            Rendering {
                path: "examples/agent-demo/run_demo.py",
                render: py_demo_token,
                context: "DEMO_TOKEN = \"",
            },
        ],
    },
    Derived {
        id: "max-live-realms-mirrors-the-surface-cap",
        says: "the per-connection cap on live realm handles, which principal.rs justifies as \
               mirroring the shim server's surface cap.",
        issue: "No issue: found while auditing #172. The justification is REAL -- \
                crates/vitrin-core/src/shim.rs's MAX_LIVE_SURFACES -- and an audit that searched \
                only the C shim under shim/ concluded it was unbacked. A justification nobody \
                can locate is one somebody eventually deletes, so it is pinned here and the \
                comment now names the constant.",
        source: Source::File {
            path: "crates/vitrin-core/src/shim.rs",
            reads: &[Read {
                after: "pub(crate) const MAX_LIVE_SURFACES: usize = ",
                shape: Shape::Digits,
            }],
        },
        renderings: &[Rendering {
            path: "crates/vitrin-core/src/principal.rs",
            render: max_live_realms,
            context: "pub(crate) const MAX_LIVE_REALMS: usize = ",
        }],
    },
];

// ---------------------------------------------------------------------------
// The coverage roll, so a set cannot shrink quietly
// ---------------------------------------------------------------------------

/// **Every claim this gate covers, named. Deleting a row from [`CLAIMS`] turns
/// this red.**
///
/// # Why a second list rather than a tidier `CLAIMS.len()`
///
/// Because until this list existed, *deleting a whole row was green*. Every
/// surface anchor, every evidence assertion and every test kept passing; the
/// only thing that moved was a number printed at the end of a passing CI step,
/// where nobody reads it. That is issue #288's failure mode -- a green check
/// over a quietly smaller set -- reproduced inside the tool built to prevent it,
/// and #288's own answer is the standard followed here: a count is acceptable
/// only when a comment names each thing it counts, so that raising or lowering
/// it is a **visible decision** in a diff rather than a reflex.
///
/// A list of ids is that comment and that count in one object, and it is
/// strictly better than either alone:
///
/// * deleting a row is red, and the failure names the id that vanished rather
///   than reporting that 24 became 23;
/// * adding a row is red until the id is written down here, so new coverage is
///   claimed deliberately;
/// * renaming an id is red in both directions at once, which a count cannot see
///   at all;
/// * the diff of a coverage change is the ids, so a reviewer reads *which*
///   claim left the gate, which is the only question worth asking.
///
/// The order is [`CLAIMS`]'s own, and it is not checked -- reordering a table is
/// not a coverage change and must not cost a red build. Duplicates ARE checked:
/// two rows sharing an id would let one masquerade as the other here.
pub const COVERED_CLAIMS: &[&str] = &[
    "accessibility-absent",
    "no-x-server",
    "no-layer-shell",
    "no-idle-inhibit",
    "clipboard-bound",
    "realm-cardinality",
    "one-output",
    "no-touch-no-tablet",
    "band-witness-headless-only",
    "drm-ci-compile-only",
    "no-portals",
    "shell-crash-loses-window-management",
    "lock-and-blank-do-not-stop-an-agent",
    "media-keys-reach-an-app-that-cannot-act",
    "no-key-repeat-on-drm",
    "host-must-permit-unprivileged-userns",
    "apparmor-profile-is-one-image-and-uninstalled",
    "host-must-have-landlock",
    // The four #172 names as known to drift, and the two the sweep for them
    // turned up.
    "fuzz-soak-never-run",
    "wlcs-advisory-and-mostly-red",
    "no-oin-membership-yet",
    "spdx-coverage-not-machine-checked",
    "dco-is-executed",
    "per-kernel-isolation-matrix-exists",
    // #188's, and the sixth surface-set this gate holds: the published gap
    // "a realm is path-confined, not syscall-confined" CHANGED rather than
    // closed, which is the case a `known-limit` sweep is most likely to do
    // on half its surfaces.
    "seccomp-is-a-deny-list",
    // #188's second. This replaced `kernel-matrix-rows-are-stale-in-their-build-half`
    // when the rows were re-collected on 2026-08-16 and that claim went red as
    // designed. The subject changed with it, deliberately: the retired row
    // published a DEBT, and this one publishes the GUARANTEE that outlives the
    // payment -- a floor move reddens the gate, and the cheap check re-boots
    // nothing. Swapping in "the rows are current" instead would have put a
    // claim with a shelf life on four surfaces.
    "kernel-matrix-rows-are-held-to-this-builds-floor",
];

/// Every derived value this gate covers. Same contract as [`COVERED_CLAIMS`],
/// and the same reason: a `Derived` row is the only thing holding four or five
/// surfaces to one number, so losing one loses all of them at once and in
/// silence.
pub const COVERED_DERIVED: &[&str] = &[
    "landlock-abi-floor",
    "wlcs-counts",
    "wlcs-version",
    "apparmor-green-run-kernel",
    "kernels-measured",
    // #188's. Six surfaces, one number, and the number is the size of a
    // deny-list -- which is the half of "deny-list" a reader is actually
    // being told.
    "seccomp-deny-list-rows",
    // #188's second, and the only derived value here that is a MEASUREMENT
    // rather than a build constant: how much of that deny-list is confinement
    // the filter adds on a real kernel. It was spelled as an English word on
    // three surfaces and existed as a literal nowhere.
    "seccomp-rows-demonstrated",
];

/// Every code-to-code mirror this gate covers. Same contract as
/// [`COVERED_CLAIMS`].
pub const COVERED_MIRRORS: &[&str] = &[
    "shim-core-fd",
    "demo-identity",
    "demo-token",
    "max-live-realms-mirrors-the-surface-cap",
];

/// Hold a table to its coverage roll: same ids, no duplicates, order free.
///
/// Both directions are failures and they are different failures, so the message
/// says which. A **missing** id is coverage that left the gate -- the one this
/// exists for. An **unlisted** id is coverage that arrived without being
/// claimed, which is not a defect in the tree but is a decision somebody has to
/// make explicitly, exactly as raising a count would be.
/// [`coverage_failures`] over the three shipped tables. Called by the gate and
/// by its own test, so neither can hold a different set from the other.
pub fn shipped_coverage_failures() -> Vec<String> {
    let claims: Vec<&str> = CLAIMS.iter().map(|c| c.id).collect();
    let derived: Vec<&str> = DERIVED.iter().map(|d| d.id).collect();
    let mirrors: Vec<&str> = MIRRORS.iter().map(|d| d.id).collect();
    let mut failures = coverage_failures("COVERED_CLAIMS", "CLAIMS", &claims, COVERED_CLAIMS);
    failures.extend(coverage_failures(
        "COVERED_DERIVED",
        "DERIVED",
        &derived,
        COVERED_DERIVED,
    ));
    failures.extend(coverage_failures(
        "COVERED_MIRRORS",
        "MIRRORS",
        &mirrors,
        COVERED_MIRRORS,
    ));
    failures
}

fn coverage_failures(label: &str, table: &str, have: &[&str], listed: &[&str]) -> Vec<String> {
    let mut failures = Vec::new();
    for (i, id) in have.iter().enumerate() {
        if have[..i].contains(id) {
            failures.push(format!(
                "[{id}] COVERAGE -- two rows in {table} share this id. One of them is invisible \
                 in every failure message this gate prints, and either could stand in for the \
                 other in {label}."
            ));
        }
    }
    for id in listed {
        if !have.contains(id) {
            failures.push(format!(
                "[{id}] COVERAGE -- {label} lists this id and {table} no longer has a row for \
                 it.\n    COVERAGE SHRANK. Deleting a row used to be green: every remaining \
                 check still passed and only a tally printed at the end of a passing step \
                 moved. That is the failure issue #288 exists for, so it is red here.\n    If \
                 the claim is genuinely gone from every published surface, remove it from \
                 {label} in the same commit and say in the message what stopped being \
                 published. If it is not, restore the row."
            ));
        }
    }
    for id in have {
        if !listed.contains(id) {
            failures.push(format!(
                "[{id}] COVERAGE -- {table} has a row for this id and {label} does not list \
                 it.\n    New coverage is claimed deliberately: add the id to {label}. The list \
                 is what makes a shrinking set visible in a diff instead of in a number nobody \
                 reads."
            ));
        }
    }
    failures
}

// ---------------------------------------------------------------------------
// Render functions.
//
// Named `fn` items rather than closures, because a `const` table cannot hold a
// closure and because each one is a published surface's own spelling of a
// value -- worth reading on its own line and worth a test. Every one of them is
// exercised by `every_rendering_moves_when_the_canonical_value_moves`, which is
// this mechanism's non-vacuity guard: a render function that ignores its input
// (returning a constant, or an empty string, both of which `contains` accepts)
// would make a row silently unable to fail.
// ---------------------------------------------------------------------------

/// `README.md`, `SECURITY.md`: "... `vitrind --print-floor` -- **6** here ...".
fn floor_bold_here(v: &[String]) -> String {
    format!("**{}** here", v[0])
}

/// `docs/book/src/limits.md`: "... **6** in this build ...".
fn floor_bold_in_this(v: &[String]) -> String {
    format!("**{}** in this", v[0])
}

/// All six surfaces that state the deny-list's size: "... 13 denied syscall
/// rows ...".
fn seccomp_rows(v: &[String]) -> String {
    format!("{} denied syscall rows", v[0])
}

/// `README.md` and `SECURITY.md`: "... 11 of the 13 denied syscall rows are
/// demonstrated against a positive control ...".
fn rows_demonstrated(v: &[String]) -> String {
    format!("{} of the 13 denied syscall rows are demonstrated", v[0])
}

/// `docs/book/src/limits.md`, which shouts the verb: "... **11 of the 13
/// denied syscall rows are DEMONSTRATED on the kernel ...".
fn rows_demonstrated_shouted(v: &[String]) -> String {
    format!("{} of the 13 denied syscall rows are DEMONSTRATED", v[0])
}

/// `docs/plan/02-phase-2-semantic-epochs.md`: "... `build.landlock_min_abi`
/// (**6**), and ...".
fn floor_bold_paren(v: &[String]) -> String {
    format!("`build.landlock_min_abi` (**{}**)", v[0])
}

/// `site/index.html`. Short and tag-free past the one element it owns, so
/// nothing straddles a boundary.
fn floor_strong_in_this_build(v: &[String]) -> String {
    format!("<strong>{}</strong> in this build", v[0])
}

/// The canonical four-number form, as `shim/wlcs/README.md` prints it.
fn wlcs_full_counts(v: &[String]) -> String {
    format!(
        "total={} passed={} failed={} skipped={}",
        v[0], v[1], v[2], v[3]
    )
}

/// The ratio register: "the same shim scores 3/180 against wlcs 1.6.1".
///
/// The two leading words are part of the rendering rather than scenery: a bare
/// `3/180` has no value-free substring but `/`, and a `Rendering::context` of
/// `/` would scan every path and date on the page.
fn wlcs_ratio(v: &[String]) -> String {
    format!("shim scores {}/{}", v[1], v[0])
}

/// The provenance register the counts are quoted in: "... on the 2026-07-25
/// run, against wlcs 1.6.1-1".
fn wlcs_against_version(v: &[String]) -> String {
    format!("run, against wlcs {}", v[0])
}

fn kernel_backticked_it_reported(v: &[String]) -> String {
    format!("`{}` it reported", v[0])
}

fn kernel_backticked_it_took(v: &[String]) -> String {
    format!("`{}` it took", v[0])
}

fn kernel_coded_that_took(v: &[String]) -> String {
    format!("<code>{}</code> that took", v[0])
}

fn kernel_on_kernel_with(v: &[String]) -> String {
    format!("on kernel `{}` with", v[0])
}

/// The limits page's second, further-down reading of the same run.
///
/// It renders the `ubuntu-latest` clause in front of the parenthesis so the
/// context can be `` `ubuntu-latest` (kernel ` ``: the page also writes
/// `` (kernel `7.1.8-arch1-3`, ...) `` about the maintainer's own box, and a
/// context of `` (kernel ` `` would redden that true sentence.
fn kernel_knob_paragraph(v: &[String]) -> String {
    format!("`ubuntu-latest` (kernel `{}`, 2026-08-15)", v[0])
}

fn kernels_distribution_kernels(v: &[String]) -> String {
    format!("{} distribution kernels", number_word(&v[0]))
}

/// The site's second register. The trailing colon is the whole of what makes
/// `` ` of them:` `` a usable context on a page that says "one of them" twice
/// about other things.
fn kernels_on_n_of_them(v: &[String]) -> String {
    format!("on {} of them:", number_word(&v[0]))
}

fn core_fd_define(v: &[String]) -> String {
    format!("#define VITRIN_CORE_FD {}", v[0])
}

fn py_demo_identity(v: &[String]) -> String {
    format!("DEMO_IDENTITY = \"{}\"", v[0])
}

fn toml_identity(v: &[String]) -> String {
    format!("identity = \"{}\"", v[0])
}

/// The repeated character and the length, which is all the Python spelling can
/// carry. See the row's own comment for the bound.
fn py_demo_token(v: &[String]) -> String {
    let fill = v[0].chars().next().unwrap_or('?');
    format!("DEMO_TOKEN = \"{}\" * {}", fill, v[0].chars().count())
}

fn max_live_realms(v: &[String]) -> String {
    format!("pub(crate) const MAX_LIVE_REALMS: usize = {};", v[0])
}

/// A small decimal count as English prose spells it.
///
/// Same shape and the same reason as `crates/xtask/src/test_census.rs`'s
/// `published_count_sentence`: the pages write "five distribution kernels", not
/// "5". Past twelve the digit is better than a wrong word, and a count that
/// large is not a sentence this project will be writing by hand anyway.
fn number_word(count: &str) -> String {
    match count {
        "0" => "no",
        "1" => "one",
        "2" => "two",
        "3" => "three",
        "4" => "four",
        "5" => "five",
        "6" => "six",
        "7" => "seven",
        "8" => "eight",
        "9" => "nine",
        "10" => "ten",
        "11" => "eleven",
        "12" => "twelve",
        other => return other.to_string(),
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

/// Entry point for `cargo xtask limits-check`.
pub fn limits_check(root: &Path) -> Result<()> {
    // Coverage first, and in the gate rather than only in a test: the tally
    // this used to print is not load-bearing, and a table that lost a row
    // should fail before anybody reads what the survivors say.
    let mut failures = shipped_coverage_failures();
    failures.extend(run_claims(root, CLAIMS)?);
    let (derived_failures, derived_values) = run_derived(root, DERIVED, "DERIVED");
    failures.extend(derived_failures);
    let (mirror_failures, mirror_values) = run_derived(root, MIRRORS, "MIRROR");
    failures.extend(mirror_failures);
    let (cross, limit_counts) = cross_check_files(root)?;
    let cross_len = cross.len();
    failures.extend(cross);
    if failures.is_empty() {
        // Per home, never only the total: a total is what a home going empty
        // hides behind, and the whole reason this line prints a number is so a
        // human reading a CI log sees a set shrink.
        let total: usize = limit_counts.iter().map(|(_, n)| n).sum();
        let breakdown = limit_counts
            .iter()
            .map(|(path, n)| format!("{n} from {path}"))
            .collect::<Vec<_>>()
            .join(", ");
        // The derived VALUES, not only how many rows there are. A `Derived`
        // row can go quiet without the count moving -- a rendering satisfied
        // by accident, a canonical read that started matching something else
        // -- and the number a human can spot as wrong in a CI log is the value
        // itself, never the tally.
        println!(
            "limits-check: {} claims hold across their surfaces and their code evidence; {} \
             derived value(s) agree in EVERY published rendering, including a surface's second \
             rendering of its own ({}); {} mirrored constant(s) match their canonical source \
             ({}); and the enumerating plan documents ({breakdown}) enumerate the same {total} \
             published limits as {LIMITS}. Each of the first three counts is held to a named \
             roll (COVERED_CLAIMS, COVERED_DERIVED, COVERED_MIRRORS), so it cannot shrink \
             without a red build.",
            CLAIMS.len(),
            DERIVED.len(),
            show_values(&derived_values),
            MIRRORS.len(),
            show_values(&mirror_values),
        );
        return Ok(());
    }
    let mut msg = String::new();
    msg.push_str(&format!(
        "{n} published claim(s) drifted ({cross_len} of them in the plan/page limit set).\n\n\
         A claim here is text this project publishes to readers. A failure means one of these \
         things, and the message below says which:\n  \
         * SURFACE  -- the claim is missing from a surface that must carry it, so two \
         published surfaces now disagree.\n  \
         * EVIDENCE -- the claim no longer matches the code, so what is published is false \
         of `main` in one direction or the other.\n  \
         * DERIVED  -- one value with one canonical definition is rendered on several surfaces, \
         and a surface's rendering no longer matches what the canonical source says. This is the \
         failure a literal anchor CANNOT see, because the surfaces spell the same value \
         differently (#172).\n  \
         * MIRROR   -- a constant duplicated in a second file, under a comment promising it \
         mirrors the first, has stopped mirroring it.\n  \
         * ISSUE    -- a claim names an issue as its provenance and no surface it is published \
         on cites that issue.\n  \
         * SELF-DRIFT -- one surface renders the same derived value twice and the two do not \
         agree, so a page contradicts itself. The message names both line numbers.\n  \
         * COVERAGE -- a table and its coverage roll disagree, so the set this gate holds has \
         changed. Shrinking it is a decision, not a side effect (#288).\n  \
         * SET      -- the enumerating plan documents and docs/book/src/limits.md no longer \
         enumerate the same limits. This one is about the SET, never the wording: reword either \
         document freely, but a limit present in one and absent from the other is drift.\n  \
         * BULLET / MARKER / ID / REGION -- an enumerated limit carries no marker, a marker is \
         malformed, or the region delimiters are gone, so the set comparison above cannot see \
         it.\n\n\
         Fix the page or fix the table in crates/xtask/src/limits.rs -- but do not weaken an \
         anchor, delete a marker, or edit a render function to make this pass. Issue #224 exists \
         because two of its own body items had gone false this way, and issue #172 exists \
         because a commit fixing three stale surfaces left three more behind.\n",
        n = failures.len(),
    ));
    for f in &failures {
        msg.push_str(&format!("\n{f}\n"));
    }
    bail!(msg);
}

// ---------------------------------------------------------------------------
// The tracker half (#172 acceptance criterion 3) -- ADVISORY, never a PR gate
// ---------------------------------------------------------------------------

/// The published surfaces a `known-limit` issue could be cited on.
///
/// Deliberately the reader-facing three and not every file in the tree: the
/// question this report answers is *"can somebody who meets this gap on a page
/// find the thing tracking it"*, and a citation in a plan document or a source
/// comment does not answer it.
const TRACKER_SURFACES: &[&str] = &[LIMITS, README, SITE];

/// `cargo xtask limits-check --tracker`: which open `known-limit` issues are
/// cited on no published surface.
///
/// # This is advisory, and saying so is the point
///
/// #172's third acceptance criterion asks that `is:open label:known-limit` and
/// the published limits page describe the same set. It is the one criterion
/// this repository cannot honestly turn into a pull-request gate, and the
/// instruction that produced this function was explicit that an offline gate
/// pretending to check the tracker is **worse than none**. So:
///
/// * it **shells out to `gh`** and needs network and credentials, which a
///   pull-request runner should not depend on for a docs check;
/// * it **exits 0 whatever it finds**. It reports. It never fails a build. A
///   gate that goes red because a third party opened an issue is the
///   *"trains people to weaken the check"* failure #224's risk list names, and
///   the whole set-equality property is impossible in one direction anyway --
///   many published limits deliberately have no issue and `README.md` promises
///   exactly that;
/// * it is wired into `.github/workflows/honesty-tracker.yml`, on a schedule
///   and on demand, **never** into `ci.yml`.
///
/// The blocking, offline half of the same concern is [`uncited_issues`], which
/// runs on every pull request and holds the other direction: every issue this
/// table names must be cited on a surface of its own claim.
///
/// If `gh` is missing or unauthenticated this prints why and still exits 0 --
/// an advisory report that cannot run is a report that did not run, and
/// dressing that up as a pass would be the same dishonesty in miniature.
pub fn tracker_report(root: &Path) -> Result<()> {
    let out = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--label",
            "known-limit",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,title",
        ])
        .current_dir(root)
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            println!(
                "limits-check --tracker: `gh issue list` failed ({}). This report is ADVISORY \
                 and exits 0 regardless: it needs network and credentials, which is exactly why \
                 it is not on the pull-request path.\n{}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return Ok(());
        }
        Err(err) => {
            println!(
                "limits-check --tracker: could not run `gh` ({err}). This report is ADVISORY and \
                 exits 0 regardless. Install and authenticate the GitHub CLI to run it, or read \
                 the same thing by hand:\n  gh issue list --label known-limit --state open"
            );
            return Ok(());
        }
    };
    let body = String::from_utf8_lossy(&out.stdout);

    // A deliberately small hand-parse rather than a serde dependency in xtask:
    // the shape is `[{"number":N,"title":"..."},...]` and this report is
    // advisory, so a parse that degrades to "found nothing" costs a printed
    // line rather than a wrong build result.
    let mut numbers: Vec<String> = Vec::new();
    let mut rest = body.as_ref();
    while let Some(at) = rest.find("\"number\":") {
        rest = &rest[at + "\"number\":".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            numbers.push(format!("#{digits}"));
        }
    }

    let mut surfaces = String::new();
    for path in TRACKER_SURFACES {
        surfaces.push_str(&fs::read_to_string(root.join(path)).unwrap_or_default());
        surfaces.push('\n');
    }
    // `cites`, not `contains`: `#15` is a substring of `#155`, and this report's
    // whole output is a list of issue numbers, so a bare substring test would
    // quietly move issues into the "cited" column that no surface names. See
    // `cites` for why only the right boundary needs checking.
    let (cited, uncited): (Vec<_>, Vec<_>) =
        numbers.iter().partition(|number| cites(&surfaces, number));

    println!(
        "limits-check --tracker (ADVISORY -- this never fails a build):\n  \
         {} open `known-limit` issue(s); {} cited on at least one of {}; {} cited on none.",
        numbers.len(),
        cited.len(),
        TRACKER_SURFACES.join(", "),
        uncited.len(),
    );
    if !uncited.is_empty() {
        println!(
            "  Cited nowhere a reader will look: {}\n  \
             That is not automatically a defect. A gap can be published with no issue on \
             purpose, and this project says so in writing. What it IS, is the list to read \
             before claiming the tracker and the limits page describe the same set -- which is \
             the claim #172 asks for and the one nothing in this repository can prove.",
            uncited
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// Run every claim and return one string per failure. Split out so the tests
/// can drive a synthetic claim table without a process exit.
pub fn run_claims(root: &Path, claims: &[Claim]) -> Result<Vec<String>> {
    // Once per run, not once per root: every `AbsentFrom` below shares it.
    let built = BuildOutput::of_tree(root)?;
    let mut failures = Vec::new();
    for claim in claims {
        // A claim with no evidence is decoration: it would pin a wording
        // forever and say nothing about whether the wording is true.
        if claim.evidence.is_empty() {
            failures.push(format!(
                "[{}] TABLE -- this claim has no code evidence. A claim-string check with no \
                 evidence half only proves three files agree with each other, which is \
                 exactly how #224's own body stayed false for weeks.",
                claim.id
            ));
        }
        if claim.surfaces.is_empty() {
            failures.push(format!(
                "[{}] TABLE -- this claim names no published surface, so it is not a published \
                 claim and does not belong in this table.",
                claim.id
            ));
        }
        failures.extend(uncited_issues(root, claim));
        for anchor in claim.surfaces {
            let path = root.join(anchor.path);
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(err) => {
                    failures.push(format!(
                        "[{}] SURFACE -- cannot read {}: {err}",
                        claim.id, anchor.path
                    ));
                    continue;
                }
            };
            if !normalize(&text).contains(&normalize(anchor.needle)) {
                failures.push(format!(
                    "[{}] SURFACE -- {} does not contain {:?}.\n    The claim: {}\n    \
                     Issue behind it: {}\n    Either this surface dropped the claim while the \
                     others kept it, or the wording moved on one surface only. Both are the \
                     drift #224 acceptance criterion 1 names.",
                    claim.id, anchor.path, anchor.needle, claim.says, claim.issue
                ));
            }
        }
        for ev in claim.evidence {
            match *ev {
                Evidence::Contains {
                    path,
                    needle,
                    means,
                } => {
                    let full = root.join(path);
                    let text = match fs::read_to_string(&full) {
                        Ok(t) => t,
                        Err(err) => {
                            failures.push(format!(
                                "[{}] EVIDENCE -- cannot read {path}: {err}",
                                claim.id
                            ));
                            continue;
                        }
                    };
                    if !normalize(&text).contains(&normalize(needle)) {
                        failures.push(format!(
                            "[{}] EVIDENCE -- {path} no longer contains {needle:?}.\n    That \
                             string was the evidence for: {means}\n    The published claim: \
                             {}\n    Published on: {}\n    The code moved and the published \
                             claim did not. Re-read the page before touching this table.",
                            claim.id,
                            claim.says,
                            surface_list(claim),
                        ));
                    }
                }
                Evidence::AbsentFrom {
                    roots,
                    needle,
                    means,
                } => {
                    let mut hits = Vec::new();
                    for r in roots {
                        collect_hits(root, &root.join(r), needle, &built, &mut hits)?;
                    }
                    if !hits.is_empty() {
                        hits.sort();
                        failures.push(format!(
                            "[{}] EVIDENCE -- {needle:?} now appears in the tree, in:\n      \
                             {}\n    Its absence was the evidence for: {means}\n    The \
                             published claim: {}\n    Published on: {}",
                            claim.id,
                            hits.join("\n      "),
                            claim.says,
                            surface_list(claim),
                        ));
                    }
                }
            }
        }
    }
    Ok(failures)
}

// ---------------------------------------------------------------------------
// The issue half of #172's third acceptance criterion -- the offline half
// ---------------------------------------------------------------------------

/// The claim's tracking issue, if it names one and no surface cites it.
///
/// # What this property is, and what it deliberately is not
///
/// #172's acceptance criterion 3 asks that `is:open label:known-limit` and the
/// published limits page **describe the same set**. That cannot be a blocking
/// gate here and it cannot be that property, and both halves of that sentence
/// are worth writing down rather than quietly shipping something weaker under
/// the same name:
///
/// * **It needs the GitHub API.** A pull-request gate that turns red because
///   somebody else opened an issue is the *"brittle... which trains people to
///   weaken the check"* outcome #224's risk list names. The tracker half is
///   therefore [`tracker_report`], which is advisory, scheduled, and never on
///   the pull-request path.
/// * **Set equality is impossible in one direction, by policy.** Many published
///   limits deliberately have no issue and say so -- `README.md` makes that an
///   explicit promise (*"Every bullet names the issue behind it, or says
///   plainly that it has none"*), and [`Claim::issue`] carries several such
///   statements in full. Demanding an issue per limit would force a tracker
///   entry for every exclusion this project has decided is permanent.
///
/// So what ships is the **offline, blocking half**: if a claim names a tracking
/// issue, at least one of that claim's own surfaces has to cite it, so a reader
/// who meets a published gap can find the thing tracking it without leaving the
/// page. That is strictly weaker than criterion 3, and it is named as weaker
/// here rather than ticked as if it were the whole.
///
/// # Which number is "the tracking issue", and why that is not a guess
///
/// Only the **first** `#N` in [`Claim::issue`] is required to be cited, and
/// only when the field does not open by declaring the claim has none. That is
/// not a heuristic invented for this check -- it is the convention every row in
/// [`CLAIMS`] was already written to, unprompted and consistently: a field
/// either opens with the owning issue (*"#213 (WS-E.2.1, closed). This is the
/// claim #224's own body got wrong..."*) or opens with *"No issue"* and then
/// argues why.
///
/// Requiring **every** number instead was tried first and is wrong, not merely
/// strict. These fields are prose, and the later numbers in them are context:
/// the workstream gate that found the gap, the issue this limit must not be
/// confused with, the follow-up that would close it one day. Demanding a
/// citation for each would push a page towards listing issues that are not
/// what a reader should go and read -- which is the opposite of the property,
/// dressed as more of it.
fn uncited_issues(root: &Path, claim: &Claim) -> Vec<String> {
    let Some(tracking) = tracking_issue(claim.issue) else {
        return Vec::new();
    };
    let mut surfaces = String::new();
    for anchor in claim.surfaces {
        if let Ok(text) = fs::read_to_string(root.join(anchor.path)) {
            surfaces.push_str(&text);
            surfaces.push('\n');
        }
    }
    if cites(&surfaces, &tracking) {
        return Vec::new();
    }
    vec![format!(
        "[{}] ISSUE -- this claim names {tracking} as the issue behind it and no surface it is \
         published on cites that number.\n    The claim: {}\n    Surfaces: {}\n    A reader who \
         meets this gap cannot find what tracks it without leaving the page, which is the \
         promise README.md makes in the very section these claims live in -- and which it was \
         breaking, on its own fuzz and wlcs bullets, until #172. Either cite {tracking} on a \
         surface, or -- if the claim genuinely has no issue -- open the `issue` field with `No \
         issue` and say why, as every row here whose `issue` field opens with those two words \
         already does.",
        claim.id,
        claim.says,
        surface_list(claim),
    )]
}

/// Does `text` cite issue `number` (spelled `#N`), rather than merely containing
/// its digits inside a longer one?
///
/// A bare `contains` is wrong here and wrong in the direction that hides work:
/// `#15` is a substring of `#155`, so a page that mentions the REUSE issue would
/// have been read as citing a hypothetical `#15` and any claim naming it would
/// have passed with nothing published. The repository already has issues whose
/// numbers nest this way -- `#15x`, `#16x`, `#17x`, `#18x` against `#1`, and
/// `#28x` against `#2` -- so this is a live false-positive rather than a
/// theoretical one.
///
/// Only the right-hand boundary needs checking. A `#` is not a digit, so
/// `#155`'s digits can never be read as the tail of a longer citation.
fn cites(text: &str, number: &str) -> bool {
    text.match_indices(number).any(|(at, _)| {
        !text[at + number.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

/// The first `#N` in `text`, unless `text` opens by declaring there is none.
fn tracking_issue(text: &str) -> Option<String> {
    if text.trim_start().starts_with("No issue") {
        return None;
    }
    let at = text.find('#')?;
    let digits: String = text[at + 1..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("#{digits}"))
}

// ---------------------------------------------------------------------------
// Derived values (#172, option (b)'s one new concept)
// ---------------------------------------------------------------------------

/// `(row id, the values it read)`, for the green line.
type DerivedValues = Vec<(String, Vec<String>)>;

/// Run one derived table. Returns the failures and, for the green line, what
/// each row actually read.
///
/// `label` is the failure prefix and the only difference between running
/// [`DERIVED`] and [`MIRRORS`]: the mechanism is one mechanism, the property
/// is two properties, and the message has to say which.
///
/// # Every occurrence, not the first one
///
/// Matching is [`scan_surface`], never `contains`. `contains` answers *"is this
/// value somewhere on the page"* and stops at the first yes, which means a page
/// that states the value twice is held only at whichever statement the search
/// reaches first -- **a surface can drift from itself with this gate green.**
/// That is not a hypothetical failure mode, it is the one on the record: the
/// site asserted the per-kernel matrix "has not been built" in one paragraph and
/// cited its measurements ninety lines later, and the limits page states both
/// the AppArmor run's kernel and the Landlock floor in two places each.
///
/// So every occurrence of the rendering's [`Rendering::context`] on the surface
/// must sit inside a full canonical rendering, and a second occurrence that
/// disagrees is a SELF-DRIFT failure naming the line number of the stale one and
/// the line number of the one it disagrees with.
///
/// # The vacuity guards, and why each one exists
///
/// A `contains` test accepts an empty needle and accepts a needle that has
/// nothing to do with the value, so this mechanism can go quiet in ways the
/// totals do not show. Five refusals, all of them modelled on
/// [`cross_check_limit_sets`]'s rule 5:
///
/// 1. **A row with no rendering is refused.** A canonical value nobody
///    publishes is not a published value; the row would read as coverage and
///    hold nothing.
/// 2. **A read that finds no value, or finds its literal more than once, is
///    refused.** The second half matters more than it looks: with two
///    occurrences the first one wins *silently*, and a canonical value that
///    silently picks a side is not canonical. This is what lets `\ntotal=`
///    disambiguate `shim/wlcs/README.md`'s counts line from its prose about the
///    summary format instead of guessing between them.
/// 3. **A rendering that produces the empty string is refused**, because
///    `"anything".contains("")` is true and such a row could never fail.
///    The stronger half of this guard is not here but in
///    `every_rendering_moves_when_the_canonical_value_moves`, which perturbs
///    the canonical value and requires every render function's output to move
///    -- a render function that ignores its input is the one vacuity a runtime
///    check cannot see.
/// 4. **An empty context is refused**, for the same reason as 3: every string
///    contains it, so every position on the surface would "carry" the value.
/// 5. **A context that does not occur exactly once in the rendered form is
///    refused.** Zero occurrences means the table names a register the render
///    function does not produce, and the scan would hold nothing; two means the
///    value's offset from the context is ambiguous, and the scan would align
///    against whichever one it picked. Both are table bugs and both are refused
///    rather than resolved.
///
/// And the green line prints the VALUES, not only a count, for the same reason
/// the limit-set line prints its size per home: a mechanism can quietly stop
/// covering something without any total moving.
pub fn run_derived(root: &Path, table: &[Derived], label: &str) -> (Vec<String>, DerivedValues) {
    let mut failures = Vec::new();
    let mut values_out: DerivedValues = Vec::new();
    for row in table {
        if row.renderings.is_empty() {
            failures.push(format!(
                "[{}] {label} -- this row names no surface that renders the value. A canonical \
                 value nobody publishes is not a published value, and the row would count \
                 towards coverage while holding nothing.",
                row.id
            ));
            continue;
        }
        let values = match read_source(root, &row.source) {
            Ok(v) => v,
            Err(err) => {
                failures.push(format!("[{}] {label} -- {err}", row.id));
                continue;
            }
        };
        values_out.push((row.id.to_string(), values.clone()));
        for rendering in row.renderings {
            let want = (rendering.render)(&values);
            if want.trim().is_empty() {
                failures.push(format!(
                    "[{}] {label} -- the rendering for {} is empty. Every string contains the \
                     empty string, so this row could never fail; a render function that drops \
                     its input is the shape this guard exists for.",
                    row.id, rendering.path
                ));
                continue;
            }
            let path = root.join(rendering.path);
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(err) => {
                    failures.push(format!(
                        "[{}] {label} -- cannot read {}: {err}",
                        row.id, rendering.path
                    ));
                    continue;
                }
            };
            let scan = match scan_surface(&text, &want, rendering.context) {
                Ok(scan) => scan,
                Err(err) => {
                    failures.push(format!(
                        "[{}] {label} -- the table row for {} is malformed: {err}\n    This is a \
                         bug in crates/xtask/src/limits.rs, not on the surface. Until it is \
                         fixed the scan holds nothing at all, which is why it is refused rather \
                         than skipped.",
                        row.id, rendering.path
                    ));
                    continue;
                }
            };
            if scan.agreeing.is_empty() {
                failures.push(format!(
                    "[{}] {label} -- {} does not render the canonical value.\n    Canonical \
                     source: {}\n    Read: {}\n    This surface must contain: {want:?}\n    The \
                     value: {}\n    Provenance: {}\n    THE VALUE MOVED AND THIS SURFACE DID \
                     NOT. Fix the surface. Do NOT fix the render function -- that is the table \
                     agreeing with the drift, and it is exactly how four published `6`s could \
                     have gone stale behind a green build.{}",
                    row.id,
                    rendering.path,
                    source_path(&row.source),
                    values.join(", "),
                    row.says,
                    row.issue,
                    if scan.disagreeing.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n    The register IS on this surface, at line(s) {} -- it is the \
                             VALUE there that is wrong. That is the whole failure, on one page.",
                            lines(&scan.disagreeing)
                        )
                    },
                ));
                continue;
            }
            if !scan.disagreeing.is_empty() {
                failures.push(format!(
                    "[{}] {label} SELF-DRIFT -- {} disagrees with ITSELF.\n    Line(s) {} render \
                     the canonical value: {want:?}\n    Line(s) {} carry the same register \
                     ({:?}) and do not.\n    Canonical source: {}\n    Read: {}\n    The value: \
                     {}\n    \
                     Provenance: {}\n    One page, two answers. A check that stopped at the first \
                     occurrence would call this green, which is exactly what site/index.html did \
                     while it said the per-kernel matrix `has not been built` in one paragraph \
                     and cited its measurements ninety lines later. Fix the stale occurrence; do \
                     not narrow the context to hide it.",
                    row.id,
                    rendering.path,
                    lines(&scan.agreeing),
                    lines(&scan.disagreeing),
                    rendering.context,
                    source_path(&row.source),
                    values.join(", "),
                    row.says,
                    row.issue,
                ));
            }
        }
    }
    (failures, values_out)
}

/// Where a surface renders a derived value, and where it claims to and does not.
///
/// Line numbers, 1-based, in the surface's own file -- the only coordinate a
/// human can act on. Both lists are line numbers of the *context*, so a failure
/// can put the two occurrences of one disagreement side by side.
#[derive(Debug, Default)]
pub struct SurfaceScan {
    /// Occurrences of the context that sit inside a full canonical rendering.
    pub agreeing: Vec<usize>,
    /// Occurrences of the context that do not. Each of these is a place on this
    /// surface that talks about this value and gets it wrong.
    pub disagreeing: Vec<usize>,
}

/// Find **every** place on `surface` that renders this value, and say which of
/// them agree with `want`.
///
/// `context` is the value-free literal that identifies the register (see
/// [`Rendering::context`]). Because it occurs exactly once inside `want`, its
/// offset there fixes where the rest of the rendering must sit relative to any
/// occurrence found on the surface -- so the scan can ask, at each occurrence,
/// *"is the value here the canonical one"* rather than the far weaker *"does
/// the canonical value appear anywhere on this page"*.
///
/// Returns `Err` only for a malformed table row, never for a drifted surface:
/// an empty context, or one that does not occur exactly once in `want`. Those
/// are refused rather than resolved, because either resolution would silently
/// pick which occurrence to hold.
pub fn scan_surface(surface: &str, want: &str, context: &str) -> Result<SurfaceScan, String> {
    let want = normalize(want);
    let context = normalize(context);
    if context.is_empty() {
        return Err(
            "the context is empty. Every string contains the empty string, so every position on \
             the surface would count as an occurrence and the scan would hold nothing."
                .to_string(),
        );
    }
    let in_want: Vec<usize> = want.match_indices(&context).map(|(at, _)| at).collect();
    if in_want.len() != 1 {
        return Err(format!(
            "the context {context:?} occurs {} time(s) in the rendered form {want:?}, and it must \
             occur exactly once. Zero means the table names a register this render function does \
             not produce; two means the value's offset from the context is ambiguous and the scan \
             would align against whichever one it happened to pick.",
            in_want.len()
        ));
    }
    let offset = in_want[0];

    // Indexed rather than plain `normalize`, because a failure that cannot name
    // the line is a failure somebody has to go and grep for -- and the whole
    // point of this scan is that there are two places to look at, not one.
    let (text, map) = normalize_indexed(surface);
    let mut scan = SurfaceScan::default();
    for (at, _) in text.match_indices(&context) {
        let line = line_at(surface, map[at]);
        // `checked_sub` and `get`, not indexing: the rendering may start before
        // the beginning of the surface, and a byte offset computed by
        // subtraction need not land on a character boundary. Both are simply
        // "this occurrence is not a canonical rendering".
        let aligned = at
            .checked_sub(offset)
            .and_then(|start| text.get(start..))
            .is_some_and(|rest| rest.starts_with(&want));
        if aligned {
            scan.agreeing.push(line);
        } else {
            scan.disagreeing.push(line);
        }
    }
    Ok(scan)
}

/// [`normalize`], plus a map from each byte of the result to the byte offset in
/// the input it came from.
///
/// The separator this inserts between two words maps to the first byte of the
/// *following* word, which is the answer that makes a line number useful: a
/// match starting at a separator is a match on the word after it.
fn normalize_indexed(text: &str) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut map: Vec<usize> = Vec::new();
    let mut in_word = false;
    for (at, ch) in text.char_indices() {
        if ch.is_whitespace() {
            in_word = false;
            continue;
        }
        if !in_word && !out.is_empty() {
            out.push(' ');
            map.push(at);
        }
        in_word = true;
        let before = out.len();
        out.push(ch);
        for _ in before..out.len() {
            map.push(at);
        }
    }
    debug_assert_eq!(out.len(), map.len());
    (out, map)
}

/// The 1-based line `offset` falls on.
fn line_at(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Line numbers as a human reads them out.
fn lines(nums: &[usize]) -> String {
    nums.iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read a derived row's canonical value(s).
fn read_source(root: &Path, source: &Source) -> Result<Vec<String>, String> {
    match source {
        Source::File { path, reads } => {
            if reads.is_empty() {
                return Err(format!(
                    "the row reads nothing out of {path}. A source with no read yields no value \
                     and every rendering of it would be a constant."
                ));
            }
            let text = fs::read_to_string(root.join(path))
                .map_err(|err| format!("cannot read the canonical source {path}: {err}"))?;
            let mut out = Vec::new();
            for read in *reads {
                out.push(read_value(&text, path, read)?);
            }
            Ok(out)
        }
        Source::FileCount { dir, suffix } => {
            let full = root.join(dir);
            let entries = fs::read_dir(&full)
                .map_err(|err| format!("cannot read the canonical directory {dir}: {err}"))?;
            let mut count = 0usize;
            for entry in entries {
                let entry = entry
                    .map_err(|err| format!("cannot walk the canonical directory {dir}: {err}"))?;
                if entry.path().is_file() && entry.file_name().to_string_lossy().ends_with(suffix) {
                    count += 1;
                }
            }
            if count == 0 {
                return Err(format!(
                    "no `*{suffix}` file under {dir}. The published sentences derive their number \
                     from that set, and a set that has become empty would render as `no` on every \
                     surface rather than failing -- which is the vacuity this refuses."
                ));
            }
            Ok(vec![count.to_string()])
        }
    }
}

/// One value, out of raw (never [`normalize`]d) text.
///
/// Raw on purpose, and this is the opposite choice from [`Anchor`]. A canonical
/// value lives in a constant declaration, a fenced code block or a `#define` --
/// none of which reflow -- and reading it raw is what makes a `\n`-prefixed
/// literal able to pick the counts line out of a file that also contains prose
/// about the counts format.
fn read_value(text: &str, path: &str, read: &Read) -> Result<String, String> {
    let occurrences = text.matches(read.after).count();
    if occurrences == 0 {
        return Err(format!(
            "{path} does not contain {:?}, so there is no canonical value to hold the surfaces \
             to. Either the definition moved or it was reworded; find it and update this row \
             rather than deleting it.",
            read.after
        ));
    }
    if occurrences > 1 {
        return Err(format!(
            "{path} contains {:?} {occurrences} times. The first one would win silently, and a \
             canonical value that silently picks between two candidates is not canonical. Make \
             the literal more specific -- a leading newline is usually enough to separate a \
             declaration from prose about it.",
            read.after
        ));
    }
    let rest = &text[text.find(read.after).expect("counted above") + read.after.len()..];
    let value = match read.shape {
        Shape::Digits => rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>(),
        Shape::UpTo(terminator) => match rest.find(terminator) {
            Some(at) => rest[..at].to_string(),
            None => {
                return Err(format!(
                    "{path} has {:?} but no {terminator:?} after it, so the value has no end.",
                    read.after
                ))
            }
        },
    };
    if value.is_empty() {
        return Err(format!(
            "{path} has {:?} but the value after it is empty. An empty value renders into \
             something every file contains.",
            read.after
        ));
    }
    Ok(value)
}

fn source_path(source: &Source) -> &'static str {
    match source {
        Source::File { path, .. } => path,
        Source::FileCount { dir, .. } => dir,
    }
}

/// `id=value` pairs for the green line, so a human sees what was compared.
fn show_values(values: &DerivedValues) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    values
        .iter()
        .map(|(id, vals)| format!("{id}={}", elide(&vals.join("/"))))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A value short enough to read in a CI log. A 64-character token printed in
/// full pushes every other value off the line, which defeats the reason the
/// line prints values at all.
fn elide(value: &str) -> String {
    let count = value.chars().count();
    if count <= 24 {
        return value.to_string();
    }
    let head: String = value.chars().take(12).collect();
    format!("{head}...({count} chars)")
}

// ---------------------------------------------------------------------------
// The set cross-check (#224 acceptance criterion 5)
// ---------------------------------------------------------------------------

/// The workstream plan document whose §6 enumerates the limits WS-E creates.
const PLAN: &str = "docs/plan/14-workstream-session-mode.md";

/// The Phase-2 plan document, whose §7 enumerates the limits Phase 2's
/// confinement work creates.
const PHASE_2: &str = "docs/plan/02-phase-2-semantic-epochs.md";

/// One document that enumerates limits, paired with its text.
///
/// `path` is carried rather than inferred so every failure message can name
/// **which** document is out of step; with one enumerating document that was a
/// constant, and a constant in a message is how a multi-document check starts
/// reporting the wrong file.
pub struct Enumeration<'a> {
    pub path: &'a str,
    pub text: &'a str,
}

/// Every document that may enumerate a published limit.
///
/// # Why a second region is not a carve-out
///
/// The property this gate holds has never been *"one document lists every
/// limit"*. It is **"every limit published to a reader is enumerated by the
/// plan document that owns the work which created it, and by exactly one"** --
/// because that enumeration is where `CLAUDE.md`'s `known-limit` rule sends
/// whoever later closes the limit to find every surface it touches. One
/// document was enough only for as long as one workstream had created every
/// marked limit, which was true on the day the gate was written and is a fact
/// about the tree rather than a property of the check.
///
/// Phase-2 confinement (E2.6/E2.7, D-020, D-037) creates limits of its own, and
/// they have no honest home in `docs/plan/14-workstream-session-mode.md`: that
/// section is titled *"Limits this workstream creates"* and its opening
/// sentence scopes every entry to a limit *"this workstream owns, not
/// inherits"*. Filing a Phase-2 confinement limit under it would make both
/// false in exactly the way §6 exists to prevent, and would send the next sweep
/// that closes it to the wrong document's surface table. So Phase 2 gets its
/// own region, in the document that schedules the work.
///
/// **What would have been a carve-out, and is not what this is.** Exempting the
/// page's new marker from the comparison; making a region optional; letting an
/// id be published with no enumerating home; or special-casing an id prefix.
/// None of those happens: both directions still hold, over the **union** of the
/// homes, and a published id with no home is the same red build it was before.
///
/// **The change is a net strengthening in three places**, each of which is a
/// hole the single-document version did not have to think about:
///
/// 1. **A registered home must enumerate at least one limit.** A home that
///    contributes nothing is a region somebody added to make a marker legal,
///    which is precisely the carve-out this comment says this is not. It fails.
/// 2. **An id is declared by exactly one home.** Two homes claiming one limit
///    would let a sweep close it in one document and leave the other standing --
///    a stale gap claim surviving in the file the next reader is sent to, which
///    is the failure #282 exists to prevent.
/// 3. **Rule 2 runs over every home.** A bullet added to the new region carries
///    the same "marked or explicitly off-page" obligation §6's bullets carry, so
///    the new region cannot become a place where limits go unrecorded.
///
/// Adding a third home is a deliberate act: it means a third body of work owns
/// limits of its own, and it costs a region, a surface table in that document,
/// and an entry here.
pub const ENUMERATORS: &[&str] = &[PLAN, PHASE_2];

/// The two comments that bound §6's limit set.
///
/// The region is delimited explicitly rather than by finding the `## 6.`
/// heading, because §6 also contains a surface table, a corrections list and a
/// measurements subsection -- none of which are limits, all of which contain
/// bullets, and every one of which would have to be excluded by a rule about
/// heading text. A heading is prose and gets rewritten; these two comments are
/// not prose and do not.
const REGION_BEGIN: &str = "<!-- limit-set: begin -->";
const REGION_END: &str = "<!-- limit-set: end -->";

/// `<!-- limit: id -->` -- this limit is on `docs/book/src/limits.md`. The same
/// marker is what the limits page itself carries, which is what makes the two
/// sets comparable at all.
const PUBLISHED: &str = "<!-- limit:";

/// `<!-- limit-not-on-page: id -- why -->` -- this limit is deliberately absent
/// from the limits page, with the reason written down. Note that this prefix is
/// not a prefix of [`PUBLISHED`] and vice versa (`limit:` versus `limit-`), so
/// scanning for one never finds the other.
const OFF_PAGE: &str = "<!-- limit-not-on-page:";

/// One parsed marker.
struct Marker {
    id: String,
    /// Everything between the id and the closing `-->`, whitespace-normalised:
    /// empty for a published marker, `-- why` for an off-page one.
    tail: String,
}

/// Every marker with `prefix` in already-[`normalize`]d text, in source order.
///
/// An unterminated marker (`<!-- limit: x` with no `-->` anywhere after it)
/// stops the scan. That is deliberate rather than an oversight: the ids after it
/// then go missing from the set, which fails loudly in the direction the set
/// comparison is built to report, instead of this function inventing an error
/// class of its own for a typo that cannot survive review anyway.
fn markers(normalized: &str, prefix: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    let mut rest = normalized;
    while let Some(at) = rest.find(prefix) {
        let after = &rest[at + prefix.len()..];
        let Some(end) = after.find("-->") else {
            break;
        };
        let inner = after[..end].trim();
        let (id, tail) = match inner.split_once(char::is_whitespace) {
            Some((id, tail)) => (id, tail.trim()),
            None => (inner, ""),
        };
        out.push(Marker {
            id: id.to_string(),
            tail: tail.to_string(),
        });
        rest = &after[end + "-->".len()..];
    }
    out
}

/// The slice of the normalised plan document between the two region markers.
///
/// Both delimiters must appear exactly once. That is not fussiness: §6's own
/// prose describes this mechanism, and a first draft of that prose spelled the
/// delimiters out in full -- which moved where this function thought the set
/// began and reported every one of the 39 published ids as missing from §6. The
/// failure was loud, and it named the wrong cause. So a second delimiter is now
/// its own error, and the prose spells the delimiters without their comment
/// brackets.
fn limit_set_region<'a>(normalized_plan: &'a str, source: &str) -> Result<&'a str, String> {
    for delimiter in [REGION_BEGIN, REGION_END] {
        let count = normalized_plan.matches(delimiter).count();
        if count > 1 {
            return Err(format!(
                "[limit-set] REGION -- {source} carries {delimiter:?} {count} times. It must \
                 appear exactly once: with two of them the region is whatever lies between the \
                 first of each, and every limit outside that accidental window is reported as \
                 missing from the document. If prose needs to name a delimiter, write it without \
                 its comment brackets."
            ));
        }
    }
    let Some(begin) = normalized_plan.find(REGION_BEGIN) else {
        return Err(format!(
            "[limit-set] REGION -- {source} carries no {REGION_BEGIN:?}. That comment is what \
             tells this gate where the document's limit set starts; without it there is no set to \
             compare and the cross-check would silently hold nothing."
        ));
    };
    let Some(end) = normalized_plan.find(REGION_END) else {
        return Err(format!(
            "[limit-set] REGION -- {source} carries no {REGION_END:?}, so the limit set has no \
             end and whatever follows it would be read as limits."
        ));
    };
    if end < begin {
        return Err(format!(
            "[limit-set] REGION -- {source} carries {REGION_END:?} before {REGION_BEGIN:?}."
        ));
    }
    Ok(&normalized_plan[begin + REGION_BEGIN.len()..end])
}

/// Is `line` the start of a top-level list item?
///
/// All of `-`, `*` and `+` start an unordered item in Markdown and render
/// identically, and `N.` or `N)` starts an ordered one. §6 uses `-` throughout
/// today. The other spellings are accepted here anyway because rule 2 exists to
/// see a limit somebody **adds**, and a limit added with `* ` was, in the first
/// version of this scan, exactly as invisible as one added with no marker at all
/// -- a false green, held now by `a_star_bullet_is_a_list_item_too`.
///
/// Leading whitespace disqualifies: an indented item is nested under a marked
/// one, which the module docs record as deliberate.
fn is_top_level_item(line: &str) -> bool {
    let bytes = line.as_bytes();
    if matches!(bytes, [b'-' | b'*' | b'+', b' ', ..]) {
        return true;
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    digits > 0 && matches!(bytes[digits..], [b'.' | b')', b' ', ..])
}

/// Rule 2: every top-level list item inside the region carries at least one
/// marker.
///
/// Line-based on purpose, and it is the one part of this check that is: the
/// question "is this item marked" is a question about document structure, and
/// structure lives in lines. See the module docs for what a wrapped marker
/// prefix costs here (a false red, never a false green).
///
/// The delimiters are matched by **containment**, not by whole-line equality,
/// and the scan reports a failure of its own if it never finds the region. Both
/// are there because equality made this rule silently vacuous: a `begin` line
/// with any trailing text left `inside` false for the whole document, the set
/// comparison went on working (it runs on normalised text and never looks at
/// lines), and an unmarked limit added underneath passed green.
fn unmarked_bullets(raw_plan: &str, source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut saw_region = false;
    let mut open: Option<(usize, String)> = None;
    let mut marked = false;
    let close = |open: &mut Option<(usize, String)>, marked: bool, out: &mut Vec<String>| {
        if let Some((line, text)) = open.take() {
            if !marked {
                out.push(format!(
                    "[limit-set] BULLET -- {source}:{line} is a limit in that document's set and \
                     carries no marker:\n      {text}\n    Every top-level list item between \
                     {REGION_BEGIN} and {REGION_END} must carry either `{PUBLISHED} <id> -->` (it \
                     is on docs/book/src/limits.md under that id) or `{OFF_PAGE} <id> -- why -->` \
                     (it is deliberately not, and here is the reason). An unmarked bullet is a \
                     limit this project has enumerated for itself and told nobody about, which is \
                     the failure §6 of {PLAN} already records itself having committed once."
                ));
            }
        }
    };
    for (i, line) in raw_plan.lines().enumerate() {
        if line.contains(REGION_BEGIN) {
            inside = true;
            saw_region = true;
            continue;
        }
        if line.contains(REGION_END) {
            close(&mut open, marked, &mut out);
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        if is_top_level_item(line) {
            close(&mut open, marked, &mut out);
            open = Some((i + 1, truncate(line)));
            marked = false;
        }
        if line.contains(PUBLISHED) || line.contains(OFF_PAGE) {
            marked = true;
        }
    }
    close(&mut open, marked, &mut out);
    if !saw_region {
        out.push(format!(
            "[limit-set] REGION -- the set comparison found the limit set, and this line-based \
             scan did not: no line of {source} contains {REGION_BEGIN}. Rule 2 -- every limit in \
             the region carries a marker -- therefore held nothing, and an unmarked limit added to \
             the region would have passed green. Do not silence this by deleting the rule; put the \
             delimiter back on a line this scan can find."
        ));
    }
    out
}

fn truncate(line: &str) -> String {
    let trimmed = line.trim();
    match trimmed.char_indices().nth(76) {
        Some((at, _)) => format!("{}...", &trimmed[..at]),
        None => trimmed.to_string(),
    }
}

/// Cross-check one enumerating document against the limits page.
///
/// The single-document form, and it exists **only** for the non-vacuity tests
/// below: one document, one page, one divergence at a time, so a test that
/// expects exactly one failure is reading a check and not an accident of how
/// many homes are registered. Production goes through
/// [`cross_check_limit_sets`] over [`ENUMERATORS`].
#[cfg(test)]
pub fn cross_check_limit_set(plan: &str, page: &str) -> Vec<String> {
    cross_check_limit_sets(
        &[Enumeration {
            path: PLAN,
            text: plan,
        }],
        page,
    )
}

/// Cross-check the enumerating documents' limit sets against the limits page's,
/// as sets of ids.
///
/// Pure over the documents' text so the tests can drive every direction without
/// writing to the tree. See the module docs for the rules and for what this
/// deliberately does not hold, and [`ENUMERATORS`] for why there is more than
/// one enumerating document and why that is not a weakening.
pub fn cross_check_limit_sets(sources: &[Enumeration<'_>], page: &str) -> Vec<String> {
    let mut failures = Vec::new();

    // A gate handed no enumerating document compares the page against nothing
    // and every rule below holds vacuously. Same refusal as rule 5, one level
    // up: the empty *list of homes* is as much a nothing-to-compare as the
    // empty set of limits.
    if sources.is_empty() {
        return vec![format!(
            "[limit-set] REGION -- no enumerating document was named at all, so {LIMITS} would be \
             compared against nothing and every rule below would hold vacuously. At least one \
             document must own the limit set."
        )];
    }

    // Regions first, and a failure here stops everything. A document whose
    // region cannot be found contributes no ids, and going on would report
    // every limit it owns as "published with no enumerating home" -- a pile of
    // derived failures whose one real cause is already in this list.
    let normalized: Vec<(&str, String)> = sources
        .iter()
        .map(|s| (s.path, normalize(s.text)))
        .collect();
    let mut regions: Vec<(&str, &str)> = Vec::new();
    for (path, text) in &normalized {
        match limit_set_region(text, path) {
            Ok(region) => regions.push((path, region)),
            Err(err) => failures.push(err),
        }
    }
    if !failures.is_empty() {
        return failures;
    }

    // Every plan-side declaration, tagged with the document that made it. The
    // tag is what makes a multi-home failure actionable and is what rules the
    // one-home version never needed are built on.
    let mut published: Vec<(&str, Marker)> = Vec::new();
    let mut off_page: Vec<(&str, Marker)> = Vec::new();
    for (path, region) in &regions {
        let pubs = markers(region, PUBLISHED);
        let offs = markers(region, OFF_PAGE);
        // **A registered home must enumerate something.** A home that declares
        // no limit at all is a region added to make some marker legal rather
        // than a document that owns limits -- exactly the carve-out
        // [`ENUMERATORS`] says this mechanism is not. It is refused here rather
        // than passing as one more empty set that agrees with everything.
        if pubs.is_empty() && offs.is_empty() {
            failures.push(format!(
                "[limit-set] SET -- {path} is registered as a home for the limit set and its \
                 region declares no limit at all, neither `{PUBLISHED} <id> -->` nor `{OFF_PAGE} \
                 <id> -- why -->`.\n    A home that enumerates nothing holds nothing: it adds an \
                 empty set to the union, which agrees with every other set, and it makes the \
                 published count read as coverage it does not have.\n    Either the document's \
                 limits moved out from between the region delimiters, or this document should \
                 stop being registered in crates/xtask/src/limits.rs."
            ));
        }
        published.extend(pubs.into_iter().map(|m| (*path, m)));
        off_page.extend(offs.into_iter().map(|m| (*path, m)));
    }
    let on_page = markers(&normalize(page), PUBLISHED);

    // **Rule 5, the vacuity guard.** Two empty sets are equal, so with no
    // published marker in any home and no marker on the limits page every rule
    // after this one holds and the gate prints "enumerate the same limit set"
    // having compared nothing. That is the shape of
    // `.github/vkms/run-advisory.sh` exiting 0 on a declared skip, and a gate
    // that certifies divergence as agreement is worse than no gate. Both halves
    // must be empty for this to fire: the homes emptied on their own are caught
    // below, once per surviving page marker. It is not subsumed by the
    // per-home rule above -- homes that declare only off-page limits satisfy
    // that rule and still leave nothing to compare.
    if published.is_empty() && on_page.is_empty() {
        failures.push(format!(
            "[limit-set] SET -- there is nothing to compare. No enumerating document ({}) \
             declares a limit published (there are {} off-page marker(s)) and {LIMITS} carries no \
             `{PUBLISHED} <id> -->` marker at all, so set equality holds between two empty sets \
             and this check would certify agreement having read nothing.\n    Something \
             structural moved: the limit bullets are outside the region delimiters, or the marker \
             spelling changed, or the page's markers were stripped. Restore them -- an empty set \
             is refused here precisely so it cannot pass as a green build.",
            sources
                .iter()
                .map(|s| s.path)
                .collect::<Vec<_>>()
                .join(", "),
            off_page.len()
        ));
    }

    for (marker, source, kind) in published
        .iter()
        .map(|(path, m)| (m, *path, "published"))
        .chain(off_page.iter().map(|(path, m)| (m, *path, "off-page")))
        .chain(on_page.iter().map(|m| (m, LIMITS, "page")))
    {
        if marker.id.is_empty()
            || !marker
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            failures.push(format!(
                "[limit-set] ID -- {source} carries a {kind} marker whose id is {:?}. An id is \
                 lowercase ASCII, digits and hyphens, and it is the whole of a limit's identity \
                 here -- a malformed one is a limit that cannot be matched in either direction.",
                marker.id
            ));
        }
    }
    for (_, marker) in &published {
        if !marker.tail.is_empty() {
            failures.push(format!(
                "[limit-set] MARKER -- the published marker for {:?} carries trailing text {:?}. \
                 `{PUBLISHED} <id> -->` takes an id and nothing else; a reason belongs on \
                 `{OFF_PAGE}`, and text here reads as a reason that nothing checks.",
                marker.id, marker.tail
            ));
        }
    }
    for (_, marker) in &off_page {
        let reason = marker.tail.strip_prefix("--").unwrap_or("").trim();
        if reason.is_empty() {
            failures.push(format!(
                "[limit-set] MARKER -- {:?} is declared off the limits page with no reason. The \
                 form is `{OFF_PAGE} <id> -- why -->`. The reason is the whole cost of the escape \
                 hatch: an unpublished limit is a real state, and one nobody had to justify in \
                 writing is how a limit stops being published by accident.",
                marker.id
            ));
        }
    }

    // **One id, one home, one verdict.** Every plan-side declaration goes into
    // one scan rather than one per kind, because with several homes the
    // interesting collisions are the cross-document and cross-kind ones: two
    // documents claiming the same limit, or one publishing what another
    // declares off-page. A sweep that closes such a limit edits the home it
    // found and leaves the other standing, which is a stale gap claim surviving
    // in the file `CLAUDE.md` sends the next reader to.
    let declarations: Vec<(&str, &str, &str)> = published
        .iter()
        .map(|(path, m)| (m.id.as_str(), *path, "published"))
        .chain(
            off_page
                .iter()
                .map(|(path, m)| (m.id.as_str(), *path, "off-page")),
        )
        .collect();
    failures.extend(duplicate_declarations(&declarations));
    failures.extend(duplicates(&on_page, LIMITS, "on the limits page"));

    let page_ids: Vec<&str> = on_page.iter().map(|m| m.id.as_str()).collect();

    for (source, marker) in &published {
        let id = marker.id.as_str();
        if !page_ids.contains(&id) {
            failures.push(format!(
                "[limit-set] SET -- {source} carries the limit {id:?} and says it is published, \
                 and {LIMITS} does not carry it.\n    Either the limits page dropped it -- a gap \
                 this project knows about and no longer tells anyone about, which is the \
                 direction #224 exists for -- or the limit stopped being published on purpose and \
                 its marker should now read `{OFF_PAGE} {id} -- why -->`.\n    Do not close this \
                 by deleting the marker in {source}."
            ));
        }
    }
    for id in &page_ids {
        if published.iter().any(|(_, m)| m.id == *id) {
            continue;
        }
        let declared_off_page = off_page.iter().find(|(_, m)| m.id == *id);
        let why = if let Some((source, _)) = declared_off_page {
            format!(
                "{source} declares {id:?} deliberately NOT on the limits page, and the limits \
                 page carries it anyway. One of the two is stale: either the limit really is \
                 published now and the marker should become `{PUBLISHED} {id} -->`, or the page \
                 is publishing something that document's own reason says it does not."
            )
        } else {
            format!(
                "{LIMITS} publishes the limit {id:?} and the plan document that owns it does not \
                 enumerate it at all -- searched: {}. Those documents are where `CLAUDE.md`'s \
                 `known-limit` rule sends whoever closes a limit to find every surface it touches, \
                 so a limit missing from all of them is a limit the next sweep will not know \
                 exists.\n    Add it to the plan document that owns the work which created it -- \
                 not to whichever one is nearest.",
                sources
                    .iter()
                    .map(|s| s.path)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        failures.push(format!("[limit-set] SET -- {why}"));
    }

    for source in sources {
        failures.extend(unmarked_bullets(source.text, source.path));
    }
    failures
}

fn duplicates(marks: &[Marker], source: &str, where_: &str) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for marker in marks {
        if seen.contains(&marker.id.as_str()) {
            out.push(format!(
                "[limit-set] ID -- {:?} appears more than once {where_} ({source}). An id is one \
                 limit's identity; two entries under one id make the set comparison ambiguous in \
                 both directions.",
                marker.id
            ));
        }
        seen.push(marker.id.as_str());
    }
    out
}

/// One id may be declared once, by one enumerating document, with one verdict.
///
/// `decls` is `(id, document, kind)` in source order; the first declaration
/// wins and every later one is reported against it, so the message names both
/// sides of the collision rather than only the newcomer.
fn duplicate_declarations(decls: &[(&str, &str, &str)]) -> Vec<String> {
    let mut seen: Vec<(&str, &str, &str)> = Vec::new();
    let mut out = Vec::new();
    for (id, source, kind) in decls {
        if let Some((_, first_source, first_kind)) = seen.iter().find(|(other, _, _)| other == id) {
            out.push(format!(
                "[limit-set] ID -- {id:?} is declared more than once by the enumerating \
                 documents: {first_source} declares it {first_kind}, and {source} declares it \
                 {kind}.\n    An id is one limit's identity and exactly one document owns it. Two \
                 declarations make the set comparison ambiguous, and a sweep that closes the limit \
                 in the document it happened to find leaves the other one standing -- a stale gap \
                 claim in the file the next reader is sent to."
            ));
        }
        seen.push((id, source, kind));
    }
    out
}

/// `(document path, how many limits it declares published)`, one per home.
type LimitCounts = Vec<(String, usize)>;

/// How many limits each home declares published, for the success line.
///
/// A green build that does not say how many limits it compared cannot be told
/// apart from one that found none, and the vacuity guard in
/// [`cross_check_limit_sets`] only refuses the fully empty case. Printing the
/// numbers **per home** puts a shrinking set in front of a human reading a CI
/// log, which is the same refusal addressed to the only reader who can act on a
/// set that is still non-empty but has quietly lost half its members.
fn published_limit_counts(sources: &[Enumeration<'_>]) -> LimitCounts {
    sources
        .iter()
        .map(|s| {
            let normalized = normalize(s.text);
            let count = limit_set_region(&normalized, s.path)
                .map_or(0, |region| markers(region, PUBLISHED).len());
            (s.path.to_string(), count)
        })
        .collect()
}

/// Read every enumerating document and the page, and cross-check them. Split
/// from [`cross_check_limit_sets`] so the check itself stays pure. Returns the
/// failures and the size of the set each home contributed.
fn cross_check_files(root: &Path) -> Result<(Vec<String>, LimitCounts)> {
    let mut texts = Vec::new();
    for path in ENUMERATORS {
        let text = fs::read_to_string(root.join(path))
            .map_err(|err| anyhow::anyhow!("limits-check: cannot read {path}: {err}"))?;
        texts.push((*path, text));
    }
    let sources: Vec<Enumeration<'_>> = texts
        .iter()
        .map(|(path, text)| Enumeration {
            path,
            text: text.as_str(),
        })
        .collect();
    let page = fs::read_to_string(root.join(LIMITS))
        .map_err(|err| anyhow::anyhow!("limits-check: cannot read {LIMITS}: {err}"))?;
    Ok((
        cross_check_limit_sets(&sources, &page),
        published_limit_counts(&sources),
    ))
}

/// Collapse every run of whitespace to one space.
///
/// **This is the difference between a usable gate and one people learn to
/// delete.** An anchor is a phrase in flowing prose, and prose gets rewrapped:
/// `README.md` wraps at 76 columns, `site/index.html` wraps inside a `<td>`, and
/// the same eight-word phrase lands with a newline in a different place on each.
/// Matching raw bytes would fail on a reflow that changed nothing a reader can
/// see, which is exactly the "trains people to weaken the check" failure #224's
/// risk list names. Normalising costs nothing real: an anchor is still absent
/// when the claim is deleted, which is the thing being checked.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn surface_list(claim: &Claim) -> String {
    claim
        .surfaces
        .iter()
        .map(|a| a.path)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every `path:line` under `dir` (or `dir` itself, if it is a file) whose text
/// contains `needle`, skipping [`VENDORED`] trees, `built` output and
/// non-UTF-8 files.
///
/// The two skips are independent and both are load-bearing. [`VENDORED`] names
/// third-party trees by path, so a vendored copy stays skipped even on the day
/// somebody checks one in; [`BuildOutput`] asks git, so a build directory is
/// skipped whatever it is called and wherever it appears. Neither implies the
/// other, and dropping either one puts back a hit this gate has already
/// reported once (issue #295).
fn collect_hits(
    root: &Path,
    dir: &Path,
    needle: &str,
    built: &BuildOutput,
    out: &mut Vec<String>,
) -> Result<()> {
    if is_vendored(root, dir) || built.covers(dir) {
        return Ok(());
    }
    let meta = match fs::metadata(dir) {
        Ok(m) => m,
        // A root that does not exist is a table bug, not a passing check.
        Err(err) => bail!("limits-check: cannot stat {}: {err}", dir.display()),
    };
    if meta.is_file() {
        scan_file(root, dir, needle, out);
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for entry in entries {
        if is_vendored(root, &entry) || built.covers(&entry) {
            continue;
        }
        if entry.is_dir() {
            collect_hits(root, &entry, needle, built, out)?;
        } else {
            scan_file(root, &entry, needle, out);
        }
    }
    Ok(())
}

fn scan_file(root: &Path, file: &Path, needle: &str, out: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(file) else {
        return; // binary or unreadable: not a claim surface
    };
    let shown = file
        .strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string();
    for (i, line) in text.lines().enumerate() {
        if normalize(line).contains(&normalize(needle)) {
            out.push(format!("{shown}:{}", i + 1));
        }
    }
}

fn is_vendored(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    VENDORED.iter().any(|v| rel.starts_with(v))
}

// ---------------------------------------------------------------------------
// Tests. The gate's own non-vacuity: a check nobody has seen fail is
// decoration, which is the sentence #224 uses about this task.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testtree::TestTree;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("crates/xtask -> crates -> workspace root")
            .to_path_buf()
    }

    /// The shipping table passes against the tree it ships with. If this fails,
    /// either a page or the code moved without the other.
    #[test]
    fn every_shipped_claim_holds_against_this_tree() {
        let failures = run_claims(&root(), CLAIMS).expect("claim roots exist");
        assert!(
            failures.is_empty(),
            "published claims drifted:\n{}",
            failures.join("\n")
        );
    }

    /// **The non-vacuity test for the SURFACE half.** A claim anchored to a
    /// string that is not on the page must fail. Without this, a typo in an
    /// anchor would make the gate silently unable to fail.
    #[test]
    fn a_claim_missing_from_a_surface_fails() {
        static SURFACES: &[Anchor] = &[Anchor {
            path: LIMITS,
            needle: "this exact sentence is not on the limits page and must never be",
        }];
        static EVIDENCE: &[Evidence] = &[Evidence::Contains {
            path: "crates/vitrin-core/src/realm.rs",
            needle: "MAX_REALMS",
            means: "true, so only the surface half can fail here",
        }];
        let claims = &[Claim {
            id: "synthetic-surface",
            says: "a claim nobody published",
            issue: "none: synthetic",
            surfaces: SURFACES,
            evidence: EVIDENCE,
        }];
        let failures = run_claims(&root(), claims).expect("roots exist");
        assert_eq!(failures.len(), 1, "expected exactly one failure");
        assert!(failures[0].contains("SURFACE"), "{}", failures[0]);
    }

    /// **The non-vacuity test for the EVIDENCE half**, in both directions: a
    /// `Contains` whose needle is gone, and an `AbsentFrom` whose needle is
    /// present. This is the half a cross-surface check does not have, and the
    /// half that would have caught #224's own two false body items.
    #[test]
    fn a_claim_the_code_contradicts_fails_in_both_directions() {
        static SURFACES: &[Anchor] = &[Anchor {
            path: LIMITS,
            needle: "Where this is honest about its limits",
        }];
        static EVIDENCE: &[Evidence] = &[
            Evidence::Contains {
                path: "crates/vitrin-core/src/realm.rs",
                needle: "MAX_REALMS: usize = 4096",
                means: "false: the constant is 16",
            },
            Evidence::AbsentFrom {
                roots: &["crates/vitrin-core/src/realm.rs"],
                needle: "MAX_REALMS",
                means: "false: it is right there",
            },
        ];
        let claims = &[Claim {
            id: "synthetic-evidence",
            says: "a claim the code contradicts",
            issue: "none: synthetic",
            surfaces: SURFACES,
            evidence: EVIDENCE,
        }];
        let failures = run_claims(&root(), claims).expect("roots exist");
        assert_eq!(failures.len(), 2, "{failures:#?}");
        assert!(failures.iter().all(|f| f.contains("EVIDENCE")));
        assert!(failures[0].contains("no longer contains"));
        assert!(failures[1].contains("now appears in the tree"));
    }

    /// A claim with no evidence is refused by the table check itself, so the
    /// gate cannot decay into a pure string-agreement check one row at a time.
    #[test]
    fn a_claim_with_no_evidence_is_refused() {
        static SURFACES: &[Anchor] = &[Anchor {
            path: LIMITS,
            needle: "Where this is honest about its limits",
        }];
        let claims = &[Claim {
            id: "synthetic-empty",
            says: "a claim with nothing behind it",
            issue: "none: synthetic",
            surfaces: SURFACES,
            evidence: &[],
        }];
        let failures = run_claims(&root(), claims).expect("roots exist");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("TABLE"), "{}", failures[0]);
    }

    /// Every shipped claim names the issue behind it, or says in its own words
    /// that it has none -- #224's task 6, held by the table rather than by a
    /// reviewer's memory.
    #[test]
    fn every_claim_names_an_issue_or_says_it_has_none() {
        for claim in CLAIMS {
            let issue = claim.issue;
            assert!(
                issue.contains('#') || issue.to_ascii_lowercase().contains("no issue"),
                "claim {} neither cites an issue nor states that it has none: {issue:?}",
                claim.id
            );
        }
    }

    // -----------------------------------------------------------------------
    // The set cross-check. Every one of these drives synthetic documents: a
    // check whose only test is "it passes against the tree it ships with"
    // cannot tell a working gate from one that matches nothing.
    // -----------------------------------------------------------------------

    /// The shipped documents enumerate the same limit set. If this fails, one
    /// of them gained or lost a limit and the others did not.
    #[test]
    fn the_shipped_documents_enumerate_the_same_limit_set() {
        let root = root();
        let texts: Vec<(&str, String)> = ENUMERATORS
            .iter()
            .map(|path| {
                (
                    *path,
                    fs::read_to_string(root.join(path)).expect("enumerating document exists"),
                )
            })
            .collect();
        let sources: Vec<Enumeration<'_>> = texts
            .iter()
            .map(|(path, text)| Enumeration {
                path,
                text: text.as_str(),
            })
            .collect();
        let page = fs::read_to_string(root.join(LIMITS)).expect("limits page exists");
        let failures = cross_check_limit_sets(&sources, &page);
        assert!(
            failures.is_empty(),
            "limit set drifted:\n{}",
            failures.join("\n")
        );
    }

    /// **Every registered home actually contributes.** `ENUMERATORS` is a
    /// hand-written list, and a path added to it that enumerates nothing would
    /// make the gate's success line read like coverage it does not have. The
    /// per-home rule inside the check refuses that at run time; this refuses it
    /// against the tree that ships, which is where a stub region would sit.
    #[test]
    fn every_registered_home_enumerates_at_least_one_limit() {
        let root = root();
        assert!(
            !ENUMERATORS.is_empty(),
            "no enumerating document registered"
        );
        for path in ENUMERATORS {
            let text = fs::read_to_string(root.join(path)).expect("enumerating document exists");
            let normalized = normalize(&text);
            let region = limit_set_region(&normalized, path).expect("region delimited");
            let declared = markers(region, PUBLISHED).len() + markers(region, OFF_PAGE).len();
            assert!(
                declared > 0,
                "{path} is registered as a home for the limit set and declares no limit"
            );
        }
    }

    /// A minimal pair of documents that agree, so each test below can introduce
    /// exactly one divergence and nothing else.
    fn agreeing() -> (String, String) {
        let plan = format!(
            "prose before\n\n{REGION_BEGIN}\n\n\
             - <!-- limit: no-key-repeat-on-drm -->\n  **A held key does not repeat.**\n\
             - <!-- limit-not-on-page: kdf-in-the-tcb -- a dependency cost, not a limit a \
             reader meets -->\n  **A KDF is now in the TCB.**\n\n{REGION_END}\n\n\
             ### Measurements\n\n- not a limit, and outside the region\n"
        );
        let page = "# limits\n\n<!-- limit: no-key-repeat-on-drm -->\n\
                    **A held key does not repeat at all.** In a different register.\n"
            .to_string();
        (plan, page)
    }

    #[test]
    fn the_agreeing_pair_is_clean_so_the_tests_below_isolate_one_change() {
        let (plan, page) = agreeing();
        assert!(cross_check_limit_set(&plan, &page).is_empty());
    }

    /// **Direction one: a §6 limit with nothing published.** The plan says a
    /// limit is on the page; the page does not carry it.
    #[test]
    fn a_plan_limit_the_page_does_not_publish_fails() {
        let (plan, _) = agreeing();
        let page = "# limits\n\n**A held key does not repeat at all.**\n";
        let failures = cross_check_limit_set(&plan, page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("SET"), "{}", failures[0]);
        assert!(
            failures[0].contains("no-key-repeat-on-drm"),
            "{}",
            failures[0]
        );
    }

    /// **Direction two: a published WS-E limit absent from §6.** The page
    /// carries a limit the plan document's set does not enumerate at all.
    #[test]
    fn a_published_limit_the_plan_does_not_carry_fails() {
        let (plan, page) = agreeing();
        let page = format!("{page}\n<!-- limit: one-output -->\n**Exactly one output.**\n");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("SET"), "{}", failures[0]);
        assert!(
            failures[0].contains("does not enumerate it"),
            "{}",
            failures[0]
        );
    }

    /// Publishing something §6 says is deliberately unpublished is the third
    /// direction, and its message must not read like direction two.
    #[test]
    fn an_off_page_limit_the_page_publishes_anyway_fails() {
        let (plan, page) = agreeing();
        let page = format!("{page}\n<!-- limit: kdf-in-the-tcb -->\n**A KDF is in the TCB.**\n");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("deliberately NOT"), "{}", failures[0]);
    }

    /// **Rule 2.** A new §6 limit with no marker is unpublished and invisible,
    /// which is worse than either drift direction because nothing reports it.
    #[test]
    fn a_plan_bullet_with_no_marker_fails() {
        let (plan, page) = agreeing();
        let added = format!("- **A limit somebody added without a marker.**\n\n{REGION_END}");
        let plan = plan.replace(REGION_END, &added);
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("BULLET"), "{}", failures[0]);
    }

    /// The escape hatch costs a reason, or it is not a cost at all.
    #[test]
    fn an_off_page_marker_with_no_reason_fails() {
        let (plan, page) = agreeing();
        let plan = plan.replace("-- a dependency cost, not a limit a reader meets ", "-- ");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("no reason"), "{}", failures[0]);
    }

    /// Two entries under one id make the comparison ambiguous in both
    /// directions, so it is refused rather than resolved.
    #[test]
    fn a_duplicated_id_fails() {
        let (plan, page) = agreeing();
        let page = format!("{page}\n<!-- limit: no-key-repeat-on-drm -->\n**Again.**\n");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("more than once"), "{}", failures[0]);
    }

    /// **The reflow property.** A marker wrapped across source lines -- which a
    /// long off-page reason on a 76-column page must be -- is still one marker.
    /// Without this, writing the reason at all would turn the build red.
    #[test]
    fn a_marker_wrapped_across_lines_is_still_found() {
        let plan = format!(
            "{REGION_BEGIN}\n\n\
             - <!-- limit:\n  no-key-repeat-on-drm\n  -->\n  **A held key does not repeat.**\n\n\
             {REGION_END}\n"
        );
        let page = "<!-- limit: no-key-repeat-on-drm -->\n**Reworded entirely.**\n";
        assert!(cross_check_limit_set(&plan, page).is_empty());
    }

    /// Rewriting either document's prose, in either register, moves nothing.
    /// This is the property the whole shape exists for and the reason an anchor
    /// phrase was refused: two registers, one set.
    #[test]
    fn rewording_either_document_changes_nothing() {
        let (plan, page) = agreeing();
        let plan = plan.replace(
            "**A held key does not repeat.**",
            "**Autorepeat is absent on bare metal**, and here is a paragraph of new argument.",
        );
        let page = page.replace(
            "**A held key does not repeat at all.** In a different register.",
            "**Hold a key on a real panel and you get one character.** Nothing else changed.",
        );
        assert!(cross_check_limit_set(&plan, &page).is_empty());
    }

    /// A region that is not delimited holds nothing, so its absence is a
    /// failure rather than an empty set that passes.
    #[test]
    fn a_missing_region_delimiter_fails() {
        let (plan, page) = agreeing();
        let plan = plan.replace(REGION_BEGIN, "");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("REGION"), "{}", failures[0]);
    }

    /// A second delimiter moves where the set starts, which is how §6's own
    /// prose about this mechanism broke it the first time it was written.
    #[test]
    fn a_duplicated_region_delimiter_fails() {
        let (plan, page) = agreeing();
        let plan = format!("prose naming {REGION_BEGIN} literally\n\n{plan}");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("2 times"), "{}", failures[0]);
    }

    /// Bullets outside the region are not limits. §6 ends with a measurements
    /// subsection whose bullets are numbers, and the shipped documents would
    /// fail rule 2 on every one of them if the region were not honoured.
    #[test]
    fn bullets_outside_the_region_are_not_limits() {
        let (plan, page) = agreeing();
        assert!(plan.contains("- not a limit, and outside the region"));
        assert!(cross_check_limit_set(&plan, &page).is_empty());
    }

    /// **Rule 5, and the reason it exists.** Two empty sets are equal. Before
    /// the vacuity guard, emptying §6's region and stripping the limits page's
    /// markers -- one restructure, or one rename of the marker spelling applied
    /// to both files -- produced a GREEN build whose success line read
    /// "enumerates the same limit set". A gate that certifies divergence as
    /// agreement is worse than no gate; this repository already has that
    /// cautionary example in `.github/vkms/run-advisory.sh`.
    #[test]
    fn a_comparison_between_two_empty_sets_is_refused_rather_than_passing() {
        let plan = format!("{REGION_BEGIN}\n\n{REGION_END}\n");
        let failures = cross_check_limit_set(&plan, "# limits\n\nNo markers anywhere.\n");
        // Two failures, and they are different refusals rather than one
        // reported twice: the home declares nothing (so it is a region, not an
        // enumeration), AND the union has nothing to compare against the page.
        // The second is the one that survives when several homes each declare
        // only off-page limits, which is why both exist.
        assert_eq!(failures.len(), 2, "{failures:#?}");
        assert!(
            failures.iter().any(|f| f.contains("nothing to compare")),
            "{failures:#?}"
        );
        assert!(
            failures
                .iter()
                .any(|f| f.contains("declares no limit at all")),
            "{failures:#?}"
        );
    }

    /// The same vacuity, reached the other cheap way: convert every §6 marker to
    /// the off-page escape hatch and strip the page. The region is not empty and
    /// every reason is written, so rules 1 to 4 all hold over an empty
    /// comparison.
    #[test]
    fn a_set_that_is_entirely_off_page_with_a_bare_page_is_refused() {
        let plan = format!(
            "{REGION_BEGIN}\n\n\
             - <!-- limit-not-on-page: no-key-repeat-on-drm -- a reason somebody wrote -->\n  \
             **A held key does not repeat.**\n\n{REGION_END}\n"
        );
        let failures = cross_check_limit_set(&plan, "# limits\n\nNo markers anywhere.\n");
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(
            failures[0].contains("nothing to compare"),
            "{}",
            failures[0]
        );
    }

    /// **A false green that was really there.** Rule 2's scan used to find the
    /// region by whole-line equality, so a `begin` line carrying any other text
    /// left it outside the region for the whole document: the set comparison
    /// went on working, rule 2 held nothing, and nothing said so. An unmarked
    /// limit added underneath passed green.
    #[test]
    fn a_delimiter_line_with_trailing_text_still_bounds_rule_two() {
        let (plan, page) = agreeing();
        let plan = plan.replace(REGION_BEGIN, &format!("{REGION_BEGIN} <!-- WS-E -->"));
        let plan = plan.replace(
            REGION_END,
            &format!("- **A limit added with no marker.**\n\n{REGION_END}"),
        );
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("BULLET"), "{}", failures[0]);
    }

    /// And where the two parsers genuinely disagree -- a delimiter wrapped
    /// across two source lines, which the normalised set comparison finds and a
    /// line-based scan cannot -- the scan reports its own blindness rather than
    /// returning an empty verdict that reads like a pass.
    #[test]
    fn a_region_the_line_scan_cannot_find_is_reported() {
        let (plan, page) = agreeing();
        let plan = plan.replace(REGION_BEGIN, "<!-- limit-set:\nbegin -->");
        let failures = cross_check_limit_set(&plan, &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("REGION"), "{}", failures[0]);
        assert!(failures[0].contains("held nothing"), "{}", failures[0]);
    }

    /// **The second false green.** `- `, `* ` and `+ ` are the same list in
    /// Markdown and render identically. The scan used to recognise only `- `, so
    /// a limit added with `* ` and no marker was invisible to rule 2 -- exactly
    /// the "enumerated for itself and told nobody" failure rule 2 exists for.
    #[test]
    fn a_star_bullet_is_a_list_item_too() {
        let (plan, page) = agreeing();
        for marker in ["* ", "+ ", "1. "] {
            let plan = plan.replace(
                REGION_END,
                &format!("{marker}**A limit with another list marker, unmarked.**\n\n{REGION_END}"),
            );
            let failures = cross_check_limit_set(&plan, &page);
            assert_eq!(failures.len(), 1, "{marker:?}: {failures:#?}");
            assert!(
                failures[0].contains("BULLET"),
                "{marker:?}: {}",
                failures[0]
            );
        }
    }

    /// Rule 2 must not fire on the things that are not list items: a horizontal
    /// rule, a bold continuation line, an emphasised word at column zero.
    #[test]
    fn things_that_look_like_list_items_and_are_not() {
        for line in [
            "---",
            "***",
            "**Bold.** A paragraph.",
            "-no space",
            "  - indented",
        ] {
            assert!(!is_top_level_item(line), "{line:?} is not a list item");
        }
        for line in ["- a", "* a", "+ a", "1. a", "12) a"] {
            assert!(is_top_level_item(line), "{line:?} is a list item");
        }
    }

    // -----------------------------------------------------------------------
    // More than one enumerating home. Every test here exists because the
    // single-document version did not have to answer the question, and a rule
    // nobody has seen fail is the thing this module refuses to ship.
    // -----------------------------------------------------------------------

    /// Two homes, each owning its own limits, and a page carrying both. This is
    /// the shape the tree actually ships, and the tests below each break it in
    /// exactly one place.
    fn two_homes() -> (String, String, String) {
        let ws_e = format!(
            "{REGION_BEGIN}\n\n\
             - <!-- limit: no-key-repeat-on-drm -->\n  **A held key does not repeat.**\n\n\
             {REGION_END}\n"
        );
        let phase_2 = format!(
            "{REGION_BEGIN}\n\n\
             - <!-- limit: host-must-permit-userns -->\n  **The host must permit it.**\n\n\
             {REGION_END}\n"
        );
        let page = "# limits\n\n<!-- limit: no-key-repeat-on-drm -->\n**One character.**\n\n\
                    <!-- limit: host-must-permit-userns -->\n**It refuses to start.**\n"
            .to_string();
        (ws_e, phase_2, page)
    }

    fn homes<'a>(ws_e: &'a str, phase_2: &'a str) -> Vec<Enumeration<'a>> {
        vec![
            Enumeration {
                path: PLAN,
                text: ws_e,
            },
            Enumeration {
                path: PHASE_2,
                text: phase_2,
            },
        ]
    }

    #[test]
    fn the_two_home_pair_is_clean_so_the_tests_below_isolate_one_change() {
        let (ws_e, phase_2, page) = two_homes();
        assert!(cross_check_limit_sets(&homes(&ws_e, &phase_2), &page).is_empty());
    }

    /// **The direction #286 walked into.** A limit published on the page with no
    /// enumerating home at all is the state the gate was in before Phase 2 got
    /// a region, and it must stay red -- a home is what the next sweep is sent
    /// to, so a limit with none is one nobody will find.
    #[test]
    fn a_published_limit_no_home_enumerates_still_fails() {
        let (ws_e, phase_2, page) = two_homes();
        let page = format!("{page}\n<!-- limit: nobody-owns-this -->\n**Orphaned.**\n");
        let failures = cross_check_limit_sets(&homes(&ws_e, &phase_2), &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(
            failures[0].contains("does not enumerate it"),
            "{}",
            failures[0]
        );
        // The message must name every home it searched, or the reader cannot
        // tell a missing entry from a home that was never consulted.
        assert!(
            failures[0].contains(PLAN) && failures[0].contains(PHASE_2),
            "{}",
            failures[0]
        );
    }

    /// **A registered home that enumerates nothing is refused.** This is the
    /// carve-out the multi-home shape would otherwise be: register a document,
    /// leave its region empty, and the page's markers all pass against the other
    /// home's set while the new document holds nothing at all.
    #[test]
    fn a_home_that_enumerates_nothing_fails() {
        let (ws_e, _, page) = two_homes();
        let empty = format!("{REGION_BEGIN}\n\nProse, and no limits.\n\n{REGION_END}\n");
        let page = page.replace(
            "<!-- limit: host-must-permit-userns -->\n**It refuses to start.**\n",
            "",
        );
        let failures = cross_check_limit_sets(&homes(&ws_e, &empty), &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(
            failures[0].contains("declares no limit at all"),
            "{}",
            failures[0]
        );
        assert!(failures[0].contains(PHASE_2), "{}", failures[0]);
    }

    /// **One id, one home.** Two documents claiming the same limit is how a
    /// sweep closes it in the one it found and leaves the other standing.
    #[test]
    fn an_id_claimed_by_two_homes_fails() {
        let (ws_e, phase_2, page) = two_homes();
        let phase_2 = phase_2.replace("host-must-permit-userns", "no-key-repeat-on-drm");
        let page = page.replace(
            "<!-- limit: host-must-permit-userns -->\n**It refuses to start.**\n",
            "",
        );
        let failures = cross_check_limit_sets(&homes(&ws_e, &phase_2), &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("more than once"), "{}", failures[0]);
        assert!(
            failures[0].contains(PLAN) && failures[0].contains(PHASE_2),
            "{}",
            failures[0]
        );
    }

    /// The same collision across *kinds*: one home publishes what another
    /// declares deliberately unpublished. Neither direction of the set
    /// comparison sees it -- the id is published somewhere and it is on the
    /// page -- so only the one-declaration rule can.
    #[test]
    fn an_id_one_home_publishes_and_another_declares_off_page_fails() {
        let (ws_e, _, page) = two_homes();
        let phase_2 = format!(
            "{REGION_BEGIN}\n\n\
             - <!-- limit-not-on-page: no-key-repeat-on-drm -- a reason somebody wrote -->\n  \
             **The same limit, declared unpublished.**\n\n{REGION_END}\n"
        );
        let page = page.replace(
            "<!-- limit: host-must-permit-userns -->\n**It refuses to start.**\n",
            "",
        );
        let failures = cross_check_limit_sets(&homes(&ws_e, &phase_2), &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("more than once"), "{}", failures[0]);
        assert!(failures[0].contains("off-page"), "{}", failures[0]);
    }

    /// **Rule 2 runs over every home**, not only the first. Without this, the
    /// new region would be the one place a limit could be written down and told
    /// to nobody -- which is the exact failure rule 2 exists for.
    #[test]
    fn an_unmarked_bullet_in_the_second_home_fails() {
        let (ws_e, phase_2, page) = two_homes();
        let phase_2 = phase_2.replace(
            REGION_END,
            &format!("- **A limit added to the new region with no marker.**\n\n{REGION_END}"),
        );
        let failures = cross_check_limit_sets(&homes(&ws_e, &phase_2), &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("BULLET"), "{}", failures[0]);
        assert!(failures[0].contains(PHASE_2), "{}", failures[0]);
    }

    /// A home whose region is gone stops the comparison rather than turning
    /// every limit it owns into a "published with no home" report. The real
    /// cause is one line; a pile of derived failures buries it.
    #[test]
    fn a_home_with_no_region_stops_the_comparison() {
        let (ws_e, phase_2, page) = two_homes();
        let phase_2 = phase_2.replace(REGION_BEGIN, "");
        let failures = cross_check_limit_sets(&homes(&ws_e, &phase_2), &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("REGION"), "{}", failures[0]);
        assert!(failures[0].contains(PHASE_2), "{}", failures[0]);
    }

    /// No home at all compares the page against nothing, which every rule
    /// satisfies. Rule 5, one level up.
    #[test]
    fn no_enumerating_home_at_all_is_refused() {
        let (_, _, page) = two_homes();
        let failures = cross_check_limit_sets(&[], &page);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(failures[0].contains("REGION"), "{}", failures[0]);
    }

    /// Vendored third-party trees never satisfy or break an absence check.
    /// Without this, `AbsentFrom` over `shim/` would fail on wlroots' own
    /// XWayland sources and the obvious repair would be to delete the check.
    ///
    /// # What #295 changed here
    ///
    /// The property is now stated as **every hit is a file this repository
    /// tracks**, which is what "third-party trees do not answer questions
    /// about this project's code" has always meant. The older form asserted
    /// only that no hit contained the string `subprojects`, and #295 walked
    /// straight past it: `shim/build/` is meson output, gitignored, with a
    /// full copy of wlroots' generated XWayland sources inside it. The copy
    /// under `shim/build/subprojects/` did contain the word, so the assertion
    /// fired -- but only in a tree where the shim had been built, so CI never
    /// saw it and the ~1500 other files of build output it walked would not
    /// have tripped the old assertion at all.
    ///
    /// Non-emptiness is not decoration. A filter that excluded all of `shim/`
    /// satisfies every other line of this test, and would silence every
    /// absence check that names a root under it.
    #[test]
    fn vendored_trees_are_skipped() {
        let root = root();
        let built = BuildOutput::of_tree(&root).expect("the workspace is a git work tree");
        let mut hits = Vec::new();
        collect_hits(&root, &root.join("shim"), "xwayland", &built, &mut hits)
            .expect("shim exists");
        assert!(
            !hits.is_empty(),
            "not one hit under shim/ for a string the shim's own build file contains: this walk \
             is reading nothing, and every absence check over shim/ now passes vacuously"
        );
        let tracked = tracked_files(&root, "shim");
        for hit in &hits {
            let file = hit.rsplit_once(':').expect("hits are path:line").0;
            assert!(
                tracked.contains(file),
                "{hit} is not tracked by git, so it is not this repository's own text and \
                 cannot answer a question about it.\n    All hits: {hits:?}"
            );
        }
    }

    /// **The built-tree lever for [`collect_hits`], constructed rather than
    /// found.** #295 reproduces only where somebody has run `meson setup`, so
    /// held against the shipped tree alone the skip above is exercised on one
    /// developer's machine and is vacuous everywhere else -- which is the
    /// exact shape of the bug: green in CI for months.
    ///
    /// The layout is the real one in miniature, and each of the two skips is
    /// the only thing that can catch its own file:
    ///
    /// * `shim/subprojects/wlroots/xwayland/` -- vendored source, untracked
    ///   but **not** ignored in this tree, so only [`VENDORED`] skips it.
    ///   This is the case the docstring above describes, and it must still be
    ///   skipped after #295 or the fix weakened the guard instead of fixing
    ///   the walk.
    /// * `shim/build/subprojects/wlroots/protocol/` -- the same generated
    ///   sources copied into a gitignored meson build directory, which no
    ///   path list names, so only [`BuildOutput`] skips it.
    /// * `shim/meson.build` -- the shim's own file, which really does say the
    ///   word, and must still be found.
    #[test]
    fn neither_a_vendored_source_nor_build_output_answers_an_absence_check() {
        let tree = TestTree::new("limits-absence");
        tree.write(".gitignore", "/shim/build/\n");
        tree.write("shim/meson.build", "# xwayland support is off\n");
        tree.write(
            "shim/subprojects/wlroots/xwayland/xwm.c",
            "static void xwayland_surface_destroy(struct wlr_xwayland_surface *s) {}\n",
        );
        tree.write(
            "shim/build/subprojects/wlroots/protocol/xwayland-shell-v1-protocol.c",
            "static const struct wl_interface xwayland_shell_v1_interface = { 0 };\n",
        );
        tree.git_init();

        let root = tree.path();
        let built = BuildOutput::of_tree(root).expect("a git work tree");
        let mut hits = Vec::new();
        collect_hits(root, &root.join("shim"), "xwayland", &built, &mut hits).expect("shim exists");
        assert_eq!(
            hits,
            ["shim/meson.build:1"],
            "the shim's own file is the only hit this tree may produce"
        );
    }

    /// The repository-relative paths git tracks under `dir`.
    fn tracked_files(root: &Path, dir: &str) -> std::collections::BTreeSet<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "-z", dir])
            .output()
            .expect("git is on PATH");
        assert!(out.status.success(), "git ls-files {dir} failed");
        let listing = String::from_utf8(out.stdout).expect("tracked paths are UTF-8");
        listing
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()
    }

    // -----------------------------------------------------------------------
    // #172: the derived tables, the mirrors, and the issue-citation half.
    // -----------------------------------------------------------------------

    /// The shipped derived rows hold against the tree they ship with, and the
    /// mirrors do too. The counterpart of
    /// `every_shipped_claim_holds_against_this_tree`.
    #[test]
    fn every_shipped_derived_value_and_mirror_holds_against_this_tree() {
        for (table, label) in [(DERIVED, "DERIVED"), (MIRRORS, "MIRROR")] {
            let (failures, values) = run_derived(&root(), table, label);
            assert!(
                failures.is_empty(),
                "{label} drifted:\n{}",
                failures.join("\n")
            );
            assert_eq!(
                values.len(),
                table.len(),
                "{label} produced no value for some row, so that row held nothing"
            );
        }
    }

    /// **The non-vacuity test for [`DERIVED`], and the one that matters most.**
    ///
    /// Perturb the canonical value alone and EVERY rendering must stop
    /// matching. This is the property `Anchor` does not have: with a literal
    /// needle, changing the constant and the table's needle together leaves the
    /// published surfaces stale and the build green, which is how the Landlock
    /// floor's four published `6`s were unheld and how the AppArmor kernel
    /// attribution drifted six ways.
    ///
    /// It runs the shipped rows against the shipped tree, so a row whose
    /// renderings are accidentally satisfied by unrelated text is caught here
    /// rather than the day the value moves.
    #[test]
    fn changing_the_canonical_value_alone_fails_every_rendering() {
        let root = root();
        for row in DERIVED.iter().chain(MIRRORS.iter()) {
            let real = read_source(&root, &row.source).expect("shipped row reads");
            let perturbed: Vec<String> = real.iter().map(|v| perturb(v)).collect();
            for rendering in row.renderings {
                let want = (rendering.render)(&perturbed);
                let text = fs::read_to_string(root.join(rendering.path)).expect("surface exists");
                let scan = scan_surface(&text, &want, rendering.context).expect("well-formed row");
                assert!(
                    scan.agreeing.is_empty(),
                    "[{}] {} still renders {want:?} at line(s) {} after the canonical value moved \
                     from {:?} to {:?}. That rendering is satisfied by something other than this \
                     value, so the row cannot fail and holds nothing.",
                    row.id,
                    rendering.path,
                    lines(&scan.agreeing),
                    real,
                    perturbed
                );
            }
        }
    }

    /// **The second non-vacuity test for [`DERIVED`]:** a render function that
    /// ignores its input is a row that can never fail, and no runtime check can
    /// see it -- the output is a perfectly good non-empty string that every
    /// surface happens to contain.
    #[test]
    fn every_rendering_moves_when_the_canonical_value_moves() {
        for row in DERIVED.iter().chain(MIRRORS.iter()) {
            let a = vec!["1".to_string(); 8];
            let b = vec!["2".to_string(); 8];
            for rendering in row.renderings {
                let ra = (rendering.render)(&a);
                let rb = (rendering.render)(&b);
                assert_ne!(
                    ra, rb,
                    "[{}] the rendering for {} produced {ra:?} for both values. It drops its \
                     input, so this row would pass whatever the canonical source says.",
                    row.id, rendering.path
                );
                assert!(
                    !ra.trim().is_empty(),
                    "[{}] the rendering for {} is empty, and every string contains the empty \
                     string.",
                    row.id,
                    rendering.path
                );
            }
        }
    }

    /// A row whose canonical literal appears twice is refused rather than
    /// silently taking the first. `shim/wlcs/README.md` contains both the
    /// counts line and prose about the counts format, and a bare `total=`
    /// would match both.
    #[test]
    fn a_canonical_literal_that_appears_twice_is_refused() {
        let err = read_value(
            "x = 1\nlater we mention x = 2 as well\n",
            "synthetic",
            &Read {
                after: "x = ",
                shape: Shape::Digits,
            },
        )
        .expect_err("two occurrences must be refused");
        assert!(err.contains("2 times"), "{err}");
    }

    /// A canonical literal that is gone is a failure, not a pass. Without this
    /// the whole mechanism degrades to "no value, no renderings checked".
    #[test]
    fn a_canonical_literal_that_is_gone_fails() {
        let err = read_value(
            "nothing here\n",
            "synthetic",
            &Read {
                after: "x = ",
                shape: Shape::Digits,
            },
        )
        .expect_err("a missing literal must be refused");
        assert!(err.contains("does not contain"), "{err}");
    }

    /// A row with no rendering holds nothing and must say so.
    #[test]
    fn a_derived_row_with_no_rendering_is_refused() {
        static ROW: &[Derived] = &[Derived {
            id: "synthetic-no-rendering",
            says: "a value nobody publishes",
            issue: "No issue: synthetic",
            source: Source::File {
                path: "crates/vitrin-realm-init/src/lib.rs",
                reads: &[Read {
                    after: "pub const LANDLOCK_MIN_ABI: u32 = ",
                    shape: Shape::Digits,
                }],
            },
            renderings: &[],
        }];
        let (failures, values) = run_derived(&root(), ROW, "DERIVED");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0].contains("no surface that renders"),
            "{failures:?}"
        );
        assert!(values.is_empty());
    }

    /// A rendering the surface does not carry fails, and the message points at
    /// the surface rather than at the table.
    #[test]
    fn a_rendering_a_surface_does_not_carry_fails() {
        fn never_on_the_page(v: &[String]) -> String {
            format!("the floor of this build is {} parsecs", v[0])
        }
        static ROW: &[Derived] = &[Derived {
            id: "synthetic-rendering",
            says: "a rendering no surface uses",
            issue: "No issue: synthetic",
            source: Source::File {
                path: "crates/vitrin-realm-init/src/lib.rs",
                reads: &[Read {
                    after: "pub const LANDLOCK_MIN_ABI: u32 = ",
                    shape: Shape::Digits,
                }],
            },
            renderings: &[Rendering {
                path: LIMITS,
                render: never_on_the_page,
                context: "the floor of this build is",
            }],
        }];
        let (failures, _) = run_derived(&root(), ROW, "DERIVED");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("does not render"), "{failures:?}");
    }

    /// A `FileCount` source over an empty set is refused rather than rendering
    /// as "no", which would quietly turn every published sentence into a
    /// different true statement.
    #[test]
    fn a_file_count_over_an_empty_set_is_refused() {
        let err = read_source(
            &root(),
            &Source::FileCount {
                dir: "tests/kernel-matrix/rows",
                suffix: ".this-suffix-matches-nothing",
            },
        )
        .expect_err("an empty count must be refused");
        assert!(err.contains("no `*"), "{err}");
    }

    /// **The non-vacuity test for the ISSUE half.** A claim naming an issue no
    /// surface cites must fail.
    #[test]
    fn a_claim_whose_issue_no_surface_cites_fails() {
        static SURFACES: &[Anchor] = &[Anchor {
            path: LIMITS,
            needle: "Where this is honest about its limits",
        }];
        static EVIDENCE: &[Evidence] = &[Evidence::Contains {
            path: "crates/vitrin-core/src/realm.rs",
            needle: "MAX_REALMS",
            means: "true, so only the issue half can fail here",
        }];
        let claims = &[Claim {
            id: "synthetic-issue",
            says: "a claim whose issue nobody published",
            issue: "#999999 owns this.",
            surfaces: SURFACES,
            evidence: EVIDENCE,
        }];
        let failures = run_claims(&root(), claims).expect("roots exist");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("ISSUE"), "{failures:?}");
    }

    /// ...and a claim that says in words it has none is not required to cite
    /// one. Fourteen shipped rows depend on this, and forcing an issue on them
    /// would mean opening a tracker entry for every permanent exclusion.
    #[test]
    fn a_claim_that_declares_it_has_no_issue_is_not_asked_to_cite_one() {
        assert_eq!(
            tracking_issue("No issue, deliberately: #224 decided..."),
            None
        );
        assert_eq!(
            tracking_issue("#213 (WS-E.2.1, closed). #224's body got it wrong."),
            Some("#213".to_string())
        );
        assert_eq!(tracking_issue("a sentence with no number"), None);
    }

    // -----------------------------------------------------------------------
    // The self-drift scan: a surface must not be able to disagree with itself.
    // -----------------------------------------------------------------------

    /// **The non-vacuity test for [`scan_surface`], and the reason it exists at
    /// all.** A page that states the value twice, agreeing once and disagreeing
    /// once, must fail -- and must name both lines, because a failure that says
    /// only "somewhere on this page" sends a reader to grep.
    #[test]
    fn a_surface_that_contradicts_itself_fails_and_names_both_lines() {
        let surface = "\
intro line
the floor is **6** in this build, refused below it
some prose
more prose
and further down the floor is **7** in this build
";
        let scan = scan_surface(surface, "**6** in this", "** in this").expect("well-formed row");
        assert_eq!(scan.agreeing, vec![2], "{scan:?}");
        assert_eq!(scan.disagreeing, vec![5], "{scan:?}");
    }

    /// The same surface with both occurrences agreeing is clean, so the test
    /// above isolates the disagreement rather than the shape.
    #[test]
    fn a_surface_that_states_the_value_twice_and_agrees_is_clean() {
        let surface = "\
the floor is **6** in this build
and further down the floor is **6** in this build
";
        let scan = scan_surface(surface, "**6** in this", "** in this").expect("well-formed row");
        assert_eq!(scan.agreeing, vec![1, 2], "{scan:?}");
        assert!(scan.disagreeing.is_empty(), "{scan:?}");
    }

    /// A first-hit `contains` calls the contradicting surface green. This test
    /// exists so the weaker check cannot be reintroduced as a simplification
    /// without a red build saying what it costs.
    #[test]
    fn the_check_this_replaced_would_have_passed_that_surface() {
        let surface = "the floor is **6** in this build\nand later **7** in this build\n";
        assert!(
            normalize(surface).contains(&normalize("**6** in this")),
            "the old containment test passes this surface, which is the point"
        );
        let scan = scan_surface(surface, "**6** in this", "** in this").expect("well-formed row");
        assert!(!scan.disagreeing.is_empty(), "{scan:?}");
    }

    /// A rendering wrapped across a line break is still one occurrence: the
    /// scan runs over normalized text, exactly as [`Anchor`] matching does, so
    /// a 76-column reflow cannot turn the build red.
    #[test]
    fn a_rendering_wrapped_across_lines_is_still_one_agreeing_occurrence() {
        let surface = "the floor is **6**\nin this build\n";
        let scan = scan_surface(surface, "**6** in this", "** in this").expect("well-formed row");
        assert_eq!(scan.agreeing, vec![1], "{scan:?}");
        assert!(scan.disagreeing.is_empty(), "{scan:?}");
    }

    /// A context the rendered form does not contain exactly once is a table bug
    /// and is refused, because either resolution silently picks which
    /// occurrence to hold.
    #[test]
    fn a_context_that_is_not_exactly_once_in_the_rendering_is_refused() {
        let err = scan_surface("anything", "**6** in this", "not in the rendering")
            .expect_err("a context absent from the rendering must be refused");
        assert!(err.contains("occurs 0 time(s)"), "{err}");
        let err = scan_surface("anything", "6 and 6", "6")
            .expect_err("an ambiguous context must be refused");
        assert!(err.contains("occurs 2 time(s)"), "{err}");
        let err =
            scan_surface("anything", "**6**", " ").expect_err("an empty context must be refused");
        assert!(err.contains("empty"), "{err}");
    }

    /// Every shipped context is **value-free**: rendering a different value
    /// leaves it in place, exactly once. A context that moved with the value
    /// would find only occurrences that already agree, which is the first-hit
    /// check wearing a scan's clothes.
    #[test]
    fn every_context_is_value_free() {
        for row in DERIVED.iter().chain(MIRRORS.iter()) {
            for rendering in row.renderings {
                let context = normalize(rendering.context);
                assert!(
                    !context.is_empty(),
                    "[{}] the context for {} is empty",
                    row.id,
                    rendering.path
                );
                for probe in [vec!["1".to_string(); 8], vec!["2".to_string(); 8]] {
                    let rendered = normalize(&(rendering.render)(&probe));
                    assert_eq!(
                        rendered.matches(&context).count(),
                        1,
                        "[{}] the context {context:?} occurs a number of times other than once \
                         in {} rendered from {probe:?} ({rendered:?}). A context has to survive \
                         the value changing, and has to pin exactly one offset.",
                        row.id,
                        rendering.path,
                    );
                }
            }
        }
    }

    /// The indexed normalizer must agree with [`normalize`] byte for byte, and
    /// its map must cover every byte -- a map that disagreed would print line
    /// numbers pointing at the wrong paragraph, which is worse than none.
    #[test]
    fn the_indexed_normalizer_agrees_with_the_plain_one() {
        for text in [
            "one two\tthree\n\n  four  ",
            "",
            "   ",
            "a\u{2014}b \u{2014} c\nd",
            "trailing\n",
        ] {
            let (out, map) = normalize_indexed(text);
            assert_eq!(out, normalize(text), "{text:?}");
            assert_eq!(out.len(), map.len(), "{text:?}");
            for (i, at) in map.iter().enumerate() {
                assert!(*at < text.len(), "{text:?} byte {i} maps past the end");
            }
        }
    }

    /// Line numbers are 1-based and count the input's own newlines.
    #[test]
    fn line_numbers_are_the_ones_an_editor_shows() {
        let text = "a\nbb\nccc\n";
        assert_eq!(line_at(text, 0), 1);
        assert_eq!(line_at(text, 2), 2);
        assert_eq!(line_at(text, 5), 3);
        assert_eq!(line_at(text, 9999), 4);
    }

    // -----------------------------------------------------------------------
    // The coverage roll: a set that shrinks must turn something red.
    // -----------------------------------------------------------------------

    /// The shipped tables match their shipped rolls. This is the assertion that
    /// makes `cargo test` red the moment a row is deleted without the roll.
    #[test]
    fn every_shipped_table_matches_its_coverage_roll() {
        let failures = shipped_coverage_failures();
        assert!(
            failures.is_empty(),
            "the tables and their coverage rolls disagree:\n{}",
            failures.join("\n")
        );
    }

    /// **The non-vacuity test for the roll.** Deleting a row must fail, adding
    /// one must fail, and a duplicated id must fail -- all three, because the
    /// property is "the set is exactly this" and not "the set is at least this".
    #[test]
    fn a_table_that_lost_a_row_fails_and_names_the_id_that_left() {
        let listed = &["a", "b", "c"];

        let failures = coverage_failures("ROLL", "TABLE", &["a", "c"], listed);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("[b] COVERAGE"), "{failures:?}");
        assert!(failures[0].contains("COVERAGE SHRANK"), "{failures:?}");

        let failures = coverage_failures("ROLL", "TABLE", &["a", "b", "c", "d"], listed);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("[d] COVERAGE"), "{failures:?}");
        assert!(failures[0].contains("does not list it"), "{failures:?}");

        let failures = coverage_failures("ROLL", "TABLE", &["a", "b", "c", "c"], listed);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("share this id"), "{failures:?}");

        assert!(coverage_failures("ROLL", "TABLE", &["c", "b", "a"], listed).is_empty());
    }

    /// A nested issue number is not a citation. `#15` is a substring of `#155`,
    /// and this repository has both shapes in its tracker, so a bare `contains`
    /// would report a claim as cited by a page that never names it.
    #[test]
    fn a_longer_issue_number_does_not_cite_a_shorter_one() {
        assert!(!cites("closed by #155 and #156", "#15"));
        assert!(cites("closed by #155 and #156", "#155"));
        assert!(cites("see #15.", "#15"));
        assert!(cites("see #15", "#15"));
        assert!(!cites("see #1550", "#155"));
    }

    /// Change one character of a value so a rendering of it cannot match by
    /// accident. Digits move to a different digit; anything else gains a
    /// character that no surface here contains.
    fn perturb(value: &str) -> String {
        if value.chars().all(|c| c.is_ascii_digit()) {
            // 7 -> 8, everything else -> 7. Never a no-op, and never a value
            // some other row publishes.
            return if value == "7" { "8" } else { "7" }.to_string();
        }
        format!("{value}-zzq")
    }
}
