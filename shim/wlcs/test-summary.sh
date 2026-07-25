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
	# Only the counts. Section 7 asserts the stderr diagnostics separately.
	wlcs_summarize_log "$1" 2>/dev/null
}

stderr_of() {
	# Only the stderr diagnostics: the counts line is discarded inside the
	# group, then the group's stderr becomes this function's stdout. Written
	# as a group rather than `2>&1 >/dev/null` so that it is unambiguous
	# (and shellcheck-clean) that stderr is the payload here, not stdout.
	{ wlcs_summarize_log "$1" >/dev/null; } 2>&1
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
# duration) and once in the end-of-run listing (without one). The trailing
# `\([0-9]+ ?ms\)$` anchor in WLCS_RE_TEST_FAILED / _SKIPPED is the only
# thing telling the two apart.
#
# This asserts the PARSER's de-duplicated counts, not merely that the
# fixture still contains a re-listing. Asserting the fixture's shape alone
# pins nothing: no other check in this file reaches the per-test counts for
# this log (checks 1 and 6 both take the end-of-run summary block, which
# wins whenever it is present), so deleting the anchor from both patterns
# used to leave the whole self-test green while every aborted run silently
# double-counted afterwards. With the anchor gone these go 5 -> 11 and
# 4 -> 9, and this check goes red.
complete_log="$HERE/testdata/wlcs-1.6.1-complete.log"
raw_failed=$(grep -cE '^\[  FAILED  \]' "$complete_log" || true)
raw_skipped=$(grep -cE '^\[(     SKIP |  SKIPPED )\]' "$complete_log" || true)
CHECKS=$((CHECKS + 1))
if [ "$raw_failed" -le 5 ] || [ "$raw_skipped" -le 4 ]; then
	fail "fixture no longer exercises the re-listing (found $raw_failed" \
		"'[  FAILED  ]' and $raw_skipped skip-tagged lines, for 5 failed" \
		"and 4 skipped tests; expected strictly more of each). The" \
		"de-duplication assertions below are vacuous without it."
fi
check_eq "de-duplicated per-test failures" "5" \
	"$(wlcs_count "$complete_log" "$WLCS_RE_TEST_FAILED")"
check_eq "de-duplicated per-test skips" "4" \
	"$(wlcs_count "$complete_log" "$WLCS_RE_TEST_SKIPPED")"
check_eq "de-duplicated per-test passes" "3" \
	"$(wlcs_count "$complete_log" "$WLCS_RE_TEST_OK")"
check_eq "per-test started lines" "12" \
	"$(wlcs_count "$complete_log" "$WLCS_RE_TEST_STARTED")"

# The same for the aborted fixture, where the per-test counts are the only
# ones there are -- nothing else here would notice WLCS_RE_TEST_* rotting
# against that dialect either, since check 2 asserts the summarised line
# rather than the individual extractions.
aborted_log="$HERE/testdata/wlcs-1.7.0-aborted.log"
check_eq "aborted: per-test failures" "1" \
	"$(wlcs_count "$aborted_log" "$WLCS_RE_TEST_FAILED")"
check_eq "aborted: per-test skips" "1" \
	"$(wlcs_count "$aborted_log" "$WLCS_RE_TEST_SKIPPED")"
check_eq "aborted: per-test passes" "1" \
	"$(wlcs_count "$aborted_log" "$WLCS_RE_TEST_OK")"
check_eq "aborted: per-test started lines" "4" \
	"$(wlcs_count "$aborted_log" "$WLCS_RE_TEST_STARTED")"

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

# The line above is NOT sufficient on its own, and this is the trap that was
# actually sprung: with sum_failed/sum_skipped empty, `failed` and `skipped`
# fall back to the per-test counts and the summarised line comes out
# identical. So a WLCS_RE_SUM_* pattern that matches only the wlcs spelling
# passes the check above while quietly reducing summary.sh to ONE
# extraction for this dialect -- and the stale-pattern canary, which then
# compares the per-test count against itself, can never fire.
#
# Each end-of-run pattern is therefore asserted directly, in both dialects.
# Empty (the "no such line" answer) is a distinct, visible failure here.
check_summary_block() {
	# check_summary_block <name> <log> <total> <passed> <failed> <skipped>
	check_eq "$1 summary-block total" "$3" \
		"$(wlcs_summary_number "$2" "$WLCS_RE_SUM_TOTAL")"
	check_eq "$1 summary-block passed" "$4" \
		"$(wlcs_summary_number "$2" "$WLCS_RE_SUM_PASSED")"
	check_eq "$1 summary-block failed" "$5" \
		"$(wlcs_summary_number "$2" "$WLCS_RE_SUM_FAILED")"
	check_eq "$1 summary-block skipped" "$6" \
		"$(wlcs_summary_number "$2" "$WLCS_RE_SUM_SKIPPED")"
}
check_summary_block "stock gtest" "$TMP/stock-gtest.log" 8 2 2 4
check_summary_block "wlcs" "$complete_log" 12 3 5 4

# --- 7. the stderr diagnostics are the product, not a side effect -------
# summary.sh's claim is not "it prints numbers", it is "a number you should
# not trust is LOUD". Two mechanisms carry that claim and both write only to
# stderr: the aborted-run warning and the stale-pattern canary. `summarize`
# discards stderr, so without this section either could be deleted outright
# and every assertion above would still pass.
echo
echo "== summary.sh stderr diagnostics =="

CHECKS=$((CHECKS + 1))
if ! stderr_of "$aborted_log" | grep -qF '::warning::wlcs run ABORTED'; then
	fail "an aborted run printed no ABORTED warning on stderr"
fi

# A complete, self-consistent run must be SILENT. Without this, a canary
# that fired unconditionally would satisfy the check below.
CHECKS=$((CHECKS + 1))
quiet=$(stderr_of "$complete_log")
if [ -n "$quiet" ]; then
	fail "a consistent complete run printed a diagnostic on stderr: $quiet"
fi

# The canary itself. A log whose end-of-run block disagrees with its own
# per-test lines cannot be produced by a healthy parser -- that shape IS the
# symptom of a rotted pattern set -- so it is synthesised. Here the block
# claims 5 passes where only 2 per-test OK lines exist.
cat >"$TMP/disagreeing.log" <<'EOF'
[==========] Running 3 tests from 1 test suite.
[ RUN      ] T.a
[       OK ] T.a (0 ms)
[ RUN      ] T.b
[       OK ] T.b (0 ms)
[ RUN      ] T.c
[  FAILED  ] T.c (0 ms)
[----------] Global test environment tear-down
[==========] 3 tests from 1 test suite ran. (0 ms total)
[  PASSED  ] 5 tests.
[  FAILED  ] 1 test, listed below:
[  FAILED  ] T.c
EOF
check_eq "disagreeing log: summary block still wins" "3 5 1 0 complete" \
	"$(summarize "$TMP/disagreeing.log")"
CHECKS=$((CHECKS + 1))
if ! stderr_of "$TMP/disagreeing.log" |
	grep -qF 'parse disagreement: passed(summary=5 per-test=2)'; then
	fail "the two extractions disagreed and no canary fired on stderr; got:" \
		"$(stderr_of "$TMP/disagreeing.log")"
fi

# --- 8. run-advisory.sh end to end -------------------------------------
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
