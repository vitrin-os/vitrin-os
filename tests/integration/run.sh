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
  test_real_clipboard.py        # WS-E.2.1 (#213): text copied in one realm
                                # reaches another ONLY after two distinct human
                                # chords, and one gesture transfers nothing. Two
                                # real shims, two real Wayland clients, and the
                                # journal grepped for the literal string it must
                                # not contain.
  test_screenshot.py            # WS-E.2.4 (#216): the human's screenshot key
                                # writes ONE file of the REALM VIEW into ONE
                                # audited directory and touches no grant. A
                                # `screenshot` line on the physical-input
                                # channel presses the real chord, so this covers
                                # detection as well as effect; the PNG is
                                # decoded with stdlib zlib (no image codec, in
                                # any dependency class) and compared against
                                # `--capture-dump` byte for byte, and the
                                # shipped binary's own startup refusals for a
                                # missing/file/symlink/world-writable/relative
                                # `--screenshot-dir` are asserted with a control
                                # that a clean directory passes.
  test_real_gestures.py         # WS-E.4.2 (#222): the touchpad classes the seat
                                # vocabulary grew -- relative motion, swipe,
                                # pinch and an app-requested pointer lock --
                                # reach a real Wayland client with every
                                # quantity where it belongs; `origin=physical`
                                # survives the wire, read off the C shim's own
                                # decode trace because the Wayland extension
                                # events carry no origin argument and the app
                                # therefore cannot witness it; the journal names
                                # the begin/end pair and the realm; and a realm
                                # switch mid-gesture pays the losing
                                # app a CANCELLED end so no app is left latched.
                                # What it cannot cover is libinput's own gesture
                                # detection, which is upstream of
                                # `intake_physical` and needs a real touchpad --
                                # `docs/drm-bringup.md` step 13a.
  test_real_confinement.py      # P2.6.2 (#186): a canary file in the operator's
                                # real $HOME is UNREACHABLE from inside a
                                # confined realm and REACHABLE from the same app,
                                # same argv, under `--isolation=off` -- the
                                # positive control the plan's own criterion
                                # demands, because an absence over a path nothing
                                # proved reachable is satisfied by no path at
                                # all. Reachability is decided by (st_dev,
                                # st_ino), never by name: the mount table has to
                                # CREATE $HOME inside the realm as a stub, so a
                                # presence test would call a correct realm a
                                # breach. Also: the flight recorder's
                                # parent-observed confinement facts checked
                                # against procfs, the supplementary-group
                                # residual D-037(5) publishes measured at the app
                                # with /dev/input unreachable beside it, and a
                                # substituted helper that unshares six namespaces
                                # and mounts NOTHING refused at startup
                                # (crates/vitrin-realm-init-fixtures). And since
                                # P2.6.3's K9b, the one property here that is
                                # about a SYSCALL rather than a path: a realm's
                                # app cannot create a user namespace inside the
                                # realm (ENOSPC at the shipped default AND at
                                # --landlock=off, so it is the realm's own
                                # max_user_namespaces and not the ruleset),
                                # against an --isolation=off positive control in
                                # the same run. No new CI wiring and no cargo
                                # feature.
  test_real_seccomp.py          # P2.6.4 (#188): the TABLE-DRIVEN seccomp gate.
                                # It reads the shipped filter table out of
                                # `vitrind --print-seccomp` rather than
                                # transcribing it, runs the repo-authored
                                # `solid-client --syscall-probe` inside a realm
                                # and again at `--isolation=off` with
                                # byte-identical argv, and for EVERY row asserts
                                # the exact errno in-realm against the outside
                                # run as that row's positive control. Two
                                # properties are the point: a row added to the
                                # filter without a probe case turns this RED
                                # (the row set comes from the binary, so there
                                # is nothing here to forget), and a row whose
                                # syscall fails outside a realm too is REPORTED
                                # NOT DEMONSTRATED rather than counted as
                                # confinement -- `bpf` and `userfaultfd` land
                                # there on a box with
                                # unprivileged_bpf_disabled or
                                # unprivileged_userfaultfd set, which is a
                                # property of the kernel and is therefore
                                # measured here rather than declared in the
                                # table. The negative controls matter as much:
                                # a filter is inherited and unremovable, so
                                # `seccomp(2)` itself, `get_mempolicy`,
                                # `fork` and the four kept socket families must
                                # SUCCEED inside the realm or Firefox's own
                                # content-process sandbox is broken. No new CI
                                # wiring and no cargo feature.
  test_launch.py                # a consented `realm_launch` grant makes the
                                # TRUSTED CORE fork a process, and the journal
                                # names who asked (#207). The one gate covering
                                # a wire-reachable path into `spawn.rs`, so its
                                # silent absence is the costliest of any here.
)
# The rest of the suite: modules that are neither a milestone gate nor a named
# property gate, but that `unittest discover` collects and this run is cited
# for. Named here for one reason (issue #288): until this list existed, SEVEN
# `test_*.py` modules appeared in no list at all, so deleting one made the
# suite collect fewer tests and exit 0 with nothing to say about it. That is
# the same failure class as a silent skip -- a green count standing in for
# absent evidence -- one level below where the two lists above could see it.
SUPPORTING_MODULES=(
  test_real_firefox.py          # M1.2's Firefox render proof. Also run by its
                                # own dedicated CI step, by exact filename.
  test_real_gtk.py              # the GTK text-field rung of the actuation
                                # ladder (needs gtk-entry-probe co-built)
  test_runtime_wiring.py        # the runtime tree, socket paths and env the
                                # shipped binary really builds
  test_hostile_client.py        # a peer that lies on the wire is refused
                                # rather than crashing the core
  test_multi_realm.py           # several realms at once over one socket
  test_actuation.py             # the actuation path against the mock shim
                                # (the component half of #108's ladder)
  test_consent_injector.py      # the scripted-consent channel itself, the
                                # instrument #138's gate rides on
)
missing=()
for gate in "${MILESTONE_GATES[@]}" "${PROPERTY_GATES[@]}" "${SUPPORTING_MODULES[@]}"; do
  [ -f "tests/integration/$gate" ] || missing+=("$gate")
done
if [ ${#missing[@]} -ne 0 ]; then
  echo "ERROR: named gate module(s) missing: ${missing[*]}" >&2
  echo "       \`unittest discover\` would simply not collect them and this suite" >&2
  echo "       would exit 0 with that gate's evidence contributing nothing." >&2
  echo "       Restore the file, or -- if a gate is genuinely being retired --" >&2
  echo "       remove it from MILESTONE_GATES/PROPERTY_GATES/SUPPORTING_MODULES" >&2
  echo "       here AND from the gate table in tests/integration/README.md, in" >&2
  echo "       the same commit." >&2
  exit 1
fi

# ...and the other direction, which is what makes the three lists a partition
# rather than a wish: a module ON DISK that appears in none of them is a
# module nobody has classified, and the next one to be deleted silently.
unlisted=()
for path in tests/integration/test_*.py; do
  name="$(basename "$path")"
  found=0
  for gate in "${MILESTONE_GATES[@]}" "${PROPERTY_GATES[@]}" "${SUPPORTING_MODULES[@]}"; do
    [ "$gate" = "$name" ] && found=1 && break
  done
  [ "$found" -eq 1 ] || unlisted+=("$name")
done
if [ ${#unlisted[@]} -ne 0 ]; then
  echo "ERROR: module(s) in tests/integration/ named in no list here: ${unlisted[*]}" >&2
  echo "       Add each to MILESTONE_GATES, PROPERTY_GATES or SUPPORTING_MODULES." >&2
  echo "       An unlisted module is one whose deletion this script cannot notice." >&2
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
#   `test_real_gestures.py` (WS-E.4.2, issue #222) rides it for the touchpad
#   classes, and the boundary is sharper there than anywhere else on this list:
#   the channel's `relative_motion` and `gesture` lines feed the same
#   `intake_physical`, so everything from the core's intake onward is real --
#   but **libinput's own gesture detection is upstream of that entry point**.
#   Nothing in CI, with or without this feature, is evidence that a human's
#   three fingers on a real touchpad produce a `gesture_begin`; that is a
#   hardware runbook step -- `docs/drm-bringup.md` step 13a, written and NOT
#   YET RUN -- and the gate's README row says so rather than letting the rung
#   imply it covers both.
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

# ---- A CONTROL run is not an ordinary run, and must not read like one -----
#
# `VITRIN_LANDLOCK` hands every gate that does not name its own Landlock
# setting one (harness.LANDLOCK_OVERRIDE). It exists so the `--landlock=off`
# half of a published measurement can be re-run from this repository -- until
# it landed, `docs/book/src/limits.md` cited a Landlock-off result for gates
# that had no way to be run Landlock-off at all.
#
# Announced here, loudly, because the whole hazard of such a knob is a run that
# proved something weaker than the log implies. Note what it does NOT do: it
# does not exempt the confinement gate, which names `highest` for its confined
# half and therefore goes RED under `VITRIN_LANDLOCK=off`. That is correct --
# a control run is not a pass -- and it is stated here so the red is read as
# the knob working rather than as a regression.
if [ -n "${VITRIN_LANDLOCK:-}" ]; then
  echo ""
  echo "==> CONTROL RUN: VITRIN_LANDLOCK=${VITRIN_LANDLOCK} -- every gate that does not"
  echo "    name its own Landlock setting runs with --landlock=${VITRIN_LANDLOCK}."
  echo "    This run is NOT evidence for any milestone and NOT the shipped default."
  echo "    test_real_confinement.py is EXPECTED to fail here: its confined half"
  echo "    asserts the session requested \`highest\`."
  echo ""
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

# ---- ...and what this run did not even COLLECT ----------------------------
#
# The presence checks at the top of this script prove the named modules exist
# on disk. They cannot prove `unittest discover` collected them: a module that
# imports but exposes no test case, a `load_tests` that returns an empty suite,
# or a discovery pattern change all leave the file in place and the evidence
# absent. So every named module is required to appear in the verbose log at
# least once -- by NAME, never as a count, because a count is a number somebody
# bumps and a name is a line a reviewer reads (issue #288).
#
# `-v` prints one `test_x (module.Class.test_x) ... ok|FAIL|ERROR|skipped` line
# per collected test, so a skipped module still satisfies this: the question
# here is "was it collected", and "did it skip" is the census above.
uncollected=()
for gate in "${MILESTONE_GATES[@]}" "${PROPERTY_GATES[@]}" "${SUPPORTING_MODULES[@]}"; do
  grep -q "${gate%.py}\." "$LOG" || uncollected+=("$gate")
done
if [ ${#uncollected[@]} -ne 0 ]; then
  echo "" >&2
  echo "ERROR: named module(s) collected ZERO tests: ${uncollected[*]}" >&2
  echo "       The file exists, so the presence check above passed, but nothing" >&2
  echo "       from it ran. Whatever this suite is cited for, it is not that." >&2
  exit 1
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

# ---- A red run must END red, and it did not ------------------------------
#
# The census above prints only when something skipped, and the success line
# below prints only when nothing skipped AND the status was 0. A run with
# failures and zero skips therefore printed NEITHER, so the last thing on the
# operator's terminal was whatever the final gate happened to log -- routinely
# a success line from a test unrelated to the failure. `tee` interleaving makes
# that worse: unittest's own `FAILED (failures=N)` goes to stderr and can land
# hundreds of lines above the last stdout line of the run.
#
# So the summary is stated here, unconditionally on the red path, in the two
# terms a reader needs: which gates failed, and that nothing in this run is
# evidence for anything. The count is read from unittest's own summary block
# (`FAIL:` / `ERROR:` at the start of a line, one per failing test) rather than
# from the inline `... FAIL` markers, which repeat under `-v` and would
# double-count. A nonzero status with zero such lines is a suite that died
# before reporting -- also red, also named, and the count says 0 rather than
# being suppressed.
if [ "$status" -ne 0 ]; then
  failing=$(grep -cE '^(FAIL|ERROR): ' "$LOG" || true)
  echo ""
  echo "==> SUITE RED: exit status $status, $failing test(s) reported FAIL/ERROR," \
       "$skipped skipped."
  if [ "$failing" -ne 0 ]; then
    grep -E '^(FAIL|ERROR): ' "$LOG" | sed 's/^/      /'
  else
    echo "      No test reported FAIL or ERROR, so the suite died before it could."
  fi
  echo "    NOTHING in this run may be cited as evidence for any milestone."
elif [ "$skipped" -eq 0 ]; then
  echo "==> full suite: no skips, every named gate ran."
fi
exit "$status"
