/* solid_client.c -- a deterministic Wayland client that paints one known
 * solid colour over its whole surface, for the P1.8.5 real-app capture gate
 * (issue #107).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS EXISTS, next to damage_client.c and gtk_entry_probe.c. The M1.3
 * exit gate asks two things of a REAL app captured through the real chain:
 *
 *   - its `observe()` frame's DOMINANT COLOUR equals the colour the app
 *     rendered (criterion 2), and
 *   - that frame agrees, by SSIM, with the core-internal capture (criterion
 *     3) -- the "grant path adds no distortion" proof against a real app.
 *
 * weston-terminal and gtk-entry-probe both render CHROME -- glyphs, borders,
 * a cursor -- so "the dominant colour is #RRGGBB over enough of the view" is
 * theme- and font-dependent, and a terminal at rest is a poor static scene
 * for an SSIM that wants two captures of ONE unchanging frame. This client is
 * the opposite: it fills the entire realm view with a single colour whose
 * channels are multiples of 0x11 (so they survive the capture's 4-bit
 * dominant-colour histogram exactly), and it never animates, so every
 * composited frame is byte-identical. That makes it the clean rung for both
 * criteria and a static scene the SSIM reads back as ~1.0.
 *
 * It is a bare wl_shm + xdg-shell client -- no toolkit -- so the "known
 * colour" is exactly the bytes it wrote, with no client-side decoration or
 * antialiasing to erode the dominant fraction or add off-palette edges. It
 * follows damage_client.c's structure (two buffers with real release
 * tracking, configure-driven geometry) so the two clients cannot drift in how
 * they speak to the shim; the only thing new here is "paint one colour".
 *
 *   CLIENT colour=RRGGBB size=WxH   printed on exit
 *
 * THE CONFINEMENT PROBE (--probe / --probe-out), added by P2.6.2 (issue #186).
 * Opt-in and inert without the flags, so every gate that already runs this app
 * is unchanged. With them, the client `open()`s each named path BEFORE it
 * touches Wayland and writes what the kernel said into a report file.
 *
 * Why it lives here rather than in a fourth Wayland client: the *Wayland*
 * behaviour a confinement gate needs is exactly this file's -- attach, paint
 * one colour, stay alive -- and the repo already carries three copies of that
 * boilerplate whose only stated justification is that they speak to the shim
 * DIFFERENTLY. A fourth copy differing in nothing would be the drift those
 * files' headers warn about. What is new is twelve lines of `open` + `fstat`.
 *
 * Why it reports (st_dev, st_ino) and not just success: "the canary is
 * absent" and "a same-named empty stub is present" are different facts, and
 * the mount table *creates* stubs -- `vitrin-realm-init` binds the app's own
 * directory at its host path, which `mkdir -p`s every ancestor onto the
 * realm's root tmpfs, so `$HOME` itself exists inside the realm as an empty
 * directory. Only inode identity separates "reachable" from "same name".
 * `crates/vitrin-core/src/spawn.rs`'s parent-side canary check learned this
 * the hard way and says so in its own comment.
 *
 * Why BEFORE the Wayland connection: so a report exists even when the
 * compositor never comes up, which is what stops "the probe never ran" from
 * being indistinguishable from "the probe ran and found nothing".
 *
 *   PROBE-VERSION 1
 *   PROBE-GROUPS n=<count> gids=<g,g,...>
 *   PROBE path=<abs> open=<ok|fail> errno=<n> dev=<u> ino=<u>   (one per --probe)
 *   PROBE-END
 */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <inttypes.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

#define BUFFER_COUNT 2

/* Enough for every path a gate has reason to probe in one run; a request for
 * more is refused rather than silently truncated, because a probe list that
 * quietly lost an entry would read as an absence that was never tested. */
#define MAX_PROBES 8

/* Default colour: pure blue. Each channel is a multiple of 0x11 (00, 00, ff),
 * so the capture's top-nibble dominant-colour histogram reads it back exactly
 * -- the same discipline the Firefox solid page uses (#0000ff). */
#define DEFAULT_COLOUR 0x0000ffu

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

	/* The 0x00RRGGBB the client was asked to paint, and the packed XRGB8888
	 * pixel it fills with (opaque). */
	uint32_t rgb;
	uint32_t pixel;

	struct buffer buffers[BUFFER_COUNT];
	bool buffers_ready;

	uint64_t commits;
};

/* ---- registry -------------------------------------------------------- */

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

	int fd = memfd_create("solid-client", MFD_CLOEXEC);
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
		/* Both buffers hold the SAME solid colour, so which one the compositor
		 * hands back is irrelevant and every composited frame is identical --
		 * exactly the static scene the SSIM proof wants. */
		for (size_t p = 0; p < size / 4; p++) {
			c->buffers[i].pixels[p] = c->pixel;
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
	/* Re-commit the same solid frame on the cadence the compositor offers.
	 * The content never changes, so this keeps the surface live (and answers
	 * the frame clock) without ever altering a pixel. */
	draw(data);
}

static const struct wl_callback_listener frame_listener = {.done = frame_done};

static void draw(struct client *c) {
	if (g_stop || c->closed) {
		return;
	}
	struct buffer *b = buffer_take(c);
	if (b == NULL) {
		/* Both buffers still held; ask for another callback rather than
		 * dropping out of the frame loop. */
		struct wl_callback *cb = wl_surface_frame(c->surface);
		wl_callback_add_listener(cb, &frame_listener, c);
		wl_surface_commit(c->surface);
		return;
	}
	wl_surface_attach(c->surface, b->wl, 0, 0);
	wl_surface_damage_buffer(c->surface, 0, 0, c->width, c->height);
	b->busy = true;
	c->commits++;

	struct wl_callback *cb = wl_surface_frame(c->surface);
	wl_callback_add_listener(cb, &frame_listener, c);
	wl_surface_commit(c->surface);
}

/* Parse a six-hex-digit RRGGBB colour into 0x00RRGGBB. Returns false on any
 * malformed input -- a client asked for a colour it cannot paint must fail
 * loudly, not silently paint black. */
static bool parse_colour(const char *s, uint32_t *out) {
	if (strlen(s) != 6) {
		return false;
	}
	char *end = NULL;
	unsigned long v = strtoul(s, &end, 16);
	if (end == NULL || *end != '\0') {
		return false;
	}
	*out = (uint32_t)v & 0x00ffffffu;
	return true;
}

/* One line of the confinement report for `path`, appended to `out`.
 *
 * `O_RDONLY` on a directory is legal and is what makes one probe verb serve
 * both a canary FILE in $HOME and a canary DIRECTORY like /dev/input. The
 * `fstat` is on the descriptor the `open` returned, never a second lookup by
 * name -- so what is reported is the identity of the object this process
 * actually opened. `O_NOCTTY` because one of the interesting probes is a tty
 * device and a client that acquired a controlling terminal by probing for one
 * would be reporting on a state it created. */
static void probe_one(FILE *out, const char *path) {
	int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOCTTY);
	if (fd < 0) {
		fprintf(out, "PROBE path=%s open=fail errno=%d dev=0 ino=0\n", path, errno);
		return;
	}
	struct stat st;
	if (fstat(fd, &st) != 0) {
		int e = errno;
		close(fd);
		/* Opened but not stattable: reported as its own outcome rather than
		 * folded into either side, because a reader must not take it for
		 * "unreachable". */
		fprintf(out, "PROBE path=%s open=nostat errno=%d dev=0 ino=0\n", path, e);
		return;
	}
	close(fd);
	fprintf(out, "PROBE path=%s open=ok errno=0 dev=%" PRIu64 " ino=%" PRIu64 "\n", path,
			(uint64_t)st.st_dev, (uint64_t)st.st_ino);
}

/* Write the whole report, and return false if anything about writing it
 * failed. A probe whose report never landed must not be mistaken for a probe
 * that ran, so the caller exits non-zero. */
static bool write_probe_report(const char *out_name, char *const *paths, int count) {
	char resolved[4096];
	if (out_name[0] == '/') {
		if ((size_t)snprintf(resolved, sizeof(resolved), "%s", out_name) >= sizeof(resolved)) {
			fprintf(stderr, "--probe-out path too long\n");
			return false;
		}
	} else {
		/* Relative names resolve against XDG_RUNTIME_DIR, which is the realm's
		 * own directory at both isolation settings and is bound read-WRITE
		 * into a confined realm. That is what lets one argv, byte for byte,
		 * serve a `--isolation=default` run and an `--isolation=off` run: the
		 * variable's VALUE differs (`/run/vitrin` vs the host path) and the
		 * host file it lands in is the same one either way. */
		const char *dir = getenv("XDG_RUNTIME_DIR");
		if (dir == NULL || dir[0] == '\0') {
			fprintf(stderr, "--probe-out is relative and XDG_RUNTIME_DIR is unset\n");
			return false;
		}
		if ((size_t)snprintf(resolved, sizeof(resolved), "%s/%s", dir, out_name) >=
				sizeof(resolved)) {
			fprintf(stderr, "--probe-out path too long\n");
			return false;
		}
	}
	int fd = open(resolved, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (fd < 0) {
		fprintf(stderr, "cannot write the probe report %s: %s\n", resolved, strerror(errno));
		return false;
	}
	FILE *out = fdopen(fd, "w");
	if (out == NULL) {
		close(fd);
		return false;
	}
	fprintf(out, "PROBE-VERSION 1\n");
	/* The supplementary groups this process still holds. D-037(5): they cannot
	 * be dropped -- `setgroups=deny` blocks the CALL and drops nothing -- so
	 * the realm keeps `video`, `render`, `input` and the rest as kgids, and
	 * the mount table is the only thing between it and the devices they open.
	 * Reported from inside so that residual is measured at the app rather than
	 * described in a document. */
	gid_t gids[NGROUPS_MAX];
	int n = getgroups(NGROUPS_MAX, gids);
	fprintf(out, "PROBE-GROUPS n=%d gids=", n);
	for (int i = 0; i < n; i++) {
		fprintf(out, "%s%u", i ? "," : "", (unsigned)gids[i]);
	}
	fprintf(out, "\n");
	for (int i = 0; i < count; i++) {
		probe_one(out, paths[i]);
	}
	/* Last, so a truncated report is detectable rather than readable. */
	fprintf(out, "PROBE-END\n");
	if (fflush(out) != 0 || fsync(fileno(out)) != 0) {
		fclose(out);
		return false;
	}
	return fclose(out) == 0;
}

int main(int argc, char **argv) {
	int run_ms = 3000;
	uint32_t rgb = DEFAULT_COLOUR;
	char *probes[MAX_PROBES];
	int probe_count = 0;
	const char *probe_out = NULL;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if ((strcmp(argv[i], "--colour") == 0 || strcmp(argv[i], "--color") == 0) &&
				i + 1 < argc) {
			if (!parse_colour(argv[++i], &rgb)) {
				fprintf(stderr, "bad colour '%s' (expected six hex digits, e.g. 0000ff)\n", argv[i]);
				return 2;
			}
		} else if (strcmp(argv[i], "--probe") == 0 && i + 1 < argc) {
			if (probe_count == MAX_PROBES) {
				fprintf(stderr, "at most %d --probe paths\n", MAX_PROBES);
				return 2;
			}
			probes[probe_count++] = argv[++i];
		} else if (strcmp(argv[i], "--probe-out") == 0 && i + 1 < argc) {
			probe_out = argv[++i];
		} else {
			fprintf(stderr,
					"usage: %s [--run-ms MS] [--colour RRGGBB] "
					"[--probe PATH]... [--probe-out FILE]\n",
					argv[0]);
			return 2;
		}
	}
	if ((probe_count > 0) != (probe_out != NULL)) {
		/* Refused rather than defaulted: a `--probe` with nowhere to report is
		 * a gate that would read an absent file as an absent canary, and a
		 * `--probe-out` with nothing to probe writes a report whose emptiness
		 * means nothing. */
		fprintf(stderr, "--probe and --probe-out are used together or not at all\n");
		return 2;
	}
	if (probe_count > 0 && !write_probe_report(probe_out, probes, probe_count)) {
		return 3;
	}
	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	struct client c = {
		.width = 0,
		.height = 0,
		.rgb = rgb,
		/* XRGB8888, opaque: little-endian memory bytes become B,G,R,X. */
		.pixel = 0xff000000u | rgb,
	};
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
	xdg_toplevel_set_title(c.toplevel, "vitrin-solid-client");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.solid-client");
	wl_surface_commit(c.surface);

	/* Block until configured: the size the compositor hands back is the realm
	 * view, so the client's geometry becomes the core's without either end
	 * being told twice (damage_client.c's contract). */
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

	printf("CLIENT colour=%06x size=%dx%d\n", c.rgb, c.width, c.height);
	fflush(stdout);
	wl_display_disconnect(c.display);
	return 0;
}
