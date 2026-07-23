#!/usr/bin/env bash
#
# Headless integration suite — the CI `integration` job's entry point.
#
# Contract (normative copy: tests/integration/README.md): bash, exit 0 =
# pass. CI invokes this as `bash tests/integration/run.sh` with a warm
# `cargo build --workspace` already done, under a 10-minute cap.
#
# # What this suite is for
#
# Everything here drives the **shipped `vitrind` binary** over a real Unix
# socket with a real forked realm. That is the whole point, and it is not
# redundant with `cargo test --workspace`: the in-process tests in
# `crates/vitrin-core/src/session.rs` build the runtime loop by calling
# `start_realm_in` directly, so they never execute `run_session` — the
# function that orders startup in the shipped binary. A regression that
# reordered `install()` and the fork would leave every unit test green and
# wedge every real session forever on `configure` (issue #77, trap T1).
# This suite is the only thing that would catch it.
#
# # Dependencies: none beyond the toolchain
#
# Pure stdlib Python driving the SDK off `PYTHONPATH`, `unittest` rather
# than pytest, no `pip install` of any kind. The CI job installs no Python
# packages, and the SDK is zero-runtime-dependency by design (D8), so
# keeping this suite dependency-free means the job needs no Python setup
# step at all and cannot rot when one drifts.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"

# The binaries this suite drives. CI builds them in an untimed warm-up step;
# a developer running this locally may not have, so build rather than fail
# with a confusing "no such file". `--workspace` matches CI exactly, plus
# `vitrin-core/dead-man-injector` (issue #109): `test_real_deadman.py` sends
# the built `vitrind` a SIGUSR1 to stand in for a completed hold-Esc chord
# (headless has no physical input device to hold a real one on), and only a
# `dead-man-injector` build has a handler installed for that signal at all --
# without the feature SIGUSR1 takes its default disposition (terminate) and
# the test fails loudly naming this exact rebuild rather than skipping or
# hanging (see that test's module docs). The feature is purely additive (an
# extra signal source in the headless backend only) and is never enabled in
# a deployment build, so turning it on here changes nothing else this suite
# exercises. A warm cache still makes this a no-op rather than a second
# compile: cargo does not recompile a crate whose enabled feature set has not
# changed since the last build.
if [ ! -x target/debug/vitrind ] || [ ! -x target/debug/vitrin-mock-shim ]; then
  echo "==> building workspace (missing target/debug/vitrind or vitrin-mock-shim)"
  cargo build --workspace --features vitrin-core/dead-man-injector
fi

# `python3` on the runner is 3.12, clearing the SDK's >= 3.11 floor (D8).
PY="${PYTHON:-python3}"
"$PY" - <<'EOF'
import sys
if sys.version_info < (3, 11):
    sys.exit(f"vitrin-os SDK needs Python >= 3.11 (D8); this is {sys.version.split()[0]}")
EOF

echo "==> $("$PY" --version), vitrind $(target/debug/vitrind --version 2>/dev/null || echo '(no --version)')"
echo "==> running headless integration suite"

# `-v` because a CI log that says only "6 tests passed" cannot tell you
# *which* acceptance criterion regressed when one later fails.
PYTHONPATH="$REPO/sdk/python/src" VITRIN_REPO="$REPO" \
  exec "$PY" -m unittest discover -s tests/integration -p 'test_*.py' -v
