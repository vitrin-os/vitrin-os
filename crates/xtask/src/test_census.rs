// SPDX-License-Identifier: Apache-2.0
//! The **compile-time** half of issue #288: a `#[test]` that no CI step
//! builds, or that every step's name filter excludes, and therefore runs
//! nowhere at all.
//!
//! # Why this is not the skip census
//!
//! [`crate::skip_census`] reads a suite's *output*. That is the right shape
//! for a runtime skip -- the test ran, decided the machine could not do the
//! thing, and said so. It is structurally blind to the failure one level
//! below: a test behind a `#[cfg(feature = "...")]` no job enables, or one
//! that every filtered step's `--exact`/substring list happens to miss.
//! **There is no output to read**, so no census over output can see it, and
//! the only visible trace is a number in `N filtered out` that nobody diffs.
//!
//! **Twenty-four** tests in this tree were in exactly that state when this
//! check was first run, and it is what found them:
//!
//! | count | where | why nothing ran them |
//! |---|---|---|
//! | 14 | `vt::tests::*` | behind `drm-backend`, which one job compiles, and outside every name that job's filter lists |
//! | 5 | `consent::injector::live::tests::*` | behind `consent-injector`, compiled by one step whose `--exact` named two other tests |
//! | 3 | `dmabuf::gpu_tests::*` | behind `gpu-tests`, which only a `clippy` step ever built |
//! | 1 | `screenshot::tests::measure_encode_cost_at_a_real_panel_size` | `#[ignore]`d, and no run asks for ignored tests |
//! | 1 | `spawn::isolation::tests::probe_under_ignored_sigchld` | `#[ignore]`d, and executed as a child process rather than selected |
//!
//! Nineteen of the twenty-four are now wired into a CI step; the rest are in
//! [`UNRUN_TESTS`], which is the checked number. (Four *other* tests had
//! already been wired in by the change this check arrived with -- among them
//! one that had never been compiled at all and did not compile when it was:
//! it carried `isolation: IsolationOptions::default()` in pattern position,
//! which is not Rust. A test that has never been compiled is a citation, not
//! evidence.)
//!
//! # The check
//!
//! Static, and derived from three tables that are each held to something
//! outside themselves:
//!
//! 1. Every `#[test]` in the tree is parsed out of the sources, with the
//!    **conjunction of `cfg` predicates** that gates it -- its own, every
//!    enclosing inline `mod`'s, and every `mod name;` declaration on the path
//!    from the crate root to its file -- plus what `#[ignore]`s it, which is
//!    not always a bare attribute: `#[cfg_attr(<pred>, ignore)]` ignores a
//!    test wherever `<pred>` holds, so the condition is carried per-build
//!    ([`ignore_conditions`]) rather than collapsed to a `bool`. A test
//!    ignored that way in every build a job makes runs nowhere at all, which
//!    is the same failure as a feature no job enables reached by another
//!    route.
//! 2. [`CI_TEST_RUNS`] names every `cargo test` invocation in
//!    `.github/workflows/ci.yml`, with the features it enables and the name
//!    filters it passes. Each row carries a literal fragment of the workflow
//!    that must still be there, so the table cannot drift away from the file
//!    it describes, and the row count is held to the number of `cargo test`
//!    lines the workflow actually contains.
//! 3. [`UNRUN_TESTS`] is what is left: every `#[test]` no row in (2)
//!    selects. Each entry says why, and carries one of two [`Disposition`]s
//!    -- a **published gap**, whose named document must really mention the
//!    test, or **re-executed by** another test, whose body must really name
//!    it and which must itself be covered. Both are read from the tree, so
//!    neither is a sentence that can quietly stop being true. The publishing
//!    document's own **count** is generated and matched too
//!    ([`published_count_sentence`]): naming every gap in a bullet list while
//!    the number above it says something else is the same overclaim in the
//!    place a reader actually looks.
//!
//! A test covered by no row in (2) must appear in (3), and an entry in (3)
//! that some row now covers is equally red. **That is what makes the total a
//! number rather than a comment**: a test added tomorrow behind a feature no
//! job builds fails `cargo xtask skip-scan` until somebody either wires it
//! into a step or writes down, in public, that it cannot run.
//!
//! # What this does NOT close -- the bound, stated rather than implied
//!
//! * A test that runs and asserts nothing -- including one whose every
//!   assertion sits inside a conditional that is false at run time, which
//!   libtest counts as a pass and every number here counts as a test that
//!   ran. Nothing here reads assertions, and no amount of counting will.
//! * Whether a step that exists is ever REACHED. Every row in
//!   [`CI_TEST_RUNS`] today sits in an unconditional step of a job that
//!   always runs, but nothing here reads `if:` conditions, `continue-on-
//!   error`, job-level `needs`, or the workflow's branch filters. A step
//!   made conditional would keep this check green.
//! * A `cfg` predicate this parser cannot evaluate. Unknown atoms are a hard
//!   error rather than an assumed `true`, so the failure is loud -- but the
//!   remedy is a human teaching [`Pred`] about the atom, and until they do,
//!   `skip-scan` is red rather than partly right.
//! * Any number in the publishing document other than the one sentence
//!   [`published_count_sentence`] generates. The paragraph around it --
//!   "twenty-four were in this state when the check first ran, nineteen were
//!   wired into a job" -- is history, nothing re-derives it, and history that
//!   has quietly stopped being true reads exactly like history that has not.
//! * `#[path = "..."]` module declarations, which this walker does not
//!   implement. That one is *detected* rather than handled: `skip_scan`
//!   hands this check its own flat list of every file containing a `#[test]`,
//!   and a file the module walk never reached is a problem rather than a
//!   silence.
//! * The C shim's own suites (`shim/`, Meson) and the Python suites, which
//!   have their own gate machinery: `tests/integration/run.sh` classifies
//!   every module and fails on one that collected nothing, and
//!   `sdk/python/tests/conftest.py` carries the collection floor.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};

use crate::skip_census::{attrs_before, is_ident, mask_code, word_positions};

/// One `cargo test` invocation `.github/workflows/ci.yml` really performs.
///
/// The fields are the two things that decide whether a given `#[test]` runs:
/// what the compiler was told to build, and what libtest was told to select.
pub struct CiTestRun {
    /// The `jobs:` key it lives under, for the message.
    pub job: &'static str,
    /// `None` for `--workspace`, `Some(pkg)` for `-p pkg`.
    pub package: Option<&'static str>,
    /// `Some(name)` for `--bin name`, which restricts the run to that
    /// target's own tests and no other target in the package.
    pub bin: Option<&'static str>,
    /// The features passed on the command line, verbatim. Their transitive
    /// closure is computed from each package's own `[features]` table (so
    /// `drm-backend` really does bring `session-keymap`), and the package's
    /// `default` list is added unless [`no_default_features`] says otherwise
    /// -- both read from `Cargo.toml` rather than restated here, because a
    /// second copy of a feature graph is a second thing to drift.
    ///
    /// [`no_default_features`]: CiTestRun::no_default_features
    pub features: &'static [&'static str],
    /// Whether `--no-default-features` was passed.
    pub no_default_features: bool,
    /// libtest's positional filters. Empty means "every test in the target".
    pub filters: &'static [&'static str],
    /// Whether `--exact` was passed, which turns the filters from substring
    /// matches into equality.
    pub exact: bool,
    /// Whether the run passes `--ignored` or `--include-ignored`.
    pub ignored: bool,
    /// Whether `debug_assertions` holds, i.e. whether this is a debug build.
    /// `cargo test --release` is the one row where it does not, and this
    /// tree has a test that exists only under it (`spawn.rs`'s
    /// `a_spawn_record_dropped_without_journaling_is_caught`, whose guard
    /// aborts in debug and logs in release).
    pub debug: bool,
    /// A literal fragment of `.github/workflows/ci.yml` that must still be
    /// present. This is what stops the table from becoming a description of a
    /// workflow that has moved on.
    pub needle: &'static str,
}

/// Every `cargo test` in `.github/workflows/ci.yml`.
///
/// Order follows the workflow. A row per invocation, including the ones that
/// name no filter -- those cover everything their features build, and leaving
/// them out would make every test they run look uncovered.
pub const CI_TEST_RUNS: &[CiTestRun] = &[
    CiTestRun {
        job: "rust",
        package: None,
        bin: None,
        features: &[],
        no_default_features: false,
        filters: &[],
        exact: false,
        ignored: false,
        debug: true,
        needle: "-- cargo test --workspace -- --show-output",
    },
    CiTestRun {
        job: "rust",
        package: None,
        bin: None,
        features: &[],
        no_default_features: false,
        filters: &[],
        exact: false,
        ignored: false,
        // The one release row. Everything else in this table is a debug
        // build, and this is why `debug` is a field rather than an assumption.
        debug: false,
        needle: "run: cargo test --workspace --release",
    },
    CiTestRun {
        job: "rust",
        package: Some("vitrin-ipc"),
        bin: None,
        features: &["client"],
        no_default_features: true,
        filters: &[],
        exact: false,
        ignored: false,
        debug: true,
        needle: "run: cargo test -p vitrin-ipc --no-default-features --features client",
    },
    CiTestRun {
        job: "rust",
        package: Some("vitrin-core"),
        bin: None,
        features: &["session-keymap"],
        no_default_features: false,
        filters: &[],
        exact: false,
        ignored: false,
        debug: true,
        needle: "cargo test -p vitrin-core --features session-keymap",
    },
    CiTestRun {
        job: "rust",
        package: Some("vitrin-core"),
        bin: Some("vitrind"),
        features: &["consent-injector", "physical-input-injector"],
        no_default_features: false,
        filters: &[
            "tests::the_injector_flag_and_the_feature_are_both_required",
            "tests::the_physical_input_flag_parses_once_and_fails_closed",
            "consent::injector::live::tests::",
        ],
        exact: false,
        ignored: false,
        debug: true,
        needle: "cargo test -p vitrin-core --features consent-injector,physical-input-injector",
    },
    CiTestRun {
        job: "drm-compile-check",
        package: Some("vitrin-core"),
        bin: Some("vitrind"),
        features: &["drm-backend"],
        no_default_features: false,
        filters: &[
            "backend::drm::",
            "the_drm_refusal_table",
            "the_vt_escape_reserves_the_function_keys_on_bare_metal",
            "a_raised_consent_prompt_does_not_swallow_the_vt_escape",
            "vt::tests::",
        ],
        exact: false,
        ignored: false,
        debug: true,
        needle: "cargo test -p vitrin-core --features drm-backend --bin vitrind",
    },
    CiTestRun {
        job: "conformance",
        package: Some("vitrin-core"),
        bin: None,
        features: &[],
        no_default_features: false,
        filters: &["c_shim"],
        exact: false,
        ignored: false,
        debug: true,
        needle: "cargo test -p vitrin-core c_shim -- --show-output",
    },
    CiTestRun {
        job: "fuzz-smoke",
        package: Some("vitrin-fuzz"),
        bin: None,
        features: &[],
        no_default_features: false,
        filters: &[],
        exact: false,
        ignored: false,
        debug: true,
        needle: "cargo test --manifest-path fuzz/Cargo.toml",
    },
];

/// What it MEANS that no libtest run in CI selects a given `#[test]`.
///
/// Two things, and conflating them would be its own overclaim -- one is a
/// hole in the evidence and the other is evidence arriving by a route
/// libtest's own accounting cannot see.
pub enum Disposition {
    /// Nothing in CI exercises it, and the project says so in public.
    /// `published` is a repository-relative document that must MENTION the
    /// test by name; the check reads it, so "published" is a fact.
    PublishedGap { published: &'static str },
    /// It IS executed, by another `#[test]` that re-execs the test binary
    /// with `--exact --ignored` and asserts on the child's exit status. The
    /// named parent must exist, must itself be covered by a CI run, and its
    /// body must really name this test -- all checked, because "something
    /// else runs it" is exactly the kind of claim that rots.
    ReExecutedBy { parent: &'static str },
}

/// One `#[test]` that no libtest run in `.github/workflows/ci.yml` selects.
///
/// This is the number the whole module exists to make checkable. It is a set
/// of names and not a count for the same reason `INVENTORY` is: a count gets
/// bumped by reflex, a name is a line a reviewer reads.
pub struct UnrunTest {
    /// `<module path>::<fn>`, exactly as [`CI_TEST_RUNS`]'s filters would see
    /// it -- so `vt::tests::x`, never `vitrind::vt::tests::x`.
    pub test: &'static str,
    /// Which package defines it.
    pub package: &'static str,
    /// Why it is not simply wired into a job, which is the preferred answer.
    pub why: &'static str,
    /// Which of the two situations this is.
    pub disposition: Disposition,
}

/// Every `#[test]` in this tree that no libtest run in CI selects, and what
/// that means for each.
///
/// **Five.** Twenty-four were in this state when this check first ran; the
/// other nineteen -- five `consent-injector` socket tests and fourteen
/// `drm-backend` VT-escape tests -- were wired into a job instead, which is
/// the preferred half of the choice every time it is available and the reason
/// this list is short.
///
/// Four of the five are published gaps: three need a GPU no GitHub-hosted
/// runner has, and one is a timing measurement that asserts nothing. The
/// fifth is not a gap at all -- it is executed as a child process by the test
/// that sets up the disposition it measures -- and saying so is the point of
/// [`Disposition`] being an enum.
pub const UNRUN_TESTS: &[UnrunTest] = &[
    UnrunTest {
        test: "dmabuf::gpu_tests::real_gpu_dmabuf_frames_are_zero_copy_end_to_end",
        package: "vitrin-core",
        why: "needs a real GPU: an EGL device with a working GBM pipeline whose renderer imports \
              XRGB8888+LINEAR dmabufs. GitHub-hosted runners have none, which is why the whole \
              module is also `#[ignore]`d behind VITRIN_GPU_TESTS=1",
        disposition: Disposition::PublishedGap {
            published: "docs/book/src/limits.md",
        },
    },
    UnrunTest {
        test: "dmabuf::gpu_tests::real_gpu_probe_accepts_dmabuf_and_kills_memfd_lie",
        package: "vitrin-core",
        why: "needs a real GPU (EGL + DRM render node); the runner has no render node to open",
        disposition: Disposition::PublishedGap {
            published: "docs/book/src/limits.md",
        },
    },
    UnrunTest {
        test: "dmabuf::gpu_tests::real_gpu_oversized_dmabuf_center_crops_the_full_view",
        package: "vitrin-core",
        why: "needs a real GPU whose renderer imports XRGB8888+LINEAR dmabufs (plan risk R3)",
        disposition: Disposition::PublishedGap {
            published: "docs/book/src/limits.md",
        },
    },
    UnrunTest {
        test: "screenshot::tests::measure_encode_cost_at_a_real_panel_size",
        package: "vitrin-core",
        why: "a MEASUREMENT, not an assertion: it times a 2560x1600 PNG encode and prints the \
              number, which its own `#[ignore]` reason says out loud. A shared runner's timing is \
              not a number anybody can act on, and there is no failure it could report -- so it \
              is published as a gap rather than wired in, which is the honest half of the choice",
        disposition: Disposition::PublishedGap {
            published: "docs/book/src/limits.md",
        },
    },
    UnrunTest {
        test: "spawn::isolation::tests::probe_under_ignored_sigchld",
        package: "vitrin-core",
        why: "the CHILD half of a two-process test. Its parent re-execs the test binary with \
              `--exact --ignored` under SIG_IGN for SIGCHLD -- a disposition the whole binary \
              would otherwise share, which would flake every sibling that reaps a real realm -- \
              and asserts on the child's exit status. So it does execute in CI, on every run of \
              its parent; what it is not is SELECTED by a libtest run, which is a different \
              statement and is why this is not a `PublishedGap`",
        disposition: Disposition::ReExecutedBy {
            parent: "spawn::isolation::tests::a_launcher_that_ignores_sigchld_still_measures",
        },
    },
];

/// The sentence a document publishing gaps must carry, with the count in it.
///
/// `docs/book/src/limits.md` opened its list with *"Four `#[test]` functions
/// in this repository run in no CI job at all"* -- a number in prose, next to
/// a paragraph explaining that #288 had made it "a checked number rather than
/// a sentence". It had not: the check held every gap to a MENTION in that
/// file, so a fifth gap would have been named in a new bullet while the word
/// "Four" above it went quietly wrong. A count nobody checks is exactly the
/// artefact this module exists to replace, and it is worse in a paragraph
/// claiming to be checked.
///
/// So the sentence is generated here and the document must contain it
/// verbatim (whitespace-insensitively, so re-wrapping a paragraph is not a
/// build break). Both directions: the right sentence must be present, and no
/// OTHER count's version of it may be.
fn published_count_sentence(count: usize) -> String {
    let word = match count {
        0 => "No",
        1 => "One",
        2 => "Two",
        3 => "Three",
        4 => "Four",
        5 => "Five",
        6 => "Six",
        7 => "Seven",
        8 => "Eight",
        9 => "Nine",
        10 => "Ten",
        11 => "Eleven",
        12 => "Twelve",
        // Past twelve the sentence is the least of the problems, and a digit
        // is better than a wrong word.
        _ => {
            return format!(
                "{count} `#[test]` functions in this repository run in no CI job at all"
            )
        }
    };
    if count == 1 {
        return "One `#[test]` function in this repository runs in no CI job at all".to_string();
    }
    format!("{word} `#[test]` functions in this repository run in no CI job at all")
}

/// `text` with every run of whitespace collapsed to one space, so a sentence
/// that spans a line break in Markdown still matches.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// cfg predicates
// ---------------------------------------------------------------------------

/// A `cfg` predicate, as far as this scan needs to understand one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pred {
    /// `feature = "name"`.
    Feature(String),
    /// `test` -- always true in the builds this module reasons about, all of
    /// which are `cargo test`.
    Test,
    /// `debug_assertions` -- true for every row in [`CI_TEST_RUNS`] except
    /// `cargo test --workspace --release`.
    DebugAssertions,
    /// `all(..)`, `any(..)`, `not(..)`.
    All(Vec<Pred>),
    Any(Vec<Pred>),
    Not(Box<Pred>),
    /// Anything else. **Never assumed true**: evaluating one is an error, so
    /// a `cfg` nobody taught this scan about is a red build rather than a
    /// silent "of course it compiles".
    Unknown(String),
}

/// Parse the inside of a `cfg(...)`.
fn parse_pred(text: &str) -> Pred {
    let text = text.trim();
    for head in ["all", "any", "not"] {
        let Some(rest) = text.strip_prefix(head) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
            continue;
        };
        let parts: Vec<Pred> = split_top_level(inner)
            .iter()
            .map(|p| parse_pred(p))
            .collect();
        return match head {
            "all" => Pred::All(parts),
            "any" => Pred::Any(parts),
            _ => Pred::Not(Box::new(
                parts
                    .into_iter()
                    .next()
                    .unwrap_or(Pred::Unknown(String::new())),
            )),
        };
    }
    if let Some((key, value)) = text.split_once('=') {
        if key.trim() == "feature" {
            return Pred::Feature(value.trim().trim_matches('"').to_string());
        }
        return Pred::Unknown(text.to_string());
    }
    if text == "test" {
        return Pred::Test;
    }
    if text == "debug_assertions" {
        return Pred::DebugAssertions;
    }
    Pred::Unknown(text.to_string())
}

/// Split `a, b(c, d), e` on top-level commas.
fn split_top_level(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// One package's `[features]` table, as far as this scan needs it.
#[derive(Debug, Clone, Default)]
struct FeatureGraph {
    /// The `default = [..]` list, empty when there is none.
    default: Vec<String>,
    /// Every other feature and what it enables. `dep:x` and `pkg/feat`
    /// entries are dropped: they turn on a dependency, never a `cfg(feature)`
    /// of this package.
    edges: BTreeMap<String, Vec<String>>,
}

/// Read a package's `[features]` table out of its `Cargo.toml`.
///
/// A hand parser, because `xtask` deliberately has no TOML dependency -- and
/// a narrow one, because it only has to read `name = [ "a", "b" ]` with
/// comments and line breaks inside the array, which is exactly how
/// `crates/vitrin-core/Cargo.toml` writes them.
fn feature_graph(manifest: &Path) -> Result<FeatureGraph> {
    let text = std::fs::read_to_string(manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let mut graph = FeatureGraph::default();
    let mut in_features = false;
    let mut key: Option<String> = None;
    let mut buffer = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        if !in_features || line.starts_with('#') {
            continue;
        }
        if key.is_none() {
            let Some((name, rest)) = line.split_once('=') else {
                continue;
            };
            key = Some(name.trim().to_string());
            buffer = rest.trim().to_string();
        } else {
            buffer.push(' ');
            buffer.push_str(line);
        }
        // Complete only once the array closes; features here span lines.
        if !buffer.contains(']') {
            continue;
        }
        let name = key.take().unwrap_or_default();
        let values: Vec<String> = buffer
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|v| !v.starts_with("dep:") && !v.contains('/'))
            .map(str::to_string)
            .collect();
        if name == "default" {
            graph.default = values;
        } else {
            graph.edges.insert(name, values);
        }
        buffer.clear();
    }
    Ok(graph)
}

/// The build one [`CiTestRun`] performs, as far as `cfg` is concerned.
#[derive(Debug, Clone, Default)]
struct Build {
    features: BTreeSet<String>,
    debug_assertions: bool,
}

/// The features a package really compiles with under `run`: the command
/// line's, plus the package's defaults unless they were turned off, closed
/// transitively over the package's own `[features]` table.
fn build_for(run: &CiTestRun, graph: &FeatureGraph) -> Build {
    let mut queue: Vec<String> = run.features.iter().map(|f| (*f).to_string()).collect();
    if !run.no_default_features {
        queue.extend(graph.default.iter().cloned());
    }
    let mut features: BTreeSet<String> = BTreeSet::new();
    while let Some(feature) = queue.pop() {
        if !features.insert(feature.clone()) {
            continue;
        }
        if let Some(implied) = graph.edges.get(&feature) {
            queue.extend(implied.iter().cloned());
        }
    }
    Build {
        features,
        debug_assertions: run.debug,
    }
}

/// Whether `pred` holds for a `cargo test` build described by `build`.
///
/// # Errors
///
/// On an atom this scan does not understand. That is deliberate: assuming
/// `true` would silently declare a test covered, which is the class of
/// mistake this module exists against.
fn holds(pred: &Pred, build: &Build) -> Result<bool> {
    Ok(match pred {
        Pred::Feature(name) => build.features.contains(name),
        Pred::Test => true,
        Pred::DebugAssertions => build.debug_assertions,
        Pred::All(parts) => {
            for p in parts {
                if !holds(p, build)? {
                    return Ok(false);
                }
            }
            true
        }
        Pred::Any(parts) => {
            for p in parts {
                if holds(p, build)? {
                    return Ok(true);
                }
            }
            false
        }
        Pred::Not(inner) => !holds(inner, build)?,
        Pred::Unknown(text) => anyhow::bail!(
            "cargo xtask skip-scan does not understand the cfg predicate `{text}`, and refuses \
             to guess. Guessing `true` would report a test as covered by a job that never built \
             it, which is the whole failure this check exists against (issue #288). Teach \
             `Pred` in crates/xtask/src/test_census.rs what it means."
        ),
    })
}

// ---------------------------------------------------------------------------
// parsing the tree
// ---------------------------------------------------------------------------

/// Which cargo target compiles a source file's tests.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// `src/lib.rs` and everything it declares.
    Lib,
    /// `src/main.rs` and everything it declares -- the package's `[[bin]]`.
    MainBin,
    /// `src/bin/<name>.rs`.
    OtherBin(String),
    /// `tests/<name>.rs`.
    Integration(String),
}

/// One `#[test]` function, with everything that decides whether it runs.
#[derive(Debug, Clone)]
struct TestItem {
    package: String,
    target: Target,
    /// Module path inside the target plus the function name -- exactly what
    /// libtest matches a filter against.
    path: String,
    file: String,
    line: usize,
    /// Conjunction of every `cfg` between the crate root and this function.
    cfg: Vec<Pred>,
    /// Whether it carries an unconditional `#[ignore]`, which keeps it out of
    /// every run that does not ask for ignored tests.
    ignored: bool,
    /// The conditions under which `#[cfg_attr(<pred>, ignore)]` ignores it --
    /// evaluated per build, because a predicate that holds in one row's
    /// feature set may not hold in another's. See [`ignore_conditions`].
    ignored_if: Vec<Pred>,
    /// The function's body, UNMASKED -- so a `Disposition::ReExecutedBy`
    /// claim can be held to the parent really naming its child, which it does
    /// in a string literal the mask would have blanked.
    body: String,
}

/// A module declared inside one file: `mod name;` or `mod name { .. }`.
struct ModDecl {
    name: String,
    cfg: Vec<Pred>,
    /// `None` for `mod name;`; the inline body's byte range otherwise.
    inline: Option<(usize, usize)>,
}

/// One `#[test] fn` found in a source file.
struct TestDecl {
    name: String,
    /// Byte offset of the `fn` keyword.
    at: usize,
    cfg: Vec<Pred>,
    ignored: bool,
    ignored_if: Vec<Pred>,
    line: usize,
    /// The body, taken from the UNMASKED source (offsets agree, because
    /// `mask_code` preserves byte length).
    body: String,
}

/// Every `mod` declaration and every `#[test] fn` in one masked source.
fn scan_source(masked: &str, src: &str) -> (Vec<ModDecl>, Vec<TestDecl>) {
    let mut mods = Vec::new();
    for at in word_positions(masked, "mod") {
        let rest = &masked[at + 3..];
        let name_start = at + 3 + (rest.len() - rest.trim_start().len());
        let name_end = name_start
            + masked[name_start..]
                .find(|c: char| !is_ident(c))
                .unwrap_or(0);
        if name_end == name_start {
            continue;
        }
        let name = masked[name_start..name_end].to_string();
        let after = &masked[name_end..];
        let semi = after.find(';');
        let brace = after.find('{');
        let inline = match (semi, brace) {
            (Some(s), Some(b)) if s < b => None,
            (Some(_), None) => None,
            (_, Some(b)) => {
                let open = name_end + b;
                let mut depth = 0usize;
                let mut close = open;
                for (offset, c) in masked[open..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                close = open + offset;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Some((open, close))
            }
            (None, None) => continue,
        };
        mods.push(ModDecl {
            name,
            cfg: cfgs(&attrs_before(masked, src, at)),
            inline,
        });
    }

    let mut tests = Vec::new();
    for at in word_positions(masked, "fn") {
        let attrs = attrs_before(masked, src, at);
        if !attrs.iter().any(|a| a.trim() == "test") {
            continue;
        }
        let ignored = attrs.iter().any(|a| is_ignore(a));
        let ignored_if = ignore_conditions(&attrs);
        let rest = &masked[at + 2..];
        let name_start = at + 2 + (rest.len() - rest.trim_start().len());
        let name_end = name_start
            + masked[name_start..]
                .find(|c: char| !is_ident(c))
                .unwrap_or(0);
        let name = masked[name_start..name_end].to_string();
        let line = masked[..at].matches('\n').count() + 1;
        let body = match masked[name_end..].find('{').map(|o| name_end + o) {
            Some(open) => {
                let mut depth = 0usize;
                let mut close = open;
                for (offset, c) in masked[open..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                close = open + offset;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                src[open..=close].to_string()
            }
            None => String::new(),
        };
        tests.push(TestDecl {
            name,
            at,
            cfg: cfgs(&attrs),
            ignored,
            ignored_if,
            line,
            body,
        });
    }
    (mods, tests)
}

/// The `cfg(..)` predicates among a set of attribute bodies.
fn cfgs(attrs: &[String]) -> Vec<Pred> {
    attrs
        .iter()
        .filter_map(|a| {
            let a = a.trim();
            // `cfg_attr(..)` is a different attribute that happens to share
            // the prefix, and reading its predicate as a compilation gate
            // would be wrong in the dangerous direction: it decides which
            // ATTRIBUTES apply, never whether the item exists.
            if a.starts_with("cfg_attr") {
                return None;
            }
            let rest = a.strip_prefix("cfg")?.trim_start();
            let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
            Some(parse_pred(inner))
        })
        .collect()
}

/// Whether an attribute body IS `#[ignore]` -- bare, or with a reason.
fn is_ignore(attr: &str) -> bool {
    let attr = attr.trim();
    attr == "ignore" || attr.starts_with("ignore ") || attr.starts_with("ignore=")
}

/// The conditions under which a set of attributes `#[ignore]`s the item.
///
/// `#[cfg_attr(<pred>, ignore)]` is the shape this exists for, and it is not
/// a curiosity: it applies `#[ignore]` to a test **wherever `<pred>` holds**,
/// so `#[cfg_attr(target_arch = "x86_64", ignore)]` on a test in a tree that
/// only ever builds x86-64 produces a test that runs nowhere at all, while
/// every count in this module reports the tree clean. It is the same failure
/// as a `#[cfg(feature)]` no job enables, arriving by the one route the
/// unconditional-`#[ignore]` check could not see, so the condition is carried
/// per-build rather than collapsed to a `bool`: a `cargo test --ignored` row
/// still selects it, and a predicate that is false in a given build does not
/// exclude it there.
///
/// Nesting (`cfg_attr(a, cfg_attr(b, ignore))`) is folded into an `all(..)`
/// rather than ignored, because "this parser stopped one level early" is the
/// silent half of the same mistake.
fn ignore_conditions(attrs: &[String]) -> Vec<Pred> {
    let mut out = Vec::new();
    for attr in attrs {
        collect_ignore_conditions(attr.trim(), &mut Vec::new(), &mut out);
    }
    out
}

/// Walk one attribute body, carrying the `cfg_attr` predicates above it.
fn collect_ignore_conditions(attr: &str, carried: &mut Vec<Pred>, out: &mut Vec<Pred>) {
    let attr = attr.trim();
    if is_ignore(attr) {
        // A bare `#[ignore]` at the top level is not a *condition*; the caller
        // reads that from the attribute list directly. Reaching here with
        // nothing carried can only happen through an empty `cfg_attr`, which
        // is not something Rust accepts.
        if !carried.is_empty() {
            out.push(Pred::All(carried.clone()));
        }
        return;
    }
    let Some(rest) = attr.strip_prefix("cfg_attr") else {
        return;
    };
    let Some(inner) = rest.trim_start().strip_prefix('(').and_then(|r| {
        // Only the LAST `)` closes the attribute; the predicate has its own.
        r.strip_suffix(')')
    }) else {
        return;
    };
    let parts = split_top_level(inner);
    let Some((pred, applied)) = parts.split_first() else {
        return;
    };
    carried.push(parse_pred(pred));
    for one in applied {
        collect_ignore_conditions(one, carried, out);
    }
    carried.pop();
}

/// Where a file's child modules live, given the file's own path.
fn child_dir(file: &Path) -> std::path::PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned());
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    match stem.as_deref() {
        Some("lib" | "main" | "mod") => dir,
        Some(other) => dir.join(other),
        None => dir,
    }
}

/// Every `#[test]` in the tree, with its target, its libtest path and the
/// `cfg` conjunction that gates it.
fn collect_tests(root: &Path) -> Result<Vec<TestItem>> {
    // package name -> package directory
    let mut packages: Vec<(String, std::path::PathBuf)> = Vec::new();
    let crates_dir = root.join("crates");
    if crates_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&crates_dir)
            .with_context(|| format!("reading {}", crates_dir.display()))?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::result::Result<_, _>>()?;
        entries.sort();
        for path in entries {
            if path.join("Cargo.toml").is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                packages.push((name.into_owned(), path));
            }
        }
    }
    // `fuzz/` is its own workspace, and `cargo test --workspace` never
    // reaches it -- which is exactly why it has to be in this census.
    if root.join("fuzz/Cargo.toml").is_file() {
        packages.push(("vitrin-fuzz".to_string(), root.join("fuzz")));
    }

    let mut out = Vec::new();
    for (package, dir) in &packages {
        // Roots: one per cargo target.
        let mut roots: Vec<(std::path::PathBuf, Target)> = Vec::new();
        for (rel, target) in [
            ("src/lib.rs", Target::Lib),
            ("src/main.rs", Target::MainBin),
        ] {
            if dir.join(rel).is_file() {
                roots.push((dir.join(rel), target));
            }
        }
        for sub in ["src/bin", "tests"] {
            let d = dir.join(sub);
            if !d.is_dir() {
                continue;
            }
            let mut entries: Vec<_> = std::fs::read_dir(&d)
                .with_context(|| format!("reading {}", d.display()))?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::result::Result<_, _>>()?;
            entries.sort();
            for path in entries {
                if path.extension().is_some_and(|e| e == "rs") {
                    let name = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    roots.push((
                        path,
                        if sub == "tests" {
                            Target::Integration(name)
                        } else {
                            Target::OtherBin(name)
                        },
                    ));
                }
            }
        }

        for (root_file, target) in roots {
            // Depth-first over the module tree of this target.
            let mut queue = vec![(root_file, String::new(), Vec::<Pred>::new())];
            while let Some((file, prefix, inherited)) = queue.pop() {
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading {}", file.display()))?;
                let masked = mask_code(&text);
                let (mods, tests) = scan_source(&masked, &text);
                let rel = file
                    .strip_prefix(root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .into_owned();

                // Inline modules, innermost-last, so a test's path and cfg
                // chain can be read off by byte containment.
                let inline: Vec<(&ModDecl, (usize, usize))> = mods
                    .iter()
                    .filter_map(|m| m.inline.map(|range| (m, range)))
                    .collect();

                for decl in tests {
                    let TestDecl {
                        name,
                        at,
                        cfg: own_cfg,
                        ignored,
                        ignored_if,
                        line,
                        body,
                    } = decl;
                    let mut path_parts: Vec<&str> = Vec::new();
                    let mut cfg = inherited.clone();
                    let mut enclosing: Vec<&(&ModDecl, (usize, usize))> = inline
                        .iter()
                        .filter(|(_, (open, close))| at > *open && at < *close)
                        .collect();
                    enclosing.sort_by_key(|(_, (open, _))| *open);
                    for (decl, _) in enclosing {
                        path_parts.push(&decl.name);
                        cfg.extend(decl.cfg.iter().cloned());
                    }
                    cfg.extend(own_cfg);
                    let mut path = String::new();
                    if !prefix.is_empty() {
                        path.push_str(&prefix);
                        path.push_str("::");
                    }
                    for part in &path_parts {
                        path.push_str(part);
                        path.push_str("::");
                    }
                    path.push_str(&name);
                    out.push(TestItem {
                        package: package.clone(),
                        target: target.clone(),
                        path,
                        file: rel.clone(),
                        line,
                        cfg,
                        ignored,
                        ignored_if,
                        body,
                    });
                }

                // File modules, queued with their own cfg folded in.
                let dir = child_dir(&file);
                for decl in &mods {
                    if decl.inline.is_some() {
                        continue;
                    }
                    let candidates = [
                        dir.join(format!("{}.rs", decl.name)),
                        dir.join(&decl.name).join("mod.rs"),
                    ];
                    let Some(child) = candidates.into_iter().find(|p| p.is_file()) else {
                        continue;
                    };
                    let mut cfg = inherited.clone();
                    cfg.extend(decl.cfg.iter().cloned());
                    let child_prefix = if prefix.is_empty() {
                        decl.name.clone()
                    } else {
                        format!("{prefix}::{}", decl.name)
                    };
                    queue.push((child, child_prefix, cfg));
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// the check
// ---------------------------------------------------------------------------

/// Whether `test` is `#[ignore]`d in a build described by `build`.
///
/// Unconditionally, or by a `#[cfg_attr(<pred>, ignore)]` whose predicate
/// holds here. The second half is per-build rather than global on purpose: a
/// conditional ignore can be true in the feature slice one job builds and
/// false in another's, and collapsing it either way would make a coverage
/// claim that is wrong in one of the two directions.
fn ignored_in(test: &TestItem, build: &Build) -> Result<bool> {
    if test.ignored {
        return Ok(true);
    }
    for pred in &test.ignored_if {
        if holds(pred, build)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `run` compiles `test` **and** libtest would select it.
///
/// Both halves matter and they fail differently: a feature no job enables
/// means the test does not exist in any binary, and a filter that does not
/// match means it exists and is counted in `N filtered out`. This module was
/// written because the tree had both.
fn covers(run: &CiTestRun, test: &TestItem, build: &Build) -> Result<bool> {
    if let Some(package) = run.package {
        if package != test.package {
            return Ok(false);
        }
    } else if test.package == "vitrin-fuzz" {
        // `--workspace` is the ROOT workspace, and fuzz/ is deliberately not
        // a member of it (fuzz/README.md). This is the one package a
        // `--workspace` row does not reach.
        return Ok(false);
    }
    if run.bin.is_some() && test.target != Target::MainBin {
        return Ok(false);
    }
    if ignored_in(test, build)? && !run.ignored {
        return Ok(false);
    }
    for pred in &test.cfg {
        if !holds(pred, build)? {
            return Ok(false);
        }
    }
    if run.filters.is_empty() {
        return Ok(true);
    }
    Ok(run.filters.iter().any(|f| {
        if run.exact {
            test.path == *f
        } else {
            test.path.contains(f)
        }
    }))
}

/// Hold every `#[test]` in the tree to a CI step that runs it, or to a
/// published gap. Appends to `problems` rather than bailing, so one run of
/// `cargo xtask skip-scan` reports everything it found.
///
/// # What "every" means here, exactly
///
/// This finds `#[test]` **syntactically, in unexpanded source**, so two
/// shapes fall outside the word and neither is closed by anything:
///
/// * `#[cfg_attr(<pred>, test)]` -- a function that becomes a test only under
///   a predicate. It is not found here, and it is not found by the return
///   scan either, so it is the one shape that evades both halves at once.
/// * a `macro_rules!` expanding to `#[test]` functions, which is modelled as
///   one nameless template rather than as the tests it produces. This tree
///   has one -- `roundtrip_test!`, on the order of forty tests -- so the
///   coverage claim is per-*template* there, not per-test.
///
/// Both are named in `vitrin_skip`'s "What this does NOT close", which is the
/// canonical list. Read the per-test claim as holding for hand-written
/// `#[test]` functions.
pub fn check_test_coverage(
    root: &Path,
    files_with_tests: &BTreeSet<String>,
    problems: &mut Vec<String>,
) -> Result<()> {
    let tests = collect_tests(root)?;
    anyhow::ensure!(
        tests.len() > 100,
        "the test census parsed only {} #[test] functions -- it is not reading the tree, and \
         every conclusion below would be vacuous",
        tests.len()
    );

    // This census walks the MODULE TREE from each cargo target's root, which
    // is the only way to know which target compiles a file. The consequence
    // is that a file no `mod` declaration reaches is invisible to it -- so
    // its tests would be reported as covered by saying nothing about them at
    // all. `skip_scan` reads every `.rs` file under crates/ and fuzz/ flatly,
    // so the two disagree exactly when that happens, and the disagreement is
    // the check. (Today's causes would be a `#[path = "..."]` attribute,
    // which this walker does not implement, or a `mod x;` whose file was
    // moved.)
    let walked: BTreeSet<&str> = tests.iter().map(|t| t.file.as_str()).collect();
    for file in files_with_tests {
        if !walked.contains(file.as_str()) {
            problems.push(format!(
                "{file} contains #[test] functions, and the test census's walk from the crate \
                 roots never reached it -- so nothing here can say which CI step runs them, and \
                 they would pass this check by being invisible to it. Either a `#[path = \"...\"]` \
                 attribute (which crates/xtask/src/test_census.rs does not implement) or a `mod` \
                 declaration that no longer resolves."
            ));
        }
    }
    // Only THIS check's findings decide whether its summary is printed: the
    // caller may already be carrying problems from the CI-declaration scan,
    // and a census that went quiet because something unrelated failed is a
    // number that disappears exactly when somebody is reading the log.
    let before = problems.len();
    check_ci_test_runs(root, problems)?;

    let published: BTreeSet<(&str, &str)> =
        UNRUN_TESTS.iter().map(|u| (u.package, u.test)).collect();

    // One feature graph per package, read from its own Cargo.toml, so
    // `drm-backend -> session-keymap` and `default -> server -> client` are
    // facts about the manifests rather than a second table to keep in step.
    let mut graphs: BTreeMap<String, FeatureGraph> = BTreeMap::new();
    for test in &tests {
        if graphs.contains_key(&test.package) {
            continue;
        }
        let manifest = if test.package == "vitrin-fuzz" {
            root.join("fuzz/Cargo.toml")
        } else {
            root.join("crates").join(&test.package).join("Cargo.toml")
        };
        graphs.insert(test.package.clone(), feature_graph(&manifest)?);
    }

    for test in &tests {
        let graph = &graphs[&test.package];
        let mut covered = false;
        for run in CI_TEST_RUNS {
            if covers(run, test, &build_for(run, graph))? {
                covered = true;
                break;
            }
        }
        let key = (test.package.as_str(), test.path.as_str());
        if covered {
            if published.contains(&key) {
                problems.push(format!(
                    "{}:{} `{}` is listed in UNRUN_TESTS as a test no libtest run in CI selects, \
                     and a row in CI_TEST_RUNS now selects it. Delete the UNRUN_TESTS entry -- a \
                     gap that has been closed is a claim the project is understating itself, and \
                     the next reader will trust it.",
                    test.file, test.line, test.path,
                ));
            }
            continue;
        }
        if !published.contains(&key) {
            problems.push(format!(
                "{}:{} `{}` is compiled and run by NO step in .github/workflows/ci.yml. Its cfg \
                 gate is {:?}{}. Either wire it into a job -- a feature slice or a filter is \
                 usually two lines, and it is the right answer whenever it is available -- or add \
                 it to UNRUN_TESTS in crates/xtask/src/test_census.rs, saying why and publishing \
                 the gap. A test nothing runs is a citation, not evidence (#288).",
                test.file,
                test.line,
                test.path,
                test.cfg,
                if test.ignored {
                    ", and it is #[ignore]d".to_string()
                } else if !test.ignored_if.is_empty() {
                    format!(
                        ", and #[cfg_attr(..)] ignores it wherever {:?} holds",
                        test.ignored_if
                    )
                } else {
                    String::new()
                },
            ));
        }
    }

    // The other direction, per entry: the test must still exist, and whatever
    // the entry claims about it must still be true.
    let by_key: BTreeMap<(&str, &str), &TestItem> = tests
        .iter()
        .map(|t| ((t.package.as_str(), t.path.as_str()), t))
        .collect();
    for entry in UNRUN_TESTS {
        if !by_key.contains_key(&(entry.package, entry.test)) {
            problems.push(format!(
                "UNRUN_TESTS names `{}` in {}, and no such #[test] exists any more. Delete the \
                 entry -- and, if it was a published gap, the sentence it points at.",
                entry.test, entry.package,
            ));
            continue;
        }
        match entry.disposition {
            // The publication itself, which is the half that makes
            // "published" a fact rather than an intention.
            Disposition::PublishedGap { published } => {
                let doc = root.join(published);
                let text = std::fs::read_to_string(&doc)
                    .with_context(|| format!("reading {} for UNRUN_TESTS", doc.display()))?;
                let leaf = entry.test.rsplit("::").next().unwrap_or(entry.test);
                if !text.contains(leaf) {
                    problems.push(format!(
                        "UNRUN_TESTS says `{}` is published in {}, and that file does not mention \
                         it. A gap nobody can read is not published; name it there, or stop \
                         claiming it is.",
                        entry.test, published,
                    ));
                }
            }
            // ...and the same standard for the other claim: the parent must
            // exist, must itself be run by CI, and must really name its child.
            Disposition::ReExecutedBy { parent } => {
                let Some(item) = by_key.get(&(entry.package, parent)) else {
                    problems.push(format!(
                        "UNRUN_TESTS says `{}` is re-executed by `{parent}`, and no #[test] by \
                         that name exists. Nothing runs the child.",
                        entry.test,
                    ));
                    continue;
                };
                if !names_whole(&item.body, entry.test) {
                    problems.push(format!(
                        "UNRUN_TESTS says `{}` is re-executed by `{parent}`, and that test's body \
                         does not name it. The claim was true once; a rename made it a sentence \
                         about nothing, and the child now runs nowhere.",
                        entry.test,
                    ));
                }
                let graph = &graphs[entry.package];
                if !CI_TEST_RUNS
                    .iter()
                    .any(|run| covers(run, item, &build_for(run, graph)).unwrap_or(false))
                {
                    problems.push(format!(
                        "UNRUN_TESTS says `{}` is re-executed by `{parent}`, and no CI step runs \
                         the PARENT either. A chain of coverage is only as long as its first \
                         link.",
                        entry.test,
                    ));
                }
            }
        }
    }

    // ...and the COUNT each publishing document states, which is a third
    // claim and was the one nothing read. A document may name every gap
    // correctly and still open with the wrong number above them.
    let mut per_document: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in UNRUN_TESTS {
        if let Disposition::PublishedGap { published } = entry.disposition {
            *per_document.entry(published).or_default() += 1;
        }
    }
    for (published, count) in &per_document {
        let doc = root.join(published);
        let text = std::fs::read_to_string(&doc)
            .with_context(|| format!("reading {} for UNRUN_TESTS", doc.display()))?;
        let text = squeeze(&text);
        let wanted = published_count_sentence(*count);
        if !text.contains(&wanted) {
            problems.push(format!(
                "{published} publishes {count} gap(s) from UNRUN_TESTS and does not contain the \
                 sentence \"{wanted}\". The list of names is checked; the NUMBER above it was \
                 not, so a fifth gap could be named in a new bullet under a paragraph that still \
                 said four. Write that sentence there, or change the table."
            ));
        }
        // The other direction: a stale count left behind by an edit that
        // added the new sentence without removing the old one.
        for other in 0..=12 {
            if other == *count {
                continue;
            }
            let stale = published_count_sentence(other);
            if text.contains(&stale) {
                problems.push(format!(
                    "{published} says \"{stale}\", and UNRUN_TESTS publishes {count} gap(s) there. \
                     One of the two is wrong, and the sentence is the half a reader believes."
                ));
            }
        }
    }

    if problems.len() == before {
        // Printed every time, itemised, because the number IS the deliverable:
        // a reader has to be able to watch it grow. A gap kept in a comment is
        // a gap that grows silently, which is the shape this module exists
        // against.
        let gaps = UNRUN_TESTS
            .iter()
            .filter(|u| matches!(u.disposition, Disposition::PublishedGap { .. }))
            .count();
        println!(
            "xtask skip-scan: {} #[test] function(s) held to {} CI test run(s); {} selected by no \
             CI run, of which {gaps} are published gaps:",
            tests.len(),
            CI_TEST_RUNS.len(),
            UNRUN_TESTS.len(),
        );
        for entry in UNRUN_TESTS {
            let tail = match entry.disposition {
                Disposition::PublishedGap { published } => {
                    format!("PUBLISHED GAP, in {published}")
                }
                Disposition::ReExecutedBy { parent } => format!("re-executed by {parent}"),
            };
            println!(
                "                [{}] {} -- {} ({tail})",
                entry.package, entry.test, entry.why
            );
        }
    }
    Ok(())
}

/// Whether `haystack` names `needle` as a WHOLE identifier path.
///
/// A plain `contains` is not enough here and the difference is not academic:
/// renaming `probe_under_ignored_sigchld` to
/// `probe_under_ignored_sigchld_RENAMED` leaves the old name as a prefix of
/// the new one, so `contains` still says yes while the parent now re-execs a
/// test that does not exist. That is the check going green over a chain it
/// no longer has -- measured, as a lever, against the first version of this
/// function.
fn names_whole(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(found) = haystack[from..].find(needle) {
        let at = from + found;
        let end = at + needle.len();
        let before = haystack[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident(c) && c != ':');
        let after = haystack[end..].chars().next().is_none_or(|c| !is_ident(c));
        if before && after {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Hold [`CI_TEST_RUNS`] to the workflow it describes, both directions.
fn check_ci_test_runs(root: &Path, problems: &mut Vec<String>) -> Result<()> {
    let path = root.join(".github/workflows/ci.yml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    for run in CI_TEST_RUNS {
        let Some(at) = lines.iter().position(|line| line.contains(run.needle)) else {
            problems.push(format!(
                "CI_TEST_RUNS describes a `{}` step containing {:?}, and .github/workflows/ci.yml \
                 no longer contains that text. The table is how this scan knows what CI runs, so \
                 a table describing a workflow that has moved on proves nothing. Update both.",
                run.job, run.needle,
            ));
            continue;
        };
        // The needle proves the COMMAND is still there; the filters are what
        // decide which tests it selects, and they are a separate claim. A row
        // that kept `vt::tests::` after the workflow dropped it would go on
        // reporting fourteen tests as covered by a step that no longer runs
        // them -- a coverage claim too WIDE, which is the direction that
        // hides a test rather than the direction that merely nags. So each
        // filter has to appear in the same step: from the needle's line down
        // to whatever `- name:` opens the next one.
        let end = lines[at + 1..]
            .iter()
            .position(|line| line.trim_start().starts_with("- name:"))
            .map_or(lines.len(), |offset| at + 1 + offset);
        let step: String = lines[at..end].join("\n");
        for filter in run.filters {
            if !step.contains(filter) {
                problems.push(format!(
                    "CI_TEST_RUNS says the `{}` step filters on {filter:?}, and that step in \
                     .github/workflows/ci.yml does not pass it. Every test the filter was \
                     covering is now selected by nothing, and this table would keep calling them \
                     covered. Restore the filter, or drop it here and let the census say what \
                     that costs.",
                    run.job,
                ));
            }
        }
        if run.exact != step.contains("--exact") {
            problems.push(format!(
                "CI_TEST_RUNS says the `{}` step {} `--exact`, and the step says otherwise. \
                 `--exact` is the difference between a filter that covers a module and one that \
                 covers two names in it -- exactly how five tests here were compiled and never \
                 run.",
                run.job,
                if run.exact { "passes" } else { "does not pass" },
            ));
        }
    }
    // ...and the other direction: an invocation the table does not describe.
    // Counted rather than matched, because a `run:` block is shell and this
    // is not a shell parser -- but a NEW `cargo test` line that nobody
    // classified is exactly the drift that would make the census silently
    // narrower than the workflow.
    let invocations = text
        .lines()
        .map(str::trim_start)
        // Prose and step TITLES both name these commands constantly, and
        // neither runs anything.
        .filter(|line| !line.starts_with('#') && !line.starts_with("- name:"))
        .filter(|line| line.contains("cargo test "))
        .count();
    if invocations != CI_TEST_RUNS.len() {
        problems.push(format!(
            ".github/workflows/ci.yml contains {invocations} non-comment `cargo test ` \
             invocation(s) and CI_TEST_RUNS describes {}. Every one has to be classified, or the \
             census reasons about a workflow that is not the one running: an undescribed run \
             makes the coverage claim too NARROW (harmless but noisy), and a described run that \
             no longer exists makes it too WIDE, which hides a test that stopped running.",
            CI_TEST_RUNS.len(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(list: &[&str]) -> Build {
        Build {
            features: list.iter().map(|f| (*f).to_string()).collect(),
            debug_assertions: true,
        }
    }

    /// The predicate parser, over the four shapes this tree really uses.
    #[test]
    fn cfg_predicates_parse_and_evaluate() {
        let none = features(&[]);
        let drm = features(&["drm-backend"]);

        let p = parse_pred("feature = \"drm-backend\"");
        assert_eq!(p, Pred::Feature("drm-backend".into()));
        assert!(!holds(&p, &none).expect("known atom"));
        assert!(holds(&p, &drm).expect("known atom"));

        // `any(test, feature = "x")` is how this tree keeps a module
        // compilable under `cargo test` without the feature.
        let p = parse_pred("any(test, feature = \"consent-injector\")");
        assert!(
            holds(&p, &none).expect("known atom"),
            "`test` is true in every build this scan reasons about"
        );

        let p = parse_pred("not(feature = \"drm-backend\")");
        assert!(holds(&p, &none).expect("known atom"));
        assert!(!holds(&p, &drm).expect("known atom"));

        let p = parse_pred("all(feature = \"a\", feature = \"b\")");
        assert!(!holds(&p, &features(&["a"])).expect("known atom"));
        assert!(holds(&p, &features(&["a", "b"])).expect("known atom"));
    }

    /// An atom nobody taught this scan is an ERROR, never an assumed `true`.
    ///
    /// This is the whole reason `holds` returns a `Result`. Assuming `true`
    /// would report a test as covered by a job that never built it -- a green
    /// claim over absent evidence, in the code written against exactly that.
    #[test]
    fn an_unknown_cfg_atom_is_refused_rather_than_assumed() {
        let p = parse_pred("target_os = \"redox\"");
        assert!(matches!(p, Pred::Unknown(_)));
        let err = holds(&p, &features(&[])).expect_err("an unknown atom must not evaluate");
        assert!(format!("{err:#}").contains("refuses to guess"));
    }

    /// libtest's own selection rules, which decide whether a compiled test
    /// actually runs: substring by default, equality under `--exact`, and
    /// `#[ignore]`d tests excluded unless asked for.
    #[test]
    fn coverage_follows_libtests_own_selection_rules() {
        let item = |path: &str, ignored: bool| TestItem {
            package: "vitrin-core".into(),
            target: Target::MainBin,
            path: path.into(),
            file: "x.rs".into(),
            line: 1,
            cfg: vec![],
            ignored,
            ignored_if: vec![],
            body: String::new(),
        };
        let run = |filters: &'static [&'static str], exact: bool, ignored: bool| CiTestRun {
            job: "rust",
            package: Some("vitrin-core"),
            bin: Some("vitrind"),
            features: &[],
            no_default_features: false,
            filters,
            exact,
            ignored,
            debug: true,
            needle: "",
        };
        let plain = features(&[]);

        // Substring, which is what makes `vt::tests::` cover fourteen tests.
        assert!(covers(
            &run(&["vt::tests::"], false, false),
            &item("vt::tests::a", false),
            &plain
        )
        .unwrap());
        assert!(
            !covers(
                &run(&["vt::tests::"], false, false),
                &item("backend::a", false),
                &plain
            )
            .unwrap(),
            "a filter that does not match must not be read as coverage"
        );

        // `--exact` is equality, which is how the injector slice used to
        // exclude the five socket tests it compiled.
        assert!(
            !covers(
                &run(&["vt::tests::"], true, false),
                &item("vt::tests::a", false),
                &plain
            )
            .unwrap(),
            "under --exact a prefix is not a match"
        );
        assert!(covers(
            &run(&["vt::tests::a"], true, false),
            &item("vt::tests::a", false),
            &plain
        )
        .unwrap());

        // `#[ignore]` keeps a test out of every run that did not ask.
        assert!(!covers(&run(&[], false, false), &item("x", true), &plain).unwrap());
        assert!(covers(&run(&[], false, true), &item("x", true), &plain).unwrap());

        // `--bin` restricts to that target and no other in the package.
        let mut lib = item("x", false);
        lib.target = Target::Lib;
        assert!(!covers(&run(&[], false, false), &lib, &plain).unwrap());
    }

    /// `#[cfg_attr(<pred>, ignore)]` is a conditional `#[ignore]`, and the
    /// census reads it as one.
    ///
    /// The hole this closes: `#[cfg_attr(target_os = "linux", ignore)]` on a
    /// test in a tree that only ever builds Linux produces a test that runs
    /// in **no** CI job, while an `ignored: bool` read from the attribute list
    /// says `false` and every count reports the tree clean. It is the same
    /// failure as a `#[cfg(feature)]` no job enables, arriving by the one
    /// route the unconditional check could not see. Asserted in both
    /// directions -- a predicate that does NOT hold must not exclude the test,
    /// or this would just be the census giving up on `cfg_attr`.
    #[test]
    fn a_conditionally_ignored_test_is_not_counted_as_run() {
        let attr = format!("#[{}]", "test");
        let src = format!(
            "{attr}\n#[cfg_attr(feature = \"slow\", ignore)]\nfn a_test() {{ assert!(x); }}\n"
        );
        let (_, decls) = scan_source(&mask_code(&src), &src);
        assert_eq!(decls.len(), 1, "the parser must find the test");
        assert!(
            !decls[0].ignored,
            "it carries no UNCONDITIONAL #[ignore], and saying it does would be the \
             opposite error"
        );
        assert_eq!(
            decls[0].ignored_if,
            vec![Pred::All(vec![Pred::Feature("slow".into())])],
            "the condition the ignore rides on must be carried, not dropped"
        );
        assert!(
            cfgs(&["cfg_attr(feature = \"slow\", ignore)".to_string()]).is_empty(),
            "a cfg_attr decides which ATTRIBUTES apply, never whether the item exists; \
             reading it as a compilation gate would hide the test a second way"
        );

        // Nested, folded into a conjunction rather than lost one level down.
        assert_eq!(
            ignore_conditions(&["cfg_attr(test, cfg_attr(debug_assertions, ignore))".to_string()]),
            vec![Pred::All(vec![Pred::Test, Pred::DebugAssertions])],
        );
        // ...and a cfg_attr that applies something else is not an ignore.
        assert!(ignore_conditions(&["cfg_attr(test, should_panic)".to_string()]).is_empty());

        // Both directions through `covers`, which is where it decides a run.
        let mut item = TestItem {
            package: "vitrin-core".into(),
            target: Target::MainBin,
            path: "somewhere::tests::a_test".into(),
            file: "x.rs".into(),
            line: 1,
            cfg: vec![],
            ignored: false,
            ignored_if: vec![Pred::All(vec![Pred::Feature("slow".into())])],
            body: String::new(),
        };
        let run = CiTestRun {
            job: "rust",
            package: Some("vitrin-core"),
            bin: Some("vitrind"),
            features: &[],
            no_default_features: false,
            filters: &[],
            exact: false,
            ignored: false,
            debug: true,
            needle: "",
        };
        assert!(
            covers(&run, &item, &features(&[])).unwrap(),
            "the condition is false in this build, so the test really does run here"
        );
        assert!(
            !covers(&run, &item, &features(&["slow"])).unwrap(),
            "...and where the condition holds, libtest will not select it -- a run that \
             claimed otherwise would be counting a test nothing executed"
        );
        // The unconditional shape still behaves, so the two paths agree.
        item.ignored = true;
        assert!(!covers(&run, &item, &features(&[])).unwrap());
    }

    /// The published COUNT is checked, not just the published names.
    ///
    /// `docs/book/src/limits.md` opened its list with "Four `#[test]`
    /// functions in this repository run in no CI job at all" inside a
    /// paragraph claiming #288 had made that "a checked number rather than a
    /// sentence". Nothing read it. Both directions here: the sentence for the
    /// real count is in the real document, and the sentence for one more is
    /// not -- so the check cannot be passing because it matches anything.
    #[test]
    fn the_published_gap_count_is_a_sentence_the_document_really_carries() {
        let root = crate::workspace_root().expect("the workspace root");
        let gaps = UNRUN_TESTS
            .iter()
            .filter(|u| matches!(u.disposition, Disposition::PublishedGap { .. }))
            .count();
        let published: BTreeSet<&str> = UNRUN_TESTS
            .iter()
            .filter_map(|u| match u.disposition {
                Disposition::PublishedGap { published } => Some(published),
                Disposition::ReExecutedBy { .. } => None,
            })
            .collect();
        assert_eq!(
            published.len(),
            1,
            "this test assumes one publishing document; teach it the others: {published:?}"
        );
        let doc = published.iter().next().expect("a document");
        let text = squeeze(&std::fs::read_to_string(root.join(doc)).expect("the document"));
        assert!(
            text.contains(&published_count_sentence(gaps)),
            "{doc} does not carry \"{}\"",
            published_count_sentence(gaps)
        );
        assert!(
            !text.contains(&published_count_sentence(gaps + 1)),
            "{doc} carries the sentence for {} gaps as well, and only one of them can be true",
            gaps + 1
        );

        // The generator's own edges, since the whole check rests on it: one is
        // singular, and the words are not off by one.
        assert!(published_count_sentence(1).starts_with("One `#[test]` function in"));
        assert!(published_count_sentence(1).contains("runs in no CI job"));
        assert!(published_count_sentence(4).starts_with("Four `#[test]` functions in"));
        assert!(published_count_sentence(5).starts_with("Five `#[test]` functions in"));
    }

    /// A `ReExecutedBy` claim is held to a WHOLE name, not a prefix.
    ///
    /// The lever that produced this test: renaming the child to
    /// `..._RENAMED` left the old name as a prefix of the new one, so a plain
    /// `contains` still said the parent named it -- the check going green
    /// over a chain it no longer had, which is the one thing this module may
    /// not do.
    #[test]
    fn a_re_exec_claim_is_held_to_a_whole_name() {
        let body = r#"cmd.args(["--exact", "spawn::isolation::tests::probe", "--ignored"]);"#;
        assert!(names_whole(body, "spawn::isolation::tests::probe"));
        let renamed =
            r#"cmd.args(["--exact", "spawn::isolation::tests::probe_RENAMED", "--ignored"]);"#;
        assert!(
            !names_whole(renamed, "spawn::isolation::tests::probe"),
            "a prefix is not a name; a rename must break the claim"
        );
        assert!(
            !names_whole("tests::other::probe", "probe"),
            "a longer path ending in the same leaf is a different test"
        );
        assert!(names_whole("probe", "probe"), "...and equality still holds");
    }

    /// `--workspace` does not reach `fuzz/`, which is its own workspace --
    /// the reason the fuzz-smoke job runs a `cargo test` of its own.
    #[test]
    fn a_workspace_run_does_not_reach_the_fuzz_workspace() {
        let workspace = CiTestRun {
            job: "rust",
            package: None,
            bin: None,
            features: &[],
            no_default_features: false,
            filters: &[],
            exact: false,
            ignored: false,
            debug: true,
            needle: "",
        };
        let seed = TestItem {
            package: "vitrin-fuzz".into(),
            target: Target::Integration("seed_corpus_reachability".into()),
            path: "a_seed".into(),
            file: "fuzz/tests/seed_corpus_reachability.rs".into(),
            line: 1,
            cfg: vec![],
            ignored: false,
            ignored_if: vec![],
            body: String::new(),
        };
        assert!(
            !covers(&workspace, &seed, &features(&[])).unwrap(),
            "fuzz/ is not a member of the root workspace; claiming otherwise would hide four \
             tests"
        );
    }

    /// The whole check against the real tree: every `#[test]` is either run
    /// by a CI step or a published gap, and every published gap still exists
    /// and is still published.
    ///
    /// Run here as well as in `skip-scan` so a local `cargo test` catches a
    /// newly uncompiled test without anybody remembering the subcommand.
    #[test]
    fn every_test_in_the_tree_is_run_by_a_ci_step_or_a_published_gap() {
        let root = crate::workspace_root().expect("the workspace root");
        let mut problems = Vec::new();
        // The flat file list `skip_scan` would hand it, rebuilt here so this
        // test exercises the same cross-check the subcommand does.
        let files = crate::skip_census::files_with_tests(&root).expect("the flat scan");
        check_test_coverage(&root, &files, &mut problems).expect("the census runs");
        assert!(
            problems.is_empty(),
            "{} test-coverage problem(s):\n{}",
            problems.len(),
            problems.join("\n")
        );
    }

    /// NON-VACUITY: a synthetic test behind a feature no row enables is
    /// reported, and the same test with the feature enabled is not.
    ///
    /// Without this the check above could pass by finding nothing.
    #[test]
    fn a_test_behind_a_feature_no_job_builds_is_reported() {
        let root = crate::workspace_root().expect("the workspace root");
        let graph =
            feature_graph(&root.join("crates/vitrin-core/Cargo.toml")).expect("the manifest");
        let gated = TestItem {
            package: "vitrin-core".into(),
            target: Target::MainBin,
            path: "somewhere::tests::a_new_test".into(),
            file: "crates/vitrin-core/src/somewhere.rs".into(),
            line: 1,
            cfg: vec![Pred::Feature("a-feature-nobody-builds".into())],
            ignored: false,
            ignored_if: vec![],
            body: String::new(),
        };
        assert!(
            CI_TEST_RUNS
                .iter()
                .all(|run| !covers(run, &gated, &build_for(run, &graph)).expect("known atoms")),
            "no CI row may claim to run a test behind an unbuilt feature"
        );

        let mut ungated = gated.clone();
        ungated.cfg = vec![];
        assert!(
            CI_TEST_RUNS
                .iter()
                .any(|run| covers(run, &ungated, &build_for(run, &graph)).expect("known atoms")),
            "...and an ungated test must be covered, or this check would fire on everything"
        );

        // The feature CLOSURE is what makes the drm slice cover the keymap
        // tests it drags in, and the `--workspace` rows cover vitrin-ipc's
        // `client`-gated ones. Asserted, because getting it wrong in the
        // other direction would quietly widen every coverage claim here.
        let drm = CI_TEST_RUNS
            .iter()
            .find(|r| r.features.contains(&"drm-backend"))
            .expect("the drm row");
        assert!(
            build_for(drm, &graph).features.contains("session-keymap"),
            "drm-backend enables session-keymap in Cargo.toml, and the closure must know it"
        );
        let ipc = feature_graph(&root.join("crates/vitrin-ipc/Cargo.toml")).expect("the manifest");
        let workspace = &CI_TEST_RUNS[0];
        assert!(
            build_for(workspace, &ipc).features.contains("client"),
            "`cargo test --workspace` takes vitrin-ipc's default `server`, which enables `client`"
        );
    }
}
