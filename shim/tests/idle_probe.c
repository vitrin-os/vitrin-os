/* idle_probe.c -- the app side of D-042 (issue #306): a Wayland client that
 * binds `zwp_idle_inhibit_manager_v1` and holds an inhibitor, so the shim's
 * relay can be driven at all.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY A CLIENT IS THE ONLY WAY TO DRIVE THIS. An idle inhibit is REQUESTED by
 * the app. Nothing on the core side and no injector line can produce one, which
 * is the same reason `gesture_probe.c` had to exist for a pointer lock. There is
 * no other program in this tree that binds this global.
 *
 * WHY A SEPARATE PROGRAM, AND NOT gesture-probe. That client is the WS-E.4.2
 * witness and is asserted against by an integration gate and a runbook; the two
 * facts here are about a different global and, more importantly, about a
 * different LIFETIME question. Bolting a mode onto it would couple two gates
 * that fail for unrelated reasons.
 *
 * WHY THERE IS NO xdg_toplevel HERE. An inhibitor is created FOR A wl_surface,
 * and nothing in the protocol requires that surface to be mapped -- wlroots
 * resolves it straight from the resource. The shim's own upstream surface
 * (object 2) exists from bring-up, before any app maps anything, so the ask it
 * relays names a valid surface either way. Keeping this client to
 * `wl_compositor` plus the manager makes it a witness for the inhibit and
 * nothing else: a failure here cannot be an xdg-shell failure wearing an idle
 * inhibit's clothes.
 *
 * THE TWO MODES ARE THE TWO HALVES OF THE ONE RULE THAT MATTERS.
 *
 *   --destroy   create the inhibitor, then DESTROY it, then disconnect
 *               cleanly. The ordinary lifecycle: the shim should relay `held`
 *               and then `released`.
 *   --leak      create the inhibitor and then exit WITHOUT destroying it, and
 *               without even a clean `wl_display_disconnect`. This is the
 *               failure mode the feature is most exposed to -- an app killed
 *               mid-film -- and the shim must still relay `released`, because
 *               wlroots destroys an inhibitor resource with its client and the
 *               shim's release is driven by that destruction rather than by the
 *               app's cooperation.
 *
 * `--count N` creates N inhibitors before releasing them one at a time, which is
 * what makes the shim's AGGREGATION observable from outside: the wire carries
 * one bit per realm, so N creations must produce exactly one `held` and the
 * release of the last one exactly one `released`.
 *
 * Output is one `IDLE ...` line per step and one `SUMMARY ...` line, on stdout
 * or in the file named by `--out` -- the file exists for the stream-splicing
 * reason `gesture_probe.c` documents at length.
 */
#define _GNU_SOURCE

#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <wayland-client.h>

#include "idle-inhibit-unstable-v1-client-protocol.h"

#define MAX_INHIBITORS 8

static FILE *g_out = NULL;

static void say(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *fmt, ...) {
	va_list ap;
	va_start(ap, fmt);
	vfprintf(g_out, fmt, ap);
	va_end(ap);
	fputc('\n', g_out);
	fflush(g_out);
}

struct client {
	struct wl_display *display;
	struct wl_compositor *compositor;
	struct zwp_idle_inhibit_manager_v1 *manager;
	struct wl_surface *surface;
	struct zwp_idle_inhibitor_v1 *inhibitors[MAX_INHIBITORS];
	int held;
};

static void on_global(void *data, struct wl_registry *registry, uint32_t name,
		const char *interface, uint32_t version) {
	struct client *c = data;
	if (strcmp(interface, wl_compositor_interface.name) == 0) {
		c->compositor = wl_registry_bind(registry, name, &wl_compositor_interface,
			version < 4 ? version : 4);
	} else if (strcmp(interface, zwp_idle_inhibit_manager_v1_interface.name) == 0) {
		c->manager = wl_registry_bind(registry, name,
			&zwp_idle_inhibit_manager_v1_interface, 1);
		say("IDLE bound manager version=%u", version);
	}
}

static void on_global_remove(void *data, struct wl_registry *registry, uint32_t name) {
	(void)data;
	(void)registry;
	(void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = on_global,
	.global_remove = on_global_remove,
};

int main(int argc, char **argv) {
	g_out = stdout;
	bool leak = false;
	int want = 1;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--leak") == 0) {
			leak = true;
		} else if (strcmp(argv[i], "--destroy") == 0) {
			leak = false;
		} else if (strcmp(argv[i], "--count") == 0 && i + 1 < argc) {
			want = atoi(argv[++i]);
			if (want < 1) {
				want = 1;
			}
			if (want > MAX_INHIBITORS) {
				want = MAX_INHIBITORS;
			}
		} else if (strcmp(argv[i], "--out") == 0 && i + 1 < argc) {
			FILE *f = fopen(argv[++i], "we");
			if (f == NULL) {
				fprintf(stderr, "idle-probe: cannot open %s\n", argv[i]);
				return 2;
			}
			g_out = f;
		} else {
			fprintf(stderr, "idle-probe: unknown argument %s\n", argv[i]);
			return 2;
		}
	}

	struct client c = {0};
	c.display = wl_display_connect(NULL);
	if (c.display == NULL) {
		say("SUMMARY status=no_display");
		return 1;
	}
	struct wl_registry *registry = wl_display_get_registry(c.display);
	wl_registry_add_listener(registry, &registry_listener, &c);
	wl_display_roundtrip(c.display);

	if (c.compositor == NULL) {
		say("SUMMARY status=no_compositor");
		return 1;
	}
	/* A MISSING GLOBAL IS A REPORT, NOT A CRASH. Before #306 the shim did not
	 * advertise this interface at all, so "not advertised" is a real historical
	 * state and the acceptance script has to be able to tell it apart from a
	 * relay that failed. */
	if (c.manager == NULL) {
		say("SUMMARY status=no_manager");
		return 1;
	}

	c.surface = wl_compositor_create_surface(c.compositor);
	for (int i = 0; i < want; i++) {
		c.inhibitors[i] = zwp_idle_inhibit_manager_v1_create_inhibitor(c.manager,
			c.surface);
		c.held++;
		say("IDLE created n=%d", c.held);
	}
	wl_display_roundtrip(c.display);

	if (leak) {
		/* No destroy, no disconnect, no flush of a destroy that was never
		 * sent: the process simply stops holding the socket. `_exit` rather
		 * than `return` so no atexit handler can tidy up on the app's behalf
		 * and turn this into the polite case by accident. */
		say("SUMMARY status=leaked held=%d", c.held);
		fflush(g_out);
		_exit(0);
	}

	for (int i = 0; i < want; i++) {
		zwp_idle_inhibitor_v1_destroy(c.inhibitors[i]);
		c.held--;
		say("IDLE destroyed remaining=%d", c.held);
		/* One roundtrip per destroy, so the shim's aggregation is observed as
		 * a sequence rather than as a batch: only the LAST of these may
		 * produce a `released` upstream. */
		wl_display_roundtrip(c.display);
	}

	say("SUMMARY status=destroyed held=%d", c.held);
	wl_display_disconnect(c.display);
	return 0;
}
