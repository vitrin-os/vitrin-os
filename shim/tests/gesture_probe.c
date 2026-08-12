/* gesture_probe.c -- the app side of WS-E.4.2 (issue #222): a Wayland client
 * that binds the three pointer-side extensions the shim grew for the touchpad
 * and writes down exactly what arrived, including whether it was left holding
 * a gesture nobody ended.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY A SEPARATE PROGRAM, AND NOT input-echo-client. That client is the
 * P1.6.3 witness for `wl_pointer` and `wl_keyboard`, and it is the app three
 * acceptance scripts and a manual runbook already assert against. The classes
 * here arrive on three *other* globals -- `zwp_relative_pointer_v1`,
 * `zwp_pointer_gestures_v1`, `zwp_pointer_constraints_v1` -- and the fact this
 * one exists to settle is not "what arrived" but "what am I still holding",
 * which needs pairing state no existing client keeps. `clipboard_peer.c` was
 * written for issue #213 on the same reasoning.
 *
 * WHAT ONLY THE RECEIVING END CAN SETTLE. The core's flight recorder says
 * what the core sent; the shim's replay trace says what the shim believes it
 * forwarded. Neither can answer the question WS-E.4.2's third acceptance
 * criterion actually asks -- whether a consumed or dropped `gesture_end` left
 * the APP accumulating a gesture forever. That is the latched-modifier failure
 * in a third shape, and only the app knows it is latched. So this client keeps
 * the same pairing discipline a real toolkit keeps:
 *
 *   * at most one gesture in flight, by the protocol's own rule;
 *   * an update belongs to the gesture of ITS kind, or it is unpaired;
 *   * an end that does not match what is in flight is unpaired;
 *
 * and it reports both numbers at exit. `in_flight=none` at the end is the
 * whole razor: the app let go. `in_flight=swipe` is an app stuck mid-gesture.
 * `unpaired=` counts everything the shim should structurally never send --
 * a second begin, an orphan update, an orphan end -- so a shim that replayed
 * one is a failure here rather than a silence.
 *
 * POINTER CONSTRAINTS ARE ASKED FOR, NEVER INJECTED. Relative motion and
 * gestures are things that happen TO an app; a pointer lock is something an
 * app REQUESTS, so no injector line can drive one and a client had to. With
 * `--lock-pointer` this client locks the pointer to its own surface on its
 * first `wl_pointer.enter`, exactly as a 3D viewport or a drawing tool does,
 * and prints every `locked`/`unlocked` edge the compositor sends it. Those
 * edges are the app-visible half of the core's `pointer_constraint_state`
 * verdict, which is otherwise observable nowhere outside the core's own
 * process.
 *
 * `--lock-on-map` is the same ask made WITHOUT waiting for an enter, and it
 * exists for one reason: an app in a realm the output is not bound to never
 * receives `wl_pointer.enter` at all, so on `--lock-pointer` it would never
 * ask -- and "that app was not told `locked`" would then be a fact about this
 * client rather than about the core's per-realm scoping. Wayland allows the
 * ask at any time (activation is the compositor's decision, not the client's),
 * so this is a legal thing for a client to do and not a contrivance. The line
 * carries `tag=` because two realms run two copies of this program, and on
 * the shared stdout an untagged ask cannot be attributed to either.
 *
 * Output is one `IN ...` line per event, then one `SUMMARY ...` line of
 * counters and the final pairing state -- on stdout, or in the file named by
 * `--out`.
 */
#define _GNU_SOURCE

#include <poll.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include <wayland-client.h>

#include "pointer-constraints-unstable-v1-client-protocol.h"
#include "pointer-gestures-unstable-v1-client-protocol.h"
#include "relative-pointer-unstable-v1-client-protocol.h"
#include "xdg-shell-client-protocol.h"

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

/* Where the `IN`/`SUMMARY` lines go. `stdout` by default, so running this by
 * hand still shows its work; `--out PATH` gives it a file of its own.
 *
 * THE FILE IS NOT A CONVENIENCE, and the reason is written down at length
 * because it is invisible until it bites and looks like a compositor bug when
 * it does. Under a nested `vitrind` this program's stdout is the CORE's
 * inherited stream, shared by the core, both shims and both copies of this
 * program. wlroots' `wlr_log` writes to an UNBUFFERED stderr, which means one
 * log line becomes several `write()` calls -- and another process's write
 * landing between two of them splices its line into the middle of that one.
 * Observed verbatim, from a run of `test_real_gestures.py`:
 *
 *   seat-replay: seq=8 event=gesture_pinch_update ... dx=0.500 dy=-0.500
 *   scaleIN swipe_begin fingers=3
 *   =0.750 rotation=10.000
 *
 * The app's line survived, welded to `scale` with no separator, so a reader
 * tokenising by whitespace could not find it -- and a gate that asserts what
 * an app received then fails for a reason that has nothing to do with the
 * compositor. No amount of buffering discipline HERE fixes a splice caused by
 * a writer over there; the only structural fix is not to share the stream. */
static FILE *g_out = NULL;

/* What this client believes it is in the middle of. The protocol allows at
 * most one, which is why this is a scalar and not a stack: a second begin is
 * not a nested gesture, it is a bug in whoever sent it. */
enum in_flight {
	FLIGHT_NONE = 0,
	FLIGHT_SWIPE,
	FLIGHT_PINCH,
};

static const char *flight_name(enum in_flight f) {
	switch (f) {
	case FLIGHT_SWIPE: return "swipe";
	case FLIGHT_PINCH: return "pinch";
	case FLIGHT_NONE: break;
	}
	return "none";
}

struct client {
	struct wl_display *display;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	struct wl_seat *seat;
	struct wl_pointer *pointer;

	struct zwp_relative_pointer_manager_v1 *relative_manager;
	struct zwp_relative_pointer_v1 *relative;
	struct zwp_pointer_gestures_v1 *gestures;
	struct zwp_pointer_gesture_swipe_v1 *swipe;
	struct zwp_pointer_gesture_pinch_v1 *pinch;
	struct zwp_pointer_constraints_v1 *constraints;
	struct zwp_locked_pointer_v1 *lock;

	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;
	struct wl_buffer *buffer;
	int width, height;
	bool configured;
	bool want_lock;
	/* Ask on the first mapped frame rather than on the first enter. See the
	 * header comment: an unfocused realm's app never gets an enter. */
	bool lock_on_map;
	/* Names this instance in the SUMMARY line. Two realms run two copies of
	 * this program; with `--out` they have a file each and the tag is
	 * belt-and-braces, but without one they share the core's stdout and two
	 * untagged summaries cannot be told apart -- and an assertion about "the
	 * app that received nothing" cannot tell which app it read. */
	const char *tag;

	enum in_flight flight;

	uint64_t n_enter, n_leave, n_motion;
	uint64_t n_relative;
	uint64_t n_swipe_begin, n_swipe_update, n_swipe_end;
	uint64_t n_pinch_begin, n_pinch_update, n_pinch_end;
	uint64_t n_locked, n_unlocked;
	/* Everything the compositor should structurally never have sent: a
	 * begin while one was in flight, an update or an end for a gesture that
	 * was not. */
	uint64_t n_unpaired;
};

/* ---- gesture pairing, the app's own copy ------------------------------ */

static void begin_flight(struct client *c, enum in_flight kind) {
	if (c->flight != FLIGHT_NONE) {
		/* Not "ignore the second": counted, because the shim enforces
		 * one-in-flight itself and a second reaching an app means that
		 * enforcement is gone. The new one is adopted so the rest of the
		 * run still reports something coherent. */
		c->n_unpaired++;
	}
	c->flight = kind;
}

/* True iff an update or an end may be attributed to what is in flight. */
static bool flight_matches(struct client *c, enum in_flight kind) {
	if (c->flight != kind) {
		c->n_unpaired++;
		return false;
	}
	return true;
}

/* ---- relative pointer ------------------------------------------------- */

static void relative_motion(void *data, struct zwp_relative_pointer_v1 *rp,
		uint32_t utime_hi, uint32_t utime_lo, wl_fixed_t dx, wl_fixed_t dy,
		wl_fixed_t dx_unaccel, wl_fixed_t dy_unaccel) {
	(void)rp; (void)utime_hi; (void)utime_lo;
	struct client *c = data;
	c->n_relative++;
	/* All four printed, and printed apart: the accelerated and unaccelerated
	 * deltas are different numbers on a real device, and a translation that
	 * copied one into the other's field is invisible unless the receiver
	 * says which it got. */
	fprintf(g_out, "IN relative_motion dx=%.3f dy=%.3f udx=%.3f udy=%.3f\n",
		wl_fixed_to_double(dx), wl_fixed_to_double(dy),
		wl_fixed_to_double(dx_unaccel), wl_fixed_to_double(dy_unaccel));
}

static const struct zwp_relative_pointer_v1_listener relative_listener = {
	.relative_motion = relative_motion,
};

/* ---- swipe ------------------------------------------------------------ */

static void swipe_begin(void *data, struct zwp_pointer_gesture_swipe_v1 *g,
		uint32_t serial, uint32_t time, struct wl_surface *surface, uint32_t fingers) {
	(void)g; (void)serial; (void)time; (void)surface;
	struct client *c = data;
	c->n_swipe_begin++;
	begin_flight(c, FLIGHT_SWIPE);
	fprintf(g_out, "IN swipe_begin fingers=%u\n", fingers);
}

static void swipe_update(void *data, struct zwp_pointer_gesture_swipe_v1 *g,
		uint32_t time, wl_fixed_t dx, wl_fixed_t dy) {
	(void)g; (void)time;
	struct client *c = data;
	c->n_swipe_update++;
	bool paired = flight_matches(c, FLIGHT_SWIPE);
	fprintf(g_out, "IN swipe_update dx=%.3f dy=%.3f paired=%d\n",
		wl_fixed_to_double(dx), wl_fixed_to_double(dy), paired ? 1 : 0);
}

static void swipe_end(void *data, struct zwp_pointer_gesture_swipe_v1 *g,
		uint32_t serial, uint32_t time, int32_t cancelled) {
	(void)g; (void)serial; (void)time;
	struct client *c = data;
	c->n_swipe_end++;
	bool paired = flight_matches(c, FLIGHT_SWIPE);
	/* Cleared whether or not it paired. An app that refused to let go of a
	 * gesture because the end looked odd would be the very latch this test
	 * is about, implemented in the witness. */
	c->flight = FLIGHT_NONE;
	fprintf(g_out, "IN swipe_end cancelled=%d paired=%d\n", cancelled, paired ? 1 : 0);
}

static const struct zwp_pointer_gesture_swipe_v1_listener swipe_listener = {
	.begin = swipe_begin,
	.update = swipe_update,
	.end = swipe_end,
};

/* ---- pinch ------------------------------------------------------------ */

static void pinch_begin(void *data, struct zwp_pointer_gesture_pinch_v1 *g,
		uint32_t serial, uint32_t time, struct wl_surface *surface, uint32_t fingers) {
	(void)g; (void)serial; (void)time; (void)surface;
	struct client *c = data;
	c->n_pinch_begin++;
	begin_flight(c, FLIGHT_PINCH);
	fprintf(g_out, "IN pinch_begin fingers=%u\n", fingers);
}

static void pinch_update(void *data, struct zwp_pointer_gesture_pinch_v1 *g,
		uint32_t time, wl_fixed_t dx, wl_fixed_t dy, wl_fixed_t scale,
		wl_fixed_t rotation) {
	(void)g; (void)time;
	struct client *c = data;
	c->n_pinch_update++;
	bool paired = flight_matches(c, FLIGHT_PINCH);
	/* `scale` is absolute since the begin while the three beside it are
	 * deltas (IDL). Printed verbatim, never accumulated -- an app that
	 * multiplied successive scales would zoom toward zero, and this client
	 * exists to catch that mistake being made upstream, not to make it. */
	fprintf(g_out, "IN pinch_update dx=%.3f dy=%.3f scale=%.3f rotation=%.3f paired=%d\n",
		wl_fixed_to_double(dx), wl_fixed_to_double(dy),
		wl_fixed_to_double(scale), wl_fixed_to_double(rotation), paired ? 1 : 0);
}

static void pinch_end(void *data, struct zwp_pointer_gesture_pinch_v1 *g,
		uint32_t serial, uint32_t time, int32_t cancelled) {
	(void)g; (void)serial; (void)time;
	struct client *c = data;
	c->n_pinch_end++;
	bool paired = flight_matches(c, FLIGHT_PINCH);
	c->flight = FLIGHT_NONE;
	fprintf(g_out, "IN pinch_end cancelled=%d paired=%d\n", cancelled, paired ? 1 : 0);
}

static const struct zwp_pointer_gesture_pinch_v1_listener pinch_listener = {
	.begin = pinch_begin,
	.update = pinch_update,
	.end = pinch_end,
};

/* ---- pointer constraints ---------------------------------------------- */

static void locked(void *data, struct zwp_locked_pointer_v1 *l) {
	(void)l;
	struct client *c = data;
	c->n_locked++;
	fprintf(g_out, "IN pointer_locked\n");
}

static void unlocked(void *data, struct zwp_locked_pointer_v1 *l) {
	(void)l;
	struct client *c = data;
	c->n_unlocked++;
	fprintf(g_out, "IN pointer_unlocked\n");
}

static const struct zwp_locked_pointer_v1_listener lock_listener = {
	.locked = locked,
	.unlocked = unlocked,
};

/* Make the ask, once, if everything it needs exists yet.
 *
 * PERSISTENT rather than ONESHOT because that is the lifetime whose
 * reactivation rule the core's derived design exists for -- a lock that came
 * back when the human switched realms back is the case a stored flag gets
 * wrong. Idempotent by the `c->lock == NULL` guard, so the two callers (the
 * first `wl_pointer.enter`, and the run loop under `--lock-on-map`) can both
 * fire without a second `zwp_locked_pointer_v1` ever being created -- which
 * would be a different test, since the shim answers a second ask by
 * superseding the first. */
static void request_lock(struct client *c) {
	if (c->lock != NULL || c->constraints == NULL || c->pointer == NULL ||
			c->surface == NULL) {
		return;
	}
	c->lock = zwp_pointer_constraints_v1_lock_pointer(c->constraints, c->surface,
		c->pointer, NULL, ZWP_POINTER_CONSTRAINTS_V1_LIFETIME_PERSISTENT);
	if (c->lock != NULL) {
		zwp_locked_pointer_v1_add_listener(c->lock, &lock_listener, c);
		fprintf(g_out, "IN lock_requested tag=%s\n", c->tag);
		fflush(g_out);
	}
}

/* ---- pointer ---------------------------------------------------------- */

static void ptr_enter(void *data, struct wl_pointer *p, uint32_t serial,
		struct wl_surface *surface, wl_fixed_t sx, wl_fixed_t sy) {
	(void)p; (void)serial; (void)surface;
	struct client *c = data;
	c->n_enter++;
	fprintf(g_out, "IN pointer_enter sx=%.3f sy=%.3f\n",
		wl_fixed_to_double(sx), wl_fixed_to_double(sy));
	/* The ask, made where a real app makes it: the pointer is over the
	 * surface, so there is something to lock it to. */
	if (c->want_lock) {
		request_lock(c);
	}
}

static void ptr_leave(void *data, struct wl_pointer *p, uint32_t serial,
		struct wl_surface *surface) {
	(void)p; (void)serial; (void)surface;
	struct client *c = data;
	c->n_leave++;
	fprintf(g_out, "IN pointer_leave\n");
}

static void ptr_motion(void *data, struct wl_pointer *p, uint32_t time,
		wl_fixed_t sx, wl_fixed_t sy) {
	(void)p; (void)time;
	struct client *c = data;
	c->n_motion++;
	fprintf(g_out, "IN pointer_motion sx=%.3f sy=%.3f\n",
		wl_fixed_to_double(sx), wl_fixed_to_double(sy));
}

static void ptr_button(void *data, struct wl_pointer *p, uint32_t serial,
		uint32_t time, uint32_t button, uint32_t state) {
	(void)data; (void)p; (void)serial; (void)time;
	fprintf(g_out, "IN pointer_button button=%u state=%u\n", button, state);
}

static void ptr_axis(void *data, struct wl_pointer *p, uint32_t time,
		uint32_t axis, wl_fixed_t value) {
	(void)data; (void)p; (void)time;
	fprintf(g_out, "IN pointer_axis axis=%u value=%.3f\n", axis, wl_fixed_to_double(value));
}

static void ptr_frame(void *data, struct wl_pointer *p) {
	(void)data; (void)p;
}

static void ptr_axis_source(void *data, struct wl_pointer *p, uint32_t source) {
	(void)data; (void)p; (void)source;
}

static void ptr_axis_stop(void *data, struct wl_pointer *p, uint32_t time, uint32_t axis) {
	(void)data; (void)p; (void)time; (void)axis;
}

static void ptr_axis_discrete(void *data, struct wl_pointer *p, uint32_t axis,
		int32_t discrete) {
	(void)data; (void)p; (void)axis; (void)discrete;
}

static void ptr_axis_value120(void *data, struct wl_pointer *p, uint32_t axis,
		int32_t value120) {
	(void)data; (void)p; (void)axis; (void)value120;
}

static void ptr_axis_relative_direction(void *data, struct wl_pointer *p,
		uint32_t axis, uint32_t direction) {
	(void)data; (void)p; (void)axis; (void)direction;
}

static const struct wl_pointer_listener pointer_listener = {
	.enter = ptr_enter,
	.leave = ptr_leave,
	.motion = ptr_motion,
	.button = ptr_button,
	.axis = ptr_axis,
	.frame = ptr_frame,
	.axis_source = ptr_axis_source,
	.axis_stop = ptr_axis_stop,
	.axis_discrete = ptr_axis_discrete,
	.axis_value120 = ptr_axis_value120,
	.axis_relative_direction = ptr_axis_relative_direction,
};

/* ---- seat ------------------------------------------------------------- */

/* The three extension objects all hang off `wl_pointer`, so none of them can
 * be created until the seat has announced one. Called from both the
 * capabilities handler and the registry handler, because the two arrive in
 * whichever order the compositor chooses and the objects need both halves. */
static void arm_pointer_extensions(struct client *c) {
	if (c->pointer == NULL) {
		return;
	}
	if (c->relative == NULL && c->relative_manager != NULL) {
		c->relative = zwp_relative_pointer_manager_v1_get_relative_pointer(
			c->relative_manager, c->pointer);
		zwp_relative_pointer_v1_add_listener(c->relative, &relative_listener, c);
	}
	if (c->swipe == NULL && c->gestures != NULL) {
		c->swipe = zwp_pointer_gestures_v1_get_swipe_gesture(c->gestures, c->pointer);
		zwp_pointer_gesture_swipe_v1_add_listener(c->swipe, &swipe_listener, c);
		c->pinch = zwp_pointer_gestures_v1_get_pinch_gesture(c->gestures, c->pointer);
		zwp_pointer_gesture_pinch_v1_add_listener(c->pinch, &pinch_listener, c);
	}
}

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
	struct client *c = data;
	fprintf(g_out, "IN seat_capabilities caps=0x%x\n", caps);
	if ((caps & WL_SEAT_CAPABILITY_POINTER) != 0 && c->pointer == NULL) {
		c->pointer = wl_seat_get_pointer(seat);
		wl_pointer_add_listener(c->pointer, &pointer_listener, c);
	}
	arm_pointer_extensions(c);
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {
	(void)data; (void)seat; (void)name;
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

/* ---- surface bring-up ------------------------------------------------- */

static void wm_base_ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {.ping = wm_base_ping};

static void xdg_surface_configure(void *data, struct xdg_surface *xdg, uint32_t serial) {
	struct client *c = data;
	xdg_surface_ack_configure(xdg, serial);
	c->configured = true;
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

static void toplevel_configure(void *data, struct xdg_toplevel *t, int32_t w, int32_t h,
		struct wl_array *states) {
	(void)t; (void)states;
	struct client *c = data;
	if (w > 0 && h > 0) {
		c->width = w;
		c->height = h;
	}
}

static void toplevel_close(void *data, struct xdg_toplevel *t) {
	(void)data; (void)t;
	g_stop = 1;
}

static const struct xdg_toplevel_listener toplevel_listener = {
	.configure = toplevel_configure,
	.close = toplevel_close,
};

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
		const char *iface, uint32_t version) {
	struct client *c = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		c->compositor = wl_registry_bind(reg, name, &wl_compositor_interface,
			version > 4 ? 4 : version);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		c->shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		c->wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
		xdg_wm_base_add_listener(c->wm_base, &wm_base_listener, c);
	} else if (strcmp(iface, wl_seat_interface.name) == 0) {
		uint32_t want = version > 9 ? 9 : version;
		c->seat = wl_registry_bind(reg, name, &wl_seat_interface, want);
		wl_seat_add_listener(c->seat, &seat_listener, c);
	} else if (strcmp(iface, zwp_relative_pointer_manager_v1_interface.name) == 0) {
		c->relative_manager = wl_registry_bind(reg, name,
			&zwp_relative_pointer_manager_v1_interface, 1);
		fprintf(g_out, "IN bound global=%s\n", iface);
	} else if (strcmp(iface, zwp_pointer_gestures_v1_interface.name) == 0) {
		/* Version 1 is the floor that carries swipe and pinch; 3 adds hold,
		 * which the wire has no event for and this client never asks for.
		 * Binding what the compositor offers up to 3 keeps the object
		 * versions a real toolkit would use. */
		c->gestures = wl_registry_bind(reg, name, &zwp_pointer_gestures_v1_interface,
			version > 3 ? 3 : version);
		fprintf(g_out, "IN bound global=%s version=%u\n", iface, version > 3 ? 3 : version);
	} else if (strcmp(iface, zwp_pointer_constraints_v1_interface.name) == 0) {
		c->constraints = wl_registry_bind(reg, name,
			&zwp_pointer_constraints_v1_interface, 1);
		fprintf(g_out, "IN bound global=%s\n", iface);
	}
	arm_pointer_extensions(c);
}

static void registry_global_remove(void *data, struct wl_registry *reg, uint32_t name) {
	(void)data; (void)reg; (void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

/* A trivial opaque buffer: the point is a mapped surface for the shim's hit
 * test to find. The pixels are not under test here. */
static bool make_buffer(struct client *c) {
	size_t stride = (size_t)c->width * 4;
	size_t size = stride * (size_t)c->height;
	int fd = memfd_create("gesture-probe", MFD_CLOEXEC);
	if (fd < 0 || ftruncate(fd, (off_t)size) != 0) {
		if (fd >= 0) { close(fd); }
		return false;
	}
	uint32_t *px = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (px == MAP_FAILED) {
		close(fd);
		return false;
	}
	for (size_t i = 0; i < size / 4; i++) {
		px[i] = 0xff2050a0u;
	}
	munmap(px, size);
	struct wl_shm_pool *pool = wl_shm_create_pool(c->shm, fd, (int32_t)size);
	c->buffer = wl_shm_pool_create_buffer(pool, 0, c->width, c->height,
		(int32_t)stride, WL_SHM_FORMAT_XRGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);
	return c->buffer != NULL;
}

static int64_t now_ms(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

int main(int argc, char **argv) {
	int run_ms = 4000;
	bool want_lock = false;
	bool lock_on_map = false;
	const char *tag = "app";
	const char *out_path = NULL;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--lock-pointer") == 0) {
			want_lock = true;
		} else if (strcmp(argv[i], "--lock-on-map") == 0) {
			lock_on_map = true;
		} else if (strcmp(argv[i], "--tag") == 0 && i + 1 < argc) {
			tag = argv[++i];
		} else if (strcmp(argv[i], "--out") == 0 && i + 1 < argc) {
			out_path = argv[++i];
		} else {
			fprintf(stderr,
				"usage: gesture-probe [--run-ms MS] [--lock-pointer] "
				"[--lock-on-map] [--tag NAME] [--out PATH]\n");
			return 2;
		}
	}

	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	g_out = stdout;
	if (out_path != NULL) {
		g_out = fopen(out_path, "w");
		if (g_out == NULL) {
			/* Refused, never demoted to stdout. A gate that asked for a
			 * private file and silently got the shared stream back would
			 * be reading exactly the spliced text the file exists to
			 * avoid, and would blame the compositor for it. */
			fprintf(stderr, "gesture-probe: cannot open --out %s\n", out_path);
			return 1;
		}
	}
	/* LINE buffered either way: one `write()` per line, so a reader can never
	 * see half of one. On the private file that is all it takes; on stdout it
	 * narrows this program's own contribution to the splicing described at the
	 * top of this file, which is the most it can do about a stream whose other
	 * writers it does not control. */
	setvbuf(g_out, NULL, _IOLBF, 0);

	struct client c = {.width = 320, .height = 240, .want_lock = want_lock,
		.lock_on_map = lock_on_map, .tag = tag};

	c.display = wl_display_connect(NULL);
	if (c.display == NULL) {
		fprintf(stderr, "cannot connect to the Wayland display\n");
		return 1;
	}
	struct wl_registry *registry = wl_display_get_registry(c.display);
	wl_registry_add_listener(registry, &registry_listener, &c);
	wl_display_roundtrip(c.display); /* globals */
	wl_display_roundtrip(c.display); /* seat capabilities */

	if (c.compositor == NULL || c.shm == NULL || c.wm_base == NULL || c.seat == NULL) {
		fprintf(stderr, "the compositor is missing a required global\n");
		return 1;
	}
	/* Named rather than assumed. A shim that stopped advertising one of the
	 * three would otherwise show up as "the app received nothing", which is
	 * indistinguishable from a routing bug in the core -- and the gate would
	 * blame the wrong half. */
	fprintf(g_out, "IN globals relative=%d gestures=%d constraints=%d\n",
		c.relative_manager != NULL, c.gestures != NULL, c.constraints != NULL);

	c.surface = wl_compositor_create_surface(c.compositor);
	c.xdg_surface = xdg_wm_base_get_xdg_surface(c.wm_base, c.surface);
	xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c);
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
	xdg_toplevel_set_title(c.toplevel, "vitrin-gesture-probe");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.GestureProbe");
	wl_surface_commit(c.surface);

	while (!c.configured && wl_display_dispatch(c.display) != -1) {
		/* wait for the first configure */
	}
	if (!make_buffer(&c)) {
		fprintf(stderr, "cannot allocate the surface buffer\n");
		return 1;
	}
	wl_surface_attach(c.surface, c.buffer, 0, 0);
	wl_surface_damage(c.surface, 0, 0, c.width, c.height);
	wl_surface_commit(c.surface);
	wl_display_roundtrip(c.display);
	fprintf(g_out, "IN mapped tag=%s width=%d height=%d lock=%d lock_on_map=%d\n", c.tag,
		c.width, c.height, want_lock, lock_on_map);
	fflush(g_out);

	int64_t deadline = now_ms() + run_ms;
	while (!g_stop && now_ms() < deadline) {
		/* Retried rather than asked once after the map: the seat's
		 * capabilities and the constraints global arrive in whatever order
		 * the compositor chooses, and `request_lock` is a no-op until both
		 * are here and idempotent afterwards. */
		if (c.lock_on_map) {
			request_lock(&c);
		}
		/* The standard prepare/read/dispatch dance, so the loop can poll a
		 * timeout instead of blocking forever on a quiet compositor. */
		while (wl_display_prepare_read(c.display) != 0) {
			wl_display_dispatch_pending(c.display);
		}
		wl_display_flush(c.display);
		struct pollfd pfd = {.fd = wl_display_get_fd(c.display), .events = POLLIN};
		int remaining = (int)(deadline - now_ms());
		int pr = poll(&pfd, 1, remaining > 100 ? 100 : (remaining > 0 ? remaining : 0));
		if (pr > 0) {
			wl_display_read_events(c.display);
		} else {
			wl_display_cancel_read(c.display);
		}
		wl_display_dispatch_pending(c.display);
		fflush(g_out);
	}

	/* `in_flight` is the razor: anything but `none` is an app left holding a
	 * gesture the compositor never ended, which is the latched-modifier
	 * failure wearing WS-E.4.2's shape. `unpaired` is its companion -- events
	 * that arrived out of a pairing the shim is supposed to enforce. */
	fprintf(g_out, "SUMMARY tag=%s enter=%llu leave=%llu motion=%llu relative=%llu "
		"swipe_begin=%llu swipe_update=%llu swipe_end=%llu "
		"pinch_begin=%llu pinch_update=%llu pinch_end=%llu "
		"locked=%llu unlocked=%llu in_flight=%s unpaired=%llu\n",
		c.tag, (unsigned long long)c.n_enter, (unsigned long long)c.n_leave,
		(unsigned long long)c.n_motion, (unsigned long long)c.n_relative,
		(unsigned long long)c.n_swipe_begin, (unsigned long long)c.n_swipe_update,
		(unsigned long long)c.n_swipe_end, (unsigned long long)c.n_pinch_begin,
		(unsigned long long)c.n_pinch_update, (unsigned long long)c.n_pinch_end,
		(unsigned long long)c.n_locked, (unsigned long long)c.n_unlocked,
		flight_name(c.flight), (unsigned long long)c.n_unpaired);
	fflush(g_out);
	if (g_out != stdout) {
		fclose(g_out);
	}

	wl_display_disconnect(c.display);
	return 0;
}
