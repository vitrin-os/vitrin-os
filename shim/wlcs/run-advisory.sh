#!/usr/bin/env bash
# run-advisory.sh -- run the real `wlcs` conformance runner against
# vitrin-shim-wlcs.so (P1.9.4, issue #47) and print a pass/fail/skip summary.
#
# ADVISORY, BY DESIGN: this script's own exit code is always 0 (even when
# every test fails) -- see the bottom of the file. The CI job that calls
# this additionally sets `continue-on-error: true` on itself, so a second,
# independent mechanism also keeps a red run here from blocking a PR. Two
# mechanisms, not one, because either alone is one bad rebase away from
# quietly starting to gate merges (a job that stops setting
# continue-on-error, or a script that starts propagating exit codes) -- see
# the issue's own "never blocks PRs" acceptance criterion.
#
# WHAT THIS DOES NOT PROVE. wlcs itself is GPL-3.0 (as is the module this
# runs -- shim/wlcs/integration.c, see its own header and README.md), so
# nothing this script does is a substitute for, or a gate alongside, the
# MIT-licensed `shim` and `conformance` CI jobs. Read shim/wlcs/README.md
# for the full scope and the current, evidence-based pass-list before
# reading too much into any single number this prints.
#
# Usage:
#   run-advisory.sh <path-to-wlcs-binary> <path-to-vitrin-shim-wlcs.so> [output-dir]
#
# Exit code: always 0. Non-zero only for a usage error (missing arguments
# or an unusable binary/module), never for a failing or skipped test.
set -euo pipefail

if [ "$#" -lt 2 ]; then
	echo "usage: $0 <wlcs-binary> <vitrin-shim-wlcs.so> [output-dir]" >&2
	exit 2
fi

WLCS_BIN="$1"
MODULE="$2"
OUT_DIR="${3:-$(pwd)/wlcs-advisory-out}"

if [ ! -x "$WLCS_BIN" ]; then
	echo "::error::wlcs binary not found or not executable: $WLCS_BIN" >&2
	exit 2
fi
if [ ! -f "$MODULE" ]; then
	echo "::error::vitrin-shim-wlcs.so not found: $MODULE" >&2
	exit 2
fi

mkdir -p "$OUT_DIR"

# THE SCOPE (issue #47: "xdg-shell+seat groups"). Chosen empirically by
# listing every suite `--gtest_list_tests` reports against this module and
# keeping the ones that exercise xdg-shell surface/toplevel/popup lifecycle
# and seat-mediated (pointer) input -- see README.md "Scope" for the list
# and the reasoning behind each exclusion:
#
#   - AllSurfaceTypes/TouchTest and every other *Touch* suite: excluded, not
#     merely expected to fail. This shim's wl_seat never advertises the
#     touch capability and `vitrin_shim_seat` (the wire protocol this
#     bridge replays input through) has no touch event at all (globals.c:
#     "v0's seat vocabulary has no touch event") -- there is structurally
#     nothing for these tests to exercise, so running them would only
#     generate noise, not information.
#   - Interactive move/resize and multi-window (parent/child) toplevel
#     tests: KEPT IN, even though README.md's pass-list explains they are
#     expected to fail or time out (xdg.c's layout is fixed
#     single-maximized-at-the-origin; see position_window_absolute in
#     integration.c) -- a kept, explained failure is still evidence; a
#     filtered-out one is not.
FILTER='XdgSurfaceStableTest.*'
FILTER+=':XdgToplevelStableTest.*'
FILTER+=':XdgToplevelStableConfigurationTest.*'
FILTER+=':XdgPopupStable/XdgPopupTest.*'
FILTER+=':ClientSurfaceEventsTest.*'
FILTER+=':SurfaceInputRegions/SurfaceInputCombinations.*'
FILTER+=':PointerCrossingSurfaceCorner/SurfacePointerMotionTest.*'
FILTER+=':PointerCrossingSurfaceEdge/SurfacePointerMotionTest.*'
FILTER+=':WlOutputTest.*'

export WLR_BACKENDS="${WLR_BACKENDS:-headless}"
export WLR_RENDERER="${WLR_RENDERER:-pixman}"

LOG="$OUT_DIR/wlcs-run.log"
XML="$OUT_DIR/wlcs-results.xml"

echo "== wlcs advisory run: $(date -u +%FT%TZ) ==" | tee "$LOG"
echo "wlcs binary: $WLCS_BIN" | tee -a "$LOG"
echo "module:      $MODULE" | tee -a "$LOG"
echo "filter:      $FILTER" | tee -a "$LOG"
echo "" | tee -a "$LOG"

# `|| true`: a non-zero gtest exit code (any failing test) must not trip
# `set -e` -- that is the whole point of this script.
"$WLCS_BIN" "$MODULE" \
	--gtest_filter="$FILTER" \
	--gtest_output="xml:$XML" \
	>>"$LOG" 2>&1 || true

PASSED=$(grep -c '^\[       OK \]' "$LOG" || true)
SKIPPED=$(grep -c '^\[     SKIP' "$LOG" || true)
# The failure count from gtest's own summary line ("[  FAILED  ] N tests
# failed:"), not a grep -c on "[  FAILED  ]" -- that pattern also matches
# each per-test line in the re-listing gtest prints below the summary
# header, which would double-count.
FAILED_LINE=$(grep -E '^\[  FAILED  \] [0-9]+ tests? failed:' "$LOG" || true)
FAILED=$(echo "$FAILED_LINE" | grep -oE '[0-9]+' | head -1)
FAILED="${FAILED:-0}"
TOTAL_LINE=$(grep -E '^\[==========\] [0-9]+ tests? from' "$LOG" | tail -1 || true)
TOTAL=$(echo "$TOTAL_LINE" | grep -oE '[0-9]+' | head -1)
TOTAL="${TOTAL:-0}"

{
	echo ""
	echo "== summary =="
	echo "total=$TOTAL passed=$PASSED failed=$FAILED skipped=$SKIPPED"
	echo ""
	echo "Dominant failure categories in this run (see shim/wlcs/README.md"
	echo "for the standing, annotated pass-list -- these counts are"
	echo "THIS RUN's, and are expected to roughly match it):"
	grep -oE 'C\+\+ exception with description "[^"]*"' "$LOG" | sort | uniq -c | sort -rn | head -10
} | tee -a "$LOG"

echo ""
echo "full log:      $LOG"
echo "gtest XML:     $XML"
echo "total=$TOTAL passed=$PASSED failed=$FAILED skipped=$SKIPPED"

exit 0
