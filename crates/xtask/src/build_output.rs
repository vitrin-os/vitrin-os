// SPDX-License-Identifier: Apache-2.0
//! Which paths in a working tree are build output rather than the
//! repository's own text (issue #295).
//!
//! Three scans in this crate read the tree from disk rather than from the
//! index: [`crate::limits`] greps named roots for a needle that must be
//! absent, [`crate::skip_census`] reads every `.rs` file under `crates/` and
//! `fuzz/`, and [`crate::test_census`] enumerates cargo packages and their
//! test targets. All three answer questions about **this repository's own
//! source** -- "does the shim mention AT-SPI", "does a CI step run this
//! `#[test]`" -- and a file that is not in the repository can answer neither.
//!
//! # The failure this exists to stop
//!
//! `limits-check`'s walk over `shim/` read `shim/build/`, which is meson
//! output: gitignored, zero tracked files, and holding a full copy of
//! wlroots' generated XWayland protocol sources. So
//! `limits::tests::vendored_trees_are_skipped` passed on a clean checkout,
//! passed in CI, and failed in every tree where the shim had actually been
//! built -- which is every tree that can run `tests/integration/run.sh`,
//! since it resolves `shim/build/vitrin-shim`.
//!
//! A local-only failure in one crate teaches `--exclude xtask`, and that
//! reflex is what #295 was filed about: PR #294 excluded the crate to dodge
//! this and thereby missed a regression it had just introduced *in that same
//! crate*. The cost of the false hit was never the false hit.
//!
//! # Why git, and not a directory name
//!
//! A name-shaped answer (`build`, `target`, `builddir`) is a list that has to
//! be right about names nobody controls: meson's build directory is named by
//! whoever runs `meson setup`, and this tree's ignored set already includes
//! `sdk/python/build/`, `fuzz/target/`, `docs/book/book/` and a stray
//! top-level `build/` that only a `.gitignore` *inside it* marks as output.
//! Git already knows the answer, from `.gitignore`, `.git/info/exclude` and
//! the user's `core.excludesFile` together, and it is the same answer CI's
//! checkout materialises -- which is the property that matters, because a
//! gate is only honest if the tree it reads is the tree CI reads.
//!
//! Ignored is deliberately narrower than untracked. A file that is untracked
//! but **not** ignored is work in progress: it is about to be committed, it
//! will be in CI, and a scan that skipped it would go quiet exactly when a
//! developer most needs it to speak. Only `--others --ignored` is dropped.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// The set of paths git ignores in one working tree, as repository-relative
/// prefixes: a wholly-ignored directory collapses to a single entry, and an
/// ignored file inside a tracked directory is an entry of its own.
///
/// Build it once per scan -- it costs one `git` invocation -- and ask
/// [`BuildOutput::covers`] per path.
#[derive(Debug)]
pub struct BuildOutput {
    /// The tree this was measured in. [`BuildOutput::covers`] takes absolute
    /// paths and strips this off, so a set can never be asked about a path in
    /// some other tree by accident.
    root: PathBuf,
    prefixes: BTreeSet<PathBuf>,
}

impl BuildOutput {
    /// Ask git which paths under `root` are ignored.
    ///
    /// # Errors
    ///
    /// If git is missing or `root` is not a work tree. That is deliberately
    /// fatal rather than "assume nothing is ignored": the quiet fallback is
    /// the behaviour #295 reports, and it fails in the one tree -- a built
    /// one -- where it is hardest to tell a false hit from a real one.
    pub fn of_tree(root: &Path) -> Result<Self> {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "ls-files",
                // NUL-separated, so a path with a space or a quote in it is
                // one entry rather than several.
                "-z",
                // Untracked only: a tracked file is never build output, even
                // if some rule would otherwise ignore it.
                "--others",
                // ... and of those, only the ignored ones.
                "--ignored",
                // .gitignore + .git/info/exclude + core.excludesFile, which
                // together are what "ignored" means to everyone else.
                "--exclude-standard",
                // Collapse a wholly-ignored directory to one entry instead of
                // walking it. `shim/build/` is ~1500 files; this makes it 1.
                "--directory",
            ])
            .output()
            .with_context(|| {
                format!(
                    "running `git ls-files` in {}. This gate reads the repository's own text \
                     and asks git which paths are build output; without git it cannot tell \
                     the two apart. See crates/xtask/src/build_output.rs (issue #295).",
                    root.display()
                )
            })?;
        if !out.status.success() {
            bail!(
                "`git ls-files` failed in {} ({}): {}\n    This gate distinguishes the \
                 repository's own text from build output by asking git, so it needs a git work \
                 tree. See crates/xtask/src/build_output.rs (issue #295).",
                root.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let prefixes = out
            .stdout
            .split(|b| *b == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| PathBuf::from(OsStr::from_bytes(entry)))
            .collect();
        Ok(Self {
            root: root.to_path_buf(),
            prefixes,
        })
    }

    /// Whether `path` is build output: it is an ignored path itself, or it is
    /// inside an ignored directory.
    ///
    /// `path` is absolute, under the tree this set was measured in. Asked
    /// about a path from some other tree it would answer "not build output"
    /// for everything, which is a filter that has silently switched itself
    /// off -- so that is an assertion rather than a sentence in a doc comment.
    pub fn covers(&self, path: &Path) -> bool {
        debug_assert!(
            path.starts_with(&self.root),
            "{} is not under {}: this set cannot answer for it, and answering \"no\" would \
             disable the filter without saying so",
            path.display(),
            self.root.display()
        );
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        // `ancestors` yields the path itself first, so an ignored FILE is
        // matched by the same lookup as an ignored directory's contents.
        rel.ancestors().any(|a| self.prefixes.contains(a))
    }

    /// How many entries git reported. Only a test that wants to say "this set
    /// is not empty, so the scan below it was actually filtered" needs this.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.prefixes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testtree::TestTree;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("crates/xtask -> crates -> workspace root")
            .to_path_buf()
    }

    /// **The end-to-end reading of git, against the real repository.** It
    /// holds in CI as well as locally, because the one path it asserts on --
    /// `target/` -- is where the binary running this assertion lives, so it
    /// exists in every tree that can execute this test at all.
    ///
    /// Both directions matter. Only the second half would notice a parse that
    /// returned "everything is ignored", which is how a filter silences the
    /// scan it was added to protect.
    #[test]
    fn the_real_tree_reports_its_build_output_and_not_its_sources() {
        let root = root();
        let out = BuildOutput::of_tree(&root).expect("the workspace is a git work tree");
        assert!(
            out.covers(&root.join("target")),
            "`target/` holds the binary running this test, so it exists and git ignores it"
        );
        assert!(
            out.covers(&root.join("target/debug/build")),
            "a path INSIDE an ignored directory is covered by the directory's entry"
        );
        for tracked in [
            "crates/xtask/src/build_output.rs",
            "shim/meson.build",
            "docs/book/src/limits.md",
        ] {
            assert!(
                !out.covers(&root.join(tracked)),
                "{tracked} is tracked source; a set that covers it would silence every scan"
            );
        }
    }

    /// The three shapes that are not "a directory called `target`", each of
    /// which a name-matching filter would have to be told about separately:
    /// an ignored directory whose name nobody chose in advance, an ignored
    /// file inside a tracked directory, and a directory marked as output only
    /// by a `.gitignore` *inside it* -- which is exactly how meson marks its
    /// own build directories, and how this repository's stray top-level
    /// `build/` is ignored without appearing in the root `.gitignore` at all.
    #[test]
    fn ignored_paths_are_recognised_whatever_their_name() {
        let tree = TestTree::new("build-output-shapes");
        tree.write(".gitignore", "/out-dir/\n*.scratch\n");
        tree.write("src/kept.rs", "// tracked\n");
        tree.write("src/notes.scratch", "ignored by suffix\n");
        tree.write("out-dir/deep/generated.rs", "// generated\n");
        tree.write("nested/.gitignore", "*\n");
        tree.write("nested/thing.rs", "// ignored by the nested rule\n");
        tree.git_init();

        let out = BuildOutput::of_tree(tree.path()).expect("a git work tree");
        assert!(out.len() >= 3, "git reported {} entries", out.len());
        for ignored in [
            "out-dir",
            "out-dir/deep/generated.rs",
            "src/notes.scratch",
            "nested",
            "nested/thing.rs",
        ] {
            assert!(
                out.covers(&tree.path().join(ignored)),
                "{ignored} is ignored in this tree"
            );
        }
        assert!(
            !out.covers(&tree.path().join("src/kept.rs")),
            "an untracked file that git does NOT ignore is work in progress, not build output"
        );
        assert!(
            !out.covers(&tree.path().join("src")),
            "a directory holding a kept file is not itself output"
        );
    }

    /// A tree with no git in it is a hard error, not an empty set. See
    /// [`BuildOutput::of_tree`]'s errors section: the empty set is the
    /// behaviour #295 reports, and it goes wrong silently.
    #[test]
    fn a_tree_that_is_not_a_work_tree_is_an_error() {
        let tree = TestTree::new("build-output-no-git");
        tree.write("src/kept.rs", "// tracked\n");
        let err = BuildOutput::of_tree(tree.path())
            .expect_err("no git work tree here")
            .to_string();
        assert!(err.contains("git ls-files"), "{err}");
        assert!(err.contains("#295"), "{err}");
    }
}
