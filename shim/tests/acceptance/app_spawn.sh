#!/usr/bin/env bash
# P1.6.5 acceptance proof: the SHIM ITSELF forks and execs the app.
#
# Up to now the app was launched EXTERNALLY, next to a shim already running
# (shim_globals_and_client.sh, upstream_frame_path.sh). #104 moves that launch
# INTO the shim: the core execs the shim holding fd 3 and conveys the app
# command after a `--`, and the shim forks/execs it once its session is up. So
# the process tree is finally one spine -- (mock) core -> vitrin-shim ->
# weston-terminal -- and this script proves exactly that spine, headless.
#
# The core here is tests/mock-core (the same peer the upstream-link proof uses,
# reproducing the real spawn contract: socketpair, fd 3, FD_CLOEXEC cleared).
# It is told to spawn `<shim> --socket X -- /usr/bin/weston-terminal`, so the
# shim -- not this script -- is what forks the terminal.
#
# Four assertions, each a mechanical check on the real process tree/wire:
#   (a) ancestry: weston-terminal is a CHILD of the shim (the shim forked it).
#   (b) a frame: at least one committed frame reaches the core through the
#       shim (the spawned app really rendered, not just started).
#   (c) isolation: the app's /proc/<pid>/fd holds NONE of the shim's private
#       descriptors -- above all not the shim's core socket (fd 3). The check
#       is by inode, so it cannot be fooled by descriptor-number reuse.
#   (d) teardown: SIGTERM to the shim reaps the app -- it is gone, not a
#       zombie -- so killing the shim never orphans its app.
#
# Requires: vitrin-shim + mock-core (meson compile -C build) and
# weston-terminal. Runs fully headless and GPU-free (pixman). weston-terminal
# missing is a FAIL, not a skip, unless VITRIN_SKIP_APP_SPAWN=1 is set (the
# opt-out for a machine that genuinely cannot install it).
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/app_spawn.sh
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM_DIR="$(cd "$HERE/../.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$SHIM_DIR/build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"
APP_BIN="${APP_BIN:-/usr/bin/weston-terminal}"

VIEW_W="${VIEW_W:-400}"
VIEW_H="${VIEW_H:-300}"
# Long enough that all of (a)-(c) settle before the mock core would tear the
# run down on its own; (d) kills the shim well inside it.
RUN_MS="${RUN_MS:-20000}"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

for bin in "$SHIM_BIN" "$MOCK_CORE"; do
	[[ -x "$bin" ]] || fail "missing $bin (run: meson compile -C $BUILD_DIR)"
done
if [[ ! -x "$APP_BIN" ]]; then
	[[ "${VITRIN_SKIP_APP_SPAWN:-0}" == "1" ]] && { echo "SKIP: $APP_BIN not installed (VITRIN_SKIP_APP_SPAWN=1)"; exit 0; }
	fail "$APP_BIN not installed; install weston, or set VITRIN_SKIP_APP_SPAWN=1 to opt out"
fi
APP_NAME="$(basename "$APP_BIN")"

# Force pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-spawn.XXXXXX")"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

CORE_PID=""
cleanup() {
	local rc=$?
	[[ -n "$CORE_PID" ]] && kill "$CORE_PID" 2>/dev/null || true
	pkill -x mock-core 2>/dev/null || true
	pkill -x vitrin-shim 2>/dev/null || true
	pkill -x "$APP_NAME" 2>/dev/null || true
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM

SOCKET="vitrin-spawn-$$"
CORE_LOG="$RUNTIME_DIR/core.log"
SHIM_LOG="$RUNTIME_DIR/shim.log"

# The mock core spawns `<shim> --socket X -- <app>`: everything after ITS `--`
# is the shim argv, and the second `--` there is the shim's own app separator.
"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms "$RUN_MS" \
	-- "$SHIM_BIN" --socket "$SOCKET" -- "$APP_BIN" \
	>"$CORE_LOG" 2>"$SHIM_LOG" &
CORE_PID=$!

SOCK="$XDG_RUNTIME_DIR/$SOCKET"
deadline=$(( $(date +%s) + 10 ))
until [[ -S "$SOCK" ]]; do
	kill -0 "$CORE_PID" 2>/dev/null || fail "the shim never bound a socket; core log:
$(cat "$CORE_LOG")
shim log:
$(cat "$SHIM_LOG")"
	(( $(date +%s) >= deadline )) && fail "timeout waiting for $SOCK"
	sleep 0.1
done
SHIM_PID="$(sed -n 's/^EV spawned shim_pid=\([0-9]*\).*/\1/p' "$CORE_LOG")"
[[ -n "$SHIM_PID" ]] || fail "the mock core did not report a shim pid"
ok "shim up (pid $SHIM_PID), socket $SOCK"

# (a) ancestry -- weston-terminal is a child of the shim (the shim forked it).
APP_PID=""
deadline=$(( $(date +%s) + 10 ))
while [[ -z "$APP_PID" ]]; do
	kill -0 "$SHIM_PID" 2>/dev/null || fail "the shim died before spawning the app; shim log:
$(cat "$SHIM_LOG")"
	APP_PID="$(pgrep -P "$SHIM_PID" -x "$APP_NAME" 2>/dev/null | head -n1 || true)"
	[[ -n "$APP_PID" ]] && break
	(( $(date +%s) >= deadline )) && fail "the shim never forked $APP_NAME as a child; shim log:
$(cat "$SHIM_LOG")"
	sleep 0.1
done
ppid="$(awk '{print $4}' "/proc/$APP_PID/stat" 2>/dev/null || echo '?')"
[[ "$ppid" == "$SHIM_PID" ]] || fail "$APP_NAME (pid $APP_PID) parent is $ppid, not the shim $SHIM_PID"
grep -q "spawned app pid=$APP_PID" "$SHIM_LOG" || fail "the shim did not log spawning pid $APP_PID"
ok "(a) $APP_NAME (pid $APP_PID) is a child of the shim (pid $SHIM_PID)"

# (b) a frame -- the spawned app rendered and the shim forwarded it upstream.
deadline=$(( $(date +%s) + 15 ))
until grep -q '^EV commit n=' "$CORE_LOG"; do
	kill -0 "$SHIM_PID" 2>/dev/null || fail "the shim died before forwarding a frame; shim log:
$(cat "$SHIM_LOG")"
	(( $(date +%s) >= deadline )) && fail "no committed frame reached the core; core log:
$(grep -E '^EV (attach|commit|configure)' "$CORE_LOG" | head)
shim log:
$(tail -20 "$SHIM_LOG")"
	sleep 0.2
done
grep -q 'app window mapped' "$SHIM_LOG" || fail "the shim never mapped the app's window"
commits="$(grep -c '^EV commit n=' "$CORE_LOG" || true)"
ok "(b) the spawned app's window mapped and $commits frame(s) forwarded through the shim"

# (c) isolation -- the app holds none of the shim's private descriptors, above
# all not the shim's core socket (fd 3). Compared by inode: descriptor NUMBER 3
# in the app is its own business (its Wayland connection may well land there);
# what must never appear is the INODE of the shim's fd 3.
core_link="$(readlink "/proc/$SHIM_PID/fd/3" 2>/dev/null || true)"
[[ "$core_link" == socket:* ]] || fail "cannot read the shim's core socket (fd 3): got '$core_link'"
for f in "/proc/$APP_PID/fd/"*; do
	[[ -e "$f" ]] || continue
	link="$(readlink "$f" 2>/dev/null || true)"
	[[ "$link" == "$core_link" ]] && fail "the app inherited the shim's core socket $core_link at $(basename "$f")"
done
# And the shim's own Wayland listen socket -- the app must be a CLIENT of it,
# not a co-owner of the listening endpoint. The app connects to the socket path
# ($SOCK), which hands it its own connected endpoint (a distinct inode); it must
# hold no fd to the LISTENING inode itself, nor to the $SOCK.lock guard file.
# The listening inode is the St==01 (LISTEN) row bound to $SOCK in /proc/net/unix.
listen_inode="$(awk -v p="$SOCK" '$6 == "01" && $NF == p {print $7}' /proc/net/unix | head -n1)"
[[ -n "$listen_inode" ]] || fail "cannot find the shim's Wayland listen-socket inode for $SOCK"
lock_path="$SOCK.lock"
for f in "/proc/$APP_PID/fd/"*; do
	[[ -e "$f" ]] || continue
	link="$(readlink "$f" 2>/dev/null || true)"
	[[ "$link" == "socket:[$listen_inode]" ]] && fail "the app inherited the shim's Wayland listen socket (inode $listen_inode) at $(basename "$f")"
	[[ "$link" == "$lock_path" ]] && fail "the app inherited the shim's socket lock $lock_path at $(basename "$f")"
done
ok "(c) $APP_NAME holds none of the shim's private descriptors (core socket $core_link, listen inode $listen_inode, and $lock_path all absent)"

# (d) teardown -- SIGTERM to the shim reaps the app; it is gone, not a zombie.
kill -TERM "$SHIM_PID" 2>/dev/null || fail "could not signal the shim"
deadline=$(( $(date +%s) + 10 ))
until ! kill -0 "$SHIM_PID" 2>/dev/null; do
	(( $(date +%s) >= deadline )) && fail "the shim did not exit on SIGTERM"
	sleep 0.1
done
# The app must be gone entirely -- neither alive nor a zombie. A zombie still
# has a /proc entry with state Z; a reaped process has none.
deadline=$(( $(date +%s) + 10 ))
while [[ -d "/proc/$APP_PID" ]]; do
	state="$(awk '{print $3}' "/proc/$APP_PID/stat" 2>/dev/null || echo '')"
	[[ "$state" == "Z" ]] && fail "$APP_NAME (pid $APP_PID) is a zombie after shim exit (unreaped)"
	(( $(date +%s) >= deadline )) && fail "$APP_NAME (pid $APP_PID) still alive after the shim exited"
	sleep 0.1
done
ok "(d) SIGTERM to the shim reaped $APP_NAME (pid $APP_PID gone, no zombie)"

wait "$CORE_PID" 2>/dev/null || true
grep -q '^FAIL' "$CORE_LOG" && fail "the mock core reported wire violations:
$(grep '^FAIL' "$CORE_LOG")"
CORE_PID=""

echo
echo "PASS: the shim forks the app, forwards its frame, isolates it from the core, and reaps it"
