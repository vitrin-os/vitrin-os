// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask`: repo-local developer tooling, aliased via
//! `.cargo/config.toml` (`xtask = "run --package xtask --"`).
//!
//! Subcommands:
//!
//! ```text
//! cargo xtask codegen           Regenerate crates/vitrin-protocol/src/generated
//!                                and shim/include/vitrin-protocol.h from
//!                                protocol/vitrin-v0.xml, in place. This is
//!                                what a human runs after editing the IDL --
//!                                review `git diff` and commit the result.
//!
//! cargo xtask codegen --check    Verify there is no drift between the IDL and
//!                                the checked-in generated files. Never
//!                                writes to the real, checked-in output
//!                                paths at all: it regenerates into an
//!                                isolated scratch directory under `target/`
//!                                and compares the result byte-for-byte
//!                                against the real paths' current on-disk
//!                                content, so the working tree is trivially
//!                                left exactly as found either way it exits.
//!                                This is what CI runs (P1.1.2 acceptance
//!                                criterion 2).
//!
//!                                Deliberately does not use `git diff`/`git
//!                                status` in any form -- comparing against
//!                                `HEAD` or the index is blind to a path
//!                                with no git history at all (as
//!                                crates/vitrin-protocol/src/generated and
//!                                shim/include/vitrin-protocol.h were on the
//!                                branch that introduced this check), so a
//!                                git-based diff would silently report "no
//!                                drift" no matter the content of such a
//!                                path. Direct filesystem comparison has no
//!                                such blind spot: it is correct whether the
//!                                real paths are untracked, staged, or
//!                                committed.
//!
//! cargo xtask bless [--filter S]  Regenerate the checked-in golden files --
//!                                the single documented entrypoint for it.
//!                                Drives the env-var-gated golden tests
//!                                (`VITRIN_REGEN_GOLDEN=1 cargo test -p
//!                                vitrin-core`) so every scattered golden --
//!                                the consent-prompt ink map, the SDK wire
//!                                bytes, the headless test-pattern image --
//!                                regenerates through one command that
//!                                produces a reviewable `git diff`. `--filter
//!                                S` narrows to golden tests whose name
//!                                contains `S` (default: every golden). See
//!                                `tests/golden/README.md`.
//!
//! cargo xtask session-matrix     Regenerate docs/book/src/session-app-matrix.md
//!                                from the corpus in src/session_matrix.rs
//!                                (WS-E.4.1, issue #221), in place.
//!
//! cargo xtask session-matrix --check
//!                                Verify the checked-in page is byte-identical
//!                                to what the generator emits, so a hand edit
//!                                to a GENERATED page is a red build. Reads
//!                                the page and compares in memory; writes
//!                                nothing, anywhere. This is what CI runs --
//!                                and it is the ONLY part of that page CI can
//!                                check, because a GitHub runner has no DRM
//!                                device, no seat and no GPU and therefore
//!                                cannot run a GUI application at all.
//!
//! cargo xtask isolation-matrix   Regenerate docs/book/src/isolation-matrix.md
//!                                (P2.6.3, issue #187): the Landlock ABI
//!                                matrix -- what this build REQUIRES of a
//!                                kernel's Landlock, one row per ABI rung,
//!                                each row naming the right it buys, what it
//!                                does NOT buy, and the published claim it
//!                                carries.
//!
//!                                Like session-matrix and unlike a probe, it
//!                                reads the repository and never the machine:
//!                                the ladder is PARSED out of
//!                                crates/vitrin-realm-init/src/landlock.rs and
//!                                the floor/ceiling out of that crate's
//!                                lib.rs, so the page is byte-identical on a
//!                                development box reporting Landlock ABI 9 and
//!                                on a CI runner reporting ABI 7. A probing
//!                                generator could not be, and `--check` would
//!                                then be red on every pull request. The
//!                                machine half stays `vitrind
//!                                --print-isolation`, which the page tells the
//!                                reader how to read against it.
//!
//! cargo xtask kernel-matrix     Regenerate docs/book/src/isolation-kernels.md
//!                                from the boot rows in tests/kernel-matrix/rows/
//!                                (issue #281), in place. Unlike every other
//!                                generator here it renders a MEASUREMENT: each
//!                                row is the shipped `vitrind`'s own output on a
//!                                pinned distribution kernel, booted under QEMU
//!                                by tests/kernel-matrix/collect.sh. This
//!                                command boots nothing and needs no network --
//!                                it only reads what that script wrote.
//!
//! cargo xtask kernel-matrix --check
//!                                Verify the checked-in page is what the
//!                                checked-in rows render to. This is what CI
//!                                runs, and it holds the PAGE to the ROWS.
//!                                Holding the ROWS to the KERNELS is
//!                                `tests/kernel-matrix/collect.sh --check`,
//!                                which needs qemu and runs in no pull request.
//!                                A green PR therefore proves the page matches
//!                                the measurement, and proves nothing about
//!                                whether the measurement is still current --
//!                                which is why every row carries a collection
//!                                date.
//!
//! cargo xtask isolation-matrix --check
//!                                Verify the checked-in page is byte-identical
//!                                to what the generator emits. Reads only.
//!                                This is what CI runs, and it goes red for
//!                                the reasons the page's own runbook lists:
//!                                a hand edit, a right moved to another rung,
//!                                a re-tuned ABI floor, a row with no
//!                                published claim, a published claim with no
//!                                row, or a cited sentence deleted from
//!                                docs/book/src/limits.md, README.md or
//!                                SECURITY.md.
//!
//! cargo xtask limits-check       Verify every published claim in the table in
//!                                src/limits.rs is (a) present on every surface
//!                                that must carry it and (b) still true of the
//!                                code (WS-E.4.4, issue #224) -- and (c) that
//!                                docs/plan/14-workstream-session-mode.md §6
//!                                and docs/book/src/limits.md enumerate the
//!                                same SET of limits, matched by an invisible
//!                                per-limit marker id rather than by wording,
//!                                because those two documents are written in
//!                                two registers on purpose. Reads only; writes
//!                                nothing. This is what CI runs.
//!
//!                                EXPLICITLY TEMPORARY: issue #172 owns the
//!                                choice of drift mechanism and has not made
//!                                it. See that module's docs for what replaces
//!                                this under each of #172's three options.
//!
//! cargo xtask verb-sets --check  Verify that every surface which ENUMERATES a
//!                                verb set -- the whole bitfield, the
//!                                facet-bearing verbs, the facetless ones, the
//!                                facet interfaces, the verbs this core does
//!                                not serve -- still lists the set the IDL
//!                                actually derives, and that its stated count
//!                                word matches. Reads only; writes nothing.
//!                                Also runs as a test, so `cargo test
//!                                --workspace` gates it.
//!
//!                                Exists because three consecutive reviews of
//!                                issue #196 found the same defect and nothing
//!                                else: one of those sets, written out in
//!                                prose, corrected in one place and left stale
//!                                in another. See the module docs for what it
//!                                deliberately does NOT catch.
//! ```
//!
//! Calls straight into the `vitrin_scanner` library (`parse`, `rust_gen`,
//! `c_gen`) rather than shelling out to the built `vitrin-scanner` binary --
//! no subprocess, no path-finding, ordinary `anyhow::Result` propagation.

use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

mod build_output;
mod isolation_matrix;
mod kernel_matrix;
mod limits;
mod session_matrix;
mod skip_census;
mod test_census;
mod verb_sets;
#[cfg(test)]
mod testtree;

/// Paths this task operates on, relative to the workspace root.
const XML_PATH: &str = "protocol/vitrin-v0.xml";
const RUST_OUT_DIR: &str = "crates/vitrin-protocol/src/generated";
const C_HEADER_PATH: &str = "shim/include/vitrin-protocol.h";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: error: {err:?}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> &'static str {
    "usage: cargo xtask codegen [--check]\n       cargo xtask demo [--headless] [--task K=V]...\n       cargo xtask bless [--filter SUBSTR]\n       cargo xtask session-matrix [--check]\n       cargo xtask isolation-matrix [--check]\n       cargo xtask kernel-matrix [--check]\n       cargo xtask limits-check [--tracker]\n       cargo xtask verb-sets [--check]\n       cargo xtask skip-scan\n       cargo xtask skip-census --min-tests N [--expect-self-marker] -- CMD [ARG...]"
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(subcommand) = args.first() else {
        bail!("missing subcommand\n\n{}", usage());
    };

    match subcommand.as_str() {
        "codegen" => {
            let mut check = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--check" => check = true,
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'codegen'\n\n{}", usage()),
                }
            }
            codegen(check)
        }
        "demo" => {
            let mut headless = false;
            // The task record the agent is handed. Collected here and
            // forwarded to `run_demo.py` VERBATIM: the demo's assertion is
            // computed from the supplied task at runtime, so this launcher
            // must not normalise, reorder or default it silently -- the
            // canonical string, and every receipt band, depends on the order
            // these arrive in.
            let mut task: Vec<String> = Vec::new();
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--headless" => headless = true,
                    "--task" => {
                        let Some(pair) = rest.next() else {
                            bail!("--task needs a K=V argument\n\n{}", usage());
                        };
                        if !pair.contains('=') {
                            bail!("--task {pair} is not of the form K=V\n\n{}", usage());
                        }
                        task.push(pair.clone());
                    }
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'demo'\n\n{}", usage()),
                }
            }
            demo(headless, &task)
        }
        "bless" => {
            let mut filter: Option<String> = None;
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--filter" => {
                        let value = rest.next().ok_or_else(|| {
                            anyhow::anyhow!("--filter needs a substring argument\n\n{}", usage())
                        })?;
                        filter = Some(value.clone());
                    }
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'bless'\n\n{}", usage()),
                }
            }
            bless(filter.as_deref())
        }
        "session-matrix" => {
            let mut check = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--check" => check = true,
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'session-matrix'\n\n{}", usage()),
                }
            }
            session_matrix::session_matrix(&workspace_root()?, check)
        }
        "isolation-matrix" => {
            let mut check = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--check" => check = true,
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!(
                        "unknown flag '{other}' for 'isolation-matrix'\n\n{}",
                        usage()
                    ),
                }
            }
            isolation_matrix::isolation_matrix(&workspace_root()?, check)
        }
        "kernel-matrix" => {
            let mut check = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--check" => check = true,
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'kernel-matrix'\n\n{}", usage()),
                }
            }
            kernel_matrix::kernel_matrix(&workspace_root()?, check)
        }
        "limits-check" => {
            // Two modes, and the second one is deliberately not the default.
            // `--tracker` shells out to `gh`, needs network and credentials,
            // and is ADVISORY: it reports and exits 0 whatever it finds. The
            // default mode is the offline gate CI runs; keeping them apart is
            // #172's answer to "an offline gate that pretends to check the
            // tracker is worse than none".
            let mut tracker = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    "--tracker" => tracker = true,
                    other => bail!("unknown flag '{other}' for 'limits-check'\n\n{}", usage()),
                }
            }
            let root = workspace_root()?;
            if tracker {
                return limits::tracker_report(&root);
            }
            limits::limits_check(&root)
        }
        "verb-sets" => {
            // Reads the IDL and the surfaces that enumerate its verb sets;
            // writes nothing. `--check` is accepted and is the only mode, so
            // the command reads like its siblings on a CI line; there is no
            // generator half, because no surface here is generated.
            for arg in &args[1..] {
                match arg.as_str() {
                    "--check" => {}
                    "-h" | "--help" => {
                        println!("{}", usage());
                        return Ok(());
                    }
                    other => bail!("unknown flag '{other}' for 'verb-sets'\n\n{}", usage()),
                }
            }
            let report = verb_sets::check(&workspace_root()?)?;
            println!("{report}");
            Ok(())
        }
        "skip-scan" => {
            // Reads sources, writes nothing -- one mode, like limits-check.
            if let Some(arg) = args.get(1) {
                if arg == "-h" || arg == "--help" {
                    println!("{}", usage());
                    return Ok(());
                }
                bail!("unknown flag '{arg}' for 'skip-scan'\n\n{}", usage());
            }
            skip_census::skip_scan(&workspace_root()?)
        }
        "skip-census" => {
            // The census's own flags come first and everything after the
            // `--` is the suite to run, verbatim: the jobs that use this run
            // different commands, and the wrapper must not normalise one into
            // the other. `--min-tests` is REQUIRED and parsed inside
            // `skip_census` so the refusal message can explain itself.
            let rest = &args[1..];
            if rest.first().is_some_and(|a| a == "-h" || a == "--help") {
                println!("{}", usage());
                return Ok(());
            }
            skip_census::skip_census(rest)
        }
        "-h" | "--help" => {
            println!("{}", usage());
            Ok(())
        }
        other => bail!("unknown subcommand '{other}'\n\n{}", usage()),
    }
}

/// This crate's own directory (`<repo>/crates/xtask`) is `CARGO_MANIFEST_DIR`,
/// baked in at compile time by Cargo -- resolving the workspace root from it
/// makes this independent of whatever directory `cargo xtask` happened to be
/// invoked from (unlike relying on the process's current directory).
fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("../..").canonicalize().with_context(|| {
        format!(
            "resolving workspace root from CARGO_MANIFEST_DIR={}",
            manifest_dir.display()
        )
    })
}

fn codegen(check: bool) -> Result<()> {
    let root = workspace_root()?;
    let xml_path = root.join(XML_PATH);
    let rust_out_dir = root.join(RUST_OUT_DIR);
    let c_header_path = root.join(C_HEADER_PATH);

    let xml =
        fs::read_to_string(&xml_path).with_context(|| format!("reading {}", xml_path.display()))?;
    let protocol = vitrin_scanner::parse::parse(&xml)
        .with_context(|| format!("parsing {}", xml_path.display()))?;

    if check {
        return check_no_drift(&root, &protocol, &rust_out_dir, &c_header_path);
    }

    // Plain `codegen`: regenerate both outputs in place, unconditionally --
    // a human runs this after editing the IDL, reviews `git diff`, and
    // commits the result.
    vitrin_scanner::rust_gen::generate(&protocol, &rust_out_dir)
        .with_context(|| format!("generating Rust modules into {}", rust_out_dir.display()))?;
    vitrin_scanner::c_gen::generate(&protocol, &c_header_path)
        .with_context(|| format!("generating C header at {}", c_header_path.display()))?;
    eprintln!("xtask: regenerated {RUST_OUT_DIR} and {C_HEADER_PATH}");
    eprintln!("xtask: codegen complete -- review `git diff` and commit.");
    Ok(())
}

/// Scratch directory `codegen --check` regenerates into -- entirely separate
/// from the real, checked-in output paths, and never written to by anything
/// else. Deliberately placed inside `target/` (already gitignored) rather
/// than under the OS temp directory: `rustfmt` discovers `rustfmt.toml` by
/// walking up from the *target file's* directory (see
/// `rust_gen::write_formatted`'s doc comment), and a path under the repo's
/// own `target/` walks up to the same repo-root `rustfmt.toml` that the real
/// in-place generation finds -- so scratch output is formatted identically
/// to a real run for reasons beyond just the explicit
/// `--config reorder_modules=false` override, keeping the comparison below
/// free of false positives from formatting differences alone.
const CHECK_SCRATCH_DIR: &str = "target/xtask-codegen-check";

/// `codegen --check`: verify that the checked-in generated files
/// (`real_rust_out_dir`, `real_c_header_path`) exactly match what `protocol`
/// currently generates -- without ever writing to those real paths.
///
/// This deliberately does not use `git diff`/`git status` in any form.
/// `git diff --exit-code HEAD -- <path>` (the previous implementation) is
/// structurally blind to a path with no git history at all: an untracked
/// file is simply absent from both sides of a `HEAD` comparison, so that
/// command reports "no difference" no matter the file's content. On the
/// branch that introduced this check, `real_rust_out_dir` and
/// `real_c_header_path` are exactly such paths -- their first introduction
/// to the repo -- so a git-based diff would silently pass regardless of
/// whether the checked-in files actually matched the IDL. Comparing freshly
/// generated bytes directly against whatever currently sits on disk sidesteps
/// git's tracking state entirely: it is correct whether the real paths are
/// untracked, staged, or committed, both today and in every future PR.
///
/// Implementation: regenerate into an isolated scratch directory under
/// `target/` (see [`CHECK_SCRATCH_DIR`]), then compare file-by-file against
/// the real paths' current on-disk content. This also makes "leaves the
/// working tree exactly as found either way it exits" trivially true --
/// there is nothing to restore, because the real paths are never written to
/// during `--check` in the first place.
fn check_no_drift(
    root: &Path,
    protocol: &vitrin_scanner::ir::Protocol,
    real_rust_out_dir: &Path,
    real_c_header_path: &Path,
) -> Result<()> {
    let scratch_root = root.join(CHECK_SCRATCH_DIR);
    // Purge any leftover scratch state from a previous `--check` run first:
    // otherwise a file some earlier XML revision generated (e.g. for an
    // interface since removed) could linger here and be mistaken for part of
    // *this* run's freshly generated, ground-truth output below.
    if scratch_root.exists() {
        fs::remove_dir_all(&scratch_root).with_context(|| {
            format!(
                "clearing stale scratch directory {}",
                scratch_root.display()
            )
        })?;
    }
    let scratch_rust_out_dir = scratch_root.join("generated");
    let scratch_c_header_path = scratch_root.join("vitrin-protocol.h");

    vitrin_scanner::rust_gen::generate(protocol, &scratch_rust_out_dir).with_context(|| {
        format!(
            "generating Rust modules into scratch directory {}",
            scratch_rust_out_dir.display()
        )
    })?;
    vitrin_scanner::c_gen::generate(protocol, &scratch_c_header_path).with_context(|| {
        format!(
            "generating C header into scratch path {}",
            scratch_c_header_path.display()
        )
    })?;

    let mut drift = Vec::new();
    diff_dir_trees(
        &scratch_rust_out_dir,
        real_rust_out_dir,
        RUST_OUT_DIR,
        &mut drift,
    )?;
    diff_single_file(
        &scratch_c_header_path,
        real_c_header_path,
        C_HEADER_PATH,
        &mut drift,
    )?;

    // Scratch output has served its purpose. Clean it up so it doesn't
    // linger (harmless either way -- `target/` is gitignored -- but tidier,
    // and it means the next run's staleness check above is a no-op).
    let _ = fs::remove_dir_all(&scratch_root);

    if drift.is_empty() {
        eprintln!(
            "xtask: codegen --check: no drift -- generated files match protocol/vitrin-v0.xml"
        );
        return Ok(());
    }

    eprintln!("xtask: codegen --check: drift detected --");
    for line in &drift {
        eprintln!("  {line}");
    }
    bail!(
        "codegen --check: generated files have drifted from protocol/vitrin-v0.xml ({} file(s) \
         listed above differ from what codegen currently produces, under {RUST_OUT_DIR} or \
         {C_HEADER_PATH}). Run `cargo xtask codegen` and commit the result.",
        drift.len()
    );
}

/// Recursively compare two directory trees for byte-for-byte equality,
/// appending one human-readable line per difference (missing, extra, or
/// changed file) to `out`. `expected_root` is freshly generated output
/// (ground truth, from this run's parsed `protocol`); `actual_root` is
/// whatever currently sits on disk at the real, checked-in path, regardless
/// of git state.
fn diff_dir_trees(
    expected_root: &Path,
    actual_root: &Path,
    label: &str,
    out: &mut Vec<String>,
) -> Result<()> {
    let mut expected_files = BTreeMap::new();
    collect_files(expected_root, expected_root, &mut expected_files)
        .with_context(|| format!("walking freshly generated {}", expected_root.display()))?;
    let mut actual_files = BTreeMap::new();
    collect_files(actual_root, actual_root, &mut actual_files)
        .with_context(|| format!("walking checked-in {}", actual_root.display()))?;

    for (rel, expected_path) in &expected_files {
        match actual_files.get(rel) {
            None => out.push(format!(
                "{label}/{rel}: missing on disk (codegen would generate this file)"
            )),
            Some(actual_path) => {
                let expected_bytes = fs::read(expected_path)
                    .with_context(|| format!("reading {}", expected_path.display()))?;
                let actual_bytes = fs::read(actual_path)
                    .with_context(|| format!("reading {}", actual_path.display()))?;
                if expected_bytes != actual_bytes {
                    out.push(format!(
                        "{label}/{rel}: on-disk content differs from fresh codegen output"
                    ));
                }
            }
        }
    }
    for rel in actual_files.keys() {
        if !expected_files.contains_key(rel) {
            out.push(format!(
                "{label}/{rel}: present on disk but codegen no longer produces this file"
            ));
        }
    }
    Ok(())
}

/// Recursively collect every regular file under `dir` into `out`, keyed by
/// its path relative to `root` with `/`-separated components regardless of
/// platform (a stable map key). A missing `dir` (e.g. the real output
/// directory not existing at all yet) yields an empty map rather than an
/// error -- that absence is itself reported as drift by the caller (via the
/// "missing on disk" branch in [`diff_dir_trees`]), not by failing here.
fn collect_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in
        fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("reading a directory entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat-ing {}", path.display()))?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if !file_type.is_file() {
            // A symlink (or fifo, socket, ...) in a generated-output tree is
            // never something codegen produces; silently skipping it would
            // make the drift check blind to whatever it points at (or
            // shadows). Fail loudly instead.
            bail!(
                "unexpected non-regular-file entry {} while walking generated output \
                 (symlinks are not produced by codegen and are not compared)",
                path.display()
            );
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("path was walked from under root, so root must be a prefix of it")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.insert(rel, path);
        }
    }
    Ok(())
}

/// Compare a single freshly generated file against the real, checked-in
/// path's current on-disk content (which may not exist at all), appending a
/// human-readable line to `out` iff they differ.
fn diff_single_file(
    expected_path: &Path,
    actual_path: &Path,
    label: &str,
    out: &mut Vec<String>,
) -> Result<()> {
    let expected_bytes = fs::read(expected_path)
        .with_context(|| format!("reading freshly generated {}", expected_path.display()))?;
    match fs::read(actual_path) {
        Ok(actual_bytes) => {
            if expected_bytes != actual_bytes {
                out.push(format!(
                    "{label}: on-disk content differs from fresh codegen output"
                ));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            out.push(format!(
                "{label}: missing on disk (codegen would generate this file)"
            ));
        }
        Err(err) => {
            return Err(err).with_context(|| format!("reading {}", actual_path.display()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `cargo xtask bless [--filter SUBSTR]` -- the single golden-regen entrypoint
// ---------------------------------------------------------------------------

/// Default test-name filter: every golden test in the repo has "golden" in
/// its name, so this regenerates all of them (the consent-prompt ink map, the
/// SDK wire bytes, and the headless test-pattern image) in one pass.
const BLESS_DEFAULT_FILTER: &str = "golden";

/// `cargo xtask bless`: the one documented way to regenerate the checked-in
/// golden files. Every golden test reads its committed file and, when
/// `VITRIN_REGEN_GOLDEN` is set, rewrites it first; this drives exactly those
/// tests with that variable set so the goldens are regenerated uniformly and
/// the result is a plain `git diff` a reviewer can read. Nothing here knows
/// the golden formats -- the tests own that -- so a new golden test is covered
/// automatically as long as its name matches the filter.
fn bless(filter: Option<&str>) -> Result<()> {
    let root = workspace_root()?;
    let name_filter = filter.unwrap_or(BLESS_DEFAULT_FILTER);

    // Reuse the same `cargo` that launched this xtask (honors any toolchain
    // override); fall back to the bare name if the env var is somehow unset.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    eprintln!(
        "xtask bless: regenerating goldens via `VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core -- {name_filter}`"
    );
    let status = Command::new(&cargo)
        .current_dir(&root)
        .args(["test", "-p", "vitrin-core", "--", name_filter])
        .env("VITRIN_REGEN_GOLDEN", "1")
        .status()
        .with_context(|| {
            format!(
                "spawning {} to run the golden tests",
                cargo.to_string_lossy()
            )
        })?;

    if !status.success() {
        bail!(
            "the golden tests failed under regeneration ({status}); goldens may be partially \
             rewritten -- inspect `git diff` before committing"
        );
    }

    eprintln!(
        "xtask bless: done. Regenerated every golden whose test name matches '{name_filter}' \
         -- review `git diff` under tests/golden/ and crates/*/tests/golden/, then commit."
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// `cargo xtask demo [--headless]` -- the P1.8.4/P1.8.7 demo agent, rewired
// onto the real per-app shim (issue #110): every venue runs
// vitrind -> vitrin-shim -> a real app, never vitrin-mock-shim. This is
// M1.5's named acceptance gate.
// ---------------------------------------------------------------------------

/// The demo identity and its pre-shared token. Must match
/// `examples/principals.toml` and `run_demo.py`'s `DEMO_IDENTITY`/`DEMO_TOKEN`:
/// the launcher writes the registry, the agent presents the credential, and
/// the R6 auto-approve guard is only satisfied because the registry written
/// below holds *nothing but* this one row.
const DEMO_IDENTITY: &str = "vitrin://local/agent/demo";
const DEMO_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Headless virtual-output size. Large enough for `weston-terminal`'s chrome
/// to lay out in (matches `tests/integration/test_real_app.py`'s
/// `REALM_SIZE`), small enough to keep captures cheap.
const HEADLESS_SIZE: &str = "640x480";

/// The headless / pure-software render selectors the real C shim's wlroots
/// backend needs -- CI (and most developer machines running the headless
/// venue) has no GPU. The shim is *always* internally headless (`shim/README.md`:
/// "It uses the headless backend and never touches real hardware"), so these
/// apply in the nested venue too, not only headless. Reaches the shim only
/// through the realm's `env_allow` (the one route a realm's environment may
/// grow by), seeded into the core's own environment for the allowlist to copy
/// from -- mirroring `tests/integration/harness.py`'s `WLR_ENV` /
/// `test_real_app.py` exactly, so the demo and the integration suite can never
/// disagree about what the shim needs to render without a GPU.
const WLR_ENV: [(&str, &str); 4] = [
    ("WLR_BACKENDS", "headless"),
    ("WLR_RENDERER", "pixman"),
    ("WLR_RENDERER_ALLOW_SOFTWARE", "1"),
    ("WLR_LIBINPUT_NO_DEVICES", "1"),
];

/// The headless venue's app: `form-target` (`shim/tests/form_target.c`), a
/// bare wl_shm + xdg-shell + wl_pointer + wl_keyboard client co-built with the
/// shim. It is the app the *goal-directed* demo needs and that no third-party
/// program provides: two locatable input fields, a locatable submit button,
/// per-field text accumulation, and -- on submit -- a receipt whose three band
/// colours are a pure function of the whole record it received, plus a
/// byte-exact `SUBMIT ... canon=<hex>` line on stdout.
///
/// Never `vitrin-mock-shim` (issue #110): the mock shim is a unit-test fixture
/// only, and must appear in no demo venue. `form-target` is not one -- it is a
/// real Wayland client -- but it IS repo-authored, which the previous headless
/// app (`weston-terminal`) was not. That trade is disclosed in
/// `examples/agent-demo/README.md` and in `docs/plan/01-phase-1-mvp.md`'s D12
/// seam table rather than left for a reader to notice.
///
/// Resolved as a sibling of the real shim binary, exactly as
/// `tests/integration/test_real_actuation.py` resolves `click-target`;
/// `VITRIN_DEMO_APP` overrides it.
const HEADLESS_APP: &str = "form-target";

/// How long the headless app stays up. It must outlive the whole agent flow
/// (locate/click/type per field, submit, receipt decode) plus the core's boot,
/// with room for a loaded CI runner; the core SIGTERMs it at teardown long
/// before this expires.
const HEADLESS_APP_RUN_MS: &str = "120000";

/// The task record the agent is handed when no `--task K=V` is supplied.
///
/// **Must name the same keys, in the same order, as `run_demo.py`'s
/// `TASK_DEFAULT`** -- this launcher passes the KEYS to the app (`--field
/// NAME`, so the app can build the same canonical string) while the agent
/// types the VALUES, so a disagreement would make the receipt unmatchable for
/// a reason that looks like a delivery failure.
/// `tests/integration/test_demo.py::DefaultTaskAgreesAcrossLaunchers` pins the
/// two together.
const DEFAULT_TASK: [(&str, &str); 2] = [("name", "Ada Lovelace"), ("email", "ada@example.org")];

/// Default Firefox ESR path for the nested venue. Overridable with
/// `VITRIN_DEMO_FIREFOX` because the binary's name and location vary by distro
/// (`/usr/bin/firefox-esr` on Debian, `/usr/bin/firefox` elsewhere). The realm
/// loader refuses a relative name, so this is always absolute.
const DEFAULT_FIREFOX: &str = "/usr/bin/firefox-esr";

/// `cargo xtask demo`: launch the shipped core and drive the demo agent
/// against it, in whichever venue was selected.
///
/// `task` is the raw `K=V` strings from the command line, in order, empty when
/// none were given. They are forwarded to the agent verbatim; only the KEYS are
/// interpreted here, and only to tell the headless app its field names.
fn demo(headless: bool, task: &[String]) -> Result<()> {
    let root = workspace_root()?;
    let bin_dir = binary_dir()?;
    let vitrind = bin_dir.join("vitrind");
    if !vitrind.is_file() {
        bail!(
            "vitrind not found at {} -- run `cargo build --workspace` first",
            vitrind.display()
        );
    }
    // The confinement helper is a **pair** with the core, not an optional
    // extra (P2.6.2, #186). It defaults to a sibling of `vitrind`, its version
    // must match the core's exactly, and `--isolation` defaults to `default`
    // -- so a `vitrind` without it beside it refuses every spawn. Checked here
    // rather than discovered at the fork, because "the demo could not launch
    // its realm" is a much worse error message than this one, and because
    // `cargo build --workspace` produces both binaries in one step: an absence
    // means a partial build, never a missing dependency.
    let realm_init = bin_dir.join("vitrin-realm-init");
    if !realm_init.is_file() {
        bail!(
            "vitrin-realm-init not found at {} -- it is built alongside vitrind by \
             `cargo build --workspace`, and at --isolation=default the core execs it for every \
             realm it spawns. A vitrind from one build beside a helper from another is refused \
             by the version handshake rather than run, so the two must be installed together",
            realm_init.display()
        );
    }

    let work = make_work_dir().context("creating the demo's throwaway runtime directory")?;
    let principals = work.join("principals.toml");
    let realm = work.join("realm.toml");
    let recorder = work.join("flight.jsonl");
    let frames = work.join("frames");

    // One-principal registry, mode 0600: the exact shape the R6 auto-approve
    // guard requires (and refuses any wider mode of).
    fs::write(
        &principals,
        format!("[[principal]]\nidentity = \"{DEMO_IDENTITY}\"\ntoken = \"{DEMO_TOKEN}\"\n"),
    )
    .with_context(|| format!("writing {}", principals.display()))?;
    fs::set_permissions(&principals, Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {}", principals.display()))?;

    // The core-inserted shim (issue #103): the core execs a `--shim` binary
    // that holds fd 3 and conveys the realm's `command` app in argv; that
    // binary then fork/execs the app inside its own private Wayland socket
    // (issue #104/#105). This is the REAL wlroots shim in both venues -- the
    // headless venue used to alias `vitrin-mock-shim` as both the `--shim`
    // binary and the realm's `command` (an animated buffer standing in for
    // the app), and the nested venue passed real Firefox as `command` with
    // NO shim in between at all, which the core cannot actually run (issue
    // #110: "the core execs it as a shim on fd 3 with an unbound
    // WAYLAND_DISPLAY -- structurally impossible"). Neither is true anymore:
    // `vitrin-mock-shim` appears in no venue below.
    let shim_bin = resolve_shim_bin(&root)?;

    // Assemble the two venue-specific pieces: the realm config, the core's
    // argv, the environment vitrind runs under, and where its socket lands.
    // The real shim is ALWAYS internally headless (`shim/README.md`), in
    // both venues, so `WLR_ENV` is set on the core's own environment here --
    // the source each venue's realm `env_allow` below copies it from -- and
    // every venue passes the real shim as `--shim`.
    let mut core_cmd = Command::new(&vitrind);
    core_cmd.env("RUST_LOG", "info");
    core_cmd.args(["--shim".as_ref(), shim_bin.as_os_str()]);
    for (name, value) in WLR_ENV {
        core_cmd.env(name, value);
    }

    let socket: PathBuf = if headless {
        // A real, trivial Wayland client -- never the mock shim -- run
        // through the real C shim. `env_allow` is the only route a realm's
        // environment may grow by, so it must name exactly the WLR_* set the
        // shim's software-render backend needs (mirrors
        // `tests/integration/test_real_app.py`'s `WLR_ENV`/`env_allow`).
        let app_bin = resolve_headless_app(&shim_bin)?;
        // `form-target` is told the field NAMES so it can build the same
        // canonical string the agent computes its expected receipt from. The
        // VALUES are never passed here -- the only way they can reach the app
        // is the agent typing them through the real chokepoint, which is the
        // whole point of the demo.
        let mut app_args: Vec<String> =
            vec!["--run-ms".to_string(), HEADLESS_APP_RUN_MS.to_string()];
        for key in task_keys(task) {
            app_args.push("--field".to_string());
            app_args.push(key);
        }
        fs::write(
            &realm,
            format!(
                "[[realm]]\nid = \"realm-0\"\ncommand = \"{}\"\nargs = {}\n\
                 env_allow = {}\n",
                app_bin.display(),
                toml_string_array(app_args.iter().map(String::as_str)),
                toml_string_array(WLR_ENV.iter().map(|(name, _)| *name)),
            ),
        )
        .with_context(|| format!("writing {}", realm.display()))?;

        // Headless owns its runtime tree, so the socket is deterministic.
        core_cmd
            .args([
                "--headless",
                "--size",
                HEADLESS_SIZE,
                "--consent=auto-approve",
            ])
            .args(["--principals".as_ref(), principals.as_os_str()])
            .args(["--realm".as_ref(), realm.as_os_str()])
            .args(["--recorder".as_ref(), recorder.as_os_str()])
            .env("XDG_RUNTIME_DIR", &work);
        work.join("vitrin-0").join("core.sock")
    } else {
        // Nested: a real Firefox ESR, run through the real C shim exactly as
        // `tests/integration/test_real_firefox.py` and `shim/docs/firefox.md`
        // §5's nested walkthrough do -- fresh per-run profile, software
        // WebRender (the shim's Wayland surface has no GPU path into it yet;
        // issue #117 tracks dmabuf zero-copy), no crash reporter, no a11y
        // bus. Correct-by-construction; a machine with a display and a
        // browser runs it, CI never does.
        let firefox = std::env::var_os("VITRIN_DEMO_FIREFOX")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_FIREFOX));
        let profile = work.join("firefox-profile");
        fs::create_dir_all(&profile).with_context(|| format!("creating {}", profile.display()))?;

        // `--no-remote` matters here, not just hygiene: without it a
        // `firefox --new-window` on a machine that already has Firefox
        // running would hand the window to that ALREADY-RUNNING host
        // instance over its own remoting protocol -- never touching the
        // shim's confined process at all, silently defeating the whole
        // rewire this task exists to do. `env_allow` copies NAMES from
        // vitrind's own environment (the only route a realm's environment
        // grows by): the Wayland/session names below, the Firefox
        // software-render names this function sets on `core_cmd`, and the
        // WLR_* names the shim itself needs.
        let firefox_env: [(&str, &str); 5] = [
            ("MOZ_ACCELERATED", "0"),
            ("LIBGL_ALWAYS_SOFTWARE", "1"),
            ("MOZ_CRASHREPORTER_DISABLE", "1"),
            ("GTK_A11Y", "none"),
            ("NO_AT_BRIDGE", "1"),
        ];
        let env_allow = toml_string_array(
            [
                "HOME",
                "LANG",
                "XDG_SESSION_TYPE",
                "MOZ_ENABLE_WAYLAND",
                "GDK_BACKEND",
                "DBUS_SESSION_BUS_ADDRESS",
            ]
            .into_iter()
            .chain(firefox_env.iter().map(|(name, _)| *name))
            .chain(WLR_ENV.iter().map(|(name, _)| *name)),
        );
        fs::write(
            &realm,
            format!(
                "[[realm]]\nid = \"realm-0\"\ncommand = \"{}\"\n\
                 args = [\"--profile\", \"{}\", \"--no-remote\", \"--new-window\", \"about:blank\"]\n\
                 env_allow = {env_allow}\n",
                firefox.display(),
                profile.display(),
            ),
        )
        .with_context(|| format!("writing {}", realm.display()))?;

        // Nested draws its window on the host compositor, so vitrind keeps the
        // host session's environment (its WAYLAND_DISPLAY, its XDG_RUNTIME_DIR)
        // and the socket lands under that runtime dir. The two Wayland vars
        // and the Firefox render selectors are injected here so env_allow
        // above has something to copy.
        let xdg = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            anyhow::anyhow!(
                "nested demo needs XDG_RUNTIME_DIR set (the host session's runtime dir, \
                 where vitrind binds its socket and finds the host compositor)"
            )
        })?;
        core_cmd
            .arg("--nested")
            .args(["--principals".as_ref(), principals.as_os_str()])
            .args(["--realm".as_ref(), realm.as_os_str()])
            .args(["--recorder".as_ref(), recorder.as_os_str()])
            .env("MOZ_ENABLE_WAYLAND", "1")
            .env("GDK_BACKEND", "wayland");
        for (name, value) in firefox_env {
            core_cmd.env(name, value);
        }
        PathBuf::from(xdg).join("vitrin-0").join("core.sock")
    };

    eprintln!(
        "xtask demo: launching vitrind ({} venue); socket {}",
        if headless { "headless" } else { "nested" },
        socket.display()
    );
    let mut core = core_cmd
        .spawn()
        .with_context(|| format!("spawning {}", vitrind.display()))?;

    // Wait for the socket, or explain why it will never appear.
    if let Err(err) = await_core_socket(&socket, &mut core, Duration::from_secs(30)) {
        terminate(&mut core);
        return Err(err);
    }

    // Run the demo agent. `python3` off the SDK's source path; no venv, no
    // install -- the SDK is stdlib-only (D8), like the integration suite.
    let demo_py = root.join("examples/agent-demo/run_demo.py");
    let pythonpath = root.join("sdk/python/src");
    let mut demo_cmd = Command::new("python3");
    demo_cmd
        .arg(&demo_py)
        .args(["--socket".as_ref(), socket.as_os_str()])
        .args(["--out".as_ref(), frames.as_os_str()])
        .args(["--recorder".as_ref(), recorder.as_os_str()])
        .env("PYTHONPATH", &pythonpath);
    if headless {
        demo_cmd.args(["--headless", "--consent", "auto-approve"]);
    } else {
        demo_cmd.args(["--consent", "interactive"]);
    }
    // Forwarded VERBATIM, in order. The agent's assertion is computed from the
    // task it is handed, at runtime, so this launcher must not touch it: a
    // reordering here would silently change every expected receipt band.
    for pair in task {
        demo_cmd.args(["--task", pair]);
    }
    let status = match demo_cmd.status() {
        Ok(status) => status,
        Err(err) => {
            // Reap the core we already spawned before propagating, so a failure
            // to launch the demo agent (e.g. `python3` off PATH) never leaks a
            // live vitrind + shim -- same guard as the socket-wait branch.
            terminate(&mut core);
            return Err(err).with_context(|| format!("running {}", demo_py.display()));
        }
    };

    // Tear the core down the clean way (SIGTERM), so the recorder's footer is
    // written and the log names this run's end.
    terminate(&mut core);

    eprintln!("xtask demo: flight recorder at {}", recorder.display());
    if status.success() {
        eprintln!("xtask demo: PASS");
        Ok(())
    } else {
        bail!(
            "demo agent exited with {} (frames under {}, recorder {})",
            status,
            frames.display(),
            recorder.display()
        );
    }
}

/// The directory holding the sibling `vitrind` binary: this `xtask`
/// executable's own directory. Resolving from `current_exe` honors
/// `CARGO_TARGET_DIR` and whatever profile built us, without guessing
/// `target/debug` from the workspace root.
fn binary_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the running xtask binary")?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("xtask binary {} has no parent directory", exe.display()))
}

/// A TOML inline array of basic string literals, for a realm's `env_allow` or
/// `args`. Mirrors `tests/integration/harness.py`'s `_toml_string_array`,
/// including its escaping: the render-selector names this used to be the only
/// caller for are fixed literals, but `args` now carries a task's field NAMES
/// straight off the command line, and those can contain anything a shell will
/// pass. An unescaped quote there would produce a realm file the loader
/// (`crates/vitrin-core/src/toml_subset.rs`) reads as something else entirely.
fn toml_string_array<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let joined = names
        .into_iter()
        .map(|name| format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{joined}]")
}

/// The field NAMES of the supplied task, in order, or [`DEFAULT_TASK`]'s keys
/// when none were supplied. Only the part before the first `=` is a key: a
/// value may contain `=` freely, and `run_demo.py`'s `parse_task` splits on the
/// FIRST one for exactly that reason, so this must too.
fn task_keys(task: &[String]) -> Vec<String> {
    if task.is_empty() {
        return DEFAULT_TASK
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
    }
    task.iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(key, _)| key)
                .unwrap_or(pair)
                .to_string()
        })
        .collect()
}

/// True if `path` names a regular, executable file.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// Resolve the real per-app Wayland shim (issue #103/#104): a wlroots
/// compositor, Meson-built outside the Cargo workspace, that the core execs
/// to hold fd 3 and which fork/execs the realm's `command` app inside its own
/// private Wayland socket. `VITRIN_C_SHIM_BIN` overrides the path -- the same
/// variable name `tests/integration/harness.py`'s real-app gates and
/// `crates/vitrin-core/src/shim.rs`'s cross-track conformance test use --
/// defaulting to the checked-in build tree's usual output path. Never
/// `vitrin-mock-shim`: that binary is a unit-test fixture only (issue #110).
fn resolve_shim_bin(root: &Path) -> Result<PathBuf> {
    let path = std::env::var_os("VITRIN_C_SHIM_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("shim/build/vitrin-shim"));
    if !is_executable(&path) {
        bail!(
            "real vitrin-shim not found at {} -- build it (`meson setup shim/build shim && \
             meson compile -C shim/build`) or point VITRIN_C_SHIM_BIN at an already-built one",
            path.display()
        );
    }
    Ok(path)
}

/// Resolve the headless venue's app: [`HEADLESS_APP`] built beside the real
/// shim, or an explicit `VITRIN_DEMO_APP`.
///
/// Not a `PATH` search any more, and that is the substantive change: the
/// goal-directed demo's app is `form-target`, which is co-built with the shim
/// (`shim/meson.build`) rather than installed by a distro, so its absence
/// beside a built shim is a build misconfiguration to report -- exactly how
/// `tests/integration/test_real_actuation.py` resolves `click-target`.
fn resolve_headless_app(shim_bin: &Path) -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("VITRIN_DEMO_APP") {
        let path = PathBuf::from(&value);
        if is_executable(&path) {
            return Ok(path);
        }
        bail!(
            "VITRIN_DEMO_APP={} does not name an executable file",
            path.display()
        );
    }
    let dir = shim_bin
        .canonicalize()
        .unwrap_or_else(|_| shim_bin.to_path_buf())
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("the shim path {} has no parent", shim_bin.display()))?;
    let sibling = dir.join(HEADLESS_APP);
    if is_executable(&sibling) {
        return Ok(sibling);
    }
    bail!(
        "no {HEADLESS_APP} beside the C shim ({}) -- it is co-built with the shim \
         (shim/meson.build), so rebuild it (`meson setup shim/build shim && meson compile \
         -C shim/build`) or set VITRIN_DEMO_APP to an absolute path",
        dir.display()
    );
}

/// A fresh 0700 runtime directory for one demo run, under the OS temp dir. The
/// pid plus a monotonic-ish suffix keeps concurrent runs apart without a
/// randomness dependency.
fn make_work_dir() -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("vitrin-demo-{}-{stamp}", std::process::id()));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Block until `socket` exists, or the core exits first, or the deadline
/// passes. Polls rather than sleeping a fixed interval: the boot path does an
/// R6 audit, a realm load, a flock, a recorder create and a fork, any of which
/// can be slow on a loaded machine.
fn await_core_socket(socket: &Path, core: &mut Child, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = core.try_wait().context("polling vitrind while it boots")? {
            bail!(
                "vitrind exited with {status} during boot, before binding {}",
                socket.display()
            );
        }
        if socket.exists() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "vitrind never bound {} within {:?}",
                socket.display(),
                timeout
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the core with SIGTERM (never SIGKILL): SIGTERM is the path that runs
/// the shutdown ladder and writes the recorder's `run_ended` footer. If it is
/// already gone, this is a no-op. Falls back to a forceful `kill()` only if
/// the process refuses to leave within a grace window.
fn terminate(core: &mut Child) {
    if let Ok(Some(_)) = core.try_wait() {
        return; // already exited
    }
    if let Some(pid) = rustix::process::Pid::from_raw(core.id() as i32) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match core.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = core.kill();
                let _ = core.wait();
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return,
        }
    }
}
