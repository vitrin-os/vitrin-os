/* clipboard_peer.c -- a deterministic Wayland client that owns a selection and
 * reports the one it is given, for the WS-E.2.1 cross-realm clipboard gate
 * (issue #213).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS EXISTS, next to solid_client.c and click_target.c. The gate has to
 * observe two things no general-purpose app can be made to do reproducibly in
 * CI: put a KNOWN string on its own clipboard without a human selecting text
 * with a mouse, and report byte-for-byte what it later receives. A terminal
 * can be driven to do the first with enough synthetic input and cannot be made
 * to do the second at all.
 *
 * It is a REAL Wayland client -- bare `wl_shm` + xdg-shell + `wl_data_device`,
 * no toolkit, no vitrin protocol, no knowledge that it is confined -- running
 * under a real `vitrin-shim`. It is not a mock of any seam: the seam under
 * test is core <-> shim, and this sits a whole Wayland connection past the far
 * side of it. Its structure follows solid_client.c so the two cannot drift in
 * how they speak to the shim; what is new is the data device.
 *
 *   --offer TEXT    own the selection, serving TEXT as text/plain;charset=utf-8
 *   --sink PATH     write whatever selection the compositor offers into PATH
 *   --colour RRGGBB paint a known solid colour (so the realm has a surface)
 *   --run-ms MS     how long to stay up
 *
 * The sink is written **atomically** (temp file + rename) so a reader can
 * never observe a half-written transfer and conclude the wrong length, and it
 * is rewritten on every selection the compositor offers -- a test wanting
 * "before" and "after" reads it twice.
 *
 *   CLIENT clipboard offered=N received=M   printed on exit
 *
 * The peer prints LENGTHS, never content. It handles an application's
 * clipboard, and this file is on the same secrecy contract the core's flight
 * recorder is: a test that put a copied string in a log line would be the leak
 * the whole design refuses.
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

#include <errno.h>
#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

#define BUFFER_COUNT 2

/* The one type this peer offers and accepts -- the core's allow-list, restated
 * on the far side of the shim so the gate proves the two agree. */
#define PEER_MIME "text/plain;charset=utf-8"

/* Ceiling on what the peer will read out of an offered selection. One byte
 * past the core's own 61440 cap, so "the core sent more than it should" is
 * observable rather than silently truncated to exactly the cap. */
#define PEER_MAX_BYTES (61440u + 1u)

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

	/* The data device and its seat (WS-E.2.1). Absent globals are fatal:
	 * this peer's entire purpose is the clipboard, and a run that quietly
	 * did nothing would be a green gate proving nothing. */
	struct wl_seat *seat;
	struct wl_data_device_manager *ddm;
	struct wl_data_device *device;
	struct wl_data_source *source;
	struct wl_data_offer *incoming;
	bool incoming_has_mime;
	/* The most recent serial the compositor gave THIS client, and whether
	 * there has been one.
	 *
	 * `wl_data_device.set_selection` takes a serial, and wlroots really does
	 * validate it (`wlr_seat_client_validate_event_serial`: "Rejecting
	 * set_selection request, serial 0 was never given to client"). So this
	 * peer cannot claim the selection out of nowhere -- it has to be handed
	 * an input event first, which for a client with a mapped window is the
	 * `wl_keyboard.enter` the shim sends when it takes focus. That is a fact
	 * about Wayland worth having in the gate: a selection is always rooted in
	 * something the compositor delivered. */
	struct wl_keyboard *keyboard;
	uint32_t last_serial;
	bool have_serial;
	bool selection_claimed;

	/* What to serve, and where to record what arrives. */
	const char *offer_text;
	size_t offer_len;
	const char *sink_path;
	uint64_t offered_transfers;
	uint64_t received;

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

/* ---- the seat, for its serials ---------------------------------------- */

static void keyboard_keymap(void *data, struct wl_keyboard *kb, uint32_t format,
		int32_t fd, uint32_t size) {
	(void)data;
	(void)kb;
	(void)format;
	(void)size;
	close(fd);
}

static void keyboard_enter(void *data, struct wl_keyboard *kb, uint32_t serial,
		struct wl_surface *surface, struct wl_array *keys) {
	(void)kb;
	(void)surface;
	(void)keys;
	struct client *c = data;
	c->last_serial = serial;
	c->have_serial = true;
}

static void keyboard_leave(void *data, struct wl_keyboard *kb, uint32_t serial,
		struct wl_surface *surface) {
	(void)kb;
	(void)surface;
	struct client *c = data;
	c->last_serial = serial;
	c->have_serial = true;
}

static void keyboard_key(void *data, struct wl_keyboard *kb, uint32_t serial,
		uint32_t time, uint32_t key, uint32_t state) {
	(void)kb;
	(void)time;
	(void)key;
	(void)state;
	struct client *c = data;
	c->last_serial = serial;
	c->have_serial = true;
}

static void keyboard_modifiers(void *data, struct wl_keyboard *kb, uint32_t serial,
		uint32_t depressed, uint32_t latched, uint32_t locked, uint32_t group) {
	(void)kb;
	(void)depressed;
	(void)latched;
	(void)locked;
	(void)group;
	struct client *c = data;
	c->last_serial = serial;
	c->have_serial = true;
}

static void keyboard_repeat_info(void *data, struct wl_keyboard *kb, int32_t rate,
		int32_t delay) {
	(void)data;
	(void)kb;
	(void)rate;
	(void)delay;
}

static const struct wl_keyboard_listener keyboard_listener = {
	.keymap = keyboard_keymap,
	.enter = keyboard_enter,
	.leave = keyboard_leave,
	.key = keyboard_key,
	.modifiers = keyboard_modifiers,
	.repeat_info = keyboard_repeat_info,
};

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
	struct client *c = data;
	if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) != 0 && c->keyboard == NULL) {
		c->keyboard = wl_seat_get_keyboard(seat);
		wl_keyboard_add_listener(c->keyboard, &keyboard_listener, c);
	}
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {
	(void)data;
	(void)seat;
	(void)name;
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

/* ---- the data device ------------------------------------------------- */

/* Serve our string down the descriptor the compositor handed us.
 *
 * Blocking on purpose: this peer has nothing else to do, the payload is small,
 * and the alternative -- a poll loop in a test client -- buys nothing and adds
 * a way for the gate to hang. `SIGPIPE` is ignored in `main`, so a reader that
 * vanished mid-transfer is an `EPIPE` return rather than a dead client. */
static void source_send(void *data, struct wl_data_source *src, const char *mime, int32_t fd) {
	(void)src;
	struct client *c = data;
	if (mime == NULL || strcmp(mime, PEER_MIME) != 0) {
		close(fd);
		return;
	}
	size_t off = 0;
	while (off < c->offer_len) {
		ssize_t n = write(fd, c->offer_text + off, c->offer_len - off);
		if (n > 0) {
			off += (size_t)n;
			continue;
		}
		if (n < 0 && errno == EINTR) {
			continue;
		}
		break;
	}
	close(fd);
	c->offered_transfers++;
}

static void source_target(void *data, struct wl_data_source *src, const char *mime) {
	(void)data;
	(void)src;
	(void)mime;
}

static void source_cancelled(void *data, struct wl_data_source *src) {
	struct client *c = data;
	/* Somebody else owns the selection now -- in this gate, the shim-owned
	 * source the core offered. Ordinary; drop ours rather than leaking it. */
	if (c->source == src) {
		c->source = NULL;
	}
	wl_data_source_destroy(src);
}

static void source_dnd_drop_performed(void *data, struct wl_data_source *src) {
	(void)data;
	(void)src;
}

static void source_dnd_finished(void *data, struct wl_data_source *src) {
	(void)data;
	(void)src;
}

static void source_action(void *data, struct wl_data_source *src, uint32_t action) {
	(void)data;
	(void)src;
	(void)action;
}

static const struct wl_data_source_listener source_listener = {
	.target = source_target,
	.send = source_send,
	.cancelled = source_cancelled,
	.dnd_drop_performed = source_dnd_drop_performed,
	.dnd_finished = source_dnd_finished,
	.action = source_action,
};

static void offer_offer(void *data, struct wl_data_offer *offer, const char *mime) {
	struct client *c = data;
	if (c->incoming == offer && mime != NULL && strcmp(mime, PEER_MIME) == 0) {
		c->incoming_has_mime = true;
	}
}

static void offer_source_actions(void *data, struct wl_data_offer *offer, uint32_t actions) {
	(void)data;
	(void)offer;
	(void)actions;
}

static void offer_action(void *data, struct wl_data_offer *offer, uint32_t action) {
	(void)data;
	(void)offer;
	(void)action;
}

static const struct wl_data_offer_listener offer_listener = {
	.offer = offer_offer,
	.source_actions = offer_source_actions,
	.action = offer_action,
};

/* Write `bytes` to the sink atomically: a reader must never see a partial
 * transfer and read the wrong length out of it. */
static void sink_write(struct client *c, const unsigned char *bytes, size_t len) {
	if (c->sink_path == NULL) {
		return;
	}
	char tmp[4096];
	int n = snprintf(tmp, sizeof(tmp), "%s.part", c->sink_path);
	if (n < 0 || (size_t)n >= sizeof(tmp)) {
		return;
	}
	int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (fd < 0) {
		return;
	}
	size_t off = 0;
	while (off < len) {
		ssize_t w = write(fd, bytes + off, len - off);
		if (w > 0) {
			off += (size_t)w;
			continue;
		}
		if (w < 0 && errno == EINTR) {
			continue;
		}
		break;
	}
	close(fd);
	if (off == len) {
		(void)rename(tmp, c->sink_path);
	} else {
		(void)unlink(tmp);
	}
}

/* The compositor handed us a selection. Receive it, whole, and record it.
 *
 * `receive` + a blocking read on our own pipe end, for `source_send`'s
 * reason: a test client with nothing else to do is better off simple. */
static void data_device_selection(void *data, struct wl_data_device *dev,
		struct wl_data_offer *offer) {
	(void)dev;
	struct client *c = data;
	if (offer == NULL) {
		c->incoming = NULL;
		c->incoming_has_mime = false;
		return;
	}
	/* **Never receive an offer while we own the selection**, and the reason is
	 * a deadlock rather than tidiness: a compositor sends the selection to the
	 * focused client whether or not that client is the source, so an owner
	 * that receives its own offer blocks reading a pipe only its own
	 * `source_send` callback can fill -- and that callback needs the dispatch
	 * loop this read is blocking. The offering peer really did hang exactly
	 * here, two loop turns after claiming the selection.
	 *
	 * `c->source` is NULL once the compositor cancels us, which is precisely
	 * when an offer becomes somebody else's and worth taking. */
	if (c->source != NULL) {
		wl_data_offer_destroy(offer);
		c->incoming = NULL;
		c->incoming_has_mime = false;
		return;
	}
	if (!c->incoming_has_mime) {
		/* Not a type we take. Nothing is written, so a sink that stays absent
		 * is itself evidence: the gate reads that as "nothing crossed". */
		wl_data_offer_destroy(offer);
		c->incoming = NULL;
		return;
	}
	int fds[2];
	if (pipe2(fds, O_CLOEXEC) != 0) {
		wl_data_offer_destroy(offer);
		c->incoming = NULL;
		c->incoming_has_mime = false;
		return;
	}
	wl_data_offer_receive(offer, PEER_MIME, fds[1]);
	close(fds[1]);
	/* The compositor only acts on `receive` when it sees the request, and it
	 * will not see it while we block on the pipe. */
	wl_display_flush(c->display);

	static unsigned char buf[PEER_MAX_BYTES];
	size_t len = 0;
	for (;;) {
		if (len >= sizeof(buf)) {
			break;
		}
		ssize_t r = read(fds[0], buf + len, sizeof(buf) - len);
		if (r > 0) {
			len += (size_t)r;
			continue;
		}
		if (r < 0 && errno == EINTR) {
			continue;
		}
		break;
	}
	close(fds[0]);
	wl_data_offer_destroy(offer);
	c->incoming = NULL;
	c->incoming_has_mime = false;
	if (len > 0) {
		sink_write(c, buf, len);
		c->received++;
	}
	/* Wiped rather than merely left: this is an application's clipboard and
	 * the peer keeps the same secrecy posture the core does. */
	memset(buf, 0, sizeof(buf));
}

static void data_device_data_offer(void *data, struct wl_data_device *dev,
		struct wl_data_offer *offer) {
	(void)dev;
	struct client *c = data;
	c->incoming = offer;
	c->incoming_has_mime = false;
	wl_data_offer_add_listener(offer, &offer_listener, c);
}

static void data_device_enter(void *data, struct wl_data_device *dev, uint32_t serial,
		struct wl_surface *surface, wl_fixed_t x, wl_fixed_t y, struct wl_data_offer *offer) {
	(void)data;
	(void)dev;
	(void)serial;
	(void)surface;
	(void)x;
	(void)y;
	(void)offer;
}

static void data_device_leave(void *data, struct wl_data_device *dev) {
	(void)data;
	(void)dev;
}

static void data_device_motion(void *data, struct wl_data_device *dev, uint32_t time,
		wl_fixed_t x, wl_fixed_t y) {
	(void)data;
	(void)dev;
	(void)time;
	(void)x;
	(void)y;
}

static void data_device_drop(void *data, struct wl_data_device *dev) {
	(void)data;
	(void)dev;
}

static const struct wl_data_device_listener data_device_listener = {
	.data_offer = data_device_data_offer,
	.enter = data_device_enter,
	.leave = data_device_leave,
	.motion = data_device_motion,
	.drop = data_device_drop,
	.selection = data_device_selection,
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
	} else if (strcmp(iface, wl_seat_interface.name) == 0) {
		c->seat = wl_registry_bind(reg, name, &wl_seat_interface, version < 5 ? version : 5);
	} else if (strcmp(iface, wl_data_device_manager_interface.name) == 0) {
		c->ddm = wl_registry_bind(reg, name, &wl_data_device_manager_interface,
			version < 3 ? version : 3);
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

/* Claim the selection, once and only once there is a serial to quote.
 *
 * Idempotent and cheap, so the dispatch loop can call it every turn instead of
 * the client needing a state machine. */
static void claim_selection(struct client *c) {
	if (c->offer_text == NULL || c->selection_claimed || !c->have_serial) {
		return;
	}
	c->source = wl_data_device_manager_create_data_source(c->ddm);
	wl_data_source_add_listener(c->source, &source_listener, c);
	wl_data_source_offer(c->source, PEER_MIME);
	wl_data_device_set_selection(c->device, c->source, c->last_serial);
	wl_display_flush(c->display);
	c->selection_claimed = true;
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

int main(int argc, char **argv) {
	int run_ms = 3000;
	uint32_t rgb = DEFAULT_COLOUR;
	const char *offer_text = NULL;
	const char *sink_path = NULL;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--offer") == 0 && i + 1 < argc) {
			offer_text = argv[++i];
		} else if (strcmp(argv[i], "--sink") == 0 && i + 1 < argc) {
			sink_path = argv[++i];
		} else if ((strcmp(argv[i], "--colour") == 0 || strcmp(argv[i], "--color") == 0) &&
				i + 1 < argc) {
			if (!parse_colour(argv[++i], &rgb)) {
				fprintf(stderr, "bad colour '%s' (expected six hex digits, e.g. 0000ff)\n", argv[i]);
				return 2;
			}
		} else {
			fprintf(stderr,
				"usage: %s [--run-ms MS] [--colour RRGGBB] [--offer TEXT] [--sink PATH]\n",
				argv[0]);
			return 2;
		}
	}
	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);
	/* A reader that vanishes mid-transfer must be an EPIPE return, never a
	 * dead client: this peer writes into descriptors the compositor supplies. */
	signal(SIGPIPE, SIG_IGN);

	struct client c = {
		.width = 0,
		.height = 0,
		.rgb = rgb,
		.offer_text = offer_text,
		.offer_len = offer_text != NULL ? strlen(offer_text) : 0,
		.sink_path = sink_path,
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
	/* Fatal, not best-effort: this peer exists to exercise the clipboard, and
	 * a run without a data device would print plausible-looking output while
	 * proving nothing. */
	if (c.seat == NULL || c.ddm == NULL) {
		fprintf(stderr, "the compositor is missing wl_seat or wl_data_device_manager\n");
		return 1;
	}
	wl_seat_add_listener(c.seat, &seat_listener, &c);
	c.device = wl_data_device_manager_get_data_device(c.ddm, c.seat);
	wl_data_device_add_listener(c.device, &data_device_listener, &c);
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

	/* Claiming the selection waits for two things, and neither is optional:
	 * the surface must exist (the shim gives an app keyboard focus when its
	 * window maps, and a compositor delivers `wl_data_device.selection` to the
	 * focused client alone) and the compositor must have given this client a
	 * serial to quote. `claim_selection` is therefore called from the dispatch
	 * loop rather than once here. */

	struct timespec start;
	clock_gettime(CLOCK_MONOTONIC, &start);
	while (!g_stop && !c.closed) {
		claim_selection(&c);
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

	/* Lengths and counts only -- never the strings. This peer handles an
	 * application's clipboard and keeps the core's secrecy contract. */
	printf("CLIENT clipboard colour=%06x size=%dx%d offered=%llu received=%llu\n",
		c.rgb, c.width, c.height,
		(unsigned long long)c.offered_transfers, (unsigned long long)c.received);
	fflush(stdout);
	wl_display_disconnect(c.display);
	return 0;
}
