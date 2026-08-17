// SPDX-License-Identifier: Apache-2.0
//! The skip census and the skip scan (issue #288): the two halves that make
//! "the job is green and the tests did not run" a red build rather than a
//! silent one -- as far as each of the three reaches, which is stated
//! per-part below and in full in `crates/vitrin-skip`'s module docs.
//!
//! # What went wrong, measured
//!
//! `cargo test` **captures stdout and stderr for tests that PASS**, printing
//! them only for failures. Every runtime skip in this repository announced
//! itself with `eprintln!`, so on a machine that could not confine, the
//! `rust` job printed `993 passed` while exercising none of the confinement
//! those tests cover. That was true of every `rust` job from the merge of
//! #186 until #287, and it surfaced only because a tenth test forgot its
//! guard and therefore FAILED loudly instead of skipping quietly.
//!
//! Two prior issues are the same failure class -- a green count standing in
//! for absent evidence. #229: `tests/integration/run.sh` exited 0 while
//! silently skipping the entire real-app ladder. #186: nine gates checked
//! mock-freeness with a `comm` prefix test that would have gone green for the
//! mock.
//!
//! # The three parts, and why no two of them suffice
//!
//! 1. [`vitrin_skip`] -- the sanctioned skip, and the **primary**
//!    mechanism. A capability probe returns an opaque `Verdict` that cannot
//!    be tested, matched, printed or destructured, so the only way to learn
//!    whether this machine is capable is to hand the verdict to
//!    `vitrin_skip::decide`, which prints the marker line and applies the
//!    require-variable first. Three silent-skip shapes that any source scan
//!    misses -- guard inverted around the whole body, guard moved inside the
//!    closure that is the measurement, guard hoisted into a helper -- stop
//!    compiling.
//!    *Its hole:* a test that re-implements a probe inline rather than
//!    calling one, and a bare `return;` inside a closure whose helper has
//!    not adopted the `Measured` token. Both are enumerated, with everything
//!    else this mechanism does and does not close, in `crates/vitrin-skip`'s
//!    own module documentation under "What this does NOT close".
//! 2. [`skip_scan`] -- the **second line of defence**, not the primary one.
//!    Walks every `#[test]` body in the tree and fails on an early `return`
//!    that did not go through the macro, against a short allowlist of the
//!    legitimate non-skip returns. It holds the [`INVENTORY`] to the sources
//!    in both directions, statically, so a newly sanctioned skip is red
//!    until somebody writes down why it is legitimate; it holds
//!    `.github/workflows/ci.yml` to [`CI_DECLARATIONS`] **per job**, so a
//!    typo in a require-variable -- or a declaration moved to a job where it
//!    enforces nothing, or one assigned as a shell prefix where it would
//!    cover a single command -- is a red scan rather than enforcement
//!    quietly off; and it runs `crates/xtask/src/test_census.rs`, which
//!    holds every `#[test]` in the tree to a CI step that compiles AND
//!    selects it -- where "every" means every one found syntactically in
//!    unexpanded source, which excludes `#[cfg_attr(<pred>, test)]` and
//!    models a test-generating `macro_rules!` as one template rather than as
//!    the tests it expands to. Both are in `vitrin_skip`'s canonical list.
//!    *Its hole:* it is a text scan over unexpanded source, so a `return`
//!    produced by a local `macro_rules!` is invisible to it; and its
//!    allowlist is an evasion path -- adding an entry is how a future author
//!    makes a red scan green, and only review distinguishes a legitimate
//!    watchdog closure from a laundered skip. The full bound is in
//!    `crates/vitrin-skip`'s "What this does NOT close".
//! 3. [`skip_census`] -- the reporting, and the runtime backstop. Runs a
//!    suite, filters the marker lines out of the passing tests' captured
//!    output, itemises them to stdout and `$GITHUB_STEP_SUMMARY`, and fails
//!    on a marker whose `(class, test)` pair is not in the [`INVENTORY`].
//!    It also refuses to make any affirmative claim over a run that executed
//!    fewer tests than that invocation's declared floor -- an affirmative
//!    over absent evidence being the exact defect this module exists to
//!    abolish, which it must therefore not commit itself. The floor is
//!    mandatory and may not be zero: `--min-tests 0` parses, reads like
//!    compliance and is satisfied by a run that executed nothing, so it is
//!    refused rather than accepted. What it counts as an executed test comes
//!    from libtest's own per-test lines **outside** any captured-output
//!    block, and the suite's stream is echoed to a sink the caller supplies:
//!    this module's own wrapper tests print fixture lines shaped exactly like
//!    libtest's, and echoing those to the real stdout put five of them into
//!    `cargo test -p xtask`'s top-level output, where the census wrapping
//!    that suite counted them as tests it had run. A census that pads its own
//!    denominator is this module's defect with the sign flipped.
//!    *Its hole:* it can only see sites that emit a marker, which is exactly
//!    what part 2 is for.
//!
//! # Why the inventory is a set of names and not a count
//!
//! An expected *count* rots into a rubber stamp within two pull requests:
//! the number is bumped by reflex and nothing records what was added. A set
//! of `(class, test)` pairs, each carrying a written justification, is a
//! diff line a reviewer sees. Bumping it is a visible decision. That is the
//! same reason `tests/integration/run.sh` itemises its skips rather than
//! counting them, and this module is the Rust side of that precedent.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::build_output::BuildOutput;

/// One skip this repository has decided is legitimate.
///
/// Every field is load-bearing. `class` and `test` are the key the runtime
/// census matches a marker line against and the key the static scan matches
/// a `vitrin_skip::skip_unless!` call site against -- both directions, so an entry
/// whose test was renamed or deleted is red, and a call site with no entry
/// is red. `why` is the part that stops this from being a rubber stamp: it
/// has to say what machine state the skip describes and why that state is an
/// honest limit rather than a misconfiguration.
struct Sanctioned {
    /// [`vitrin_skip::Class::name`].
    class: &'static str,
    /// The test's full runtime path, `<crate>::<module path>::<fn>` --
    /// exactly what the marker line carries, which is what libtest names the
    /// running thread.
    test: &'static str,
    /// Why a skip here is an honest machine state rather than a broken job.
    why: &'static str,
}

/// The skips this repository sanctions, and the reason each one is not a
/// misconfiguration.
///
/// **Adding a line here is the visible decision.** A new sanctioned skip
/// fails `cargo xtask skip-scan` until it appears, and the entry must say
/// which machine state it describes -- not "this can skip", which is what
/// the code already said.
///
/// The count is deliberately not written down anywhere: it is derived from
/// this table by both directions of the scan, so there is no number for
/// anybody to transcribe and none to drift. (Issue #288 says ten
/// namespace guards; the tree has nine. That is exactly the hazard.)
const INVENTORY: &[Sanctioned] = &[
    // ---- confinement: the nine real confined-realm spawns ----------------
    //
    // Every one of these forks `vitrin-realm-init` into six real namespaces.
    // A kernel that refuses them cannot run the measurement at all, and no
    // amount of test-side effort changes that -- but CI's runner CAN, after
    // taking the remedy `vitrind`'s own preflight prints, so the `rust` job
    // sets `VITRIN_REQUIRE_CONFINEMENT=1` and a skip there is a failure.
    //
    // The `rust` job is the ONLY one that sets it. The `integration` job
    // takes the same sysctl remedy, but it runs no Rust unit test -- it
    // builds `vitrind` and drives the Python suite -- so a require-variable
    // there would enforce nothing, and an earlier draft of this comment
    // claiming it set one was simply false. `CI_DECLARATIONS` below is the
    // checked fact; this sentence is a restatement of it.
    //
    // The skip that remains is for a developer machine with unprivileged
    // user namespaces switched off.
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_confined_realm_cannot_reach_the_canary_and_the_core_proves_it_from_outside",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_confined_spawn_journals_the_split_between_what_it_read_and_what_it_was_told",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_capped_session_enforces_the_rung_it_asked_for_and_journals_both_numbers",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::the_confined_shim_holds_no_authority_it_was_not_handed",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::killing_the_supervisor_takes_the_realm_down_with_it",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_helper_that_unshares_but_mounts_nothing_is_refused",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_canary_the_realm_really_can_reach_is_refused",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_stub_the_mount_table_had_to_create_is_not_a_reachable_canary",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    Sanctioned {
        class: "confinement",
        test: "vitrind::spawn::tests::a_leaked_directory_descriptor_does_not_survive_the_second_execve",
        why: "the kernel refuses the six namespaces a confined realm needs (ns.all=false)",
    },
    // ---- host-tooling: three host-layout preconditions --------------------
    //
    // All three are required in CI (`VITRIN_REQUIRE_HOST_TOOLING=1`), because
    // each would otherwise fire after an unrelated change with no other
    // symptom: a slimmer container image without /usr/bin/env, a
    // CARGO_TARGET_DIR move that leaves the build directory with no bindable
    // ancestor, or a container that runs the suite as root.
    Sanctioned {
        class: "host-tooling",
        test: "vitrind::spawn::tests::a_canary_the_realm_really_can_reach_is_refused",
        why: "this host has no /usr/bin/env, so there is no canary the realm really can reach",
    },
    Sanctioned {
        class: "host-tooling",
        test: "vitrind::spawn::tests::a_stub_the_mount_table_had_to_create_is_not_a_reachable_canary",
        why: "the build directory has no bindable ancestor outside /usr and /etc, so the mount \
              table has no stub to create",
    },
    Sanctioned {
        class: "host-tooling",
        test: "vitrind::backlight::tests::a_read_only_brightness_file_is_not_writable",
        why: "this uid can open a mode-444 file for writing -- it is root, or holds \
              CAP_DAC_OVERRIDE -- so `Outcome::NotWritable` is unreachable and the \
              permission half of D-041's failure collapse cannot be observed on this \
              machine. An honest limit rather than a broken job: GitHub's runners are \
              ordinary users, so the class is REQUIRED in CI and a container that ran the \
              suite as root goes red here instead of green-having-measured-nothing",
    },
    // ---- landlock-abi: six rung measurements -----------------------------
    //
    // A ladder, so the require-variable is a FLOOR rather than a flag: CI
    // declares the ABI its runner was measured at and every rung at or below
    // that must run. `docs/book/src/limits.md` records the measurement the
    // number comes from.
    Sanctioned {
        class: "landlock-abi",
        test: "vitrind::spawn::tests::a_capped_session_enforces_the_rung_it_asked_for_and_journals_both_numbers",
        why: "below ABI 2 the cap to rung 2 could not have weakened anything, so the assertion \
              would be vacuous rather than wrong",
    },
    Sanctioned {
        class: "landlock-abi",
        test: "vitrin_realm_init::tests::rung_one_forbids_reparenting_that_the_rung_above_permits",
        why: "the REFER right this contrasts against does not exist below ABI 2",
    },
    Sanctioned {
        class: "landlock-abi",
        test: "vitrin_realm_init::tests::a_bind_naming_a_file_gets_rights_the_kernel_accepts",
        why: "no ruleset can be built at all below ABI 1",
    },
    Sanctioned {
        class: "landlock-abi",
        test: "vitrin_realm_init::tests::the_audit_log_flag_is_off_unless_asked_for_and_the_kernel_takes_it",
        why: "the LANDLOCK_RESTRICT_SELF_LOG_* flags arrived at ABI 7; this is the highest rung \
              the build requires and the one a runner-image change would take away first",
    },
    Sanctioned {
        class: "landlock-abi",
        test: "vitrin_realm_init::tests::a_realm_can_write_where_it_was_granted_and_nowhere_else",
        why: "no ruleset can be built at all below ABI 1",
    },
    Sanctioned {
        class: "landlock-abi",
        test: "vitrin_realm_init::tests::the_truncate_rung_is_measured_and_its_absence_is_measured_with_it",
        why: "the TRUNCATE right this measures arrived at ABI 3",
    },
    // ---- c-shim: the two cross-track conformance checks -------------------
    //
    // `Require::UnderCiUnlessDeclared`: required in every CI job that does
    // not declare `VITRIN_C_SHIM_CONFORMANCE_SKIP=1`. The `rust` job has no C
    // toolchain and declares it; the `conformance` job builds the shim and is
    // required without saying anything.
    Sanctioned {
        class: "c-shim",
        test: "vitrind::shim::tests::c_shim_conforms_to_the_real_core",
        why: "no C shim was built here (VITRIN_C_SHIM_BIN unset); the `conformance` job is where \
              this is satisfied for real",
    },
    Sanctioned {
        class: "c-shim",
        test: "vitrind::backend::headless::tests::c_shim_consent_prompt_occludes_the_human_visible_output_but_never_the_real_apps_capture",
        why: "no C shim was built here (VITRIN_C_SHIM_BIN unset); the `conformance` job's \
              `c_shim` filter matches this test by name and satisfies it for real",
    },
    // ---- gpu: the three real-GPU dmabuf acceptances -----------------------
    //
    // No CI job runs these -- they are `#[ignore]`d, behind the `gpu-tests`
    // feature, behind VITRIN_GPU_TESTS=1, and no runner has a GPU. They are
    // inventoried anyway so a developer's GPU box gets an itemised census of
    // which half its driver actually reached, and so the scan can hold the
    // call sites to this table like every other class.
    Sanctioned {
        class: "gpu",
        test: "vitrind::dmabuf::gpu_tests::real_gpu_dmabuf_frames_are_zero_copy_end_to_end",
        why: "no real GPU here (VITRIN_GPU_TESTS unset), or this renderer does not import \
              XRGB8888+LINEAR dmabufs -- a per-GPU reality (plan risk R3)",
    },
    Sanctioned {
        class: "gpu",
        test: "vitrind::dmabuf::gpu_tests::real_gpu_probe_accepts_dmabuf_and_kills_memfd_lie",
        why: "no real GPU here (VITRIN_GPU_TESTS unset)",
    },
    Sanctioned {
        class: "gpu",
        test: "vitrind::dmabuf::gpu_tests::real_gpu_oversized_dmabuf_center_crops_the_full_view",
        why: "no real GPU here (VITRIN_GPU_TESTS unset), or this renderer does not import \
              XRGB8888+LINEAR dmabufs -- a per-GPU reality (plan risk R3)",
    },
];

/// A `return` at a `#[test]`'s own level that is **not** a skip.
///
/// This is the scan's rot surface, and it is kept to exactly one entry
/// because [`body_level_returns`] does the work an allowlist would
/// otherwise have to: a return inside a closure or a nested `fn` leaves the
/// closure, not the test, and is not counted at all. (The audit behind #288
/// predicted four entries here -- three of the four are closure returns the
/// classifier now excludes on its own, and it found eight such test bodies
/// rather than four.)
///
/// The entry names the exact number of returns permitted, so growing a
/// laundered skip inside an already-allowlisted test is still red. Adding an
/// entry is how a future author would make a red scan green; nothing
/// distinguishes a legitimate one from a laundered skip except review, which
/// is the same cost this repository already accepts explicitly for
/// `crates/xtask/src/limits.rs`.
struct AllowedReturn {
    /// Repository-relative path.
    file: &'static str,
    /// The `#[test]` function's own name.
    test: &'static str,
    /// How many `return`s that body may contain.
    count: usize,
    /// What they are, and why none of them ends the test early.
    why: &'static str,
}

/// The one non-skip return at a test's own level in this tree, enumerated.
const ALLOWED_RETURNS: &[AllowedReturn] = &[AllowedReturn {
    file: "crates/vitrin-core/src/spawn.rs",
    test: "an_ignored_signal_in_the_cores_launch_context_never_reaches_the_child",
    count: 1,
    why: "the INNER arm of a test that re-execs itself under a shell holding SIGINT/SIGQUIT/\
          SIGTERM at SIG_IGN. The inner run does the whole measurement and returns before the \
          outer arm's re-exec; skipping nothing, asserting everything",
}];

/// A source file, masked so that every byte inside a comment, a string
/// literal or a character literal is a space.
///
/// This is what makes the scan's brace matching and its whole-word search
/// correct rather than approximate: `format!("{}", x)` must not open a
/// block, and the word `return` inside a doc comment or a panic message must
/// not look like control flow. Rust's lifetime syntax shares the `'` with
/// character literals, so the mask resolves that the way the grammar does --
/// a `'` is a literal only when the token that follows closes as one.
///
/// **The result has the same byte length as its input**, and every newline
/// sits at the same offset. That is not decoration: `crates/xtask/src/
/// test_census.rs` reads an attribute's *body* out of the ORIGINAL source at
/// offsets this scan found in the masked one -- it has to, because the mask
/// blanks the `"drm-backend"` inside `#[cfg(feature = "drm-backend")]` -- and
/// a mask that shortened a multi-byte character would silently hand it the
/// wrong slice. [`the_mask_is_byte_for_byte_the_same_length`] holds it.
pub(crate) fn mask_code(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = vec![' '; b.len()];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // Preserve newlines everywhere so line numbers survive the mask.
        if c == '\n' {
            out[i] = '\n';
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            let mut depth = 1usize;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '\n' {
                    out[i] = '\n';
                }
                if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && i + 1 < b.len() && b[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Raw strings: r"..", r#".."#, br#".."#, and any number of hashes.
        if c == 'r' || (c == 'b' && i + 1 < b.len() && b[i + 1] == 'r') {
            let mut j = if c == 'b' { i + 2 } else { i + 1 };
            let hash_start = j;
            while j < b.len() && b[j] == '#' {
                j += 1;
            }
            let hashes = j - hash_start;
            if j < b.len() && b[j] == '"' {
                j += 1;
                'raw: while j < b.len() {
                    if b[j] == '\n' {
                        out[j] = '\n';
                    }
                    if b[j] == '"' {
                        let mut k = j + 1;
                        let mut seen = 0;
                        while k < b.len() && b[k] == '#' && seen < hashes {
                            k += 1;
                            seen += 1;
                        }
                        if seen == hashes {
                            j = k;
                            break 'raw;
                        }
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        if c == '"' {
            i += 1;
            while i < b.len() {
                if b[i] == '\n' {
                    out[i] = '\n';
                }
                if b[i] == '\\' {
                    // The escaped character is skipped WITHOUT being masked,
                    // so it has to be checked for a newline here or the line
                    // is lost. `"...\` + newline is Rust's string
                    // continuation and this file is full of it: before this
                    // branch preserved it, `skip-scan`'s reported line
                    // numbers in crates/vitrin-core/src/spawn.rs were 118
                    // lines short, because 133 continuations there each ate
                    // one newline. [`the_mask_preserves_every_line`] holds
                    // the invariant, and [`skip_scan`] re-checks it on every
                    // file it reads.
                    if i + 1 < b.len() && b[i + 1] == '\n' {
                        out[i + 1] = '\n';
                    }
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '\'' {
            // `'a` is a lifetime; `'x'` and `'\n'` are literals. Decide by
            // looking for the closing quote the way the grammar does.
            let is_literal = if i + 1 < b.len() && b[i + 1] == '\\' {
                true
            } else {
                i + 2 < b.len() && b[i + 2] == '\''
            };
            if is_literal {
                i += 1;
                while i < b.len() {
                    if b[i] == '\n' {
                        out[i] = '\n';
                    }
                    if b[i] == '\\' {
                        // Same rule as the string branch above, for the same
                        // reason: an escaped byte is skipped rather than
                        // masked, so a newline hiding behind the backslash
                        // has to be put back by hand.
                        if i + 1 < b.len() && b[i + 1] == '\n' {
                            out[i + 1] = '\n';
                        }
                        i += 2;
                        continue;
                    }
                    if b[i] == '\'' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }
        out[i] = c;
        i += 1;
    }
    // Re-assembled byte-for-byte rather than char-for-char: a masked
    // multi-byte character becomes as many spaces as it had bytes, so every
    // offset into the mask is the same offset into the source.
    let mut masked = String::with_capacity(src.len());
    for (i, &c) in b.iter().enumerate() {
        if out[i] == c {
            masked.push(c);
        } else {
            for _ in 0..c.len_utf8() {
                masked.push(' ');
            }
        }
    }
    masked
}

/// Whether `c` can continue a Rust identifier -- the boundary test for
/// whole-word matching, the same rule `enforcement.rs`'s one-path scan uses.
pub(crate) fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Byte offsets of every whole-word occurrence of `needle` in `masked`.
pub(crate) fn word_positions(masked: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = masked[from..].find(needle) {
        let at = from + found;
        let end = at + needle.len();
        let before = masked[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident(c));
        let after = masked[end..].chars().next().is_none_or(|c| !is_ident(c));
        if before && after {
            out.push(at);
        }
        from = at + 1;
    }
    out
}

/// Whether `header` -- the masked code between the previous statement
/// boundary and a `{` -- opens a **closure body** rather than a plain block.
///
/// The rule this replaces counted `|` characters and called two or more of
/// them a closure. That is wrong in the one direction that matters: `if
/// matches!(x, A | B | C) { ...; return; }` is entirely idiomatic Rust with
/// three pipes and no closure, so the whole block became opaque and a bare
/// `return` inside it was invisible to the scan. A skip written that way
/// evaded the check this module exists to be.
///
/// So the question asked is not "how many pipes" but "**can a `|` here open
/// a closure whose body is THIS brace**". Two conditions, and the second one
/// is the one a `matches!` fix alone still got wrong:
///
/// 1. The `|` must sit in *expression* (prefix) position -- the token before
///    it is a delimiter or an operator (`(`, `,`, `{`, `[`, `=`, `=>`, `;`,
///    `:`), or the word `move`, or nothing at all ([`can_open_a_closure`]).
///    In `matches!(x, A | B | C)` and in `a || b` every `|` is preceded by a
///    value, so none of them can open a closure.
/// 2. The `|` must sit at the **same parenthesis/bracket depth as the brace**.
///    A closure's body brace is inside exactly the delimiters its parameter
///    list is inside -- `.map(|s| { .. })` has both at depth 1, `let f = |x|
///    { .. }` has both at depth 0. A `|` that is *deeper* than the brace
///    belongs to a closure that was closed again inside the header, and the
///    brace is somebody else's.
///
/// Condition 2 is what closes the most ordinary evasion in this repository's
/// language: `if xs.iter().any(|m| m.matches()) { return; }`. The pipes are
/// in prefix position (after `(` and after a parameter name), so condition 1
/// alone reads the `if`'s block as a closure body, makes it opaque, and the
/// bare `return` -- a real early return from the test -- becomes invisible.
/// Any `if`, `while` or `match` whose condition calls a closure-taking method
/// has that shape. Measured: the five variants in
/// [`a_pipe_heavy_header_that_is_not_a_closure_still_exposes_its_return`] were
/// all invisible under condition 1 alone, the depth rule catches all five, and
/// it changed no verdict on any source in this tree -- `skip-scan` stayed
/// green over 1282 test bodies with [`ALLOWED_RETURNS`] untouched, which is
/// the claim that matters in the other direction.
///
/// [`a_pipe_heavy_header_that_is_not_a_closure_still_exposes_its_return`]:
///     tests::a_pipe_heavy_header_that_is_not_a_closure_still_exposes_its_return
///
/// This deliberately keeps the shape the old rule got right: the param list
/// need not sit at the *end* of the header, because `insert_source(src, |a,
/// b| match a { .. })` opens its brace on the `match` and a `return` there
/// still leaves only the closure -- its `|` and its brace are both at depth 1.
fn opens_a_closure(header: &str) -> bool {
    // Where the brace sits. The header runs from the previous statement
    // boundary up to the `{`, so its closing depth IS the brace's.
    let brace_depth = delimiter_depth(header);
    let mut depth = 0isize;
    let mut chars = header.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '|' if depth == brace_depth && can_open_a_closure(&header[..at]) => {
                // `||` is an empty parameter list, closed the moment it opens.
                if chars.peek().is_some_and(|&(_, next)| next == '|') {
                    return true;
                }
                // ...otherwise the list has to be closed by a second `|` at
                // the same depth, still inside this header.
                if closes_a_parameter_list(&header[at + c.len_utf8()..]) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// The parenthesis/bracket nesting `text` leaves open, which for a block's
/// header is the depth its `{` sits at. Negative when the header begins
/// inside delimiters opened before the previous statement boundary; only the
/// comparison matters, and both sides are measured from the same origin.
fn delimiter_depth(text: &str) -> isize {
    let mut depth = 0isize;
    for c in text.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Whether `rest` closes a closure's parameter list: a `|` at the depth the
/// opening one was at, before any enclosing delimiter closes.
fn closes_a_parameter_list(rest: &str) -> bool {
    let mut depth = 0isize;
    for c in rest.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '|' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Whether a `|` immediately after `prefix` could be a closure's, rather than
/// a `matches!` pattern alternative, a bitwise or, or the second half of `||`.
fn can_open_a_closure(prefix: &str) -> bool {
    let prefix = prefix.trim_end();
    match prefix.chars().next_back() {
        // Nothing before it at all: the header IS the closure.
        None => true,
        // Delimiters and operators -- every position a Rust expression may
        // start in. `>` covers the `=>` of a match arm.
        Some('(' | ',' | '{' | '[' | '=' | '>' | ';' | ':') => true,
        // `move |x|` / `move ||`, the one word that can precede a closure.
        Some(c) if is_ident(c) => prefix
            .rsplit(|c: char| !is_ident(c))
            .next()
            .is_some_and(|word| word == "move"),
        _ => false,
    }
}

/// How many `return`s in `body` return from the **test function itself**.
///
/// The distinction is the whole difference between a rule that finds skips
/// and a rule that finds Rust. A `return` inside a closure -- a watchdog
/// thread, an event-loop callback, the child half of a `fork_and_measure`
/// probe -- leaves the closure, not the test, and this repository is full of
/// them (eight test bodies, where the audit behind #288 predicted four). A
/// `return` at the test's own level is the shape a skip takes, and only
/// those are counted.
///
/// Frames are pushed for every `{` and classified from the code that opens
/// them: a closure ([`opens_a_closure`]) or a nested `fn` item is **opaque**;
/// everything else -- a plain block, a `match` arm, an `if`, a struct literal
/// -- is transparent, because a `return` inside one really does leave the
/// test. Getting that classification wrong in the permissive direction is how
/// a skip hides, which is why [`opens_a_closure`] asks whether a `|` can open
/// a closure rather than counting pipes.
fn body_level_returns(body: &str) -> usize {
    let mut opaque_depth = 0usize;
    let mut stack: Vec<bool> = Vec::new();
    let returns: BTreeSet<usize> = word_positions(body, "return").into_iter().collect();
    let mut count = 0usize;
    for (at, c) in body.char_indices() {
        if returns.contains(&at) && opaque_depth == 0 {
            count += 1;
        }
        match c {
            '{' => {
                let cut = ["; ", "{", "}", "=>"]
                    .iter()
                    .filter_map(|d| {
                        body[..at]
                            .rfind(d.trim_end())
                            .map(|i| i + d.trim_end().len())
                    })
                    .max()
                    .unwrap_or(0);
                let header = body[cut..at].trim();
                let opaque =
                    header.starts_with("fn ") || header.contains(" fn ") || opens_a_closure(header);
                stack.push(opaque);
                if opaque {
                    opaque_depth += 1;
                }
            }
            '}' => {
                if let Some(true) = stack.pop() {
                    opaque_depth = opaque_depth.saturating_sub(1);
                }
            }
            _ => {}
        }
    }
    count
}

/// One `#[test]` function found by the scan.
struct TestFn {
    /// Repository-relative path of the file it lives in.
    file: String,
    /// The function's own name.
    name: String,
    /// 1-indexed line the `fn` sits on.
    line: usize,
    /// The masked body, braces included.
    body: String,
}

/// Every `#[test]` function in `text`, with its body delimited by brace
/// matching over the masked source.
fn test_fns(file: &str, text: &str) -> Vec<TestFn> {
    let masked = mask_code(text);
    let mut out = Vec::new();
    for at in word_positions(&masked, "fn") {
        // Walk backwards over the whole attribute stack above this `fn` and
        // ask whether ANY of it is the test attribute. Walking only to the
        // NEAREST `#` would miss `#[test]` followed by `#[ignore = "..."]`
        // -- which is exactly how the three real-GPU dmabuf acceptances are
        // written, so the first version of this scan silently covered none
        // of them.
        let mut before = masked[..at].trim_end();
        let mut is_test = false;
        while before.ends_with(']') {
            let mut depth = 0usize;
            let mut open = None;
            for (offset, c) in before.char_indices().rev() {
                match c {
                    ']' => depth += 1,
                    '[' => {
                        depth -= 1;
                        if depth == 0 {
                            open = Some(offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(open) = open else { break };
            let head = before[..open].trim_end();
            // `#[..]` outer, `#![..]` inner; either way a `#` must precede.
            let head = head.strip_suffix('!').unwrap_or(head);
            let Some(head) = head.strip_suffix('#') else {
                break;
            };
            if before[open + 1..before.len() - 1].trim() == "test" {
                is_test = true;
            }
            before = head.trim_end();
        }
        if !is_test {
            continue;
        }
        let rest = &masked[at + 2..];
        let name_start = at + 2 + (rest.len() - rest.trim_start().len());
        let name_end = name_start
            + masked[name_start..]
                .find(|c: char| !is_ident(c))
                .unwrap_or(0);
        let name = masked[name_start..name_end].to_string();
        let Some(open) = masked[name_end..].find('{').map(|o| name_end + o) else {
            continue;
        };
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
        out.push(TestFn {
            file: file.to_string(),
            name,
            line: masked[..at].matches('\n').count() + 1,
            body: masked[open..=close].to_string(),
        });
    }
    out
}

/// Every attribute body immediately above the item at `at`, innermost first.
///
/// Walks back over the attribute stack **and** over the qualifiers that may
/// sit between it and the item -- `pub`, `pub(crate)`, `pub(super)`, `async`,
/// `unsafe`, `const`, `extern "C"` -- because `#[cfg(feature = "drm-backend")]
/// pub mod drm;` is as common in this tree as `#[test] fn x()`, and a walk
/// that stopped at the visibility keyword would read that module as ungated.
///
/// The *structure* is found in `masked`, so a `#[` inside a string or a doc
/// comment cannot be mistaken for an attribute; each body is then returned
/// from `src`, because the mask blanks string literals and
/// `#[cfg(feature = "drm-backend")]` read off the mask says `feature = ""`.
/// That is safe exactly because [`mask_code`] preserves byte length.
pub(crate) fn attrs_before(masked: &str, src: &str, at: usize) -> Vec<String> {
    debug_assert_eq!(masked.len(), src.len(), "mask_code preserves byte length");
    let mut out = Vec::new();
    let mut before = masked[..at].trim_end();
    loop {
        if before.ends_with(']') {
            let Some(open) = matching_open(before, '[', ']') else {
                break;
            };
            let head = before[..open].trim_end();
            // `#[..]` outer, `#![..]` inner; either way a `#` must precede.
            let head = head.strip_suffix('!').unwrap_or(head);
            let Some(head) = head.strip_suffix('#') else {
                break;
            };
            out.push(src[open + 1..before.len() - 1].trim().to_string());
            before = head.trim_end();
            continue;
        }
        if before.ends_with(')') {
            // The only `(..)` that may sit between an attribute and an item
            // is a restricted visibility.
            let Some(open) = matching_open(before, '(', ')') else {
                break;
            };
            let head = before[..open].trim_end();
            if last_word(head) == "pub" {
                before = head[..head.len() - 3].trim_end();
                continue;
            }
            break;
        }
        match last_word(before) {
            "pub" | "async" | "unsafe" | "const" | "default" | "extern" => {
                let word = last_word(before);
                before = before[..before.len() - word.len()].trim_end();
            }
            _ => break,
        }
    }
    out
}

/// The byte offset of the `open` delimiter matching the `close` that `text`
/// ends with.
fn matching_open(text: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in text.char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some(offset);
            }
        }
    }
    None
}

/// The trailing identifier of `text`, or `""`.
fn last_word(text: &str) -> &str {
    let start = text.rfind(|c: char| !is_ident(c)).map_or(0, |i| i + 1);
    &text[start..]
}

/// Every `.rs` file under `dir`, recursively, except build output.
///
/// This skipped one directory NAME -- `target` -- until #295. That is the
/// right rule stated in the wrong units: `fuzz/target/` is skipped because
/// `.gitignore` says `**/target`, not because of the seven letters, and a
/// meson build directory under a scanned root is named by whoever ran `meson
/// setup`. Asking git covers both, and covers them with the same answer CI's
/// checkout gives -- so a `#[test]` in a directory that will never exist in
/// CI can no longer be demanded of a CI step.
pub(crate) fn rust_sources(
    dir: &Path,
    root: &Path,
    built: &BuildOutput,
    out: &mut Vec<(String, String)>,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::result::Result<_, _>>()?;
    entries.sort();
    for path in entries {
        if built.covers(&path) {
            continue;
        }
        if path.is_dir() {
            rust_sources(&path, root, built, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            out.push((rel, text));
        }
    }
    Ok(())
}

/// The directories the scan covers. Everything in the repository that holds
/// `#[test]` functions -- `fuzz/` is its own workspace and is included for
/// the same reason the audit that produced this module scanned it: a scan
/// that covers "most of" the tree cannot state a negative result.
const SCANNED: &[&str] = &["crates", "fuzz"];

/// `cargo xtask skip-scan`: the static half. Reads sources, writes nothing.
/// Every repository-relative `.rs` file under [`SCANNED`] that defines at
/// least one `#[test]`, found by reading files rather than by walking module
/// declarations.
///
/// `crates/xtask/src/test_census.rs` walks the module tree instead, because
/// that is the only way to know which cargo target compiles a file -- and a
/// file its walk cannot reach would be covered by silence. This flat list is
/// what it is checked against.
///
/// `skip_scan` builds the same set inline from sources it has already read;
/// this exists so `test_census`'s own test can exercise the identical
/// cross-check without the subcommand.
#[cfg(test)]
pub(crate) fn files_with_tests(root: &Path) -> Result<BTreeSet<String>> {
    let built = BuildOutput::of_tree(root)?;
    let mut sources = Vec::new();
    for dir in SCANNED {
        rust_sources(&root.join(dir), root, &built, &mut sources)?;
    }
    let mut out = BTreeSet::new();
    for (file, text) in &sources {
        if !test_fns(file, text).is_empty() {
            out.insert(file.clone());
        }
    }
    Ok(out)
}

pub fn skip_scan(root: &Path) -> Result<()> {
    // Once per scan; `check_test_coverage` below reads the same tree and asks
    // git the same question, and the two answers have to agree or the
    // cross-check between them reports a file rather than a defect.
    let built = BuildOutput::of_tree(root)?;
    let mut sources = Vec::new();
    for dir in SCANNED {
        rust_sources(&root.join(dir), root, &built, &mut sources)?;
    }
    // A scan that found no sources would pass silently, which is the exact
    // failure this module exists to forbid.
    anyhow::ensure!(
        sources.len() > 50,
        "the scan found only {} Rust sources under {SCANNED:?} -- it is not reading the tree",
        sources.len()
    );

    let mut tests: Vec<TestFn> = Vec::new();
    for (file, text) in &sources {
        // Every line number this scan prints comes from counting newlines in
        // the MASKED text, so a mask that loses one silently misreports every
        // problem below it -- which it did, by 118 lines in one file, until
        // the string branch of `mask_code` learned about `\` + newline. The
        // invariant is cheap enough to re-check per file, so the next such
        // bug is a red scan rather than a wrong line number nobody diffs.
        anyhow::ensure!(
            mask_code(text).matches('\n').count() == text.matches('\n').count(),
            "the mask lost a line in {file}: every line number this scan reports would be \
             wrong. See mask_code in crates/xtask/src/skip_census.rs."
        );
        tests.extend(test_fns(file, text));
    }
    anyhow::ensure!(
        tests.len() > 100,
        "the scan found only {} #[test] functions -- it is not parsing the tree",
        tests.len()
    );

    let mut problems: Vec<String> = Vec::new();
    check_ci_declarations(root, &mut problems)?;
    // The compile-time half: a `#[test]` no CI step builds, or that every
    // step's name filter excludes, emits no output at all, so neither the
    // marker scan above nor the runtime census can see it. See
    // crates/xtask/src/test_census.rs.
    //
    // It is handed THIS scan's file list on purpose. It walks the module tree
    // from each cargo target's root (the only way to know which target
    // compiles a file); this scan reads every `.rs` file flatly. A file the
    // walk cannot reach would otherwise be covered by silence, and the two
    // lists disagreeing is what makes that a red build instead.
    let files_with_tests: BTreeSet<String> = tests.iter().map(|t| t.file.clone()).collect();
    crate::test_census::check_test_coverage(root, &files_with_tests, &mut problems)?;

    // (1) No bare early return, and no `exit`, in a test body.
    for test in &tests {
        let returns = body_level_returns(&test.body);
        let allowed = ALLOWED_RETURNS
            .iter()
            .find(|a| a.file == test.file && a.test == test.name);
        let budget = allowed.map_or(0, |a| a.count);
        if returns > budget {
            let extra = match allowed {
                Some(a) => format!(
                    "\n      the allowlist permits {} here ({}), so {} of them are new",
                    a.count,
                    a.why,
                    returns - a.count
                ),
                None => String::new(),
            };
            problems.push(format!(
                "{}:{} `{}` contains {returns} bare `return`(s) in a #[test] body.\n      A skip \
                 must go through `vitrin_skip::skip_unless!`, which carries the return AND \
                 prints the marker line the census reads. If these are not skips, add them to \
                 ALLOWED_RETURNS in crates/xtask/src/skip_census.rs with a reason.{extra}",
                test.file, test.line, test.name,
            ));
        }
        // `std::process::exit(0)` is the one evasion of the return rule
        // cheap enough to close here: it ends the whole test binary, which
        // libtest reports as a pass for every test that had already run.
        // `libc::_exit` is deliberately NOT matched -- it is how this
        // repository's forked children legitimately leave, and conflating
        // the two would turn a real pattern into an allowlist entry.
        if test.body.contains("process::exit") {
            problems.push(format!(
                "{}:{} `{}` calls process::exit inside a #[test] body. That ends the test \
                 binary, and libtest reports every test that already ran as a pass; use \
                 `vitrin_skip::skip_unless!`.",
                test.file, test.line, test.name,
            ));
        }
    }

    // (2) Every sanctioned skip call site is inventoried, and every
    //     inventory entry has a call site. Both directions, statically.
    let found = call_sites(&tests);
    let inventoried: BTreeSet<(String, String)> = INVENTORY
        .iter()
        .map(|s| {
            let leaf = s.test.rsplit("::").next().unwrap_or(s.test);
            (s.class.to_string(), leaf.to_string())
        })
        .collect();
    for (class, leaf, file, line) in &found {
        if !inventoried.contains(&(class.clone(), leaf.clone())) {
            problems.push(format!(
                "{file}:{line} `{leaf}` skips for class `{class}` and is NOT in the INVENTORY in \
                 crates/xtask/src/skip_census.rs. Add it, saying which machine state it \
                 describes and why that state is an honest limit rather than a broken job.",
            ));
        }
    }
    let found_keys: BTreeSet<(String, String)> = found
        .iter()
        .map(|(c, l, _, _)| (c.clone(), l.clone()))
        .collect();
    for key in inventoried.difference(&found_keys) {
        problems.push(format!(
            "the INVENTORY has an entry for class `{}` on test `{}`, and no test by that name \
             skips for that class any more. Delete the entry -- a stale inventory is how this \
             table becomes the rubber stamp it exists to replace.",
            key.0, key.1,
        ));
    }

    if !problems.is_empty() {
        for p in &problems {
            eprintln!("xtask skip-scan: {p}");
        }
        bail!(
            "{} skip-scan problem(s); see above. Issue #288: a skip that CI cannot see is a \
             green job that measured nothing.",
            problems.len()
        );
    }

    println!(
        "xtask skip-scan: {} #[test] bodies in {} files; {} sanctioned skip site(s), all \
         inventoried; {} allowlisted non-skip return(s); {} CI declaration(s) checked.",
        tests.len(),
        sources.len(),
        found.len(),
        ALLOWED_RETURNS.iter().map(|a| a.count).sum::<usize>(),
        CI_DECLARATIONS.len(),
    );
    // Said out loud rather than left implied. This scan is the SECOND line of
    // defence since the probes were sealed, and a reader who takes its green
    // line as "no silent skip can exist" is making exactly the inference this
    // whole change exists to forbid.
    println!(
        "                the shapes a source scan cannot see -- an inverted guard wrapping a \
         whole body, a guard inside the measurement's own closure -- are closed by \
         `vitrin_skip::Verdict` and `vitrin_skip::Measured` being unreadable, not by this \
         scan. A test that re-implements a probe inline, one that hides its `return` inside a \
         `macro_rules!` of its own, and one whose every assertion sits in a conditional that is \
         false at run time are closed by NEITHER; the full bound is in \
         crates/vitrin-skip/src/lib.rs under \"What this does NOT close\"."
    );
    Ok(())
}

/// The one macro a sanctioned skip goes through.
const SKIP_MACRO: &str = "skip_unless";

/// Every `vitrin_skip::skip_unless!` call site, as
/// `(class name, test fn name, file, line)`.
///
/// The class is read out of the macro's FIRST argument: the constants are
/// named exactly `class.name.to_uppercase().replace('-', "_")`, which
/// [`class_names`] asserts, so the mapping is derived rather than a second
/// table to keep in sync. The rung a ladder class needed is deliberately
/// NOT read here -- since the probes were sealed it travels inside the
/// `Verdict`, so there is no second copy of it to parse or to drift.
fn call_sites(tests: &[TestFn]) -> BTreeSet<(String, String, String, usize)> {
    let mut out = BTreeSet::new();
    for test in tests {
        for at in word_positions(&test.body, SKIP_MACRO) {
            if !test.body[at + SKIP_MACRO.len()..]
                .trim_start()
                .starts_with('!')
            {
                continue;
            }
            let args = &test.body[at..];
            let Some(open) = args.find('(') else { continue };
            let arg = args[open + 1..]
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .rsplit("::")
                .next()
                .unwrap_or("")
                .to_string();
            let class = arg.to_lowercase().replace('_', "-");
            let line = test.body[..at].matches('\n').count() + test.line;
            out.insert((class, test.name.clone(), test.file.clone(), line));
        }
    }
    out
}

/// What `.github/workflows/ci.yml` must say about one skip class, **and in
/// which job**.
///
/// The require-variables are the enforcement, and until this table existed
/// nothing asserted the workflow declared them at all: a one-character typo
/// in a variable NAME left every job green and every class unenforced, with
/// the diff reading as a no-op. Both directions are checked, because
/// "nothing sets this" is as much a claim as "the `rust` job sets it to 1".
///
/// The job is part of the row, not a comment beside it. A workflow-wide check
/// -- which is what this was first written as -- goes green when
/// `VITRIN_REQUIRE_CONFINEMENT: "1"` is *moved* from the job that runs the
/// confinement tests to one that runs no Rust test at all: the variable is
/// still declared somewhere, and it now enforces nothing. That is the same
/// green-over-absent-evidence shape one level up, so the scope is the job.
struct CiDeclaration {
    /// [`vitrin_skip::Class::require_var`].
    var: &'static str,
    /// `(job id, exact value)` for every job that must set it. **Empty means
    /// no job in the workflow may set it at all**, and a job outside this
    /// list setting it is a problem either way.
    jobs: &'static [(&'static str, &'static str)],
    /// Why that is the right declaration for this repository's runners.
    why: &'static str,
}

/// What the workflow must declare, per class.
const CI_DECLARATIONS: &[CiDeclaration] = &[
    CiDeclaration {
        var: "VITRIN_REQUIRE_CONFINEMENT",
        jobs: &[("rust", "1")],
        why: "the `rust` job takes the sysctl remedy vitrind's own preflight prints, so its \
              runner CAN confine and a confinement skip there is a broken job rather than an \
              honest kernel. It is also the only job that runs those tests -- `integration` \
              takes the same sysctl but drives the Python suite, so a declaration there would \
              enforce nothing",
    },
    CiDeclaration {
        var: "VITRIN_REQUIRE_HOST_TOOLING",
        jobs: &[("rust", "1")],
        why: "/usr/bin/env and a bindable build-directory ancestor both hold on the runner \
              image, and both would otherwise start skipping after an unrelated change with no \
              other symptom",
    },
    CiDeclaration {
        var: "VITRIN_REQUIRE_LANDLOCK_ABI",
        jobs: &[("rust", "7")],
        why: "the ABI the runner was MEASURED at on 2026-08-14 (docs/book/src/limits.md), which \
              is also this build's own startup floor -- change it only against a fresh \
              measurement",
    },
    CiDeclaration {
        var: "VITRIN_C_SHIM_CONFORMANCE_SKIP",
        jobs: &[("rust", "1")],
        why: "this variable EXCUSES rather than requires: the `rust` job has no C toolchain and \
              says so, which is what leaves the `conformance` job required by default. The job \
              scope is load-bearing here -- `conformance` setting the same variable would excuse \
              the one job that satisfies the class for real, and a workflow-wide check could not \
              tell the two apart",
    },
    CiDeclaration {
        var: "VITRIN_REQUIRE_GPU",
        jobs: &[],
        why: "no runner has a GPU, so no job may claim one. Asserting the ABSENCE is the point: \
              a job that started setting this would turn three honest skips into red tests",
    },
];

/// Every `VITRIN_*: value` assignment in a workflow, **attributed to the job
/// that makes it**, ignoring comment lines.
///
/// A line-oriented read rather than a YAML parse, and that is a deliberate
/// limit: this workflow's `env:` blocks are flat `NAME: "value"` lines and its
/// jobs are the only keys at two-space indent under `jobs:`, so the whole
/// subtlety is ignoring `#` lines -- the prose comments around these blocks
/// name the same variables constantly. Anything before `jobs:` is attributed
/// to the empty job id, which no row may name.
fn workflow_declarations(text: &str) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let mut out: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut in_jobs = false;
    let mut job = String::new();
    for line in text.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        // A job id: the only two-space-indented bare key in this file.
        if in_jobs
            && !line.starts_with("   ")
            && line.starts_with("  ")
            && line.ends_with(':')
            && line[2..line.len() - 1]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            && line.len() > 3
        {
            job = line[2..line.len() - 1].to_string();
            continue;
        }
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.starts_with("VITRIN_")
            || !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        out.entry(name.to_string())
            .or_default()
            .insert((job.clone(), value.to_string()));
    }
    out
}

/// Render a set of `(job, value)` pairs for a message.
fn render_declarations(found: &BTreeSet<(String, String)>) -> String {
    found
        .iter()
        .map(|(job, value)| format!("{job}={value:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A require-variable assigned in **shell**, rather than in a job's `env:`.
///
/// [`workflow_declarations`] reads `NAME: "value"` lines, which is where a
/// declaration belongs and the only place [`CI_DECLARATIONS`] can reason
/// about. `run: VITRIN_REQUIRE_CONFINEMENT=0 cargo test ...` -- or the same
/// line inside a `run: |` block -- is invisible to it in both directions:
/// the `env:` block still reads `"1"`, the table still matches, and the one
/// command that actually runs the tests has the enforcement switched off.
/// The value does not matter (`=1` in one step is no better: it makes a
/// per-command declaration that the job-scoped table cannot see, which is the
/// same drift with the opposite sign), so any such assignment is a problem.
///
/// Prose is exempt only where it is a comment line -- the workflow's own
/// commentary names these variables constantly, and it runs nothing.
fn shell_overrides(text: &str, problems: &mut Vec<String>) {
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        for class in vitrin_skip::CLASSES {
            if !trimmed.contains(&format!("{}=", class.require_var)) {
                continue;
            }
            problems.push(format!(
                ".github/workflows/ci.yml:{} assigns {} in shell, not in a job's `env:` block: \
                 {trimmed:?}. A require-variable's scope is the JOB -- that is what \
                 CI_DECLARATIONS in crates/xtask/src/skip_census.rs states and checks -- and an \
                 assignment scoped to one command is invisible to it, so `...=0 cargo test` \
                 disables the enforcement for exactly the command that needed it while the env \
                 block above still reads as declared. Declare it in `env:`, or not at all.",
                index + 1,
                class.require_var,
            ));
        }
    }
}

/// Hold `.github/workflows/ci.yml` to [`CI_DECLARATIONS`], both directions.
fn check_ci_declarations(root: &Path, problems: &mut Vec<String>) -> Result<()> {
    let path = root.join(".github/workflows/ci.yml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let declared = workflow_declarations(&text);
    shell_overrides(&text, problems);

    // Every class has a stated position, so a new class cannot be added
    // without somebody deciding whether CI declares it.
    for class in vitrin_skip::CLASSES {
        if !CI_DECLARATIONS.iter().any(|d| d.var == class.require_var) {
            problems.push(format!(
                "class `{}` has no row in CI_DECLARATIONS in crates/xtask/src/skip_census.rs. \
                 Say whether .github/workflows/ci.yml declares {}, in which job and with what \
                 value -- a class nobody decided about is a class nothing enforces.",
                class.name, class.require_var
            ));
        }
    }
    for row in CI_DECLARATIONS {
        if !vitrin_skip::CLASSES
            .iter()
            .any(|c| c.require_var == row.var)
        {
            problems.push(format!(
                "CI_DECLARATIONS names {}, which is no class's require_var any more. Delete the \
                 row, or fix the name.",
                row.var
            ));
            continue;
        }
        let empty = BTreeSet::new();
        let found = declared.get(row.var).unwrap_or(&empty);
        let want: BTreeSet<(String, String)> = row
            .jobs
            .iter()
            .map(|(job, value)| ((*job).to_string(), (*value).to_string()))
            .collect();
        for (job, value) in want.difference(found) {
            problems.push(format!(
                ".github/workflows/ci.yml does not set {} to {value:?} in the `{job}` job (it \
                 sets: {}). {}. Without that declaration, in THAT job, every skip in the class is \
                 a silent pass on a runner that could have run the measurement -- which is issue \
                 #288 exactly, reintroduced by a one-line edit to a workflow.",
                row.var,
                if found.is_empty() {
                    "nothing".to_string()
                } else {
                    render_declarations(found)
                },
                row.why,
            ));
        }
        for (job, value) in found.difference(&want) {
            let job = if job.is_empty() {
                "the workflow's top level".to_string()
            } else {
                format!("the `{job}` job")
            };
            problems.push(format!(
                ".github/workflows/ci.yml sets {}={value:?} in {job}, and this repository's \
                 declaration for that variable is {}. {}. A require-variable in the wrong job \
                 enforces nothing where it matters and may excuse the job where it does.",
                row.var,
                if row.jobs.is_empty() {
                    "that NO job may set it".to_string()
                } else {
                    format!("{:?}", row.jobs)
                },
                row.why,
            ));
        }
    }
    Ok(())
}

/// One parsed marker line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Marker {
    class: String,
    test: String,
    reason: String,
}

/// Parse one line of a suite's output, returning a marker if it is one.
fn parse_marker(line: &str) -> Option<Marker> {
    let at = line.find(vitrin_skip::MARKER)?;
    let rest = line[at + vitrin_skip::MARKER.len()..].trim_start();
    let mut parts = rest.splitn(3, ' ');
    let class = parts.next()?.to_string();
    let test = parts.next()?.to_string();
    let reason = parts.next().unwrap_or("").trim().to_string();
    if class.is_empty() || test.is_empty() {
        return None;
    }
    Some(Marker {
        class,
        test,
        reason,
    })
}

/// Whether a marker came from the mechanism's own suite.
///
/// `crates/vitrin-skip`'s unit tests call `vitrin_skip::decide` on purpose,
/// so every census over a run that includes them sees real marker lines that
/// are not skips of any measurement. They are exempt from the [`INVENTORY`],
/// counted separately -- and, where an invocation passes
/// `--expect-self-marker`, REQUIRED: a census that can no longer see a
/// marker it knows was printed has stopped being a census, and only a
/// planted marker can tell it so.
fn is_self_test(marker: &Marker) -> bool {
    marker.test.starts_with("vitrin_skip::")
}

/// `cargo xtask skip-census --min-tests N [--expect-self-marker] -- <command...>`:
/// run a suite, filter its output, itemise every skip, and fail on one nobody
/// has justified -- or on a run too small to have proved anything.
///
/// The command is passed whole rather than assembled here, because the jobs
/// that use it run different suites. Three things are enforced before any
/// affirmative claim is printed:
///
/// 1. **`--show-output`.** libtest captures a passing test's output, so
///    without it the census would read a stream with no markers in it and
///    report a clean run.
/// 2. **`--min-tests N`, required on every invocation, and N may not be
///    zero.** A name-filtered `cargo test` that matches nothing prints `0
///    passed; 0 failed; 998 filtered out` and exits 0, and the census used to
///    answer that with "every sanctioned skip site in this suite ran" -- an
///    affirmative over an empty run, which is the precise defect this module
///    exists to abolish. The floor is per-invocation and not a global
///    constant, because the two-test injector slice and the thousand-test
///    workspace run cannot share a number, and a number that fits both fits
///    neither. `--min-tests 0` is **refused rather than accepted**: it parses,
///    it reads like a floor, and it reinstates exactly the hole the flag was
///    added to close, so it is the one value that must not be spellable. A
///    run that genuinely expects no test is not a census; do not wrap it.
///
///    **What the floor does not prove.** The denominator is counted from
///    libtest-shaped lines on the wrapped command's stdout, so it is
///    forgeable: anything that writes `test <name> ... ok` past libtest's
///    own capture inflates it. One real offender existed -- this module's
///    wrapper tests printed fixture suites that the outer census counted --
///    and it is fixed by echoing to a caller-supplied sink instead of
///    stdout, with a test pinning that. But the general property is
///    unenforced: the floor detects a suite that ran far too few tests, not
///    a suite lying about how many it ran. It is a tripwire against filters
///    and target moves, not a proof of execution.
/// 3. **`--expect-self-marker`**, where the run includes `crates/vitrin-skip`'s
///    own suite: at least one marker must have been seen, which is the
///    census's own non-vacuity check.
pub fn skip_census(argv: &[String]) -> Result<()> {
    let options = parse_census_args(argv)?;
    // The step summary is GitHub's, so it is read from the environment here
    // and passed down -- `run_census` takes it as an argument so the tests
    // below can drive a whole census without writing into a real CI job's
    // summary or racing each other over a process-wide variable.
    // The suite's own stream goes to the real stdout, through the same
    // `Stdout` handle `print!` uses so the census's report and the output it
    // is about stay in order. The tests below hand it a buffer instead, which
    // is not a convenience: see [`run_census`].
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    run_census(
        &options,
        std::env::var_os("GITHUB_STEP_SUMMARY").map(PathBuf::from),
        &mut sink,
    )
}

/// One parsed, validated `skip-census` invocation.
struct CensusOptions<'a> {
    /// How many tests the wrapped command must EXECUTE before the census is
    /// allowed to say anything affirmative. Never zero; see [`skip_census`].
    min_tests: usize,
    /// Whether this run is expected to contain `crates/vitrin-skip`'s own
    /// planted markers.
    expect_self_marker: bool,
    /// The command to run, verbatim.
    argv: &'a [String],
}

/// Parse and validate the census's own flags, stopping at `--`.
fn parse_census_args(argv: &[String]) -> Result<CensusOptions<'_>> {
    let mut min_tests: Option<usize> = None;
    let mut expect_self_marker = false;
    let mut rest = argv;
    while let Some(flag) = rest.first() {
        match flag.as_str() {
            "--min-tests" => {
                let Some(value) = rest.get(1) else {
                    bail!("--min-tests needs a number");
                };
                let parsed: usize = value
                    .parse()
                    .with_context(|| format!("--min-tests {value}: not a number"))?;
                if parsed == 0 {
                    bail!(
                        "--min-tests 0 is refused. A floor of zero is satisfied by a run that \
                         executed NOTHING, which is the exact green-over-absent-evidence defect \
                         `--min-tests` was added to close -- an escape hatch shaped like \
                         compliance, and the one value nobody reviewing the diff would read as a \
                         decision. Measure what this command really runs and declare that; if the \
                         honest answer is that it may legitimately run no test, it is not a \
                         census and must not be wrapped in one."
                    );
                }
                min_tests = Some(parsed);
                rest = &rest[2..];
            }
            "--expect-self-marker" => {
                expect_self_marker = true;
                rest = &rest[1..];
            }
            "--" => {
                rest = &rest[1..];
                break;
            }
            _ => break,
        }
    }
    let Some(min_tests) = min_tests else {
        bail!(
            "skip-census needs `--min-tests N`: the number of tests this invocation must \
             actually EXECUTE before its census is allowed to say anything affirmative. A \
             name-filtered run that matches nothing exits 0 having run no test, and a census \
             that answers that with `every sanctioned skip site ran` is the same green-over-\
             absent-evidence defect it exists to catch. Measure the number for THIS command and \
             pass it; there is deliberately no default, and zero is refused."
        );
    };
    Ok(CensusOptions {
        min_tests,
        expect_self_marker,
        argv: rest,
    })
}

/// Run one census. `summary` is `$GITHUB_STEP_SUMMARY`, or `None` off CI, and
/// `sink` is where the wrapped suite's own output is echoed.
///
/// **`sink` is a parameter because of what the tests below print.** They drive
/// this wrapper over fake suites whose fixture text is shaped exactly like
/// libtest's own (`test alpha ... ok`) -- it has to be, or they would assert
/// nothing about the counting. Echoing that to the process's real stdout put
/// five such lines into `cargo test -p xtask`'s output stream at the TOP
/// level, where libtest's capture never sees them (a locked `Stdout` handle
/// bypasses it) and where the outer `cargo xtask skip-census` wrapping that
/// suite counted every one as a test the run had executed. A census padding
/// its own denominator with its own fixtures is the defect this module exists
/// to abolish, one level removed, so the fixtures now go to a buffer and
/// never reach fd 1 at all.
fn run_census(
    options: &CensusOptions,
    summary: Option<PathBuf>,
    sink: &mut dyn std::io::Write,
) -> Result<()> {
    let &CensusOptions {
        min_tests,
        expect_self_marker,
        argv,
    } = options;
    let Some((program, args)) = argv.split_first() else {
        bail!(
            "skip-census needs a command to run, e.g. `--min-tests 900 -- cargo test \
             --workspace -- --show-output`"
        );
    };
    // `contains` rather than equality: a wrapped command (`unshare ... sh -c
    // '... exec <binary> --show-output'`, which is how this repository's own
    // non-vacuity lever denies the kernel's namespaces) carries the flag
    // inside one argv element.
    if !argv
        .iter()
        .any(|a| a.contains("--show-output") || a.contains("--nocapture"))
    {
        bail!(
            "skip-census refuses to run `{}` without `--show-output`: libtest captures a PASSING \
             test's stdout, so every marker line would be invisible and this census would report \
             a clean run over a suite that skipped everything. That is issue #288 with an extra \
             step.",
            argv.join(" ")
        );
    }

    // One pipe for both streams, so the child's own interleaving survives.
    let (reader, writer) = std::io::pipe().context("creating the census pipe")?;
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::from(writer.try_clone().context("cloning the pipe")?))
        .stderr(Stdio::from(writer))
        .spawn()
        .with_context(|| format!("spawning {program}"))?;

    let mut markers: Vec<Marker> = Vec::new();
    // Which tests passed, learned from libtest's own per-test lines, which
    // stream BEFORE the `--show-output` blocks at the end of a binary's run.
    let mut passed: BTreeSet<String> = BTreeSet::new();
    // ...and how many EXECUTED, which is what the floor is about. `ignored`
    // is deliberately not counted: an ignored test ran nothing, and counting
    // it would let a suite satisfy its floor with tests it declined to run.
    let mut executed = 0usize;
    // The captured-output block currently being suppressed, if any.
    let mut suppressing = false;

    for line in BufReader::new(reader).lines() {
        let line = line.context("reading the suite's output")?;
        // Only OUTSIDE a captured-output block. libtest prints its own
        // per-test lines at the top level and a test's captured stdout under a
        // `---- <path> stdout ----` heading, so a test that itself prints
        // something shaped like `test alpha ... ok` -- this module's own
        // wrapper tests do, deliberately -- would otherwise be counted as
        // tests this run executed. A census that inflates its own denominator
        // is the defect one step removed.
        if !suppressing {
            if let Some(rest) = line.strip_prefix("test ") {
                if let Some((name, verdict)) = rest.rsplit_once(" ... ") {
                    if verdict.starts_with("ok") {
                        passed.insert(name.trim().to_string());
                        executed += 1;
                    } else if verdict.starts_with("FAILED") {
                        executed += 1;
                    }
                }
            }
        }
        if let Some(marker) = parse_marker(&line) {
            markers.push(marker);
        }
        // `---- <path> stdout ----` opens a captured-output block. Blocks
        // belonging to tests that PASSED are the noise `--show-output`
        // generates (996 tests in the vitrind binary alone, many of which
        // print measurements); blocks belonging to failures are the whole
        // reason anybody reads a red log, so they pass through untouched.
        if let Some(rest) = line.strip_prefix("---- ") {
            let name = rest
                .rsplit_once(" std")
                .map(|(n, _)| n.trim())
                .unwrap_or("")
                .to_string();
            suppressing = passed.contains(&name);
        } else if line == "successes:" || line == "failures:" || line.starts_with("test result:") {
            suppressing = false;
        }
        if !suppressing || parse_marker(&line).is_some() {
            writeln!(sink, "{line}").context("writing the suite's output")?;
        }
    }
    sink.flush().ok();

    let status = child.wait().context("waiting for the suite")?;

    // Deduplicated on the whole `(class, test, reason)` triple, because a
    // REQUIRED skip prints its marker twice on purpose: once from the
    // `println!` and once inside the panic message, which repeats the line so
    // a red test carries it too. A test can only skip once -- `decide`'s
    // caller returns -- so two identical triples are always one skip seen
    // twice, and counting them twice would make "N skip(s)" a number that
    // depends on whether the class was required.
    let markers: Vec<Marker> = markers
        .into_iter()
        .collect::<BTreeSet<Marker>>()
        .into_iter()
        .collect();
    let (self_markers, markers): (Vec<Marker>, Vec<Marker>) =
        markers.into_iter().partition(is_self_test);
    report_census(&markers, &self_markers, executed, min_tests, summary)?;

    let problems = census_problems(
        &markers,
        &self_markers,
        executed,
        min_tests,
        expect_self_marker,
    );
    if !problems.is_empty() {
        // Streamed first, so each problem lands in the log next to the suite
        // output it is about; repeated in the error, because an error that
        // says only "1 problem" is itself a claim with the evidence left
        // somewhere else.
        for p in &problems {
            eprintln!("xtask skip-census: {p}");
        }
        bail!(
            "{} census problem(s). Issue #288: a green claim may not exceed the evidence.\n  - {}",
            problems.len(),
            problems.join("\n  - ")
        );
    }

    if !status.success() {
        bail!(
            "the suite failed ({status}). The census above is what it did NOT prove, and is not \
             a verdict on the failure."
        );
    }
    Ok(())
}

/// Everything a finished census has to fail on, as a pure function of what
/// the run produced.
///
/// Extracted from [`run_census`] so the rules can be asserted directly. That
/// matters most for the **floor**, which was the one new gate in this module
/// with no test of its own: a gate whose failure path nothing exercises is a
/// gate that can be broken by an edit that looks like a simplification, which
/// is the same "nobody checked" shape one level up from the skips it guards.
fn census_problems(
    markers: &[Marker],
    self_markers: &[Marker],
    executed: usize,
    min_tests: usize,
    expect_self_marker: bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    // The floor first, because everything below it is a claim about a run,
    // and a run that executed nothing supports no claim at all.
    if executed < min_tests {
        problems.push(format!(
            "this invocation executed {executed} test(s) and declared a floor of {min_tests}. \
             Whatever this step is cited for, it did not do it -- a name filter that matches \
             nothing, a renamed test, a binary that failed to build its harness, all look like \
             this and all exit 0. Either fix the command, or lower the floor deliberately and \
             say why in .github/workflows/ci.yml."
        ));
    }
    if expect_self_marker && self_markers.is_empty() {
        problems.push(format!(
            "--expect-self-marker was passed and no marker from `vitrin_skip::` appeared. That \
             suite plants markers on purpose, so seeing none means this census cannot see a \
             marker it KNOWS was printed -- the parser, the `{}` constant or `--show-output` \
             has broken, and every clean census from here on would be meaningless.",
            vitrin_skip::MARKER
        ));
    }
    let known: BTreeSet<&str> = vitrin_skip::CLASSES.iter().map(|c| c.name).collect();
    let inventoried: BTreeSet<(&str, &str)> = INVENTORY.iter().map(|s| (s.class, s.test)).collect();
    for marker in markers {
        if !known.contains(marker.class.as_str()) {
            problems.push(format!(
                "marker names class `{}`, which is not one of {known:?}",
                marker.class
            ));
            continue;
        }
        if !inventoried.contains(&(marker.class.as_str(), marker.test.as_str())) {
            problems.push(format!(
                "`{}` skipped for class `{}` and is not in the INVENTORY in \
                 crates/xtask/src/skip_census.rs -- add it with the reason that skip is an \
                 honest machine state rather than a broken job",
                marker.test, marker.class
            ));
        }
    }
    problems
}

/// Print the census -- to stdout always, and to `summary`
/// (`$GITHUB_STEP_SUMMARY`) when the caller was given one.
///
/// Printed even when nothing skipped, because the failure mode this guards
/// against IS a success: a reader has to be able to tell "nothing skipped"
/// from "nobody looked", and only one of those two prints a line.
///
/// **The affirmative sentence is gated on the floor.** Over a run that
/// executed fewer than `min_tests`, "every sanctioned skip site in this
/// suite ran" is a claim about tests that did not exist in this run, and
/// printing it -- as this function did, over `0 passed; 998 filtered out` --
/// is the same defect the module is written against. Below the floor it says
/// what happened instead, and the caller fails.
fn report_census(
    markers: &[Marker],
    self_markers: &[Marker],
    executed: usize,
    min_tests: usize,
    summary: Option<PathBuf>,
) -> Result<()> {
    let out = census_text(markers, self_markers, executed, min_tests);
    print!("{out}");
    std::io::stdout().flush().ok();

    if let Some(path) = summary {
        let mut block = String::from("### Skip census\n\n```text\n");
        block.push_str(&out);
        block.push_str("```\n");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.write_all(block.as_bytes())
            .context("writing the step summary")?;
    }
    Ok(())
}

/// The census itself, as text -- built rather than printed, so the rule about
/// what it may and may not claim can be asserted directly.
fn census_text(
    markers: &[Marker],
    self_markers: &[Marker],
    executed: usize,
    min_tests: usize,
) -> String {
    let mut by_class: BTreeMap<&str, Vec<&Marker>> = BTreeMap::new();
    for m in markers {
        by_class.entry(m.class.as_str()).or_default().push(m);
    }

    let mut out = String::new();
    if executed < min_tests {
        let _ = writeln!(
            out,
            "==> skip census: NOT ASSERTED. {executed} test(s) executed, and this invocation \
             declared a floor of {min_tests}. Nothing about skips is claimed either way over a \
             run this small."
        );
    } else if markers.is_empty() {
        let _ = writeln!(
            out,
            "==> skip census: 0 skipped in {executed} test(s) (floor {min_tests}). Every \
             sanctioned skip site this run reached ran."
        );
    } else {
        let _ = writeln!(
            out,
            "==> skip census: {} skip(s) in {executed} test(s) (floor {min_tests}). This run did \
             NOT prove:",
            markers.len()
        );
        for (class, ms) in &by_class {
            let var = vitrin_skip::CLASSES
                .iter()
                .find(|c| c.name == *class)
                .map_or("?", |c| c.require_var);
            let _ = writeln!(
                out,
                "    [{class}] ({} skip(s); require with {var})",
                ms.len()
            );
            for m in ms {
                let _ = writeln!(out, "      {} -- {}", m.test, m.reason);
                // The machine's reason and the repository's justification,
                // side by side. They are supposed to describe the same
                // machine state; a reader who can see both can notice when
                // they have drifted apart, which no count could show.
                match INVENTORY
                    .iter()
                    .find(|s| s.class == m.class && s.test == m.test)
                {
                    Some(s) => {
                        let _ = writeln!(out, "        sanctioned because: {}", s.why);
                    }
                    None => {
                        let _ = writeln!(
                            out,
                            "        NOT SANCTIONED: no INVENTORY entry -- this census FAILS"
                        );
                    }
                }
            }
        }
    }
    if !self_markers.is_empty() {
        // Not a skip of anything, and said so explicitly rather than folded
        // into the count: these are `crates/vitrin-skip`'s own tests calling
        // `decide`, and their arrival is the proof that this census can see a
        // marker at all.
        let _ = writeln!(
            out,
            "    [self-test] {} planted marker(s) from crates/vitrin-skip's own suite -- not \
             skips; their presence is this census proving it can see one.",
            self_markers.len()
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testtree::TestTree;

    /// The const-name mapping [`call_sites`] relies on, asserted rather than
    /// assumed: the scan reads `vitrin_skip::CONFINEMENT` out of a macro
    /// argument and turns it into `confinement` by case alone.
    #[test]
    fn class_names() {
        for class in vitrin_skip::CLASSES {
            let konst = class.name.to_uppercase().replace('-', "_");
            assert_eq!(
                konst.to_lowercase().replace('_', "-"),
                class.name,
                "the const name for `{}` does not round-trip",
                class.name
            );
        }
    }

    /// The mask is the whole reason the scan is exact rather than
    /// approximate, so it is asserted against the three constructs that
    /// would break a naive scanner: a brace inside a format string, the word
    /// `return` inside a doc comment, and a lifetime that is not a char
    /// literal.
    #[test]
    fn the_mask_hides_strings_and_comments_and_keeps_lifetimes() {
        let src = r#"
/// return this doc comment's word
fn f<'a>(x: &'a str) -> String {
    let s = format!("{} {{", x);
    let c = '}';
    s
}
"#;
        let masked = mask_code(src);
        assert!(
            word_positions(&masked, "return").is_empty(),
            "a doc comment's `return` must not read as control flow"
        );
        assert_eq!(
            masked.matches('{').count(),
            1,
            "only the fn body's brace survives: {masked:?}"
        );
        assert_eq!(masked.matches('}').count(), 1, "...and its partner");
        assert!(masked.contains("fn f"), "code itself must survive the mask");
    }

    /// The mask must preserve EVERY line, and the construct that broke it is
    /// the one Rust uses most in this tree: a `\` at the end of a line inside
    /// a string literal, which continues the string onto the next line.
    ///
    /// The escaped character is skipped rather than masked, so without an
    /// explicit check the newline behind the backslash is eaten and every
    /// line number after it is short by one. Measured before the fix: 6987
    /// newlines in `crates/vitrin-core/src/spawn.rs`, 6854 in the mask, so
    /// `skip-scan` reported that file's problems 118 lines above where they
    /// were.
    #[test]
    fn the_mask_preserves_every_line() {
        let src = "fn f() {\n    panic!(\n        \"a long message that \\\n         continues \
                   here\"\n    );\n}\n";
        assert!(
            src.contains("\\\n"),
            "the fixture must really contain a backslash-newline continuation"
        );
        let masked = mask_code(src);
        assert_eq!(
            masked.matches('\n').count(),
            src.matches('\n').count(),
            "a continuation inside a string literal must not eat a line: {masked:?}"
        );

        // The same, for a char literal's escape, and for a raw string.
        for src in [
            "let c = '\\n';\nlet d = 1;\n",
            "let s = r#\"one\ntwo\"#;\nlet d = 1;\n",
            "// a comment\n/* a\nblock */\nlet d = 1;\n",
        ] {
            assert_eq!(
                mask_code(src).matches('\n').count(),
                src.matches('\n').count(),
                "the mask lost a line in {src:?}"
            );
        }
    }

    /// The mask is byte-for-byte the same length as its input, over every
    /// source in the tree.
    ///
    /// `crates/xtask/src/test_census.rs` reads an attribute's body out of the
    /// ORIGINAL source at an offset this scan found in the MASKED one -- it
    /// has to, because the mask blanks the `"drm-backend"` inside
    /// `#[cfg(feature = "drm-backend")]`, and a cfg predicate read off the
    /// mask would say `feature = ""` and match nothing. This file is full of
    /// em dashes and other multi-byte characters inside the comments the mask
    /// blanks, so without this invariant those offsets would drift.
    #[test]
    fn the_mask_is_byte_for_byte_the_same_length() {
        let src = "// an em dash — and a quote \"héllo\"\nfn f() { let s = \"…\"; }\n";
        assert_ne!(
            src.len(),
            src.chars().count(),
            "the fixture must really contain multi-byte characters"
        );
        assert_eq!(mask_code(src).len(), src.len());

        let root = crate::workspace_root().expect("the workspace root");
        let built = BuildOutput::of_tree(&root).expect("the workspace is a git work tree");
        let mut sources = Vec::new();
        for dir in SCANNED {
            rust_sources(&root.join(dir), &root, &built, &mut sources).expect("reading sources");
        }
        assert!(sources.len() > 50, "found only {} sources", sources.len());
        for (file, text) in &sources {
            assert_eq!(
                mask_code(text).len(),
                text.len(),
                "the mask changed the byte length of {file}, so every offset into it is wrong"
            );
        }
    }

    /// ...and the same invariant over the REAL tree, which is where a
    /// construct nobody thought of would actually show up.
    #[test]
    fn the_mask_preserves_every_line_of_every_source_in_the_tree() {
        let root = crate::workspace_root().expect("the workspace root");
        let built = BuildOutput::of_tree(&root).expect("the workspace is a git work tree");
        let mut sources = Vec::new();
        for dir in SCANNED {
            rust_sources(&root.join(dir), &root, &built, &mut sources).expect("reading sources");
        }
        assert!(
            sources.len() > 50,
            "found only {} sources -- this assertion would be vacuous",
            sources.len()
        );
        for (file, text) in &sources {
            assert_eq!(
                mask_code(text).matches('\n').count(),
                text.matches('\n').count(),
                "the mask lost a line in {file}"
            );
        }
    }

    /// **#295's shape, in the roots THIS scan walks.** The sibling gate's bug
    /// was a walk over `shim/` that read `shim/build/`; the same walk here is
    /// over `crates/` and `fuzz/`, and it skipped exactly one directory name.
    ///
    /// A `#[test]` in a directory CI will never check out cannot be run by a
    /// CI step, so demanding one of it reports the developer's disk rather
    /// than the repository -- and the census's cross-check would name the
    /// file as unreachable-by-`mod`, which is a real defect class, so the
    /// false report looks exactly like a true one.
    ///
    /// The build directory here is deliberately not called `target`: naming
    /// it `builddir` is what a `meson setup` under a scanned root would do,
    /// and the name check this replaced would have walked straight into it.
    /// The tracked source beside it must still be read, or the scan has been
    /// silenced rather than corrected.
    #[test]
    fn build_output_under_a_scanned_root_is_not_a_source() {
        let tree = TestTree::new("skip-census-build-output");
        tree.write(".gitignore", "builddir/\n");
        tree.write(
            "crates/vitrin-core/src/lib.rs",
            "#[test]\nfn a_real_test() {}\n",
        );
        tree.write(
            "crates/vitrin-core/builddir/sub/generated.rs",
            "#[test]\nfn a_generated_test() {}\n",
        );
        tree.git_init();

        let built = BuildOutput::of_tree(tree.path()).expect("a git work tree");
        let mut sources = Vec::new();
        rust_sources(
            &tree.path().join("crates"),
            tree.path(),
            &built,
            &mut sources,
        )
        .expect("reading sources");
        let files: Vec<&str> = sources.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(files, ["crates/vitrin-core/src/lib.rs"]);
    }

    /// NON-VACUITY for the scan: a synthetic test body with a bare `return`
    /// is found, and the same body routed through the macro is not. If this
    /// ever passes in both directions the scan has stopped being a scan.
    #[test]
    fn the_scan_finds_a_bare_return_and_forgives_a_sanctioned_one() {
        let attr = format!("#[{}]", "test");
        let bare =
            format!("{attr}\nfn a_test() {{\n    if cond() {{ return; }}\n    assert!(x);\n}}\n");
        let found = test_fns("synthetic.rs", &bare);
        assert_eq!(found.len(), 1, "the parser must find the test");
        assert_eq!(found[0].name, "a_test");
        assert_eq!(
            word_positions(&found[0].body, "return").len(),
            1,
            "a bare early return must be visible to the scan"
        );

        let sanctioned = format!(
            "{attr}\nfn a_test() {{\n    vitrin_skip::skip_unless!(vitrin_skip::CONFINEMENT, \
             probe());\n    assert!(x);\n}}\n"
        );
        let found = test_fns("synthetic.rs", &sanctioned);
        assert!(
            word_positions(&found[0].body, "return").is_empty(),
            "the sanctioned form carries its return inside the macro"
        );
        let sites = call_sites(&found);
        assert_eq!(
            sites
                .iter()
                .map(|(c, t, _, _)| (c.as_str(), t.as_str()))
                .collect::<Vec<_>>(),
            vec![("confinement", "a_test")],
            "the scan must read the class out of the macro argument"
        );
    }

    /// A `|`-heavy header that is NOT a closure must stay transparent, so a
    /// bare `return` behind it is still counted.
    ///
    /// This is a regression test for two real holes, found one after the
    /// other. The rule here first said "two or more `|` characters in the
    /// header means a closure", which made the entirely idiomatic `if
    /// matches!(x, A | B | C) { ...; return; }` opaque and hid the return
    /// completely. Asking instead whether a `|` sits in prefix position fixed
    /// that row and left a bigger one open: in `if xs.iter().any(|m| ...) {
    /// return; }` the pipes ARE in prefix position, so the `if`'s block was
    /// still read as a closure body and the return was still invisible --
    /// and that is not an exotic shape, it is what a condition looks like
    /// whenever it asks a question about a collection. The five `CLOSURE IN
    /// THE CONDITION` rows below are that hole, each of them a line somebody
    /// would write without thinking; they are caught by comparing the pipe's
    /// parenthesis depth with the brace's.
    ///
    /// Every row is asserted in BOTH directions: the non-closures must expose
    /// their return, and the real closures must still swallow theirs, or the
    /// fix would just be the scan giving up on closures.
    #[test]
    fn a_pipe_heavy_header_that_is_not_a_closure_still_exposes_its_return() {
        // (body, expected body-level returns, what it is)
        let cases: &[(&str, usize, &str)] = &[
            // ---- CLOSURE IN THE CONDITION: the block is the test's own ----
            (
                "{\n    if xs.iter().any(|m| m.reached()) { return; }\n    assert!(y);\n}",
                1,
                "a closure inside an `if` condition -- the brace is the IF's, not the \
                 closure's, and the return leaves the test",
            ),
            (
                "{\n    if !xs.iter().all(|m| m.ok()) { return; }\n    assert!(y);\n}",
                1,
                "the same shape negated",
            ),
            (
                "{\n    if xs.iter().filter(|m| m.ok()).count() == 0 { return; }\n    \
                 assert!(y);\n}",
                1,
                "a closure in a condition that continues past it",
            ),
            (
                "{\n    if let Some(m) = xs.iter().find(|m| m.ok()) { return; }\n    assert!(y);\n}",
                1,
                "`if let` over a closure-taking iterator method",
            ),
            (
                "{\n    while xs.iter().any(|m| m.ok()) { return; }\n    assert!(y);\n}",
                1,
                "a `while` condition, which has the identical shape",
            ),
            (
                "{\n    if matches!(x, A | B | C) { return; }\n    assert!(y);\n}",
                1,
                "a matches! with three pattern alternatives is not a closure",
            ),
            (
                "{\n    if matches!(x, A | B) || flag { return; }\n    assert!(y);\n}",
                1,
                "pattern alternatives plus a boolean or",
            ),
            (
                "{\n    if a || b || c { return; }\n    assert!(y);\n}",
                1,
                "three boolean ors",
            ),
            (
                "{\n    if (mask | bit) == (other | bit) { return; }\n    assert!(y);\n}",
                1,
                "bitwise or on both sides of a comparison -- the shape the old \
                 comment admitted it would misread",
            ),
            (
                "{\n    match x { A | B => { return; } _ => {} }\n    assert!(y);\n}",
                1,
                "a match arm with alternatives -- its return leaves the TEST",
            ),
            // ...and the closures, which must still be opaque.
            (
                "{\n    v.retain(|a| { if a { return true; } false });\n    assert!(y);\n}",
                0,
                "an ordinary closure argument",
            ),
            (
                "{\n    std::thread::spawn(move || { return; });\n    assert!(y);\n}",
                0,
                "`move ||`, an empty parameter list",
            ),
            (
                "{\n    insert_source(src, |a, b| match a { _ => { return; } });\n    assert!(y);\n}",
                0,
                "a closure whose brace is opened by a `match`, not by the param list",
            ),
            (
                "{\n    let f = |x| { return x; };\n    assert!(y);\n}",
                0,
                "a closure bound by `let`",
            ),
            (
                "{\n    let f = |x: u8, y: u8| -> u8 { return x + y; };\n    assert!(y);\n}",
                0,
                "an annotated closure with a return type",
            ),
            (
                "{\n    assert!(v.iter().all(|x| { return x > 0; }));\n    assert!(y);\n}",
                0,
                "a closure nested two delimiters deep -- its brace is deep too, so the \
                 depth rule must not mistake it for a block of the test's",
            ),
            (
                "{\n    let g = xs.map(|s| s.trim()).for_each(|s| { return; });\n    \
                 assert!(y);\n}",
                0,
                "two closures in one header, the second of which owns the brace",
            ),
        ];
        for (body, want, what) in cases {
            assert_eq!(
                body_level_returns(body),
                *want,
                "{what}: {body:?} -- a return the scan cannot see is a skip the scan cannot see"
            );
        }
    }

    /// The workflow reader sees an assignment, ignores the prose, and knows
    /// WHICH JOB made it.
    ///
    /// Both directions, because a reader that saw the comments would report
    /// the declarations present whatever the `env:` blocks said -- and this
    /// file's comments name every one of these variables. The job attribution
    /// is asserted for the same reason it exists: a workflow-wide reader is
    /// satisfied by a declaration that has been moved to a job where it
    /// enforces nothing.
    #[test]
    fn the_workflow_reader_separates_declarations_from_prose_and_knows_the_job() {
        let yaml = "jobs:\n  rust:\n    env:\n      VITRIN_REQUIRE_CONFINEMENT: \"1\"\n      # \
                    VITRIN_REQUIRE_GPU: \"1\" would be a lie on this runner\n      \
                    VITRIN_REQUIRE_LANDLOCK_ABI: 7\n  conformance:\n    env:\n      \
                    VITRIN_REQUIRE_CONFINEMENT: \"0\"\n";
        let found = workflow_declarations(yaml);
        assert_eq!(
            found["VITRIN_REQUIRE_CONFINEMENT"]
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                ("conformance".to_string(), "0".to_string()),
                ("rust".to_string(), "1".to_string()),
            ],
            "the same variable in two jobs is two different declarations"
        );
        assert!(
            found["VITRIN_REQUIRE_LANDLOCK_ABI"].contains(&("rust".to_string(), "7".to_string()))
        );
        assert!(
            !found.contains_key("VITRIN_REQUIRE_GPU"),
            "a commented-out declaration is prose, not a declaration: {found:?}"
        );
    }

    /// NON-VACUITY for the job scope: the declarations table rejects a
    /// workflow that declares the right variables in the wrong jobs.
    ///
    /// The first version of this check was workflow-wide and would have
    /// passed every fixture below -- each of them declares every variable,
    /// with the right value, somewhere. That is what "the check is wider than
    /// the sentence describing it" cost, and this test is the sentence.
    #[test]
    fn a_declaration_in_the_wrong_job_is_rejected() {
        let root = crate::workspace_root().expect("the workspace root");
        let real = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))
            .expect("the checked-in workflow");

        // Moving the confinement declaration to a job that runs no Rust unit
        // test leaves it declared -- and enforcing nothing.
        let moved = real.replace(
            "      VITRIN_REQUIRE_CONFINEMENT: \"1\"",
            "      VITRIN_MOVED_AWAY: \"1\"",
        );
        assert_ne!(moved, real, "the fixture must really change the workflow");
        let moved = moved.replace(
            "  integration:\n",
            "  integration:\n    env:\n      VITRIN_REQUIRE_CONFINEMENT: \"1\"\n",
        );
        let found = workflow_declarations(&moved);
        assert!(
            !found["VITRIN_REQUIRE_CONFINEMENT"].contains(&("rust".into(), "1".into())),
            "the fixture must have moved the declaration out of `rust`"
        );
        assert!(
            found["VITRIN_REQUIRE_CONFINEMENT"].contains(&("integration".into(), "1".into())),
            "...and into `integration`"
        );

        // And the class whose whole design is that ONE job excuses itself:
        // `conformance` declaring the C-shim skip would excuse the only job
        // that satisfies the class for real.
        let excused = real.replace(
            "  conformance:\n",
            "  conformance:\n    env:\n      VITRIN_C_SHIM_CONFORMANCE_SKIP: \"1\"\n",
        );
        let found = workflow_declarations(&excused);
        assert!(
            found["VITRIN_C_SHIM_CONFORMANCE_SKIP"].contains(&("conformance".into(), "1".into())),
            "the fixture must really excuse the conformance job"
        );
        let row = CI_DECLARATIONS
            .iter()
            .find(|d| d.var == "VITRIN_C_SHIM_CONFORMANCE_SKIP")
            .expect("the row");
        assert_eq!(
            row.jobs,
            [("rust", "1")],
            "only `rust` may excuse itself; a second job here would be the hole"
        );
    }

    /// A require-variable set by a SHELL PREFIX is caught, and the same name
    /// in prose or in a real `env:` block is not.
    ///
    /// The hole: `run: VITRIN_REQUIRE_CONFINEMENT=0 cargo test ...` leaves the
    /// job's `env:` block -- the thing CI_DECLARATIONS reads -- saying `"1"`,
    /// so every check here stayed green while the one command that runs the
    /// confinement tests had the enforcement switched off for its own
    /// process. Both directions, because a rule that fires on the workflow's
    /// commentary would be reverted within a week.
    #[test]
    fn a_require_variable_assigned_in_shell_is_caught() {
        let mut problems = Vec::new();
        shell_overrides(
            "jobs:\n  rust:\n    steps:\n      - run: |\n          \
             VITRIN_REQUIRE_CONFINEMENT=0 cargo test --workspace\n",
            &mut problems,
        );
        assert_eq!(problems.len(), 1, "the override must be seen: {problems:?}");
        assert!(
            problems[0].contains("VITRIN_REQUIRE_CONFINEMENT") && problems[0].contains("assigns"),
            "{}",
            problems[0]
        );

        // A one-line `run:` prefix, which is the other spelling.
        let mut problems = Vec::new();
        shell_overrides(
            "        run: VITRIN_REQUIRE_LANDLOCK_ABI=0 cargo test -p vitrin-realm-init\n",
            &mut problems,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");

        // ...and the honest forms stay quiet: an `env:` declaration, and the
        // workflow's own prose about these variables.
        let mut problems = Vec::new();
        shell_overrides(
            "      VITRIN_REQUIRE_CONFINEMENT: \"1\"\n      # a comment about \
             VITRIN_REQUIRE_CONFINEMENT=0 and why nobody may write it\n",
            &mut problems,
        );
        assert!(
            problems.is_empty(),
            "a declaration and a comment are not overrides: {problems:?}"
        );

        // The checked-in workflow contains none, which is the claim the scan
        // makes on every run.
        let root = crate::workspace_root().expect("the workspace root");
        let real = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))
            .expect("the checked-in workflow");
        let mut problems = Vec::new();
        shell_overrides(&real, &mut problems);
        assert!(problems.is_empty(), "{problems:?}");
    }

    /// The workflow really does declare what [`CI_DECLARATIONS`] says, and
    /// every class has a row. Run here as well as in `skip-scan` so a local
    /// `cargo test -p xtask` catches a workflow edit without the scan.
    #[test]
    fn the_checked_in_workflow_matches_the_declarations_table() {
        let root = crate::workspace_root().expect("the workspace root");
        let mut problems = Vec::new();
        check_ci_declarations(&root, &mut problems).expect("reading the workflow");
        assert!(
            problems.is_empty(),
            ".github/workflows/ci.yml disagrees with CI_DECLARATIONS:\n{}",
            problems.join("\n")
        );
    }

    /// A self-test marker is recognised as one, and a real skip is not.
    #[test]
    fn the_census_tells_a_planted_self_test_marker_from_a_real_skip() {
        let planted = parse_marker(&vitrin_skip::line(
            &vitrin_skip::GPU,
            "vitrin_skip::tests::an_incapable_verdict_panics_where_the_class_is_required",
            "no GPU, on a machine that claimed one",
        ))
        .expect("a marker");
        assert!(is_self_test(&planted));

        let real = parse_marker(&vitrin_skip::line(
            &vitrin_skip::CONFINEMENT,
            "vitrind::spawn::tests::a_confined_realm_cannot_reach_the_canary",
            "ns.all=false",
        ))
        .expect("a marker");
        assert!(
            !is_self_test(&real),
            "a real skip must never be excused as a self-test"
        );
    }

    /// NON-VACUITY for the census parser, the same shape
    /// `shim/wlcs/test-summary.sh` uses: a fixture transcript that really
    /// contains markers must produce them, and a line that merely looks
    /// similar must not.
    #[test]
    fn the_census_parses_a_real_transcript_and_rejects_a_near_miss() {
        let marker = vitrin_skip::line(
            &vitrin_skip::CONFINEMENT,
            "vitrind::spawn::tests::a_confined_realm_cannot_reach_the_canary",
            "this kernel reports ns.all=false",
        );
        let parsed = parse_marker(&marker).expect("a real marker line parses");
        assert_eq!(parsed.class, "confinement");
        assert_eq!(
            parsed.test,
            "vitrind::spawn::tests::a_confined_realm_cannot_reach_the_canary"
        );
        assert!(parsed.reason.contains("ns.all=false"));

        assert!(
            parse_marker("test spawn::tests::x ... ok").is_none(),
            "an ordinary libtest line must not parse as a marker"
        );
        assert!(
            parse_marker(&format!("{} confinement", vitrin_skip::MARKER)).is_none(),
            "a marker missing its test path is not a marker"
        );
    }

    /// Build an argv the way a shell would.
    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    /// THE FLOOR, both directions, as a rule rather than as a run.
    ///
    /// `--min-tests` was the one new gate in this module with no test of its
    /// own: breaking it -- inverting the comparison, dropping the branch --
    /// left every census green and silent, which is precisely the shape the
    /// module exists to abolish, one level up from the skips it counts.
    #[test]
    fn the_floor_fails_a_run_that_executed_fewer_tests_than_it_declared() {
        let under = census_problems(&[], &[], 19, 20, false);
        assert_eq!(under.len(), 1, "one problem, the floor: {under:?}");
        assert!(
            under[0].contains("executed 19 test(s) and declared a floor of 20"),
            "the message must name both numbers: {}",
            under[0]
        );

        // Exactly at the floor is met -- an off-by-one here would either fire
        // on every honest run or never fire at all.
        assert!(census_problems(&[], &[], 20, 20, false).is_empty());
        assert!(census_problems(&[], &[], 21, 20, false).is_empty());

        // ...and the affirmative sentence is withheld below the floor, which
        // is the half a reader of the log sees.
        let below = census_text(&[], &[], 0, 20);
        assert!(
            below.contains("NOT ASSERTED"),
            "a run below its floor must claim nothing: {below}"
        );
        assert!(
            !below.contains("Every sanctioned skip site"),
            "the affirmative sentence must not appear over an empty run: {below}"
        );
        let above = census_text(&[], &[], 20, 20);
        assert!(
            above.contains("Every sanctioned skip site"),
            "...and it must still appear over a run that met its floor: {above}"
        );
    }

    /// The floor again, end to end through the real wrapper: a fake suite
    /// that prints two libtest `ok` lines passes a floor of 2 and fails a
    /// floor of 3.
    ///
    /// The unit test above asserts the rule; this asserts that the rule is
    /// wired to the thing that counts, which is the half a refactor breaks.
    ///
    /// **Every fixture here goes to a buffer, never to fd 1.** The lines below
    /// are shaped like libtest's own -- they have to be -- and a locked
    /// `Stdout` handle bypasses libtest's capture, so echoing them put five
    /// phantom `test ... ok` lines into `cargo test -p xtask`'s top-level
    /// stream, which the `skip-census` wrapping that suite in CI counted as
    /// tests it had executed. `sink` exists so this test cannot inflate the
    /// denominator of the census that measures it.
    #[test]
    fn the_wrapper_counts_what_the_suite_really_executed() {
        // `--show-output` rides inside the script, which is also how this
        // repository's own lever wraps a suite in `unshare`.
        let script = "printf 'test alpha ... ok\\ntest beta ... ok\\n'  # --show-output";
        let meets = argv(&["--min-tests", "2", "--", "sh", "-c", script]);
        let met = parse_census_args(&meets).expect("the flags parse");
        let mut echoed = Vec::new();
        run_census(&met, None, &mut echoed).expect("two executed tests meet a floor of two");
        let echoed = String::from_utf8(echoed).expect("the suite's output is text");
        assert!(
            echoed.contains("test alpha ... ok"),
            "the wrapper must still pass the suite's own output through: {echoed:?}"
        );

        let misses = argv(&["--min-tests", "3", "--", "sh", "-c", script]);
        let missed = parse_census_args(&misses).expect("the flags parse");
        let err = run_census(&missed, None, &mut Vec::new())
            .expect_err("two executed tests miss a floor of three");
        let text = format!("{err:#}");
        assert!(
            text.contains("census problem"),
            "the wrapper must FAIL, not merely report: {text}"
        );

        // ...and a PASSING test's own captured output does not inflate the
        // count. libtest prints it under a `---- <path> stdout ----` heading,
        // and this very test prints lines shaped like libtest's own; a census
        // that counted them would be padding its own denominator, which is
        // the defect one step removed from the one it exists to catch.
        let echo = "printf 'test alpha ... ok\\n---- alpha stdout ----\\ntest ghost ... ok\\n\
                    test result: ok.\\n'  # --show-output";
        let padded = argv(&["--min-tests", "2", "--", "sh", "-c", echo]);
        let padded = parse_census_args(&padded).expect("the flags parse");
        let mut suppressed = Vec::new();
        let err = run_census(&padded, None, &mut suppressed)
            .expect_err("the line inside alpha's captured block is not a test this run executed");
        assert!(
            format!("{err:#}").contains("executed 1 test(s)"),
            "one real test ran, not two: {err:#}"
        );
        let suppressed = String::from_utf8(suppressed).expect("the suite's output is text");
        assert!(
            !suppressed.contains("test ghost ... ok"),
            "a passing test's captured block is noise the census suppresses: {suppressed:?}"
        );
    }

    /// The wrapper writes the suite's stream to the sink it was HANDED, and
    /// nowhere else.
    ///
    /// This is the assertion the fix above rests on, and it is worth its own
    /// test because the failure it prevents is invisible from inside: the
    /// phantom lines only ever showed up in the output of a *different*
    /// process, the census wrapping `cargo test -p xtask`. A future edit that
    /// reintroduces a `println!` or a `stdout().lock()` in [`run_census`]
    /// leaves every other test here green.
    #[test]
    fn the_wrapper_echoes_to_its_sink_and_not_to_the_process_stdout() {
        let script = "printf 'test phantom_one ... ok\\ntest phantom_two ... ok\\n'  \
                      # --show-output";
        let argv = argv(&["--min-tests", "2", "--", "sh", "-c", script]);
        let options = parse_census_args(&argv).expect("the flags parse");
        let mut sink = Vec::new();
        run_census(&options, None, &mut sink).expect("the fake suite meets its floor");
        let sink = String::from_utf8(sink).expect("text");
        assert_eq!(
            sink.lines()
                .filter(|l| l.starts_with("test phantom_"))
                .count(),
            2,
            "both lines belong in the sink: {sink:?}"
        );
        // The source is what the assertion is really about: this file must
        // contain no second path to the real stdout for that stream.
        let here = std::fs::read_to_string(
            crate::workspace_root()
                .expect("the workspace root")
                .join("crates/xtask/src/skip_census.rs"),
        )
        .expect("this module");
        let body = here
            .split_once("fn run_census(")
            .and_then(|(_, rest)| rest.split_once("\n/// Everything a finished census"))
            .map(|(body, _)| body)
            .expect("run_census, up to the next item");
        // Read off the MASKED body, so the prose in this function's own
        // comments -- which discusses stdout constantly -- is not the thing
        // being asserted about. Whole words, so the `eprintln!` that streams
        // PROBLEMS to stderr is not mistaken for a write to stdout, and
        // `.stdout(` (the child's pipe, which is the point of the function)
        // is excluded by the dot before it.
        let masked = mask_code(body);
        for forbidden in ["println", "print", "stdout"] {
            let free: Vec<usize> = word_positions(&masked, forbidden)
                .into_iter()
                .filter(|at| !masked[..*at].ends_with('.'))
                .collect();
            assert!(
                free.is_empty(),
                "run_census contains `{forbidden}`, which writes past the sink to fd 1 -- \
                 where this crate's own fixture output becomes tests the outer census \
                 believes it executed"
            );
        }
    }

    /// `--min-tests 0` is refused rather than accepted.
    ///
    /// It parses, it reads like a floor and it is satisfied by a run that
    /// executed nothing -- an escape hatch shaped like compliance, which
    /// reinstates the exact defect the flag closes while leaving the diff
    /// looking compliant. There is deliberately no spelling of "no floor".
    #[test]
    fn a_floor_of_zero_is_refused_rather_than_being_an_escape_hatch() {
        let zero = argv(&["--min-tests", "0", "--", "true"]);
        let text = match parse_census_args(&zero) {
            Ok(_) => panic!("a floor of zero must be refused"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            text.contains("--min-tests 0 is refused"),
            "the refusal must say what it refused: {text}"
        );

        // ...and a missing floor is still refused, which is the same rule at
        // its other end.
        let none = argv(&["--", "true"]);
        let text = match parse_census_args(&none) {
            Ok(_) => panic!("a missing floor must be refused"),
            Err(e) => format!("{e:#}"),
        };
        assert!(text.contains("--min-tests"), "{text}");

        // A real floor parses, so the two refusals above are a rule about bad
        // input rather than a rule that rejects everything.
        let good = argv(&["--min-tests", "7", "--expect-self-marker", "--", "x"]);
        let ok = parse_census_args(&good).unwrap_or_else(|e| panic!("a real floor parses: {e:#}"));
        assert_eq!(ok.min_tests, 7);
        assert!(ok.expect_self_marker);
        assert_eq!(ok.argv, ["x"]);
    }

    /// `--expect-self-marker` fires only when it was asked for, and only when
    /// no planted marker arrived.
    #[test]
    fn the_self_marker_check_fires_only_on_a_census_that_saw_none() {
        let planted = parse_marker(&vitrin_skip::line(
            &vitrin_skip::GPU,
            "vitrin_skip::tests::x",
            "a planted marker",
        ))
        .expect("a marker");
        assert!(census_problems(&[], &[], 10, 1, true).len() == 1);
        assert!(census_problems(&[], std::slice::from_ref(&planted), 10, 1, true).is_empty());
        assert!(census_problems(&[], &[], 10, 1, false).is_empty());
    }

    /// Every inventory entry names a class that exists. A typo here would
    /// otherwise sanction nothing while looking like it sanctioned
    /// something.
    #[test]
    fn every_inventory_entry_names_a_real_class() {
        let known: BTreeSet<&str> = vitrin_skip::CLASSES.iter().map(|c| c.name).collect();
        for entry in INVENTORY {
            assert!(
                known.contains(entry.class),
                "INVENTORY entry for `{}` names unknown class `{}`",
                entry.test,
                entry.class
            );
            assert!(
                entry.why.len() > 30,
                "INVENTORY entry for `{}` has no real justification: {:?}",
                entry.test,
                entry.why
            );
            assert!(
                entry.test.contains("::"),
                "INVENTORY entry `{}` must be a full runtime path",
                entry.test
            );
        }
    }
}
