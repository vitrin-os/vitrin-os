#!/usr/bin/env bash
# test-summary.sh -- self-test for shim/wlcs/summary.sh and run-advisory.sh.
#
# Runs anywhere: no wlcs package, no built module, no compositor, no GPU. It
# replays checked-in wlcs output from testdata/ through the same parser
# run-advisory.sh uses, and drives run-advisory.sh end to end against a stub
# "runner" that replays one of those captures.
#
# WHY THIS EXISTS. The counting patterns in summary.sh are matched against a
# format nothing in this repository controls: the wlcs runner's own gtest
# event listener. A pattern that stops matching does not fail loudly, it
# reports zero -- "failed=0" reads exactly like "nothing failed". This file
# is the mechanism that makes that impossible to land unnoticed: every
# fixture below asserts NON-ZERO failure and skip counts, so a pattern that
# rots takes this test red with it.
#
# PROVENANCE OF testdata/*.log -- all three are real wlcs output, captured on
# 2026-07-25 against a vitrin-shim-wlcs.so built from this tree with system
# wlroots 0.19.3 and wayland 1.25.0, WLR_BACKENDS=headless,
# WLR_RENDERER=pixman. They are verbatim except for one substitution: the
# capturing machine's absolute source path was rewritten to `/build/vitrin`
# and its scratch directory to `/build/scratch`. Nothing else was added,
# removed or reordered -- in particular the counts asserted below are the
# counts those captures really produced.
#
#   wlcs-1.6.1-complete.log   wlcs 1.6.1-1, the version Ubuntu 24.04 (noble)
#                             ships and therefore the one the wlcs-advisory
#                             CI job installs. A complete run of a filtered
#                             subset of run-advisory.sh's scope, kept small
#                             while still containing passes, failures AND
#                             skips plus the end-of-run summary block.
#   wlcs-1.7.0-aborted.log    wlcs 1.7.0-1ubuntu1 (a LATER Ubuntu release's
#                             package, not noble's), where the runner
#                             segfaults mid-suite -- see README.md's "Known
#                             hazard". A skip, a failure and a pass complete
#                             first, so this is precisely the case a summary
#                             parser that only reads the end-of-run block
#                             reports as total=0 failed=0: a run with a real
#                             failure in it, indistinguishable from a clean
#                             sweep.
#   wlcs-loadfail.log         the module failing to dlopen: no test ever ran,
#                             and the log has no googletest output in it at
#                             all. This is the shape of the log from the CI
#                             incident that run-advisory.sh's `|| true`
#                             guards exist for.
#
# Usage: bash shim/wlcs/test-summary.sh
# Exit code: 0 if every assertion holds, 1 otherwise. Unlike run-advisory.sh,
# this one is a real test and DOES report failure through its exit code.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=shim/wlcs/summary.sh
. "$HERE/summary.sh"

FAILURES=0
CHECKS=0

fail() {
	echo "FAIL: $*" >&2
	FAILURES=$((FAILURES + 1))
}

check_eq() {
	# check_eq <what> <expected> <actual>
	CHECKS=$((CHECKS + 1))
	if [ "$2" != "$3" ]; then
		fail "$1: expected '$2', got '$3'"
	fi
}

check_gt0() {
	# check_gt0 <what> <actual> -- the anti-silent-zero assertion
	CHECKS=$((CHECKS + 1))
	if ! [ "$2" -gt 0 ] 2>/dev/null; then
		fail "$1: expected a non-zero count, got '$2'"
	fi
}

summarize() {
	# Discards the stderr diagnostics; the assertions are on the counts.
	wlcs_summarize_log "$1" 2>/dev/null
}

echo "== summary.sh against real wlcs captures =="

# --- 1. a complete run, on the wlcs version CI uses ---------------------
# The regression the whole file guards: the failure and skip counts must be
# non-zero for a run that really did have 5 failures and 4 skips.
got=$(summarize "$HERE/testdata/wlcs-1.6.1-complete.log")
check_eq "complete run (wlcs 1.6.1)" "12 3 5 4 complete" "$got"
read -r _ _ c_failed c_skipped _ <<<"$got"
check_gt0 "complete run: failed" "$c_failed"
check_gt0 "complete run: skipped" "$c_skipped"

# --- 2. a run that died mid-suite --------------------------------------
# One skip, one failure and one pass completed, then the runner segfaulted
# before printing any summary block. THIS IS THE REGRESSION: a parser that
# reads only the end-of-run block reports total=0 failed=0 here, i.e. a run
# in which a test demonstrably failed reads as one in which nothing did. The
# counts must be the three that completed, the failure count must be
# non-zero, and the status must say the tally is partial.
got=$(summarize "$HERE/testdata/wlcs-1.7.0-aborted.log")
check_eq "aborted run (wlcs 1.7.0)" "3 1 1 1 aborted" "$got"
read -r _ _ a_failed a_skipped _ <<<"$got"
check_gt0 "aborted run: failed" "$a_failed"
check_gt0 "aborted run: skipped" "$a_skipped"

# --- 3. the module failing to load -------------------------------------
got=$(summarize "$HERE/testdata/wlcs-loadfail.log")
check_eq "module load failure" "0 0 0 0 no-output" "$got"

# --- 4. degenerate logs: empty, and absent -----------------------------
# The `set -e` safety net run-advisory.sh's comments describe. Neither of
# these may kill the script; both must summarise as an empty run.
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
: >"$TMP/empty.log"
check_eq "empty log" "0 0 0 0 no-output" "$(summarize "$TMP/empty.log")"
check_eq "missing log" "0 0 0 0 no-output" "$(summarize "$TMP/does-not-exist.log")"

# --- 5. the end-of-run listing must not be double-counted ---------------
# wlcs prints every failed and skipped test twice: once as it happens (with a
# duration) and once in the end-of-run listing (without one). Counting the
# bracket tag alone would report 11 failures for the 5 real ones in the
# fixture.
listing_failed=$(grep -cE '^\[  FAILED  \]' "$HERE/testdata/wlcs-1.6.1-complete.log" || true)
CHECKS=$((CHECKS + 1))
if [ "$listing_failed" -le 5 ]; then
	fail "fixture no longer exercises the re-listing (found $listing_failed" \
		"'[  FAILED  ]' lines for 5 failed tests; expected more than 5)"
fi

# --- 6. stock-googletest output is understood too ----------------------
# Not a wlcs capture: googletest's own PrettyUnitTestResultPrinter, which a
# differently-built wlcs may use instead. Written inline rather than checked
# in, because unlike testdata/ it is not evidence about this shim -- it is
# just the other dialect summary.sh claims to accept.
cat >"$TMP/stock-gtest.log" <<'EOF'
[==========] Running 8 tests from 4 test suites.
[----------] 2 tests from XdgSurfaceStableTest
[ RUN      ] XdgSurfaceStableTest.supports_xdg_shell_stable_protocol
[       OK ] XdgSurfaceStableTest.supports_xdg_shell_stable_protocol (0 ms)
[ RUN      ] XdgSurfaceStableTest.creating_xdg_surface_is_an_error
[  FAILED  ] XdgSurfaceStableTest.creating_xdg_surface_is_an_error (0 ms)
[ RUN      ] XdgToplevelStableTest.parent_can_be_set
[  FAILED  ] XdgToplevelStableTest.parent_can_be_set (0 ms)
[ RUN      ] XdgToplevelStableTest.null_parent_can_be_set
[  SKIPPED ] XdgToplevelStableTest.null_parent_can_be_set (0 ms)
[ RUN      ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/0
[  SKIPPED ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/0 (0 ms)
[ RUN      ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/1
[  SKIPPED ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/1 (0 ms)
[ RUN      ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/2
[  SKIPPED ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/2 (0 ms)
[ RUN      ] WlOutputTest.wl_output_properties_set
[       OK ] WlOutputTest.wl_output_properties_set (0 ms)
[----------] Global test environment tear-down
[==========] 8 tests from 4 test suites ran. (0 ms total)
[  PASSED  ] 2 tests.
[  SKIPPED ] 4 tests, listed below:
[  SKIPPED ] XdgToplevelStableTest.null_parent_can_be_set
[  SKIPPED ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/0
[  SKIPPED ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/1
[  SKIPPED ] SurfaceInputRegions/SurfaceInputCombinations.surface_gets_input/2
[  FAILED  ] 2 tests, listed below:
[  FAILED  ] XdgSurfaceStableTest.creating_xdg_surface_is_an_error
[  FAILED  ] XdgToplevelStableTest.parent_can_be_set

 2 FAILED TESTS
EOF
check_eq "stock googletest dialect" "8 2 2 4 complete" "$(summarize "$TMP/stock-gtest.log")"

# --- 7. run-advisory.sh end to end -------------------------------------
# A stub standing in for the wlcs runner: replays a real capture and exits
# non-zero, exactly as the runner does when tests fail. This exercises the
# whole script -- argument checks, LD_LIBRARY_PATH computation, the `|| true`
# around the run, the summary block -- and asserts the two properties the
# script exists to guarantee: it reports the real counts, and it exits 0.
echo
echo "== run-advisory.sh end to end (stub runner) =="

cat >"$TMP/stub-wlcs" <<EOF
#!/usr/bin/env bash
cat "$HERE/testdata/wlcs-1.6.1-complete.log"
exit 1
EOF
chmod +x "$TMP/stub-wlcs"
: >"$TMP/vitrin-shim-wlcs.so"

set +e
out=$(bash "$HERE/run-advisory.sh" "$TMP/stub-wlcs" "$TMP/vitrin-shim-wlcs.so" "$TMP/out" 2>&1)
rc=$?
set -e
check_eq "run-advisory.sh exit code with failing tests" "0" "$rc"
CHECKS=$((CHECKS + 1))
if ! printf '%s\n' "$out" | grep -q 'total=12 passed=3 failed=5 skipped=4 status=complete'; then
	fail "run-advisory.sh did not print the expected summary line; got:"
	printf '%s\n' "$out" >&2
fi

# The same, with a runner that produces nothing at all -- the shape of the
# CI incident run-advisory.sh's `|| true` guards were added for. Must still
# reach `exit 0`.
cat >"$TMP/stub-wlcs-silent" <<'EOF'
#!/usr/bin/env bash
exit 127
EOF
chmod +x "$TMP/stub-wlcs-silent"
set +e
out=$(bash "$HERE/run-advisory.sh" "$TMP/stub-wlcs-silent" "$TMP/vitrin-shim-wlcs.so" "$TMP/out2" 2>&1)
rc=$?
set -e
check_eq "run-advisory.sh exit code with a silent runner" "0" "$rc"
CHECKS=$((CHECKS + 1))
if ! printf '%s\n' "$out" | grep -q 'total=0 passed=0 failed=0 skipped=0 status=no-output'; then
	fail "run-advisory.sh did not report an empty run correctly; got:"
	printf '%s\n' "$out" >&2
fi

echo
if [ "$FAILURES" -eq 0 ]; then
	echo "OK: $CHECKS checks passed"
	exit 0
fi
echo "FAILED: $FAILURES of $CHECKS checks" >&2
exit 1
