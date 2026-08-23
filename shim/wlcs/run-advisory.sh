#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
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
# MPL-2.0 `shim` and `conformance` CI jobs. Read shim/wlcs/README.md
# for the full scope and the current, evidence-based pass-list before
# reading too much into any single number this prints.
#
# Usage:
#   run-advisory.sh <path-to-wlcs-binary> <path-to-vitrin-shim-wlcs.so> [output-dir]
#
# Exit code: always 0. Non-zero only for a usage error (missing arguments
# or an unusable binary/module), never for a failing or skipped test.
#
# Self-test: `bash shim/wlcs/test-summary.sh` -- exercises this script and
# shim/wlcs/summary.sh against checked-in real wlcs output, with no wlcs
# package or built module required. Run it after touching anything here.
set -euo pipefail

if [ "$#" -lt 2 ]; then
	echo "usage: $0 <wlcs-binary> <vitrin-shim-wlcs.so> [output-dir]" >&2
	exit 2
fi

# The log parsing lives in its own file so shim/wlcs/test-summary.sh can drive
# it against checked-in, real wlcs output without a wlcs binary, a built
# module or a compositor. Run that self-test after touching anything about
# how this script counts: `bash shim/wlcs/test-summary.sh`.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=shim/wlcs/summary.sh
. "$SCRIPT_DIR/summary.sh"

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

# SINCE ISSUE #283 THIS BLOCK IS NORMALLY INERT, AND THAT IS THE POINT.
# `shim/meson.build`'s `project()` defaults now carry `default_library=static`
# (D-043), which subprojects inherit, so a vendored build produces
# `libwlroots-0.19.a` and no `.so` at all: the `find` below yields nothing and
# LD_LIBRARY_PATH is never exported. Measured 2026-08-19 on all three build
# shapes this repo has: the plain vendored build and a nested build configured
# `--force-fallback-for=wayland-server,wayland-client,wayland-scanner,wayland`
# each leave ZERO `.so*` files under the build tree; a
# `--wrap-mode=forcefallback` build leaves four, and every one of them is
# inside `subprojects/libliftoff/test/`, where libliftoff builds a FAKE
# `libdrm.so.2` for its own test suite to intercept ioctls with.
#
# That third shape is why the `find` below excludes `test`/`tests` paths.
# Without the exclusion this script would put a stub libdrm ahead of the real
# one on the module's library path and announce that it had found a shared
# vendored build -- a mitigation doing active harm on a shape nobody had
# measured, which is worse than the leak it exists to prevent.
#
# It is KEPT rather than deleted because it is still the mitigation for the
# shape that produces it: `-Ddefault_library=shared` on the command line, or a
# subproject that overrides the inherited default. The reasoning below is what
# that shape costs, and it has not been re-measured since the static default
# landed -- so read it as "why this exists", not as a description of what CI
# builds today. What CI builds today needs none of it.
#
# THE ORIGINAL REASONING, for a SHARED vendored build.
#
# Meson's wlroots-0.19 dependency() has a fallback to a vendored subproject
# build (see shim/ci/install-deps.sh's note: Ubuntu 24.04 ships wayland 1.22,
# below wlroots 0.19's >=1.23.1 floor, so the fallback compiles a newer
# wayland from source too). Built shared, those fallback libraries are
# UNINSTALLED -- they live as plain .so files inside the build tree, never
# copied anywhere ld.so's default search path would find them.
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
# under $MODULE_DIR is an uninstalled shared fallback build -- a system
# wlroots-0.19 was found and no subprojects were compiled, or (since #283,
# the usual case) they were compiled static: the `find` below then yields
# nothing to add, and the line after it does nothing.
MODULE_DIR="$(cd "$(dirname "$MODULE")" && pwd)"
EXTRA_LIBDIRS="$(find "$MODULE_DIR" -type f -name '*.so*' \
	-not -path '*/test/*' -not -path '*/tests/*' \
	-exec dirname {} \; 2>/dev/null | sort -u | paste -sd: -)"
if [ -n "$EXTRA_LIBDIRS" ]; then
	export LD_LIBRARY_PATH="$EXTRA_LIBDIRS${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
	echo "note: exporting LD_LIBRARY_PATH=$EXTRA_LIBDIRS -- a SHARED vendored build was found" >&2
	echo "      under $MODULE_DIR. Since #283 the shim links vendored libraries statically," >&2
	echo "      so this is a non-default build shape; see the comment above." >&2
else
	echo "note: no shared objects under $MODULE_DIR outside a subproject's own test" >&2
	echo "      scaffolding, so LD_LIBRARY_PATH is untouched. That is the expected shape" >&2
	echo "      since #283 (static vendored libraries)." >&2
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
#
#     THAT EXCLUSION IS INCOMPLETE, AND IT IS A TIME BOMB. Excluding
#     *Touch* SUITES does not exclude the touch-device PARAMETERS of the
#     parameterised suites kept below:
#     SurfaceInputRegions/SurfaceInputCombinations is instantiated over
#     (surface type x input device), so half its parameters use a touch
#     device. On wlcs 1.6.1 (what Ubuntu 24.04 ships, so what CI installs)
#     those parameters fail before they ever reach create_touch, so the
#     run completes. On wlcs 1.7.0 the first one that gets that far
#     SEGFAULTS the runner, taking the remaining ~131 tests in scope with
#     it -- see README.md's "Known hazard". Left as-is deliberately: a
#     crash the summary reports as `status=aborted` is evidence; a crash
#     filtered out by test index is a number nobody can interpret later.
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

# Counting is delegated to summary.sh's wlcs_summarize_log; see that file for
# why every count is derived twice and why the patterns are wlcs's own
# listener's format rather than stock googletest's.
#
# It swallows its own errors by construction so the guarantee this script
# exists to keep survives a log that isn't there. That is not a theoretical
# concern: when the integration module failed to dlopen (undefined-symbol
# failure, see the LD_LIBRARY_PATH comment above) $LOG had no googletest
# output in it at all, every inner `grep` matched nothing and exited 1, and
# under `set -o pipefail` that failure became the exit status of the
# assignment itself -- tripping `set -e` and aborting the script before it
# ever reached `exit 0`. (This is not hypothetical: it is what actually
# happened in CI. testdata/wlcs-loadfail.log is a real capture of that log
# shape, and test-summary.sh asserts this path still ends at `exit 0`.)
#
# STATUS, not just counts. `failed=0` from a run that DIED mid-suite is the
# same three characters as `failed=0` from a clean run; the status word is
# what tells those apart, and it is why the summary line below carries one.
read -r TOTAL PASSED FAILED SKIPPED STATUS < <(wlcs_summarize_log "$LOG")

{
	echo ""
	echo "== summary =="
	echo "total=$TOTAL passed=$PASSED failed=$FAILED skipped=$SKIPPED status=$STATUS"
	if [ "$STATUS" = "aborted" ]; then
		echo ""
		echo "WARNING: the wlcs runner died before finishing (no end-of-run"
		echo "summary in the log). The counts above are only the tests that"
		echo "completed before it died -- everything after that point never"
		echo "ran and is counted nowhere. See shim/wlcs/README.md's \"Known"
		echo "blocker\" section."
	elif [ "$STATUS" = "no-output" ]; then
		echo ""
		echo "WARNING: no test ever started -- the log contains no wlcs test"
		echo "output at all. This normally means the module failed to load"
		echo "(check the log for a dlopen/undefined-symbol error), not that"
		echo "the suite passed."
	fi
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
echo "total=$TOTAL passed=$PASSED failed=$FAILED skipped=$SKIPPED status=$STATUS"

exit 0
