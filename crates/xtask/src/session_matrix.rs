// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask session-matrix [--check]` -- generate
//! `docs/book/src/session-app-matrix.md` (WS-E.4.1, issue #221).
//!
//! # Why this is a generator and not a hand-written page
//!
//! Issue #221 asks for a published list of the applications this project has
//! actually run, and names the failure mode in the same breath: a published
//! matrix is a maintenance treadmill, and a stale matrix is worse than none
//! because it is read as current. The device it borrows -- from the planned
//! P2.8.6 IME matrix (`docs/plan/02-phase-2-semantic-epochs.md`) -- is to make
//! the effort cap *structural* rather than promised in prose:
//!
//! * **A cell exists only if a runner executed it.** [`Execution`] is the only
//!   thing that renders a row in the two application tables, and it cannot be
//!   constructed without a citation to a recorded run, an observable that was
//!   checked, and a package version. There is no variant of any type here
//!   meaning "we believe this works".
//! * **The app set cannot widen by assertion.** [`Requested`] names the apps
//!   somebody wants covered; a requested app with no matching [`Execution`] is
//!   *declined* -- it is named on the page as not emitted, with its reason,
//!   and it gets no row. Widening the matrix therefore requires running the
//!   app and landing its evidence, not editing a table.
//! * **The page cannot be hand-edited.** `--check` re-renders in memory and
//!   compares byte-for-byte against the checked-in copy, and CI runs it, so a
//!   hand edit is a red build.
//!
//! # Why `--check` needs no scratch directory (unlike `codegen --check`)
//!
//! `codegen --check` regenerates into `target/` because its generator shells
//! out to `rustfmt`, which writes files. This one renders to a `String` with
//! no external formatter and no filesystem writes at all, so `--check` reads
//! the checked-in bytes, compares, and touches nothing. It is the same
//! promise -- the working tree is left exactly as found either way it exits --
//! reached the cheap way.
//!
//! Like `codegen --check`, this deliberately does not use `git diff`/`git
//! status`: a direct filesystem comparison is correct whether the page is
//! untracked, staged or committed, and a git-based diff is blind to a path
//! with no git history at all.
//!
//! # What is NOT here, on purpose
//!
//! The generator probes nothing at run time -- no `pacman`, no `readelf`, no
//! `uname`, and above all no application. If it did, its output would depend
//! on the machine it ran on and could never be diff-clean on a GitHub runner,
//! which has no DRM device, no seat and no GPU. Every fact below is a
//! checked-in transcription of a measurement a human took, exactly as
//! `protocol/vitrin-v0.xml` is the checked-in input to `codegen`. The runbook
//! rendered onto the page is how a human takes new measurements and lands
//! them here.

use std::fs;
use std::path::Path;

use anyhow::{bail, Result};

/// The checked-in page this module owns, relative to the workspace root.
pub const PAGE_PATH: &str = "docs/book/src/session-app-matrix.md";

// ---------------------------------------------------------------------------
// Types. The invariant is carried by the shapes: nothing here can express an
// application that nobody ran.
// ---------------------------------------------------------------------------

/// Where a version number came from, because the two are not equally strong.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// The repository pins this exact build, so the recorded run provably used
    /// it (e.g. the checksummed Firefox ESR tarball).
    PinnedByRepo(&'static str),
    /// Read from the machine's package database at inventory time. Where the
    /// run it accompanies is undated, this is what is installed *now* -- not
    /// proof of what ran.
    InstalledAtInventory(&'static str),
    /// Built from this repository at the stated revision.
    BuiltFromRepo(&'static str),
    /// Shipped and self-updated by its vendor, so no package database
    /// describes what actually runs.
    VendorBundled(&'static str),
    /// Read from the measured machine, while the run it accompanies happened
    /// somewhere else -- a CI runner with its own distribution package. This
    /// is NOT the build that ran.
    NotTheBuildThatRan(&'static str),
}

impl Version {
    fn cell(self) -> String {
        match self {
            Version::PinnedByRepo(v) => format!("{v} (pinned by this repo)"),
            Version::InstalledAtInventory(v) => format!("{v} (installed at inventory)"),
            Version::BuiltFromRepo(v) => format!("{v} (built from this repo)"),
            Version::VendorBundled(v) => format!("{v} (vendor-bundled)"),
            Version::NotTheBuildThatRan(v) => {
                format!("{v} on the measured machine — NOT the build the cited CI run used")
            }
        }
    }

    fn text(self) -> &'static str {
        match self {
            Version::PinnedByRepo(v)
            | Version::InstalledAtInventory(v)
            | Version::BuiltFromRepo(v)
            | Version::VendorBundled(v)
            | Version::NotTheBuildThatRan(v) => v,
        }
    }
}

/// How high the app was made to jump. There are exactly two heights in this
/// repository and conflating them is the honesty gap issue #221 exists to
/// close.
#[derive(Clone, Copy)]
pub enum Bar {
    /// The weak bar: the app mapped a window and repainted, and nothing else
    /// was checked.
    MappedAndRepainted,
    /// A named task in this repository, which asserts something specific.
    NamedTask(&'static str),
}

impl Bar {
    fn cell(self) -> String {
        match self {
            Bar::MappedAndRepainted => "weak bar".to_string(),
            Bar::NamedTask(task) => format!("named task: {task}"),
        }
    }
}

/// Which backend the app ran against. No desktop application in this corpus
/// has ever run on the bare-metal one, and the page says so.
#[derive(Clone, Copy)]
pub enum Venue {
    Headless,
    NestedUnderHyprland,
    BareMetalDrm,
}

impl Venue {
    fn cell(self) -> &'static str {
        match self {
            Venue::Headless => "headless",
            Venue::NestedUnderHyprland => "nested under Hyprland",
            Venue::BareMetalDrm => "bare-metal DRM/KMS",
        }
    }
}

/// Why an app never reached a mapped window. The distinction is load-bearing:
/// issue #221 requires that failures with a non-X11 cause are never counted as
/// X11 gaps.
#[derive(Clone, Copy)]
pub enum Cause {
    /// The app needs an X server and this stack serves none.
    NoXServer,
    /// Not an X11 gap. Names the cause and whose problem it is.
    NotAnX11Gap {
        cause: &'static str,
        owner: &'static str,
    },
}

/// What happened.
#[derive(Clone, Copy)]
pub enum Verdict {
    /// The app reached whatever the [`Bar`] asks for.
    Met,
    /// It did not, for the stated cause.
    DidNotMap(Cause),
}

impl Verdict {
    fn cell(self) -> String {
        match self {
            Verdict::Met => "met the bar".to_string(),
            Verdict::DidNotMap(Cause::NoXServer) => {
                "**did not map** — needs an X server, and there is none".to_string()
            }
            Verdict::DidNotMap(Cause::NotAnX11Gap { cause, owner }) => {
                format!("**did not map** — {cause}. **Not an X11 gap**; owned by {owner}")
            }
        }
    }
}

/// When the run happened. Most of the seed corpus is undated, and pretending
/// otherwise by reusing the commit date would be exactly the fabrication this
/// page exists to refuse.
#[derive(Clone, Copy)]
pub enum When {
    /// The record states this date.
    Dated(&'static str),
    /// The run itself carries no date. All that is known is that the record
    /// existed by the time this commit landed -- which bounds when the row was
    /// *written*, not when the app was *run*.
    UndatedBoundedBy {
        commit: &'static str,
        date: &'static str,
    },
    /// Runs on every pull request in CI, so it has a continuous pass history
    /// and no single date. The commit is when it first landed.
    EveryPullRequestSince {
        commit: &'static str,
        date: &'static str,
    },
}

impl When {
    fn cell(self) -> String {
        match self {
            When::Dated(d) => d.to_string(),
            When::UndatedBoundedBy { commit, date } => {
                format!("**undated**; recorded by `{commit}` ({date})")
            }
            When::EveryPullRequestSince { commit, date } => {
                format!("every PR in CI since `{commit}` ({date})")
            }
        }
    }
}

/// Desktop application, or a test client this repository wrote. Nobody daily
/// drives the second kind, and the strongest evidence in the tree is about the
/// second kind, so they get separate tables.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    DesktopApp,
    RepoTestClient,
}

/// One recorded execution against `vitrind`. **The only thing that renders a
/// row in the two application tables.**
#[derive(Clone, Copy)]
pub struct Execution {
    pub app: &'static str,
    pub kind: Kind,
    pub version: Version,
    pub venue: Venue,
    pub bar: Bar,
    /// The observable that was checked. Never a bare pass -- [`render`]
    /// rejects the words that mean nothing.
    pub observable: &'static str,
    pub verdict: Verdict,
    pub when: When,
    /// Where the record is, so a reader can go and disagree.
    pub evidence: &'static str,
    /// What this row over-claims if read quickly.
    pub caveat: Option<&'static str>,
}

/// An app somebody wants covered. Carries no evidence, and therefore renders
/// no row on its own -- it is the *question*, and [`Execution`] is the answer.
#[derive(Clone, Copy)]
pub struct Requested {
    pub app: &'static str,
    /// Why it is on the list at all.
    pub why: &'static str,
    /// If it is declined, where a reader should look instead.
    pub see_also: Option<&'static str>,
}

/// What an ELF linkage measurement concluded.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    X11Only,
    HasWaylandMode,
    /// Linkage contradicted itself and only a run settles it.
    Inconclusive,
}

/// A read-only inventory measurement of the machine. **This is not a run**,
/// and it renders into its own tables, under their own heading, saying so.
#[derive(Clone, Copy)]
pub struct Linkage {
    pub app: &'static str,
    pub version: Version,
    pub family: Family,
    /// The switch that selects Wayland, or what is missing.
    pub wayland_mode: &'static str,
    /// The command that was run and what it returned.
    pub measurement: &'static str,
}

/// The single machine, the build, and the date. Issue #221's acceptance
/// criteria name every field here.
#[derive(Clone, Copy)]
pub struct Header {
    pub inventory_date: &'static str,
    pub kernel: &'static str,
    pub kernel_caveat: &'static str,
    pub mesa: &'static str,
    pub wlroots: &'static str,
    pub wlroots_note: &'static str,
    pub vitrind_revision: &'static str,
    pub revision_caveat: &'static str,
    pub machine: &'static str,
    pub host_compositor: &'static str,
    pub last_recorded_run: &'static str,
}

/// Everything the page is rendered from.
pub struct Corpus {
    pub header: Header,
    pub requested: &'static [Requested],
    pub executions: &'static [Execution],
    pub linkage: &'static [Linkage],
}

/// A requested app that got no row, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declined {
    pub app: String,
    pub why: String,
    pub see_also: Option<String>,
}

/// The rendered page plus the two things the tests interrogate.
#[derive(Debug)]
pub struct Rendered {
    pub page: String,
    pub declined: Vec<Declined>,
}

// ---------------------------------------------------------------------------
// Markdown table parsing -- so the tests read the bytes the page actually
// carries, not a parallel structure that could drift from it.
// ---------------------------------------------------------------------------

/// One data row of one markdown table on the rendered page.
#[derive(Clone, Debug)]
pub struct TableRow {
    /// Nearest preceding `##`/`###` heading.
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

fn split_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Every **data** row of every markdown table on `page` -- header rows and
/// separator rows excluded.
///
/// This is what the non-vacuity test asserts against. Parsing the emitted
/// markdown, rather than inspecting the corpus that produced it, is the point:
/// a change to [`render`] that emits an unexecuted application as a row shows
/// up here no matter which code path put it there.
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
        if let Some(rest) = trimmed.strip_prefix("### ") {
            section = rest.trim().to_string();
            continue;
        }
        if !trimmed.starts_with('|') || is_separator(line) {
            continue;
        }
        // A header row is the line immediately above a separator.
        if lines.get(i + 1).is_some_and(|next| is_separator(next)) {
            continue;
        }
        rows.push(TableRow {
            section: section.clone(),
            cells: split_cells(line),
            line: (*line).to_string(),
        });
    }
    rows
}

// ---------------------------------------------------------------------------
// Validation. A cell that says nothing is refused rather than published.
// ---------------------------------------------------------------------------

/// Words that record no observable. Issue #221 requires each cell to record
/// "app, package version, and the observable checked" -- so a cell whose
/// observable is one of these is a bug in the corpus, and the generator
/// refuses to render at all rather than publish it.
const BARE_PASS_WORDS: &[&str] = &["", "-", "n/a", "na", "pass", "passed", "ok", "works", "yes"];

fn reject_bare(field: &str, value: &str, app: &str) -> Result<()> {
    if BARE_PASS_WORDS.contains(&value.trim().to_ascii_lowercase().as_str()) {
        bail!(
            "session-matrix: {app}: {field} is {value:?}, which records nothing. Every cell must \
             name the observable that was checked (issue #221 acceptance criterion 3)."
        );
    }
    Ok(())
}

fn reject_markup(field: &str, value: &str, app: &str) -> Result<()> {
    if value.contains('|') || value.contains('\n') {
        bail!(
            "session-matrix: {app}: {field} contains a pipe or newline, which would corrupt the \
             emitted markdown table."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

/// Render the page from `corpus`.
///
/// Admission, which is the whole mechanism:
///
/// 1. Every [`Execution`] must be named in `requested` -- otherwise the two
///    lists have drifted and the run would render without anyone having asked
///    for it. That is an error, not a silent pass.
/// 2. Every [`Requested`] app with no matching [`Execution`] is **declined**:
///    it is listed on the page as not emitted, with its reason, and it gets no
///    row in any table.
/// 3. Every [`Linkage`] entry with no measurement is **dropped**: an inventory
///    claim with nothing behind it is not published either.
pub fn render(corpus: &Corpus) -> Result<Rendered> {
    let Corpus {
        header,
        requested,
        executions,
        linkage,
    } = corpus;

    // (1) No execution may render without having been asked for.
    for exec in *executions {
        if !requested.iter().any(|r| r.app == exec.app) {
            bail!(
                "session-matrix: {} has an execution record but is not in REQUESTED. Add it there \
                 (with why it is on the list) or remove the execution -- the two lists are the \
                 page's join and must not drift.",
                exec.app
            );
        }
        reject_bare("observable", exec.observable, exec.app)?;
        reject_bare("evidence", exec.evidence, exec.app)?;
        reject_bare("version", exec.version.text(), exec.app)?;
        for (field, value) in [
            ("app", exec.app),
            ("observable", exec.observable),
            ("evidence", exec.evidence),
            ("version", exec.version.text()),
        ] {
            reject_markup(field, value, exec.app)?;
        }
        if let Some(caveat) = exec.caveat {
            reject_markup("caveat", caveat, exec.app)?;
        }
    }

    // (3) An inventory row with no measurement behind it is dropped.
    let measured: Vec<&Linkage> = linkage
        .iter()
        .filter(|l| !BARE_PASS_WORDS.contains(&l.measurement.trim().to_ascii_lowercase().as_str()))
        .collect();
    for entry in &measured {
        reject_markup("measurement", entry.measurement, entry.app)?;
        reject_markup("wayland_mode", entry.wayland_mode, entry.app)?;
    }

    // (2) Requested minus executed.
    let mut declined = Vec::new();
    for req in *requested {
        if executions.iter().any(|e| e.app == req.app) {
            continue;
        }
        declined.push(Declined {
            app: req.app.to_string(),
            why: req.why.to_string(),
            see_also: req.see_also.map(str::to_string),
        });
    }

    let mut p = String::new();
    render_preamble(&mut p, header);
    render_reading_guide(&mut p);
    render_execution_table(
        &mut p,
        executions,
        Kind::DesktopApp,
        "Desktop applications executed against `vitrind`",
        DESKTOP_TABLE_INTRO,
    );
    render_execution_table(
        &mut p,
        executions,
        Kind::RepoTestClient,
        "Repository test clients executed against `vitrind`",
        TEST_CLIENT_TABLE_INTRO,
    );
    render_not_x11_gaps(&mut p, executions);
    render_linkage_table(
        &mut p,
        &measured,
        Family::X11Only,
        "The machine's X11-only software (linkage, not execution)",
        X11_ONLY_INTRO,
    );
    render_linkage_table(
        &mut p,
        &measured,
        Family::HasWaylandMode,
        "Software with a Wayland mode, so not an X11 dependency (linkage, not execution)",
        WAYLAND_MODE_INTRO,
    );
    render_linkage_table(
        &mut p,
        &measured,
        Family::Inconclusive,
        "Where linkage did not settle the question (linkage, not execution)",
        INCONCLUSIVE_INTRO,
    );
    render_declined(&mut p, &declined);
    render_gaps(&mut p);
    render_runbook(&mut p);

    verify_emitted_rows(&p)?;

    Ok(Rendered { page: p, declined })
}

/// Re-read the bytes just emitted and refuse to hand back a page carrying a
/// cell that records nothing.
///
/// The corpus is already validated above, so this looks redundant -- it is
/// not. It catches the *rendering* bug rather than the *data* bug: a column
/// added to a table but not to every row, a format string that drops a field,
/// a cell that renders empty for one enum variant. Those produce a cell that
/// says nothing from a corpus entry that said plenty, and the promise issue
/// #221 extracts is about the published cell.
fn verify_emitted_rows(page: &str) -> Result<()> {
    for row in table_rows(page) {
        for (i, cell) in row.cells.iter().enumerate() {
            if BARE_PASS_WORDS.contains(&cell.trim().to_ascii_lowercase().as_str()) {
                bail!(
                    "session-matrix: rendered a cell that records nothing (column {i} of a row \
                     under {:?}): {}",
                    row.section,
                    row.line
                );
            }
        }
    }
    Ok(())
}

fn render_preamble(p: &mut String, h: &Header) {
    p.push_str(
        "<!-- GENERATED FILE -- DO NOT EDIT.\n\
         \n\
         Produced by `cargo xtask session-matrix` from the corpus in\n\
         crates/xtask/src/session_matrix.rs. `cargo xtask session-matrix --check`\n\
         re-renders and compares byte-for-byte, and CI runs it, so a hand edit to\n\
         this file is a red build. To change what this page says, change the\n\
         measurement -- see the runbook at the bottom.\n\
         -->\n\n",
    );
    p.push_str("# Session app matrix\n\n");
    p.push_str(
        "Which applications this project has **actually run**, on **one machine**, at what\n\
         **bar**, with the **observable** that was checked named in every cell.\n\n",
    );
    p.push_str(
        "This page is generated, and the generator can only emit a cell somebody executed. An\n\
         application nobody ran does not appear as a row here no matter how confident anyone is\n\
         that it would work; it appears in [Requested, and not\n\
         emitted](#requested-and-not-emitted) instead. That is the whole design: the app set\n\
         cannot widen by assertion, only by running something and landing its evidence.\n\n",
    );
    p.push_str("## The machine, the build, and the date\n\n");
    p.push_str(
        "**One machine was measured.** Everything below is one laptop, one Intel iGPU, one\n\
         internal panel, one kernel. None of it generalises, and reading this page as \"Vitrin\n\
         runs these applications\" would be false.\n\n",
    );
    p.push_str(&format!("- **Inventory read**: {}\n", h.inventory_date));
    p.push_str(&format!(
        "- **Last recorded run in this corpus**: {}\n",
        h.last_recorded_run
    ));
    p.push_str(&format!(
        "- **Kernel**: `{}` — {}\n",
        h.kernel, h.kernel_caveat
    ));
    p.push_str(&format!("- **Mesa**: `{}`\n", h.mesa));
    p.push_str(&format!(
        "- **wlroots**: `{}` — {}\n",
        h.wlroots, h.wlroots_note
    ));
    p.push_str(&format!(
        "- **`vitrind` revision**: `{}` — {}\n",
        h.vitrind_revision, h.revision_caveat
    ));
    p.push_str(&format!("- **Machine**: {}\n", h.machine));
    p.push_str(&format!(
        "- **Host compositor for nested runs**: {}\n\n",
        h.host_compositor
    ));
    p.push_str(
        "**CI cannot produce this page's contents.** A GitHub runner has no DRM device, no seat\n\
         and no GPU, so it cannot run `vitrind` against a panel and cannot run a GUI application\n\
         at all. What CI *can* do, and does, is assert that the checked-in page is byte-identical\n\
         to what the generator emits — which catches a hand edit, and catches nothing else.\n\
         The measurements themselves come from a human executing the runbook at the bottom of\n\
         this page, on the target machine.\n\n",
    );
}

fn render_reading_guide(p: &mut String) {
    p.push_str("## How to read a cell\n\n");
    p.push_str("### The bar is weak, and here is exactly how weak\n\n");
    p.push_str(
        "Most rows below were scored at one bar: **the application mapped a window and\n\
         repainted**. Nothing was typed into it, nothing was clicked, and nothing was checked\n\
         for correct rendering.\n\n",
    );
    p.push_str(
        "An application can map a window and repaint and still be **unusable**, because a realm\n\
         is missing things a desktop application assumes it has:\n\n",
    );
    p.push_str(
        "- **No cross-realm clipboard through the app's own clipboard interface.** The shim\n\
         advertises `wl_data_device_manager`, but see `shim/src/globals.c:217-224`: \"THIS GLOBAL\n\
         STILL GRANTS NOTHING ACROSS THE REALM BOUNDARY\" — a shim serves exactly one client on\n\
         exactly one private socket, so both ends of any transfer through it are the same\n\
         application. What exists across realms instead is a core-mediated channel driven by two\n\
         physical human chords (WS-E.2.1, D-024), reachable by no client at any verb set.\n",
    );
    p.push_str("- **No portals.** No file chooser, no screen share, no opening a link.\n");
    p.push_str(
        "- **No session bus.** A realm has no session D-Bus of its own, so anything that expects\n\
         one degrades or fails.\n",
    );
    p.push_str(
        "- **No IME.** Nothing here serves `text-input`/`input-method`, so composing text in any\n\
         non-Latin script does not work.\n",
    );
    p.push_str(
        "- **No XWayland child processes.** There is no X server anywhere in this stack, so an\n\
         application that forks an X11 helper loses that helper.\n\n",
    );
    p.push_str(
        "Where a row was proved by a **named task** instead — an integration gate, a runbook\n\
         checklist, an issue's acceptance criterion — the row names that task, and the task is\n\
         what the row means. Those rows assert something specific and are much stronger than the\n\
         weak bar.\n\n",
    );
    p.push_str("### The three evidence classes on this page\n\n");
    p.push_str(
        "| Class | What it proves | Where it appears |\n\
         |---|---|---|\n\
         | Named task | The stated assertion held in a run of the shipped chain | \
         `Bar` column reads `named task: ...` |\n\
         | Weak bar | The application mapped a window and repainted, and nothing else was \
         checked | `Bar` column reads `weak bar` |\n\
         | Linkage | An ELF/`strings` measurement of a binary on the machine. **Not a run.** | \
         The three inventory tables near the bottom |\n\n",
    );
    p.push_str(
        "Linkage is the weakest class here and it has demonstrated false positives **in both\n\
         directions** on this very machine — see [What this page does not\n\
         measure](#what-this-page-does-not-measure). It is published because the question \"what\n\
         on this machine actually needs X11\" has no better answer that does not require\n\
         launching everything, and it is never mixed into the two execution tables.\n\n",
    );
}

const DESKTOP_TABLE_INTRO: &str = "Software a person would daily drive. Every row is a recorded \
execution against `vitrind`; there are no inferred rows.\n\n";

const TEST_CLIENT_TABLE_INTRO: &str = "Clients this repository wrote. Nobody daily drives them, \
and **the strongest evidence in the tree is about them** — which is exactly why they are in a \
separate table. In particular: no desktop application has ever run on bare-metal DRM/KMS. Both \
bare-metal runs used `solid-client`.\n\n";

fn render_execution_table(
    p: &mut String,
    executions: &[Execution],
    kind: Kind,
    heading: &str,
    intro: &str,
) {
    p.push_str(&format!("## {heading}\n\n"));
    p.push_str(intro);
    p.push_str(
        "| App | Version | Where it ran | Bar | Observable checked | Outcome | Recorded, and where |\n",
    );
    p.push_str("|---|---|---|---|---|---|---|\n");
    for e in executions.iter().filter(|e| e.kind == kind) {
        p.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} — {} |\n",
            e.app,
            e.version.cell(),
            e.venue.cell(),
            e.bar.cell(),
            e.observable,
            e.verdict.cell(),
            e.when.cell(),
            e.evidence,
        ));
    }
    p.push('\n');

    let caveats: Vec<&Execution> = executions
        .iter()
        .filter(|e| e.kind == kind && e.caveat.is_some())
        .collect();
    if !caveats.is_empty() {
        p.push_str("**What these rows over-claim if read quickly:**\n\n");
        for e in caveats {
            p.push_str(&format!(
                "- **{}** — {}\n",
                e.app,
                e.caveat.unwrap_or_default()
            ));
        }
        p.push('\n');
    }
}

fn render_not_x11_gaps(p: &mut String, executions: &[Execution]) {
    p.push_str("## Failures that are not X11 gaps\n\n");
    p.push_str(
        "Issue #221 asks for this section by name, because a reader counting failures would\n\
         otherwise fold every one of them into \"no X11\" and overstate what an X11 shim would\n\
         buy.\n\n",
    );
    let mut any = false;
    for e in executions {
        if let Verdict::DidNotMap(Cause::NotAnX11Gap { cause, owner }) = e.verdict {
            any = true;
            p.push_str(&format!("- **{}** — {cause}. This is {owner}.\n", e.app));
        }
    }
    if !any {
        p.push_str("- No recorded failure currently has a non-X11 cause.\n");
    }
    p.push('\n');
    p.push_str(
        "**Issue #203 is closed and fixed on `main`, and is described here in the past tense.**\n\
         It belongs in this section because it removed applications from the measurable set for a\n\
         reason that had nothing to do with X11: `on_new_deco` answered an `xdg-decoration`\n\
         request eagerly, and `wlr_xdg_toplevel_decoration_v1_set_mode` schedules a configure\n\
         whose `wlr_xdg_surface_schedule_configure` asserts `surface->initialized` — which is\n\
         correctly false at `new_toplevel_decoration` time, because xdg-decoration requires the\n\
         decoration object to be created *before* the surface's first commit. Every\n\
         decoration-aware client therefore aborted the shim at startup. The fix defers the mode\n\
         reply to the initial commit; `shim/src/globals.c:55-77` carries the reasoning, and\n\
         `shim/tests/xdg_conformance_client.c`'s FACT 4 is the regression test. Firefox never\n\
         binds `zxdg_decoration_manager_v1` at all, which is the only reason the entire\n\
         acceptance suite missed it.\n\n",
    );
}

const X11_ONLY_INTRO: &str = "Measured on this machine by ELF linkage, **not** by running \
anything. These are the applications that would need Phase 3 **E3.2** (per-app rootless X server \
with an embedded window manager) before they could run in a realm at all.\n\n\
Two entries are here for completeness and are **not** an argument for E3.2: `picom` and \
`openbox` are an X11 compositor and an X11 window manager, so they are X11-only by definition \
rather than for want of a port, and running either inside a per-app rootless X server is not a \
thing anyone wants.\n\n\
**The interim, stated as what it is.** Until E3.2 lands, the owner keeps a second session for \
the software in this table. So \"I did not have to reboot into Hyprland\" is **false for this \
named set**, and the claim must not be made without that carve-out. This is a workaround the \
owner accepts as a cost, not a mitigation the project offers: nothing in this stack confines \
that second session, it runs another compositor with full access to the same devices, and \
switching to it leaves the confined world entirely. See [Where this is honest about its \
limits](limits.md).\n\n";

const WAYLAND_MODE_INTRO: &str = "Measured on this machine by ELF linkage and `strings`, **not** \
by running anything. These are **not** X11 dependencies: each carries a Wayland path, selected \
by the switch in the third column. Counting any of them toward the X11 gap would overstate it.\n\n";

const INCONCLUSIVE_INTRO: &str = "Linkage contradicted itself and only a run settles these. They \
are published as explicit unknowns rather than guessed into one of the tables above.\n\n\
**Games are recorded here as a measured dependency of this machine and as nothing else.** \
E3.2's exit criteria (`docs/plan/03-phase-3-network-x11-fleet.md` §E3.2) say nothing about \
games; games additionally need relative pointer, pointer constraints, gamepads and GPU features \
far beyond E3.2. This row is a measurement, not a commitment, and no schedule anywhere in this \
repository covers it.\n\n";

fn render_linkage_table(
    p: &mut String,
    measured: &[&Linkage],
    family: Family,
    heading: &str,
    intro: &str,
) {
    p.push_str(&format!("## {heading}\n\n"));
    p.push_str(intro);
    p.push_str("| Binary or app | Version | Wayland mode | What was measured |\n");
    p.push_str("|---|---|---|---|\n");
    for l in measured.iter().filter(|l| l.family == family) {
        p.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            l.app,
            l.version.cell(),
            l.wayland_mode,
            l.measurement
        ));
    }
    p.push('\n');
}

fn render_declined(p: &mut String, declined: &[Declined]) {
    p.push_str("## Requested, and not emitted\n\n");
    p.push_str(
        "These applications are on the list somebody wants covered, and the generator **refused\n\
         to give them a row** because no execution against `vitrind` is recorded for them. They\n\
         are named here rather than dropped silently, so the absence is legible.\n\n",
    );
    if declined.is_empty() {
        p.push_str(
            "- *(empty)* — every requested application has a recorded execution. If this list is\n\
             ever empty for long, suspect that the requested set has been trimmed to match the\n\
             evidence rather than the other way round.\n\n",
        );
        return;
    }
    for d in declined {
        match &d.see_also {
            Some(see) => p.push_str(&format!("- **{}** — {} See {see}.\n", d.app, d.why)),
            None => p.push_str(&format!("- **{}** — {}\n", d.app, d.why)),
        }
    }
    p.push('\n');
}

fn render_gaps(p: &mut String) {
    p.push_str("## What this page does not measure\n\n");
    p.push_str(
        "- **Most of the seed rows are undated.** The weak-bar rows carry no log, no recorder\n\
         dump and no screenshot; the only bound available is the commit that wrote the record\n\
         down, which bounds when the *row was written*, not when the *application was run*. The\n\
         `Recorded, and where` column says `undated` where that is the case rather than reusing\n\
         a commit date as if it were a run date.\n",
    );
    p.push_str(
        "- **The verbatim `xterm` failure line was never captured.** What this repository holds\n\
         is the fragment `Can't open display`. The real format string in `libXt` is `Can't open\n\
         display: %s`, so the emitted line has the shape `<progname>: <error type>: Can't open\n\
         display: <display>` — but the bytes `xterm` actually wrote in that realm are gone, and\n\
         they are not reconstructed here. Capturing them is a runbook step.\n",
    );
    p.push_str(
        "- **No desktop application has ever run on bare metal.** Both DRM/KMS runs used\n\
         `solid-client`. There is a named reason to expect a difference, recorded by the second\n\
         run itself: the shim never emits dmabuf, so the zero-copy scanout path is dead code\n\
         against every real application.\n",
    );
    p.push_str(
        "- **Every inventory row is linkage, not behaviour**, and linkage has demonstrated false\n\
         positives in both directions on this machine. Chromium, Blender, scrcpy, OpenRGB and\n\
         rpi-imager all classify X11-only by `DT_NEEDED` and all five have Wayland paths\n\
         (Chromium statically links its own libwayland; Blender and SDL3 `dlopen` theirs; Qt\n\
         loads its platform plugin by name). Alacritty and kitty classify as *neither*, because\n\
         they `dlopen` everything. Method, for reproducibility: `readelf -d` walked recursively\n\
         with sonames resolved from `ldconfig -p`, plus `strings -a`. Never `ldd` — `ldd` invokes\n\
         the dynamic loader and can execute the binary under inspection.\n",
    );
    p.push_str(
        "- **That walk does not follow `DT_RUNPATH`, so a private library directory ends it.**\n\
         Measured, not theorised: `/usr/bin/inkscape` is a 19-soname launcher stub (7 direct `NEEDED` entries) carrying neither\n\
         family, because its entire GUI lives in `/usr/lib/inkscape/libinkscape_base.so` — a path\n\
         `ldconfig -p` does not know — whose own closure is 147 sonames and carries all three\n\
         Wayland libraries. Any row here whose closure looks implausibly small is this case, and\n\
         the fix is to point the walk at the real library.\n",
    );
    p.push_str(
        "- **The installed set is not the used set.** A bulk scan of `/usr/bin` on 2026-08-10\n\
         classified **494** ELF binaries as X11-only, owned by **114** packages, 37 of them\n\
         `xorg-*` and 58 explicitly installed. (Method, so the number is reproducible: every\n\
         non-symlink ELF file in `/usr/bin`, transitive `DT_NEEDED` closure via `readelf -d` with\n\
         sonames resolved from `ldconfig -p`, counted when the closure contains an X11 soname and\n\
         no `libwayland-*`. The dlopen false-positive above applies to this count too, so it is an\n\
         upper bound on the X11-only set, not an exact one.) Which of those the owner actually\n\
         uses is **not measured here**: no shell history was read, no access times, no launcher\n\
         history. The requirement list handed to E3.2 comes from the owner naming what he needs,\n\
         never from ranking a `/usr/bin`.\n",
    );
    p.push_str(
        "- **`xlsclients` was empty at inventory time, and that means less than it looks.**\n\
         XWayland *is* running under the host compositor and the connection genuinely worked\n\
         (`xprop -root _NET_SUPPORTING_WM_CHECK` returned a window id), yet `xlsclients` listed\n\
         zero X11 clients. That is one instant on one day in a session a few hours old. It is\n\
         not evidence that the owner never runs X11 software — the installed set proves he can —\n\
         and a truthful version needs sampling over days, which nobody has done.\n",
    );
    p.push_str(
        "- **Steam games themselves are entirely unmeasured.** What was measured is\n\
         `steamwebhelper`, the client UI. No game binary was inspected: a Proton title's\n\
         windowing path runs through Wine inside a container, which no static scan here reaches.\n",
    );
    p.push_str(
        "- **Nothing was launched to produce this page.** No `vitrind`, no realm, no nested\n\
         compositor, no application under test. Every machine row is an inference from bytes on\n\
         disk, and every execution row is a transcription of a run somebody else recorded\n\
         earlier. Widening the matrix means executing the runbook below.\n\n",
    );
}

fn render_runbook(p: &mut String) {
    p.push_str("## Runbook: regenerate this page on the target machine\n\n");
    p.push_str(
        "**CI has no DRM device, no seat and no GPU, and structurally cannot run this.** It can\n\
         only assert that the checked-in page matches a regeneration. Everything below is done\n\
         by a human on the target machine.\n\n",
    );
    p.push_str("### 0. Safety, non-negotiable\n\n");
    p.push_str(
        "Run every step of section 2 in a **nested** host window. Never against the DRM/TTY\n\
         backend from inside a live session: that takes DRM master and the seat, and kills the\n\
         session you are sitting in. Bare-metal runs happen from a spare TTY, with the escape\n\
         route rehearsed first — see `docs/drm-bringup.md`.\n\n",
    );
    p.push_str("### 1. Re-take the read-only machine inventory\n\n");
    p.push_str(
        "Nothing here launches anything. Record what these print; they are the header fields and\n\
         the three inventory tables.\n\n",
    );
    p.push_str(
        "```bash\n\
         uname -srvmo                       # kernel -> header\n\
         pacman -Q mesa wlroots0.19         # mesa, wlroots -> header\n\
         git -C \"$REPO\" describe --tags --dirty   # vitrind revision -> header\n\
         pacman -Q <app>                    # one per row, for the Version column\n\
         xlsclients -l                      # X11 clients live in the current session\n\
         \n\
         # Linkage, per candidate binary. NEVER use ldd: it invokes the dynamic\n\
         # loader and can execute the binary under inspection.\n\
         readelf -dW /usr/bin/<app> | grep NEEDED\n\
         strings -a /usr/bin/<app> | grep -iE 'libwayland-client|wl_compositor|ozone-platform'\n\
         ```\n\n",
    );
    p.push_str(
        "A binary that shows neither family in `NEEDED` is not evidence of anything: alacritty,\n\
         kitty, Blender, SDL3 and every Qt application load their backend at run time. Check\n\
         `strings` before concluding, and if the two disagree, the row belongs in the\n\
         inconclusive table, not in a guess.\n\n",
    );
    p.push_str("### 2. Run an application in a realm, and record what you saw\n\n");
    p.push_str(
        "Build first:\n\n```bash\ncargo build --workspace\nmeson compile -C shim/build\n```\n\n",
    );
    p.push_str("Write a one-realm file naming the application by **absolute** path:\n\n");
    p.push_str(
        "```bash\n\
         cat > /tmp/matrix-realm.toml <<'EOF'\n\
         [[realm]]\n\
         id = \"realm-0\"\n\
         command = \"/usr/bin/xterm\"\n\
         args = []\n\
         env_allow = []\n\
         EOF\n\
         ```\n\n",
    );
    p.push_str(
        "`realm.toml` refuses a relative `command`. Then run the core nested, capturing **both**\n\
         streams — the failure you are measuring is usually on the application's stderr, not in\n\
         the core's log:\n\n",
    );
    p.push_str(
        "```bash\n\
         ./target/debug/vitrind --nested \\\n\
         \x20 --realm /tmp/matrix-realm.toml \\\n\
         \x20 --shim \"$PWD/shim/build/vitrin-shim\" \\\n\
         \x20 2>&1 | tee /tmp/matrix-xterm.log\n\
         ```\n\n",
    );
    p.push_str("Record, for the row:\n\n");
    p.push_str(
        "1. **The observable you checked.** Not \"it worked\". Either the specific thing a named\n\
         task asserts, or — if all you did was look at it — the weak bar, `mapped a window and\n\
         repainted`, and nothing more.\n\
         2. **The verbatim failure line**, copied out of the log, if it failed. A fragment is not\n\
         a quote.\n\
         3. **The package version** from step 1, and **the date**.\n\
         4. **The cause**, if it failed: was an X server missing, or was it something else? A\n\
         failure with a non-X11 cause goes in `Cause::NotAnX11Gap` with its owner named, or the\n\
         page will overstate the X11 gap.\n\n",
    );
    p.push_str("### 3. Land the measurement, and regenerate\n\n");
    p.push_str(
        "Edit **`crates/xtask/src/session_matrix.rs`** — never this page. Add the application to\n\
         `REQUESTED` if it is not there, then add an `Execution` (or a `Linkage`) carrying the\n\
         evidence. The generator refuses a cell whose observable is a bare `pass`/`works`/`ok`,\n\
         and refuses an `Execution` for an application that is not in `REQUESTED`.\n\n",
    );
    p.push_str(
        "```bash\n\
         cargo xtask session-matrix           # rewrite this page in place\n\
         git diff -- docs/book/src/session-app-matrix.md   # review what changed\n\
         cargo test -p xtask                  # the generator's own gates\n\
         cargo xtask session-matrix --check   # what CI runs; must print \"no drift\"\n\
         ```\n\n",
    );
    p.push_str(
        "### 4. If you want a row and cannot get evidence for it\n\n\
         Add it to `REQUESTED` with the reason, and leave it declined. It will appear under\n\
         [Requested, and not emitted](#requested-and-not-emitted) with your reason attached,\n\
         which is the honest outcome and the one this page is built to make cheap.\n",
    );
}

// ---------------------------------------------------------------------------
// The corpus. Every entry is transcribed from a record in this repository or
// from a read-only measurement of the target machine, both cited in the row.
// ---------------------------------------------------------------------------

/// Cited over and over, so it is spelled once. Deliberately anchored to the
/// document's section rather than a line number: that file is edited often,
/// and a line number in a *generated* page does not get fixed by regeneration.
const WSE_TABLE: &str =
    "`docs/plan/14-workstream-session-mode.md` §2 \"What already works, measured\"";

const HEADER: Header = Header {
    inventory_date: "2026-08-10",
    last_recorded_run: "2026-08-09 — the second bare-metal DRM run (`docs/drm-bringup.md`)",
    kernel: "7.1.6-arch1-1",
    kernel_caveat:
        "**this is not the kernel the bare-metal evidence was taken on.** Both DRM runs \
                    ran on `7.1.5-arch1-2` (`docs/drm-bringup.md`); the machine has since moved up \
                    one release",
    mesa: "1:26.1.6-1",
    wlroots: "wlroots0.19 0.19.3-1",
    wlroots_note: "the built shim links `libwlroots-0.19.so` (`readelf -dW \
                   shim/build/vitrin-shim`). 0.17 and 0.20 are also installed on this machine and \
                   are not what is used",
    vitrind_revision: "38978f6",
    revision_caveat: "`v0.1.0-56-g38978f6`, the revision of the tree at the last regeneration. \
                      Not a self-reported `vitrind --version`: nothing was launched to produce \
                      this page. Individual runs predate it and name their own where one is \
                      recorded — the nested lock-screen run recorded core `f9f2b8a`, and the \
                      second DRM run followed the three fixes in `cf0e7ff`",
    machine: "one laptop. Intel iGPU on `/dev/dri/card1` (`i915`) drives the only connected \
              output, `eDP-1`, at 2560x1600 @ 240 Hz, scale 1. A second card (`/dev/dri/card2`, \
              `nvidia`) has every connector disconnected and is not in the display path",
    host_compositor: "Hyprland 0.56.2-1, `XDG_SESSION_TYPE=wayland`",
};

/// The applications somebody wants this page to cover. Membership here is a
/// *question*; only [`EXECUTIONS`] answers it.
const REQUESTED: &[Requested] = &[
    Requested {
        app: "Firefox ESR",
        why: "named in issue #221's seed list, and the M1.2 acceptance application.",
        see_also: None,
    },
    Requested {
        app: "alacritty",
        why: "named in issue #221's seed list; the terminal that found #203.",
        see_also: None,
    },
    Requested {
        app: "kitty",
        why: "named in issue #221's seed list.",
        see_also: None,
    },
    Requested {
        app: "Chromium",
        why: "named in issue #221's seed list.",
        see_also: None,
    },
    Requested {
        app: "Visual Studio Code",
        why: "named in issue #221's seed list; the Electron representative.",
        see_also: None,
    },
    Requested {
        app: "nautilus",
        why: "named in issue #221's seed list; the GTK4 representative.",
        see_also: None,
    },
    Requested {
        app: "gimp",
        why: "named in issue #221's seed list.",
        see_also: None,
    },
    Requested {
        app: "inkscape",
        why: "named in issue #221's seed list.",
        see_also: None,
    },
    Requested {
        app: "pavucontrol",
        why: "named in the recorded GTK3 row but silently dropped from issue #221's seed list, \
               so it is carried here rather than lost.",
        see_also: None,
    },
    Requested {
        app: "xterm",
        why: "named in issue #221's seed list; the X11 failure the whole issue is about.",
        see_also: None,
    },
    Requested {
        app: "waybar",
        why: "issue #221 requires the bar/launcher failure be classified so it is never counted \
               as an X11 gap.",
        see_also: None,
    },
    Requested {
        app: "rofi",
        why: "same row as waybar; same classification requirement.",
        see_also: None,
    },
    Requested {
        app: "wofi",
        why: "named in the recorded failing row, but not installed on the measured machine \
               (`pacman -Q wofi` reports it absent), so no package version can be recorded and \
               the cell cannot be regenerated here even by a live runbook.",
        see_also: Some("the waybar and rofi rows for the same failure"),
    },
    Requested {
        app: "Discord",
        why: "the recorded Electron row reads \"VS Code — so also Discord, Slack, Obsidian\". \
               That parenthetical is an inference from a shared runtime, not a measurement: no \
               record anywhere shows it executed against `vitrind`.",
        see_also: Some("the Electron runtime row in the Wayland-mode table"),
    },
    Requested {
        app: "Slack",
        why: "same inferred parenthetical as Discord; never executed against `vitrind`.",
        see_also: Some("the Electron runtime row in the Wayland-mode table"),
    },
    Requested {
        app: "Obsidian",
        why: "same inferred parenthetical as Discord; never executed against `vitrind`.",
        see_also: Some("the Electron runtime row in the Wayland-mode table"),
    },
    Requested {
        app: "Steam (steamwebhelper)",
        why: "the owner named Steam and games as a real dependency of this machine. Nothing in \
               the Steam stack has ever been executed against `vitrind`, and no schedule in this \
               repository covers games.",
        see_also: Some(
            "its row in the inconclusive table: the client's windowing path came out \
             **unknown**, which is a measurement and not a commitment",
        ),
    },
    Requested {
        app: "google-chrome",
        why: "the packaged Google build, distinct from Arch's chromium. Never executed against \
               `vitrind`, and its linkage contradicts itself.",
        see_also: Some("the inconclusive table"),
    },
    Requested {
        app: "weston-terminal",
        why: "the bottom rung of the M1.2 real-app ladder.",
        see_also: None,
    },
    Requested {
        app: "gtk-entry-probe",
        why: "the middle rung of the M1.2 real-app ladder.",
        see_also: None,
    },
    Requested {
        app: "clipboard-peer",
        why: "the cross-realm clipboard property gate's client.",
        see_also: None,
    },
    Requested {
        app: "solid-client",
        why: "the only client ever run on bare-metal DRM/KMS.",
        see_also: None,
    },
    Requested {
        app: "input-echo-client",
        why: "the nested lock-screen runbook's client.",
        see_also: None,
    },
];

const EXECUTIONS: &[Execution] = &[
    // ---- Desktop applications -------------------------------------------
    Execution {
        app: "Firefox ESR",
        kind: Kind::DesktopApp,
        version: Version::PinnedByRepo("140.12.0esr, sha256 3323ee13…f433d92"),
        venue: Venue::Headless,
        bar: Bar::NamedTask("`tests/integration/test_real_firefox.py`"),
        observable: "real `vitrind` execs the real `vitrin-shim`, which execs this pinned Firefox \
                     rendering a local `file://` page; the real Python SDK captures a frame \
                     through the real enforcement/capture path and asserts its dominant colour is \
                     the served `#0000ff`, and that the globals ledger contains nothing outside \
                     `shim/docs/firefox-refused-globals.txt`. No mock on any seam",
        verdict: Verdict::Met,
        when: When::EveryPullRequestSince {
            commit: "1ebeee2",
            date: "2026-07-22",
        },
        evidence: "`tests/integration/test_real_firefox.py`, `shim/docs/firefox.md` §7 (the M1.2 \
                   milestone proof)",
        caveat: Some(
            "this is the pinned Mozilla ESR tarball, not a distro package — Arch ships no \
             `firefox-esr`, and what is installed on the measured machine is \
             `firefox-developer-edition`, which was never run against `vitrind`. The row is about \
             140.12.0esr and nothing else. The commit that first brought Firefox up in a realm, \
             `cae70f0` (2026-07-20), wired only `firefox_bringup.sh` into CI — under the mock \
             core and declared SKIPPED with `VITRIN_SKIP_FIREFOX_GATE=1` — so it is not this \
             row's provenance.",
        ),
    },
    Execution {
        app: "alacritty",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("0.17.0-1"),
        venue: Venue::NestedUnderHyprland,
        bar: Bar::NamedTask("issue #203 acceptance criterion 1"),
        observable: "\"a real toolkit terminal (alacritty) runs to completion under `vitrind \
                     --nested`\" — i.e. a live nested run to completion, after the eager-`set_mode` \
                     abort that killed it beforehand",
        verdict: Verdict::Met,
        when: When::Dated("2026-08-06 (the fix landed in `af98130` at 16:38:43 +0900)"),
        evidence: "issue #203, checked acceptance criterion; commit `af98130`",
        caveat: Some(
            "#203 records only `vitrind --nested`; it does not name the host compositor, so the \
             venue here is this machine's host and not something the record states.",
        ),
    },
    Execution {
        app: "kitty",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("0.48.2-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: Some(
            "kitty was proved to **crash** before #203's fix — the abort was reproduced with \
             alacritty and with kitty. #203's acceptance criterion names alacritty alone as \
             re-run to completion afterwards, and no record anywhere shows kitty re-run post-fix. \
             So this row is the weak bar and not #203's named task.",
        ),
    },
    Execution {
        app: "Chromium",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("151.0.7922.108-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: None,
    },
    Execution {
        app: "Visual Studio Code",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("visual-studio-code-bin 1.131.0-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: Some(
            "the recorded row reads \"Electron (VS Code — so also Discord, Slack, Obsidian)\". \
             Only VS Code was run; the other three are an inference from a shared runtime and are \
             declined below rather than given rows.",
        ),
    },
    Execution {
        app: "nautilus",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("50.2.2-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: None,
    },
    Execution {
        app: "gimp",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("3.2.4-2"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: None,
    },
    Execution {
        app: "inkscape",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("1.4.4-4"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: None,
    },
    Execution {
        app: "pavucontrol",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("1:6.2-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "mapped a window and repainted. Nothing was typed into it, clicked, or \
                     checked for correct rendering",
        verdict: Verdict::Met,
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: None,
    },
    Execution {
        app: "xterm",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("410-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "started in a realm and never mapped: it reported it could not open a \
                     display. There is no XWayland anywhere in this stack — not in the core, not \
                     in the shim's advertised global set, not as a process the shim ever execs",
        verdict: Verdict::DidNotMap(Cause::NoXServer),
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: Some(
            "the repository holds only the fragment `Can't open display`, never the as-emitted \
             line. `libXt`'s format string is `Can't open display: %s`, so the real line carries a \
             colon and the display name; those bytes were not captured and are not reconstructed \
             here.",
        ),
    },
    Execution {
        app: "waybar",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("0.15.0-2"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "connected, bound six globals, and never mapped. The shim advertises no \
                     `zwlr_layer_shell_v1`, which is the interface a bar maps through",
        verdict: Verdict::DidNotMap(Cause::NotAnX11Gap {
            cause: "there is no `zwlr_layer_shell_v1`, so it has nothing to map into",
            owner: "WS-E Stage 2, which owns layer-shell",
        }),
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: "the same row, plus D-021's cost list in `docs/plan/20-decision-log.md`, which \
                   attaches the measurement: \"measured: waybar connects, binds six globals and \
                   never maps\"",
        caveat: None,
    },
    Execution {
        app: "rofi",
        kind: Kind::DesktopApp,
        version: Version::InstalledAtInventory("2.0.0-1"),
        venue: Venue::Headless,
        bar: Bar::MappedAndRepainted,
        observable: "started in a realm and never mapped, in the same recorded row as waybar",
        verdict: Verdict::DidNotMap(Cause::NotAnX11Gap {
            cause: "there is no `zwlr_layer_shell_v1`, so it has nothing to map into",
            owner: "WS-E Stage 2, which owns layer-shell",
        }),
        when: When::UndatedBoundedBy {
            commit: "7863702",
            date: "2026-08-06",
        },
        evidence: WSE_TABLE,
        caveat: Some(
            "the six-globals count is recorded for waybar specifically, in D-021; the recorded \
             row groups rofi with it but attaches no separate measurement. rofi also keeps an X11 \
             path, so it appears in the Wayland-mode inventory table too.",
        ),
    },
    // ---- Repository test clients ----------------------------------------
    Execution {
        app: "weston-terminal",
        kind: Kind::RepoTestClient,
        version: Version::NotTheBuildThatRan("weston 15.0.1-3"),
        venue: Venue::Headless,
        bar: Bar::NamedTask("`tests/integration/test_real_app.py`"),
        observable: "real `vitrind` execs the real C shim which fork/execs `weston-terminal`; the \
                     application's own frames flow shim → core and are byte-captured, and its \
                     identity is read from procfs. The #105 / M1.2 bottom-rung gate, no mock on \
                     any seam",
        verdict: Verdict::Met,
        when: When::EveryPullRequestSince {
            commit: "e742f25",
            date: "2026-07-22",
        },
        evidence: "`tests/integration/test_real_app.py` (`APP_NAME = \"weston-terminal\"`)",
        caveat: Some(
            "a real third-party Wayland client with a mock-free gate — stronger evidence than any \
             weak-bar row above — but nobody daily drives it, so it says nothing about a desktop. \
             The cited gate runs on `ubuntu-latest` against apt's `weston` \
             (`shim/ci/install-deps.sh`), whose version this page does not record.",
        ),
    },
    Execution {
        app: "gtk-entry-probe",
        kind: Kind::RepoTestClient,
        version: Version::BuiltFromRepo("`shim/tests/gtk_entry_probe.c` at this revision"),
        venue: Venue::Headless,
        bar: Bar::NamedTask("`tests/integration/test_real_gtk.py`"),
        observable: "a real GTK3 `GtkEntry` client renders a grey toplevel with a white text \
                     field headlessly, and its committed frame carries real non-uniform content \
                     through the real chain. The gate asserts render, explicitly not input",
        verdict: Verdict::Met,
        when: When::EveryPullRequestSince {
            commit: "1ebeee2",
            date: "2026-07-22",
        },
        evidence: "`tests/integration/test_real_gtk.py`, `shim/tests/gtk_entry_probe.c`",
        caveat: Some(
            "this is a GTK3 **fixture**, not nautilus, gimp or inkscape, and it must not be cited \
             as evidence for those three. No GTK4 or Qt6 fixture exists.",
        ),
    },
    Execution {
        app: "clipboard-peer",
        kind: Kind::RepoTestClient,
        version: Version::BuiltFromRepo("`shim/tests` at this revision"),
        venue: Venue::Headless,
        bar: Bar::NamedTask("`tests/integration/test_real_clipboard.py`"),
        observable: "two realms, two real `clipboard-peer`s under two real C shims: the offer \
                     chord against an empty slot sends nothing, the promote chord still reaches \
                     no other realm, and only the second chord in the realm the output moved to \
                     produces `offer_selection` — after which the receiving application has the \
                     string byte for byte through a real `wl_data_device` transfer",
        verdict: Verdict::Met,
        when: When::EveryPullRequestSince {
            commit: "2f7c7cf",
            date: "2026-08-08",
        },
        evidence: "`tests/integration/test_real_clipboard.py`, described in \
                   `tests/integration/README.md` (WS-E.2.1, issue #213)",
        caveat: Some(
            "the repository's own precedent for how to record a substitution honestly: #213 names \
             alacritty and Firefox, neither of which can be made in CI to put a *known* string on \
             a clipboard without a human's mouse, so a toolkit-free client stands in and the \
             README says so. Not asserted: that a real chord on real hardware produces any of it.",
        ),
    },
    Execution {
        app: "solid-client",
        kind: Kind::RepoTestClient,
        version: Version::BuiltFromRepo("`shim/tests` at this revision, debug build"),
        venue: Venue::BareMetalDrm,
        bar: Bar::NamedTask("`docs/drm-bringup.md` observation checklist, first run"),
        observable: "first execution of the DRM/KMS backend by anyone: 2560x1600 @ 240 Hz set on \
                     `card1`; the client's green square drew; the trusted band drew MIRRORED \
                     along the bottom edge; VT switch away and back was **impossible**, so the \
                     human could not leave; `vitrind` at 99.1% CPU over a 471 s run",
        verdict: Verdict::Met,
        when: When::Dated("2026-08-09, on kernel 7.1.5-arch1-2"),
        evidence: "`docs/drm-bringup.md`, first-run record",
        caveat: Some(
            "`solid-client` commits `wl_shm` buffers and is not a desktop application. No desktop \
             application has ever run on bare metal, and the second run's own open list notes the \
             shim never emits dmabuf, so the zero-copy scanout path is dead code against every \
             real application.",
        ),
    },
    Execution {
        app: "solid-client",
        kind: Kind::RepoTestClient,
        version: Version::BuiltFromRepo("`shim/tests` at this revision, release build"),
        venue: Venue::BareMetalDrm,
        bar: Bar::NamedTask("`docs/drm-bringup.md` observation checklist, second run"),
        observable: "after the first run's three fixes: trusted band at the TOP (the mirror is \
                     fixed); consent card drawn on the panel and clicked with a real mouse, \
                     petition granted, 13 captures served; held-Esc revocation against a LIVE \
                     grant (`dead_man_triggered held_ms=1005 revoked=1`); 5 chorded VT switches \
                     honoured; 32.9% CPU release against 99.1% debug; centre pixel read \
                     `#55aa00ff` for a realm configured `00aa55`, which is exactly xrgb8888 \
                     little-endian",
        verdict: Verdict::Met,
        when: When::Dated("2026-08-09, after the three fixes in `cf0e7ff`"),
        evidence: "`docs/drm-bringup.md`, second-run record",
        caveat: Some(
            "still `solid-client`. Three items stayed open after this run: `vitrind`'s own log \
             line renders the connector name empty, the shim never emits dmabuf, and \
             `refresh_view_cache` composes for absent consumers.",
        ),
    },
    Execution {
        app: "input-echo-client",
        kind: Kind::RepoTestClient,
        version: Version::BuiltFromRepo("`shim/tests` at this revision"),
        venue: Venue::NestedUnderHyprland,
        bar: Bar::NamedTask("`shim/docs/nested-lock-screen.md`, eight-step by-eye checklist"),
        observable: "PASS on steps 1–7 with the client resolving keys through xkbcommon as a real \
                     toolkit does (`a`/`b`/`c` → `keysym=0x61/0x62/0x63`); a held `Shift` did not \
                     postpone the idle lock, and its release while locked was handled",
        verdict: Verdict::Met,
        when: When::Dated("2026-08-09, core `f9f2b8a`"),
        evidence: "`shim/docs/nested-lock-screen.md`, executed record",
        caveat: Some(
            "the run found a shipped defect — `TextureKey::current` enumerated every input to \
             `compose_human_visible` except the lock, which is a locked session that looked \
             unlocked. Two further warnings: the page states step 7 is weaker than it reads \
             (`input-echo-client` is static, so every frame carried the identical digest), and \
             the page contradicts itself by also carrying an empty \"not yet executed\" record \
             block further down. It *was* executed.",
        ),
    },
];

const LINKAGE: &[Linkage] = &[
    // ---- X11-only -------------------------------------------------------
    Linkage {
        app: "xterm",
        version: Version::InstalledAtInventory("410-1"),
        family: Family::X11Only,
        wayland_mode: "none exists — there is no Wayland port of xterm",
        measurement: "`readelf -dW /usr/bin/xterm` → `libXaw.so.7`, `libXt.so.6`, `libX11.so.6`, \
                      `libXmu.so.6`, `libXext.so.6`, `libICE.so.6` and no Wayland library. \
                      `strings -a` matching any of `libwayland-client`, `wl_compositor` or \
                      `wayland` → 0 hits",
    },
    Linkage {
        app: "feh",
        version: Version::InstalledAtInventory("3.12.2-1"),
        family: Family::X11Only,
        wayland_mode: "none — raw Xlib plus imlib2, so there is no toolkit backend to switch",
        measurement: "`readelf -dW /usr/bin/feh` → `libX11.so.6`, `libXinerama.so.1`, \
                      `libImlib2.so.1`; no Wayland library in the transitive closure. `strings -a` \
                      wayland matches → 0",
    },
    Linkage {
        app: "nvidia-settings",
        version: Version::InstalledAtInventory("610.57.04-1"),
        family: Family::X11Only,
        wayland_mode: "none — `libXxf86vm` is an X-server-only extension with no Wayland \
                       equivalent",
        measurement: "`readelf -dW` → exactly `libXxf86vm.so.1`, `libjansson.so.4`, \
                      `libX11.so.6`, `libXext.so.6`, `libm.so.6`, `libc.so.6`. It **does** carry \
                      nine case-insensitive `wayland` strings, and they are a trap: every one is \
                      an NVIDIA probe symbol — `wconn_get_wayland_display`, \
                      `wconn_get_wayland_output_info`, `libnvidia-wayland-client.so.610.57.04`, \
                      `Wayland Connector Library failed to connect.` — for querying outputs under \
                      a Wayland session, not a GUI backend. A grep-only method misclassifies this \
                      one",
    },
    Linkage {
        app: "dmenu",
        version: Version::InstalledAtInventory("5.4-1"),
        family: Family::X11Only,
        wayland_mode: "none — suckless raw-Xlib launcher",
        measurement: "`readelf -dW /usr/bin/dmenu` → `libX11.so.6`, `libXinerama.so.1`, \
                      `libXft.so.2`, `libfontconfig.so.1`, `libc.so.6` — no Wayland library, and \
                      `strings -a` finds no Wayland name to dlopen either. Note it would be \
                      unusable for **two** independent reasons: it is X11-only *and* it is a \
                      launcher, which is the layer-shell gap above",
    },
    Linkage {
        app: "slock",
        version: Version::InstalledAtInventory("1.7-1"),
        family: Family::X11Only,
        wayland_mode: "none, and none could exist — it locks by grabbing the X server",
        measurement: "`readelf -dW /usr/bin/slock` → `libX11.so.6`, `libXrandr.so.2`, \
                      `libcrypt.so.2`, `libc.so.6`; wayland strings → 0",
    },
    Linkage {
        app: "polybar",
        version: Version::InstalledAtInventory("3.7.2-2"),
        family: Family::X11Only,
        wayland_mode: "none — EWMH and ICCCM are X11 window-manager protocols with no Wayland \
                       analogue",
        measurement: "`readelf -dW /usr/bin/polybar` → `libxcb-ewmh.so.2`, `libxcb-icccm.so.4`, \
                      `libxcb-randr.so.0`, `libxcb-xkb.so.1`, `libxcb-cursor.so.0` and the rest of \
                      the xcb set; wayland strings → 0",
    },
    Linkage {
        app: "lemonbar",
        version: Version::InstalledAtInventory("lemonbar-git v1.5.r2.g59b0d28-1"),
        family: Family::X11Only,
        wayland_mode: "none — raw xcb bar",
        measurement: "`readelf -dW /usr/bin/lemonbar` → `libxcb.so.1`, `libxcb-randr.so.0`; \
                      wayland strings → 0",
    },
    Linkage {
        app: "picom",
        version: Version::InstalledAtInventory("picom-git 2855_12.197.g6d676824_2026.06.02-1"),
        family: Family::X11Only,
        wayland_mode: "none, by definition — it **is** an X11 compositing manager",
        measurement: "`readelf -dW /usr/bin/picom` → `libxcb-composite.so.0`, \
                      `libxcb-damage.so.0`, `libxcb-glx.so.0`, `libxcb-present.so.0`, \
                      `libxcb-xfixes.so.0`, `libX11-xcb.so.1`; wayland strings → 0. **Not an E3.2 \
                      requirement**: E3.2 gives each X application its own rootless X server, \
                      which is not a thing to run a compositor inside",
    },
    Linkage {
        app: "openbox",
        version: Version::InstalledAtInventory("3.6.1-14"),
        family: Family::X11Only,
        wayland_mode: "none, by definition — it **is** an X11 window manager",
        measurement: "`readelf -dW /usr/bin/openbox` → `libX11.so.6`, `libXcursor.so.1`, \
                      `libXinerama.so.1`, `libXrandr.so.2`, `libXext.so.6`, `libSM.so.6`, \
                      `libICE.so.6`; the 64-soname transitive closure contains no `libwayland-*` \
                      and wayland strings → 0. **Not an E3.2 requirement**, for the same reason as \
                      picom",
    },
    Linkage {
        app: "xsel",
        version: Version::InstalledAtInventory("1.2.1-2"),
        family: Family::X11Only,
        wayland_mode: "none — an Xlib selection client",
        measurement: "`readelf -dW /usr/bin/xsel` → `libX11.so.6`, `libc.so.6`, and nothing \
                      else; the 6-soname closure contains no `libwayland-*`. Directly relevant to \
                      this workstream: the cross-realm clipboard (D-024) is the project's answer \
                      to this class and `xsel` cannot participate in it. `xdotool` and `xclip` \
                      are **not installed** on this machine",
    },
    Linkage {
        app: "steamwebhelper (the Steam client UI)",
        version: Version::VendorBundled(
            "a self-updating CEF build under `~/.local/share/Steam/ubuntu12_64`, dated 2026-07-22; \
             the Arch `steam 1.0.0.87-1` package ships only a shell wrapper and does not describe \
             what runs",
        ),
        family: Family::Inconclusive,
        wayland_mode: "**unknown** — the executable carries no Wayland path; the library that \
                       draws does. Only a run settles it",
        measurement: "the executable's own 36-soname `readelf -dW` closure carries `libX11.so.6`, \
                      `libXi.so.6`, `libXrandr.so.2`, `libXcomposite.so.1` and **no** \
                      `libwayland-*`, and `strings -a` matches \
                      `ozone-platform`/`wl_compositor`/`libwayland-client` **0** times. That reads \
                      as settled and is not. Its direct `NEEDED` entries include `libcef.so` \
                      (219 444 168 bytes, 2026-07-09) and `libSDL3.so.0`, neither on \
                      `ldconfig`'s path, so the walk stops exactly where the answer is: \
                      `strings -a` on `libcef.so` matches — `ozone-platform`, \
                      `ozone-platform-hint`, `wl_compositor`, `enable-wayland-ime`, \
                      `Failed to initialize Wayland platform` — and SDL3 dlopens \
                      `libwayland-client.so.0` (see its row). This is `google-chrome`'s case one \
                      library deeper. Steam has been run on this machine — \
                      `~/.local/share/Steam/steamapps` holds 7 appmanifests, of which exactly one \
                      is a game and the rest are Proton and the Steam Linux Runtimes. **No game \
                      binary was inspected and no game was run**",
    },
    Linkage {
        app: "OpenJDK desktop AWT/Swing (the system JVM)",
        version: Version::InstalledAtInventory("jdk-openjdk 26.0.2.u10-1"),
        family: Family::X11Only,
        wayland_mode: "none — the system JVM ships no Wayland AWT backend",
        measurement: "`find /usr/lib/jvm -name 'libawt*.so'` → exactly `libawt.so`, \
                      `libawt_xawt.so`, `libawt_headless.so`, and nothing else. \
                      `libawt_xawt.so`'s closure is `libX11.so.6`, `libXext.so.6`, `libXi.so.6`, \
                      `libXrender.so.1`, `libXtst.so.6`, `libxcb.so.1` with no Wayland library. A \
                      capability statement about the runtime; no Swing application was launched",
    },
    // ---- Has a Wayland mode ---------------------------------------------
    Linkage {
        app: "Chromium",
        version: Version::InstalledAtInventory("151.0.7922.108-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "`--ozone-platform=wayland`, or `--ozone-platform-hint=auto`",
        measurement: "linkage alone would have called this X11-only and been **wrong**. The \
                      89-soname `DT_NEEDED` closure carries `libX11.so.6` and **no `libwayland-*` \
                      at all**, because Chromium statically links its own libwayland from \
                      `third_party`. What settles it is `strings -a /usr/lib/chromium/chromium`, \
                      which yields the flag `ozone-platform` and the Wayland wire-protocol \
                      interface names `wl_compositor` and `xdg_wm_base`. Corroborated by its \
                      weak-bar execution row above",
    },
    Linkage {
        app: "Visual Studio Code",
        version: Version::InstalledAtInventory("visual-studio-code-bin 1.131.0-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "`--ozone-platform-hint=auto`, already configured by the owner",
        measurement: "the strongest non-execution evidence on this page, because the owner \
                      has already configured it: `~/.config/code-flags.conf` exists and contains, \
                      verbatim, `--ozone-platform-hint=auto` and `--enable-wayland-ime` under the \
                      comment \"Native Wayland + text-input-v3 so fcitx5 works without \
                      GTK_IM_MODULE\". Linkage is weaker and must be read carefully: \
                      `/usr/share/code/code` links **no** `libwayland-*` directly; its 95-soname \
                      closure reaches all three only through `libgtk-3.so.0`, which is GTK's \
                      Wayland and not Electron's. The binary's own 5 ozone/Wayland strings are what \
                      say the ozone machinery is compiled in",
    },
    Linkage {
        app: "Electron runtime (and the Electron applications on it)",
        version: Version::InstalledAtInventory("electron43 43.3.0-1, plus ten other runtimes"),
        family: Family::HasWaylandMode,
        wayland_mode: "`--ozone-platform=wayland` / `--ozone-platform-hint=auto`",
        measurement: "`/usr/lib/electron43/electron` links **no** `libwayland-*` directly — \
                      its 103-soname closure reaches `libwayland-client.so.0`, \
                      `libwayland-cursor.so.0` and `libwayland-egl.so.1` only through \
                      `libgtk-3.so.0`. What says the ozone machinery is present is `strings -a`, \
                      which matches `ozone-platform`/`wl_compositor`/`libwayland-client` 5 times. \
                      `/usr/lib/slack/slack` measures identically (95-soname closure, no direct \
                      Wayland linkage). `discord 1:1.0.152-1`, `obsidian 1.13.4-2` and \
                      `element-desktop 1.12.23-1` are installed but their real binaries could not \
                      be resolved from their launchers without executing them, so they are \
                      asserted by runtime family and **not measured individually** — which is \
                      exactly why they are declined rather than given rows",
    },
    Linkage {
        app: "alacritty",
        version: Version::InstalledAtInventory("0.17.0-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "native — prefers Wayland when `WAYLAND_DISPLAY` is set, falls back to X11",
        measurement: "the `DT_NEEDED` closure carries **neither** `libX11` nor \
                      `libwayland-client`: everything is dlopened. `strings -a /usr/bin/alacritty` \
                      yields both `libwayland-client.so.0` and `libX11.so.6`. Corroborated by the \
                      #203 run, which happened under `vitrind --nested`, a stack serving no X11 \
                      at all",
    },
    Linkage {
        app: "kitty",
        version: Version::InstalledAtInventory("0.48.2-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "native, via dlopen",
        measurement: "linkage says nothing either way — only 5 sonames in the closure and neither \
                      family among them, and `/usr/bin/kitty` is a small launcher so `strings` \
                      finds neither name. What places it here is that this repository ran it \
                      under `vitrind`, which serves no X11",
    },
    Linkage {
        app: "nautilus, gimp, inkscape (GTK3/GTK4)",
        version: Version::InstalledAtInventory("50.2.2-1, 3.2.4-2, 1.4.4-4"),
        family: Family::HasWaylandMode,
        wayland_mode: "`GDK_BACKEND=wayland` — GTK selects Wayland automatically when \
                       `WAYLAND_DISPLAY` is set",
        measurement: "each carries both families, and each by a different route, which is the \
                      point. `/usr/bin/nautilus` links `libwayland-client.so.0` **directly** \
                      (145-soname closure, which adds cursor and egl). `/usr/bin/gimp-3.2` links \
                      none directly; its 115-soname closure reaches all three through GTK. \
                      `/usr/bin/inkscape` is a 19-soname launcher stub (7 direct `NEEDED` entries) whose GUI lives in \
                      `/usr/lib/inkscape/libinkscape_base.so`, a 147-soname closure carrying all \
                      three. GTK carries both backends in one library and picks at run time — \
                      which is exactly why `shim/docs/firefox.md` sets `GDK_BACKEND=wayland` \
                      explicitly, so a stray `DISPLAY` cannot silently drop the browser onto X11",
    },
    Linkage {
        app: "blender",
        version: Version::InstalledAtInventory("17:5.2.0-4"),
        family: Family::HasWaylandMode,
        wayland_mode: "native (`GHOST_Wayland`), via dlopen",
        measurement: "a second case where linkage alone is **wrong**: the 266-soname closure \
                      carries `libX11.so.6` and `libX11-xcb.so.1` and **no `libwayland-*` at \
                      all**, yet `strings -a /usr/bin/blender` matches `libwayland-client`, \
                      `wl_compositor` or `GHOST_Wayland` 3 times — Blender dlopens its Wayland \
                      backend",
    },
    Linkage {
        app: "scrcpy",
        version: Version::InstalledAtInventory("4.1-2, on sdl3 3.4.14-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "native via SDL3, which dlopens `libwayland-client.so.0` and `libdecor-0.so.0`",
        measurement: "the clearest false positive in the scan. scrcpy's direct `NEEDED` is \
                      `libavformat`, `libavcodec`, `libavutil`, `libswresample`, `libSDL3.so.0`, \
                      `libavdevice`, `libusb-1.0` — so the X11 in its 163-soname closure arrives \
                      through `libavdevice`, which is ffmpeg's x11grab **capture input**, not its \
                      GUI. Its GUI is SDL3, and `libSDL3.so.0` has a 3-soname closure with no hard \
                      X11 and no hard Wayland while its strings carry `libwayland-client.so.0`, \
                      `libdecor-0.so.0` and `wl_compositor`",
    },
    Linkage {
        app: "openrgb",
        version: Version::InstalledAtInventory("1.0rc3-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "`QT_QPA_PLATFORM=wayland`",
        measurement: "`readelf -dW /usr/bin/openrgb` → `libQt5Widgets.so.5`, `libQt5Gui.so.5` and \
                      nothing else relevant: Qt loads its platform plugin **by name** at run time, \
                      which is why the binary shows zero wayland strings. The plugin is installed \
                      (`qt5-wayland 5.15.19+kde+r55-1`; `libqwayland-egl.so` and \
                      `libqwayland-generic.so` are present)",
    },
    Linkage {
        app: "rpi-imager",
        version: Version::InstalledAtInventory("2.0.9-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "`QT_QPA_PLATFORM=wayland`",
        measurement: "Qt6 Quick application (`libQt6Quick.so.6`, `libQt6Gui.so.6`); \
                      `qt6-wayland 6.11.1-1` is installed and `libqwayland.so` sits beside \
                      `libqxcb.so` in the Qt6 platform plugin directory",
    },
    Linkage {
        app: "fcitx5-config-qt",
        version: Version::InstalledAtInventory("fcitx5-configtool 5.1.14-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "`QT_QPA_PLATFORM=wayland`",
        measurement: "Qt6 (`libQt6Widgets.so.6`, `libQt6Gui.so.6`) with `qt6-wayland` installed \
                      and `libqwayland.so` present",
    },
    Linkage {
        app: "Android Studio (JetBrains Runtime)",
        version: Version::InstalledAtInventory("android-studio 2026.1.3.7-1, JBR 25.0.2"),
        family: Family::HasWaylandMode,
        wayland_mode: "a native Wayland AWT backend in the bundled runtime",
        measurement: "`find /opt/android-studio -name 'libawt_*.so'` → `libawt_xawt.so`, \
                      `libawt_headless.so` **and `libawt_wlawt.so`**, against a system JDK that \
                      ships no such file. A genuine split worth recording, because it contradicts \
                      the system-JDK row above. Not launched, and it was not verified that the \
                      backend is enabled by default",
    },
    Linkage {
        app: "gamescope",
        version: Version::InstalledAtInventory("3.16.25-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "native — it is itself a Wayland compositor, and a Wayland client when nested",
        measurement: "links both families directly: a 56-soname closure with \
                      `libwayland-client.so.0` and `libwayland-server.so.0` beside \
                      `libX11.so.6`, `libICE.so.6` and `libSM.so.6`. Named \
                      here for two reasons: it is the one piece of the Steam stack on this machine \
                      that speaks Wayland natively, and it is the prior art \
                      `docs/plan/03-phase-3-network-x11-fleet.md` cites for E3.2's \
                      per-app-Xwayland design",
    },
    Linkage {
        app: "waybar, rofi",
        version: Version::InstalledAtInventory("0.15.0-2, 2.0.0-1"),
        family: Family::HasWaylandMode,
        wayland_mode: "native — both link `libwayland-client` directly",
        measurement: "waybar links `libwayland-client.so.0` **directly**; its 108-soname \
                      closure adds cursor and egl, and its X11 entries are GTK's. rofi links \
                      `libwayland-client.so.0` and `libwayland-cursor.so.0` directly while keeping \
                      its whole xcb path (`libxcb-ewmh`, `libxcb-icccm`, `libxcb-randr`, \
                      `libxcb-cursor`) in a 60-soname closure, so it classifies as **both**. \
                      Listed here so their failure above is never misfiled as an X11 gap — it is \
                      layer-shell. `wofi` is **not installed**, so its cell cannot be regenerated \
                      on this machine at all",
    },
    // ---- Inconclusive ---------------------------------------------------
    Linkage {
        app: "google-chrome",
        version: Version::InstalledAtInventory("151.0.7922.71-1"),
        family: Family::Inconclusive,
        wayland_mode: "**unknown** — presumably `--ozone-platform=wayland`, unverified",
        measurement: "the contradiction **is** the finding. The 71-soname closure of \
                      `/opt/google/chrome/chrome` carries `libX11.so.6`, `libXi.so.6`, \
                      `libXrandr.so.2` and **no `libwayland-*` at all** — yet the same binary \
                      matches `ozone-platform`/`wl_compositor`/`libwayland-client` 6 times, and \
                      Arch's chromium measures **identically** (89 sonames, zero Wayland linkage) \
                      while demonstrably having a working `--ozone-platform=wayland`. Linkage \
                      cannot settle this one. Only a run can, and nothing was launched",
    },
];

/// The corpus this page is generated from.
pub fn corpus() -> Corpus {
    Corpus {
        header: HEADER,
        requested: REQUESTED,
        executions: EXECUTIONS,
        linkage: LINKAGE,
    }
}

// ---------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------

/// `cargo xtask session-matrix [--check]`.
pub fn session_matrix(root: &Path, check: bool) -> Result<()> {
    let rendered = render(&corpus())?;
    let path = root.join(PAGE_PATH);

    if check {
        let on_disk = fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "session-matrix --check: reading {}: {e}. Run `cargo xtask session-matrix` and \
                 commit the result.",
                path.display()
            )
        })?;
        if on_disk == rendered.page {
            eprintln!(
                "xtask: session-matrix --check: no drift -- {PAGE_PATH} matches what the \
                 generator emits"
            );
            return Ok(());
        }
        let (line, disk_line, gen_line) = first_difference(&on_disk, &rendered.page);
        bail!(
            "session-matrix --check: {PAGE_PATH} has drifted from what the generator emits.\n  \
             first difference at line {line}:\n    on disk:   {disk_line}\n    generator: \
             {gen_line}\n  This page is GENERATED and must not be hand-edited. Change the corpus \
             in crates/xtask/src/session_matrix.rs, run `cargo xtask session-matrix`, and commit \
             the result."
        );
    }

    fs::write(&path, &rendered.page)
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    eprintln!(
        "xtask: session-matrix: wrote {PAGE_PATH} ({} declined, {} table rows)",
        rendered.declined.len(),
        table_rows(&rendered.page).len()
    );
    eprintln!("xtask: session-matrix complete -- review `git diff` and commit.");
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

    /// A synthetic application nobody has ever run, whose name cannot collide
    /// with anything real.
    const UNEXECUTED: &str = "zzz-never-executed-probe";

    fn baseline() -> Rendered {
        render(&corpus()).expect("the checked-in corpus must render")
    }

    /// Render the real page with one extra *requested* application that has no
    /// execution record and no linkage measurement.
    fn with_unexecuted_request() -> Rendered {
        static REQ: &[Requested] = &[Requested {
            app: UNEXECUTED,
            why: "synthetic: injected by the non-vacuity test.",
            see_also: None,
        }];
        // Leak is fine and deliberate: `Corpus` holds `&'static` slices, and
        // this is a test process that exits.
        let mut requested: Vec<Requested> = REQUESTED.to_vec();
        requested.extend_from_slice(REQ);
        let requested: &'static [Requested] = Box::leak(requested.into_boxed_slice());
        render(&Corpus {
            header: HEADER,
            requested,
            executions: EXECUTIONS,
            linkage: LINKAGE,
        })
        .expect("adding a requested-but-unexecuted app must not break rendering")
    }

    /// **The non-vacuity test.** Issue #221's acceptance criterion 2: feed the
    /// generator an application it did not execute, and assert no row is
    /// emitted.
    ///
    /// This asserts against the **rendered markdown bytes**, not against the
    /// corpus that produced them: [`table_rows`] parses the page the generator
    /// actually wrote. So a change to `render` that emits an unexecuted
    /// application as a row fails here no matter which code path emitted it,
    /// which table it landed in, or which column it landed in.
    #[test]
    fn an_application_with_no_recorded_execution_gets_no_row() {
        let base = baseline();
        let injected = with_unexecuted_request();

        // 1. It appears in NO row of ANY table -- checked against the whole
        //    row, not just the app column, so it cannot hide in a later cell.
        for row in table_rows(&injected.page) {
            assert!(
                !row.line.contains(UNEXECUTED),
                "an application with no recorded execution was emitted as a table row in section \
                 {:?}: {}",
                row.section,
                row.line
            );
        }

        // 2. The row count did not move. A generator that emitted it anywhere
        //    -- including a table this test does not know the name of -- fails
        //    here even if it somehow spelled the name differently.
        assert_eq!(
            table_rows(&injected.page).len(),
            table_rows(&base.page).len(),
            "requesting an unexecuted application changed the number of table rows; the only \
             thing that may render a row is a recorded execution or a recorded measurement"
        );

        // 3. It was declined, with its reason -- not silently dropped.
        assert!(
            injected.declined.iter().any(|d| d.app == UNEXECUTED),
            "the unexecuted application was neither emitted nor declined, i.e. it vanished"
        );
        assert!(
            injected.page.contains(UNEXECUTED),
            "the unexecuted application must be NAMED on the page as not emitted; an absence \
             nobody can see is not an honest one"
        );
    }

    /// The declined mechanism must bite on the **real** corpus, not only on a
    /// synthetic input. If this ever fails, the requested set has been trimmed
    /// to match the evidence instead of the other way round, and the page has
    /// quietly stopped saying what it does not know.
    #[test]
    fn the_real_page_declines_at_least_one_application_somebody_wants() {
        let rendered = baseline();
        assert!(
            !rendered.declined.is_empty(),
            "no application is declined on the real page. Either every requested application has \
             been executed (land the evidence and celebrate) or REQUESTED has been trimmed to \
             match EXECUTIONS, which defeats the whole mechanism."
        );
        // The four the recorded Electron row over-claims, and the one that
        // cannot be regenerated on this machine.
        for app in ["Discord", "Slack", "Obsidian", "wofi"] {
            assert!(
                rendered.declined.iter().any(|d| d.app == app),
                "{app} has no execution record anywhere in this repository and must be declined"
            );
        }
    }

    /// Issue #221's acceptance criterion 3: each cell records app, package
    /// version, and the observable checked -- never a bare pass.
    #[test]
    fn no_emitted_cell_is_empty_or_a_bare_pass() {
        let rendered = baseline();
        let rows = table_rows(&rendered.page);
        assert!(rows.len() > 20, "the page rendered suspiciously few rows");
        for row in rows {
            for (i, cell) in row.cells.iter().enumerate() {
                let normalised = cell.trim().to_ascii_lowercase();
                assert!(
                    !BARE_PASS_WORDS.contains(&normalised.as_str()),
                    "cell {i} of a row in section {:?} records nothing ({cell:?}): {}",
                    row.section,
                    row.line
                );
            }
        }
    }

    /// The generator refuses to render a corpus whose observable says nothing,
    /// rather than publishing it. Watched failing is the point: without this,
    /// "each cell records the observable checked" is a promise in prose.
    #[test]
    fn a_bare_pass_in_the_corpus_refuses_to_render() {
        static REQ: &[Requested] = &[Requested {
            app: "bare-pass-probe",
            why: "synthetic.",
            see_also: None,
        }];
        static EXEC: &[Execution] = &[Execution {
            app: "bare-pass-probe",
            kind: Kind::DesktopApp,
            version: Version::InstalledAtInventory("1.0"),
            venue: Venue::Headless,
            bar: Bar::MappedAndRepainted,
            observable: "works",
            verdict: Verdict::Met,
            when: When::Dated("2026-08-10"),
            evidence: "synthetic",
            caveat: None,
        }];
        let err = render(&Corpus {
            header: HEADER,
            requested: REQ,
            executions: EXEC,
            linkage: &[],
        })
        .expect_err("an observable of \"works\" must be refused, not published");
        assert!(
            err.to_string().contains("records nothing"),
            "unexpected error: {err}"
        );
    }

    /// An execution nobody asked for must not render either: the two lists are
    /// the page's join, and letting them drift would let a row appear with no
    /// entry in the question list to explain why it is there.
    #[test]
    fn an_execution_absent_from_the_requested_set_refuses_to_render() {
        static EXEC: &[Execution] = &[Execution {
            app: "unrequested-probe",
            kind: Kind::DesktopApp,
            version: Version::InstalledAtInventory("1.0"),
            venue: Venue::Headless,
            bar: Bar::MappedAndRepainted,
            observable: "mapped a window and repainted",
            verdict: Verdict::Met,
            when: When::Dated("2026-08-10"),
            evidence: "synthetic",
            caveat: None,
        }];
        let err = render(&Corpus {
            header: HEADER,
            requested: &[],
            executions: EXEC,
            linkage: &[],
        })
        .expect_err("an execution not present in REQUESTED must be refused");
        assert!(
            err.to_string().contains("not in REQUESTED"),
            "unexpected error: {err}"
        );
    }

    /// An inventory claim with no measurement behind it is dropped, exactly as
    /// an application with no execution is.
    #[test]
    fn a_linkage_entry_with_no_measurement_gets_no_row() {
        static LINK: &[Linkage] = &[Linkage {
            app: "unmeasured-linkage-probe",
            version: Version::InstalledAtInventory("1.0"),
            family: Family::X11Only,
            wayland_mode: "none",
            measurement: "",
        }];
        let rendered = render(&Corpus {
            header: HEADER,
            requested: REQUESTED,
            executions: EXECUTIONS,
            linkage: LINK,
        })
        .expect("dropping an unmeasured linkage entry is not an error");
        for row in table_rows(&rendered.page) {
            assert!(
                !row.line.contains("unmeasured-linkage-probe"),
                "a linkage entry with no measurement was emitted: {}",
                row.line
            );
        }
    }

    /// The page must state the weak bar and say why it is weak, in itself --
    /// issue #221's key decision 3. A future edit that quietly drops any of
    /// these turns this red.
    #[test]
    fn the_page_names_the_weak_bar_and_why_it_is_weak() {
        let page = baseline().page;
        for phrase in [
            "mapped a window and repainted",
            "shim/src/globals.c:217-224",
            "GRANTS NOTHING ACROSS THE REALM BOUNDARY",
            "No portals",
            "No session bus",
            "No IME",
            "No XWayland child processes",
        ] {
            assert!(page.contains(phrase), "the page must state {phrase:?}");
        }
    }

    /// The non-X11 failures must be classified on the page, or a reader
    /// counting failures overstates what an X11 shim would buy.
    #[test]
    fn non_x11_failures_are_classified_as_such() {
        let page = baseline().page;
        assert!(page.contains("## Failures that are not X11 gaps"));
        assert!(page.contains("zwlr_layer_shell_v1"));
        assert!(page.contains("WS-E Stage 2"));
        // #203 is CLOSED and fixed on main, so it is described in the past
        // tense. This asserts the tense, which is the thing a reader gets
        // wrong.
        assert!(
            page.contains("Issue #203 is closed and fixed on `main`"),
            "#203 must be described as closed and fixed, not in the present tense"
        );
    }

    /// Issue #221's acceptance criterion 4: the header carries the machine,
    /// the versions, the revision and the date, and says CI cannot run this.
    #[test]
    fn the_header_carries_the_machine_the_versions_and_the_date() {
        let page = baseline().page;
        for phrase in [
            "**Inventory read**: 2026-08-10",
            "**Kernel**: `7.1.6-arch1-1`",
            "**Mesa**: `1:26.1.6-1`",
            "**wlroots**: `wlroots0.19 0.19.3-1",
            "**`vitrind` revision**: `38978f6",
            "One machine was measured",
            "no DRM device, no seat and no GPU",
        ] {
            assert!(page.contains(phrase), "the header must carry {phrase:?}");
        }
    }

    /// Issue #221's acceptance criterion 3 again, from the other side: the
    /// runbook has to be executable by the owner with nobody present.
    #[test]
    fn the_runbook_is_present_and_names_the_commands() {
        let page = baseline().page;
        for phrase in [
            "## Runbook: regenerate this page on the target machine",
            "cargo xtask session-matrix",
            "cargo xtask session-matrix --check",
            "readelf -dW",
            "NEVER use ldd",
            "vitrind --nested",
            "crates/xtask/src/session_matrix.rs",
        ] {
            assert!(page.contains(phrase), "the runbook must carry {phrase:?}");
        }
    }

    /// The markdown-table parser the non-vacuity test rests on must actually
    /// distinguish header rows, separators and data rows -- otherwise that
    /// test could pass by parsing nothing.
    #[test]
    fn the_table_parser_finds_data_rows_and_only_data_rows() {
        let page = "## S\n\n| A | B |\n|---|---|\n| one | two |\n| three | four |\n\ntext\n";
        let rows = table_rows(page);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].cells, vec!["one", "two"]);
        assert_eq!(rows[1].cells, vec!["three", "four"]);
        assert_eq!(rows[0].section, "S");
    }

    /// Every application table row must carry an evidence cell that points
    /// somewhere. A row whose last column is blank is a row nobody can check.
    #[test]
    fn every_application_row_points_at_its_evidence() {
        let rendered = baseline();
        for row in table_rows(&rendered.page) {
            if !row.section.contains("executed against") {
                continue;
            }
            assert_eq!(row.cells.len(), 7, "unexpected row shape: {}", row.line);
            let evidence = &row.cells[6];
            assert!(
                evidence.contains('`') || evidence.contains("issue #"),
                "row {:?} carries no checkable evidence: {evidence:?}",
                row.cells[0]
            );
        }
    }
}
