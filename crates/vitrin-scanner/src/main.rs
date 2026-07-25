// SPDX-License-Identifier: Apache-2.0
//! `vitrin-scanner`: reads `protocol/vitrin-v0.xml` and emits generated code.
//!
//! Usage:
//!
//! ```text
//! vitrin-scanner <input-xml> <rust-out-dir> [--c-header <path>]
//! ```
//!
//! - `<input-xml>` -- path to the protocol IDL (`protocol/vitrin-v0.xml`).
//! - `<rust-out-dir>` -- directory to (re)generate Rust modules into
//!   (conventionally `crates/vitrin-protocol/src/generated`); created if
//!   missing, and existing `*.rs` files in it are overwritten.
//! - `--c-header <path>` -- also (re)generate a single self-contained C
//!   header at `<path>` (conventionally `shim/include/vitrin-protocol.h`)
//!   from the same parsed IR, with marshal helpers for the wlroots shim.
//!   Optional: omit it to regenerate only the Rust side.
//!
//! One invocation can therefore produce both outputs:
//!
//! ```text
//! vitrin-scanner protocol/vitrin-v0.xml crates/vitrin-protocol/src/generated \
//!     --c-header shim/include/vitrin-protocol.h
//! ```
//!
//! This binary is a thin CLI wrapper; the actual parse/codegen logic lives in
//! the `vitrin_scanner` library (`src/lib.rs`) so `cargo xtask codegen` can
//! call it directly instead of shelling out to this binary.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

use vitrin_scanner::{c_gen, parse, rust_gen};

struct Args {
    input_xml: PathBuf,
    rust_out_dir: PathBuf,
    c_header: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut positional = Vec::new();
    let mut c_header = None;

    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--c-header" => {
                let path = raw.next().context("--c-header requires a path argument")?;
                c_header = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with("--") => {
                bail!("unknown flag '{other}'");
            }
            other => positional.push(other.to_string()),
        }
    }

    if positional.len() != 2 {
        print_usage();
        bail!(
            "expected 2 positional arguments (input-xml, rust-out-dir), got {}",
            positional.len()
        );
    }

    Ok(Args {
        input_xml: PathBuf::from(&positional[0]),
        rust_out_dir: PathBuf::from(&positional[1]),
        c_header,
    })
}

fn print_usage() {
    eprintln!("usage: vitrin-scanner <input-xml> <rust-out-dir> [--c-header <path>]");
    eprintln!();
    eprintln!("  <input-xml>       protocol/vitrin-v0.xml");
    eprintln!("  <rust-out-dir>    crates/vitrin-protocol/src/generated (created if missing)");
    eprintln!("  --c-header <path> also emit a self-contained C header, e.g.");
    eprintln!("                    shim/include/vitrin-protocol.h (optional)");
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("vitrin-scanner: error: {err:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;

    let xml = std::fs::read_to_string(&args.input_xml)
        .with_context(|| format!("reading {}", args.input_xml.display()))?;
    let protocol =
        parse::parse(&xml).with_context(|| format!("parsing {}", args.input_xml.display()))?;

    rust_gen::generate(&protocol, &args.rust_out_dir).with_context(|| {
        format!(
            "generating Rust modules into {}",
            args.rust_out_dir.display()
        )
    })?;
    eprintln!(
        "vitrin-scanner: wrote {} interface module(s) to {}",
        protocol.interfaces.len(),
        args.rust_out_dir.display()
    );

    if let Some(c_header) = &args.c_header {
        c_gen::generate(&protocol, c_header)
            .with_context(|| format!("generating C header at {}", c_header.display()))?;
        eprintln!("vitrin-scanner: wrote C header to {}", c_header.display());
    }

    Ok(())
}
