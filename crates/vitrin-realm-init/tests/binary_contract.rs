// SPDX-License-Identifier: MPL-2.0
//! The `vitrin-realm-init` **binary**'s own contract.
//!
//! This file has a second, load-bearing job beyond what it asserts: an
//! integration test is what makes Cargo build a package's binary targets
//! during `cargo test`. `crates/vitrin-core`'s confinement tests (P2.6.2,
//! issue #186) really `exec` this binary and locate it beside their own test
//! executable, so without an integration test here `cargo test --workspace`
//! would compile the helper only in test mode and every confined-spawn test
//! would fail to find a program to exec. That is not a hypothetical: CI's
//! `rust` job runs `cargo test --workspace` with no preceding `cargo build`,
//! and `cargo clippy --all-targets` does not link binaries -- so the whole
//! confinement suite was red on a clean checkout until this file existed.
//! If this file is ever deleted, delete the confined-spawn tests with it --
//! or replace this mechanism first.
//!
//! The same mechanism, for the same reason, is why
//! `crates/vitrin-mock-shim/tests/binary_contract.rs` and
//! `crates/vitrin-realm-init-fixtures/tests/binary_contract.rs` exist.

use std::io::Write;
use std::process::{Command, Stdio};

use vitrin_realm_init::PRE_EXEC_EXIT;

const BIN: &str = env!("CARGO_BIN_EXE_vitrin-realm-init");

#[test]
fn run_by_hand_it_refuses_rather_than_confining_something() {
    // The helper's entire input is the `CONFIG` frame the core sends down the
    // seqpacket on fd 0. Run by hand -- stdin is `/dev/null`, as `.output()`
    // leaves it -- there is no channel, so it must stop before it unshares
    // anything, and stop with the reserved pre-exec status rather than with a
    // status the core could mistake for the shim's own.
    let out = Command::new(BIN).output().expect("the helper binary runs");
    assert_eq!(
        out.status.code(),
        Some(PRE_EXEC_EXIT),
        "stdout: {:?} stderr: {:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_channel_that_is_not_the_core_is_refused_before_the_unshare() {
    // A pipe on fd 0 gets past the `F_DUPFD_CLOEXEC` but is not a socket, so
    // the very first `recv` fails. What matters is that it fails *at the
    // version stage* -- before `unshare` -- and leaves the reserved status:
    // a helper that unshared first and asked questions afterwards would be
    // building namespaces for a party it had not identified.
    let mut child = Command::new(BIN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the helper binary runs");
    let mut stdin = child.stdin.take().expect("piped stdin");
    // Deliberately not a valid frame: the point is that no byte sequence on a
    // non-socket can talk this helper into a namespace.
    let _ = stdin.write_all(b"not a frame");
    drop(stdin);
    let out = child.wait_with_output().expect("the helper is reapable");
    assert_eq!(
        out.status.code(),
        Some(PRE_EXEC_EXIT),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}
