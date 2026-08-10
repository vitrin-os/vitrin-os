/* ledger.c -- the "globals touched" ledger and the probe catalogue (P1.6.4).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * See ledger.h for what this is for and why the probe half has to exist at
 * all. This file is the mechanism:
 *
 *   1. a `wl_display` protocol logger, which is the one place in libwayland
 *      where every message in both directions passes through code we own;
 *   2. a client-created/destroyed listener, so "the app gave up" is a record
 *      rather than an absence of records;
 *   3. the probe globals and their generic dispatcher.
 *
 * WHY A PROTOCOL LOGGER AND NOT PER-GLOBAL BIND CALLBACKS. Almost every
 * global the shim advertises is created by wlroots, which owns its
 * `wl_global_create` bind callback; instrumenting those would mean wrapping
 * or forking wlroots per global, and a global added later would be missing
 * from the ledger until somebody remembered to wrap it too. The logger sees
 * the registry traffic itself, so it observes globals nobody instrumented,
 * including ones wlroots creates as a side effect of something else -- which
 * is exactly the class of surprise the v0 "exactly this set" contract exists
 * to catch. It also sees the version the client ASKED for, which the bind
 * callback does not: libwayland clamps before calling it.
 */
#define _POSIX_C_SOURCE 200809L

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <wayland-server-core.h>
#include <wayland-server-protocol.h>

#include <wlr/util/log.h>

#include "ledger.h"
#include "server.h"

/* Generated at configure time from whichever probe protocols the build
 * machine's wayland-protocols actually ships (shim/meson.build). It defines
 * `vitrin_probe_catalogue[]` / `vitrin_probe_catalogue_len` -- so the
 * catalogue can never claim to probe an interface this binary has no
 * marshalling table for. */
#include "probe-catalogue.h"

/* ---- record emission -------------------------------------------------- */

/* Every record goes to wlroots' log (where a human reading a shim run finds
 * it in context) and, when --globals-log is set, to a bare file (which is
 * what CI archives and the acceptance script greps).
 *
 * ONE RECORD IS ONE LINE, STRUCTURALLY. Most of what reaches this function is
 * text the shim composed, but not all of it: the interface name in a
 * `wl_registry.bind` and the body of a protocol error both originate with the
 * CONFINED APP. A client that embedded a newline in either could otherwise
 * author its own records -- and the record it would author is
 * `globals-demand`, the exact line a new global is supposed to cite, which
 * would make the evidence the v0 set is argued from attacker-controlled
 * (ledger.h). Callers validate those fields at the source; scrubbing control
 * bytes here is the backstop that makes forgery impossible rather than merely
 * unlikely. An injected payload then lands as one ugly value inside one
 * record, which is a cosmetic problem instead of an integrity one.
 *
 * FLUSHED ONLY FOR THE RECORDS A RUN MAY NOT SURVIVE TO REPEAT. `globals-demand`
 * and `globals-error` are emitted at WLR_ERROR precisely because the runs
 * worth diagnosing are the ones that die moments later, so those are flushed
 * immediately. Routine INFO/DEBUG records are not: a flush per record meant a
 * write(2) per bind, and bind rate is the client's to choose. Teardown
 * flushes and closes, so nothing is lost on any ordinary exit. */
static void record(struct vitrin_shim *s, enum wlr_log_importance level,
		const char *fmt, ...) __attribute__((format(printf, 3, 4)));
static void record(struct vitrin_shim *s, enum wlr_log_importance level,
		const char *fmt, ...) {
	char line[512];
	va_list ap;
	va_start(ap, fmt);
	vsnprintf(line, sizeof(line), fmt, ap);
	va_end(ap);

	for (char *p = line; *p != '\0'; p++) {
		unsigned char c = (unsigned char)*p;
		if (c < 0x20 || c == 0x7f) {
			*p = '.';
		}
	}

	wlr_log(level, "%s", line);
	if (s->ledger.sink != NULL) {
		fprintf(s->ledger.sink, "%s\n", line);
		if (level <= WLR_ERROR) {
			fflush(s->ledger.sink);
		}
	}
}

/* The Wayland interface-name grammar. Names come from two places: our own
 * `wl_global`s (trustworthy) and the confined app's bind requests (not), and
 * a name that fails this never becomes a row -- see on_registry_bind. */
static bool valid_interface_name(const char *name) {
	if (name == NULL || name[0] == '\0') {
		return false;
	}
	for (const char *p = name; *p != '\0'; p++) {
		bool ok = (*p >= 'a' && *p <= 'z') || (*p >= '0' && *p <= '9') ||
			(*p >= 'A' && *p <= 'Z') || *p == '_';
		if (!ok) {
			return false;
		}
	}
	return true;
}

/* ---- the row table ---------------------------------------------------- */

static struct vitrin_ledger_row *row_find(struct vitrin_ledger *l, const char *iface) {
	for (int i = 0; i < l->row_count; i++) {
		if (strcmp(l->rows[i].interface, iface) == 0) {
			return &l->rows[i];
		}
	}
	return NULL;
}

/* Find or create. Returns NULL only on overflow, which is counted and
 * reported -- a full table must degrade to "we stopped recording, and here
 * is how much we missed", never to a silent gap. */
static struct vitrin_ledger_row *row_get(struct vitrin_ledger *l, const char *iface) {
	struct vitrin_ledger_row *r = row_find(l, iface);
	if (r != NULL) {
		return r;
	}
	if (l->row_count >= VITRIN_LEDGER_MAX_ROWS) {
		l->overflow++;
		return NULL;
	}
	r = &l->rows[l->row_count++];
	memset(r, 0, sizeof(*r));
	snprintf(r->interface, sizeof(r->interface), "%s", iface);
	r->version_min = UINT32_MAX;
	return r;
}

static const char *row_class(const struct vitrin_ledger_row *r) {
	return r->probe ? "probe" : "v0";
}

/* ---- the probe dispatcher --------------------------------------------- */

/* A probe resource must be INERT BUT WELL-FORMED: it accepts every request
 * the interface defines, answers none of them, and leaves the connection in
 * a state the client can keep using. Anything less turns the survey into a
 * single-shot -- see the "inert rather than absent" argument in ledger.h.
 *
 * `wl_resource_set_dispatcher` is libwayland's generic request entry point
 * (the hook language bindings use to implement Wayland servers without a C
 * function per request). It hands us the demarshalled arguments and the
 * `wl_message` that describes them, which is everything needed to keep the
 * object graph consistent without knowing a single interface by name.
 *
 * So: give every `new_id` argument a real server-side resource inheriting
 * this same dispatcher, destroy the object when the request was a
 * destructor, absorb everything else.
 *
 * Walking the signature is the fiddly part and getting it wrong is silent:
 * a signature may OPEN WITH A SINCE-VERSION NUMBER and a nullable argument
 * is PREFIXED WITH '?', neither of which is an argument. Counting them would
 * misalign `msg->types[]` against the real arguments and mint child
 * resources of the wrong interface -- which presents as a protocol error
 * three requests later, in the client. */
static int probe_dispatch(const void *impl, void *target, uint32_t opcode,
		const struct wl_message *msg, union wl_argument *args) {
	(void)impl;
	(void)opcode;
	struct wl_resource *resource = target;
	struct wl_client *client = wl_resource_get_client(resource);

	int argi = 0;
	for (const char *p = msg->signature; *p != '\0'; p++) {
		if (*p == '?' || (*p >= '0' && *p <= '9')) {
			continue;
		}
		const struct wl_interface *child_iface =
			(*p == 'n' && msg->types != NULL) ? msg->types[argi] : NULL;
		argi++;
		if (child_iface == NULL) {
			continue;
		}
		/* A child object's version is inherited from its parent, capped by
		 * what the child's own interface defines. */
		int child_version = wl_resource_get_version(resource);
		if (child_version > child_iface->version) {
			child_version = child_iface->version;
		}
		struct wl_resource *child =
			wl_resource_create(client, child_iface, child_version, args[argi - 1].n);
		if (child == NULL) {
			wl_client_post_no_memory(client);
			return 0;
		}
		wl_resource_set_dispatcher(child, probe_dispatch, NULL, NULL, NULL);
		wlr_log(WLR_DEBUG, "globals-probe: %s.%s created inert %s@%u",
			wl_resource_get_class(resource), msg->name,
			child_iface->name, args[argi - 1].n);
	}

	/* Destructors must really destroy: the client has already retired the id
	 * on its side, and a server that never sends `delete_id` leaks it forever
	 * and will reject the client's eventual reuse of it. There is no
	 * destructor flag on `struct wl_message`, so this goes by name -- which
	 * covers every protocol in wayland-protocols, all of which spell it
	 * `destroy` or `release`. A miss costs one leaked inert resource for the
	 * life of a diagnostic run, and nothing else. */
	if (strcmp(msg->name, "destroy") == 0 || strcmp(msg->name, "release") == 0) {
		wl_resource_destroy(resource);
		return 0;
	}

	/* Deliberately not a `record()`: a probe run produces thousands of these
	 * and they answer a different question ("how far did the app get with the
	 * thing it wanted?") than the contract report. */
	wlr_log(WLR_DEBUG, "globals-probe: request %s.%s absorbed",
		wl_resource_get_class(resource), msg->name);
	return 0;
}

/* ---- probe globals ---------------------------------------------------- */

/* libwayland hands a bind callback the global's USER DATA and not the
 * interface, so a probe global carries its own `wl_interface` there -- the
 * callback would otherwise have nothing to build a resource from. The
 * catalogue entries are static const, so this needs no per-probe allocation
 * and no side table. */
static void probe_global_bind(struct wl_client *client, void *data,
		uint32_t version, uint32_t id) {
	const struct wl_interface *iface = data;
	struct wl_resource *resource = wl_resource_create(client, iface, (int)version, id);
	if (resource == NULL) {
		wl_client_post_no_memory(client);
		return;
	}
	wl_resource_set_dispatcher(resource, probe_dispatch, NULL, NULL, NULL);
}

/* Is `name` a WHOLE comma-separated element of `list`?
 *
 * Substring matching would be wrong here in a way that is easy to miss: the
 * catalogue is full of names that are prefixes of each other's relatives
 * (`wp_presentation` against a future `wp_presentation_*`,
 * `zwp_tablet_manager_v2` against the rest of the tablet family). A
 * bisection run that silently arms one probe more than it was asked for
 * would attribute an app's recovery to the wrong interface -- which is worse
 * than not bisecting at all, because it produces a confident wrong answer. */
static bool list_contains(const char *list, const char *name) {
	size_t len = strlen(name);
	for (const char *p = list; *p != '\0';) {
		const char *comma = strchr(p, ',');
		size_t seg = comma != NULL ? (size_t)(comma - p) : strlen(p);
		if (seg == len && strncmp(p, name, len) == 0) {
			return true;
		}
		if (comma == NULL) {
			break;
		}
		p = comma + 1;
	}
	return false;
}

/* The interfaces the shim REALLY implements -- the v0 contract, by name.
 *
 * It exists for one job that cannot be done from the wire: a probe must not
 * be created for an interface we already provide. Probe globals are created
 * before any client connects, so at that moment the ledger has observed no
 * `wl_registry.global` events and has no other way to know what phase B
 * advertised. Advertising an interface TWICE -- once real, once inert -- is
 * legal Wayland and a disaster for the measurement: the app binds whichever
 * name it sees first, and a probe run that hands Firefox an inert
 * `wl_subcompositor` measures a browser strictly worse than the real one.
 * (Observed, before this check existed: 3 forwarded frames instead of ~1000.)
 *
 * A hand-maintained second copy of the global set would be exactly the kind
 * of thing that drifts from globals.c, so IT IS CROSS-CHECKED AGAINST THE
 * WIRE at teardown: every interface the client was actually offered as v0
 * must be in this list, and vice versa. A mismatch is reported as
 * `globals-contract-drift`, which also catches the failure mode the "exactly
 * this set" contract exists for -- a dependency starting to create a global
 * as a side effect of something else. */
static const char *const vitrin_v0_contract[] = {
	"wl_compositor",
	"wl_subcompositor",
	"wl_shm",
	"wl_output",
	"xdg_wm_base",
	"wl_seat",
	/* The three version-2 pointer interfaces (WS-E.4.2, issue #222). They
	 * must be here as well as in globals.c, and the omission would not be
	 * quiet: `in_v0_contract` is what stops the probe catalogue advertising a
	 * SECOND, inert copy that the app could bind instead of the real one, and
	 * the teardown wire check would report `globals-contract-drift` for each. */
	"zwp_relative_pointer_manager_v1",
	"zwp_pointer_gestures_v1",
	"zwp_pointer_constraints_v1",
	"wl_data_device_manager",
	"zxdg_decoration_manager_v1",
	"zwp_linux_dmabuf_v1", /* only with --dmabuf; see is_optional below */
};

static bool in_v0_contract(const char *iface) {
	for (size_t i = 0; i < sizeof(vitrin_v0_contract) / sizeof(*vitrin_v0_contract); i++) {
		if (strcmp(vitrin_v0_contract[i], iface) == 0) {
			return true;
		}
	}
	return false;
}

/* Is `name` in the compiled-in catalogue at all? Used only to tell a
 * `--probe-globals=X` typo apart from a deliberate skip, which are the two
 * ways a bisection can arm nothing. */
static bool catalogue_has(const char *name) {
	for (int i = 0; i < vitrin_probe_catalogue_len; i++) {
		if (vitrin_probe_catalogue[i] != NULL &&
				strcmp(vitrin_probe_catalogue[i]->name, name) == 0) {
			return true;
		}
	}
	return false;
}

/* Report every `--probe-globals=` element that armed nothing, and say WHICH
 * of the two reasons applies.
 *
 * A bisection that arms nothing still produces a full report ending in
 * `demanded=0`, and `demanded=0` reads as "the app did not want this" when
 * the truth is "this was never offered, so the question was not asked". That
 * is the confident wrong answer list_contains() exists to avoid, arrived at
 * from the other direction, and it is worse than not bisecting at all. */
static void report_unmatched_filter(struct vitrin_shim *s) {
	const char *list = s->cfg.probe_filter;
	if (list == NULL) {
		return;
	}
	for (const char *p = list; *p != '\0';) {
		const char *comma = strchr(p, ',');
		size_t seg = comma != NULL ? (size_t)(comma - p) : strlen(p);
		if (seg > 0 && seg < VITRIN_LEDGER_NAME_MAX) {
			char name[VITRIN_LEDGER_NAME_MAX];
			memcpy(name, p, seg);
			name[seg] = '\0';
			if (in_v0_contract(name)) {
				wlr_log(WLR_ERROR,
					"--probe-globals=%s: '%s' IS ALREADY IN THE V0 SET and was not "
					"armed -- probing it would advertise it twice and let the app "
					"bind the inert copy. To re-test whether it is still needed, "
					"remove it from globals.c and from vitrin_v0_contract[], then "
					"probe it.", list, name);
			} else if (!catalogue_has(name)) {
				wlr_log(WLR_ERROR,
					"--probe-globals=%s: '%s' is not in this binary's probe "
					"catalogue (%d entries) -- check the spelling, or the build "
					"machine's wayland-protocols may not ship it. NOTHING was "
					"armed for it.", list, name, vitrin_probe_catalogue_len);
			}
		}
		if (comma == NULL) {
			break;
		}
		p = comma + 1;
	}
}

void vitrin_ledger_create_probes(struct vitrin_shim *s) {
	if (!s->cfg.probe_globals) {
		return;
	}
	struct vitrin_ledger *l = &s->ledger;
	for (int i = 0; i < vitrin_probe_catalogue_len && i < VITRIN_LEDGER_MAX_ROWS; i++) {
		const struct wl_interface *iface = vitrin_probe_catalogue[i];
		if (iface == NULL) {
			continue;
		}
		if (s->cfg.probe_filter != NULL &&
				!list_contains(s->cfg.probe_filter, iface->name)) {
			continue;
		}
		/* Never shadow a real global with an inert one. */
		if (in_v0_contract(iface->name)) {
			wlr_log(WLR_DEBUG,
				"probe catalogue: %s is in the v0 set; not probing it",
				iface->name);
			continue;
		}
		/* The interface doubles as the global's user data -- see
		 * probe_global_bind. Casting away const is safe: the bind callback
		 * only reads it, and libwayland's user-data slot is untyped. */
		struct wl_global *g = wl_global_create(s->display, iface, iface->version,
			(void *)(uintptr_t)iface, probe_global_bind);
		if (g == NULL) {
			wlr_log(WLR_ERROR, "probe global %s could not be created", iface->name);
			continue;
		}
		l->probes[l->probe_count++] = g;

		/* Seed the row NOW, not on first bind: a probe nobody binds is a
		 * result too ("the app did not want this"), and it can only be
		 * reported if the catalogue entry exists in the table. */
		struct vitrin_ledger_row *r = row_get(l, iface->name);
		if (r != NULL) {
			r->probe = true;
		}
	}
	/* THE BANNER BELONGS HERE, not in vitrin_ledger_init: the number that
	 * matters is how many probes were ARMED, and that is only known after the
	 * `--probe-globals=` filter and the v0-contract skip have both run.
	 * Announcing the catalogue size at init told a filtered run that 21
	 * interfaces were advertised-but-unimplemented when the real answer was
	 * sometimes zero. */
	if (l->probe_count > 0) {
		wlr_log(WLR_ERROR,
			"--probe-globals: %d interface(s) ARE ADVERTISED BUT NOT IMPLEMENTED. "
			"This is a diagnostic mode: an app that waits on one of them will hang. "
			"Never use it to run a realm.", l->probe_count);
	}
	report_unmatched_filter(s);
	if (l->probe_count == 0) {
		wlr_log(WLR_ERROR,
			"--probe-globals armed NO probes at all (catalogue=%d, filter=%s). "
			"This run measures NOTHING: every `demanded=0` below means 'not "
			"offered, so never asked', not 'the app did not want it'.",
			vitrin_probe_catalogue_len,
			s->cfg.probe_filter != NULL ? s->cfg.probe_filter : "(all)");
	}
	record(s, WLR_INFO,
		"globals-log: probes_armed=%d catalogue=%d filter=%s "
		"(these interfaces are ADVERTISED BUT NOT IMPLEMENTED)",
		l->probe_count, vitrin_probe_catalogue_len,
		s->cfg.probe_filter != NULL ? s->cfg.probe_filter : "(all)");
}

/* ---- the protocol logger ---------------------------------------------- */

/* Resolve a numeric registry name through the `global` events WE sent. This
 * is the ledger's authoritative mapping from a registry name to an interface:
 * we chose both, and we recorded them as they went out. */
static struct vitrin_ledger_row *row_by_registry_name(
		struct vitrin_ledger *l, uint32_t name) {
	for (int i = 0; i < l->row_count; i++) {
		if (l->rows[i].advertised && l->rows[i].registry_name == name) {
			return &l->rows[i];
		}
	}
	return NULL;
}

static void on_registry_global(struct vitrin_shim *s, const union wl_argument *args,
		int argc) {
	if (argc < 3 || !valid_interface_name(args[1].s)) {
		return;
	}
	struct vitrin_ledger *l = &s->ledger;
	struct vitrin_ledger_row *r = row_get(l, args[1].s);
	if (r == NULL) {
		return;
	}
	/* RE-OFFERS ARE NOT NEWS. A client may create any number of `wl_registry`
	 * objects and each one replays the entire global list, so a record per
	 * offer is a record per (global x registry) at a rate the client picks.
	 * The first offer of an interface is recorded, and so is any offer whose
	 * version or registry name CHANGED -- which would be real news, and is
	 * the case this collapse must not swallow. The rest are counted. */
	bool news = !r->advertised || r->advertised_version != args[2].u ||
		r->registry_name != args[0].u;
	r->advertised = true;
	r->advertised_version = args[2].u;
	r->registry_name = args[0].u;
	if (!news) {
		l->offers_suppressed++;
		return;
	}
	record(s, WLR_DEBUG, "globals-offer: seq=%llu interface=%s version=%u name=%u class=%s",
		(unsigned long long)++l->seq, r->interface, r->advertised_version,
		r->registry_name, row_class(r));
}

/* THE ONE PLACE THE CONFINED APP DRIVES THE LEDGER, so it is the one place
 * that has to distrust its input.
 *
 * The wire form of `wl_registry.bind` is (uint name, string interface, uint
 * version, new_id id), and that STRING IS THE CLIENT'S. Worse, libwayland
 * runs protocol loggers before it validates the request
 * (`log_closure()` precedes `wl_closure_invoke()` in wayland-server.c), so a
 * ledger built from the client's string records binds that never happen,
 * under interface names that may never have existed. Recording it verbatim
 * let a client embed a newline and mint whole records of its own -- including
 * `globals-demand`, which is the line the v0 global set is argued FROM.
 *
 * So the numeric registry name is the key and the client's string is only a
 * cross-check: a disagreement is exactly the set of binds libwayland is about
 * to reject, and it is reported from OUR side of the mismatch. The client can
 * no longer create a row, name a row, or choose what a row says. */
static void on_registry_bind(struct vitrin_shim *s, const union wl_argument *args,
		int argc) {
	struct vitrin_ledger *l = &s->ledger;
	if (argc < 2) {
		return;
	}
	uint32_t registry_name = args[0].u;
	struct vitrin_ledger_row *r = row_by_registry_name(l, registry_name);
	if (r == NULL) {
		/* A registry name we never advertised. libwayland answers this with
		 * `global %s (%u) is unavailable` and the connection dies; there is
		 * no interface here to attribute anything to. */
		return;
	}
	/* Shape B: (name, id). The requested version is unknowable from the wire,
	 * so the advertised one is reported rather than invented. */
	uint32_t version = r->advertised_version;

	/* Shape A: (name, interface, version, id) -- the expanded form, and the
	 * only one carrying the version the client ASKED for (libwayland clamps
	 * before any bind callback sees it), which is why the ledger wants it. */
	if (argc >= 4 && args[1].s != NULL) {
		if (strcmp(args[1].s, r->interface) != 0) {
			record(s, WLR_ERROR,
				"globals-bind-rejected: seq=%llu name=%u expected=%s "
				"(the client named a different interface for this registry name; "
				"libwayland is about to kill the connection over it)",
				(unsigned long long)++l->seq, registry_name, r->interface);
			return;
		}
		version = args[2].u;
	}

	r->binds++;
	l->binds_total++;
	bool first = r->binds == 1;
	bool new_version = version < r->version_min || version > r->version_max;
	if (version < r->version_min) {
		r->version_min = version;
	}
	if (version > r->version_max) {
		r->version_max = version;
	}
	uint64_t seq = ++l->seq;
	if (r->first_seq == 0) {
		r->first_seq = seq;
	}
	r->last_seq = seq;

	/* ONE RECORD PER (INTERFACE, VERSION), NOT ONE PER BIND. Rebinding a
	 * global is legal, unlimited Wayland -- a client can do it as fast as it
	 * can write -- and every record costs ~140 bytes on the stderr vitrind
	 * inherits from the shim. Measured before this collapse: 200,000 binds in
	 * 0.8 s produced 28 MB of stderr and 20 MB of ledger, roughly six times
	 * the app's own wire bytes, generated on the app's behalf by the process
	 * confining it.
	 *
	 * Nothing is lost: the row accumulates `binds`, `version_min/max` and
	 * `first_seq/last_seq`, and `globals-touched` reports all of it at
	 * teardown. What is dropped is only the Nth identical restatement, and
	 * the count of those is in `globals-summary`. */
	if (first || new_version) {
		record(s, WLR_INFO,
			"globals-bind: seq=%llu interface=%s class=%s version_requested=%u "
			"version_advertised=%u",
			(unsigned long long)seq, r->interface, row_class(r), version,
			r->advertised_version);
	} else {
		l->binds_suppressed++;
	}

	/* THE CRUX LINE. A probe bind is the app saying, on the wire, that it
	 * went looking for an interface the v0 contract does not contain. Loud,
	 * live, and at ERROR level on purpose: it is the only record that
	 * survives an app which dies moments later, and it is the line a future
	 * global addition is supposed to cite. Capped per interface, because at
	 * ERROR level on an inherited stderr the crux line is also the loudest
	 * thing a hostile app could ask us to repeat. */
	if (r->probe) {
		l->demands++;
		r->demands++;
		if (r->demands <= VITRIN_LEDGER_DEMAND_RECORDS) {
			record(s, WLR_ERROR,
				"globals-demand: seq=%llu interface=%s version_requested=%u "
				"(the app bound a PROBE global -- it wants an interface the v0 set "
				"does not provide)",
				(unsigned long long)seq, r->interface, version);
		} else {
			l->demands_suppressed++;
		}
	}
}

/* A protocol error we sent the client is the other half of "what did the app
 * do when it could not find something": if the app reacted to a missing
 * global by misusing one it did have, this is where that shows up, with the
 * message libwayland formatted. */
static void on_display_error(struct vitrin_shim *s, const union wl_argument *args,
		int argc) {
	if (argc < 3) {
		return;
	}
	/* The offending object is not printed separately: `args[0]` is a
	 * `struct wl_object *`, which is opaque outside libwayland, and every
	 * message libwayland formats for `wl_resource_post_error` already names
	 * the object ("invalid method 3, object wp_viewport@27"). Reaching into
	 * the struct to re-derive what the string already says would trade a
	 * layout assumption for nothing. */

	/* THIS MESSAGE QUOTES THE CLIENT BACK AT ITSELF. Several of libwayland's
	 * error strings interpolate a client-supplied name -- "invalid interface
	 * for global %u: expected %s, got %s" is formatted straight from the
	 * interface string in a bind request -- so this field is the widest path
	 * an app has into the log. `record()` scrubs control bytes, which is what
	 * keeps one record on one line; the double quote is handled here, because
	 * this is the only record with a quoted field and an app that could close
	 * it early would still be shaping the grammar a parser sees. */
	char msg[256];
	snprintf(msg, sizeof(msg), "%s", args[2].s != NULL ? args[2].s : "");
	for (char *p = msg; *p != '\0'; p++) {
		if (*p == '"' || *p == '\\') {
			*p = '\'';
		}
	}
	record(s, WLR_ERROR, "globals-error: seq=%llu code=%u message=\"%s\"",
		(unsigned long long)++s->ledger.seq, args[1].u, msg);
}

static void protocol_logger(void *user_data, enum wl_protocol_logger_type direction,
		const struct wl_protocol_logger_message *message) {
	struct vitrin_shim *s = user_data;
	if (message->message == NULL || message->message->name == NULL ||
			message->resource == NULL) {
		return;
	}
	const char *cls = wl_resource_get_class(message->resource);
	const char *name = message->message->name;

	if (direction == WL_PROTOCOL_LOGGER_EVENT) {
		if (strcmp(cls, "wl_registry") == 0 && strcmp(name, "global") == 0) {
			on_registry_global(s, message->arguments, message->arguments_count);
		} else if (strcmp(cls, "wl_display") == 0 && strcmp(name, "error") == 0) {
			on_display_error(s, message->arguments, message->arguments_count);
		}
		return;
	}
	if (strcmp(cls, "wl_registry") == 0 && strcmp(name, "bind") == 0) {
		on_registry_bind(s, message->arguments, message->arguments_count);
	}
}

/* ---- client lifecycle ------------------------------------------------- */

/* One slot per tracked client. Firefox is not one process talking to us --
 * the parent connects, and depending on build and prefs a GPU/utility child
 * may connect too -- so "the app disconnected" is only meaningful if each
 * connection is counted. Overflow past this simply stops tracking
 * disconnects, which costs a record and nothing else. */
#define VITRIN_LEDGER_MAX_CLIENTS 8

struct vitrin_ledger_client {
	struct wl_listener destroy;
	struct vitrin_shim *shim;
	uint64_t binds_at_connect;
	bool in_use;
};

static struct vitrin_ledger_client g_clients[VITRIN_LEDGER_MAX_CLIENTS];

static void on_client_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_ledger_client *c = wl_container_of(listener, c, destroy);
	struct vitrin_shim *s = c->shim;
	s->ledger.clients_gone++;
	record(s, WLR_INFO, "globals-client: seq=%llu event=gone clients=%llu binds=%llu",
		(unsigned long long)++s->ledger.seq,
		(unsigned long long)(s->ledger.clients_connected - s->ledger.clients_gone),
		(unsigned long long)(s->ledger.binds_total - c->binds_at_connect));
	wl_list_remove(&c->destroy.link);
	c->in_use = false;
}

static void on_client_created(struct wl_listener *listener, void *data) {
	struct vitrin_ledger *l = wl_container_of(listener, l, client_created);
	struct vitrin_shim *s = wl_container_of(l, s, ledger);
	struct wl_client *client = data;

	l->clients_connected++;
	record(s, WLR_INFO, "globals-client: seq=%llu event=connected clients=%llu binds=%llu",
		(unsigned long long)++l->seq,
		(unsigned long long)(l->clients_connected - l->clients_gone),
		(unsigned long long)l->binds_total);

	for (int i = 0; i < VITRIN_LEDGER_MAX_CLIENTS; i++) {
		if (g_clients[i].in_use) {
			continue;
		}
		g_clients[i].in_use = true;
		g_clients[i].shim = s;
		g_clients[i].binds_at_connect = l->binds_total;
		g_clients[i].destroy.notify = on_client_destroy;
		wl_client_add_destroy_listener(client, &g_clients[i].destroy);
		return;
	}
}

/* ---- lifecycle -------------------------------------------------------- */

void vitrin_ledger_init(struct vitrin_shim *s) {
	struct vitrin_ledger *l = &s->ledger;
	if (l->active) {
		return;
	}

	if (s->cfg.globals_log != NULL) {
		l->sink = fopen(s->cfg.globals_log, "w");
		if (l->sink == NULL) {
			/* Not fatal, but loud: a run asked to produce evidence and could
			 * not, and a reader must never mistake an empty file for "the app
			 * touched nothing". */
			wlr_log(WLR_ERROR, "cannot open --globals-log '%s'; "
				"the ledger will only reach the shim log", s->cfg.globals_log);
		}
	}

	l->logger = wl_display_add_protocol_logger(s->display, protocol_logger, s);
	if (l->logger == NULL) {
		wlr_log(WLR_ERROR, "cannot attach the globals ledger's protocol logger");
	}
	l->client_created.notify = on_client_created;
	wl_display_add_client_created_listener(s->display, &l->client_created);
	l->active = true;

	record(s, WLR_INFO, "globals-log: version=1 probe_mode=%d catalogue=%d view=%dx%d",
		(int)s->cfg.probe_globals, vitrin_probe_catalogue_len,
		s->cfg.width, s->cfg.height);
	/* No probe banner here: this runs before the probes are created, so the
	 * only number available is the catalogue size, which is not the number
	 * armed. vitrin_ledger_create_probes announces the real one. */
}

void vitrin_ledger_finish(struct vitrin_shim *s) {
	struct vitrin_ledger *l = &s->ledger;
	if (!l->active) {
		return;
	}

	int probes_armed = l->probe_count;
	int advertised = 0, bound = 0, untouched = 0;
	for (int i = 0; i < l->row_count; i++) {
		struct vitrin_ledger_row *r = &l->rows[i];
		if (r->advertised) {
			advertised++;
		}
		if (r->binds > 0) {
			bound++;
		} else if (r->advertised) {
			untouched++;
		}
		/* A row with no binds has version_min still at its UINT32_MAX sentinel;
		 * print 0 rather than 4294967295, which reads as a real version. */
		uint32_t vmin = r->binds > 0 ? r->version_min : 0;
		record(s, WLR_INFO,
			"globals-touched: interface=%s class=%s advertised=%u binds=%llu "
			"version_min=%u version_max=%u first_seq=%llu last_seq=%llu status=%s",
			r->interface, row_class(r), r->advertised_version,
			(unsigned long long)r->binds, vmin, r->version_max,
			(unsigned long long)r->first_seq, (unsigned long long)r->last_seq,
			r->binds > 0 ? "bound" : "untouched");
	}

	/* Cross-check the hand-written contract list against what the client was
	 * actually offered. Only meaningful once a client has connected, because
	 * the offered set is learned from `wl_registry.global` events and a run
	 * nobody connected to produced none. */
	int drift = 0;
	if (l->clients_connected > 0) {
		for (int i = 0; i < l->row_count; i++) {
			const struct vitrin_ledger_row *r = &l->rows[i];
			if (r->advertised && !r->probe && !in_v0_contract(r->interface)) {
				drift++;
				record(s, WLR_ERROR,
					"globals-contract-drift: interface=%s direction=advertised-but-not-in-contract "
					"(something created a global the v0 set does not name)",
					r->interface);
			}
		}
		for (size_t i = 0; i < sizeof(vitrin_v0_contract) / sizeof(*vitrin_v0_contract); i++) {
			const char *want = vitrin_v0_contract[i];
			/* linux-dmabuf is opt-in (D3) and is also best-effort: a software
			 * renderer cannot back it, so its absence is expected in exactly
			 * the configuration this shim usually runs in. */
			if (strcmp(want, "zwp_linux_dmabuf_v1") == 0 && !s->cfg.dmabuf) {
				continue;
			}
			const struct vitrin_ledger_row *r = row_find(l, want);
			if (r == NULL || !r->advertised || r->probe) {
				drift++;
				record(s, WLR_ERROR,
					"globals-contract-drift: interface=%s direction=in-contract-but-not-advertised",
					want);
			}
		}
	}

	record(s, WLR_INFO,
		"globals-summary: clients=%llu advertised=%d bound=%d untouched=%d "
		"demanded=%llu probe_mode=%d probes_armed=%d catalogue=%d drift=%d overflow=%d "
		"suppressed_offers=%llu suppressed_binds=%llu suppressed_demands=%llu",
		(unsigned long long)l->clients_connected, advertised, bound, untouched,
		(unsigned long long)l->demands, (int)s->cfg.probe_globals,
		probes_armed, vitrin_probe_catalogue_len, drift, l->overflow,
		(unsigned long long)l->offers_suppressed,
		(unsigned long long)l->binds_suppressed,
		(unsigned long long)l->demands_suppressed);

	if (l->logger != NULL) {
		wl_protocol_logger_destroy(l->logger);
		l->logger = NULL;
	}
	wl_list_remove(&l->client_created.link);
	/* The probe globals are display-owned and go with wl_display_destroy;
	 * removing them here would only race the teardown order. Clients still
	 * holding probe resources are destroyed by wl_display_destroy_clients. */
	l->probe_count = 0;
	if (l->sink != NULL) {
		fclose(l->sink);
		l->sink = NULL;
	}
	l->active = false;
}
