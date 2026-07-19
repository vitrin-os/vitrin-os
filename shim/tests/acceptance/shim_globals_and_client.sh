#!/usr/bin/env bash
# P1.6.1 acceptance proof for the Vitrin OS Wayland shim skeleton:
#
#   (a) `wayland-info` against the shim's socket lists EXACTLY the expected
#       v0 globals (a superset or subset both fail).
#   (b) `weston-terminal` runs against the shim "blind" -- nothing here
#       connects the shim to a core, so the terminal's frames go nowhere, but
#       it must connect, bind globals, and neither crash nor hang the shim.
#
# Both checks run the shim with `--no-upstream`. Since P1.6.2 the shim
# REFUSES to start without the core connection at fd 3 -- holding that
# descriptor is what makes a process a realm's shim -- so standalone
# operation is an explicit opt-in. The upstream link has its own proof in
# upstream_frame_path.sh, which spawns the shim the way the core does.
#
# Requires: vitrin-shim (built), wayland-info (Arch: wayland-utils / Debian:
# wayland-utils), weston-terminal (Arch/Debian: weston). Runs fully headless
# and GPU-free (pixman software renderer), so it is CI-safe.
#
# Usage: SHIM_BIN=./build/vitrin-shim bash shim/tests/acceptance/shim_globals_and_client.sh
set -Eeuo pipefail

SHIM_BIN="${SHIM_BIN:-./build/vitrin-shim}"
SOCKET_TIMEOUT="${SOCKET_TIMEOUT:-10}"
CLIENT_TIMEOUT="${CLIENT_TIMEOUT:-5}"
SOCKET_NAME="${SOCKET_NAME:-wl-vitrin-shim-$$}"
WANT_DMABUF="${WANT_DMABUF:-0}"

# Expected registry, one interface per line, sorted. wl_shm/wl_output are
# created by the shim just like the rest; dmabuf is opt-in.
expected=(wl_compositor wl_shm wl_seat wl_output xdg_wm_base zxdg_decoration_manager_v1)
if [[ "$WANT_DMABUF" == "1" ]]; then
	expected+=(zwp_linux_dmabuf_v1)
fi
EXPECTED="$(printf '%s\n' "${expected[@]}" | sort -u)"

# Force pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

# Private 0700 runtime dir; the shim must be the SERVER, so drop any inherited
# session and point WAYLAND_DISPLAY at the socket we are about to create.
RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-rt.XXXXXX")"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY
export WAYLAND_DISPLAY="$SOCKET_NAME"
SOCK="$XDG_RUNTIME_DIR/$SOCKET_NAME"

SHIM_PID=""
SHIM_LOG="$RUNTIME_DIR/shim.log"

cleanup() {
	local rc=$?
	if [[ -n "$SHIM_PID" ]] && kill -0 "$SHIM_PID" 2>/dev/null; then
		# Kill the whole process group (setsid); fall back to the pid.
		kill -- "-${SHIM_PID}" 2>/dev/null || kill "$SHIM_PID" 2>/dev/null || true
		wait "$SHIM_PID" 2>/dev/null || true
	fi
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }

dmabuf_arg=()
[[ "$WANT_DMABUF" == "1" ]] && dmabuf_arg=(--dmabuf)

# Launch the shim in its own process group so cleanup can reap any children.
setsid "$SHIM_BIN" --socket "$SOCKET_NAME" --no-upstream "${dmabuf_arg[@]}" >"$SHIM_LOG" 2>&1 &
SHIM_PID=$!

# Poll for the socket (never a fixed sleep); bail fast if the shim dies first.
deadline=$(( $(date +%s) + SOCKET_TIMEOUT ))
until [[ -S "$SOCK" ]]; do
	kill -0 "$SHIM_PID" 2>/dev/null || fail "shim exited before binding a socket; log:
$(cat "$SHIM_LOG")"
	(( $(date +%s) >= deadline )) && fail "timeout waiting for $SOCK; log:
$(cat "$SHIM_LOG")"
	sleep 0.1
done
echo "OK: shim up (pid $SHIM_PID), socket $SOCK"

# (a) exact global set.
INFO="$RUNTIME_DIR/wayland-info.txt"
timeout 10 wayland-info >"$INFO" 2>&1 || fail "wayland-info failed:
$(cat "$INFO")"
GOT="$(grep -oE "interface: '[^']+'" "$INFO" | sed -E "s/interface: '(.*)'/\\1/" | sort -u)"

if ! diff <(printf '%s\n' "$EXPECTED") <(printf '%s\n' "$GOT") >"$RUNTIME_DIR/globals.diff"; then
	echo "Globals mismatch (< expected, > actual):" >&2
	cat "$RUNTIME_DIR/globals.diff" >&2
	fail "advertised global set != expected v0 contract"
fi
echo "OK: globals match the v0 contract exactly:"
printf '  %s\n' $GOT

# (b) blind weston-terminal: success == still running when the timeout kills it
# (GNU coreutils `timeout` exit 124), with the shim still alive afterwards.
set +e
timeout "$CLIENT_TIMEOUT" weston-terminal >"$RUNTIME_DIR/wt.log" 2>&1
WT_RC=$?
set -e
[[ "$WT_RC" -eq 124 ]] || fail "weston-terminal exited $WT_RC (crash or connect failure), expected 124 (timed out while running); log:
$(cat "$RUNTIME_DIR/wt.log")"
kill -0 "$SHIM_PID" 2>/dev/null || fail "shim died while weston-terminal was connected; log:
$(cat "$SHIM_LOG")"
echo "OK: weston-terminal ran blind, killed by timeout (rc=124), shim still alive"

echo "PASS: both P1.6.1 acceptance checks green"
