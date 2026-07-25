// SPDX-License-Identifier: Apache-2.0
//! The `vitrin-mock-shim` **binary**'s own contract.
//!
//! This file has a second, load-bearing job beyond what it asserts: an
//! integration test is what makes Cargo build a package's binary targets
//! during `cargo test`. `crates/vitrin-core`'s spawn tests (P1.5.2, issue
//! #31) really fork and `exec` this binary and locate it beside their own
//! test executable, so without an integration test here `cargo test
//! --workspace` would compile the binary only in test mode and those spawn
//! tests would fail to find it. If this file is ever deleted, delete the
//! spawn acceptance tests with it -- or replace this mechanism first.

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_vitrin-mock-shim");

#[test]
fn serving_by_hand_is_refused_rather_than_guessed_at() {
    // The binary's identity comes from an inherited descriptor and nothing
    // else. Run without one -- fd 3 closed, as any ordinary `exec` leaves
    // it -- it must say so and stop, never speak protocol into whatever
    // descriptor 3 happens to be.
    let out = Command::new(BIN)
        .arg("--serve")
        .output()
        .expect("the fixture binary runs");
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("spawned by the trusted core"),
        "the refusal must explain the spawn contract: {stderr}"
    );
}

#[test]
fn an_unknown_mode_is_refused() {
    let out = Command::new(BIN).output().expect("the fixture binary runs");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_app_mode_lives_until_its_stdin_closes() {
    // The leaf process the `core -> shim -> app` topology test observes:
    // it must stay alive while the harness reads its `/proc` state, and
    // exit cleanly once released, so no test leaks a process.
    let mut child = Command::new(BIN)
        .arg("--app")
        .stdin(Stdio::piped())
        .spawn()
        .expect("the app mode starts");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin.write_all(b"still here").expect("the app is alive");
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the app must not exit while its stdin is open"
    );
    drop(stdin);
    let status = child.wait().expect("the app is reapable");
    assert!(status.success(), "a released app exits cleanly: {status:?}");
}
