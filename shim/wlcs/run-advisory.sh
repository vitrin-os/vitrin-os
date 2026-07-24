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

# Meson's wlroots-0.19 dependency() has a fallback to a vendored subproject
# build (see shim/ci/install-deps.sh's note: Ubuntu 24.04 ships wayland 1.22,
# below wlroots 0.19's >=1.23.1 floor, so the fallback compiles a newer
# wayland from source too). Those fallback libraries are UNINSTALLED --
# they live as plain .so files inside the build tree, never copied
# anywhere ld.so's default search path would find them.
#
# $WLCS_BIN is a foreign, already-linked process (the apt `wlcs` package):
# by the time it dlopen()s $MODULE, ld.so has already resolved $WLCS_BIN's
# own libwayland-client.so.0 against the *system* copy (1.22, on a runner
# like Ubuntu 24.04's). An rpath baked into $MODULE or libwlroots-0.19.so
# cannot retroactively change that -- it only affects how *their own*
# NEEDED entries resolve, and by then libwayland-client.so.0 is already
# loaded process-wide under that soname, so ld.so reuses the old one
# instead of consulting anyone's rpath. libwlroots-0.19.so was built
# against (and calls symbols only in, e.g. wl_proxy_get_queue) the newer
# fallback wayland, so it fails to resolve against the old one --
# "undefined symbol: wl_proxy_get_queue".
#
# LD_LIBRARY_PATH, unlike rpath, is consulted for $WLCS_BIN's *own*
# process-startup dependency resolution too (it's exec'd by us below, not
# merely dlopen'd), so putting every directory under the build tree that
# holds a shared object ahead of the default search path makes $WLCS_BIN
# and $MODULE agree on the SAME (newer) libwayland-client from the start
# -- there is then only ever one copy of the soname loaded, and it is the
# one that actually has the symbols wlroots needs. Harmless when nothing
# under $MODULE_DIR is an uninstalled fallback build (e.g. a system
# wlroots-0.19 was found and no subprojects were compiled): the `find`
# below then yields nothing to add.
MODULE_DIR="$(cd "$(dirname "$MODULE")" && pwd)"
EXTRA_LIBDIRS="$(find "$MODULE_DIR" -name '*.so*' -exec dirname {} \; 2>/dev/null | sort -u | paste -sd: -)"
if [ -n "$EXTRA_LIBDIRS" ]; then
	export LD_LIBRARY_PATH="$EXTRA_LIBDIRS${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi

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
# Each `|| true` below (not just on the outer grep -E) matters: this is
# exactly the scenario that actually happened when the integration module
# failed to load (undefined-symbol / dlopen failure, see the LD_LIBRARY_PATH
# comment above) -- $LOG then has neither a "[ FAILED ]" summary line nor a
# "[==========] N tests from" line at all, so FAILED_LINE/TOTAL_LINE are
# empty and the inner `grep -oE '[0-9]+'` on an empty string matches
# nothing and exits 1. Under `set -o pipefail`, that failure is the exit
# status of the whole `echo | grep | head` pipeline feeding the assignment,
# which trips `set -e` on the assignment itself -- aborting the script
# before it ever reaches `exit 0`, the one guarantee this script exists to
# keep. (This is not hypothetical: it's what actually happened in CI.)
FAILED_LINE=$(grep -E '^\[  FAILED  \] [0-9]+ tests? failed:' "$LOG" || true)
FAILED=$(echo "$FAILED_LINE" | grep -oE '[0-9]+' | head -1 || true)
FAILED="${FAILED:-0}"
TOTAL_LINE=$(grep -E '^\[==========\] [0-9]+ tests? from' "$LOG" | tail -1 || true)
TOTAL=$(echo "$TOTAL_LINE" | grep -oE '[0-9]+' | head -1 || true)
TOTAL="${TOTAL:-0}"

{
	echo ""
	echo "== summary =="
	echo "total=$TOTAL passed=$PASSED failed=$FAILED skipped=$SKIPPED"
	echo ""
	echo "Dominant failure categories in this run (see shim/wlcs/README.md"
	echo "for the standing, annotated pass-list -- these counts are"
	echo "THIS RUN's, and are expected to roughly match it):"
	# `|| true`: under `set -o pipefail`, a `grep` that matches zero lines
	# (e.g. every test passed, or failures came from a mechanism that
	# doesn't format as a gtest "C++ exception" line) exits 1, which would
	# otherwise trip `set -e` here and abort the script before `exit 0` --
	# exactly the "always exits 0" guarantee this script exists to keep.
	grep -oE 'C\+\+ exception with description "[^"]*"' "$LOG" | sort | uniq -c | sort -rn | head -10 || true
} | tee -a "$LOG"

echo ""
echo "full log:      $LOG"
echo "gtest XML:     $XML"
echo "total=$TOTAL passed=$PASSED failed=$FAILED skipped=$SKIPPED"

exit 0
