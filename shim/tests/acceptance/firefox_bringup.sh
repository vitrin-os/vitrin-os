#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# SHIM-ONLY UNIT CHECK: Firefox against the shim under the MOCK core.
#
# WHAT THIS IS, AND WHAT IT IS NOT (relabelled by P1.6.6, issue #106).
#
# This script runs the real pinned Firefox ESR against the real C shim, but the
# core it runs against is `shim/tests/mock_core.c` -- a hand-written core
# stand-in that reads a dominant colour off every committed frame. That makes
# it a valuable SHIM-IN-ISOLATION SMOKE TEST: it exercises the shim standalone,
# with no Rust core and no Python SDK in the path, and it can assert an ORDERED
# colour sequence per frame in a way the poll-per-frame SDK cannot. It is NOT,
# however, the M1.2 milestone integration proof, precisely because the core is
# a mock -- one half of the system has never met the other real half here.
#
# THE MILESTONE PROOF IS `tests/integration/test_real_firefox.py` (P1.6.6): the
# shipped `vitrind` execs this same shim, which fork/execs this same Firefox,
# and the real Python SDK captures a real Firefox frame of a solid-colour page
# and asserts its dominant colour, with the globals ledger asserted against the
# same refused-globals allowlist check (D) uses below. That gate, not this
# script, is what "Shim runs Firefox" is held on. This script stays because it
# still usefully exercises the shim on its own; treat a green here as "the shim
# is healthy in isolation", not as "the milestone is met".
#
# The top of the R4 bring-up ladder (weston-terminal -> GTK app -> Firefox).
# Firefox is the MVP's real app, so this script is deliberately empirical: it
# runs the pinned ESR against the shim under the mock core and asserts on what
# crossed the wire.
#
# WHAT IS ASSERTED, AND HOW EACH CLAIM IS MADE OBSERVABLE
#
# This script runs under `mock_core.c`, not the real `vitrind` (see the
# header above) -- a deliberate shim-in-isolation choice, not a limitation of
# `spawn_realm`, which has had a real, non-test caller since P1.5.4/#103
# (`session::start_realm` in the shipped binary). So every criterion here is
# reduced to something measurable in the pixels the shim actually forwarded.
# The mock core reports a DOMINANT COLOUR per committed
# frame (mock_core.c), which is the M1.2 verification the plan specifies:
# "Firefox smoke (local page rendering a known solid color, assert dominant
# color)" (docs/plan/01-phase-1-mvp.md section 5). Every page is a local
# file:// URL. Nothing here touches the network, ever.
#
#   (A) "renders and repaints in the core's nested window"
#         pages/repaint.html paints #0000ff, then #00ff00 2.5 s later. Assert
#         both, IN THAT ORDER, among the committed frames -- a browser that
#         painted once and wedged satisfies a one-colour assertion perfectly.
#         Plus: frames genuinely flowed (commit count), the core validated
#         every one of them (zero wire violations), and at least one repaint
#         was PARTIAL (damage_area < surface_area), which is the damage path
#         doing its job rather than a full-surface blit every frame.
#   (B) "injected pointer scroll works in the page"
#         pages/scroll.html is three viewports tall, starts #ff0000, and
#         repaints #ffff00 only once the document has really scrolled past a
#         third of a viewport. A frame digest change would prove only that
#         SOMETHING moved; the colour can change for one reason.
#   (C) "injected text lands in the URL bar"
#         Ctrl+L, then the file:// URL of pages/urlbar-target.html as ONE
#         `text` payload ending in "\n" (which the IDL requires be delivered
#         as Return). If #00ffff becomes dominant, the text reached the URL
#         bar as text, parsed as a URL, and Return actuated it. The URL bar is
#         browser chrome with no readable text buffer, so this is the
#         strongest available observation -- see pages/urlbar-target.html for
#         what it does and does not establish.
#   (D) "globals-touched log; every stub addition traces to a log entry"
#         A probe run (shim --probe-globals) records every interface Firefox
#         binds INCLUDING the ones the v0 set does not provide, which produce
#         no wire traffic at all otherwise (shim/include/ledger.h). Assert the
#         v0 contract is exactly what we think it is, that the probe mechanism
#         genuinely fired, and that Firefox demanded NOTHING outside the
#         documented refusal list in shim/docs/firefox.md. A new ESR that
#         wants something new turns this red, which is the point: the global
#         set is a contract, and a change to what an app needs should require
#         a decision, not go unnoticed.
#
# Requires: the pinned Firefox (shim/tests/firefox/fetch-esr.sh) plus
# vitrin-shim and mock-core from `meson compile -C build`. Runs headless and
# GPU-free with software WebRender -- a documented supported configuration,
# see shim/docs/firefox.md.
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/firefox_bringup.sh
set -Eeuo pipefail

# Numbers are compared as text, so the decimal separator must be the one the C
# programs print rather than the one the operator's locale prefers.
export LC_ALL=C

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM_DIR="$(cd "$HERE/../.." && pwd)"
REPO_ROOT="$(cd "$SHIM_DIR/.." && pwd)"
FF_DIR="$SHIM_DIR/tests/firefox"
PAGES="$FF_DIR/pages"

BUILD_DIR="${BUILD_DIR:-$SHIM_DIR/build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"

VIEW_W="${VIEW_W:-1024}"
VIEW_H="${VIEW_H:-768}"

# How much of the realm view a page's own colour covers. The rest is Firefox's
# chrome (tab strip + toolbar), which is ~12% of a 1024x768 view -- measured,
# not guessed. The bar is set well below that so a chrome-height change in a
# future ESR does not fail a render assertion, while still being far above
# what any incidental colour could reach.
MIN_DOMINANT_PCT="${MIN_DOMINANT_PCT:-55}"
# Frames the shim must have forwarded for "frames flow" to mean anything.
# Observed runs produce 50-1200 depending on how hard the page works the
# compositor; 20 is a floor, not a target.
MIN_COMMITS="${MIN_COMMITS:-20}"

# How much longer the mock core lives than the browser's own window. The core
# is wound down explicitly once the browser is gone (see run_scenario), so this
# is a SAFETY NET for a run that wedges, not the scenario length.
CORE_MARGIN_S="${CORE_MARGIN_S:-30}"

# The probe catalogue this build is expected to compile in, and how many of
# those get armed. shim/meson.build resolves each catalogue row against the
# build machine's wayland-protocols and DROPS any row whose XML it cannot find,
# so a machine one release behind produces a smaller catalogue -- and check (D)
# below would then pass having never offered Firefox the interfaces it dropped.
# Asserting both numbers is what keeps "a new ESR that wants something new
# turns this red" true; without it the check gets weaker the more rows the
# build silently loses.
#
# EXPECT_ARMED = EXPECT_CATALOGUE - (catalogue entries that are in the v0 set),
# because vitrin_ledger_create_probes refuses to shadow a real global with an
# inert one. Today FOUR entries qualify: wl_subcompositor, which P1.6.4
# promoted into the contract, and the three pointer interfaces WS-E.4.2 (issue
# #222) did -- zwp_relative_pointer_manager_v1, zwp_pointer_gestures_v1 and
# zwp_pointer_constraints_v1. Changing the catalogue means changing both.
#
# The three rows STAY in the catalogue rather than being deleted from
# shim/meson.build, deliberately: the catalogue is a compiled-in list of
# things a build COULD probe, and `in_v0_contract` is the runtime rule that
# declines to arm one that is already served. Deleting the rows would shrink
# EXPECT_CATALOGUE and lose the record that these three were once probes --
# which is the evidence that admitted them.
EXPECT_CATALOGUE="${EXPECT_CATALOGUE:-21}"
EXPECT_ARMED="${EXPECT_ARMED:-17}"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# --- the Firefox gate ----------------------------------------------------
# The same rule the P1.6.2 cross-track conformance test and the P1.6.3 GTK
# gate apply to themselves: opt-in locally, but NEVER a silent skip under CI.
# A named acceptance criterion that only ever reports SKIP on the machine that
# gates merges is a criterion nobody is holding. The shim's CI job does not
# ship a browser (shim/ci/install-deps.sh keeps that job small and Rust-free),
# so a job that wants this check must fetch the pinned ESR --
# shim/tests/firefox/fetch-esr.sh -- or declare the gap deliberately.
FIREFOX_BIN="${FIREFOX_BIN:-$(bash "$FF_DIR/fetch-esr.sh" --print)}"
if [[ ! -x "$FIREFOX_BIN" ]]; then
	if [[ -n "${CI:-}" && -z "${VITRIN_SKIP_FIREFOX_GATE:-}" ]]; then
		fail "the pinned Firefox is not present at $FIREFOX_BIN, so NONE of the
P1.6.4 criteria were proved. Fetch it (bash shim/tests/firefox/fetch-esr.sh)
or declare the gap deliberately with VITRIN_SKIP_FIREFOX_GATE=1."
	fi
	echo "SKIP: no Firefox at $FIREFOX_BIN"
	echo "SKIP: run 'bash shim/tests/firefox/fetch-esr.sh' first."
	echo "SKIP: NONE of the P1.6.4 render/scroll/urlbar/globals criteria are proven here."
	exit 0
fi

for bin in "$SHIM_BIN" "$MOCK_CORE"; do
	[[ -x "$bin" ]] || fail "missing $bin (run: meson compile -C $BUILD_DIR)"
done

# Verify the pin on every run rather than trusting a browser that happens to
# be on disk: an unverified binary makes every result below unattributable to
# a version.
#
# --verify-only, so this NEVER reaches the network. Plain fetch-esr.sh
# re-downloads whenever the tarball is missing, which is a normal state for a
# developer who reclaimed 75 MB and kept the unpacked browser -- and a test
# that silently fetches from a CDN is a test a network flake can redden.
#
# Since issue #298 `--verify-only` checks the UNPACKED tree as well as the
# tarball (application.ini's Version and BuildID), which is the check that
# catches Firefox updating itself in place. The `--version` comparison below
# is NOT redundant with it: fetch-esr.sh verifies the tree it manages, while
# $FIREFOX_BIN is overridable from the environment, so this is what holds a
# caller-supplied browser to the pin the rest of this script's evidence is
# attributed to.
source "$FF_DIR/firefox-esr.pin"
bash "$FF_DIR/fetch-esr.sh" --verify-only >/dev/null \
	|| fail "the pinned Firefox failed verification (run: bash shim/tests/firefox/fetch-esr.sh)"
reported="$("$FIREFOX_BIN" --version 2>/dev/null || true)"
[[ "$reported" == *"$VITRIN_FIREFOX_VERSION"* ]] \
	|| fail "pinned $VITRIN_FIREFOX_VERSION but the binary reports '$reported'"
ok "Firefox pin verified: $reported (sha256 $VITRIN_FIREFOX_SHA256, build $VITRIN_FIREFOX_BUILDID)"

export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1

# Short runtime dir: a Unix socket path is capped at 108 bytes.
RUNTIME_DIR="$(mktemp -d /tmp/vitrin-ff.XXXXXX)"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

CORE_PID=""
# Reap on EVERY exit path, and reap only THIS run. The previous cleanup used
# `pkill -x mock-core` / `pkill -x vitrin-shim`, which also killed the cores
# and shims of any other test running on the machine at the time. Everything
# this script starts either descends from $CORE_PID or names $RUNTIME_DIR --
# which is mktemp-unique -- so both handles are precise.
cleanup() {
	local rc=$?
	if [[ -n "$CORE_PID" ]]; then
		kill -TERM "$CORE_PID" 2>/dev/null || true
		# The core SIGKILLs its shim if it does not go quietly, so a brief
		# wait here is what turns "asked to stop" into "stopped".
		for _ in 1 2 3 4 5 6 7 8 9 10; do
			kill -0 "$CORE_PID" 2>/dev/null || break
			sleep 0.2
		done
		kill -KILL "$CORE_PID" 2>/dev/null || true
	fi
	# Anything still holding this run's runtime dir: the browser (whose
	# profile, socket and HOME all live under it) and any straggler shim.
	pkill -f "$RUNTIME_DIR" 2>/dev/null || true
	rm -rf "$RUNTIME_DIR"
	exit "$rc"
}
trap cleanup EXIT INT TERM

# Run one scenario. $1 = tag, $2 = page (or "" for about:blank),
# $3 = run seconds, $4 = input script or "", $5.. = extra shim args.
run_scenario() {
	local tag="$1" page="$2" secs="$3" script="$4"; shift 4
	local socket="ff-$tag-$$"
	CORE_LOG="$RUNTIME_DIR/$tag.core.log"
	SHIM_LOG="$RUNTIME_DIR/$tag.shim.log"
	APP_LOG="$RUNTIME_DIR/$tag.app.log"
	LEDGER="$RUNTIME_DIR/$tag.globals.log"

	local input_args=()
	[[ -n "$script" ]] && input_args=(--input "$script" --input-after-commits 1)

	# THE CORE MUST OUTLIVE THE BROWSER, and it is wound down deliberately
	# rather than by expiry.
	#
	# This used to be `--run-ms $((secs * 1000))`, started BEFORE the socket
	# wait and the profile copy, with the browser then given its own `timeout
	# $secs`. The core's deadline therefore always expired first, by the
	# startup time -- so every scenario ended by pulling the compositor out
	# from under a live Firefox, whose WaylandProxy watchdog aborted the
	# process ("Wayland protocol error: Compositor () crashed", then a core
	# dump). A browser crash was the NORMAL end of a passing run, which made
	# the health check below unusable: "Firefox crashed" cannot be an error
	# condition while the harness guarantees it every time. It also filled the
	# system coredump journal with multi-megabyte dumps, several per run.
	#
	# So --run-ms is now only a safety net for a wedged run, and the real
	# sequence is: browser exits -> SIGTERM the core -> core prints its
	# SUMMARY and tears the shim down. A crash is now genuinely anomalous.
	"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms $(( (secs + CORE_MARGIN_S) * 1000 )) \
		"${input_args[@]}" \
		-- "$SHIM_BIN" --socket "$socket" --globals-log "$LEDGER" "$@" \
		>"$CORE_LOG" 2>"$SHIM_LOG" &
	CORE_PID=$!

	local sock="$XDG_RUNTIME_DIR/$socket"
	local deadline=$(( $(date +%s) + 20 ))
	until [[ -S "$sock" ]]; do
		kill -0 "$CORE_PID" 2>/dev/null || fail "[$tag] the shim never bound a socket:
$(cat "$SHIM_LOG")"
		(( $(date +%s) >= deadline )) && fail "[$tag] timeout waiting for $sock"
		sleep 0.1
	done

	# A FRESH PROFILE PER SCENARIO. Session state, a restored scroll position
	# or a "restore previous session" bar would each make a later run differ
	# from an earlier one -- which is the definition of a flaky harness.
	local prof="$RUNTIME_DIR/$tag.prof"
	mkdir -p "$prof"
	cp "$FF_DIR/profile.user.js" "$prof/user.js"

	# `env -i`: the shim's app must inherit exactly the environment the demo
	# documents and nothing from the operator's session -- no host
	# WAYLAND_DISPLAY, no DISPLAY, no toolkit theme, no locale. See
	# shim/docs/firefox.md for what each variable is for.
	local url="about:blank"
	[[ -n "$page" ]] && url="file://$page"
	# THE BROWSER'S FATE IS EVIDENCE, so it is captured rather than discarded.
	# `-k 5`: a browser that ignores the timeout's SIGTERM gets SIGKILL five
	# seconds later, so a wedged run still ends -- `timeout` reports 124 for a
	# timeout either way.
	local ff_rc=0
	env -i \
		HOME="$prof" \
		XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
		WAYLAND_DISPLAY="$socket" \
		PATH=/usr/bin:/bin \
		MOZ_ENABLE_WAYLAND=1 \
		GDK_BACKEND=wayland \
		MOZ_ACCELERATED=0 \
		LIBGL_ALWAYS_SOFTWARE=1 \
		MOZ_CRASHREPORTER_DISABLE=1 \
		GTK_A11Y=none \
		NO_AT_BRIDGE=1 \
		timeout -k 5 "$secs" "$FIREFOX_BIN" --profile "$prof" --no-remote "$url" \
		>"$APP_LOG" 2>&1 || ff_rc=$?

	# Now that the browser is gone, wind the core down. It handles SIGTERM by
	# leaving its loop, printing SUMMARY and tearing the shim down, so the
	# report is as complete as an expiry would have produced.
	kill -TERM "$CORE_PID" 2>/dev/null || true
	wait "$CORE_PID" || fail "[$tag] the mock core reported violations:
$(grep '^FAIL' "$CORE_LOG" || true)"
	if grep -q '^FAIL' "$CORE_LOG"; then
		fail "[$tag] wire violations:
$(grep '^FAIL' "$CORE_LOG")"
	fi
	# A wlroots assertion kills the shim outright and takes the realm with it.
	# Firefox found exactly one of those during this task (a state request
	# before the initial commit, xdg.c), so it is checked every run.
	if grep -q 'Assertion' "$SHIM_LOG"; then
		fail "[$tag] the shim aborted on an assertion:
$(grep -B2 'Assertion' "$SHIM_LOG")"
	fi

	# DID THE BROWSER SURVIVE ITS WINDOW?
	#
	# Without this, every assertion below is one a corpse can satisfy: a
	# Firefox that painted blue, painted green and then segfaulted at t=6s in
	# a 20 s run leaves a log that check (A) reads as a complete pass, because
	# the colours and the commit count are all in it. The frames a dead
	# browser already forwarded do not go away.
	#
	# 124 is `timeout` doing its job -- the browser was still running when its
	# window closed, which is the healthy outcome for every scenario here. 0 is
	# a clean self-exit. Anything else, and in particular >=128 (killed by a
	# signal: 139 SIGSEGV, 134 SIGABRT), means it died, and it is reported as
	# ITSELF rather than being left to surface three assertions later as a
	# confusing "no committed frame was dominantly #00ffff".
	if (( ff_rc != 124 && ff_rc != 0 )); then
		local how="exit status $ff_rc"
		(( ff_rc >= 128 )) && how="killed by signal $(( ff_rc - 128 ))"
		fail "[$tag] Firefox did not survive its ${secs}s window ($how).
The render/scroll/urlbar assertions below are NOT evidence when the browser
died mid-run -- the frames it had already forwarded stay in the log.
--- firefox (crash markers) ---
$(grep -E 'Wayland protocol error|Exiting due to channel error|dumped core|Segmentation' "$APP_LOG" | head -10 || true)
--- firefox (tail) ---
$(tail -15 "$APP_LOG")
--- shim (tail) ---
$(tail -15 "$SHIM_LOG")"
	fi
	# Even a browser that reached its timeout can have lost its connection and
	# limped on, which is the shape the compositor-pulled-out-from-under bug
	# produced. These strings are Firefox reporting exactly that.
	if grep -qE 'Wayland protocol error|Exiting due to channel error' "$APP_LOG"; then
		fail "[$tag] Firefox reported losing its Wayland connection during the run:
$(grep -E 'Wayland protocol error|Exiting due to channel error' "$APP_LOG" | head -5)
The core is supposed to outlive the browser (see run_scenario); if this fires,
the teardown order regressed or the shim dropped the connection."
	fi

	grep -q 'app window mapped' "$SHIM_LOG" \
		|| fail "[$tag] Firefox never mapped a window:
$(tail -20 "$SHIM_LOG")
--- firefox ---
$(tail -20 "$APP_LOG")"
}

# The dominant colours of the committed frames, in order, de-duplicated so the
# sequence reads as "what was on screen, in what order" rather than as one
# entry per frame.
dominant_sequence() { grep -o 'dominant=[0-9a-f]*' "$CORE_LOG" | sed 's/dominant=//' | uniq; }

commits() { grep -c '^EV commit' "$CORE_LOG" || true; }

# Assert a colour was dominant on some frame, with enough coverage to mean it,
# and return the index of the first such frame so ORDER can be asserted too.
first_index_of() {
	local want="$1"
	dominant_sequence | grep -n -x -- "$want" | head -1 | cut -d: -f1
}

assert_colour() {
	local tag="$1" want="$2"
	local pct
	pct="$(grep -o "dominant=$want dominant_pct=[0-9]*" "$CORE_LOG" \
		| sed 's/.*dominant_pct=//' | sort -rn | head -1)"
	[[ -n "$pct" ]] || fail "[$tag] no committed frame was dominantly #$want.
Observed colour sequence: $(dominant_sequence | tr '\n' ' ')"
	(( pct >= MIN_DOMINANT_PCT )) || fail \
		"[$tag] #$want was dominant but only over $pct% of the view (need $MIN_DOMINANT_PCT%)"
	echo "$pct"
}

echo "== (A) Firefox renders and repaints; frames flow shim -> core =="
run_scenario a "$PAGES/repaint.html" 20 ""
n="$(commits)"
(( n >= MIN_COMMITS )) || fail "[a] only $n frames reached the core (need $MIN_COMMITS)"
pct_blue="$(assert_colour a 0000ff)"
pct_green="$(assert_colour a 00ff00)"
i_blue="$(first_index_of 0000ff)"
i_green="$(first_index_of 00ff00)"
[[ -n "$i_blue" && -n "$i_green" ]] || fail "[a] could not locate both colours in the sequence"
(( i_blue < i_green )) || fail "[a] #00ff00 appeared before #0000ff -- the page's own
order is blue then green, so this is not the repaint the test set up:
$(dominant_sequence | tr '\n' ' ')"
ok "$n frames forwarded; #0000ff ($pct_blue%) then #00ff00 ($pct_green%) -- rendered AND repainted"

# Partial damage: at least one commit repainted less than the whole surface.
# Without this the render criterion is satisfied by a full-surface blit per
# frame, which is not what the damage path is for.
partial="$(awk '
	{
		da = ""; sa = ""
		for (i = 1; i <= NF; i++) {
			if ($i ~ /^damage_area=/)  { da = substr($i, 13) }
			if ($i ~ /^surface_area=/) { sa = substr($i, 14) }
		}
		if (da != "" && sa != "" && da+0 > 0 && da+0 < sa+0) { c++ }
	}
	END { print c+0 }' "$CORE_LOG")"
(( partial > 0 )) || fail "[a] every one of the $n commits damaged the whole surface;
no incremental repaint was observed"
ok "$partial of $n commits carried partial damage (incremental repaint, not full blits)"

echo "== (B) injected pointer scroll reaches the page =="
# Six notches with the pointer over the content area. The page repaints only
# after the document has really scrolled past a third of a viewport, so this
# cannot be satisfied by the scroll event merely being delivered.
cat >"$RUNTIME_DIR/b.script" <<EOF
delay 9000
motion $((VIEW_W / 2)) $((VIEW_H / 2)) emulated
delay 300
scroll vertical 120 emulated
delay 150
scroll vertical 120 emulated
delay 150
scroll vertical 120 emulated
delay 150
scroll vertical 120 emulated
delay 150
scroll vertical 120 emulated
delay 150
scroll vertical 120 emulated
delay 4000
EOF
run_scenario b "$PAGES/scroll.html" 22 "$RUNTIME_DIR/b.script"
pct_red="$(assert_colour b ff0000)"
pct_yellow="$(assert_colour b ffff00)"
i_red="$(first_index_of ff0000)"
i_yellow="$(first_index_of ffff00)"
(( i_red < i_yellow )) || fail "[b] the page was yellow before it was red, so the
colour change cannot be attributed to the injected scroll:
$(dominant_sequence | tr '\n' ' ')"
sends="$(sed -n 's/.*SUMMARY .*seat_sends=\([0-9]*\).*/\1/p' "$CORE_LOG")"
(( sends == 7 )) || fail "[b] the core sent $sends seat events, expected 7"
ok "7 injected events; #ff0000 ($pct_red%) -> #ffff00 ($pct_yellow%) -- the document really scrolled"

echo "== (C) injected text lands in the URL bar =="
# Ctrl+L focuses the URL bar; the payload is one `text` event ending in a
# newline, which the IDL requires be delivered as Return (P1.6.3 check F).
TARGET_URL="file://$PAGES/urlbar-target.html"
cat >"$RUNTIME_DIR/c.script" <<EOF
delay 10000
motion $((VIEW_W / 2)) $((VIEW_H / 2)) emulated
delay 300
key 0xffe3 pressed physical
key 0x006c pressed physical
key 0x006c released physical
key 0xffe3 released physical
delay 800
text emulated ${TARGET_URL}\\n
delay 6000
EOF
run_scenario c "$PAGES/repaint.html" 24 "$RUNTIME_DIR/c.script"
pct_cyan="$(assert_colour c 00ffff)"
i_blue="$(first_index_of 0000ff)"
i_cyan="$(first_index_of 00ffff)"
[[ -n "$i_blue" ]] || fail "[c] the origin page never rendered, so nothing was navigated FROM"
(( i_blue < i_cyan )) || fail "[c] the target colour preceded the origin colour:
$(dominant_sequence | tr '\n' ' ')"
ok "typed URL navigated the browser: #00ffff ($pct_cyan%) after the origin page"

echo "== (D) the globals-touched ledger, and what Firefox demanded =="
# A probe run: every interface Firefox binds is recorded, INCLUDING the ones
# the v0 set does not provide -- which produce no wire traffic whatsoever
# without the probe catalogue (shim/include/ledger.h).
run_scenario d "$PAGES/repaint.html" 20 "" --probe-globals

# THE PROBE CATALOGUE WAS FULLY ARMED.
#
# Everything below is a subset test against an allowlist, and a subset test
# gets EASIER the fewer probes were offered: with a two-entry catalogue all
# four of check (D)'s assertions pass while 19 of the interfaces Firefox wants
# were never offered to it. meson drops any catalogue row whose XML it cannot
# resolve, so that is not a hypothetical -- it is what a build machine one
# wayland-protocols release behind produces, and it reports it with a
# configure-time message nobody reads in a normal ninja build.
#
# So the two numbers the ledger already carries are asserted, and the armed
# count is printed in the OK line, so a short run can never read as a full one.
armed="$(sed -n 's/^globals-log: probes_armed=\([0-9]*\) .*/\1/p' "$LEDGER")"
catalogue="$(sed -n 's/^globals-log: probes_armed=[0-9]* catalogue=\([0-9]*\) .*/\1/p' "$LEDGER")"
[[ -n "$armed" && -n "$catalogue" ]] || fail "[d] the ledger recorded no probe banner at all;
--probe-globals may not have reached the shim:
$(grep '^globals-log:' "$LEDGER" || true)"
(( catalogue == EXPECT_CATALOGUE )) || fail "[d] this build compiled in $catalogue probe
catalogue entries, expected $EXPECT_CATALOGUE. meson DROPS a row whose XML it cannot find on
the build machine, so a short catalogue silently narrows every assertion below.
Re-run 'meson setup --reconfigure' and read the 'probe catalogue: no XML found
for ...' warning. If the catalogue changed deliberately, update EXPECT_CATALOGUE
and EXPECT_ARMED at the top of this script."
(( armed == EXPECT_ARMED )) || fail "[d] $armed probes were armed, expected $EXPECT_ARMED
(catalogue $catalogue, minus the entries already in the v0 set). Firefox was
therefore never offered some of the interfaces this check claims to survey:
$(grep '^globals-log:' "$LEDGER" || true)"

# The v0 contract, asserted against the wire rather than against a comment.
# `class=v0` is everything the shim advertised that is NOT a probe, so this
# fails the moment a dependency starts creating a global as a side effect --
# which is the failure mode "a contract, not a floor" exists to catch.
#
# ANCHORED, like every record grep in this file. A ledger record is a record
# only if it STARTS a line (ledger.h, "one record is one line, structurally").
# The confined app cannot forge a whole record -- ledger.c resolves binds
# through the registry name and scrubs control bytes -- but libwayland's error
# text quotes the client's own interface string back at it, so an app CAN get
# arbitrary bytes into the middle of a legitimate `globals-error` line. An
# unanchored grep reads those bytes as a record: an app that binds with the
# interface string "zz\nglobals-touched: interface=x class=v0" put a v0 row
# into this very comparison. Anchoring is what makes the app's bytes stay
# inside the field they landed in.
got_v0="$(grep -o '^globals-touched: interface=[a-z0-9_]* class=v0' "$LEDGER" \
	| sed 's/.*interface=\([a-z0-9_]*\) .*/\1/' | sort -u | tr '\n' ' ')"
want_v0="wl_compositor wl_data_device_manager wl_output wl_seat wl_shm wl_subcompositor xdg_wm_base zwp_pointer_constraints_v1 zwp_pointer_gestures_v1 zwp_relative_pointer_manager_v1 zxdg_decoration_manager_v1 "
[[ "$got_v0" == "$want_v0" ]] || fail "[d] the advertised v0 global set is not the contract
  got:  $got_v0
  want: $want_v0
If this is a deliberate change, update shim/README.md, shim/src/globals.c and
the want list here -- in that order, citing the globals-demand line."
ok "the advertised v0 set is exactly the contract: $got_v0"

# The shim's OWN cross-check ran and agreed. ledger.c holds a second copy of
# the contract (it needs one before any client connects, to keep a probe from
# shadowing a real global) and reconciles it against the wire at teardown.
# Asserting `drift=0` here proves that reconciliation actually executed --
# without it, a build where the check silently no-ops would still pass the
# comparison above.
drift="$(sed -n 's/^globals-summary:.* drift=\([0-9]*\).*/\1/p' "$LEDGER")"
[[ "$drift" == "0" ]] || fail "[d] the shim reported contract drift ($drift):
$(grep '^globals-contract-drift' "$LEDGER")"
ok "the shim's own contract cross-check agrees with the wire (drift=0)"

# The summary's own copy of the probe counts must agree with the banner's.
# They are emitted by different code paths at opposite ends of the run, so a
# disagreement means probes were torn down or recounted mid-run.
armed_end="$(sed -n 's/^globals-summary:.* probes_armed=\([0-9]*\).*/\1/p' "$LEDGER")"
[[ "$armed_end" == "$armed" ]] || fail "[d] the ledger disagrees with itself about how many
probes were armed: banner says $armed, summary says $armed_end"

# The probe mechanism genuinely fired. Without this the next assertion passes
# trivially on a build where probing silently does nothing.
demands="$(grep -o '^globals-demand: seq=[0-9]* interface=[a-z0-9_]*' "$LEDGER" \
	| sed 's/.*interface=//' | sort -u)"
[[ -n "$demands" ]] || fail "[d] the probe run recorded no globals-demand lines at all.
Either --probe-globals did nothing, or the catalogue is empty:
$(grep '^globals-log:' "$LEDGER")"

# Every demand must be an interface we have already decided to refuse, with
# the argument written down. A new one is not a failure of the shim -- it is
# NEWS, and it must reach a human rather than a log nobody reads.
REFUSED_LIST="$SHIM_DIR/docs/firefox-refused-globals.txt"
[[ -f "$REFUSED_LIST" ]] || fail "[d] missing $REFUSED_LIST"
unexpected=""
while read -r iface; do
	[[ -z "$iface" ]] && continue
	grep -qx -- "$iface" "$REFUSED_LIST" || unexpected="$unexpected $iface"
done <<<"$demands"
[[ -z "$unexpected" ]] || fail "[d] Firefox $VITRIN_FIREFOX_VERSION demanded interface(s)
that are not in the documented refusal list:$unexpected

This is the log doing its job. Decide deliberately for each one -- add it to
the v0 set in shim/src/globals.c citing the globals-demand line, or add it to
$REFUSED_LIST with the reason in shim/docs/firefox.md. Do not silence it."
n_demands="$(wc -l <<<"$demands")"
ok "$armed of $catalogue catalogue interfaces armed; $n_demands demanded, all already
    argued in docs/firefox.md:
    $(tr '\n' ' ' <<<"$demands")"

# CRITERION (d) ITSELF, MECHANIZED: "every stub addition traces to a log entry".
#
# The two globals added empirically so far each name a checked-in log as their
# evidence, in src/globals.c. That citation is prose, and prose rots: the
# original P1.6.4 citation pointed at a log that -- because the shim stops
# probing an interface the moment it joins the contract -- could not contain
# the demand line it claimed, and nothing noticed. So the citation is checked.
#
# Only wl_subcompositor is checked here: wl_data_device_manager predates the
# ledger (P1.6.3) and was argued from a reconstructed failure, which is
# recorded honestly as such in globals.c and firefox.md rather than dressed up
# as a log line that never existed.
EVIDENCE="$SHIM_DIR/docs/globals-demand-wl_subcompositor-140.12.0esr.log"
[[ -f "$EVIDENCE" ]] || fail "[d] src/globals.c cites $EVIDENCE as the evidence for
adding wl_subcompositor to the v0 set, and it is not there."
grep -q '^globals-demand: .*interface=wl_subcompositor' "$EVIDENCE" \
	|| fail "[d] $EVIDENCE is cited by src/globals.c as the demand evidence for
wl_subcompositor, but contains no 'globals-demand: ... interface=wl_subcompositor'
record. A bind is not a demand -- see ledger.h. Regenerate it with the
procedure in that file's own header, or fix the citation."
n_evidence="$(grep -c '^globals-demand: .*interface=wl_subcompositor' "$EVIDENCE")"
ok "the v0 addition made by this task cites real evidence:
    $n_evidence globals-demand line(s) for wl_subcompositor in $(basename "$EVIDENCE")"

echo
echo "PASS: all Firefox shim-in-isolation checks green (shim + real Firefox, MOCK core)"
echo "      ($reported, software WebRender, headless pixman, no network)"
echo "      NOTE: the milestone 'Shim runs Firefox' proof is the REAL-core gate,"
echo "      tests/integration/test_real_firefox.py -- this is a shim unit check."
