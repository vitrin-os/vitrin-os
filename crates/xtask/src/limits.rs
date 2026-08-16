// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask limits-check` -- the published-claim drift gate (WS-E.4.4,
//! issue #224, acceptance criterion 1).
//!
//! # THIS IS EXPLICITLY TEMPORARY, AND IT SAYS SO IN ITS OWN COMMENTS
//!
//! Issue #224's first task reads: *"Reuse #172's chosen drift mechanism rather
//! than inventing a second convention. If #172 has not landed, add an
//! explicitly temporary claim-string check and label it as such in its own
//! comments, so it is replaced rather than entrenched."*
//!
//! **#172 has not landed.** It is open, it has not chosen between its three
//! candidate shapes (a single generated source of truth; a claim-string drift
//! check; a checklist convention in the issue template), and
//! `.github/workflows/ci.yml` carried zero references to `docs/book/src/limits.md`
//! before this module. So this is #172's **option (b)**, built narrow, and it
//! is a placeholder for whatever #172 decides -- not the decision.
//!
//! What "replaced rather than entrenched" means concretely, so a later reader
//! does not have to reconstruct it:
//!
//! * If #172 picks **option (a)**, a single source the surfaces are generated
//!   from, this module is **deleted**, not extended. A generator makes the
//!   surfaces agree by construction, and a string check over generated text
//!   checks nothing.
//! * If #172 picks **option (b)**, this module is the seed and the four claims
//!   #172 names as known to drift -- the fuzz soak, the wlcs counts, OIN, REUSE
//!   -- are the first ones to be added to [`CLAIMS`]. **They are deliberately
//!   NOT here yet**: they are #172's to normalise (the site quotes an undated
//!   `3/180` derived from counts it does not show, which is #172's own
//!   complaint), and gating a number before it is normalised would freeze the
//!   wrong wording into CI.
//! * If #172 picks **option (c)**, a checklist convention, this module stays as
//!   the machine half of it, because a checklist is exactly the thing #224's
//!   acceptance criterion refuses to accept on its own: *"Changing a WS-E claim
//!   on one surface and not the others **fails something in CI**."*
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
    pub issue: &'static str,
    pub surfaces: &'static [Anchor],
    pub evidence: &'static [Evidence],
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

/// Directories whose contents are third-party and must never satisfy or break
/// an [`Evidence::AbsentFrom`] check. `shim/subprojects/` is vendored wlroots,
/// pixman, libxkbcommon and v4l-utils; every one of them mentions X11, touch
/// and accessibility, and none of them is this project's code.
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
                        four pages moving with it -- and the constant is pinned as a whole \
                        line, value included, precisely so a silent re-tune cannot leave four \
                        published numbers stale.",
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
                needle: "pub const FLOOR: &[Mechanism] = &[Mechanism::Namespaces, \
                         Mechanism::Landlock];",
                means: "Landlock is still a STARTUP GATE and not merely applied. This is the \
                        one row that decides whether the published requirement is true at all: \
                        take `Mechanism::Landlock` back out of `FLOOR` and every page above \
                        describes a refusal that no longer happens, which is the overclaiming \
                        direction. It is pinned as the whole line rather than as \
                        `Mechanism::Landlock`, which also appears in `APPLIED` -- a mechanism \
                        can be applied without gating startup, and that distinction is exactly \
                        what this claim is about.",
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
];

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

/// Entry point for `cargo xtask limits-check`.
pub fn limits_check(root: &Path) -> Result<()> {
    let mut failures = run_claims(root, CLAIMS)?;
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
        println!(
            "limits-check: {} claims hold across their surfaces and their code evidence, and the \
             enumerating plan documents ({breakdown}) enumerate the same {total} published limits \
             as {LIMITS}.",
            CLAIMS.len()
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
         * SET      -- the enumerating plan documents and docs/book/src/limits.md no longer \
         enumerate the same limits. This one is about the SET, never the wording: reword either \
         document freely, but a limit present in one and absent from the other is drift.\n  \
         * BULLET / MARKER / ID / REGION -- an enumerated limit carries no marker, a marker is \
         malformed, or the region delimiters are gone, so the set comparison above cannot see \
         it.\n\n\
         Fix the page or fix the table in crates/xtask/src/limits.rs -- but do not weaken an \
         anchor or delete a marker to make this pass. Issue #224 exists because two of its own \
         body items had gone false this way.\n",
        n = failures.len(),
    ));
    for f in &failures {
        msg.push_str(&format!("\n{f}\n"));
    }
    bail!(msg);
}

/// Run every claim and return one string per failure. Split out so the tests
/// can drive a synthetic claim table without a process exit.
pub fn run_claims(root: &Path, claims: &[Claim]) -> Result<Vec<String>> {
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
                        collect_hits(root, &root.join(r), needle, &mut hits)?;
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
/// contains `needle`, skipping [`VENDORED`] trees and non-UTF-8 files.
fn collect_hits(root: &Path, dir: &Path, needle: &str, out: &mut Vec<String>) -> Result<()> {
    if is_vendored(root, dir) {
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
        if is_vendored(root, &entry) {
            continue;
        }
        if entry.is_dir() {
            collect_hits(root, &entry, needle, out)?;
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
    #[test]
    fn vendored_trees_are_skipped() {
        let root = root();
        let mut hits = Vec::new();
        collect_hits(&root, &root.join("shim"), "xwayland", &mut hits).expect("shim exists");
        assert!(
            hits.iter().all(|h| !h.contains("subprojects")),
            "vendored hits leaked: {hits:?}"
        );
    }
}
