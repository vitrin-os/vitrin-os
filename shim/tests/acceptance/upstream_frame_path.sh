#!/usr/bin/env bash
# P1.6.2 acceptance proof: the shim's upstream link to the trusted core.
#
# Every check below is made against the WIRE, by a peer that spawns the shim
# exactly as the core does (socketpair, fd 3, FD_CLOEXEC cleared) and applies
# the real core's validation rules to everything the shim sends -- see
# tests/mock_core.c. The shim is never asked whether it thinks it worked.
#
# The four acceptance criteria of issue #34 and where each is proven:
#
#   1. "the app's window appears in the core's nested window, rendered
#      through shim -> core"
#         (A) session bring-up + a real weston-terminal window forwarded as
#             valid, validated shm frames.
#   2. "damage-only updates verified in logs (a small repaint does not
#      forward the full surface)"
#         (B) a client whose repaint is a known 32x32 square; the core sees
#             32x32 damage against a whole-surface buffer.
#   3. "app frame callbacks fire at the core's composition cadence (no
#      free-running client)"
#         (C) the same client against a core deliberately presenting at
#             10Hz; its callback count must follow the core, not the shim's
#             own output timer.
#   4. "shim survives core-side slow frames without unbounded buffering"
#         (D) a client committing as fast as it can into a 10Hz core, with
#             the shim's descriptor count and RSS sampled throughout.
#
# Plus two structural checks the criteria depend on but do not name:
#         (E) the shim refuses to start without the core connection, and
#             re-arms FD_CLOEXEC on it so no app can inherit it.
#         (F) deferred `buffer_done` (the dmabuf passthrough timing regime)
#             does not deadlock the buffer pool.
#
# Requires: vitrin-shim, mock-core, damage-client (all built by
# `meson compile -C build`) and weston-terminal for (A). Runs fully headless
# and GPU-free (pixman software renderer), with no Rust toolchain anywhere --
# the shim CI job's standing invariant.
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/upstream_frame_path.sh
set -Eeuo pipefail

BUILD_DIR="${BUILD_DIR:-./build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"
DAMAGE_CLIENT="${DAMAGE_CLIENT:-$BUILD_DIR/damage-client}"

VIEW_W="${VIEW_W:-400}"
VIEW_H="${VIEW_H:-300}"
SURFACE_AREA=$((VIEW_W * VIEW_H))
# The core's presentation period for the pacing/backpressure checks. Slow
# enough that "paced by the core" and "paced by the shim's own 60Hz output"
# are separated by an order of magnitude, so the assertion needs no tight
# timing tolerance.
SLOW_MS="${SLOW_MS:-100}"

for bin in "$SHIM_BIN" "$MOCK_CORE" "$DAMAGE_CLIENT"; do
	[[ -x "$bin" ]] || { echo "FAIL: missing $bin (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
done

# Force pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-up.XXXXXX")"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

cleanup() {
	local rc=$?
	pkill -x mock-core 2>/dev/null || true
	pkill -x vitrin-shim 2>/dev/null || true
	pkill -x damage-client 2>/dev/null || true
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# Read a `key=value` token out of the mock core's SUMMARY line.
summary_field() { sed -n 's/.*[[:space:]]'"$2"'=\([^[:space:]]*\).*/\1/p' <<<"$(grep '^SUMMARY' "$1")"; }

# Run one scenario: start the mock core (which spawns the shim), give the
# socket a moment, then run $3.. as the app against it.
# $1 = tag, $2 = extra mock-core args (space separated), rest = app argv
run_scenario() {
	local tag="$1"; shift
	local core_args="$1"; shift
	local socket="vitrin-$tag-$$"
	CORE_LOG="$RUNTIME_DIR/$tag.core.log"
	SHIM_LOG="$RUNTIME_DIR/$tag.shim.log"
	APP_LOG="$RUNTIME_DIR/$tag.app.log"

	# shellcheck disable=SC2086
	"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" $core_args \
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
	SHIM_PID="$(sed -n 's/^EV spawned shim_pid=\([0-9]*\).*/\1/p' "$CORE_LOG")"
	[[ -n "$SHIM_PID" ]] || fail "[$tag] the mock core did not report a shim pid"

	if (( $# > 0 )); then
		WAYLAND_DISPLAY="$socket" "$@" >"$APP_LOG" 2>&1 || true
	fi
	wait "$CORE_PID" || fail "[$tag] the mock core reported protocol violations:
$(grep '^FAIL' "$CORE_LOG" || true)"
	grep -q '^FAIL' "$CORE_LOG" && fail "[$tag] wire violations:
$(grep '^FAIL' "$CORE_LOG")"
	return 0
}

echo "== (A) session bring-up and a real app's window through the wire =="
if command -v weston-terminal >/dev/null 2>&1; then
	run_scenario wt "--run-ms 7000" timeout 5 weston-terminal
	# Flow G of docs/protocol/09: configure first, then the two mints, with
	# strictly increasing ids above the bootstrap object.
	grep -q '^EV configure realm=realm-0' "$CORE_LOG" || fail "no configure was sent"
	grep -q '^EV create_surface id=2$' "$CORE_LOG" || fail "surface was not minted as id 2"
	grep -q '^EV get_seat id=3$' "$CORE_LOG" || fail "seat was not minted as id 3"
	[[ "$(grep -n '^EV create_surface' "$CORE_LOG" | cut -d: -f1)" \
		-lt "$(grep -n '^EV get_seat' "$CORE_LOG" | cut -d: -f1)" ]] \
		|| fail "get_seat must follow create_surface (watermark rule)"
	grep -q 'app window mapped' "$SHIM_LOG" || fail "weston-terminal's window never mapped"
	# Every attach: shm kind, a v0 format, the realm-view geometry, a tight
	# stride, a seal-capable fd big enough for the geometry it declares.
	# (mock_core.c has already enforced all of this; this is the shape check
	# the log makes human-readable.)
	grep -qE "^EV attach buffer_id=[0-9]+ kind=0 format=0x34325258 w=$VIEW_W h=$VIEW_H stride=$((VIEW_W * 4)) " \
		"$CORE_LOG" || fail "no well-formed shm attach at the configured geometry:
$(grep '^EV attach' "$CORE_LOG" | head -3)"
	commits="$(summary_field "$CORE_LOG" commits)"
	(( commits >= 1 )) || fail "no frame reached the core"
	ok "bring-up (configure -> create_surface=2 -> get_seat=3), window mapped, $commits frame(s) forwarded"
else
	echo "SKIP: weston-terminal not installed; (A) falls back to the synthetic client"
	run_scenario wt "--run-ms 5000" timeout 3 "$DAMAGE_CLIENT" --run-ms 2000
	grep -q '^EV create_surface id=2$' "$CORE_LOG" || fail "surface was not minted as id 2"
	ok "bring-up verified without weston-terminal"
fi

echo "== (B) damage-only updates do not forward the full surface =="
run_scenario damage "--run-ms 6000" timeout 6 "$DAMAGE_CLIENT" --run-ms 3500
# The client's steady-state repaint is one 32x32 square, so the core must see
# 32x32 damage rectangles against 400x300 buffers. If the shim coalesced
# damage to the full surface this line finds nothing.
small="$(grep -c "^EV damage x=[0-9]* y=[0-9]* w=32 h=32$" "$CORE_LOG" || true)"
(( small >= 5 )) || fail "expected 32x32 damage rectangles, saw $small:
$(grep '^EV damage' "$CORE_LOG" | sort | uniq -c | sort -rn | head)"
# And the commits carrying them must declare far less than the whole surface.
small_commits="$(awk -v area="$SURFACE_AREA" \
	'/^EV commit/ { for (i=1;i<=NF;i++) if ($i ~ /^damage_area=/) { split($i,a,"="); if (a[2] * 20 <= area) n++ } } END { print n+0 }' \
	"$CORE_LOG")"
total_commits="$(summary_field "$CORE_LOG" commits)"
(( small_commits >= 5 )) || fail "no commit forwarded less than 5% of the surface ($small_commits of $total_commits)"
(( small_commits * 2 > total_commits )) \
	|| fail "only $small_commits of $total_commits commits were damage-only; expected a majority"
ok "$small_commits of $total_commits commits forwarded <=5% of the surface ($small were exactly 32x32)"

echo "== (C) the app is paced by the core, not by the shim =="
# Baseline: a fast core. The client draws on frame callbacks, so this is the
# rate the shim's own output timer allows.
run_scenario fast "--run-ms 5000" timeout 6 "$DAMAGE_CLIENT" --run-ms 3000
fast_client="$(sed -n 's/^CLIENT frames=\([0-9]*\).*/\1/p' "$APP_LOG")"
fast_core="$(summary_field "$CORE_LOG" frame_dones)"
(( fast_client >= 60 )) || fail "baseline too slow to be a baseline: $fast_client frames in 3s"
ok "fast core: client saw $fast_client frame callbacks, core presented $fast_core"

# Same client, same duration, against a core presenting at ~10Hz.
run_scenario slow "--run-ms 5000 --frame-delay $SLOW_MS" timeout 6 "$DAMAGE_CLIENT" --run-ms 3000
slow_client="$(sed -n 's/^CLIENT frames=\([0-9]*\).*/\1/p' "$APP_LOG")"
slow_core="$(summary_field "$CORE_LOG" frame_dones)"
# The claim is exactly "the callbacks ARE the core's frame_dones": one
# in-flight frame, one callback per presentation, so the two counts must
# agree within the couple of frames in flight at the ends of the run.
diff=$(( slow_client - slow_core )); (( diff < 0 )) && diff=$(( -diff ))
(( diff <= 3 )) || fail "client callbacks ($slow_client) do not track the core's presentations ($slow_core)"
# And it must be an order of magnitude below what the shim alone would allow,
# which is what "no free-running client" means.
(( slow_client * 4 < fast_client )) \
	|| fail "the slow core did not throttle the app: $slow_client vs $fast_client on a fast core"
ok "slow core: client saw $slow_client callbacks vs the core's $slow_core presentations (baseline $fast_client)"

echo "== (D) a free-running app against a slow core buffers nothing =="
socket="vitrin-bp-$$"
CORE_LOG="$RUNTIME_DIR/bp.core.log"
SHIM_LOG="$RUNTIME_DIR/bp.shim.log"
APP_LOG="$RUNTIME_DIR/bp.app.log"
"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms 7000 --frame-delay "$SLOW_MS" \
	-- "$SHIM_BIN" --socket "$socket" >"$CORE_LOG" 2>"$SHIM_LOG" &
CORE_PID=$!
sock="$XDG_RUNTIME_DIR/$socket"
deadline=$(( $(date +%s) + 10 ))
until [[ -S "$sock" ]]; do
	(( $(date +%s) >= deadline )) && fail "[bp] timeout waiting for $sock"
	sleep 0.1
done
SHIM_PID="$(sed -n 's/^EV spawned shim_pid=\([0-9]*\).*/\1/p' "$CORE_LOG")"

WAYLAND_DISPLAY="$socket" timeout 8 "$DAMAGE_CLIENT" --run-ms 5000 --free-run >"$APP_LOG" 2>&1 &
APP_PID=$!
# Sample the shim's resource footprint while the app floods it. Unbounded
# buffering would show up as either growth: a pooled memfd is a descriptor
# AND a mapping. Sampling starts after a warm-up, because the swapchain, the
# buffer pool and the client's own buffers are all allocated on first use --
# that one-time ramp is not the growth this check is looking for.
SAMPLES="$RUNTIME_DIR/bp.samples"
: >"$SAMPLES"
sleep 1.5
for _ in $(seq 1 12); do
	if [[ -d "/proc/$SHIM_PID" ]]; then
		echo "$(ls "/proc/$SHIM_PID/fd" 2>/dev/null | wc -l) $(awk '/VmRSS/{print $2}' "/proc/$SHIM_PID/status" 2>/dev/null)" >>"$SAMPLES"
	fi
	sleep 0.3
done
wait "$APP_PID" || true
wait "$CORE_PID" || fail "[bp] the mock core reported protocol violations:
$(grep '^FAIL' "$CORE_LOG" || true)"

app_commits="$(sed -n 's/^CLIENT .*commits=\([0-9]*\).*/\1/p' "$APP_LOG")"
forwarded="$(summary_field "$CORE_LOG" commits)"
max_inflight="$(summary_field "$CORE_LOG" max_inflight)"
[[ -n "$app_commits" && -n "$forwarded" ]] || fail "[bp] missing counters"
grep -q 'error in client communication' "$SHIM_LOG" \
	&& fail "the flooding client killed its own connection; the run proves nothing about the shim"
(( app_commits > forwarded * 5 )) \
	|| fail "the app did not out-run the core ($app_commits commits vs $forwarded forwarded); the test proves nothing"
(( max_inflight == 1 )) \
	|| fail "the shim had $max_inflight commits in flight at once; the frame path must admit exactly one"

fd_min="$(awk 'NR==1{m=$1} $1<m{m=$1} END{print m}' "$SAMPLES")"
fd_max="$(awk 'NR==1{m=$1} $1>m{m=$1} END{print m}' "$SAMPLES")"
rss_min="$(awk 'NR==1{m=$2} $2<m{m=$2} END{print m}' "$SAMPLES")"
rss_max="$(awk 'NR==1{m=$2} $2>m{m=$2} END{print m}' "$SAMPLES")"
(( fd_max - fd_min <= 2 )) \
	|| fail "the shim's descriptor count grew from $fd_min to $fd_max under load (leaking buffers?)"
# The pool is two buffers; a shim that queued frames would grow by a whole
# buffer at a time (VIEW_W*VIEW_H*4 bytes each). 2 MiB of slack is far below
# one buffer at any realistic view size and far above allocator noise.
(( (rss_max - rss_min) < 2048 )) \
	|| fail "the shim's RSS grew from ${rss_min}kB to ${rss_max}kB under load (queueing frames?)"
deferred="$(sed -n 's/.*deferred=\([0-9]*\).*/\1/p' <<<"$(grep 'vitrin-shim down' "$SHIM_LOG")")"
ok "app committed $app_commits frames, core presented $forwarded, max in flight $max_inflight"
ok "shim footprint flat under load: fds $fd_min..$fd_max, RSS ${rss_min}..${rss_max} kB (${deferred:-0} composites skipped)"

echo "== (E) the core connection is mandatory and never inherited =="
# Holding fd 3 IS being a realm's shim, so a process without one must not
# pretend to be one.
set +e
"$SHIM_BIN" --socket "should-not-bind-$$" >"$RUNTIME_DIR/nofd.log" 2>&1 3<&-
nofd_rc=$?
set -e
(( nofd_rc != 0 )) || fail "the shim started with no core connection at fd 3"
grep -q "fd 3 is not open" "$RUNTIME_DIR/nofd.log" \
	|| fail "the shim's refusal did not name the missing descriptor"
# ... and when it does have one, FD_CLOEXEC must be re-armed on it (it arrives
# CLEARED, which is how it survived execve). O_CLOEXEC is 02000000 octal in
# /proc/PID/fdinfo/N's flags field.
socket="vitrin-cloexec-$$"
"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms 3000 \
	-- "$SHIM_BIN" --socket "$socket" >"$RUNTIME_DIR/cx.core.log" 2>/dev/null &
CORE_PID=$!
deadline=$(( $(date +%s) + 10 ))
until [[ -S "$XDG_RUNTIME_DIR/$socket" ]]; do
	(( $(date +%s) >= deadline )) && fail "[cloexec] timeout waiting for the socket"
	sleep 0.1
done
SHIM_PID="$(sed -n 's/^EV spawned shim_pid=\([0-9]*\).*/\1/p' "$RUNTIME_DIR/cx.core.log")"
flags="$(sed -n 's/^flags:[[:space:]]*\([0-7]*\)/\1/p' "/proc/$SHIM_PID/fdinfo/3")"
[[ -n "$flags" ]] || fail "[cloexec] cannot read the shim's fd 3 flags"
(( (8#$flags & 8#2000000) != 0 )) \
	|| fail "FD_CLOEXEC is NOT set on the shim's core connection (flags=$flags): every app it spawns would inherit the core"
wait "$CORE_PID" || true
ok "the shim refuses to start without fd 3, and sets FD_CLOEXEC on it when it has one (flags=$flags)"

echo "== (F) deferred buffer release does not deadlock the pool =="
# `buffer_done` withheld until the NEXT commit is the dmabuf passthrough
# timing regime. A one-buffer pool cannot build the replacement it is being
# asked for and stops dead; the two-slot pool must keep going.
run_scenario defer "--run-ms 5000 --frame-delay 40 --defer-release" \
	timeout 6 "$DAMAGE_CLIENT" --run-ms 3000
defer_commits="$(summary_field "$CORE_LOG" commits)"
(( defer_commits >= 10 )) \
	|| fail "only $defer_commits commits under deferred release; the buffer pool deadlocked"
ok "$defer_commits frames forwarded with buffer_done deferred to the next commit"

echo "== (G) socketpair EOF alone shuts the shim down =="
# Hanging up is the FIRST rung of the core's orderly shutdown ladder (P1.5.3);
# SIGTERM is the second. A shim that only died on the signal would make every
# clean shutdown take the timeout path -- and, worse, would sit spinning on a
# hangup that stays readable forever. The mock core closes, then waits ~500ms
# before signalling, so "shut down cleanly" and "needed the signal" are
# distinguishable in the log.
run_scenario eof "--run-ms 1500"
grep -q "the core closed the connection" "$SHIM_LOG" \
	|| fail "the shim did not act on socketpair EOF; it had to be signalled:
$(tail -3 "$SHIM_LOG")"
ok "the shim shut down on hangup alone, without waiting for a signal"

echo
echo "PASS: all P1.6.2 upstream-link acceptance checks green"
