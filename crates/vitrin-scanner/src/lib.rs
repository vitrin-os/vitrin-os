//! `vitrin-scanner` library: parses `protocol/vitrin-v0.xml` into an IR
//! ([`ir`], built by [`parse`]) and emits generated code from it ([`rust_gen`]
//! for `crates/vitrin-protocol/src/generated`, [`c_gen`] for
//! `shim/include/vitrin-protocol.h`).
//!
//! This is exposed as a library, in addition to the `vitrin-scanner` CLI
//! binary (`src/main.rs`), so `cargo xtask codegen` can call straight into
//! [`parse::parse`], [`rust_gen::generate`], and [`c_gen::generate`] as
//! ordinary function calls with `anyhow::Result` error propagation, rather
//! than shelling out to the built binary and scraping its exit code/stderr.
//! The binary itself is a thin CLI wrapper over the same functions.

pub mod c_gen;
pub mod casing;
pub mod gen_util;
pub mod ir;
pub mod parse;
pub mod rust_gen;
