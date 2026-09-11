#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# P2.6.7 (issue #191) COMPONENT proof: the shim relays a designation to its
# app over the realm's own `designation.sock`, and holds nothing afterwards.
#
# THIS IS A COMPONENT TEST, NOT MILESTONE ACCEPTANCE. It drives the real shim
# against `tests/mock_core.c`, a hand-written stand-in for the trusted core
# (see that file's own header, and CLAUDE.md's definition-of-done rule). It
# proves what the SHIM does between `vitrin_shim_session.designation` arriving
# on fd 3 and a descriptor landing in an app; it proves nothing about
# `vitrind`'s picker, its resolver, or that any human ever chose anything. The
# mock-free rung -- real core, real picker, real app on the socket -- is
# P2.6.9's, and nothing here may be cited for it.
#
# WHAT ONLY THIS CAN SETTLE. `designation_receive.sh` (P2.6.5) proves the shim
# survives a designation and holds nothing when NO app is connected -- the
# no-client path. `test_wire_designation.c` proves the transport's ownership
# rules over a socketpair. Neither can observe the last hop, because the last
# hop is a second process: an app, spawned BY THE SHIM so it runs with the
# app's real composed environment and the app's real inherited descriptor
# table, connecting to a socket announced by nothing, and receiving over it.
# The client is `tests/designation_client.c`, a test instrument (its header
# says what that means); every arm below is a fact about the shim that is
# visible only with that process alive on the far end of the socket:
#
#   (A) THE SOCKET IS WHERE THE CONVENTION SAYS AND NOWHERE ELSE.
#       `$XDG_RUNTIME_DIR/designation.sock` exists, is a socket, is mode 0700
#       (stricter than `wayland-0`, whose node is umask-derived), and sits in
#       the runtime dir. ANNOUNCED BY NOTHING: the shim's own /proc cmdline and
#       environ contain no `designation.sock`, so an app can only find it the
#       way it finds `wayland-0`. The ledger's `designation-sock` record says
#       `tier=host-path`, because this run is not in a realm mount namespace
#       and the path is not the constant `/run/vitrin` -- the record exists so
#       a reader of a real run can tell the two apart.
#
#   (B) THE APP GOT THE FILE THE CORE OPENED. Not "a designation arrived":
#       the mock core prints the subject's `(st_dev, st_ino)` from ITS fstat
#       before sending, the client prints the pair from ITS fstat of what it
#       received, and the two are compared -- with id, kind, mode and basename
#       -- as ordered lists, one line per designation. `--pread` then reads the
#       subject's bytes back through the received descriptor. The frame is the
#       core's, verbatim: `object_id=1` and the 40-byte size for a 14-byte name
#       are what the generated decoder accepted. And the client's first line
#       says fd 3 was closed when it started, which is the spawn contract the
#       whole design leans on (a copy of the core link in the app is the
#       confinement gone).
#
#   (C) THE MODE IS THE MODE. `--write-probe` on a read-only designation is
#       EBADF -- the kernel's answer, which the shim cannot fake. That result
#       would be meaningless on its own (every broken descriptor is EBADF), so
#       the POSITIVE CONTROL runs the same probe on a `read_write` designation:
#       the write succeeds, the client reads it back, and the subject on disk
#       carries the marker afterwards.
#
#   (D) A THOUSAND RELAYS LEAVE THE TABLE THE SIZE IT WAS. The shim's
#       descriptor census -- taken through /proc WHILE the shim and the client
#       are both alive, which is the only moment a leak is visible, since exit
#       releases everything -- is the same before the first send and after the
#       thousandth receipt, zero entries resolve to the subject, and the client's
#       own table is flat too. The after-census is POLLED (bounded at 2 s):
#       the client's RX line can precede the shim's close of its own copy by
#       microseconds, and a census that raced it would fail a correct relay.
#
#   (E) THE SOCKET ACCEPTS NOTHING. An app that writes on it gets closed --
#       the client observes EOF -- and the ledger records `wrote_and_closed`
#       with the byte count. Then a SECOND designation, gated on a second
#       trigger the script creates only after the drop, lands `no_client`:
#       the slot was freed, not merely the connection forgotten.
#
#   (E2) ...AND NOTHING THE APP ATTACHES GETS INSTALLED. The same, with an
#       SCM_RIGHTS descriptor on the byte the app writes. The shim drains with
#       `read(2)`, so the kernel drops that descriptor without installing it
#       -- and the census proves it: no shim descriptor resolves to the file
#       the client attached. A `recvmsg` drain would have leaked it.
#
#   (F) ONE CONNECTION, FIRST HOLDS. A second connection sees EOF (recorded
#       `refused_occupied`), and the first still receives afterwards. Run on a
#       DIRECTORY designation so the kind comparison in (B)'s shape is not
#       vacuously `file` on every line.
#
#   (G) BACKPRESSURE FREES THE SLOT. A holder that never reads fills the
#       kernel's queue behind its connection; the shim's next send fails
#       EAGAIN, is recorded `send_failed errno=EAGAIN`, and the connection is
#       dropped, so the sends after it are `no_client`. The stalled client
#       observes the hangup without reading. This is the arm that catches a
#       relay which frees the slot on success and forgets to on failure -- and
#       the census after it is baseline minus exactly the dropped connection's
#       two descriptors. (Why so many sends: see the note at G_COUNT.)
#
#   (H) THE SUMMARY IS THE RECORDS. On every run, `designation-summary`'s
#       totals equal the per-record counts in the same ledger.
#
#   (I) THE CAP CAPS. Nine refusals in a row print exactly eight
#       `refused_occupied` records -- VITRIN_LEDGER_DEMAND_RECORDS -- and the
#       summary says `suppressed_refused=1`. App-paced records are bounded
#       (ledger.h: "no record is unbounded"); this is the one place that bound
#       is measured rather than read.
#
# HOW THE ORDERING IS SEQUENCED. The ledger (`--globals-log`) is read LIVE
# only for `designation-*` records, which ledger.c flushes on write for
# exactly this purpose; everything else is sequenced on the client's `--out`
# file (fsync per line) and the mock core's `EV` lines. The baseline census is
# taken after the client's READY line and BEFORE the mock core may send: the
# mock core waits for a trigger file (`--designate-when-exists`) the script
# creates only after the census, which turns "before the send" from a sleep
# into a fact. Every census asserts `/proc/<shim>/fd` exists at that moment
# and reads at least one entry, so a census of nothing cannot pass.
#
# Requires: vitrin-shim, mock-core, designation-client (all built by
# `meson compile -C build`). Runs fully headless and GPU-free, with no Rust
# toolchain anywhere -- the shim CI job's standing invariant.
#
# Usage:
#   BUILD_DIR=./build bash shim/tests/acceptance/designation_relay.sh
set -Eeuo pipefail

BUILD_DIR="${BUILD_DIR:-./build}"
SHIM_BIN="${SHIM_BIN:-$BUILD_DIR/vitrin-shim}"
MOCK_CORE="${MOCK_CORE:-$BUILD_DIR/mock-core}"
CLIENT="${CLIENT:-$BUILD_DIR/designation-client}"

VIEW_W="${VIEW_W:-400}"
VIEW_H="${VIEW_H:-300}"
# A BACKSTOP, not the pace: every run is ended by the script killing the mock
# core once its assertions are done, so this only bounds a run whose
# assertions never complete (and meson's own timeout bounds the whole script).
RUN_MS="${RUN_MS:-60000}"
# (D): a thousand, one millisecond apart, so the relay is measured at a rate
# no human produces and the census has something to be flat across.
BULK_COUNT="${BULK_COUNT:-1000}"
# (G): enough sends to fill the kernel's queue behind a connection that never
# reads. The bound is NOT `net.unix.max_dgram_qlen`: that check is skipped for
# a peer connected back to the sender, which a SEQPACKET connection is. The
# bound that applies is the SENDER's socket send buffer (`net.core.wmem_default`,
# 212992 bytes by default) against each queued message's in-kernel size, which
# on this kernel is a few hundred messages of 40 bytes. A thousand is above
# that with margin on any default-tuned kernel; the arm's vacuity guard (at
# least one `send_failed`) is what says whether it was enough here.
G_COUNT="${G_COUNT:-1000}"
# The ledger's per-kind cap on app-paced client records (ledger.h,
# VITRIN_LEDGER_DEMAND_RECORDS). (I) sends one more than this.
DEMAND_RECORDS=8

# Kept in step with mock_core.c's MOCK_DESIGNATION_ID by the assertions below
# failing loudly if it drifts.
FIRST_ID=4242

for bin in "$SHIM_BIN" "$MOCK_CORE" "$CLIENT"; do
	[[ -x "$bin" ]] || { echo "FAIL: missing $bin (run: meson compile -C $BUILD_DIR)" >&2; exit 1; }
done

# Force pure-software headless operation; no seat, no DRM, no GPU.
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
export WLR_LIBINPUT_NO_DEVICES=1
# The shim's log lines are at INFO; the default filters them out.
export WLR_DEBUG=0

# One directory per run, because each run's shim binds `designation.sock` in
# its runtime dir and the shim refuses (fatally, by design -- D2's stale-node
# probe) to bind over one another shim is serving. Under ${TMPDIR:-/tmp} and
# not somewhere deeper: the path has to fit an AF_UNIX `sun_path`.
TOP="$(mktemp -d "${TMPDIR:-/tmp}/vitrin-relay.XXXXXX")"
chmod 700 "$TOP"
unset WAYLAND_DISPLAY DISPLAY

CORE_PID=""
# BY PID, NEVER BY NAME. `meson test` runs the acceptance scripts in parallel,
# so a `pkill -x mock-core` here reaches into whatever OTHER test is running --
# and into a developer's unrelated shim on their own desktop. Killing the mock
# core is enough on its own: it installs a SIGTERM handler and takes its shim
# down with it (mock_core.c's shutdown ladder), and the shim takes its app --
# the lingering client -- down with it (main.c, vitrin_reap_app).
cleanup() {
	local rc=$?
	[[ -n "$CORE_PID" ]] && kill "$CORE_PID" 2>/dev/null || true
	rm -rf "$TOP"
	exit "$rc"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# --- per-run state ----------------------------------------------------------
TAG=""; DIR=""; SUBJECT=""; SUBJECT_KIND=file
CORE_LOG=""; SHIM_LOG=""; LEDGER=""; APP_OUT=""; READY=""; TRIGGER=""; TRIGGER2=""
SHIM_PID=""

dump_logs() {
	echo "core log ($CORE_LOG):"
	tail -n 30 "$CORE_LOG" 2>/dev/null || echo '(none)'
	echo "shim log ($SHIM_LOG):"
	tail -n 30 "$SHIM_LOG" 2>/dev/null || echo '(none)'
	echo "ledger ($LEDGER):"
	grep 'designation' "$LEDGER" 2>/dev/null || echo '(no designation records)'
	echo "app out ($APP_OUT):"
	tail -n 30 "$APP_OUT" 2>/dev/null || echo '(none)'
}

# wait_for SECS DESC CMD...: poll CMD ten times a second until it succeeds;
# fail with every log if it has not within SECS, or if the run ended first.
wait_for() {
	local secs="$1" desc="$2"; shift 2
	local tries=$(( secs * 10 ))
	until "$@"; do
		if [[ -n "$CORE_PID" ]] && ! kill -0 "$CORE_PID" 2>/dev/null; then
			fail "[$TAG] the run ended while waiting for $desc
$(dump_logs)"
		fi
		(( tries > 0 )) || fail "[$TAG] timeout (${secs}s) waiting for $desc
$(dump_logs)"
		tries=$(( tries - 1 ))
		sleep 0.1
	done
}

# start_run TAG [mock-core designation args...] -- [client args...]
# Starts the mock core, which spawns the shim exactly as the real core does
# (socketpair, fd 3, FD_CLOEXEC cleared), which spawns the client as its app.
# Returns once the client has connected and created its READY file, with
# SHIM_PID set and /proc/<shim>/fd verified readable. The first designation
# is gated on $TRIGGER, which nothing has created yet.
start_run() {
	TAG="$1"; shift
	DIR="$TOP/$TAG"
	mkdir -m 700 "$DIR"
	export XDG_RUNTIME_DIR="$DIR"
	CORE_LOG="$DIR/core.log"; SHIM_LOG="$DIR/shim.log"; LEDGER="$DIR/ledger.log"
	APP_OUT="$DIR/app.out"; READY="$DIR/ready"; TRIGGER="$DIR/trigger1"; TRIGGER2="$DIR/trigger2"
	# The subject: a real, named path so a /proc/PID/fd link can be resolved
	# to it, with known bytes so --pread has something to compare.
	if [[ "$SUBJECT_KIND" == dir ]]; then
		SUBJECT="$DIR/subject.d"
		mkdir "$SUBJECT"
	else
		SUBJECT="$DIR/subject.txt"
		printf 'vitrin designation subject\n' >"$SUBJECT"
	fi

	local core_args=()
	while [[ $# -gt 0 && "$1" != "--" ]]; do
		core_args+=("$1"); shift
	done
	[[ "${1:-}" == "--" ]] && shift

	"$MOCK_CORE" --size "${VIEW_W}x${VIEW_H}" --run-ms "$RUN_MS" \
		--designate "$SUBJECT" --designate-after-ms 0 \
		--designate-when-exists "$TRIGGER" "${core_args[@]}" \
		-- "$SHIM_BIN" --socket "vitrin-$TAG-$$" --globals-log "$LEDGER" \
		-- "$CLIENT" --out "$APP_OUT" --ready "$READY" "$@" \
		>"$CORE_LOG" 2>"$SHIM_LOG" &
	CORE_PID=$!

	wait_for 15 "the mock core to report the shim's pid" grep -qs '^EV spawned shim_pid=' "$CORE_LOG"
	SHIM_PID="$(sed -n 's/^EV spawned shim_pid=\([0-9]\{1,\}\).*/\1/p' "$CORE_LOG" | head -n1)"
	[[ -n "$SHIM_PID" ]] || fail "[$TAG] no shim pid in the core log"
	# The client's READY comes after its connect (and after any second
	# connections it was asked to make), so this is the steady state every
	# baseline is taken in. A CONNECT_FAILED line means the shim did not serve
	# the socket the way the app expects to find it; fail on it at once rather
	# than at the timeout.
	wait_for 15 "the client's READY line" bash -c \
		'grep -qs "^READY " "$1" || { grep -qs "^CONNECT_FAILED\|^READY_FAILED\|^FAIL " "$1" && exit 0; exit 1; }' _ "$APP_OUT"
	grep -q '^READY ' "$APP_OUT" || fail "[$TAG] the client could not connect or signal readiness:
$(dump_logs)"
	[[ -d "/proc/$SHIM_PID/fd" ]] || fail "[$TAG] no /proc/$SHIM_PID/fd, so no census below \
would inspect anything. This test needs a readable /proc for the process it spawned."
	# THE FRAME PATH'S STEADY STATE, before any census. The shim allocates its
	# upstream frame memfd LAZILY, on the first output frame it forwards
	# (upstream.c, pool_acquire -> buffer_alloc), and with no Wayland client
	# there is exactly one such frame -- the first composite, after which
	# nothing is damaged and nothing more is forwarded. That frame fires on
	# the headless backend's own clock, independent of the relay, so a
	# baseline taken before it and an after-census taken after it differ by
	# one `/memfd:vitrin-shim-frame` entry that has nothing to do with a
	# designation. Observed under a loaded `meson test`, where the client
	# connected before the output's first frame. Waiting for the core's
	# `frame_done` -- the end of that one round trip -- is what makes the
	# census a statement about the relay and nothing else.
	wait_for 15 "the shim's first frame round trip" grep -qs '^EV frame_done ' "$CORE_LOG"
}

# End the run: SIGTERM the mock core, which closes the core link; the shim
# exits on that EOF, reaps its app and writes the ledger's summary. Then the
# core log is checked for wire violations and (H) is asserted on the ledger.
end_run() {
	kill -TERM "$CORE_PID" 2>/dev/null || true
	wait "$CORE_PID" || true
	CORE_PID=""
	if grep -q '^FAIL' "$CORE_LOG"; then
		fail "[$TAG] the mock core reported failures:
$(grep '^FAIL' "$CORE_LOG")"
	fi
	grep -q '^EV shim_eof' "$CORE_LOG" \
		&& fail "[$TAG] the core saw the shim's connection close during the run:
$(dump_logs)"
	assert_summary
}

# --- the census -------------------------------------------------------------
# One line per /proc/<pid>/fd entry: "N<TAB>target". `-L`, not `-e`: a socket
# or an epoll descriptor resolves to `socket:[ino]` / `anon_inode:...`, which
# no path test would pass, and those are exactly the entries this test is
# about.
snapshot_fds() {
	local pid="$1" out="$2" link
	: >"$out"
	for link in "/proc/$pid/fd/"*; do
		[[ -L "$link" ]] || continue
		printf '%s\t%s\n' "${link##*/}" "$(readlink "$link" 2>/dev/null || echo '?')" >>"$out"
	done
}
census_count() { grep -c . "$1" || true; }
census_links_to() { awk -F'\t' -v t="$2" '$2 == t' "$1" | grep -c . || true; }

# The inode of the shim's end of the held connection, from /proc/net/unix: an
# accepted unix socket carries its listener's bound path, so the CONNECTED
# (St=03) entry naming `designation.sock` is the held connection and nothing
# else can be. This is the vacuity guard for every census: a `socket:[ino]`
# link the shim holds on that inode is proof the census looked at the process
# that holds the app's connection, and not at some other table.
held_conn_inode() {
	awk -v p="$XDG_RUNTIME_DIR/designation.sock" '$8 == p && $6 == "03" { print $7 }' /proc/net/unix
}

take_baseline() {
	[[ -d "/proc/$SHIM_PID/fd" ]] || fail "[$TAG] /proc/$SHIM_PID/fd vanished before the baseline census"
	BASELINE="$DIR/census.baseline"
	snapshot_fds "$SHIM_PID" "$BASELINE"
	BASE_N="$(census_count "$BASELINE")"
	(( BASE_N > 0 )) || fail "[$TAG] the baseline census read no entries from /proc/$SHIM_PID/fd"
	CONN_INO="$(held_conn_inode)"
	[[ -n "$CONN_INO" ]] || fail "[$TAG] /proc/net/unix lists no connected socket on \
$XDG_RUNTIME_DIR/designation.sock, so no census could be checked against the held connection"
	[[ "$(printf '%s\n' "$CONN_INO" | grep -c .)" -eq 1 ]] \
		|| fail "[$TAG] more than one connected socket on designation.sock: $CONN_INO"
	grep -q "socket:\[$CONN_INO\]" "$BASELINE" \
		|| fail "[$TAG] the shim's table holds no descriptor on the held connection (inode $CONN_INO); \
the baseline census is not looking at the shim that holds the app's connection:
$(cat "$BASELINE")"
}

# census_settles EXPECTED_N: poll (2 s) until the shim's table has EXPECTED_N
# entries, then assert no entry resolves to the subject.
census_settles() {
	local expected="$1"
	[[ -d "/proc/$SHIM_PID/fd" ]] || fail "[$TAG] /proc/$SHIM_PID/fd vanished before the after-census"
	AFTER="$DIR/census.after"
	local tries=20
	while :; do
		snapshot_fds "$SHIM_PID" "$AFTER"
		[[ "$(census_count "$AFTER")" -eq "$expected" ]] && break
		(( tries > 0 )) || fail "[$TAG] the shim's descriptor table did not return to $expected \
entries within 2 s (has $(census_count "$AFTER")); a descriptor it did not close:
$(diff --label baseline --label after "$BASELINE" "$AFTER" || true)"
		tries=$(( tries - 1 ))
		sleep 0.1
	done
	kill -0 "$SHIM_PID" 2>/dev/null || fail "[$TAG] the shim exited under the census"
	local held
	held="$(census_links_to "$AFTER" "$SUBJECT")"
	(( held == 0 )) || fail "[$TAG] the shim is holding $held descriptor(s) on $SUBJECT after \
relaying; the handler's copy must be closed the instant the handler returns:
$(cat "$AFTER")"
}

# --- (H) --------------------------------------------------------------------
summary_field() { sed -n 's/.*[[:space:]]'"$1"'=\([0-9]*\).*/\1/p' <<<"$(grep '^designation-summary:' "$LEDGER")"; }
count_records() { grep -c "$1" "$LEDGER" || true; }

assert_summary() {
	grep -q '^designation-summary:' "$LEDGER" \
		|| fail "[$TAG] no designation-summary in the ledger; the shim did not reach its teardown:
$(dump_logs)"
	local f
	for f in relayed no_client send_failed clients refused writes suppressed_clients suppressed_refused suppressed_writes; do
		[[ -n "$(summary_field "$f")" ]] || fail "[$TAG] designation-summary lacks $f=:
$(grep '^designation-summary:' "$LEDGER")"
	done
	[[ "$(summary_field relayed)" -eq "$(count_records 'designation-relay: .* outcome=relayed$')" ]] \
		|| fail "[$TAG] summary relayed=$(summary_field relayed) but the ledger has $(count_records 'outcome=relayed$') relayed records"
	[[ "$(summary_field no_client)" -eq "$(count_records 'designation-relay: .* outcome=no_client$')" ]] \
		|| fail "[$TAG] summary no_client=$(summary_field no_client) but the ledger has $(count_records 'outcome=no_client$') no_client records"
	[[ "$(summary_field send_failed)" -eq "$(count_records 'designation-relay: .* outcome=send_failed errno=')" ]] \
		|| fail "[$TAG] summary send_failed=$(summary_field send_failed) but the ledger has $(count_records 'outcome=send_failed') send_failed records"
	# Client records are capped per kind: printed + suppressed is the count.
	# `connected` and `gone` share `suppressed_clients`; no run here connects
	# more than once, so that shared field must be zero and `clients` must be
	# exactly the printed `connected` records.
	[[ "$(summary_field suppressed_clients)" -eq 0 ]] \
		|| fail "[$TAG] suppressed_clients=$(summary_field suppressed_clients) in a run with one connection"
	[[ "$(summary_field clients)" -eq "$(count_records 'designation-client: .* event=connected ')" ]] \
		|| fail "[$TAG] summary clients=$(summary_field clients) but the ledger has $(count_records 'event=connected ') connected records"
	[[ "$(summary_field refused)" -eq $(( $(count_records 'event=refused_occupied$') + $(summary_field suppressed_refused) )) ]] \
		|| fail "[$TAG] summary refused=$(summary_field refused) but printed refused_occupied ($(count_records 'event=refused_occupied$')) + suppressed_refused ($(summary_field suppressed_refused)) disagree"
	[[ "$(summary_field writes)" -eq $(( $(count_records 'event=wrote_and_closed bytes=') + $(summary_field suppressed_writes) )) ]] \
		|| fail "[$TAG] summary writes=$(summary_field writes) but printed wrote_and_closed ($(count_records 'event=wrote_and_closed')) + suppressed_writes ($(summary_field suppressed_writes)) disagree"
	ok "[$TAG] (H) designation-summary equals the per-record counts"
}

# --- (B)'s shape: the core's EV lines against the client's RX lines ---------
# Ordered, not sorted: one connection over SEQPACKET delivers in order, so a
# reordering would be a finding too.
assert_delivery_matches() {
	local want="$1"
	local ev rx
	ev="$(sed -n 's/^EV designation \(designation_id=.* st_ino=[0-9]*\)$/\1/p' "$CORE_LOG")"
	rx="$(sed -n 's/^RX n=[0-9]* \(designation_id=.* st_ino=[0-9]*\) frame_bytes=.*/\1/p' "$APP_OUT")"
	[[ "$(printf '%s\n' "$ev" | grep -c .)" -eq "$want" ]] \
		|| fail "[$TAG] the mock core printed $(printf '%s\n' "$ev" | grep -c .) EV designation lines, not $want:
$(grep '^EV designation' "$CORE_LOG" | head -n 5)"
	[[ "$(printf '%s\n' "$rx" | grep -c .)" -eq "$want" ]] \
		|| fail "[$TAG] the client printed $(printf '%s\n' "$rx" | grep -c .) RX lines, not $want:
$(dump_logs)"
	if [[ "$ev" != "$rx" ]]; then
		fail "[$TAG] what the app received is not what the core designated:
$(diff --label 'core EV' --label 'client RX' <(printf '%s\n' "$ev") <(printf '%s\n' "$rx") || true)"
	fi
	grep -q "^RX n=$want " "$APP_OUT" || fail "[$TAG] no RX n=$want line"
	# Every frame the client decoded is the core's frame verbatim: the header
	# the decoder validated says object_id 1 (the session), and 40 bytes is
	# exactly 8 + 3x4 + (4 + 14 padded to 16) for the 14-byte basename.
	local bad
	bad="$(grep '^RX ' "$APP_OUT" | grep -vc 'frame_bytes=40 object_id=1$' || true)"
	(( bad == 0 )) || fail "[$TAG] $bad RX line(s) are not a 40-byte frame on object 1:
$(grep '^RX ' "$APP_OUT" | grep -v 'frame_bytes=40 object_id=1$' | head -n 3)"
	grep -q '^START .*fd3=closed ' "$APP_OUT" \
		|| fail "[$TAG] the client did not find fd 3 closed at start -- the core link leaked into the app:
$(grep '^START' "$APP_OUT" || echo '(no START line)')"
}

assert_relayed_records() {
	local want="$1" kind="$2" mode="$3"
	local n
	n="$(count_records "designation-relay: .* kind=$kind mode=$mode name_len=14 outcome=relayed$")"
	[[ "$n" -eq "$want" ]] || fail "[$TAG] expected $want relayed records (kind=$kind mode=$mode), ledger has $n:
$(grep 'designation-relay' "$LEDGER" | head -n 5)"
	n="$(grep -c "and was relayed to the connected app" "$SHIM_LOG" || true)"
	[[ "$n" -eq "$want" ]] || fail "[$TAG] the shim log names the relayed disposition $n times, not $want"
	grep -q "^designation-relay: seq=[0-9]* designation_id=$FIRST_ID " "$LEDGER" \
		|| fail "[$TAG] the first relay record does not carry designation_id=$FIRST_ID (mock_core.c's MOCK_DESIGNATION_ID drifted?)"
}

subject_hex() { od -An -tx1 -v "$1" | tr -d ' \n'; }

# =============================================================================
echo "== (A)+(B)+(C-) one read-only file designation, pread and write-probe =="
start_run relay -- --expect 1 --pread --write-probe

# --- (A) ------------------------------------------------------------------
SOCK="$XDG_RUNTIME_DIR/designation.sock"
[[ -S "$SOCK" ]] || fail "[$TAG] $SOCK is not a socket (or does not exist)"
MODE="$(stat -c %a "$SOCK")"
[[ "$MODE" == "700" ]] || fail "[$TAG] $SOCK is mode $MODE, not 700"
[[ "$(dirname "$SOCK")" == "$XDG_RUNTIME_DIR" ]] || fail "[$TAG] the socket is not in the runtime dir"
grep -q "^designation-sock: path=$SOCK mode=0700 tier=host-path$" "$LEDGER" \
	|| fail "[$TAG] no designation-sock record for $SOCK at tier=host-path:
$(grep 'designation-sock' "$LEDGER" || echo '(none)')"
CMDLINE="$(tr '\0' ' ' <"/proc/$SHIM_PID/cmdline")"
ENVIRON="$(tr '\0' '\n' <"/proc/$SHIM_PID/environ")"
grep -q -- '--socket' <<<"$CMDLINE" || fail "[$TAG] read no --socket in the shim's cmdline; the check below inspected nothing"
grep -q '^XDG_RUNTIME_DIR=' <<<"$ENVIRON" || fail "[$TAG] read no XDG_RUNTIME_DIR in the shim's environ; the check below inspected nothing"
grep -q 'designation.sock' <<<"$CMDLINE" && fail "[$TAG] the shim's cmdline names designation.sock; the path is meant to be announced by nothing"
grep -q 'designation.sock' <<<"$ENVIRON" && fail "[$TAG] the shim's environ names designation.sock; the path is meant to be announced by nothing"
grep -q "^CONNECTED path=$SOCK$" "$APP_OUT" || fail "[$TAG] the client did not connect to $SOCK by convention:
$(head -n 3 "$APP_OUT")"
ok "[$TAG] (A) $SOCK: socket, mode 0700, in the runtime dir, announced by nothing, recorded tier=host-path"

# --- (B) ------------------------------------------------------------------
take_baseline
touch "$TRIGGER"
wait_for 15 "the client's DONE line" grep -qs '^DONE n=1 ' "$APP_OUT"
assert_delivery_matches 1
EXPECT_HEX="$(subject_hex "$SUBJECT")"
grep -q "^PREAD n=1 bytes=$(stat -c %s "$SUBJECT") hex=$EXPECT_HEX$" "$APP_OUT" \
	|| fail "[$TAG] pread through the received descriptor did not return the subject's bytes:
$(grep '^PREAD' "$APP_OUT" || echo '(no PREAD line)')"
assert_relayed_records 1 file read
census_settles "$BASE_N"
grep -q "socket:\[$CONN_INO\]" "$AFTER" || fail "[$TAG] the held connection (inode $CONN_INO) is gone from the after-census"
ok "[$TAG] (B) the app received designation $FIRST_ID: same (st_dev, st_ino) the core opened, bytes readable, frame verbatim, fd 3 closed, ledger and log say relayed; census flat at $BASE_N"

# --- (C), the negative half -------------------------------------------------
grep -q '^WRITE_PROBE n=1 result=EBADF$' "$APP_OUT" \
	|| fail "[$TAG] a write on a read-only designation was not refused EBADF:
$(grep '^WRITE_PROBE' "$APP_OUT" || echo '(no WRITE_PROBE line)')"
head -c 14 "$SUBJECT" | grep -q 'WRITTEN-BY-APP' && fail "[$TAG] the read-only subject was written to"
end_run
ok "[$TAG] (C-) write on a read-only designation: EBADF, subject untouched"

# =============================================================================
echo "== (C+) the positive control: a read_write designation is writable =="
start_run rw --designate-mode read_write -- --expect 1 --pread --write-probe
take_baseline
touch "$TRIGGER"
wait_for 15 "the client's DONE line" grep -qs '^DONE n=1 ' "$APP_OUT"
assert_delivery_matches 1
grep -q '^WRITE_PROBE n=1 result=ok bytes=14$' "$APP_OUT" \
	|| fail "[$TAG] the write on a read_write designation did not succeed:
$(grep '^WRITE_PROBE' "$APP_OUT" || echo '(no WRITE_PROBE line)')"
MARKER_HEX="$(printf 'WRITTEN-BY-APP' | od -An -tx1 -v | tr -d ' \n')"
grep -q "^PREAD n=1 bytes=[0-9]* hex=$MARKER_HEX" "$APP_OUT" \
	|| fail "[$TAG] pread after the write did not read the marker back:
$(grep '^PREAD' "$APP_OUT" || echo '(no PREAD line)')"
[[ "$(head -c 14 "$SUBJECT")" == "WRITTEN-BY-APP" ]] \
	|| fail "[$TAG] the subject on disk does not carry the app's marker: $(head -c 32 "$SUBJECT")"
assert_relayed_records 1 file read_write
grep -q "designation $FIRST_ID arrived (file, read-write, name 14 bytes) and was relayed" "$SHIM_LOG" \
	|| fail "[$TAG] the shim log does not name the read-write mode"
census_settles "$BASE_N"
end_run
ok "[$TAG] (C+) write on a read_write designation succeeds, reads back, and reached the disk"

# =============================================================================
echo "== (D) $BULK_COUNT designations at 1 ms: census flat =="
start_run bulk --designate-count "$BULK_COUNT" --designate-interval-ms 1 -- --expect "$BULK_COUNT"
take_baseline
touch "$TRIGGER"
wait_for 45 "the client's DONE n=$BULK_COUNT line" grep -qs "^DONE n=$BULK_COUNT " "$APP_OUT"
assert_delivery_matches "$BULK_COUNT"
assert_relayed_records "$BULK_COUNT" file read
census_settles "$BASE_N"
grep -q "socket:\[$CONN_INO\]" "$AFTER" || fail "[$TAG] the held connection (inode $CONN_INO) is gone from the after-census"
# The client's own table: START's count plus the evidence file plus the
# socket, however many descriptors passed through it.
START_FDS="$(sed -n 's/^START .* open_fds=\([0-9]*\)$/\1/p' "$APP_OUT")"
DONE_FDS="$(sed -n 's/^DONE n=[0-9]* open_fds=\([0-9]*\)$/\1/p' "$APP_OUT")"
[[ -n "$START_FDS" && -n "$DONE_FDS" ]] || fail "[$TAG] the client did not report its descriptor counts"
[[ "$DONE_FDS" -eq $(( START_FDS + 2 )) ]] \
	|| fail "[$TAG] the client's table grew from $START_FDS to $DONE_FDS over $BULK_COUNT designations (expected +2: its evidence file and its socket)"
end_run
ok "[$TAG] (D) $BULK_COUNT designations relayed in order; shim census flat at $BASE_N, client table flat"

# =============================================================================
echo "== (E) an app that writes on the socket is closed; the next designation is no_client =="
start_run garbage --designate-count 2 --designate-when-exists "$TOP/garbage/trigger2" -- --expect 1 --send-garbage
take_baseline
touch "$TRIGGER"
wait_for 15 "the client's DONE line" grep -qs '^DONE n=1 ' "$APP_OUT"
assert_delivery_matches 1
grep -q '^GARBAGE wrote=13 then=eof$' "$APP_OUT" \
	|| fail "[$TAG] the client wrote on the socket and did not observe EOF:
$(grep '^GARBAGE' "$APP_OUT" || echo '(no GARBAGE line)')"
wait_for 5 "the ledger's wrote_and_closed record" grep -qs '^designation-client: seq=[0-9]* event=wrote_and_closed bytes=13$' "$LEDGER"
# The drop freed exactly the connection and its event-loop dup: two entries,
# and the held inode is gone.
census_settles $(( BASE_N - 2 ))
grep -q "socket:\[$CONN_INO\]" "$AFTER" && fail "[$TAG] the shim still holds the dropped connection (inode $CONN_INO)"
DROPPED_N="$(census_count "$AFTER")"
touch "$TRIGGER2"
wait_for 15 "the second designation's no_client record" grep -qs "^designation-relay: seq=[0-9]* designation_id=$(( FIRST_ID + 1 )) kind=file mode=read name_len=14 outcome=no_client$" "$LEDGER"
grep -q "designation $(( FIRST_ID + 1 )) arrived (file, read-only, name 14 bytes) and was CLOSED, not relayed: no app is connected" "$SHIM_LOG" \
	|| fail "[$TAG] the shim log does not name the no-client disposition for the second designation"
[[ "$(grep -c '^EV designation ' "$CORE_LOG")" -eq 2 ]] || fail "[$TAG] the mock core did not send exactly two designations"
census_settles "$DROPPED_N"
end_run
ok "[$TAG] (E) write of 13 bytes: EOF to the app, wrote_and_closed recorded, slot freed, next designation no_client; census flat"

# =============================================================================
echo "== (E2) ...and a descriptor attached to that write is never installed =="
start_run garbagefd --designate-count 2 --designate-when-exists "$TOP/garbagefd/trigger2" -- --expect 1 --send-garbage-fd
take_baseline
touch "$TRIGGER"
wait_for 15 "the client's DONE line" grep -qs '^DONE n=1 ' "$APP_OUT"
assert_delivery_matches 1
ATTACHED="$(sed -n 's/^GARBAGE_FD path=\(.*\) wrote=1 then=eof$/\1/p' "$APP_OUT")"
[[ -n "$ATTACHED" ]] || fail "[$TAG] the client did not send a byte with a descriptor attached and observe EOF:
$(grep '^GARBAGE_FD' "$APP_OUT" || echo '(no GARBAGE_FD line)')"
[[ -e "$ATTACHED" ]] || fail "[$TAG] the attached file $ATTACHED does not exist, so a link to it could not be found anyway"
wait_for 5 "the ledger's wrote_and_closed record" grep -qs '^designation-client: seq=[0-9]* event=wrote_and_closed bytes=1$' "$LEDGER"
census_settles $(( BASE_N - 2 ))
grep -q "socket:\[$CONN_INO\]" "$AFTER" && fail "[$TAG] the shim still holds the dropped connection (inode $CONN_INO)"
HELD_ATTACHED="$(census_links_to "$AFTER" "$ATTACHED")"
(( HELD_ATTACHED == 0 )) || fail "[$TAG] the shim's table holds $HELD_ATTACHED descriptor(s) on the file the app attached ($ATTACHED): the drain installed an app-supplied descriptor"
DROPPED_N="$(census_count "$AFTER")"
touch "$TRIGGER2"
wait_for 15 "the second designation's no_client record" grep -qs "^designation-relay: seq=[0-9]* designation_id=$(( FIRST_ID + 1 )) .* outcome=no_client$" "$LEDGER"
census_settles "$DROPPED_N"
end_run
ok "[$TAG] (E2) an SCM_RIGHTS descriptor on the app's write was dropped by the kernel, not installed in the shim; census flat"

# =============================================================================
echo "== (F) a second connection sees EOF; the first still receives (directory designation) =="
SUBJECT_KIND=dir
start_run second --designate-kind dir -- --expect 1 --second-connect
SUBJECT_KIND=file
grep -q '^SECOND_CONNECT i=1 result=eof$' "$APP_OUT" \
	|| fail "[$TAG] the second connection did not see EOF:
$(grep '^SECOND_CONNECT' "$APP_OUT" || echo '(no SECOND_CONNECT line)')"
wait_for 5 "the ledger's refused_occupied record" grep -qs '^designation-client: seq=[0-9]* event=refused_occupied$' "$LEDGER"
[[ "$(count_records 'event=refused_occupied$')" -eq 1 ]] || fail "[$TAG] expected one refused_occupied record"
take_baseline
touch "$TRIGGER"
wait_for 15 "the client's DONE line" grep -qs '^DONE n=1 ' "$APP_OUT"
assert_delivery_matches 1
grep -q "^RX n=1 designation_id=$FIRST_ID kind=directory mode=read " "$APP_OUT" \
	|| fail "[$TAG] the client did not decode kind=directory"
assert_relayed_records 1 directory read
grep -q "designation $FIRST_ID arrived (directory subtree, read-only, name 14 bytes) and was relayed" "$SHIM_LOG" \
	|| fail "[$TAG] the shim log does not name the directory kind"
census_settles "$BASE_N"
grep -q "socket:\[$CONN_INO\]" "$AFTER" || fail "[$TAG] the held connection (inode $CONN_INO) is gone from the after-census"
end_run
ok "[$TAG] (F) second connection refused with EOF and recorded; the first connection received a directory designation"

# =============================================================================
echo "== (G) a holder that never reads: send fails EAGAIN, slot freed, then no_client =="
start_run stall --designate-count "$G_COUNT" -- --stall
take_baseline
touch "$TRIGGER"
wait_for 30 "all $G_COUNT relay records" bash -c '[[ "$(grep -c "^designation-relay: " "$1" || true)" -ge "$2" ]]' _ "$LEDGER" "$G_COUNT"
[[ "$(count_records '^designation-relay: ')" -eq "$G_COUNT" ]] || fail "[$TAG] more relay records than designations"
N_RELAYED="$(count_records 'outcome=relayed$')"
N_FAILED="$(count_records 'outcome=send_failed errno=EAGAIN$')"
N_NOCLIENT="$(count_records 'outcome=no_client$')"
# Vacuity: the queue filled at all. If it did not, G_COUNT is below this
# kernel's send-buffer bound and the arm measured nothing.
(( N_FAILED >= 1 )) || fail "[$TAG] no send_failed errno=EAGAIN after $G_COUNT sends to a holder that never reads ($N_RELAYED relayed); raise G_COUNT above this kernel's send-buffer bound (net.core.wmem_default=$(cat /proc/sys/net/core/wmem_default 2>/dev/null || echo '?') bytes):
$(grep 'designation-relay' "$LEDGER" | tail -n 3)"
(( N_FAILED == 1 )) || fail "[$TAG] $N_FAILED send_failed records; the connection must be dropped on the first"
(( N_NOCLIENT >= 1 )) || fail "[$TAG] no no_client record after the drop; the slot was not freed"
(( N_RELAYED + N_FAILED + N_NOCLIENT == G_COUNT )) || fail "[$TAG] $N_RELAYED + $N_FAILED + $N_NOCLIENT != $G_COUNT"
FAIL_LINE="$(grep -n 'outcome=send_failed errno=EAGAIN$' "$LEDGER" | head -n1 | cut -d: -f1)"
[[ "$(grep -n 'outcome=relayed$' "$LEDGER" | tail -n1 | cut -d: -f1)" -lt "$FAIL_LINE" ]] \
	|| fail "[$TAG] a relayed record follows the send failure; the dropped slot was reused without a connection"
[[ "$(grep -n 'outcome=no_client$' "$LEDGER" | head -n1 | cut -d: -f1)" -gt "$FAIL_LINE" ]] \
	|| fail "[$TAG] a no_client record precedes the send failure"
grep -q "and was CLOSED, not relayed: the send failed (EAGAIN)" "$SHIM_LOG" \
	|| fail "[$TAG] the shim log does not name the send-failed disposition with the errno"
wait_for 5 "the stalled client to observe the hangup" grep -qs '^STALL closed=1 ' "$APP_OUT"
census_settles $(( BASE_N - 2 ))
grep -q "socket:\[$CONN_INO\]" "$AFTER" && fail "[$TAG] the shim still holds the dropped connection (inode $CONN_INO)"
end_run
ok "[$TAG] (G) $N_RELAYED queued behind a holder that never reads, then send_failed EAGAIN, slot freed, $N_NOCLIENT no_client; census flat"

# =============================================================================
echo "== (I) $(( DEMAND_RECORDS + 1 )) refusals: $DEMAND_RECORDS records printed, one suppressed =="
start_run refusals -- --expect 1 --second-connect-loop $(( DEMAND_RECORDS + 1 ))
[[ "$(grep -c '^SECOND_CONNECT i=[0-9]* result=eof$' "$APP_OUT")" -eq $(( DEMAND_RECORDS + 1 )) ]] \
	|| fail "[$TAG] not every second connection saw EOF:
$(grep '^SECOND_CONNECT' "$APP_OUT")"
wait_for 5 "$DEMAND_RECORDS refused_occupied records" bash -c '[[ "$(grep -c "event=refused_occupied$" "$1" || true)" -ge "$2" ]]' _ "$LEDGER" "$DEMAND_RECORDS"
take_baseline
touch "$TRIGGER"
wait_for 15 "the client's DONE line" grep -qs '^DONE n=1 ' "$APP_OUT"
assert_delivery_matches 1
census_settles "$BASE_N"
end_run
[[ "$(count_records 'event=refused_occupied$')" -eq "$DEMAND_RECORDS" ]] \
	|| fail "[$TAG] $(count_records 'event=refused_occupied$') refused_occupied records printed, not $DEMAND_RECORDS"
[[ "$(summary_field refused)" -eq $(( DEMAND_RECORDS + 1 )) ]] || fail "[$TAG] summary refused=$(summary_field refused)"
[[ "$(summary_field suppressed_refused)" -eq 1 ]] || fail "[$TAG] summary suppressed_refused=$(summary_field suppressed_refused), not 1"
ok "[$TAG] (I) $(( DEMAND_RECORDS + 1 )) refusals: exactly $DEMAND_RECORDS records printed, suppressed_refused=1"

echo "PASS: shim/tests/acceptance/designation_relay.sh (COMPONENT test against mock_core.c)"
