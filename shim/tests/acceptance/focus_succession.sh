#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Keyboard-focus succession between sibling windows of ONE app in ONE realm
# (issue #268): mapping a second toplevel moves the keyboard to it, unmapping
# that toplevel hands the keyboard BACK to the survivor -- which is the bug,
# found on bare metal with alacritty and nautilus, where the survivor was left
# visible and untypable -- and unmapping the last one still clears focus to
# nobody. The sequence is repeated with an `xdg_popup.grab` open, because a
# menu holds one and wlroots defers every focus change to an active keyboard
# grab. The assertions and the reasoning behind each live in
# tests/focus_succession_client.c; this file only stands a shim up in front of
# it and reports the client's verdict.
#
# LIKE xdg_conformance.sh AND UNLIKE ITS OTHER SIBLINGS HERE, this one is
# wired into `meson test` (see the `test('focus-succession', ...)` call in
# meson.build). It can be: its only dependencies are the shim and a client
# built from this tree, and it is headless and GPU-free. There is no seat
# device and no core in the loop either -- `vitrin_seat_init` runs
# unconditionally at bring-up (src/globals.c), so the virtual keyboard exists
# and the app can observe real wl_keyboard.enter/leave events under
# `--no-upstream`.
#
# Usage: bash shim/tests/acceptance/focus_succession.sh [SHIM_BIN] [CLIENT_BIN]
#        BUILD_DIR=/path/to/build bash shim/tests/acceptance/focus_succession.sh
set -Eeuo pipefail

# Resolved from this script's own location, never from the caller's CWD --
# `meson test` runs with the build directory as its working directory, and a
# stale `./build` elsewhere on the filesystem is exactly how a sibling script
# once ended up testing a shim built two globals ago.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM_DIR="$(cd "$HERE/../.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$SHIM_DIR/build}"
SHIM_BIN="${1:-${SHIM_BIN:-$BUILD_DIR/vitrin-shim}}"
CLIENT_BIN="${2:-${CLIENT_BIN:-$BUILD_DIR/focus-succession-client}}"

[[ -x "$SHIM_BIN" ]] || { echo "FAIL: missing $SHIM_BIN (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
[[ -x "$CLIENT_BIN" ]] || { echo "FAIL: missing $CLIENT_BIN (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
# `meson test` hands these over relative to its working directory; pin them
# now, before anything below can change what "relative" means.
SHIM_BIN="$(cd "$(dirname "$SHIM_BIN")" && pwd)/$(basename "$SHIM_BIN")"
CLIENT_BIN="$(cd "$(dirname "$CLIENT_BIN")" && pwd)/$(basename "$CLIENT_BIN")"

SOCKET_TIMEOUT="${SOCKET_TIMEOUT:-10}"
CLIENT_TIMEOUT="${CLIENT_TIMEOUT:-15}"
SOCKET_NAME="${SOCKET_NAME:-wl-vitrin-focus-$$}"

# Pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-focus.XXXXXX")"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
# The shim must be the SERVER here, so drop any inherited session.
unset WAYLAND_DISPLAY DISPLAY || true
export WAYLAND_DISPLAY="$SOCKET_NAME"
SOCK="$XDG_RUNTIME_DIR/$SOCKET_NAME"

SHIM_PID=""
SHIM_LOG="$RUNTIME_DIR/shim.log"

cleanup() {
	local rc=$?
	if [[ -n "$SHIM_PID" ]] && kill -0 "$SHIM_PID" 2>/dev/null; then
		kill -- "-${SHIM_PID}" 2>/dev/null || kill "$SHIM_PID" 2>/dev/null || true
		wait "$SHIM_PID" 2>/dev/null || true
	fi
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }

# `--no-upstream`: nothing here plays the core, and focus is synthesized
# shim-side -- there is no focus event on the wire for a core to send.
setsid "$SHIM_BIN" --socket "$SOCKET_NAME" --no-upstream >"$SHIM_LOG" 2>&1 &
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

CLIENT_OUT="$RUNTIME_DIR/client.txt"
if ! timeout "$CLIENT_TIMEOUT" "$CLIENT_BIN" >"$CLIENT_OUT" 2>&1; then
	cat "$CLIENT_OUT" >&2
	fail "focus succession client reported failures; shim log:
$(cat "$SHIM_LOG")"
fi
cat "$CLIENT_OUT"

# The client exits non-zero on any failed check, so reaching here is the pass.
# Re-assert the verdict line anyway: a client that dies between its last check
# and its exit would otherwise be indistinguishable from one that passed.
grep -q '^PASS ' "$CLIENT_OUT" || fail "client did not print its PASS verdict"

echo "PASS: the keyboard follows the surviving window and is released with the last one"
