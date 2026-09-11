#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# P2.6.5 (issue #189) COMPONENT proof: the shipped shim receives a designated
# descriptor instead of dying on it.
#
# THIS IS A COMPONENT TEST, NOT MILESTONE ACCEPTANCE. It drives the real shim
# against `tests/mock_core.c`, a hand-written stand-in for the trusted core (see
# that file's own header, and CLAUDE.md's definition-of-done rule). It proves
# what the SHIM does with `vitrin_shim_session.designation`; it proves nothing
# about `vitrind`'s picker, its resolver, or that any human ever chose anything.
#
# WHAT ONLY THIS CAN SETTLE. `test_wire_designation.c` calls the transport
# directly and covers every violation path; it cannot observe the failure this
# stream exists to remove, because that failure is a DEAD REALM. Before the
# receive side landed, `shim/src/wire.c` treated any arriving descriptor as a
# violation, closed it and killed the core connection -- so the first successful
# designation would have taken the app down with it. The three facts below are
# about the binary, spawned under the real fd-3 contract, running its real event
# loop:
#
#   (A) IT SURVIVES. The shim is still running after the designation, and the
#       mock core never saw its end of the socketpair close. `EV shim_eof` in
#       the core's log is exactly the old behaviour, so its absence is a real
#       assertion rather than a tautology.
#
#   (B) IT SAYS SO. The shim logs the arrival, naming the designation id the
#       mock core sent, and says plainly that the descriptor was closed rather
#       than relayed -- because no designation socket exists yet (P2.6.7, issue
#       #191). A silent success and a silent drop look identical in a log.
#
#   (D) IT STILL REFUSES A DESCRIPTOR ON A FRAME WHOSE SIGNATURE HAS NONE.
#       Receiving `designation` narrowed which frames may carry an fd; it must
#       not have relaxed what happens to the ones that may not. A second run
#       (`--smuggle-fd`) sends a real `frame_done` with the header's own
#       `fd_count` set to 1 and a descriptor attached -- `fd_violation`'s first
#       disjunct, which the transport cannot see because positional matching is
#       satisfied. Conventions 2.4 makes it fatal and 5.2 delivers a shim
#       fatal as log-and-close, so the assertion is that the shim logs it AND
#       the connection dies.
#
#   (C) IT HOLDS NOTHING. Counted through /proc/PID/fd WHILE THE SHIM IS STILL
#       RUNNING, which is the only moment a leak is visible: a descriptor the
#       shim never closes is released when it exits, so a census taken after
#       teardown passes on a leaking shim. The designated file is a real path
#       (not unlinked) precisely so `readlink` can name it.
#
# Requires: vitrin-shim, mock-core (both built by `meson compile -C build`).
# Runs fully headless and GPU-free, with no Rust toolchain anywhere -- the shim
# CI job's standing invariant.
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/designation_receive.sh
set -Eeuo pipefail

BUILD_DIR="${BUILD_DIR:-./build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"

VIEW_W="${VIEW_W:-400}"
VIEW_H="${VIEW_H:-300}"
RUN_MS="${RUN_MS:-4000}"
DESIGNATE_AFTER_MS="${DESIGNATE_AFTER_MS:-500}"

# Kept in step with mock_core.c's MOCK_DESIGNATION_ID by this assertion failing
# loudly if it drifts: the shim's log line is the only place the two meet.
DESIGNATION_ID=4242

for bin in "$SHIM_BIN" "$MOCK_CORE"; do
	[[ -x "$bin" ]] || { echo "FAIL: missing $bin (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
done

# Force pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1
# The shim's log line is at INFO; the default filters it out.
export WLR_DEBUG=0

RUNTIME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-designation.XXXXXX")"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

CORE_PID=""
# BY PID, NEVER BY NAME. `meson test` runs the acceptance scripts in parallel,
# so a `pkill -x mock-core` here reaches into whatever OTHER test is running --
# and into a developer's unrelated shim on their own desktop. Killing the mock
# core is enough on its own: it installs a SIGTERM handler and takes its shim
# down with it (mock_core.c's shutdown ladder), which is the same teardown the
# real core performs.
cleanup() {
	local rc=$?
	[[ -n "$CORE_PID" ]] && kill "$CORE_PID" 2>/dev/null || true
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# The file the mock core designates. A real, named, non-empty file so that (C)
# can identify it by the path /proc/PID/fd/N resolves to.
SUBJECT="$RUNTIME_DIR/designated.txt"
printf 'vitrin designation subject\n' >"$SUBJECT"

CORE_LOG="$RUNTIME_DIR/core.log"
SHIM_LOG="$RUNTIME_DIR/shim.log"
SOCKET="vitrin-designation-$$"

"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms "$RUN_MS" \
	--designate "$SUBJECT" --designate-after-ms "$DESIGNATE_AFTER_MS" \
	-- "$SHIM_BIN" --socket "$SOCKET" >"$CORE_LOG" 2>"$SHIM_LOG" &
CORE_PID=$!

# The shim binding its socket is the bring-up the timing budget is really for.
SOCK="$XDG_RUNTIME_DIR/$SOCKET"
DEADLINE=$(( $(date +%s) + 15 ))
until [[ -S "$SOCK" ]]; do
	kill -0 "$CORE_PID" 2>/dev/null || fail "the shim never bound a socket; core log:
$(cat "$CORE_LOG")
shim log:
$(cat "$SHIM_LOG")"
	(( $(date +%s) >= DEADLINE )) && fail "timeout waiting for $SOCK"
	sleep 0.1
done

# The pid the mock core forked. Everything in (C) is measured against it, so a
# missing pid must stop the run rather than let the census inspect nothing.
SHIM_PID="$(sed -n 's/^EV spawned shim_pid=\([0-9]\{1,\}\).*/\1/p' "$CORE_LOG" | head -n1)"
[[ -n "$SHIM_PID" ]] || fail "the mock core did not report the shim's pid; core log:
$(cat "$CORE_LOG")"
[[ -d "/proc/$SHIM_PID/fd" ]] || fail "no /proc/$SHIM_PID/fd, so assertion (C) would \
inspect nothing. This test needs a readable /proc for the process it spawned."

# Wait for the shim's own account of the designation, which is also what marks
# the moment (C) has to measure at: after the event, before teardown.
DEADLINE=$(( $(date +%s) + 15 ))
until grep -q "designation $DESIGNATION_ID arrived" "$SHIM_LOG"; do
	kill -0 "$CORE_PID" 2>/dev/null || fail "the run ended before the shim logged a \
designation. This is the pre-#189 failure exactly: the shim treated the \
descriptor as a violation and died. core log:
$(cat "$CORE_LOG")
shim log:
$(cat "$SHIM_LOG")"
	(( $(date +%s) >= DEADLINE )) && fail "timeout waiting for the shim to log the \
designation; shim log:
$(cat "$SHIM_LOG")"
	sleep 0.1
done

# --- (A) the shim is still running ---------------------------------------
kill -0 "$SHIM_PID" 2>/dev/null \
	|| fail "the shim exited after receiving the designation; shim log:
$(cat "$SHIM_LOG")"
ok "the shim survived a designation (pid $SHIM_PID still running)"

# --- (C) it is holding no descriptor on the designated file --------------
# Measured now, with the shim alive. Its own count of descriptors naming
# $SUBJECT must be zero: the shim closes what it cannot relay.
HELD=0
SCANNED=0
for link in "/proc/$SHIM_PID/fd/"*; do
	[[ -e "$link" ]] || continue
	SCANNED=$(( SCANNED + 1 ))
	target="$(readlink "$link" 2>/dev/null || true)"
	[[ "$target" == "$SUBJECT" ]] && HELD=$(( HELD + 1 ))
done
# Vacuity guard: a scan that saw no descriptors at all found zero matches for
# the same reason a leaking shim would, and every process has stdin.
(( SCANNED > 0 )) || fail "the descriptor census read no entries from \
/proc/$SHIM_PID/fd, so it proves nothing about a leak"
(( HELD == 0 )) || fail "the shim is holding $HELD descriptor(s) on $SUBJECT after \
the designation; a descriptor it cannot relay must be closed, and one left open \
pins the file for the life of the realm"
ok "the shim holds no descriptor on the designated file ($SCANNED fds scanned)"

# --- (B) it said what it did with the descriptor -------------------------
grep -q "designation $DESIGNATION_ID arrived" "$SHIM_LOG" \
	|| fail "the shim did not log designation $DESIGNATION_ID"
grep -q "CLOSED, not relayed" "$SHIM_LOG" \
	|| fail "the shim logged a designation without saying what became of the \
descriptor. Until P2.6.7 (#191) serves the realm's designation socket the app \
never receives it, and a log that does not say so reads as a delivery; shim log:
$(grep -i designation "$SHIM_LOG" || echo '(no designation lines)')"
ok "the shim logged the arrival and named the disposition"

wait "$CORE_PID" || true
CORE_PID=""

if grep -q '^FAIL' "$CORE_LOG"; then
	fail "the mock core reported wire violations:
$(grep '^FAIL' "$CORE_LOG")"
fi
grep -q '^EV designation ' "$CORE_LOG" \
	|| fail "the mock core never sent a designation, so nothing above was tested:
$(cat "$CORE_LOG")"
grep -q 'designated=1' "$CORE_LOG" \
	|| fail "the mock core's summary does not report a designation:
$(grep '^SUMMARY' "$CORE_LOG" || echo '(no summary)')"
# (A), the other half: the core never saw the shim's end close. That is the
# pre-#189 behaviour, so its absence is the assertion.
grep -q '^EV shim_eof' "$CORE_LOG" \
	&& fail "the core saw the shim's connection close during the run:
$(cat "$CORE_LOG")"
ok "the core link was never torn down by the designation"

# --- (D) a descriptor on a frame whose signature declares none ------------
# A SECOND run, hostile core: the same descriptor, on `frame_done`, with
# `fd_count` lied to 1. Separate because it ends with a dead connection, which
# is the opposite of what (A) asserts.
D_RUNTIME="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-fdviolation.XXXXXX")"
chmod 700 "$D_RUNTIME"
D_SUBJECT="$D_RUNTIME/smuggled.txt"
printf 'vitrin smuggled subject\n' >"$D_SUBJECT"
D_CORE_LOG="$D_RUNTIME/core.log"
D_SHIM_LOG="$D_RUNTIME/shim.log"

XDG_RUNTIME_DIR="$D_RUNTIME" "$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms "$RUN_MS" \
	--designate "$D_SUBJECT" --smuggle-fd --designate-after-ms "$DESIGNATE_AFTER_MS" \
	-- "$SHIM_BIN" --socket "vitrin-fdviolation-$$" >"$D_CORE_LOG" 2>"$D_SHIM_LOG" &
CORE_PID=$!
wait "$CORE_PID" || true
CORE_PID=""

grep -q '^EV smuggled_fd ' "$D_CORE_LOG" \
	|| fail "the mock core never smuggled a descriptor, so (D) tested nothing:
$(cat "$D_CORE_LOG")"
grep -q 'fd_violation' "$D_SHIM_LOG" \
	|| fail "the shim did not name fd_violation on a descriptor attached to a \
frame whose signature declares none. conventions 2.4 makes that fatal; shim log:
$(cat "$D_SHIM_LOG")"
# The connection dying is the assertion, not the log line: 5.2 says a shim
# fatal is log-and-CLOSE, and a shim that logged and carried on would have left
# positional fd matching desynchronized for every later frame.
grep -q '^EV shim_eof' "$D_CORE_LOG" \
	|| fail "the shim logged fd_violation and kept the core connection open. \
conventions 5.2: a shim protocol violation is log-and-CLOSE; core log:
$(cat "$D_CORE_LOG")
shim log:
$(cat "$D_SHIM_LOG")"
rm -rf "$D_RUNTIME"
ok "the shim refused a descriptor on a frame whose signature declares none, and closed"

echo "PASS: shim/tests/acceptance/designation_receive.sh (COMPONENT test against mock_core.c)"
