#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
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

# ---- Named milestone gates must EXIST ------------------------------------
#
# `unittest discover` below enumerates whatever `test_*.py` files happen to be
# present, so a named gate that was never written -- or one dropped or renamed
# in a later refactor -- is indistinguishable from a green suite: nothing is
# collected, nothing fails, exit 0. That is not a gate that cannot fail; it is
# a MISSING gate that cannot be noticed, and it is exactly how a milestone
# comes to be recorded as done on evidence that does not exist (issue #138 was
# filed on precisely that shape: M1.4's consent half was claimed with no gate
# in this directory at all).
#
# So the milestone gates are named here, once, and their absence is a hard
# failure before a single test runs. This list is the machine-checkable half
# of tests/integration/README.md's gate table; the two are edited together.
MILESTONE_GATES=(
  test_real_app.py              # M1.2 exit gate (#105)
  test_real_capture_fidelity.py # M1.3 exit gate (#107)
  test_real_actuation.py        # M1.4, actuation half (#108)
  test_real_deadman.py          # M1.4, dead-man half (#109)
  test_real_consent.py          # M1.4, consent half (#138)
  test_demo.py                  # M1.5 exit gate (#110)
)
missing=()
for gate in "${MILESTONE_GATES[@]}"; do
  [ -f "tests/integration/$gate" ] || missing+=("$gate")
done
if [ ${#missing[@]} -ne 0 ]; then
  echo "ERROR: named milestone gate module(s) missing: ${missing[*]}" >&2
  echo "       \`unittest discover\` would simply not collect them and this suite" >&2
  echo "       would exit 0 with that milestone's evidence contributing nothing." >&2
  echo "       Restore the file, or -- if a gate is genuinely being retired --" >&2
  echo "       remove it from MILESTONE_GATES here AND from the gate table in" >&2
  echo "       tests/integration/README.md, in the same commit." >&2
  exit 1
fi

# The binaries this suite drives. CI builds them in an untimed warm-up step;
# a developer running this locally may not have, so build rather than fail
# with a confusing "no such file". `--workspace` matches CI exactly, plus two
# test-only cargo features, each the stand-in for a human action a GPU-less,
# input-device-less headless runner cannot perform:
#
# - `vitrin-core/dead-man-injector` (issue #109): `test_real_deadman.py` sends
#   the built `vitrind` a SIGUSR1 to stand in for a completed hold-Esc chord.
#   Only a `dead-man-injector` build has a handler installed for that signal
#   at all -- without the feature SIGUSR1 takes its default disposition
#   (terminate) and the test fails loudly naming this exact rebuild rather
#   than skipping or hanging (see that test's module docs).
# - `vitrin-core/consent-injector` (issue #138): `test_real_consent.py` runs
#   the core under `--headless --consent=interactive --consent-injector-fd N`
#   and answers the raised prompt over the inherited socketpair. A plain
#   build REFUSES that pair at startup (a plain headless core can raise no
#   prompt and answer none) and does not know the flag at all, so without the
#   feature the core never binds its socket and that test fails naming this
#   rebuild. Note the feature ALONE is not enough at runtime: the refusal
#   relaxes only when the flag is also passed, which is what makes a running
#   instrumented core identifiable from `/proc/<pid>/cmdline`.
#
# Both are purely additive (an extra signal source, an extra socket source,
# and a card-footprint export, all on the headless backend only) and neither
# is ever enabled in a deployment build, so turning them on here changes
# nothing else this suite exercises.
#
# **Unconditional, deliberately.** This build step used to be guarded by
# `if [ ! -x target/debug/vitrind ]`, which tested that *a* binary existed and
# never that it was the right one -- so a `target/debug/vitrind` left behind by
# any other build (`cargo build -p vitrin-core`, a run with only one of the two
# features, CI's warm-build step passing a different list) silently won, and
# the suite reported on a core missing a handler it needs. That is not
# hypothetical: it turns up as `test_real_deadman` and `DemoHeadlessHoldEsc`
# failing with SIGUSR1 taking its default disposition, which reads as a broken
# dead-man switch rather than as a stale binary. Two injectors now share this
# hazard, so the guard's whole premise is gone.
#
# Dropping it costs nothing: cargo does not recompile a crate whose enabled
# feature set has not changed since the last build, so a warm tree makes this a
# no-op, and a tree built with any other feature set is exactly the case that
# MUST recompile. A gate that can run against the wrong binary reports
# something other than what it claims, which is the one thing this suite exists
# to not do.
INJECTORS=vitrin-core/dead-man-injector,vitrin-core/consent-injector
echo "==> building workspace with $INJECTORS"
cargo build --workspace --features "$INJECTORS"

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
