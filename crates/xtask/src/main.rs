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
//! ```
//!
//! Calls straight into the `vitrin_scanner` library (`parse`, `rust_gen`,
//! `c_gen`) rather than shelling out to the built `vitrin-scanner` binary --
//! no subprocess, no path-finding, ordinary `anyhow::Result` propagation.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

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
    "usage: cargo xtask codegen [--check]"
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
