/* wlcs/integration.c -- advisory WLCS (Wayland Conformance Test Suite)
 * integration for vitrin-shim (P1.9.4, issue #47).
 *
 * SPDX-License-Identifier: GPL-3.0-only
 *
 * ============================================================================
 * WHY THIS ONE FILE IS GPL-3.0, IN AN OTHERWISE MPL-2.0 REPOSITORY
 * ============================================================================
 * This translation unit #includes <wlcs/display_server.h> and
 * <wlcs/pointer.h> (from Canonical's `wlcs` project, MirServer/wlcs), which
 * are themselves licensed GPL-3.0-only (see /usr/share/doc/wlcs/copyright in
 * the `wlcs` Debian/Ubuntu package, or LICENSE in the wlcs source tree). A
 * compiled object that implements that header's ABI is a work based on it,
 * so the resulting shared module is a GPL-3.0 combined work regardless of
 * how the rest of this repository is licensed. That combination is lawful
 * because the shim sources this module links (shim/src/*.c) are MPL-2.0,
 * which names GPL-3.0 a Secondary License (MPL 2.0 section 1.12) and
 * permits distributing the Larger Work under GPL terms (section 3.3) --
 * which is exactly what MPL's Exhibit B switches off, so no file under
 * shim/ may ever carry the Exhibit B notice. (The one Apache-2.0 file that
 * also lands here, the generated include/vitrin-protocol.h, is one-way
 * compatible into GPL-3.0.) shim/wlcs/README.md is the normative statement
 * of that boundary and of everything this module does NOT cover; read it
 * before touching this file.
 *
 * The boundary is drawn at the directory: nothing outside shim/wlcs/ needs
 * to know this file exists, the main `vitrin-shim` executable never links
 * it, and it is never installed (meson.build: no `install: true`). It is
 * built, at most, by one advisory, non-blocking CI job and by a developer
 * who explicitly asked for it (see the `wlcs` Meson option).
 *
 * ============================================================================
 * WHAT THIS FILE IS: A BRIDGE, NOT A REIMPLEMENTATION
 * ============================================================================
 * WLCS dlopen()s a shared module and calls `wlcs_server_integration` (the
 * ABI wlcs/display_server.h defines) to stand up ONE compositor instance per
 * test, drive it, and tear it down. Every hook below is implemented by
 * calling the REAL shim bring-up/teardown phases (server.c/globals.c/
 * output.c/xdg.c/seat.c, via server.h/seat.h/upstream.h) in exactly the
 * order main.c uses for `--no-upstream` -- this is not a second compositor
 * written for the occasion, it is the same one main.c drives, missing only
 * the argv-parsing, app-forking and core-link parts main.c does not need in
 * this mode.
 *
 * ============================================================================
 * THE ONE PIECE OF NEW MACHINERY: A WAKE-UP PIPE, BECAUSE WLCS IS
 * MULTI-THREADED AND wl_display's INTERNALS ARE NOT
 * ============================================================================
 * wlcs/display_server.h's `start` hook must return WITHOUT blocking, and
 * `create_client_socket` and `create_pointer`'s callbacks are then invoked
 * from WLCS's OWN thread while the compositor's event loop is running on
 * ITS thread (spawned by `start`, below). Calling `wl_client_create` --
 * or, for input, `vitrin_seat_handle_event`, which walks the same
 * `wlr_scene`/`wlr_seat` the display thread is compositing against -- from
 * a second thread while `wl_display_run` may be blocked in the display
 * thread's own `epoll_wait` is a data race on wayland-server's internal
 * client list and on wlroots' scene/seat state, not merely "unsupported":
 * both are plain, non-atomic C structures with no lock of their own.
 *
 * The fix used throughout this file is the same one single-threaded
 * Wayland compositors always need when a second thread has to reach in:
 * a self-pipe, registered on the display's OWN event loop
 * (`wl_event_loop_add_fd`), that the display thread polls alongside every
 * client fd. A caller on another thread (`run_on_display_thread` below)
 * queues one request, writes one byte to wake `epoll_wait` even if it was
 * blocked indefinitely, and blocks on a condition variable until the
 * display thread's own callback (`on_wakeup`) has run the request AND
 * signals back. Exactly one request is ever in flight, so nothing here
 * needs anything more elaborate than a mutex, a condvar and an enum. */

/* _GNU_SOURCE, not _POSIX_C_SOURCE: pipe2(2) is a Linux/glibc extension, not
 * POSIX. WLR_USE_UNSTABLE is supplied project-wide by meson.build's
 * add_project_arguments, so it is deliberately not (re)defined here. */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#include <wayland-server-core.h>

#include <wlr/backend.h>
#include <wlr/util/log.h>

#include <wlcs/display_server.h>
#include <wlcs/pointer.h>

#include "seat.h"
#include "server.h"
#include "upstream.h"
#include "vitrin-protocol.h"

/* ---- the one-request-at-a-time cross-thread handoff -------------------- */

enum pending_kind {
	PENDING_NONE = 0,
	PENDING_CREATE_CLIENT,
	PENDING_SEAT_EVENT,
	PENDING_TERMINATE,
};

/* Largest frame this module ever hands to vitrin_seat_handle_event: the
 * motion and button events are both header(8) + 3*u32(12) = 20 bytes.
 * Rounded up generously; the _Static_asserts below pin it to the real
 * events. */
#define PENDING_FRAME_CAP 32u

/* The wire length of the only two events this module encodes, spelled the way
 * the generated encoders in vitrin-protocol.h spell it:
 *
 *   uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
 *
 * -- three u32-wide scalar arguments (x/y/origin, button/state/origin), no
 * strings, no arrays, no fds. The generated header exposes no per-message
 * length constant to import, so this restates that arithmetic. */
#define VITRIN_WLCS_SEAT_FRAME_LEN (VITRIN_HEADER_LEN + 4u + 4u + 4u)

/* WHAT THESE ASSERTIONS DO AND DO NOT COVER -- the previous version of this
 * comment claimed a `_Static_assert` "below" that did not exist anywhere in
 * the file, so be precise about the ones that now do.
 *
 * They catch PENDING_FRAME_CAP being shrunk below what the seat events
 * already need, and -- via sizeof on the generated argument structs -- they
 * also catch the IDL GROWING either event by another fixed-width scalar,
 * because that grows the struct in lockstep with the frame. What they cannot
 * catch is a string or array argument being added: the struct would then gain
 * a small descriptor while the wire frame gained an unbounded payload, and
 * the two would stop tracking each other.
 *
 * That residual case is still caught, just at runtime rather than at compile
 * time, and NOT silently: emit_motion/emit_button encode into a
 * PENDING_FRAME_CAP-sized buffer, so an over-long frame makes the encoder
 * return VITRIN_ENCODE_ERR_OVERFLOW, which those functions log at WLR_ERROR
 * and then DROP -- run_on_display_thread's own frame_len > PENDING_FRAME_CAP
 * branch does the same. A dropped input event means the wlcs test waiting on
 * it times out rather than reporting anything about the shim, so if this
 * module ever starts logging "failed to encode" or "frame too large", the
 * run's numbers are meaningless until the cap is raised. */
_Static_assert(PENDING_FRAME_CAP >= VITRIN_WLCS_SEAT_FRAME_LEN,
	"PENDING_FRAME_CAP must hold a whole vitrin_shim_seat motion/button frame");
_Static_assert(PENDING_FRAME_CAP >= VITRIN_HEADER_LEN + sizeof(vitrin_shim_seat_evt_motion_t),
	"vitrin_shim_seat.motion grew past PENDING_FRAME_CAP -- raise the cap");
_Static_assert(PENDING_FRAME_CAP >= VITRIN_HEADER_LEN + sizeof(vitrin_shim_seat_evt_button_t),
	"vitrin_shim_seat.button grew past PENDING_FRAME_CAP -- raise the cap");

struct vitrin_wlcs_server {
	/* MUST be first: WLCS hands every hook a `WlcsDisplayServer*` that is
	 * really a pointer to this struct, cast back via wrap() below -- the
	 * same "base struct is the first member" idiom the wlcs headers'
	 * own examples use, and the only thing that makes the cast legal. */
	struct WlcsDisplayServer base;

	struct vitrin_shim shim;

	/* The display thread `start` spawns and `stop` joins. */
	pthread_t display_thread;
	bool display_thread_running;

	/* The wake-up pipe: [0] read end, polled by the shim's own event loop;
	 * [1] write end, written by whichever thread calls
	 * run_on_display_thread. Both O_CLOEXEC so a spawned test app (there is
	 * never one here, but the invariant costs nothing) cannot inherit them. */
	int wakeup[2];
	struct wl_event_source *wakeup_source;

	pthread_mutex_t lock;
	pthread_cond_t cond;
	enum pending_kind pending;
	int pending_fd;                       /* PENDING_CREATE_CLIENT */
	uint8_t pending_frame[PENDING_FRAME_CAP]; /* PENDING_SEAT_EVENT */
	size_t pending_frame_len;

	struct WlcsExtensionDescriptor extensions[1];
	struct WlcsIntegrationDescriptor descriptor;
};

static struct vitrin_wlcs_server *wrap(const struct WlcsDisplayServer *server) {
	/* Casting away const: every hook below only reads through this pointer
	 * except the ones the header itself declares non-const. */
	return (struct vitrin_wlcs_server *)(void *)server;
}

/* Runs `kind` on the display thread and blocks until it has completed.
 * Exactly one request may be in flight (the outer while-loop below), which
 * is all any current caller needs: WLCS drives one test at a time and never
 * calls two of these hooks concurrently for the same server. */
static void run_on_display_thread(struct vitrin_wlcs_server *ws,
		enum pending_kind kind, int fd, const uint8_t *frame, size_t frame_len) {
	pthread_mutex_lock(&ws->lock);
	while (ws->pending != PENDING_NONE) {
		pthread_cond_wait(&ws->cond, &ws->lock);
	}
	ws->pending = kind;
	ws->pending_fd = fd;
	if (frame != NULL) {
		if (frame_len > PENDING_FRAME_CAP) {
			wlr_log(WLR_ERROR, "wlcs integration: frame too large for the "
				"pending buffer (%zu > %u); dropped", frame_len, PENDING_FRAME_CAP);
			ws->pending = PENDING_NONE;
			pthread_mutex_unlock(&ws->lock);
			return;
		}
		memcpy(ws->pending_frame, frame, frame_len);
		ws->pending_frame_len = frame_len;
	}
	/* Wake epoll_wait even if it is blocked with an infinite timeout: adding
	 * readiness to an fd already in the epoll set is visible to a
	 * concurrently-blocked waiter on Linux, which is exactly the property
	 * this self-pipe relies on. */
	uint8_t byte = 1;
	ssize_t written = write(ws->wakeup[1], &byte, 1);
	(void)written; /* best-effort: a full pipe still has an unread byte in it */
	while (ws->pending == kind) {
		pthread_cond_wait(&ws->cond, &ws->lock);
	}
	pthread_mutex_unlock(&ws->lock);
}

/* Runs ON the display thread (it is the event loop's own fd callback), so
 * everything it touches -- wl_client_create's client list, the seat replay,
 * the scene it hit-tests against -- is exactly as safe to call here as it
 * is from any other listener the same loop dispatches. */
static int on_wakeup(int fd, uint32_t mask, void *data) {
	(void)mask;
	struct vitrin_wlcs_server *ws = data;
	uint8_t drain[64];
	ssize_t n;
	while ((n = read(fd, drain, sizeof drain)) > 0) {
		/* drain every queued wake-up byte */
	}
	(void)n;

	pthread_mutex_lock(&ws->lock);
	switch (ws->pending) {
	case PENDING_CREATE_CLIENT:
		wl_client_create(ws->shim.display, ws->pending_fd);
		break;
	case PENDING_SEAT_EVENT:
		vitrin_seat_handle_event(&ws->shim, ws->pending_frame, ws->pending_frame_len);
		vitrin_seat_frame_boundary(&ws->shim);
		break;
	case PENDING_TERMINATE:
		wl_display_terminate(ws->shim.display);
		break;
	case PENDING_NONE:
		break; /* spurious wake-up: nothing queued (harmless) */
	}
	ws->pending = PENDING_NONE;
	pthread_cond_broadcast(&ws->cond);
	pthread_mutex_unlock(&ws->lock);
	return 0;
}

/* ---- WlcsPointer: emulated pointer, replayed through the real seat path */

struct vitrin_wlcs_pointer {
	struct WlcsPointer base; /* first member, same cast idiom as above */
	struct vitrin_wlcs_server *server;
	double x, y;
	bool have_position;
};

static void emit_motion(struct vitrin_wlcs_server *ws, double x, double y) {
	vitrin_shim_seat_evt_motion_t ev = {
		.x = vitrin_fixed_from_double(x),
		.y = vitrin_fixed_from_double(y),
		/* EMULATED, never PHYSICAL: seat.h's B2 argument is exactly that a
		 * synthetic actuator must carry the tag a real device would not,
		 * and WLCS's fake pointer is a synthetic actuator by construction.
		 * There is no `vitrin_origin_physical()` to reach for even if this
		 * were a mistake to want (seat.h, "the shim ... has even less
		 * business inventing one"). */
		.origin = VITRIN_SHIM_SEAT_ORIGIN_EMULATED,
	};
	uint8_t buf[PENDING_FRAME_CAP];
	int32_t n = vitrin_shim_seat_evt_motion_encode(&ev, VITRIN_SEAT_ID, buf, sizeof buf);
	if (n < 0) {
		wlr_log(WLR_ERROR, "wlcs integration: failed to encode a motion event");
		return;
	}
	run_on_display_thread(ws, PENDING_SEAT_EVENT, -1, buf, (size_t)n);
}

static void emit_button(struct vitrin_wlcs_server *ws, uint32_t button,
		vitrin_actuator_pointer_button_state_t state) {
	vitrin_shim_seat_evt_button_t ev = {
		.button = button,
		.state = state,
		.origin = VITRIN_SHIM_SEAT_ORIGIN_EMULATED,
	};
	uint8_t buf[PENDING_FRAME_CAP];
	int32_t n = vitrin_shim_seat_evt_button_encode(&ev, VITRIN_SEAT_ID, buf, sizeof buf);
	if (n < 0) {
		wlr_log(WLR_ERROR, "wlcs integration: failed to encode a button event");
		return;
	}
	run_on_display_thread(ws, PENDING_SEAT_EVENT, -1, buf, (size_t)n);
}

static void pointer_move_absolute(WlcsPointer *pointer, wl_fixed_t x, wl_fixed_t y) {
	struct vitrin_wlcs_pointer *p = (struct vitrin_wlcs_pointer *)pointer;
	p->x = wl_fixed_to_double(x);
	p->y = wl_fixed_to_double(y);
	p->have_position = true;
	emit_motion(p->server, p->x, p->y);
}

static void pointer_move_relative(WlcsPointer *pointer, wl_fixed_t dx, wl_fixed_t dy) {
	struct vitrin_wlcs_pointer *p = (struct vitrin_wlcs_pointer *)pointer;
	if (!p->have_position) {
		/* WLCS always calls move_absolute at least once per test before
		 * relying on a relative move in practice, but starting from (0, 0)
		 * rather than an uninitialised value keeps this defined either way. */
		p->x = 0.0;
		p->y = 0.0;
		p->have_position = true;
	}
	p->x += wl_fixed_to_double(dx);
	p->y += wl_fixed_to_double(dy);
	emit_motion(p->server, p->x, p->y);
}

static void pointer_button_up(WlcsPointer *pointer, int button) {
	struct vitrin_wlcs_pointer *p = (struct vitrin_wlcs_pointer *)pointer;
	emit_button(p->server, (uint32_t)button, VITRIN_ACTUATOR_POINTER_BUTTON_STATE_RELEASED);
}

static void pointer_button_down(WlcsPointer *pointer, int button) {
	struct vitrin_wlcs_pointer *p = (struct vitrin_wlcs_pointer *)pointer;
	emit_button(p->server, (uint32_t)button, VITRIN_ACTUATOR_POINTER_BUTTON_STATE_PRESSED);
}

static void pointer_destroy(WlcsPointer *pointer) {
	free(pointer);
}

static WlcsPointer *create_pointer(WlcsDisplayServer *server) {
	struct vitrin_wlcs_pointer *p = calloc(1, sizeof(*p));
	if (p == NULL) {
		return NULL;
	}
	p->base.version = WLCS_POINTER_VERSION;
	p->base.move_absolute = pointer_move_absolute;
	p->base.move_relative = pointer_move_relative;
	p->base.button_up = pointer_button_up;
	p->base.button_down = pointer_button_down;
	p->base.destroy = pointer_destroy;
	p->server = wrap(server);
	return &p->base;
}

/* No wl_touch capability exists on this seat (globals.c: "v0's seat
 * vocabulary has no touch event"), and there is no `touch` opcode on
 * `vitrin_shim_seat` for this module to encode even if there were a capability
 * to advertise -- see shim/wlcs/README.md's "Limitations".
 *
 * RETURNING NULL DOES NOT MAKE WLCS SKIP TOUCH TESTS. An earlier version of
 * this comment asserted it did ("the Touch* test group skips itself rather
 * than crashing"); that is not what happens. On wlcs 1.7.0 the first test
 * parameter that gets far enough to actually use a touch device SEGFAULTS
 * inside the wlcs runner binary, taking the rest of the run with it. On
 * wlcs 1.6.1 -- what Ubuntu 24.04 ships, so what CI installs -- the same
 * parameters fail on a protocol error before reaching this function, so the
 * run survives; that is luck, not design. See README.md's "Known hazard"
 * for the reproduction and the evidence. run-advisory.sh's `--gtest_filter`
 * excludes the *Touch* suites, but NOT the touch-device parameters of the
 * parameterised input suites, so nothing in the scope protects against
 * this.
 *
 * There is nothing to return instead: WlcsTouch's ABI has no "unsupported"
 * value, and a stub that accepted touch events would have no wire event to
 * encode them into. This stays NULL, honestly documented, until either
 * `vitrin_shim_seat` gains touch or the scope stops including touch
 * parameters. */
static WlcsTouch *create_touch(WlcsDisplayServer *server) {
	(void)server;
	return NULL;
}

/* No-op: xdg.c's whole layout policy is fixed single-maximized-at-the-origin
 * (D9/E6 -- "LAYOUT IS ONE RULE"), so there is no window position for WLCS to
 * set. Every toplevel this shim ever creates is already exactly where the
 * realm view is, and `configure_to_view` puts it back there on every
 * initial commit regardless of what a client (or WLCS, standing in for one)
 * asked for. Tests that depend on placing a window away from (0, 0) will
 * observe the maximized geometry instead -- a real, documented gap, not a
 * bug in this bridge. See shim/wlcs/README.md. */
static void position_window_absolute(WlcsDisplayServer *server,
		struct wl_display *client, struct wl_surface *surface, int x, int y) {
	(void)server;
	(void)client;
	(void)surface;
	(void)x;
	(void)y;
}

static WlcsIntegrationDescriptor const *get_descriptor(WlcsDisplayServer const *server) {
	return &wrap(server)->descriptor;
}

static void *display_thread_main(void *arg) {
	struct vitrin_wlcs_server *ws = arg;
	wl_display_run(ws->shim.display); /* returns once on_wakeup calls terminate */
	return NULL;
}

static void start(WlcsDisplayServer *server) {
	struct vitrin_wlcs_server *ws = wrap(server);
	if (!wlr_backend_start(ws->shim.backend)) {
		wlr_log(WLR_ERROR, "wlcs integration: backend start failed");
		return;
	}
	/* No-op in this standalone configuration (upstream.h: "False in
	 * standalone mode ... every entry point below is a no-op"), called
	 * anyway to mirror main.c's real bring-up order exactly. */
	vitrin_upstream_start(&ws->shim);
	int rc = pthread_create(&ws->display_thread, NULL, display_thread_main, ws);
	if (rc != 0) {
		wlr_log(WLR_ERROR, "wlcs integration: pthread_create failed: %s", strerror(rc));
		return;
	}
	ws->display_thread_running = true;
}

static void stop(WlcsDisplayServer *server) {
	struct vitrin_wlcs_server *ws = wrap(server);
	if (!ws->display_thread_running) {
		return;
	}
	run_on_display_thread(ws, PENDING_TERMINATE, -1, NULL, 0);
	pthread_join(ws->display_thread, NULL);
	ws->display_thread_running = false;
}

static int create_client_socket(WlcsDisplayServer *server) {
	struct vitrin_wlcs_server *ws = wrap(server);
	int sv[2];
	if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) != 0) {
		wlr_log(WLR_ERROR, "wlcs integration: socketpair failed: %s", strerror(errno));
		return -1;
	}
	/* sv[0] becomes the server-side wl_client (owned by wl_client_create,
	 * which closes it on client disconnect); sv[1] is handed to WLCS, which
	 * the header says WLCS owns and will close. */
	run_on_display_thread(ws, PENDING_CREATE_CLIENT, sv[0], NULL, 0);
	return sv[1];
}

static WlcsDisplayServer *create_server(int argc, char const **argv) {
	static bool wlr_log_ready = false;
	if (!wlr_log_ready) {
		/* wlroots defaults to WLR_ERROR-and-above with no callback until
		 * this runs once; WLCS never calls it for us. Once per process, not
		 * once per server: wlr_log_init resets global state every call, and
		 * WLCS's own tests create/destroy many servers in one process. */
		wlr_log_init(getenv("VITRIN_WLCS_DEBUG") != NULL ? WLR_DEBUG : WLR_ERROR, NULL);
		wlr_log_ready = true;
	}

	struct vitrin_wlcs_server *ws = calloc(1, sizeof(*ws));
	if (ws == NULL) {
		return NULL;
	}
	ws->base.version = WLCS_DISPLAY_SERVER_VERSION;
	ws->base.start = start;
	ws->base.stop = stop;
	ws->base.create_client_socket = create_client_socket;
	ws->base.position_window_absolute = position_window_absolute;
	ws->base.create_pointer = create_pointer;
	ws->base.create_touch = create_touch;
	ws->base.get_descriptor = get_descriptor;
	/* start_on_this_thread deliberately left NULL: the header requires only
	 * one of {start, start_on_this_thread}, and `start` (a real background
	 * thread) is the one this bridge implements. */

	ws->wakeup[0] = -1;
	ws->wakeup[1] = -1;
	ws->pending_fd = -1;

	ws->shim.app_pid = -1;
	vitrin_upstream_init(&ws->shim);
	ws->shim.cfg.standalone = true;
	ws->shim.cfg.width = 1280;
	ws->shim.cfg.height = 720;
	/* --width/--height, the only two flags this bridge honours from the
	 * "COMPOSITOR_OPTIONS" WLCS passes through after stripping its own --
	 * everything else main.c parses (--socket, --dmabuf, --probe-globals,
	 * the app-spawn `--`) is meaningless here: there is no named socket to
	 * bind, no core link, and no app to spawn. */
	for (int i = 0; i + 1 < argc; i++) {
		if (strcmp(argv[i], "--width") == 0) {
			ws->shim.cfg.width = atoi(argv[i + 1]);
		} else if (strcmp(argv[i], "--height") == 0) {
			ws->shim.cfg.height = atoi(argv[i + 1]);
		}
	}
	if (ws->shim.cfg.width <= 0 || ws->shim.cfg.height <= 0) {
		wlr_log(WLR_ERROR, "wlcs integration: --width/--height must be positive");
		free(ws);
		return NULL;
	}

	/* O_NONBLOCK is load-bearing on the READ end: on_wakeup drains the pipe
	 * in a loop until read() reports "nothing left" (errno == EAGAIN with a
	 * blocking fd, read() would instead block forever on the very call that
	 * is supposed to detect "empty" -- the display thread would wedge on
	 * its own drain loop the first time it ever ran it). Applying it to
	 * both ends is harmless: the write side is always a single-byte,
	 * best-effort write anyway (run_on_display_thread ignores a short
	 * write; a full pipe still has an unread wake-up byte in it). */
	if (pipe2(ws->wakeup, O_CLOEXEC | O_NONBLOCK) != 0) {
		wlr_log(WLR_ERROR, "wlcs integration: pipe2 failed: %s", strerror(errno));
		free(ws);
		return NULL;
	}
	pthread_mutex_init(&ws->lock, NULL);
	pthread_cond_init(&ws->cond, NULL);
	ws->pending = PENDING_NONE;

	if (!vitrin_backend_bringup(&ws->shim)) {
		wlr_log(WLR_ERROR, "wlcs integration: backend bring-up failed");
		goto err;
	}
	ws->wakeup_source = wl_event_loop_add_fd(ws->shim.loop, ws->wakeup[0],
		WL_EVENT_READABLE, on_wakeup, ws);
	if (ws->wakeup_source == NULL) {
		wlr_log(WLR_ERROR, "wlcs integration: wl_event_loop_add_fd failed");
		goto err;
	}
	vitrin_ledger_init(&ws->shim);
	if (!vitrin_create_globals(&ws->shim)) {
		wlr_log(WLR_ERROR, "wlcs integration: global creation failed");
		goto err;
	}
	if (!vitrin_setup_output(&ws->shim)) {
		wlr_log(WLR_ERROR, "wlcs integration: output setup failed");
		goto err;
	}
	if (!vitrin_setup_xdg(&ws->shim)) {
		wlr_log(WLR_ERROR, "wlcs integration: xdg-shell setup failed");
		goto err;
	}

	/* Advertised so WLCS can skip whatever it knows to gate on an extension
	 * list (its own layer-shell/tablet/primary-selection groups): this shim
	 * implements exactly xdg_wm_base version 6 and nothing else optional. */
	ws->extensions[0].name = "xdg_shell";
	ws->extensions[0].version = 6;
	ws->descriptor.version = WLCS_INTEGRATION_DESCRIPTOR_VERSION;
	ws->descriptor.num_extensions = 1;
	ws->descriptor.supported_extensions = ws->extensions;

	return &ws->base;

err:
	vitrin_shim_finish(&ws->shim);
	if (ws->wakeup[0] >= 0) {
		close(ws->wakeup[0]);
	}
	if (ws->wakeup[1] >= 0) {
		close(ws->wakeup[1]);
	}
	pthread_mutex_destroy(&ws->lock);
	pthread_cond_destroy(&ws->cond);
	free(ws);
	return NULL;
}

static void destroy_server(WlcsDisplayServer *server) {
	struct vitrin_wlcs_server *ws = wrap(server);
	if (ws->display_thread_running) {
		stop(server); /* defensive: WLCS is expected to have already called
		               * this, but a half-torn-down server must never leak
		               * the display thread. */
	}
	if (ws->wakeup_source != NULL) {
		wl_event_source_remove(ws->wakeup_source);
		ws->wakeup_source = NULL;
	}
	vitrin_shim_finish(&ws->shim);
	if (ws->wakeup[0] >= 0) {
		close(ws->wakeup[0]);
	}
	if (ws->wakeup[1] >= 0) {
		close(ws->wakeup[1]);
	}
	pthread_mutex_destroy(&ws->lock);
	pthread_cond_destroy(&ws->cond);
	free(ws);
}

const struct WlcsServerIntegration wlcs_server_integration = {
	.version = WLCS_SERVER_INTEGRATION_VERSION,
	.create_server = create_server,
	.destroy_server = destroy_server,
};
