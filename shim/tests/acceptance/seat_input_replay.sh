#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# P1.6.3 acceptance proof: virtual-seat input replay.
#
# Three independent witnesses, deliberately, because no two of them share a
# code path and none of them is the shim asking itself whether it worked:
#
#   the CORE    tests/mock_core.c, which spawns the shim exactly as the real
#               core does and plays a scripted run of `vitrin_shim_seat`
#               events at it, tracing each one as `EV seat_send ... origin=`
#   the SHIM    its own structured delivery trace, `seat-replay: ... origin=`,
#               one line per event, carrying the tag it was given
#   the APP     tests/input_echo_client.c, a real Wayland client that
#               resolves every key through the keymap it was sent, with
#               xkbcommon, exactly as a toolkit does
#
# The four acceptance criteria of issue #35 and where each is proven:
#
#   1. "input-echo test client receives exactly the scripted synthetic
#      events (order, coordinates, no duplicates)"
#         (A) a scripted run whose every event is asserted at the app, in
#             order, with exact values and exact counts.
#   2. "héllo→世界 arrives intact in a GTK text field (R5 gate)"
#         (C) a real GtkEntry, read back as bytes. (B) proves the same
#             string at the keysym level first, so a failure separates
#             "the keymap is wrong" from "GTK's input method rejected it".
#   3. "view → surface-local coordinate mapping verified (D10)"
#         (A) with the client declaring a CSD window-geometry inset, which
#             is what makes the mapping a translation rather than a copy.
#   4. "origin tag (B2) preserved through replay"
#         (D) core-sent origins correlated one-to-one, in order, against
#             shim-delivered origins. See the README and seat.h for what
#             this can and cannot establish: the flight recorder is
#             core-side and there is no shim -> core input acknowledgement,
#             so "observable in the flight-recorder trace of the delivery"
#             is not literally satisfiable without a protocol change.
#
# Plus the structural checks the criteria depend on but do not name:
#         (E) the modifier-suppression rule, against BOTH mechanisms by
#             which a modifier can re-resolve an already-resolved keysym:
#             Shift (level selection) and Caps Lock (libxkbcommon's separate
#             keysym capitalization, which a one-level key type does not
#             suppress on its own).
#         (F) keymap regeneration is bounded and additive -- ASCII costs
#             zero regenerations, and a keycode already bound never changes
#             meaning.
#         (G) a string past the dynamic region chunks without corrupting.
#         (H) a keymap regeneration does not silently reset the app's
#             modifier state, so a chord held across one still works.
#         (I) `text` delivers a Unicode string, not keystrokes: control
#             characters other than the two the IDL names are dropped rather
#             than pressed.
#         (J) a key the human is physically holding and the same character
#             in an agent payload do not collide on one keycode.
#         (K) pointer events that arrive in one wire batch share one
#             `wl_pointer.frame`; ones that do not, do not.
#
# Requires: vitrin-shim, mock-core, input-echo-client (all built by
# `meson compile -C build`); gtk-entry-probe for (C), which is built only
# where GTK is installed and reported as SKIP where it is not. Runs fully
# headless and GPU-free, with no Rust toolchain anywhere.
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/seat_input_replay.sh
set -Eeuo pipefail

# Every number in this script is compared as text, so the decimal separator
# must be the one the C programs print, not the one the operator's locale
# prefers. A comma here would fail every coordinate assertion on a machine
# set to a European locale -- and would do it by printing two strings that
# look identical at a glance.
export LC_ALL=C

BUILD_DIR="${BUILD_DIR:-./build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"
ECHO_CLIENT="${ECHO_CLIENT:-$BUILD_DIR/input-echo-client}"
GTK_PROBE="${GTK_PROBE:-$BUILD_DIR/gtk-entry-probe}"

VIEW_W="${VIEW_W:-400}"
VIEW_H="${VIEW_H:-300}"
# The client's CSD shadow border. Any nonzero value works; what matters is
# that view -> surface-local becomes a translation by exactly this much, so
# a shim that forwarded coordinates unchanged fails visibly.
INSET="${INSET:-20}"

# The R5 gate string. Non-ASCII (é, Latin-1 keysym), an arrow (U+2192, a
# legacy "technical" keysym, NOT the Unicode-offset form), and two CJK
# ideographs (U+4E16, U+754C, which have no legacy keysym at all) -- three
# different corners of xkbcommon's keysym space in eight characters.
GATE_TEXT='héllo→世界'

for bin in "$SHIM_BIN" "$MOCK_CORE" "$ECHO_CLIENT"; do
	[[ -x "$bin" ]] || { echo "FAIL: missing $bin (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
done

export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1
# GTK must not go looking for an accessibility bus or an IBus daemon: neither
# exists here, and waiting for them turns a 4-second probe into a 30-second
# one. `gtk-im-context-simple` is also the input method the R5 gate is about
# -- the one a legacy app gets with no IME running.
export GTK_A11Y=none
export NO_AT_BRIDGE=1
export GTK_IM_MODULE=gtk-im-context-simple

# Short runtime dir: a Unix socket path is capped at 108 bytes and the
# default TMPDIR under some harnesses is long enough to blow it.
RUNTIME_DIR="$(mktemp -d /tmp/vitrin-seat.XXXXXX)"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

cleanup() {
	local rc=$?
	pkill -x mock-core 2>/dev/null || true
	pkill -x vitrin-shim 2>/dev/null || true
	pkill -x input-echo-client 2>/dev/null || true
	pkill -x gtk-entry-probe 2>/dev/null || true
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# Run one scenario: mock core (which spawns the shim) + an input script + an
# app. $1 = tag, $2 = input script path, $3.. = app argv.
run_scenario() {
	local tag="$1"; shift
	local script="$1"; shift
	local socket="vs-$tag-$$"
	CORE_LOG="$RUNTIME_DIR/$tag.core.log"
	SHIM_LOG="$RUNTIME_DIR/$tag.shim.log"
	APP_LOG="$RUNTIME_DIR/$tag.app.log"

	"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms 12000 \
		--input "$script" --input-after-commits 2 \
		-- "$SHIM_BIN" --socket "$socket" >"$CORE_LOG" 2>"$SHIM_LOG" &
	CORE_PID=$!

	local sock="$XDG_RUNTIME_DIR/$socket"
	local deadline=$(( $(date +%s) + 15 ))
	until [[ -S "$sock" ]]; do
		kill -0 "$CORE_PID" 2>/dev/null || fail "[$tag] the shim never bound a socket:
$(cat "$SHIM_LOG")"
		(( $(date +%s) >= deadline )) && fail "[$tag] timeout waiting for $sock"
		sleep 0.1
	done

	WAYLAND_DISPLAY="$socket" "$@" >"$APP_LOG" 2>&1 || true
	wait "$CORE_PID" || fail "[$tag] the mock core reported violations:
$(grep '^FAIL' "$CORE_LOG" || true)"
	if grep -q '^FAIL' "$CORE_LOG"; then
		fail "[$tag] wire violations:
$(grep '^FAIL' "$CORE_LOG")"
	fi
}

# Exactly-once assertion on the app's event log, with the count stated so a
# duplicate is as loud as a missing event. $1 = tag, $2 = expected count,
# $3 = fixed string.
expect_app() {
	local n
	n="$(grep -c -F -x -- "$3" "$APP_LOG" || true)"
	(( n == $2 )) || fail "[$1] expected $2 x '$3' in the app's log, saw $n:
$(grep '^IN ' "$APP_LOG")"
}

# The same, by regex, for lines carrying a value that is an implementation
# detail rather than part of the claim. Keycodes are the case: which slot of
# the dynamic keymap a keysym lands in is the shim's business and is allowed
# to move when the warm set grows, whereas the KEYSYM the app resolves is the
# contract. Pinning the keycode here would make an unrelated addition to the
# keymap fail this test for no reason.
expect_app_re() {
	local n
	n="$(grep -c -E -- "$3" "$APP_LOG" || true)"
	(( n == $2 )) || fail "[$1] expected $2 line(s) matching /$3/ in the app's log, saw $n:
$(grep '^IN ' "$APP_LOG")"
}

# The shim's delivery trace, one line per event, stripped of wlroots' log
# prefix so it can be diffed against the core's record.
shim_trace() { sed -n 's/.*\(seat-replay: .*\)/\1/p' "$SHIM_LOG"; }

# Every keycode the app saw must alternate PRESSED, RELEASED, PRESSED...
#
# This is the "no duplicates" criterion stated as an invariant rather than as
# a count, and it catches what counting cannot: a second PRESSED for a
# keycode already down, or a RELEASED for one that never went down. Neither
# is a stream a real compositor produces, and a client tracking key state --
# or an Xwayland bridge in Phase 3 -- would end up disagreeing with the shim
# about what is held. Applied to every scenario that sends keys, because the
# interleavings that break it are not the ones a test thinks to enumerate.
assert_key_pairing() {
	local bad
	bad="$(awk '
		/^IN key /{
			k = ""; s = ""
			for (i = 1; i <= NF; i++) {
				if ($i ~ /^keycode=/) { k = substr($i, 9) }
				if ($i ~ /^state=/)   { s = substr($i, 7) }
			}
			if (s == "1") {
				if (held[k]) { print "keycode " k " pressed twice with no release" }
				held[k] = 1
			} else {
				if (!held[k]) { print "keycode " k " released while not held" }
				held[k] = 0
			}
		}' "$APP_LOG")"
	[[ -z "$bad" ]] || fail "[$1] the key stream is not press/release paired:
$bad
$(grep '^IN key ' "$APP_LOG")"
}

echo "== (A) the app receives exactly the scripted events =="
# Coordinates are chosen so every claim is separable:
#   (140, 90)  -> surface-local (160, 110): a translation by the inset
#   (200, 150) -> surface-local (220, 170): a SECOND, distinct position, so a
#                 real `wl_pointer.motion` is observable (wlroots collapses a
#                 motion to the position an `enter` already established --
#                 correctly: an identical position is not a motion)
#   (2000, 90) -> off the surface entirely while a button is held: the
#                 implicit grab must keep it flowing, at surface-local
#                 (2020, 110), rather than tearing focus away mid-drag
cat >"$RUNTIME_DIR/a.script" <<EOF
delay 400
motion 140 90 emulated
delay 80
motion 200 150 emulated
delay 80
button 272 pressed emulated
delay 80
motion 2000 90 emulated
delay 80
button 272 released emulated
delay 80
scroll vertical -120 physical
delay 80
scroll horizontal 240 emulated
delay 80
key 0xff51 pressed physical
key 0xff51 released physical
delay 400
EOF
run_scenario a "$RUNTIME_DIR/a.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"

grep -q '^IN mapped ' "$APP_LOG" || fail "[a] the echo client never mapped a window:
$(cat "$APP_LOG")"
sends="$(sed -n 's/.*SUMMARY .*seat_sends=\([0-9]*\).*/\1/p' "$CORE_LOG")"
(( sends == 9 )) || fail "[a] the core sent $sends seat events, expected 9"

# --- coordinates, criterion 3 (D10) ---
# The inset is what makes this an assertion about MAPPING. 140 + 20 = 160.
expect_app a 1 "IN pointer_enter sx=$(printf '%.3f' $((140 + INSET))) sy=$(printf '%.3f' $((90 + INSET)))"
expect_app a 1 "IN pointer_motion sx=$(printf '%.3f' $((200 + INSET))) sy=$(printf '%.3f' $((150 + INSET)))"
ok "view -> surface-local is a translation by the client's window geometry, not a copy"

# --- the implicit grab ---
# 2000 is far outside a 400-wide view; without a grab the hit test finds
# nothing and the app would get a leave and a stuck button.
expect_app a 1 "IN pointer_motion sx=$(printf '%.3f' $((2000 + INSET))) sy=$(printf '%.3f' $((90 + INSET)))"
expect_app a 0 "IN pointer_leave"
ok "a drag off the surface keeps flowing (implicit grab) and never leaves the app"

# --- buttons, scroll, keys: exact values, exactly once each ---
expect_app a 1 "IN pointer_button button=272 state=1"
expect_app a 1 "IN pointer_button button=272 state=0"
expect_app a 1 "IN pointer_axis_value120 axis=0 value120=-120"
expect_app a 1 "IN pointer_axis_value120 axis=1 value120=240"
# 15 px per notch, the same rate the core converts pixels to v120 with.
expect_app a 1 "IN pointer_axis axis=0 value=-15.000"
expect_app a 1 "IN pointer_axis axis=1 value=30.000"
expect_app_re a 1 '^IN key keycode=[0-9]+ state=1 keysym=0x0000ff51 name=Left '
expect_app_re a 1 '^IN key keycode=[0-9]+ state=0 keysym=0x0000ff51 name=Left '

# --- no duplicates, stated as totals ---
summary="$(grep '^SUMMARY ' "$APP_LOG")"
field() { sed -n "s/.*[[:space:]]$1=\([0-9]*\).*/\1/p" <<<"$summary"; }
[[ "$(field enter)" == 1 ]] || fail "[a] enter count $(field enter), expected 1: $summary"
[[ "$(field motion)" == 2 ]] || fail "[a] motion count $(field motion), expected 2: $summary"
[[ "$(field button)" == 2 ]] || fail "[a] button count $(field button), expected 2: $summary"
[[ "$(field v120)" == 2 ]] || fail "[a] value120 count $(field v120), expected 2: $summary"
[[ "$(field press)" == 1 ]] || fail "[a] key press count $(field press), expected 1: $summary"
[[ "$(field release)" == 1 ]] || fail "[a] key release count $(field release), expected 1: $summary"
[[ "$(field leave)" == 0 ]] || fail "[a] the app was left focus: $summary"
# One `wl_pointer.frame` per logical group and not one more: enter, motion,
# press, drag-motion, release, scroll, scroll = 7.
[[ "$(field frame)" == 7 ]] || fail "[a] frame count $(field frame), expected 7 (one per group): $summary"
# The whole run must cost ZERO keymap regenerations: Left is layout-invariant
# and pre-bound in the warm region, so generation 1 (uploaded at startup,
# before the app connected) is the only one the app ever sees. This is the
# caching decision, asserted.
[[ "$(field keymaps)" == 1 ]] || fail "[a] the app saw $(field keymaps) keymaps; an all-warm run must cost exactly 1"
assert_key_pairing a
ok "exactly 9 scripted events -> exactly the expected app events, no duplicates, 1 keymap"

# --- order ---
# The app's event order must be the script's order. Compare the projected
# sequence rather than eyeballing it.
# The trailing space in the alternation is load-bearing: without it `key`
# also matches the `IN keymap ...` lines, and the projected order silently
# gains two entries that are not input events at all.
got_order="$(grep -oE '^IN (pointer_enter|pointer_motion|pointer_button|pointer_axis_value120|key) ' "$APP_LOG" | awk '{print $2}' | tr '\n' ' ')"
want_order="pointer_enter pointer_motion pointer_button pointer_motion pointer_button pointer_axis_value120 pointer_axis_value120 key key "
[[ "$got_order" == "$want_order" ]] || fail "[a] event order
  got:  $got_order
  want: $want_order"
ok "event order matches the script exactly"

echo "== (B) the R5 gate string survives at the keysym level =="
cat >"$RUNTIME_DIR/b.script" <<EOF
delay 400
motion 100 100 emulated
delay 80
text emulated ${GATE_TEXT}
delay 80
text emulated ${GATE_TEXT}
delay 400
EOF
run_scenario b "$RUNTIME_DIR/b.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
got_text="$(sed -n 's/^TEXT //p' "$APP_LOG")"
[[ "$got_text" == "${GATE_TEXT}${GATE_TEXT}" ]] \
	|| fail "[b] the app resolved '$got_text', expected '${GATE_TEXT}${GATE_TEXT}'
$(grep '^IN key' "$APP_LOG")"
# Every codepoint one press and one release, nothing repeated: 8 characters
# twice. Client-side key repeat is disabled precisely so this holds.
summary="$(grep '^SUMMARY ' "$APP_LOG")"
[[ "$(field press)" == 16 && "$(field release)" == 16 ]] \
	|| fail "[b] expected 16 presses and 16 releases, got: $summary"
# --- criterion (F): regeneration is bounded and, on a repeat, free ---
# Four non-ASCII codepoints cost ONE regeneration on their first appearance
# and zero on the second, because a binding once made is never unmade. So the
# app sees exactly 2 keymaps: the warm one, plus one.
[[ "$(field keymaps)" == 2 ]] \
	|| fail "[b] the app saw $(field keymaps) keymaps; 4 new codepoints typed twice must cost exactly 2"
assert_key_pairing b
ok "'${GATE_TEXT}' resolves character-for-character, twice, on 1 keymap regeneration"

echo "== (C) the R5 gate: a real GTK text field =="
if [[ -x "$GTK_PROBE" ]]; then
	cat >"$RUNTIME_DIR/c.script" <<EOF
delay 1500
text emulated ${GATE_TEXT}
delay 1200
EOF
	run_scenario c "$RUNTIME_DIR/c.script" timeout 15 "$GTK_PROBE" --run-ms 4000
	entry="$(sed -n 's/^ENTRY //p' "$APP_LOG" | head -1)"
	entry_hex="$(sed -n 's/^ENTRY_HEX //p' "$APP_LOG" | head -1)"
	want_hex="$(printf '%s' "$GATE_TEXT" | od -An -tx1 | tr -d ' \n')"
	[[ -n "$entry_hex" ]] || fail "[c] the GTK probe printed no entry contents:
$(cat "$APP_LOG")"
	[[ "$entry_hex" == "$want_hex" ]] || fail "[c] the GTK entry contains
  got:  '$entry' ($entry_hex)
  want: '$GATE_TEXT' ($want_hex)"
	ok "'$GATE_TEXT' arrives byte-for-byte in a GtkEntry ($entry_hex)"

	# The same criterion with Caps Lock latched by a human first. GTK
	# resolves keys through the keymap with xkbcommon, so it inherits
	# xkbcommon's Caps capitalization: without a key type that consumes Lock
	# the entry receives 'HÉLLO→世界'. The keysym-level case in (E) proves the
	# mechanism; this proves the named criterion survives it.
	cat >"$RUNTIME_DIR/c2.script" <<EOF
delay 1500
key 0xffe5 pressed physical
key 0xffe5 released physical
delay 100
text emulated ${GATE_TEXT}
delay 1200
EOF
	run_scenario c2 "$RUNTIME_DIR/c2.script" timeout 15 "$GTK_PROBE" --run-ms 4000
	entry="$(sed -n 's/^ENTRY //p' "$APP_LOG" | head -1)"
	entry_hex="$(sed -n 's/^ENTRY_HEX //p' "$APP_LOG" | head -1)"
	[[ -n "$entry_hex" ]] || fail "[c2] the GTK probe printed no entry contents:
$(cat "$APP_LOG")"
	[[ "$entry_hex" == "$want_hex" ]] || fail "[c2] with Caps Lock latched, the GTK entry contains
  got:  '$entry' ($entry_hex)
  want: '$GATE_TEXT' ($want_hex)"
	ok "'$GATE_TEXT' survives a latched Caps Lock in a GtkEntry ($entry_hex)"
elif [[ -n "${CI:-}" && -z "${VITRIN_SKIP_GTK_GATE:-}" ]]; then
	# Opt-in locally, but NOT silently so under CI -- the same rule the
	# cross-track conformance test applies to itself. A named acceptance
	# criterion that only ever reports SKIP on the machine that gates merges is
	# a criterion nobody is holding, and this one in particular cannot be
	# inferred from (B): (B) proves the keysyms, the toolkit's input method is
	# what turns keysyms into a text buffer, and only a real widget exercises
	# that. shim/ci/install-deps.sh installs libgtk-3-dev for exactly this.
	fail "[c] gtk-entry-probe was not built in CI, so the R5 text-field gate proved nothing.
Install GTK (shim/ci/install-deps.sh) or declare the gap with VITRIN_SKIP_GTK_GATE=1."
else
	echo "SKIP: $GTK_PROBE was not built (no GTK on this machine); the R5 text-field gate is NOT proven here"
fi

echo "== (D) B2: the origin tag survives core -> shim -> delivery =="
# The core's record of what it sent, and the shim's record of what it
# delivered, projected to (event, origin) and compared in order. They are
# produced by two different programs from two different data structures, so
# agreement is not tautological.
cat >"$RUNTIME_DIR/d.script" <<EOF
delay 400
motion 100 100 physical
delay 60
motion 120 120 emulated
delay 60
button 272 pressed physical
delay 60
button 272 released physical
delay 60
scroll vertical -120 emulated
delay 60
key 0xff1b pressed physical
delay 60
key 0xff1b released physical
delay 60
text emulated abc
delay 400
EOF
run_scenario d "$RUNTIME_DIR/d.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"

core_tags="$(sed -n 's/^EV seat_send .*event=\([a-z]*\) origin=\([a-z]*\).*/\1:\2/p' "$CORE_LOG")"
shim_tags="$(shim_trace | sed -n 's/.*event=\([a-z]*\) origin=\([a-z]*\).*/\1:\2/p')"
[[ -n "$core_tags" ]] || fail "[d] the core traced no seat sends"
if [[ "$core_tags" != "$shim_tags" ]]; then
	fail "[d] the origin tag did not survive delivery
  core sent:      $(tr '\n' ' ' <<<"$core_tags")
  shim delivered: $(tr '\n' ' ' <<<"$shim_tags")"
fi
n_phys="$(grep -c ':physical' <<<"$shim_tags" || true)"
n_emul="$(grep -c ':emulated' <<<"$shim_tags" || true)"
(( n_phys == 5 && n_emul == 3 )) \
	|| fail "[d] expected 5 physical and 3 emulated deliveries, got $n_phys/$n_emul"
# And nothing untagged ever reached the trace: `unset` is what the biased
# encoding prints when a tag was never constructed (seat.h, B2 argument 2).
if shim_trace | grep -q 'origin=unset'; then
	fail "[d] an untagged event reached delivery:
$(shim_trace | grep 'origin=unset')"
fi
assert_key_pairing d
ok "8 events, 5 physical / 3 emulated, identical sequences at the core and at delivery, none untagged"

echo "== (E) the modifier-suppression rule (no double-shift) =="
# Shift_L is held down across an already-resolved 'a'. The normative rule
# says the shim binds non-modifier keysyms at an unmodified level, so the app
# must resolve 'a' as 'a' -- while genuinely seeing Shift as depressed, since
# a chord (Ctrl+C) has to keep working. Getting 'A' here is exactly the VNC
# double-shift bug the rule exists to forbid.
cat >"$RUNTIME_DIR/e.script" <<EOF
delay 400
key 0xffe1 pressed physical
delay 60
key 0x0061 pressed physical
key 0x0061 released physical
delay 60
key 0xffe1 released physical
delay 60
text emulated aA
delay 400
EOF
run_scenario e "$RUNTIME_DIR/e.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
got_text="$(sed -n 's/^TEXT //p' "$APP_LOG")"
[[ "$got_text" == "aaA" ]] \
	|| fail "[e] with Shift held, the app resolved '$got_text', expected 'aaA' (double-shift bug?)
$(grep '^IN key\|^IN modifiers' "$APP_LOG")"
# The Shift really was depressed as far as the app is concerned -- otherwise
# this would pass by not delivering the modifier at all, which is a different
# bug wearing the same result.
grep -qE '^IN modifiers depressed=0x1 ' "$APP_LOG" \
	|| fail "[e] the app never saw Shift depressed, so suppression proves nothing:
$(grep '^IN modifiers' "$APP_LOG")"
grep -qE '^IN modifiers depressed=0x0 ' "$APP_LOG" \
	|| fail "[e] the app never saw Shift released:
$(grep '^IN modifiers' "$APP_LOG")"
assert_key_pairing e
ok "Shift reaches the app as a real modifier, and does not re-shift resolved keysyms"

# --- the same rule, against the OTHER mechanism: Caps Lock ---
# Shift and Caps are not the same test, and passing the Shift case says
# nothing about this one. Shift drives LEVEL SELECTION, which any one-level
# key type neutralises for free. Caps Lock drives a separate libxkbcommon
# KEYSYM TRANSFORMATION applied after the level lookup, suppressed only by a
# key type that CONSUMES Lock. With a `modifiers = none` type -- the obvious
# way to write "no levels" -- the transformation is fully live, and a human
# tapping Caps Lock in the host window (the core translates evdev 58 to
# XK_Caps_Lock, so this is the ordinary physical path) silently upper-cases
# every subsequent agent payload until they tap it again. The shim's own
# delivery trace cannot see it: the shim sends the right keysyms and the app
# resolves them into different characters.
cat >"$RUNTIME_DIR/e2.script" <<EOF
delay 400
key 0xffe5 pressed physical
key 0xffe5 released physical
delay 60
text emulated hello
delay 60
text emulated ${GATE_TEXT}
delay 400
EOF
run_scenario e2 "$RUNTIME_DIR/e2.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
got_text="$(sed -n 's/^TEXT //p' "$APP_LOG")"
[[ "$got_text" == "hello${GATE_TEXT}" ]] \
	|| fail "[e2] with Caps Lock latched, the app resolved '$got_text', expected 'hello${GATE_TEXT}'
$(grep '^IN key\|^IN modifiers' "$APP_LOG")"
# Caps really did latch, so this cannot pass by failing to deliver it -- which
# would be a different bug with the same symptom. 0x2 is the Lock bit.
grep -qE '^IN modifiers depressed=0x0 latched=0x0 locked=0x2 ' "$APP_LOG" \
	|| fail "[e2] the app never saw Caps Lock locked, so suppression proves nothing:
$(grep '^IN modifiers' "$APP_LOG")"
assert_key_pairing e2
ok "Caps Lock reaches the app as locked state, and does not re-capitalize resolved keysyms"

echo "== (F) a newline renders as Return and a tab as Tab (D7, normative) =="
# The IDL pins these two by name, and xkbcommon does NOT agree by default:
# xkb_utf32_to_keysym('\n') is Linefeed (0xff0a), which no toolkit treats as
# Enter. So this checks that the shim overrides the library, not that the
# library is sane.
cat >"$RUNTIME_DIR/f.script" <<'EOF'
delay 400
text emulated a\tb\nc
delay 400
EOF
run_scenario f "$RUNTIME_DIR/f.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
expect_app_re f 1 '^IN key keycode=[0-9]+ state=1 keysym=0x0000ff09 name=Tab '
expect_app_re f 1 '^IN key keycode=[0-9]+ state=1 keysym=0x0000ff0d name=Return '
expect_app_re f 0 'keysym=0x0000ff0a'
got_hex="$(sed -n 's/^TEXT_HEX //p' "$APP_LOG")"
# 61 09 62 0a 63 = a <TAB> b <NL> c, asserted as bytes so a Linefeed slipping
# in where a Return belongs cannot pass by looking the same.
[[ "$got_hex" == "6109620a63" ]] \
	|| fail "[f] expected 'a<TAB>b<NL>c' (6109620a63), got $got_hex ($(sed -n 's/^TEXT //p' "$APP_LOG"))"
assert_key_pairing f
ok "a tab is delivered as Tab and a newline as Return (never Linefeed)"

echo "== (G) a string past the dynamic region is chunked, not corrupted =="
# 200 distinct CJK codepoints against a dynamic ring of 112: the delivery
# has to split into chunks, uploading one keymap each, and MUST NOT recycle
# a keycode while the string that used it is still being typed. The failure
# this catches is silent and ugly -- the tail of the string arriving as
# repeats of its own middle.
python3 - "$RUNTIME_DIR/g.script" <<'PYEOF'
import sys
# U+4E00.. is contiguous unified-CJK: 200 distinct codepoints with no legacy
# keysym at all -- the worst case for the dynamic keymap.
text = ''.join(chr(0x4e00 + i) for i in range(200))
with open(sys.argv[1], 'w') as f:
    f.write('delay 400\n')
    f.write('text emulated ' + text + '\n')
    f.write('delay 800\n')
with open(sys.argv[1] + '.expected', 'w') as f:
    f.write(text)
PYEOF
run_scenario g "$RUNTIME_DIR/g.script" timeout 15 "$ECHO_CLIENT" --run-ms 4000 --inset "$INSET"
got_text="$(sed -n 's/^TEXT //p' "$APP_LOG")"
want_text="$(cat "$RUNTIME_DIR/g.script.expected")"
[[ "$got_text" == "$want_text" ]] || fail "[g] a 200-codepoint string was corrupted by chunking
  got  (${#got_text} bytes): ${got_text:0:60}...
  want (${#want_text} bytes): ${want_text:0:60}..."
chunks="$(shim_trace | sed -n 's/.*chunks=\([0-9]*\).*/\1/p' | tail -1)"
(( chunks >= 2 )) || fail "[g] expected the delivery to chunk (>=2), the shim reported chunks=$chunks"
assert_key_pairing g
ok "200 distinct CJK codepoints delivered intact across $chunks keymap chunks"

echo "== (H) a keymap regeneration does not reset the app's modifier state =="
# This shim regenerates its keymap whenever a codepoint is new, which real
# compositors never do -- so the wlroots path that handles it is one nobody
# else exercises. `wlr_keyboard_set_keymap` builds a fresh xkb_state, replays
# the held keycodes into it, and emits only `events.keymap`: the client is
# told to rebuild its state (which resets its modifiers to zero) and is never
# told what the modifiers are. The shim must re-announce them itself.
#
# The probe is Ctrl+C. `utf8=""` is xkbcommon's control transformation
# working -- the app resolved U+0003 -- and `utf8="c"` is it not working,
# i.e. the app no longer believes Control is down. A human holding Ctrl while
# an agent types one new character is the concurrent operation this product
# is for, so "chords break from here on" is not an exotic case.
cat >"$RUNTIME_DIR/h.script" <<EOF
delay 400
key 0xffe3 pressed physical
delay 60
key 0x0063 pressed physical
key 0x0063 released physical
delay 60
text emulated 世
delay 60
key 0x0063 pressed physical
key 0x0063 released physical
delay 60
key 0xffe3 released physical
delay 400
EOF
run_scenario h "$RUNTIME_DIR/h.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
summary="$(grep '^SUMMARY ' "$APP_LOG")"
(( $(field keymaps) >= 2 )) \
	|| fail "[h] the new codepoint did not regenerate the keymap, so nothing was tested: $summary"
# Two chords, press and release each: four lines, all resolving under Control.
#
# The probe is the BYTE, not the rendering. `xkb_state_key_get_utf8` returns
# U+0003 for Ctrl+C, which the echo client prints verbatim -- so the log line
# reads `utf8=""` in a terminal while actually containing 0x03, and a regex
# written against what the eye sees would match the broken case too. The
# literal control byte goes into the pattern instead.
CTRL_C=$'\003'
expect_app_re h 4 "^IN key keycode=[0-9]+ state=[01] keysym=0x00000063 name=c utf8=\"$CTRL_C\"\$"
expect_app_re h 0 '^IN key keycode=[0-9]+ state=[01] keysym=0x00000063 name=c utf8="c"$'
assert_key_pairing h
ok "a chord held across a keymap regeneration still resolves as a chord"

echo "== (I) 'text' delivers a string, not keystrokes =="
# The IDL names exactly two control characters that become keystrokes (\n ->
# Return, \t -> Tab, checked in (F)). Every other one must be dropped, and
# that cannot be left to xkbcommon: NOT ONE C0 codepoint comes back as
# NoSymbol from `xkb_utf32_to_keysym`. U+0008 is BackSpace, U+001B is Escape,
# U+007F is Delete, U+000B is Clear -- all pre-bound in the warm region, so
# they would be delivered without even a regeneration. An agent pasting text
# scraped from a terminal or an LLM response with one stray byte would erase
# the user's existing text or dismiss the dialog it was filling in.
#
# Asserted on the `IN key ... name=` lines, NOT on TEXT: the echo client
# deliberately excludes control characters from its accumulator (as a widget
# does), so a payload whose backspace ate a character still compares equal.
cat >"$RUNTIME_DIR/i.script" <<'EOF'
delay 400
text emulated ab\x08c\x1bd\x7fe\x0bf\x01g
delay 400
EOF
run_scenario i "$RUNTIME_DIR/i.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
for keyname in BackSpace Escape Delete Clear; do
	expect_app_re i 0 "^IN key .* name=$keyname "
done
# Unicode-offset keysyms (0x0100000b and friends) are the other half of the
# same mistake: they carry the control character itself.
expect_app_re i 0 '^IN key .* keysym=0x010000[0-1][0-9a-f] '
got_hex="$(sed -n 's/^TEXT_HEX //p' "$APP_LOG")"
[[ "$got_hex" == "$(printf 'abcdefg' | od -An -tx1 | tr -d ' \n')" ]] \
	|| fail "[i] expected the seven letters and nothing else, got $got_hex
$(grep '^IN key' "$APP_LOG")"
# The drop is reported rather than silent: five refused codepoints, and the
# delivery is `partial` because the app got fewer characters than were asked
# for.
shim_trace | grep -q 'event=text .* reason=partial .* unmappable=5 ' \
	|| fail "[i] the shim did not report the refused control characters:
$(shim_trace | grep 'event=text')"
assert_key_pairing i
ok "control characters in a text payload are dropped and counted, never pressed"

echo "== (J) a key the human holds does not collide with agent text =="
# The human rests a finger on space while an agent types a string containing
# one. Both want the same keysym. Reusing the keycode the app already holds
# would emit a second PRESSED with no RELEASED between AND clear the shim's
# `held` flag, so the human's real release would be dropped as unpaired --
# leaving the app holding a key down forever. The collision set is every
# keysym the core can send that `text` can also produce: space, Tab, Return
# (the IDL's own scenario types a string ending in "\n"), Escape, BackSpace,
# the arrows.
cat >"$RUNTIME_DIR/j.script" <<EOF
delay 400
key 0x0020 pressed physical
delay 60
text emulated a b
delay 60
key 0x0020 released physical
delay 400
EOF
run_scenario j "$RUNTIME_DIR/j.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
assert_key_pairing j
# The human's press and release are one pair on ONE keycode, and the agent's
# space is a second pair on a different one: three presses of the space
# keysym in total across exactly two distinct keycodes.
space_codes="$(grep -oE '^IN key keycode=[0-9]+ state=1 keysym=0x00000020 ' "$APP_LOG" \
	| awk '{print $3}' | sort -u | wc -l)"
(( space_codes == 2 )) || fail "[j] the space keysym was pressed on $space_codes distinct keycodes, expected 2:
$(grep 'keysym=0x00000020' "$APP_LOG")"
# The human's own release was delivered, not swallowed as unpaired.
shim_trace | grep -q 'event=key .* delivered=1 .* keysym=0x00000020 state=released' \
	|| fail "[j] the human's space release was not delivered:
$(shim_trace | grep 'keysym=0x00000020')"
# And the string is intact: the human's held space, then 'a b'.
got_text="$(sed -n 's/^TEXT //p' "$APP_LOG")"
[[ "$got_text" == " a b" ]] || fail "[j] expected ' a b', got '$got_text'
$(grep '^IN key' "$APP_LOG")"
ok "a held key and the same character in agent text take separate keycodes"

echo "== (K) pointer events batch per wire batch, not per event =="
# The second open question the issue names, asserted rather than asserted-to.
# `wl_pointer.frame` marks a group of pointer events that belong together,
# and the group the core has is the one it routed from a single cause -- a
# diagonal scroll is two `scroll` events for one wheel motion. The shim
# closes a frame per drained wire batch, so the two scrolls below share one
# frame when they arrive together and take one each when they do not.
#
# Without both halves this proves nothing: an implementation that emitted a
# frame after every single event would pass the first assertion's event
# counts and fail only the frame count, and one that never batched at all
# would pass a lone unbatched scenario. The mock core writes consecutive
# script events in one socket write precisely so "arrived together" is a
# fact rather than a race with the scheduler.
cat >"$RUNTIME_DIR/k1.script" <<EOF
delay 400
motion 100 100 emulated
delay 200
scroll vertical -120 emulated
scroll horizontal 240 emulated
delay 400
EOF
run_scenario k1 "$RUNTIME_DIR/k1.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
summary="$(grep '^SUMMARY ' "$APP_LOG")"
[[ "$(field v120)" == 2 ]] || fail "[k1] expected 2 scroll events: $summary"
# 2 = the enter's own frame (wlroots sends it) + one shared by both scrolls.
[[ "$(field frame)" == 2 ]] \
	|| fail "[k1] two scrolls in one wire batch produced $(field frame) frames, expected 2 (one shared): $summary"

cat >"$RUNTIME_DIR/k2.script" <<EOF
delay 400
motion 100 100 emulated
delay 200
scroll vertical -120 emulated
delay 200
scroll horizontal 240 emulated
delay 400
EOF
run_scenario k2 "$RUNTIME_DIR/k2.script" timeout 12 "$ECHO_CLIENT" --run-ms 3000 --inset "$INSET"
summary="$(grep '^SUMMARY ' "$APP_LOG")"
[[ "$(field v120)" == 2 ]] || fail "[k2] expected 2 scroll events: $summary"
[[ "$(field frame)" == 3 ]] \
	|| fail "[k2] two scrolls in separate wire batches produced $(field frame) frames, expected 3: $summary"
ok "the same two scrolls share one frame when batched and take one each when not"

echo
echo "PASS: all P1.6.3 seat-input-replay acceptance checks green"
