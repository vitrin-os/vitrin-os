#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# WS-E.4.4 (issue #306) COMPONENT proof: the shim's idle-inhibit relay.
#
# THIS IS A COMPONENT TEST, NOT MILESTONE ACCEPTANCE. It drives the real shim
# against `tests/mock_core.c`, which is a hand-written stand-in for the trusted
# core (see that file's own header, and CLAUDE.md's definition-of-done rule). It
# proves what the SHIM sends; it proves nothing about what `vitrind` does with
# it, and in particular it cannot prove that a screen stayed lit -- blanking
# needs a display controller, and no CI runner has one.
#
# What it does settle, mechanically, is the pair of facts the feature's worst
# failure mode turns on:
#
#   (A) THE ORDINARY LIFECYCLE. An app that creates N inhibitors and destroys
#       them all produces exactly ONE `held` and exactly ONE `released` on the
#       wire -- the aggregation the wire's one-bit-per-realm shape requires. The
#       mock core FAILS a second `held`, so "the shim relays levels instead of
#       edges" is a red test rather than an invisible inefficiency.
#
#   (B) THE LEAK. An app that creates an inhibitor and is then killed WITHOUT
#       destroying it must still leave the shim releasing. This is the failure
#       that matters most: a leaked inhibit pins a human's panel awake forever,
#       and it is exactly what an app crashing mid-film does. The probe's
#       `--leak` mode exits with `_exit(0)` -- no destroy, no disconnect, no
#       atexit tidying -- so the release can only come from wlroots destroying
#       the resource with its client.
#
# Requires: vitrin-shim, mock-core, idle-probe (all built by
# `meson compile -C build`). Runs fully headless and GPU-free (pixman software
# renderer), with no Rust toolchain anywhere -- the shim CI job's standing
# invariant.
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/idle_inhibit.sh
set -Eeuo pipefail

BUILD_DIR="${BUILD_DIR:-./build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"
IDLE_PROBE="${IDLE_PROBE:-$BUILD_DIR/idle-probe}"

VIEW_W="${VIEW_W:-400}"
VIEW_H="${VIEW_H:-300}"
RUN_MS="${RUN_MS:-4000}"

for bin in "$SHIM_BIN" "$MOCK_CORE" "$IDLE_PROBE"; do
	[[ -x "$bin" ]] || { echo "FAIL: missing $bin (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
done

# Force pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-idle.XXXXXX")"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

cleanup() {
	local rc=$?
	pkill -x mock-core 2>/dev/null || true
	pkill -x vitrin-shim 2>/dev/null || true
	pkill -x idle-probe 2>/dev/null || true
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

summary_field() { sed -n 's/.*[[:space:]]'"$2"'=\([^[:space:]]*\).*/\1/p' <<<"$(grep '^SUMMARY' "$1")"; }

# Start the mock core (which spawns the shim exactly as the real core does:
# socketpair, fd 3, FD_CLOEXEC cleared), wait for the shim's socket, then run
# the probe against it as the app.
# $1 = tag, rest = probe argv
run_scenario() {
	local tag="$1"; shift
	local socket="vitrin-$tag-$$"
	CORE_LOG="$RUNTIME_DIR/$tag.core.log"
	SHIM_LOG="$RUNTIME_DIR/$tag.shim.log"
	APP_LOG="$RUNTIME_DIR/$tag.app.log"

	"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms "$RUN_MS" \
		-- "$SHIM_BIN" --socket "$socket" >"$CORE_LOG" 2>"$SHIM_LOG" &
	CORE_PID=$!

	local sock="$XDG_RUNTIME_DIR/$socket"
	local deadline=$(( $(date +%s) + 10 ))
	until [[ -S "$sock" ]]; do
		kill -0 "$CORE_PID" 2>/dev/null || fail "[$tag] the shim never bound a socket; core log:
$(cat "$CORE_LOG")
shim log:
$(cat "$SHIM_LOG")"
		(( $(date +%s) >= deadline )) && fail "[$tag] timeout waiting for $sock"
		sleep 0.1
	done

	WAYLAND_DISPLAY="$socket" timeout 5 "$@" --out "$APP_LOG" >/dev/null 2>&1 || true
	wait "$CORE_PID" || true
	if grep -q '^FAIL' "$CORE_LOG"; then
		fail "[$tag] the mock core reported wire violations:
$(grep '^FAIL' "$CORE_LOG")"
	fi
}

# The global has to be advertised at all, and the probe is what says so from the
# app's side. Before #306 this line did not exist, so its absence is a real
# historical state rather than a hypothetical.
assert_bound() {
	local tag="$1"
	grep -q '^IDLE bound manager' "$RUNTIME_DIR/$tag.app.log" \
		|| fail "[$tag] the app never saw zwp_idle_inhibit_manager_v1; app log:
$(cat "$RUNTIME_DIR/$tag.app.log")"
}

# Exactly one held, exactly one released, in that order, and nothing held at
# teardown.
assert_one_edge_pair() {
	local tag="$1"
	local log="$RUNTIME_DIR/$tag.core.log"
	local held released
	held="$(grep -c '^EV idle_inhibit state=held' "$log" || true)"
	released="$(grep -c '^EV idle_inhibit state=released' "$log" || true)"
	(( held == 1 )) || fail "[$tag] expected exactly one \`held\`, saw $held:
$(grep '^EV idle_inhibit' "$log" || echo '(none)')"
	(( released == 1 )) || fail "[$tag] expected exactly one \`released\`, saw $released:
$(grep '^EV idle_inhibit' "$log" || echo '(none)')"
	[[ "$(grep -n '^EV idle_inhibit state=held' "$log" | cut -d: -f1)" \
		-lt "$(grep -n '^EV idle_inhibit state=released' "$log" | cut -d: -f1)" ]] \
		|| fail "[$tag] the release preceded the hold"
	[[ "$(summary_field "$log" idle_edges)" == "2" ]] \
		|| fail "[$tag] expected 2 idle edges, got $(summary_field "$log" idle_edges)"
	[[ "$(summary_field "$log" idle_held)" == "0" ]] \
		|| fail "[$tag] the shim was still holding an inhibit at teardown -- a human's panel \
would never blank again"
}

echo "== (A) three inhibitors, all destroyed: one held, one released =="
run_scenario destroy "$IDLE_PROBE" --destroy --count 3
assert_bound destroy
grep -q '^SUMMARY status=destroyed held=0' "$RUNTIME_DIR/destroy.app.log" \
	|| fail "the probe did not destroy all three inhibitors:
$(cat "$RUNTIME_DIR/destroy.app.log")"
assert_one_edge_pair destroy
ok "three inhibitors aggregated into exactly one held/released pair on the wire"

echo "== (B) an app killed holding an inhibitor still releases =="
run_scenario leak "$IDLE_PROBE" --leak --count 1
assert_bound leak
grep -q '^SUMMARY status=leaked held=1' "$RUNTIME_DIR/leak.app.log" \
	|| fail "the probe did not leak its inhibitor, so (B) proves nothing:
$(cat "$RUNTIME_DIR/leak.app.log")"
assert_one_edge_pair leak
ok "a leaked inhibitor is released by the shim when its client disconnects"

echo "PASS: shim/tests/acceptance/idle_inhibit.sh (COMPONENT test against mock_core.c)"
