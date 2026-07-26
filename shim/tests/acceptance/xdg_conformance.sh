#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# xdg-shell conformance checks for src/xdg.c that only the app's side of the
# socket can make: the initial configure is sent on the client's initial
# commit and NOT before it, `xdg_toplevel.wm_capabilities` arrives once,
# before that configure, carrying exactly the capabilities the shim
# implements, and a popup is configured on its own initial commit so that
# opening a menu does not get the app disconnected. The assertions and the
# reasoning behind each live in tests/xdg_conformance_client.c; this file only
# stands a shim up in front of it and reports the client's verdict.
#
# UNLIKE ITS SIBLINGS IN THIS DIRECTORY, this one is wired into `meson test`
# (see the `test('xdg-conformance', ...)` call in meson.build). It can be:
# its only dependencies are the shim and a client built from this tree, with
# no wayland-info, no weston, no GTK and no Firefox, and it is headless and
# GPU-free. The other acceptance scripts need one of those and so stay
# separate CI steps.
#
# Usage: bash shim/tests/acceptance/xdg_conformance.sh [SHIM_BIN] [CLIENT_BIN]
#        BUILD_DIR=/path/to/build bash shim/tests/acceptance/xdg_conformance.sh
set -Eeuo pipefail

# Resolved from this script's own location, never from the caller's CWD --
# `meson test` runs with the build directory as its working directory, and a
# stale `./build` elsewhere on the filesystem is exactly how a sibling script
# once ended up testing a shim built two globals ago.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM_DIR="$(cd "$HERE/../.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$SHIM_DIR/build}"
SHIM_BIN="${1:-${SHIM_BIN:-$BUILD_DIR/vitrin-shim}}"
CLIENT_BIN="${2:-${CLIENT_BIN:-$BUILD_DIR/xdg-conformance-client}}"

[[ -x "$SHIM_BIN" ]] || { echo "FAIL: missing $SHIM_BIN (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
[[ -x "$CLIENT_BIN" ]] || { echo "FAIL: missing $CLIENT_BIN (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
# `meson test` hands these over relative to its working directory; pin them
# now, before anything below can change what "relative" means.
SHIM_BIN="$(cd "$(dirname "$SHIM_BIN")" && pwd)/$(basename "$SHIM_BIN")"
CLIENT_BIN="$(cd "$(dirname "$CLIENT_BIN")" && pwd)/$(basename "$CLIENT_BIN")"

SOCKET_TIMEOUT="${SOCKET_TIMEOUT:-10}"
CLIENT_TIMEOUT="${CLIENT_TIMEOUT:-15}"
SOCKET_NAME="${SOCKET_NAME:-wl-vitrin-xdgconf-$$}"

# Pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-xdgconf.XXXXXX")"
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

# `--no-upstream`: nothing here plays the core, and this test is about what the
# shim says to its app, not about what it forwards.
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
	fail "xdg conformance client reported failures; shim log:
$(cat "$SHIM_LOG")"
fi
cat "$CLIENT_OUT"

# The client exits non-zero on any failed check, so reaching here is the pass.
# Re-assert the verdict line anyway: a client that dies between its last check
# and its exit would otherwise be indistinguishable from one that passed.
grep -q '^PASS ' "$CLIENT_OUT" || fail "client did not print its PASS verdict"

echo "PASS: xdg-shell configure ordering and wm_capabilities are as specified"
