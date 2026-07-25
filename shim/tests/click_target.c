/* click_target.c -- a deterministic Wayland client that visibly responds to a
 * pointer click landing on a known feature, for the P1.8.6 agent-actuation
 * gate (issue #108).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS EXISTS. The M1.4 gate asks that an agent's injected pointer click
 * lands on a REAL app surface at the coordinate the agent observed, and that
 * the landing is confirmed by the agent's own observe() -- decision D10, the
 * "click on surface" proof deferred out of the broken #43 demo. weston-terminal
 * and gtk-entry-probe respond to a click only subtly (a focus ring, a blinking
 * cursor); neither gives a clean, GPU-free, whole-frame colour change an
 * observe() can assert without eyeballing. This client is built for exactly
 * that:
 *
 *   before a hit   a solid BACKGROUND with one TARGET square at a known place
 *   on a hit        the WHOLE surface repaints to a distinct HIT colour
 *
 * so the proof is unambiguous: the agent captures the frame, LOCATES the target
 * by its colour, clicks its centroid (a realm-view coordinate the shim maps to
 * surface-local, D10), and observes the dominant colour flip to the hit colour.
 * A click anywhere else does nothing, so a flip means the click genuinely
 * landed on the observed feature. It also prints `HIT sx=.. sy=..` (the
 * surface-local coordinate it received) so the "the real app demonstrably
 * received it" half is confirmable off the app's own report, and the coordinate
 * mapping is legible.
 *
 * All three colours use channels that are multiples of 0x11 so the capture's
 * 4-bit dominant-colour histogram reads them back exactly. It is a bare
 * wl_shm + xdg-shell + wl_pointer client -- no toolkit -- so the response is
 * exactly the bytes it wrote, and it follows solid_client.c's buffer handling
 * and input_echo_client.c's pointer listener so it cannot drift from either.
 */
#define _GNU_SOURCE

#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include <linux/input-event-codes.h>
#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

#define BUFFER_COUNT 2

/* Channels ×0x11 so the dominant-colour histogram reads them back exactly. */
#define BACKGROUND 0x000000u /* black: the pre-click dominant colour */
#define TARGET 0x00ff00u     /* green: the feature the agent locates + clicks */
#define HIT 0xff0000u        /* red: the whole-surface response to a landed click */

/* The target square's edge, in pixels. Large enough for the agent to locate by
 * colour and click near-centre with margin; small enough that BACKGROUND, not
 * TARGET, dominates the pre-click frame. */
#define TARGET_SIZE 160

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

static inline uint32_t pack(uint32_t rgb) {
	return 0xff000000u | rgb; /* XRGB8888, opaque */
}

struct buffer {
	struct wl_buffer *wl;
	uint32_t *pixels;
	size_t size;
	bool busy;
};

struct client {
	struct wl_display *display;
	struct wl_registry *registry;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	struct wl_seat *seat;
	struct wl_pointer *pointer;

	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;

	int width, height;
	bool configured;
	bool closed;

	/* The target rectangle, centred once the configured size is known. */
	int tx0, ty0, tx1, ty1;

	/* Latest surface-local pointer position (from enter/motion), and whether a
	 * click has landed on the target. `hit` latches: the first landed click
	 * repaints and the response is stable thereafter. */
	int px, py;
	bool have_pos;
	bool hit;

	struct buffer buffers[BUFFER_COUNT];
	bool buffers_ready;

	uint64_t commits;
};

/* ---- drawing --------------------------------------------------------- */

static void fill(uint32_t *pixels, size_t count, uint32_t px) {
	for (size_t i = 0; i < count; i++) {
		pixels[i] = px;
	}
}

/* Paint one buffer for the current state: before a hit, BACKGROUND with the
 * TARGET square; after a hit, the whole surface in HIT. */
static void paint(struct client *c, struct buffer *b) {
	size_t count = (size_t)c->width * (size_t)c->height;
	if (c->hit) {
		fill(b->pixels, count, pack(HIT));
		return;
	}
	fill(b->pixels, count, pack(BACKGROUND));
	uint32_t t = pack(TARGET);
	for (int y = c->ty0; y < c->ty1; y++) {
		uint32_t *row = b->pixels + (size_t)y * (size_t)c->width;
		for (int x = c->tx0; x < c->tx1; x++) {
			row[x] = t;
		}
	}
}

/* ---- registry -------------------------------------------------------- */

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps);
static void seat_name(void *data, struct wl_seat *seat, const char *name) {
	(void)data;
	(void)seat;
	(void)name;
}
static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
		const char *iface, uint32_t version) {
	struct client *c = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		uint32_t want = version < 4 ? 4 : version;
		c->compositor = wl_registry_bind(reg, name, &wl_compositor_interface, want > 6 ? 6 : want);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		c->shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		c->wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
	} else if (strcmp(iface, wl_seat_interface.name) == 0) {
		uint32_t want = version < 5 ? version : 5;
		c->seat = wl_registry_bind(reg, name, &wl_seat_interface, want);
		wl_seat_add_listener(c->seat, &seat_listener, c);
	}
}

static void registry_global_remove(void *data, struct wl_registry *reg, uint32_t name) {
	(void)data;
	(void)reg;
	(void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

/* ---- pointer --------------------------------------------------------- */

static inline bool in_target(const struct client *c, int x, int y) {
	return x >= c->tx0 && x < c->tx1 && y >= c->ty0 && y < c->ty1;
}

static void ptr_enter(void *data, struct wl_pointer *p, uint32_t serial,
		struct wl_surface *surface, wl_fixed_t sx, wl_fixed_t sy) {
	(void)p;
	(void)serial;
	(void)surface;
	struct client *c = data;
	c->px = (int)wl_fixed_to_double(sx);
	c->py = (int)wl_fixed_to_double(sy);
	c->have_pos = true;
}

static void ptr_leave(void *data, struct wl_pointer *p, uint32_t serial,
		struct wl_surface *surface) {
	(void)p;
	(void)serial;
	(void)surface;
	((struct client *)data)->have_pos = false;
}

static void ptr_motion(void *data, struct wl_pointer *p, uint32_t time,
		wl_fixed_t sx, wl_fixed_t sy) {
	(void)p;
	(void)time;
	struct client *c = data;
	c->px = (int)wl_fixed_to_double(sx);
	c->py = (int)wl_fixed_to_double(sy);
	c->have_pos = true;
}

static void ptr_button(void *data, struct wl_pointer *p, uint32_t serial,
		uint32_t time, uint32_t button, uint32_t state) {
	(void)p;
	(void)serial;
	(void)time;
	struct client *c = data;
	/* A landed left-press ON the target is the event the gate is about. The
	 * latch means a stray later click cannot un-hit; the response is stable for
	 * the agent to observe. Coordinates come from the tracked enter/motion
	 * position -- wl_pointer.button carries none. */
	if (state == WL_POINTER_BUTTON_STATE_PRESSED && button == BTN_LEFT && c->have_pos &&
			!c->hit && in_target(c, c->px, c->py)) {
		c->hit = true;
		printf("HIT sx=%d sy=%d\n", c->px, c->py);
		fflush(stdout);
	}
}

static void ptr_axis(void *data, struct wl_pointer *p, uint32_t time,
		uint32_t axis, wl_fixed_t value) {
	(void)data;
	(void)p;
	(void)time;
	(void)axis;
	(void)value;
}
static void ptr_frame(void *data, struct wl_pointer *p) {
	(void)data;
	(void)p;
}
static void ptr_axis_source(void *data, struct wl_pointer *p, uint32_t source) {
	(void)data;
	(void)p;
	(void)source;
}
static void ptr_axis_stop(void *data, struct wl_pointer *p, uint32_t time, uint32_t axis) {
	(void)data;
	(void)p;
	(void)time;
	(void)axis;
}
static void ptr_axis_discrete(void *data, struct wl_pointer *p, uint32_t axis, int32_t discrete) {
	(void)data;
	(void)p;
	(void)axis;
	(void)discrete;
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
};

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
	struct client *c = data;
	if ((caps & WL_SEAT_CAPABILITY_POINTER) != 0 && c->pointer == NULL) {
		c->pointer = wl_seat_get_pointer(seat);
		wl_pointer_add_listener(c->pointer, &pointer_listener, c);
	}
}

/* ---- xdg-shell ------------------------------------------------------- */

static void wm_base_ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {.ping = wm_base_ping};

static void toplevel_configure(void *data, struct xdg_toplevel *tl, int32_t width,
		int32_t height, struct wl_array *states) {
	(void)tl;
	(void)states;
	struct client *c = data;
	if (width > 0 && height > 0) {
		c->width = width;
		c->height = height;
	}
}

static void toplevel_close(void *data, struct xdg_toplevel *tl) {
	(void)tl;
	((struct client *)data)->closed = true;
}

static void toplevel_configure_bounds(void *data, struct xdg_toplevel *tl,
		int32_t width, int32_t height) {
	(void)data;
	(void)tl;
	(void)width;
	(void)height;
}

static void toplevel_wm_capabilities(void *data, struct xdg_toplevel *tl,
		struct wl_array *caps) {
	(void)data;
	(void)tl;
	(void)caps;
}

static const struct xdg_toplevel_listener toplevel_listener = {
	.configure = toplevel_configure,
	.close = toplevel_close,
	.configure_bounds = toplevel_configure_bounds,
	.wm_capabilities = toplevel_wm_capabilities,
};

static void xdg_surface_configure(void *data, struct xdg_surface *xs, uint32_t serial) {
	struct client *c = data;
	xdg_surface_ack_configure(xs, serial);
	c->configured = true;
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

/* ---- buffers --------------------------------------------------------- */

static void buffer_release(void *data, struct wl_buffer *wl) {
	(void)wl;
	((struct buffer *)data)->busy = false;
}

static const struct wl_buffer_listener buffer_listener = {.release = buffer_release};

static bool buffers_create(struct client *c) {
	size_t stride = (size_t)c->width * 4;
	size_t size = stride * (size_t)c->height;
	size_t total = size * BUFFER_COUNT;

	int fd = memfd_create("click-target", MFD_CLOEXEC);
	if (fd < 0 || ftruncate(fd, (off_t)total) != 0) {
		perror("memfd");
		return false;
	}
	void *base = mmap(NULL, total, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (base == MAP_FAILED) {
		perror("mmap");
		close(fd);
		return false;
	}
	struct wl_shm_pool *pool = wl_shm_create_pool(c->shm, fd, (int32_t)total);
	for (int i = 0; i < BUFFER_COUNT; i++) {
		c->buffers[i].wl = wl_shm_pool_create_buffer(pool, (int32_t)(size * (size_t)i),
			c->width, c->height, (int32_t)stride, WL_SHM_FORMAT_XRGB8888);
		c->buffers[i].pixels = (uint32_t *)((char *)base + size * (size_t)i);
		c->buffers[i].size = size;
		c->buffers[i].busy = false;
		wl_buffer_add_listener(c->buffers[i].wl, &buffer_listener, &c->buffers[i]);
	}
	wl_shm_pool_destroy(pool);
	close(fd);
	c->buffers_ready = true;
	return true;
}

static struct buffer *buffer_take(struct client *c) {
	for (int i = 0; i < BUFFER_COUNT; i++) {
		if (!c->buffers[i].busy) {
			return &c->buffers[i];
		}
	}
	return NULL;
}

/* ---- the draw loop --------------------------------------------------- */

static void draw(struct client *c);

static void frame_done(void *data, struct wl_callback *cb, uint32_t time) {
	(void)time;
	wl_callback_destroy(cb);
	/* Re-commit each cadence: the content only changes at the one hit, so this
	 * keeps the surface live and picks up the post-hit repaint on the next
	 * frame after the click lands. */
	draw(data);
}

static const struct wl_callback_listener frame_listener = {.done = frame_done};

static void draw(struct client *c) {
	if (g_stop || c->closed) {
		return;
	}
	struct buffer *b = buffer_take(c);
	if (b == NULL) {
		struct wl_callback *cb = wl_surface_frame(c->surface);
		wl_callback_add_listener(cb, &frame_listener, c);
		wl_surface_commit(c->surface);
		return;
	}
	paint(c, b);
	wl_surface_attach(c->surface, b->wl, 0, 0);
	wl_surface_damage_buffer(c->surface, 0, 0, c->width, c->height);
	b->busy = true;
	c->commits++;

	struct wl_callback *cb = wl_surface_frame(c->surface);
	wl_callback_add_listener(cb, &frame_listener, c);
	wl_surface_commit(c->surface);
}

int main(int argc, char **argv) {
	int run_ms = 4000;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else {
			fprintf(stderr, "usage: %s [--run-ms MS]\n", argv[0]);
			return 2;
		}
	}
	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	struct client c = {.width = 0, .height = 0};
	c.display = wl_display_connect(NULL);
	if (c.display == NULL) {
		fprintf(stderr, "cannot connect to the Wayland display\n");
		return 1;
	}
	c.registry = wl_display_get_registry(c.display);
	wl_registry_add_listener(c.registry, &registry_listener, &c);
	wl_display_roundtrip(c.display);

	if (c.compositor == NULL || c.shm == NULL || c.wm_base == NULL) {
		fprintf(stderr, "the compositor is missing wl_compositor, wl_shm or xdg_wm_base\n");
		return 1;
	}
	if (c.seat == NULL) {
		fprintf(stderr, "the compositor advertised no wl_seat: no pointer to click with\n");
		return 1;
	}
	xdg_wm_base_add_listener(c.wm_base, &wm_base_listener, &c);

	c.surface = wl_compositor_create_surface(c.compositor);
	c.xdg_surface = xdg_wm_base_get_xdg_surface(c.wm_base, c.surface);
	xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c);
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
	xdg_toplevel_set_title(c.toplevel, "vitrin-click-target");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.click-target");
	wl_surface_commit(c.surface);

	/* Block until configured: the size the compositor hands back is the realm
	 * view (damage_client.c's contract), and the target square is centred in
	 * it -- so its position is a pure function of the view size the agent's
	 * capture reports, no coordinate told twice. */
	while (!c.configured && wl_display_dispatch(c.display) != -1) {
		if (g_stop) {
			return 0;
		}
	}
	if (c.width <= 0 || c.height <= 0) {
		fprintf(stderr, "the compositor configured no size\n");
		return 1;
	}
	c.tx0 = (c.width - TARGET_SIZE) / 2;
	c.ty0 = (c.height - TARGET_SIZE) / 2;
	c.tx1 = c.tx0 + TARGET_SIZE;
	c.ty1 = c.ty0 + TARGET_SIZE;
	printf("TARGET x0=%d y0=%d x1=%d y1=%d size=%dx%d\n",
		c.tx0, c.ty0, c.tx1, c.ty1, c.width, c.height);
	fflush(stdout);

	if (!buffers_create(&c)) {
		return 1;
	}

	draw(&c);

	struct timespec start;
	clock_gettime(CLOCK_MONOTONIC, &start);
	while (!g_stop && !c.closed) {
		if (wl_display_dispatch(c.display) == -1) {
			break;
		}
		struct timespec now;
		clock_gettime(CLOCK_MONOTONIC, &now);
		long elapsed = (long)(now.tv_sec - start.tv_sec) * 1000 +
			(now.tv_nsec - start.tv_nsec) / 1000000;
		if (elapsed >= run_ms) {
			break;
		}
	}

	printf("CLIENT hit=%d commits=%llu size=%dx%d\n",
		c.hit ? 1 : 0, (unsigned long long)c.commits, c.width, c.height);
	fflush(stdout);
	wl_display_disconnect(c.display);
	return 0;
}
