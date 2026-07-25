#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# summary.sh -- parse a wlcs run log into total/passed/failed/skipped counts.
#
# Sourced by run-advisory.sh and exercised directly by test-summary.sh against
# checked-in, real wlcs output in testdata/. Kept in its own file precisely so
# the self-test can drive the parsing without a wlcs binary, a built module or
# a compositor: the counting is the part with no other way to test it, and the
# part whose failure mode is a plausible-looking zero rather than an error.
#
# WHY THIS IS NOT "JUST GREP FOR GOOGLETEST'S SUMMARY". The wlcs runner does
# NOT print stock googletest output. It installs its own gtest event listener,
# and that listener's per-test and end-of-run lines differ from
# PrettyUnitTestResultPrinter's in ways that matter to every pattern below:
#
#   wlcs                                  stock googletest
#   ------------------------------------  ---------------------------------
#   [     SKIP ] Suite.test (1ms)         [  SKIPPED ] Suite.test (1 ms)
#   [  FAILED  ] 4 tests failed:          [  FAILED  ] 4 tests, listed below:
#   [  SKIPPED ] 4 tests skipped:         [  SKIPPED ] 4 tests, listed below:
#   [  PASSED  ] 5 tests                  [  PASSED  ] 5 tests.
#   [=====] 13 tests from 4 test cases    [=====] 13 tests from 4 test suites
#           run. (63ms total elapsed)             ran. (63 ms total)
#
# Both dialects are accepted below, because which one you get depends on the
# wlcs build, not on anything this repository controls -- and test-summary.sh
# asserts that for every pattern in BOTH dialects, per-test AND end-of-run,
# because a summary pattern that matches only one dialect degrades this file
# to a single extraction without failing anything. testdata/*.log is real
# wlcs output; see test-summary.sh for the provenance of each fixture.
#
# TWO INDEPENDENT EXTRACTIONS, ON PURPOSE. Every count is derived twice:
#
#   1. from the per-test result lines the runner prints as it goes, and
#   2. from the end-of-run summary block the runner prints when it finishes.
#
# (2) is authoritative when present -- it is the runner's own tally. (1) is
# what makes an ABORTED run (the runner segfaulting mid-suite, which is a live
# failure mode for this module: see README.md's "Known hazard") report the
# failures it did observe instead of a summary block that does not exist and
# therefore reads as "failed=0". When both are available and they disagree,
# that means one of the two pattern sets has gone stale against a newer wlcs,
# and wlcs_summarize_log says so loudly on stderr rather than silently
# reporting a zero. A silent zero is the exact defect this file exists to make
# impossible.

# Per-test result lines. Anchored on the trailing "(N ms)" / "(Nms)" duration,
# which is what distinguishes a per-test line from the SAME tag re-printed
# without a duration in the end-of-run listing -- counting the tag alone would
# double-count every failed and skipped test.
WLCS_RE_TEST_OK='^\[       OK \] .+ \([0-9]+ ?ms\)$'
WLCS_RE_TEST_FAILED='^\[  FAILED  \] .+ \([0-9]+ ?ms\)$'
# Both spellings: wlcs's own listener uses "[     SKIP ]", stock googletest
# uses "[  SKIPPED ]".
WLCS_RE_TEST_SKIPPED='^\[(     SKIP |  SKIPPED )\] .+ \([0-9]+ ?ms\)$'
WLCS_RE_TEST_STARTED='^\[ RUN      \] '

# End-of-run summary block.
# "N tests from M test cases run." (wlcs) / "N tests from M test suites ran."
# (stock googletest). The leading "[0-9]+ tests?" is what keeps this from also
# matching the START-of-run line, "[==========] Running N tests from ...".
WLCS_RE_SUM_TOTAL='^\[==========\] [0-9]+ tests? from .*(ran|run)\.'
WLCS_RE_SUM_PASSED='^\[  PASSED  \] [0-9]+ tests?[.]?$'
# NOTE THE PLACEMENT OF THE SPACE. wlcs writes "5 tests failed:" (space,
# then the word); stock googletest writes "2 tests, listed below:" (comma
# IMMEDIATELY after the count, no space). Putting the space outside the
# alternation -- `tests? (failed|, listed below)` -- silently matches only
# the wlcs spelling, which costs nothing on a wlcs log and quietly reduces
# summary.sh to a SINGLE extraction on a stock-googletest one: sum_failed
# and sum_skipped stay empty, the per-test fallback becomes the only
# source, and the canary below then compares that source with itself and
# can never fire. The space therefore belongs inside the alternation.
# test-summary.sh asserts both spellings against both dialects.
WLCS_RE_SUM_FAILED='^\[  FAILED  \] [0-9]+ tests?( failed|, listed below):'
WLCS_RE_SUM_SKIPPED='^\[  SKIPPED \] [0-9]+ tests?( skipped|, listed below):'

# wlcs_count <log> <ere> -- number of matching lines, always a decimal integer
# on stdout, always exit 0.
#
# The `|| true` and the `${n:-0}` are BOTH load-bearing, and for different
# reasons. `grep -c` exits 1 when it matches nothing (a normal, expected
# outcome here: a clean run has no failures) and exits 2 when the file does
# not exist (also expected: the runner can die before creating the log). The
# first case still prints "0"; the second prints nothing at all. Without both
# guards, `set -euo pipefail` in the caller turns either into an abort before
# run-advisory.sh's `exit 0` -- which is the CI incident the long comment in
# run-advisory.sh describes, and the one guarantee that script exists to keep.
wlcs_count() {
	local n
	n=$(grep -cE "$2" "$1" 2>/dev/null || true)
	printf '%s' "${n:-0}"
}

# wlcs_summary_number <log> <ere> -- the first integer on the last line
# matching <ere>, or the empty string when there is no such line. Empty is a
# meaningful answer here ("the runner never printed this summary line"), which
# is why it is not normalised to 0: 0 and "absent" lead to different reporting.
wlcs_summary_number() {
	local line
	line=$(grep -E "$2" "$1" 2>/dev/null | tail -1 || true)
	if [ -z "$line" ]; then
		return 0
	fi
	printf '%s' "$line" | grep -oE '[0-9]+' | head -1 || true
}

# wlcs_summarize_log <log>
#
# Prints one line: "<total> <passed> <failed> <skipped> <status>", where
# status is one of:
#
#   complete   the runner printed its end-of-run summary block
#   aborted    the runner started at least one test and then died without
#              printing that block (segfault, timeout kill, ...) -- the counts
#              are what completed before it died, not the whole suite
#   no-output  no test ever started (module failed to dlopen, usage error,
#              empty or missing log)
#
# Always exits 0. Diagnostics, if any, go to stderr.
wlcs_summarize_log() {
	local log="$1"
	local seen_ok seen_failed seen_skipped seen_started
	local sum_total sum_passed sum_failed sum_skipped
	local total passed failed skipped status

	seen_ok=$(wlcs_count "$log" "$WLCS_RE_TEST_OK")
	seen_failed=$(wlcs_count "$log" "$WLCS_RE_TEST_FAILED")
	seen_skipped=$(wlcs_count "$log" "$WLCS_RE_TEST_SKIPPED")
	seen_started=$(wlcs_count "$log" "$WLCS_RE_TEST_STARTED")

	sum_total=$(wlcs_summary_number "$log" "$WLCS_RE_SUM_TOTAL")
	sum_passed=$(wlcs_summary_number "$log" "$WLCS_RE_SUM_PASSED")
	sum_failed=$(wlcs_summary_number "$log" "$WLCS_RE_SUM_FAILED")
	sum_skipped=$(wlcs_summary_number "$log" "$WLCS_RE_SUM_SKIPPED")

	if [ -n "$sum_total" ]; then
		status="complete"
	elif [ "$seen_started" -gt 0 ]; then
		status="aborted"
	else
		status="no-output"
	fi

	# Prefer the runner's own tally where it exists; fall back to the
	# per-test lines otherwise. Note the asymmetry: a MISSING failure/skip
	# summary line in a complete run legitimately means zero (the runner
	# only prints those headers when the count is non-zero), so the
	# fallback there is the per-test count, which is zero too in that case
	# -- and non-zero exactly when the header pattern has gone stale.
	passed="${sum_passed:-$seen_ok}"
	failed="${sum_failed:-$seen_failed}"
	skipped="${sum_skipped:-$seen_skipped}"
	if [ "$status" = "complete" ]; then
		total="$sum_total"
	else
		total=$((passed + failed + skipped))
	fi

	if [ "$status" = "aborted" ]; then
		echo "::warning::wlcs run ABORTED: $seen_started test(s) started but" \
			"the runner never printed its end-of-run summary (it died" \
			"mid-suite). The counts below are only what completed before" \
			"that -- treat them as a floor, not a tally." >&2
	fi

	# The stale-pattern canary. Only meaningful for a complete run, where
	# both extractions describe the same finished suite.
	if [ "$status" = "complete" ]; then
		local mismatch=""
		[ "$passed" = "$seen_ok" ] || mismatch="$mismatch passed(summary=$passed per-test=$seen_ok)"
		[ "$failed" = "$seen_failed" ] || mismatch="$mismatch failed(summary=$failed per-test=$seen_failed)"
		[ "$skipped" = "$seen_skipped" ] || mismatch="$mismatch skipped(summary=$skipped per-test=$seen_skipped)"
		if [ -n "$mismatch" ]; then
			echo "::warning::wlcs summary parse disagreement:$mismatch." \
				"One of the two pattern sets in shim/wlcs/summary.sh has" \
				"gone stale against this wlcs build -- fix the patterns" \
				"and extend shim/wlcs/test-summary.sh with a capture of" \
				"this run's output before trusting these numbers." >&2
		fi
	fi

	echo "$total $passed $failed $skipped $status"
}
