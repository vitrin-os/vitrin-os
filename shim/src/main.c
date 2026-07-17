/* main.c -- Vitrin OS Wayland shim skeleton (P1.6.1) entry point.
 *
 * Owns the process lifecycle and the single load-bearing bring-up order:
 * core -> globals -> output -> bind socket -> start backend -> run loop ->
 * teardown. Every wlroots call lives behind one of the three phase helpers
 * (server.c / globals.c / output.c) so this file stays a readable script of
 * "what happens, in what order".
 */
#define _POSIX_C_SOURCE 200809L

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <wayland-server-core.h>

#include <wlr/backend.h>
#include <wlr/util/log.h>

#include "server.h"

/* Consume the checked-in, generated wire-protocol header (from P1.1.2 / #11).
 * The upstream link to the core lands in P1.6.2; here we only prove the shim
 * genuinely builds against it -- a hard requirement of this task. The
 * _Static_assert binds the shim to the wire framing at compile time, and the
 * version is logged at startup so the dependency is real, not cosmetic. */
#include "vitrin-protocol.h"
_Static_assert(VITRIN_HEADER_LEN == 8,
	"wire frame header must be 8 bytes (docs/protocol/00-conventions.md)");

/* Event-loop signal source: terminates the display so wl_display_run() returns
 * and vitrin_shim_finish() runs a clean teardown. Delivered via the loop's
 * signalfd, so it is not subject to async-signal-safety limits. The core will
 * spawn and kill the shim by signal (P1.5.3 lifecycle); exit cleanly for it. */
static int handle_terminate(int sig, void *data) {
	(void)sig;
	wl_display_terminate((struct wl_display *)data);
	return 0;
}

static void parse_args(int argc, char **argv, struct vitrin_config *cfg) {
	cfg->socket_name = NULL;
	cfg->dmabuf = false;
	cfg->width = 1280;
	cfg->height = 720;

	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--dmabuf") == 0) {
			cfg->dmabuf = true;
		} else if (strcmp(argv[i], "--socket") == 0 && i + 1 < argc) {
			cfg->socket_name = argv[++i];
		} else if (strcmp(argv[i], "--width") == 0 && i + 1 < argc) {
			cfg->width = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--height") == 0 && i + 1 < argc) {
			cfg->height = atoi(argv[++i]);
		} else {
			fprintf(stderr,
				"usage: %s [--socket NAME] [--dmabuf] [--width W] [--height H]\n",
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
	parse_args(argc, argv, &s.cfg);

	if (!vitrin_backend_bringup(&s)) {
		wlr_log(WLR_ERROR, "backend bring-up failed");
		goto err;
	}
	if (!vitrin_create_globals(&s)) {
		wlr_log(WLR_ERROR, "global creation failed");
		goto err;
	}
	if (!vitrin_setup_output(&s)) {
		wlr_log(WLR_ERROR, "output setup failed");
		goto err;
	}

	/* wl_display_add_socket returns 0 on success, -1 on failure. */
	if (wl_display_add_socket(s.display, s.cfg.socket_name) != 0) {
		wlr_log(WLR_ERROR, "failed to bind Wayland socket '%s'", s.cfg.socket_name);
		goto err;
	}
	/* Name our own socket so a child app spawned from this process (P1.6.2+)
	 * talks to us, not to the host compositor. */
	setenv("WAYLAND_DISPLAY", s.cfg.socket_name, 1);

	if (!wlr_backend_start(s.backend)) {
		wlr_log(WLR_ERROR, "backend start failed");
		goto err;
	}

	/* Shut down cleanly on the signals the core (or a user) will send. */
	wl_event_loop_add_signal(s.loop, SIGTERM, handle_terminate, s.display);
	wl_event_loop_add_signal(s.loop, SIGINT, handle_terminate, s.display);

	wlr_log(WLR_INFO,
		"vitrin-shim up: WAYLAND_DISPLAY=%s dmabuf=%d wire_protocol_v%u",
		s.cfg.socket_name, (int)s.cfg.dmabuf, VITRIN_PROTOCOL_VERSION);

	wl_display_run(s.display); /* blocks until wl_display_terminate() */

	vitrin_shim_finish(&s);
	return 0;

err:
	vitrin_shim_finish(&s);
	return 1;
}
