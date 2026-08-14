// SPDX-License-Identifier: Apache-2.0
//! The fixture binaries' own contract.
//!
//! This file has a second, load-bearing job beyond what it asserts: an
//! integration test is what makes Cargo build a package's binary targets
//! during `cargo test`. `crates/vitrin-core`'s checkpoint tests (P2.6.2,
//! issue #186) `exec` these binaries and locate them beside their own test
//! executable, so without an integration test here `cargo test --workspace`
//! would not produce them at all and those tests would fail to find a
//! program to exec. Do not delete this file without replacing the mechanism.
//!
//! What it asserts is the property that makes a fixture safe to keep in the
//! tree: **run by hand, each one refuses.** A deliberately broken confinement
//! helper that would happily produce an unconfined realm if somebody ran it
//! is not an instrument, it is a hazard.

use std::process::Command;

use vitrin_realm_init::PRE_EXEC_EXIT;

#[test]
fn a_fixture_run_without_a_config_channel_refuses() {
    for bin in [
        env!("CARGO_BIN_EXE_unshare-only-init"),
        env!("CARGO_BIN_EXE_leaks-a-dirfd-init"),
    ] {
        let out = Command::new(bin).output().expect("the fixture binary runs");
        assert_eq!(
            out.status.code(),
            Some(PRE_EXEC_EXIT),
            "{bin} did not refuse; stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_leaking_wrapper_does_not_carry_a_helper_of_its_own() {
    // `leaks-a-dirfd-init` is a wrapper that `execve`s the real, unmodified
    // `vitrin-realm-init` beside it. That is the property that makes the test
    // it serves a test of the shipped code path rather than of a copy: if it
    // ever grew its own mount table, the K13 assertion would be proving
    // something about the fixture.
    // Comments stripped first. A source-reading test that scans prose finds
    // the prose explaining the ban -- this file's own first draft failed on
    // the word `pivot_root` inside the wrapper's module docs.
    let source: String = include_str!("../src/bin/leaks-a-dirfd-init.rs")
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        source.contains("vitrin-realm-init"),
        "the wrapper must exec the real helper"
    );
    assert!(
        source.contains("execv"),
        "comment stripping ate the code: nothing here to check"
    );
    for forbidden in ["unshare(", "pivot_root", "MS_BIND"] {
        assert!(
            !source.contains(forbidden),
            "the wrapper has grown a confinement step of its own ({forbidden}); it is supposed \
             to leak one descriptor and get out of the way"
        );
    }
}
