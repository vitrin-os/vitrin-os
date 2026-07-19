/* ledger.h -- the "globals touched" ledger (P1.6.4).
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
#include <stdint.h>
#include <stdio.h>

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

	/* `--globals-log PATH`, or NULL. Flushed after every record the run may
	 * not survive to repeat -- `globals-demand` and `globals-error`, both at
	 * WLR_ERROR -- and at teardown. Routine records ride the stdio buffer:
	 * flushing those too meant a write(2) per `wl_registry.bind`, which a
	 * client controls the rate of. */
	FILE *sink;
};

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
