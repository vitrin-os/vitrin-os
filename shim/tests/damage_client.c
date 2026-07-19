/* damage_client.c -- a deterministic Wayland client for the P1.6.2
 * acceptance proof.
 *
 * weston-terminal proves the shim is usable by a real app, which is what the
 * P1.6.1 test uses it for. It cannot prove the two claims this task has to
 * make mechanically, because what it damages and how often it draws are its
 * own business:
 *
 *   - "a small repaint does not forward the full surface" needs a client
 *     whose repaint is a KNOWN small rectangle;
 *   - "the app throttles to the core's cadence" needs a client that draws as
 *     fast as it is allowed to and reports exactly how often it was allowed.
 *
 * So this client does precisely that and nothing else:
 *
 *   frame 0    fill the whole surface with a fixed color, damage all of it
 *   frame n>0  draw one 32x32 square at a moving position and damage ONLY
 *              those 32x32 pixels
 *
 * and it requests a new `wl_surface.frame` callback every frame, so its
 * frame rate is whatever the compositor chain allows -- which, with the
 * upstream link live, is the core's presentation cadence. It prints its
 * callback count on exit; comparing that against the mock core's
 * `frame_dones` is the pacing assertion.
 *
 * Two buffers with real `wl_buffer.release` tracking, because release
 * timing is the compositor's to choose and a single-buffer client would
 * either tear or deadlock depending on which it chose.
 */
#define _GNU_SOURCE

#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <poll.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

#define SQUARE 32
#define BUFFER_COUNT 2

/* --free-run commit rate; see the loop in main() for why it is capped. */
#define FREE_RUN_HZ 200

/* Fixed so the mock core's content digest is reproducible run to run. */
#define BACKGROUND 0xff204060u
#define FOREGROUND 0xffe0d0a0u

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
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

	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;

	int width, height;
	bool configured;
	bool closed;

	struct buffer buffers[BUFFER_COUNT];
	bool buffers_ready;

	/* Draw on a local timer instead of on frame callbacks -- a client that
	 * ignores the pacing it is offered. The shim must survive it without
	 * queueing frames, which is the "no unbounded buffering" criterion seen
	 * from the app's side rather than the core's. */
	bool free_run;

	uint64_t frames;  /* wl_surface.frame callbacks received */
	uint64_t commits; /* wl_surface.commit calls that carried a buffer */
};

/* ---- registry -------------------------------------------------------- */

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
		const char *iface, uint32_t version) {
	struct client *c = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		/* Version 4 is the floor: wl_surface.damage_buffer, which is the
		 * only way to declare damage in BUFFER coordinates -- exactly the
		 * space the shim forwards upstream. */
		uint32_t want = version < 4 ? 4 : version;
		c->compositor = wl_registry_bind(reg, name, &wl_compositor_interface, want > 6 ? 6 : want);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		c->shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		c->wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
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

	int fd = memfd_create("damage-client", MFD_CLOEXEC);
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
		/* Both buffers start identical, so the first frame's content is the
		 * same whichever one is picked -- which is what lets the mock core
		 * assert a fixed digest for it. */
		for (size_t p = 0; p < size / 4; p++) {
			c->buffers[i].pixels[p] = BACKGROUND;
		}
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
	struct client *c = data;
	c->frames++;
	if (!c->free_run) {
		draw(c);
	}
}

static const struct wl_callback_listener frame_listener = {.done = frame_done};

static void draw(struct client *c) {
	if (g_stop || c->closed) {
		return;
	}
	struct buffer *b = buffer_take(c);
	if (b == NULL) {
		if (c->free_run) {
			return; /* try again on the next tick of the local timer */
		}
		/* Every buffer is still held by the compositor. Ask for another
		 * callback rather than dropping out of the loop. */
		struct wl_callback *cb = wl_surface_frame(c->surface);
		wl_callback_add_listener(cb, &frame_listener, c);
		wl_surface_commit(c->surface);
		return;
	}

	if (c->commits == 0) {
		/* The full-surface frame: everything changed, so say so. */
		wl_surface_attach(c->surface, b->wl, 0, 0);
		wl_surface_damage_buffer(c->surface, 0, 0, c->width, c->height);
	} else {
		/* A small repaint: one square moves diagonally, and the damage is
		 * exactly the square. Nothing else about the surface changed, and
		 * the shim must be able to say so upstream. */
		int step = (int)(c->commits % 16);
		int x = 64 + step * SQUARE;
		int y = 64 + step * SQUARE;
		if (x + SQUARE > c->width) {
			x = c->width - SQUARE;
		}
		if (y + SQUARE > c->height) {
			y = c->height - SQUARE;
		}
		for (int row = 0; row < SQUARE; row++) {
			uint32_t *line = b->pixels + (size_t)(y + row) * (size_t)c->width + (size_t)x;
			for (int col = 0; col < SQUARE; col++) {
				line[col] = FOREGROUND ^ (uint32_t)c->commits;
			}
		}
		wl_surface_attach(c->surface, b->wl, 0, 0);
		wl_surface_damage_buffer(c->surface, x, y, SQUARE, SQUARE);
	}
	b->busy = true;
	c->commits++;

	struct wl_callback *cb = wl_surface_frame(c->surface);
	wl_callback_add_listener(cb, &frame_listener, c);
	wl_surface_commit(c->surface);
}

int main(int argc, char **argv) {
	int run_ms = 3000;
	bool free_run = false;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--free-run") == 0) {
			free_run = true;
		} else {
			fprintf(stderr, "usage: %s [--run-ms MS] [--free-run]\n", argv[0]);
			return 2;
		}
	}
	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	struct client c = {.width = 0, .height = 0, .free_run = free_run};
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
	xdg_wm_base_add_listener(c.wm_base, &wm_base_listener, &c);

	c.surface = wl_compositor_create_surface(c.compositor);
	c.xdg_surface = xdg_wm_base_get_xdg_surface(c.wm_base, c.surface);
	xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c);
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
	xdg_toplevel_set_title(c.toplevel, "vitrin-damage-client");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.damage-client");
	wl_surface_commit(c.surface);

	/* Block until the compositor has configured us: the size it hands back
	 * is the realm view, which is what makes the client's geometry the
	 * core's geometry without either end being told twice. */
	while (!c.configured && wl_display_dispatch(c.display) != -1) {
		if (g_stop) {
			return 0;
		}
	}
	if (c.width <= 0 || c.height <= 0) {
		fprintf(stderr, "the compositor configured no size\n");
		return 1;
	}
	if (!buffers_create(&c)) {
		return 1;
	}

	draw(&c);

	struct timespec start;
	clock_gettime(CLOCK_MONOTONIC, &start);
	struct timespec last_draw = start;
	while (!g_stop && !c.closed) {
		if (c.free_run) {
			/* Drive from a local timer, deliberately ignoring the cadence
			 * the compositor offers. Events are still read (a client that
			 * stopped reading would stall on wl_buffer.release and prove
			 * nothing about pacing); they simply do not drive the drawing.
			 *
			 * The rate is capped at FREE_RUN_HZ rather than "as fast as the
			 * CPU allows" for a protocol reason, not politeness: libwayland's
			 * client connection buffer is fixed-size, so a client that queues
			 * requests faster than the compositor drains them is killed by
			 * its OWN "Data too big for buffer" error. That kills the test,
			 * not the shim. 200Hz is twenty times the slow core's cadence --
			 * far more than enough to prove frames are coalesced rather than
			 * queued -- and stays inside what the connection can carry. */
			if (wl_display_flush(c.display) == -1) {
				break;
			}
			struct pollfd pfd = {.fd = wl_display_get_fd(c.display), .events = POLLIN};
			int pr = poll(&pfd, 1, 1);
			int rc = pr > 0 ? wl_display_dispatch(c.display)
				: wl_display_dispatch_pending(c.display);
			if (rc == -1) {
				break;
			}
			struct timespec t;
			clock_gettime(CLOCK_MONOTONIC, &t);
			long since_us = (long)(t.tv_sec - last_draw.tv_sec) * 1000000 +
				(t.tv_nsec - last_draw.tv_nsec) / 1000;
			if (since_us >= 1000000 / FREE_RUN_HZ) {
				last_draw = t;
				draw(&c);
			}
		} else if (wl_display_dispatch(c.display) == -1) {
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

	printf("CLIENT frames=%llu commits=%llu size=%dx%d\n",
		(unsigned long long)c.frames, (unsigned long long)c.commits,
		c.width, c.height);
	fflush(stdout);
	wl_display_disconnect(c.display);
	return 0;
}
