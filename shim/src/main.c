/* main.c -- Vitrin OS Wayland shim entry point.
 *
 * Owns the process lifecycle and the single load-bearing bring-up order:
 *
 *   upstream -> backend -> ledger -> globals -> output -> window
 *   -> bind socket -> start backend -> arm the core link -> run loop
 *   -> teardown
 *
 * The first step is first for a protocol reason, not a stylistic one. The
 * core's `configure` is "guaranteed to precede the processing of any shim
 * request: the shim performs one synchronous read at startup -- before it
 * begins serving its own private Wayland socket" (conventions 7.2), and it
 * carries the realm-view geometry every later phase is sized from. Reading
 * it before anything else exists is what lets the whole shim assume a
 * configured session instead of carrying a deferred-configure state machine.
 *
 * Every wlroots call lives behind one of the phase helpers (server.c /
 * globals.c / output.c / xdg.c) and every core-facing call behind
 * upstream.c, so this file stays a readable script of "what happens, in what
 * order".
 */
#define _POSIX_C_SOURCE 200809L

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <wayland-server-core.h>

#include <wlr/backend.h>
#include <wlr/util/log.h>

#include "ledger.h"
#include "server.h"
#include "upstream.h"

/* The generated wire header (P1.1.2 / #11) is the shim's only contract with
 * the protocol. The _Static_assert binds this binary to the wire framing at
 * compile time; wire.c is what puts bytes on the socket. */
#include "vitrin-protocol.h"
_Static_assert(VITRIN_HEADER_LEN == 8,
	"wire frame header must be 8 bytes (docs/protocol/00-conventions.md)");

/* Event-loop signal source: terminates the display so wl_display_run() returns
 * and vitrin_shim_finish() runs a clean teardown. Delivered via the loop's
 * signalfd, so it is not subject to async-signal-safety limits. The core
 * spawns and kills the shim by signal (P1.5.3 lifecycle); exit cleanly for it. */
static int handle_terminate(int sig, void *data) {
	(void)sig;
	wl_display_terminate((struct wl_display *)data);
	return 0;
}

static void parse_args(int argc, char **argv, struct vitrin_config *cfg) {
	cfg->socket_name = NULL;
	cfg->dmabuf = false;
	cfg->standalone = false;
	cfg->width = 1280;
	cfg->height = 720;
	cfg->globals_log = NULL;
	cfg->probe_globals = false;
	cfg->probe_filter = NULL;

	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--dmabuf") == 0) {
			cfg->dmabuf = true;
		} else if (strcmp(argv[i], "--no-upstream") == 0) {
			cfg->standalone = true;
		} else if (strcmp(argv[i], "--probe-globals") == 0) {
			cfg->probe_globals = true;
		} else if (strncmp(argv[i], "--probe-globals=", 16) == 0) {
			cfg->probe_globals = true;
			cfg->probe_filter = argv[i] + 16;
		} else if (strcmp(argv[i], "--socket") == 0 && i + 1 < argc) {
			cfg->socket_name = argv[++i];
		} else if (strcmp(argv[i], "--globals-log") == 0 && i + 1 < argc) {
			cfg->globals_log = argv[++i];
		} else if (strcmp(argv[i], "--width") == 0 && i + 1 < argc) {
			cfg->width = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--height") == 0 && i + 1 < argc) {
			cfg->height = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--") == 0) {
			/* Everything after `--` is the app command the core wants
			 * this shim to exec (the core→shim argv contract of
			 * P1.5.4 / #103: `<shim> [shim-args] -- <app> <app-args>`).
			 * Actually fork/exec'ing that app is the wlroots app-spawn
			 * step (#104); until it lands, tolerate and ignore the tail
			 * so the argv the core now sends does not trip the usage
			 * branch below and abort the shim at startup. */
			break;
		} else {
			fprintf(stderr,
				"usage: %s [--socket NAME] [--dmabuf] [--no-upstream] "
				"[--width W] [--height H] [--globals-log PATH] "
				"[--probe-globals[=IFACE,IFACE,...]]\n",
				argv[0]);
			exit(2);
		}
	}

	/* --socket wins; else honor an inherited $WAYLAND_DISPLAY (the spawn
	 * contract of P1.5.2 names the private socket this way); else default. */
	if (cfg->socket_name == NULL) {
		cfg->socket_name = getenv("WAYLAND_DISPLAY");
	}
	if (cfg->socket_name == NULL || cfg->socket_name[0] == '\0') {
		cfg->socket_name = "vitrin-shim-0";
	}
	if (cfg->width <= 0 || cfg->height <= 0) {
		fprintf(stderr, "width/height must be positive\n");
		exit(2);
	}
}

int main(int argc, char **argv) {
	wlr_log_init(WLR_DEBUG, NULL);

	struct vitrin_shim s = {0};
	/* Before the standalone/upstream branch below, because teardown runs on
	 * both: this is what makes every descriptor the shim owns start at -1
	 * rather than at 0 (see vitrin_upstream_init). */
	vitrin_upstream_init(&s);
	parse_args(argc, argv, &s.cfg);

	/* Phase A0. Refusing to start without fd 3 is the point: holding that
	 * descriptor IS being a realm's shim (there is no handshake and no
	 * credential), so a process that starts without one is not a shim and
	 * should not pretend to be. --no-upstream is the explicit opt-out for
	 * development and for the globals acceptance test. */
	if (!s.cfg.standalone) {
		if (!vitrin_upstream_open(&s)) {
			wlr_log(WLR_ERROR, "no usable core connection; refusing to start");
			goto err;
		}
	} else {
		wlr_log(WLR_INFO,
			"--no-upstream: running with no core connection. Nothing is "
			"forwarded and the app is paced locally.");
	}

	if (!vitrin_backend_bringup(&s)) {
		wlr_log(WLR_ERROR, "backend bring-up failed");
		goto err;
	}
	/* Before the globals, not after: the ledger learns what was advertised
	 * from the `wl_registry.global` events on the wire, and the client's
	 * first roundtrip -- which is all of the interesting traffic -- happens
	 * the instant the socket below is bound. Instrumenting late would leave
	 * exactly the discovery phase unobserved. */
	vitrin_ledger_init(&s);
	if (!vitrin_create_globals(&s)) {
		wlr_log(WLR_ERROR, "global creation failed");
		goto err;
	}
	if (!vitrin_setup_output(&s)) {
		wlr_log(WLR_ERROR, "output setup failed");
		goto err;
	}
	if (!vitrin_setup_xdg(&s)) {
		wlr_log(WLR_ERROR, "xdg-shell setup failed");
		goto err;
	}

	/* Only now, with the session configured and the compositor built, is the
	 * app allowed to exist. */
	/* wl_display_add_socket returns 0 on success, -1 on failure. */
	if (wl_display_add_socket(s.display, s.cfg.socket_name) != 0) {
		wlr_log(WLR_ERROR, "failed to bind Wayland socket '%s'", s.cfg.socket_name);
		goto err;
	}
	/* Name our own socket so a child app spawned from this process talks to
	 * us, not to the host compositor. */
	setenv("WAYLAND_DISPLAY", s.cfg.socket_name, 1);

	if (!wlr_backend_start(s.backend)) {
		wlr_log(WLR_ERROR, "backend start failed");
		goto err;
	}

	/* Phase A1: from here the core link is pumped by the same event loop
	 * that serves the app, so a `frame_done` from the core and a commit from
	 * the app are dispatched by one thread in arrival order. */
	if (!vitrin_upstream_start(&s)) {
		wlr_log(WLR_ERROR, "cannot serve the core connection");
		goto err;
	}

	/* Shut down cleanly on the signals the core (or a user) will send. */
	wl_event_loop_add_signal(s.loop, SIGTERM, handle_terminate, s.display);
	wl_event_loop_add_signal(s.loop, SIGINT, handle_terminate, s.display);

	wlr_log(WLR_INFO,
		"vitrin-shim up: WAYLAND_DISPLAY=%s view=%dx%d dmabuf=%d upstream=%d "
		"wire_protocol_v%u",
		s.cfg.socket_name, s.cfg.width, s.cfg.height, (int)s.cfg.dmabuf,
		(int)s.up.active, VITRIN_PROTOCOL_VERSION);

	wl_display_run(s.display); /* blocks until wl_display_terminate() */

	wlr_log(WLR_INFO,
		"vitrin-shim down: forwarded=%llu deferred=%llu lost=%llu "
		"damage_rects=%llu frame_dones=%llu buffer_dones=%llu",
		(unsigned long long)s.up.frames_forwarded,
		(unsigned long long)s.up.frames_deferred,
		(unsigned long long)s.up.frames_lost,
		(unsigned long long)s.up.damage_rects_sent,
		(unsigned long long)s.up.frame_dones,
		(unsigned long long)s.up.buffer_dones);

	vitrin_shim_finish(&s);
	return 0;

err:
	vitrin_shim_finish(&s);
	return 1;
}
