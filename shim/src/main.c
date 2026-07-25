/* main.c -- Vitrin OS Wayland shim entry point.
 *
 * SPDX-License-Identifier: MPL-2.0
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

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

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

/* Reap the app child and bring the realm down with it. Registered on the
 * event loop's SIGCHLD signalfd (like handle_terminate above), so it runs as
 * an ordinary callback, free of async-signal-safety limits -- it may waitpid,
 * log, and terminate the display. The core reaps the SHIM this same way
 * (P1.5.3); this is the shim reaping its APP. Terminating the display when the
 * app exits is what makes the realm come down with its one app (no zombies,
 * one shim/one app/one universe). */
static int handle_sigchld(int sig, void *data) {
	(void)sig;
	struct vitrin_shim *s = data;
	int status;
	pid_t pid;
	while ((pid = waitpid(-1, &status, WNOHANG)) > 0) {
		if (pid != s->app_pid) {
			continue; /* not the app we track */
		}
		s->app_pid = -1;
		if (WIFEXITED(status)) {
			wlr_log(WLR_INFO, "app (pid %d) exited status=%d; terminating realm",
				(int)pid, WEXITSTATUS(status));
		} else if (WIFSIGNALED(status)) {
			wlr_log(WLR_INFO, "app (pid %d) killed by signal %d; terminating realm",
				(int)pid, WTERMSIG(status));
		}
		wl_display_terminate(s->display);
	}
	return 0;
}

/* Fork and exec the app the core conveyed after `--`, if any. The app inherits
 * the core-composed WAYLAND_DISPLAY + XDG_RUNTIME_DIR set just above and talks
 * to THIS shim, not the host compositor. It must inherit none of the shim's
 * private descriptors -- fd 3 (the core link) above all, since a live channel
 * to the TCB in a confined app is the whole confinement gone. All the shim's
 * fds are close-on-exec: fd 3 (wire.c re-arms FD_CLOEXEC), the Wayland listen
 * socket and the event-loop epoll fd (libwayland opens both with SOCK/EPOLL
 * _CLOEXEC), and the renderer/DRM fds (wlroots opens them O_CLOEXEC), so
 * execve drops every one. We assert fd 3's flag before forking rather than
 * trust it -- the one descriptor whose leak matters most. Returns false only
 * on a fork failure the caller must treat as a bring-up failure; on execv
 * failure the CHILD _exit(127)s and the parent learns via SIGCHLD. */
static bool vitrin_spawn_app(struct vitrin_shim *s) {
	if (s->cfg.app_argv == NULL) {
		wlr_log(WLR_INFO, "no app command after `--`; spawning nothing");
		return true;
	}
	if (!s->cfg.standalone) {
		int fl = fcntl(VITRIN_CORE_FD, F_GETFD);
		if (fl < 0 || (fl & FD_CLOEXEC) == 0) {
			wlr_log(WLR_ERROR,
				"fd %d is not FD_CLOEXEC before app spawn; refusing to leak the "
				"core connection to the confined app",
				VITRIN_CORE_FD);
			return false;
		}
	}
	pid_t pid = fork();
	if (pid < 0) {
		wlr_log(WLR_ERROR, "cannot fork the app: %s", strerror(errno));
		return false;
	}
	if (pid == 0) {
		/* Child: async-signal-safe only, from here to execv.
		 *
		 * Restore the default signal mask first. The shim runs its own
		 * SIGTERM/SIGINT/SIGCHLD through the event loop's signalfd, which
		 * blocks them process-wide -- and fork inherits that blocked mask
		 * across execve. An app left with SIGTERM blocked cannot be killed by
		 * it, and SIGTERM is exactly how both the core's teardown ladder
		 * (P1.5.3) and this shim's own vitrin_reap_app bring the app down; a
		 * blocked-mask app would wedge teardown. sigprocmask is
		 * async-signal-safe and the post-fork child is single-threaded. */
		sigset_t empty;
		sigemptyset(&empty);
		sigprocmask(SIG_SETMASK, &empty, NULL);
		/* The absolute program the core already audited -- execv, never
		 * execvp, so no $PATH search can substitute a different binary than
		 * the audited one. On failure the child dies with 127, the shell's
		 * "cannot exec" code, and the parent sees it via SIGCHLD. */
		execv(s->cfg.app_argv[0], s->cfg.app_argv);
		_exit(127);
	}
	s->app_pid = pid;
	wlr_log(WLR_INFO, "spawned app pid=%d: %s", (int)pid, s->cfg.app_argv[0]);
	return true;
}

/* Take the app down with the shim: SIGTERM it and reap it, so a shim told to
 * exit (the SIGTERM/SIGINT teardown and the err: path) never orphans its app
 * (P1.5.2: "killing the shim removes the surface and the app; the core
 * survives"). Idempotent -- a no-op once SIGCHLD has already reaped the app
 * (app_pid == -1) or when none was ever spawned. */
static void vitrin_reap_app(struct vitrin_shim *s) {
	if (s->app_pid <= 0) {
		return;
	}
	pid_t pid = s->app_pid;
	/* SIGTERM first, then a brief grace before escalating to SIGKILL --
	 * mirroring the core's own shim-shutdown ladder (P1.5.3): a well-behaved
	 * app exits on the term, and one that will not must still never wedge the
	 * shim's teardown. ~2s in 20ms polls. */
	kill(pid, SIGTERM);
	bool reaped = false;
	for (int i = 0; i < 100; i++) {
		pid_t r = waitpid(pid, NULL, WNOHANG);
		if (r == pid) {
			reaped = true;
			break;
		}
		if (r < 0 && errno != EINTR) {
			break; /* ECHILD: already reaped by the SIGCHLD handler */
		}
		struct timespec ts = {.tv_sec = 0, .tv_nsec = 20 * 1000 * 1000};
		nanosleep(&ts, NULL);
	}
	if (!reaped) {
		kill(pid, SIGKILL);
		pid_t r;
		do {
			r = waitpid(pid, NULL, 0);
		} while (r < 0 && errno == EINTR);
		wlr_log(WLR_INFO, "app (pid %d) did not exit on SIGTERM; killed and reaped", (int)pid);
	} else {
		wlr_log(WLR_INFO, "app (pid %d) reaped on shim teardown", (int)pid);
	}
	s->app_pid = -1;
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
	cfg->app_argv = NULL;
	cfg->app_argc = 0;

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
			 * Capture it as the app command to fork/exec once the session
			 * is up (#104). `argv[argc]` is a guaranteed NULL, so
			 * `&argv[i + 1]` is already the NULL-terminated vector execv
			 * wants -- no copy. An EMPTY tail (`--` with nothing after it,
			 * or no `--` at all) leaves app_argv NULL: the shim spawns
			 * nothing, which preserves --no-upstream dev mode and the
			 * acceptance tests that run the shim with no app. */
			if (i + 1 < argc) {
				cfg->app_argv = &argv[i + 1];
				cfg->app_argc = argc - (i + 1);
			}
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

	/* Environment fallbacks for the two DIAGNOSTIC knobs, so a bring-up run
	 * under the REAL core can request the globals ledger and probe catalogue.
	 *
	 * The production spawn path (P1.5.4 / #103) has the core convey ONLY the
	 * app command after `--`; it controls the shim's ENVIRONMENT, not its
	 * leading argv (crates/vitrin-core/src/spawn.rs). A realm's environment
	 * grows by exactly one route -- the realm's `env_allow` list, which
	 * copies named values from the core's own environment into the shim's --
	 * so these names are how the real-core Firefox render gate
	 * (tests/integration/test_real_firefox.py) asks this shim for the same
	 * globals-touched evidence firefox_bringup.sh gathers under the mock core.
	 * Both mirror the $WAYLAND_DISPLAY fallback just above: an explicit flag
	 * always wins, the env only fills a field the argv left unset. Neither
	 * changes what the shim advertises to a normal app (both default off), so
	 * the isolation-invisible wire contract (PRD Doc 2 §4.5) is untouched. */
	if (cfg->globals_log == NULL) {
		const char *env = getenv("VITRIN_SHIM_GLOBALS_LOG");
		if (env != NULL && env[0] != '\0') {
			cfg->globals_log = env;
		}
	}
	if (!cfg->probe_globals) {
		const char *env = getenv("VITRIN_SHIM_PROBE_GLOBALS");
		if (env != NULL && env[0] != '\0') {
			/* Any non-empty value arms the whole catalogue -- the bare
			 * `--probe-globals` form. A filtered probe stays argv-only:
			 * bisecting the catalogue is an interactive developer act, not
			 * something a spawned realm needs to express. */
			cfg->probe_globals = true;
		}
	}
	if (cfg->width <= 0 || cfg->height <= 0) {
		fprintf(stderr, "width/height must be positive\n");
		exit(2);
	}
}

int main(int argc, char **argv) {
	wlr_log_init(WLR_DEBUG, NULL);

	struct vitrin_shim s = {0};
	/* No app forked yet: -1, not the 0 that `= {0}` leaves (0 is a legal pid,
	 * so a bare zero would make vitrin_reap_app try to signal process 0's whole
	 * group on an early teardown). */
	s.app_pid = -1;
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
	/* Reap the app and bring the realm down when it exits. Armed BEFORE the
	 * fork below, so the child's exit can never race ahead of a handler that
	 * is not yet on the loop. */
	wl_event_loop_add_signal(s.loop, SIGCHLD, handle_sigchld, &s);

	/* Only now -- session configured, compositor built, socket bound, backend
	 * and core link running, reaper armed -- is the app forked and exec'd. */
	if (!vitrin_spawn_app(&s)) {
		wlr_log(WLR_ERROR, "app spawn failed");
		goto err;
	}

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

	/* Take the app down with the shim before releasing the compositor it
	 * renders into, so a shim exit never orphans its app. */
	vitrin_reap_app(&s);
	vitrin_shim_finish(&s);
	return 0;

err:
	vitrin_reap_app(&s);
	vitrin_shim_finish(&s);
	return 1;
}
