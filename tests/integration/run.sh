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

# ---- Named gates must EXIST ----------------------------------------------
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
# Named PROPERTY gates: mock-free, real-app, and equally invisible if they
# vanish -- but deliberately NOT milestone gates. Kept in a second list rather
# than folded into the one above, because a milestone's definition of done is a
# claim about that milestone and D12 adjudicated this property out of M1.4's
# criteria (plan §5). Same presence check, different claim.
PROPERTY_GATES=(
  test_real_trust_band.py       # trusted-band negative half (#139, refs #85)
  test_two_realms.py            # cross-realm capture confidentiality (#209)
  test_layout.py                # layout_focus moves the output; captures stay
                                # per-realm afterwards (#210)
  test_input_switch.py          # physical input follows the binding, an agent's
                                # actuation follows its grant (#212)
  test_attention.py             # the human's attention key lifts `preempted` for
                                # exactly one layout use, and only for principals
                                # holding layout authority (#232)
  test_shell.py                 # the switcher and launcher is an ordinary SDK
                                # client: a separate host-side process moves the
                                # output, and killing it leaves both realms
                                # running with the last-bound one still taking
                                # the human's input (#211)
  test_launch.py                # a consented `realm_launch` grant makes the
                                # TRUSTED CORE fork a process, and the journal
                                # names who asked (#207). The one gate covering
                                # a wire-reachable path into `spawn.rs`, so its
                                # silent absence is the costliest of any here.
)
missing=()
for gate in "${MILESTONE_GATES[@]}" "${PROPERTY_GATES[@]}"; do
  [ -f "tests/integration/$gate" ] || missing+=("$gate")
done
if [ ${#missing[@]} -ne 0 ]; then
  echo "ERROR: named gate module(s) missing: ${missing[*]}" >&2
  echo "       \`unittest discover\` would simply not collect them and this suite" >&2
  echo "       would exit 0 with that gate's evidence contributing nothing." >&2
  echo "       Restore the file, or -- if a gate is genuinely being retired --" >&2
  echo "       remove it from MILESTONE_GATES/PROPERTY_GATES here AND from the" >&2
  echo "       gate table in tests/integration/README.md, in the same commit." >&2
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
# - `vitrin-core/physical-input-injector` (issue #212): `test_input_switch.py`
#   runs the core under `--headless --physical-input-fd N` and makes
#   physical-origin seat input happen over the inherited socketpair, through
#   the SAME `input::intake_physical` entry point the nested backend's winit
#   handler uses. CI has no input device and headless is the only backend it
#   runs (D-019(4)), so without this the "physical input follows the bound
#   realm" half of #212 has no mock-free gate at all. A plain build does not
#   know the flag and exits at argument parsing, so that test fails naming
#   this rebuild rather than skipping. Same two-gate posture as the consent
#   channel: the feature alone changes nothing at runtime without the flag.
#   `test_attention.py` (issue #232) rides the same channel and the same
#   argument: the attention chord is physical input, so its *trigger* is
#   unreachable here without the injector, and its `attention` line presses the
#   run's configured chord through the same `physical_key`. What stays outside
#   CI, and is a runbook step in `shim/docs/nested-multi-realm.md` rather than a
#   criterion, is that a real Super press on real hardware produces one.
INJECTORS=vitrin-core/dead-man-injector,vitrin-core/consent-injector,vitrin-core/physical-input-injector
echo "==> building workspace with $INJECTORS"
cargo build --workspace --features "$INJECTORS"

# ---- The C shim: resolve it, and never let its absence be SILENT ---------
#
# Every module in the real-app ladder skips itself when `VITRIN_C_SHIM_BIN` is
# unset. That skip is deliberate and stays (README: it is the local-dev path
# for anyone without a C toolchain). What was NOT deliberate is that it was
# invisible: this script never mentioned the variable, so a run without a built
# shim collected 97 tests, skipped the 25 that make this a gate, and exited
# **0** -- byte-identical, in exit status, to a full mock-free pass.
#
# That is the same defect as the missing-gate check above, one step later: a
# gate that does not RUN proves exactly as much as a gate that does not EXIST.
# It is not hypothetical either. A brand-new mock-free gate (#212's
# `test_input_switch.py`) was authored, "verified" against this script, and had
# in fact never executed once; two real routing bugs were sitting behind it.
# Issue #229.
#
# Three changes, in increasing order of how much they can save you:
#
#  1. AUTO-RESOLVE. If the shim has been built where `meson setup shim/build`
#     puts it, use it. Nobody who has done the work should have to also
#     remember the variable, and forgetting it was the whole failure mode.
#  2. VALIDATE. Set-but-wrong is a misconfiguration, not a machine state, so it
#     fails here rather than 25 times inside setUp.
#  3. ANNOUNCE. The mode is printed before the first test and the skips are
#     itemised after the last one, so "what did this run actually prove" is
#     answerable from the log alone rather than by counting.
#
# `VITRIN_REQUIRE_REAL_APPS=1` turns a degraded run into a failure. CI sets it:
# CI builds the shim itself, so a skipped ladder there means that build step
# broke, and the one thing this suite must never do is stay green while the
# evidence quietly stops being collected.
DEFAULT_SHIM="shim/build/vitrin-shim"
if [ -z "${VITRIN_C_SHIM_BIN:-}" ] && [ -x "$REPO/$DEFAULT_SHIM" ]; then
  export VITRIN_C_SHIM_BIN="$REPO/$DEFAULT_SHIM"
  echo "==> C shim auto-resolved: $DEFAULT_SHIM (export VITRIN_C_SHIM_BIN to override)"
fi

if [ -n "${VITRIN_C_SHIM_BIN:-}" ] && [ ! -x "${VITRIN_C_SHIM_BIN}" ]; then
  echo "ERROR: VITRIN_C_SHIM_BIN=${VITRIN_C_SHIM_BIN} is not an executable file." >&2
  echo "       It is SET, so a real-app run was requested; refusing to fall back to" >&2
  echo "       a component-only run that would look identical in the exit code." >&2
  echo "       Build it:  meson setup shim/build shim && meson compile -C shim/build" >&2
  exit 1
fi

if [ -n "${VITRIN_C_SHIM_BIN:-}" ]; then
  echo "==> mode: FULL -- the real-app ladder will run against $VITRIN_C_SHIM_BIN"
elif [ "${VITRIN_REQUIRE_REAL_APPS:-0}" = "1" ]; then
  echo "ERROR: VITRIN_REQUIRE_REAL_APPS=1, but no C shim was found or named." >&2
  echo "       The real-app ladder would skip and this suite would exit 0 having" >&2
  echo "       proved none of what it is cited for. Refusing to report that as a pass." >&2
  echo "       Build it:  meson setup shim/build shim && meson compile -C shim/build" >&2
  exit 1
else
  echo "==> mode: COMPONENT-ONLY -- no C shim, so the real-app ladder will SKIP."
  echo "    This run CANNOT be cited as mock-free evidence for any milestone."
  echo "    For the full suite:  meson setup shim/build shim && meson compile -C shim/build"
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
#
# Deliberately NOT `exec`. The census below has to read what actually ran, and
# `exec` replaces this shell with Python -- which is precisely why a skipped
# ladder could never be noticed from here before. Losing one process is a
# trivial price for the suite being able to describe its own coverage.
LOG="$(mktemp -t vitrin-integration-XXXXXX.log)"
trap 'rm -f "$LOG"' EXIT

set +e
PYTHONPATH="$REPO/sdk/python/src" VITRIN_REPO="$REPO" \
  "$PY" -m unittest discover -s tests/integration -p 'test_*.py' -v 2>&1 | tee "$LOG"
status=${PIPESTATUS[0]}
set -e

# ---- The census: what this run did NOT prove ------------------------------
#
# Printed unconditionally, including on success, because the failure mode this
# guards against IS a success. Skips are itemised rather than counted: "25
# skipped" invites the reader to assume they know which 25, and the two that
# matter are never the two they picture.
#
# Note what this does NOT do: fail on any skip. Some skips are honest machine
# states rather than misconfigurations -- no GTK dev headers at `meson setup`,
# no `node` on PATH -- and `test_real_gtk.py` already draws that line in as
# many words ("a loud SKIP, not a fail -- unlike a missing shim, which is a
# misconfig"). Turning those into failures would make the suite unrunnable on
# ordinary developer machines, which buys nothing: an absent GTK probe cannot
# be mistaken for a passing GTK gate, whereas an absent shim silently took the
# whole ladder with it.
skipped=$(grep -c '\.\.\. skipped' "$LOG" || true)
if [ "$skipped" -ne 0 ]; then
  echo ""
  echo "==> $skipped test(s) SKIPPED. This run did not exercise:"
  grep '\.\.\. skipped' "$LOG" | sed 's/^/      /'
fi

# The one skip class that is always a misconfiguration, checked even though the
# gate above should have made it unreachable: a guard worth having is worth
# having twice when the thing it protects is "did we actually collect the
# evidence we are about to claim".
shim_skips=$(grep -c 'VITRIN_C_SHIM_BIN is unset' "$LOG" || true)
if [ "$shim_skips" -ne 0 ] && [ "${VITRIN_REQUIRE_REAL_APPS:-0}" = "1" ]; then
  echo "" >&2
  echo "ERROR: $shim_skips test(s) skipped for want of VITRIN_C_SHIM_BIN under" >&2
  echo "       VITRIN_REQUIRE_REAL_APPS=1. The real-app ladder did not run." >&2
  exit 1
fi

if [ "$status" -eq 0 ] && [ "$skipped" -eq 0 ]; then
  echo "==> full suite: no skips, every named gate ran."
fi
exit "$status"
