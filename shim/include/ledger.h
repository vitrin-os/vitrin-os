/* ledger.h -- the "globals touched" ledger (P1.6.4).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * A permanent diagnostic, not a debugging hack. The v0 global set is "a
 * contract, not a floor" (plan E6 / R2), which is only an honest thing to say
 * if additions to it are *driven by evidence*. This file is that evidence: it
 * records what an app was offered, what it took, at which version, and -- the
 * hard part -- what it wanted and could not find.
 *
 * The rule it exists to enforce: EVERY GLOBAL ADDED TO THE V0 SET TRACES TO A
 * LINE IN THIS LOG. `wl_data_device_manager` was added in P1.6.3 from an
 * argument reconstructed by hand after GTK failed in a confusing way; that
 * reconstruction is what this makes mechanical.
 *
 * ====================================================================
 * THE CRUX: AN INTERFACE WE NEVER ADVERTISE GENERATES NO BIND
 * ====================================================================
 *
 * Wayland discovery is push, not pull. A client learns what exists from the
 * `wl_registry.global` events the server chooses to send, and binds only
 * what it was told about. So the single most valuable fact -- "the app needed
 * X and X was not there" -- produces NO WIRE TRAFFIC AT ALL. A ledger built
 * by logging `wl_registry.bind` is structurally blind to exactly the case it
 * was built for. It can tell you what the app used; it cannot tell you what
 * the app missed, and the app's own reaction to missing it ranges from a
 * warning on stderr to a silent degradation to a SIGSEGV.
 *
 * The way to make demand observable is to make the interface bindable, so
 * this file has two halves:
 *
 *   THE BIND LEDGER (always on). A `wl_display` protocol logger watches the
 *   registry traffic in both directions: every `global` event we send (what
 *   the app was OFFERED, learned from the wire rather than from a
 *   hand-maintained list, so it cannot drift from what the code actually
 *   creates) and every `bind` request the app sends (what it TOOK, at what
 *   version). One generic hook, no per-global plumbing, so a global added
 *   anywhere in the shim is in the ledger the day it is added.
 *
 *   THE PROBE CATALOGUE (`--probe-globals`, off by default). A curated set of
 *   interfaces the shim does NOT implement, advertised anyway, purely so that
 *   an app's demand for one becomes an ordinary, observable `bind`. Binding a
 *   probe is the log line that justifies a stub: it is the app stating, on
 *   the wire, that it looked for this.
 *
 * WHY PROBE MODE IS NOT THE DEFAULT, stated plainly: advertising a global we
 * do not implement is a lie to the client. A probe resource accepts every
 * request and answers none, so an app that waits for an event from one waits
 * forever -- probe mode can hang or degrade the very app it is measuring.
 * That is an acceptable price for a diagnostic run and an unacceptable one
 * for a realm, so it is opt-in, announced at startup, and stamped into every
 * report it produces (`probe_mode=1`) so no reader can mistake a probe run's
 * output for a production run's.
 *
 * WHY THE PROBES ARE INERT RATHER THAN ABSENT. The obvious cheap trick --
 * synthesize a `wl_interface` with no methods so the bind succeeds -- kills
 * the client on its first request to it (libwayland answers an out-of-range
 * opcode with a fatal `invalid_method`), which yields exactly one datum per
 * run and a corpse. Probes therefore use the REAL generated marshalling
 * tables and `wl_resource_set_dispatcher`, libwayland's generic request
 * entry point. Every request demarshals correctly, every `new_id` argument
 * gets a real child resource, and destructors really destroy -- so the
 * connection stays well-formed and the app keeps running and keeps asking
 * for things. One run surveys the whole catalogue instead of the first
 * entry in it.
 *
 * WHAT A FUTURE READER GETS, per interface: which interface, the version it
 * was offered vs. the version range it asked for, how many times, when
 * (sequence numbers, so order is recoverable), whether it is contract or
 * probe, and whether an advertised global was never touched at all -- which
 * is the evidence for REMOVING something, the direction a "contract, not a
 * floor" also has to be able to move in.
 *
 * ====================================================================
 * RECORD GRAMMAR (v1)
 * ====================================================================
 *
 * Stable, `key=value`, one record per line, in the same style as
 * `seat-replay:` (seat.h). Written to wlroots' log always, and additionally
 * to a bare file with `--globals-log PATH` -- CI archives that file and the
 * acceptance script greps it, neither of which should have to parse around a
 * wlroots log prefix.
 *
 * THIS BLOCK IS AN INTERFACE CONTRACT FOR LOG PARSERS, so it is kept in step
 * with the emitter field for field; the acceptance script parses `drift=` and
 * `probes_armed=` out of the summary, and a grammar that omitted them would
 * invite a consumer that silently cannot see the signals that matter.
 *
 *   globals-log:     version=1 probe_mode=0|1 catalogue=N view=WxH
 *   globals-log:     probes_armed=N catalogue=N filter=NAMES|(all)
 *                    (probe runs only, emitted after the probes are created)
 *   globals-offer:   seq=N interface=NAME version=V name=REGISTRY_NAME class=v0|probe
 *   globals-bind:    seq=N interface=NAME class=v0|probe version_requested=V
 *                    version_advertised=A
 *   globals-bind-rejected: seq=N name=REGISTRY_NAME expected=NAME
 *   globals-demand:  seq=N interface=NAME version_requested=V  <- THE CRUX LINE
 *   globals-error:   seq=N code=C message="..."
 *   globals-client:  seq=N event=connected|gone clients=N binds=N
 *   globals-touched: interface=NAME class=v0|probe advertised=A binds=N
 *                    version_min=V version_max=V first_seq=N last_seq=N
 *                    status=bound|untouched
 *   globals-contract-drift: interface=NAME direction=DIR
 *   globals-summary: clients=N advertised=N bound=N untouched=N demanded=N
 *                    probe_mode=0|1 probes_armed=N catalogue=N drift=N
 *                    overflow=N suppressed_offers=N suppressed_binds=N
 *                    suppressed_demands=N
 *
 * THE DESIGNATION RELAY'S RECORDS (P2.6.7, issue #191). Appended to the same
 * grammar, and `version=1` STAYS: every record above is byte-for-byte what it
 * was, and the additions are new record NAMES a parser that does not know
 * them skips by prefix -- the Wayland-style growth rule the wire itself
 * follows (conventions §6). A version bump would tell every existing parser
 * the records it knew had changed, which is false.
 *
 *   designation-sock:    path=PATH mode=0700 tier=in-realm|host-path
 *   designation-client:  seq=N event=connected|gone|refused_occupied|wrote_and_closed
 *                        [peer_pid=N app=0|1] [bytes=N] [errno=NAME]
 *   designation-relay:   seq=N designation_id=ID kind=file|directory
 *                        mode=read|read_write name_len=N
 *                        outcome=relayed|no_client|send_failed [errno=NAME]
 *   designation-summary: relayed=N no_client=N send_failed=N clients=N
 *                        refused=N writes=N suppressed_clients=N
 *                        suppressed_refused=N suppressed_writes=N
 *
 * `tier=in-realm` iff the socket's directory is exactly `/run/vitrin`, the
 * constant the realm's mount namespace presents at `--isolation=default`;
 * `host-path` is every other spelling, and is the tier at which any process
 * of the operator's uid can connect first (limits.md).
 *
 * `peer_pid` and `app` appear on `connected` only: `app=1` iff the peer's
 * SO_PEERCRED pid is the app the shim forked, `app=0` for any other process
 * in the realm (a helper, a test client spawned by hand). `bytes` appears on
 * `wrote_and_closed` only, and is the length of the whole message the app
 * sent (received with MSG_TRUNC, so a message longer than the drain buffer
 * still reports its real size); it may be 0, since SEQPACKET permits an
 * empty message and one is still a write. `errno` on `gone` when the
 * connection ended in an error rather than an EOF, and on `send_failed`.
 *
 * `outcome=relayed` is DEFINED as: the kernel accepted the message, with its
 * one descriptor, for the held connection. Receipt by the app is
 * unobservable from the shim by design -- the socket has no reply path -- so
 * `relayed` is the strongest claim this ledger can make and a reader must not
 * promote it to "the app has it". `no_client` is a designation that arrived
 * with no connection held (closed, never queued); `send_failed` is one the
 * kernel refused for the held connection, whose errno names why (EAGAIN: the
 * app stopped reading and its kernel queue is full; EPIPE/ECONNRESET: the app
 * is gone; ETOOMANYREFS: this uid's in-flight-descriptor budget is exhausted,
 * possibly by another process), and after which the held connection is
 * dropped and the slot freed.
 *
 * THE ASYMMETRY IN WHAT IS CAPPED. `designation-client` records are APP-PACED:
 * the app chooses how often to connect, disconnect and write, so, under the
 * "no record is unbounded" rule below, at most VITRIN_LEDGER_DEMAND_RECORDS of
 * each event kind are printed per run and the rest are counted into the
 * summary's `suppressed_*` (connected and gone share `suppressed_clients`;
 * refused_occupied is `suppressed_refused`; wrote_and_closed is
 * `suppressed_writes`). `designation-relay` records are CORE-PACED -- one per
 * completed human decision in the picker -- and are UNCAPPED: the rate is
 * bounded by a human's hand, and every one of them is the evidence a reader
 * of a designation's journey needs.
 *
 * The basename the human chose is NEVER recorded (P2.6.5's rule: name LENGTH
 * only). It is display copy for a picker, and a per-realm log is not a place
 * the designation authorized it to go.
 *
 * `globals-error` carries no `object=` field: libwayland's own message text
 * already names the offending object ("invalid method 3, object wp_viewport@27")
 * and the closure's object argument is opaque outside libwayland.
 *
 * `globals-demand` is emitted LIVE, at WLR_ERROR, the instant a probe is
 * bound -- not deferred to the teardown dump. Firefox's failure mode during
 * this very task was a SIGSEGV moments after the bind that explained it, and
 * a diagnostic that only speaks at teardown is silent about precisely the
 * runs worth diagnosing. (The teardown dump still runs: the shim outlives its
 * client. The live line is belt and braces.)
 *
 * ====================================================================
 * THE RECORDED SET IS BOUNDED, AND ONE INPUT IS UNTRUSTED
 * ====================================================================
 *
 * Two properties this file's implementation has to keep, both learned the
 * hard way, both about the fact that A CONFINED APP DRIVES THIS LOG:
 *
 *   NO RECORD IS FORGEABLE. The interface name in a `wl_registry.bind` is a
 *   string the app chose, and libwayland runs the protocol logger BEFORE it
 *   validates that string. Recording it verbatim let an app embed a newline
 *   and author its own records -- including `globals-demand`, the very line
 *   a global addition is supposed to cite. Binds are therefore resolved
 *   through the registry name against the `global` events WE sent, and
 *   `record()` scrubs control bytes as a structural backstop.
 *
 *   NO RECORD IS UNBOUNDED. Rebinding a global is legal, unlimited Wayland,
 *   and so is creating registries. A record per bind made the shim emit ~6x
 *   the app's own wire bytes onto the stderr vitrind inherits (measured:
 *   200k binds -> 28 MB in 0.8 s). Repeat offers and repeat binds are
 *   therefore collapsed and counted, not printed -- the aggregate rows carry
 *   the totals, so the evidence survives and the firehose does not.
 */
#ifndef VITRIN_LEDGER_H
#define VITRIN_LEDGER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/types.h> /* pid_t (the designation socket's SO_PEERCRED peer) */

#include <wayland-server-core.h>

struct vitrin_shim;
struct wl_interface;

/* Rows are a fixed table, deliberately. A diagnostic must never be the thing
 * that fails the run: no allocation on the observation path means no
 * allocation failure on it either. 96 rows is roughly four times the largest
 * plausible union of (v0 contract + probe catalogue + anything a future
 * global adds), and an overflow is counted and reported rather than silently
 * dropping evidence. */
#define VITRIN_LEDGER_MAX_ROWS 96
/* Longest real interface name in wayland-protocols today is 46 characters
 * (`wp_color_management_surface_feedback_v1` and friends); 64 leaves room and
 * keeps the row a round size. Longer names are truncated, and truncation is
 * visible because the row still records the binds. */
#define VITRIN_LEDGER_NAME_MAX 64
/* How many `globals-demand` lines one interface may print before the rest are
 * merely counted. The line is the crux of the whole file, so the cap is not
 * 1: an app that binds a probe from several processes, or at several
 * versions, is saying something slightly different each time. But an app that
 * binds one in a loop is saying nothing new by the ninth repetition, and the
 * record is at ERROR level on the stderr vitrind inherits. */
#define VITRIN_LEDGER_DEMAND_RECORDS 8

struct vitrin_ledger_row {
	char interface[VITRIN_LEDGER_NAME_MAX];
	/* We sent the client a `wl_registry.global` for this interface. Learned
	 * from the wire, not from a list -- see the header comment. */
	bool advertised;
	/* This row is a probe: advertised only to make demand observable, backed
	 * by nothing. Seeded when the probe global is created, so it is known
	 * even for probes nobody ever binds. */
	bool probe;
	uint32_t advertised_version;
	uint32_t registry_name;
	uint64_t binds;
	/* Probe binds for this interface. Counted per row so the record cap is
	 * per interface: one chatty probe must not silence a different one. */
	uint64_t demands;
	uint32_t version_min;
	uint32_t version_max;
	uint64_t first_seq;
	uint64_t last_seq;
};

struct vitrin_ledger {
	bool active;
	struct wl_protocol_logger *logger;
	struct wl_listener client_created;

	struct vitrin_ledger_row rows[VITRIN_LEDGER_MAX_ROWS];
	int row_count;
	int overflow; /* rows that did not fit; reported, never silent */

	uint64_t seq;
	uint64_t clients_connected;
	uint64_t clients_gone;
	uint64_t binds_total;
	uint64_t demands; /* probe binds: interfaces the app wanted and we lack */

	/* Records collapsed rather than printed, reported in `globals-summary`.
	 * A suppressed record is never a lost fact -- the aggregate rows carry
	 * the totals -- but it IS a fact about the app's behaviour ("it rebound
	 * the same global 200,000 times"), so it is counted rather than hidden. */
	uint64_t offers_suppressed;
	uint64_t binds_suppressed;
	uint64_t demands_suppressed;

	/* Probe globals, so teardown can destroy them explicitly. */
	struct wl_global *probes[VITRIN_LEDGER_MAX_ROWS];
	int probe_count;

	/* The designation relay's counters (P2.6.7, issue #191; grammar in the
	 * header comment). Totals for `designation-summary`, plus the per-kind
	 * printed counts that implement the cap on the app-paced
	 * `designation-client` records. */
	uint64_t designations_relayed;
	uint64_t designations_no_client;
	uint64_t designations_send_failed;
	uint64_t designation_clients;  /* `connected` events */
	uint64_t designation_gone;     /* `gone` events */
	uint64_t designation_refused;  /* `refused_occupied` events */
	uint64_t designation_writes;   /* `wrote_and_closed` events */
	/* Records of each client event kind actually printed, indexed by
	 * `enum vitrin_ledger_designation_client_event`; the cap compares
	 * against these, and what exceeds it lands in the three below. */
	uint64_t designation_client_printed[4];
	uint64_t designation_suppressed_clients; /* connected + gone */
	uint64_t designation_suppressed_refused;
	uint64_t designation_suppressed_writes;

	/* `--globals-log PATH`, or NULL. Flushed after every record the run may
	 * not survive to repeat -- `globals-demand` and `globals-error`, both at
	 * WLR_ERROR -- and at teardown. Routine records ride the stdio buffer:
	 * flushing those too meant a write(2) per `wl_registry.bind`, which a
	 * client controls the rate of.
	 *
	 * ALSO flushed after every `designation-*` record, whatever its level.
	 * The bind-rate argument does not apply there: `designation-relay` is
	 * paced by a human's hand in the picker, and `designation-client` is
	 * capped at VITRIN_LEDGER_DEMAND_RECORDS per kind, so the most an app can
	 * extract is a few dozen write(2)s per run. What the flush buys is a
	 * ledger the acceptance script can read LIVE, sequencing "the shim relayed
	 * it" against the client's own account without waiting for teardown. */
	FILE *sink;
};

/* ---- the designation relay's emitters (P2.6.7, issue #191) ------------
 *
 * Exported so designation.c can emit records while `record()` stays static
 * and the grammar stays in one file. Each is one record, flushed; at INFO,
 * except `designation-relay ... outcome=send_failed`, which is at ERROR --
 * a designation the human made that the app will not get is the run a reader
 * diagnoses (ledger.c). */

/* The `designation-client` event kinds, in the grammar's spelling order.
 * Values are the index into `designation_client_printed[]`. */
enum vitrin_ledger_designation_client_event {
	VITRIN_LEDGER_DESIGNATION_CONNECTED = 0,
	VITRIN_LEDGER_DESIGNATION_GONE = 1,
	VITRIN_LEDGER_DESIGNATION_REFUSED_OCCUPIED = 2,
	VITRIN_LEDGER_DESIGNATION_WROTE_AND_CLOSED = 3,
};

/* The `designation-relay` outcomes. Returned by `vitrin_designation_relay`
 * as well, so the log line in upstream.c and the ledger record cannot
 * disagree about what happened. */
enum vitrin_ledger_designation_outcome {
	VITRIN_LEDGER_DESIGNATION_RELAYED,
	VITRIN_LEDGER_DESIGNATION_NO_CLIENT,
	VITRIN_LEDGER_DESIGNATION_SEND_FAILED,
};

/* `designation-sock: path=... mode=0700 tier=...`, once, from
 * `vitrin_designation_init` after the post-bind mode check passed (the record
 * states 0700 as a fact, so it is emitted only once that is one). */
void vitrin_ledger_designation_sock(struct vitrin_shim *s, const char *path, bool in_realm);

/* One `designation-client` record, capped per kind. `peer_pid`/`app` are
 * printed for `connected`, `bytes` for `wrote_and_closed`, `err` (an errno,
 * 0 for none) for `gone`; the others are ignored for the other kinds. */
void vitrin_ledger_designation_client(struct vitrin_shim *s,
	enum vitrin_ledger_designation_client_event event,
	pid_t peer_pid, bool app, size_t bytes, int err);

/* One `designation-relay` record, uncapped. `err` is the errno for
 * `send_failed` and ignored otherwise. Never takes the name: only its length. */
void vitrin_ledger_designation_relay(struct vitrin_shim *s, uint32_t designation_id,
	bool directory, bool read_write, uint32_t name_len,
	enum vitrin_ledger_designation_outcome outcome, int err);

/* The symbolic name of an errno (`EAGAIN`, `EPIPE`, ...), for `errno=NAME`
 * fields and for the shim log's send-failure suffix. Never NULL: an errno
 * without a known name renders as `E<number>`, which is still greppable and
 * still one token. */
const char *vitrin_ledger_errno_name(int err);

/* Attach the protocol logger and the client listener. Must run before the
 * Wayland socket is bound, or the first client's registry traffic -- which is
 * all of the interesting traffic -- happens unobserved. Never fatal: a shim
 * that cannot instrument itself still serves its app. */
void vitrin_ledger_init(struct vitrin_shim *s);

/* Create the probe catalogue's globals (no-op unless --probe-globals).
 * Called at the end of phase B so probes are advertised after, and are
 * visibly distinct from, the real v0 set. */
void vitrin_ledger_create_probes(struct vitrin_shim *s);

/* Dump the per-interface rows and the summary, then release everything.
 * Idempotent, and safe after a partial bring-up. */
void vitrin_ledger_finish(struct vitrin_shim *s);

#endif /* VITRIN_LEDGER_H */
