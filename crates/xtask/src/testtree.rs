// SPDX-License-Identifier: Apache-2.0
//! A throwaway working tree for tests that must reason about **ignored**
//! paths (issue #295).
//!
//! The scans in this crate read the repository they ship in, which is the
//! right default: a gate that holds the real tree cannot go stale against it.
//! But the property #295 is about -- "build output is not a claim surface" --
//! is invisible in a clean checkout, because a clean checkout has no build
//! output. Held against the real tree alone it would be exercised only on the
//! machine of whoever had run `meson setup`, and green in CI for the same
//! reason the bug was green in CI.
//!
//! So the tests that need an ignored path build one, in a tree of their own,
//! with a real `git init` behind it -- git's ignore rules are the thing under
//! test and a hand-written stand-in would only test itself.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A temporary directory, deleted when the test drops it.
pub(crate) struct TestTree {
    path: PathBuf,
}

/// Distinguishes trees built by tests running concurrently in one process;
/// the pid distinguishes concurrent processes.
static SEQ: AtomicUsize = AtomicUsize::new(0);

impl TestTree {
    /// An empty tree named after `label`, which appears in the path so a
    /// leaked directory names the test that leaked it.
    pub(crate) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vitrin-xtask-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("creating the test tree");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Write `text` to `rel`, creating parent directories.
    pub(crate) fn write(&self, rel: &str, text: &str) {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("creating a parent directory");
        }
        std::fs::write(&full, text).expect("writing a file into the test tree");
    }

    /// Make this tree a git work tree. Nothing is committed: the ignore rules
    /// are read from the files on disk, and `--others --ignored` needs no
    /// history to answer.
    ///
    /// The repository's own `core.excludesFile` is emptied, because local
    /// config beats global and a test whose result depends on the developer's
    /// personal ignore file is a test that reports the developer.
    pub(crate) fn git_init(&self) {
        self.git(&["init", "-q"]);
        self.git(&["config", "core.excludesFile", "/dev/null"]);
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
