/* input_echo_client.c -- the app side of the P1.6.3 acceptance proof: a
 * Wayland client that receives seat input and writes down exactly what it
 * got.
 *
 * `damage_client.c` proves things about the frame path; this one proves
 * things about the input path, and it has to be a separate program for the
 * same reason: the claims are mechanical, so the client has to be
 * deterministic and has to report machine-readable facts rather than draw
 * pictures a human squints at.
 *
 * WHAT IT ESTABLISHES THAT THE SHIM'S OWN LOG CANNOT. The shim's delivery
 * trace says what the shim believes it sent. This client is the independent
 * witness on the other side of the Wayland connection: it says what actually
 * arrived, in what order, with what coordinates, and how many times. "No
 * duplicates" in particular is a claim only the receiver can settle.
 *
 * THE KEYMAP IS INTERPRETED, NOT INSPECTED. Every `wl_keyboard.key` is
 * resolved to a keysym and then to UTF-8 through the keymap the compositor
 * sent -- compiled with xkbcommon, exactly as GTK, Qt and Firefox do it.
 * That is what makes this a test of the dynamic keymap (D7) rather than a
 * test of keycode plumbing: the accumulated text at the end is the string
 * the app would have typed into a text field, character for character. It
 * also re-reads the keymap on every `keymap` event, so a regeneration
 * mid-run is visible as a generation count rather than as silent corruption.
 *
 * THE COORDINATE CASE. `--inset N` makes the client behave like every real
 * CSD toolkit: it allocates a buffer N pixels larger than the size it was
 * configured at, on all four sides, and declares the inner rectangle as its
 * `xdg_surface.set_window_geometry` -- the shadow border a GTK or Qt window
 * draws around itself. That moves the surface to (-N, -N) inside its own
 * scene tree, so realm-view (x, y) is surface-local (x + N, y + N) and the
 * view -> surface-local mapping stops being the identity. Without it the
 * mapping is untestable: the shim pins a single maximized toplevel at the
 * view origin, so every coordinate would arrive unchanged whether the shim
 * mapped it or merely copied it.
 *
 * Output is one `IN ...` line per event, then `TEXT` (escaped, always one
 * line) and `TEXT_HEX` (byte-exact) carrying the accumulated string, then a
 * `SUMMARY ...` line of counters.
 */
#define _GNU_SOURCE

#include <fcntl.h>
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
#include <xkbcommon/xkbcommon.h>

#include "xdg-shell-client-protocol.h"

/* Enough for any string `vitrin_shim_seat.text` can carry (4096 bytes) plus
 * whatever a scripted key run adds. */
#define TEXT_CAP 16384

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

struct client {
	struct wl_display *display;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	struct wl_seat *seat;
	struct wl_pointer *pointer;
	struct wl_keyboard *keyboard;

	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;
	struct wl_buffer *buffer;
	/* The size the compositor configured us at -- the window-geometry
	 * rectangle, in CSD terms. The buffer is `inset` larger on every side. */
	int width, height;
	int inset;
	bool configured;
	bool mapped;

	struct xkb_context *xkb;
	struct xkb_keymap *keymap;
	struct xkb_state *xkb_state;

	/* Everything the acceptance script asserts on. */
	uint64_t n_enter, n_leave, n_motion, n_button, n_axis, n_v120, n_frame;
	uint64_t n_keymap, n_key_press, n_key_release, n_kb_enter, n_modifiers;
	char text[TEXT_CAP];
	size_t text_len;
};

static void note_text(struct client *c, const char *utf8) {
	size_t n = strlen(utf8);
	if (c->text_len + n + 1 >= sizeof(c->text)) {
		return;
	}
	memcpy(c->text + c->text_len, utf8, n);
	c->text_len += n;
	c->text[c->text_len] = '\0';
}

/* ---- pointer --------------------------------------------------------- */

static void ptr_enter(void *data, struct wl_pointer *p, uint32_t serial,
		struct wl_surface *surface, wl_fixed_t sx, wl_fixed_t sy) {
	(void)p; (void)serial; (void)surface;
	struct client *c = data;
	c->n_enter++;
	printf("IN pointer_enter sx=%.3f sy=%.3f\n",
		wl_fixed_to_double(sx), wl_fixed_to_double(sy));
}

static void ptr_leave(void *data, struct wl_pointer *p, uint32_t serial,
		struct wl_surface *surface) {
	(void)p; (void)serial; (void)surface;
	struct client *c = data;
	c->n_leave++;
	printf("IN pointer_leave\n");
}

static void ptr_motion(void *data, struct wl_pointer *p, uint32_t time,
		wl_fixed_t sx, wl_fixed_t sy) {
	(void)p; (void)time;
	struct client *c = data;
	c->n_motion++;
	printf("IN pointer_motion sx=%.3f sy=%.3f\n",
		wl_fixed_to_double(sx), wl_fixed_to_double(sy));
}

static void ptr_button(void *data, struct wl_pointer *p, uint32_t serial,
		uint32_t time, uint32_t button, uint32_t state) {
	(void)p; (void)serial; (void)time;
	struct client *c = data;
	c->n_button++;
	printf("IN pointer_button button=%u state=%u\n", button, state);
}

static void ptr_axis(void *data, struct wl_pointer *p, uint32_t time,
		uint32_t axis, wl_fixed_t value) {
	(void)p; (void)time;
	struct client *c = data;
	c->n_axis++;
	printf("IN pointer_axis axis=%u value=%.3f\n", axis, wl_fixed_to_double(value));
}

static void ptr_frame(void *data, struct wl_pointer *p) {
	(void)p;
	struct client *c = data;
	c->n_frame++;
	printf("IN pointer_frame\n");
}

static void ptr_axis_source(void *data, struct wl_pointer *p, uint32_t source) {
	(void)data; (void)p;
	printf("IN pointer_axis_source source=%u\n", source);
}

static void ptr_axis_stop(void *data, struct wl_pointer *p, uint32_t time, uint32_t axis) {
	(void)data; (void)p; (void)time;
	printf("IN pointer_axis_stop axis=%u\n", axis);
}

static void ptr_axis_discrete(void *data, struct wl_pointer *p, uint32_t axis,
		int32_t discrete) {
	(void)data; (void)p;
	printf("IN pointer_axis_discrete axis=%u discrete=%d\n", axis, discrete);
}

static void ptr_axis_value120(void *data, struct wl_pointer *p, uint32_t axis,
		int32_t value120) {
	(void)p;
	struct client *c = data;
	c->n_v120++;
	printf("IN pointer_axis_value120 axis=%u value120=%d\n", axis, value120);
}

static void ptr_axis_relative_direction(void *data, struct wl_pointer *p,
		uint32_t axis, uint32_t direction) {
	(void)data; (void)p;
	printf("IN pointer_axis_relative_direction axis=%u direction=%u\n", axis, direction);
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

/* ---- keyboard -------------------------------------------------------- */

static void kb_keymap(void *data, struct wl_keyboard *k, uint32_t format,
		int32_t fd, uint32_t size) {
	(void)k;
	struct client *c = data;
	c->n_keymap++;

	if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1) {
		printf("IN keymap generation=%llu format=%u UNSUPPORTED\n",
			(unsigned long long)c->n_keymap, format);
		close(fd);
		return;
	}
	char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
	if (map == MAP_FAILED) {
		printf("IN keymap generation=%llu MMAP_FAILED\n", (unsigned long long)c->n_keymap);
		close(fd);
		return;
	}
	/* Recompile from scratch on every keymap event -- which is exactly what
	 * a correct toolkit does, and exactly what an app that "reads the keymap
	 * once" does NOT do. Keeping this honest is the point: the shim's
	 * keycode-stability argument has to be checked against a client that
	 * plays by the rules, and the failure mode of one that does not is then
	 * a separate, documented claim. */
	struct xkb_keymap *keymap = xkb_keymap_new_from_string(c->xkb, map,
		XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
	munmap(map, size);
	close(fd);
	if (keymap == NULL) {
		printf("IN keymap generation=%llu COMPILE_FAILED size=%u\n",
			(unsigned long long)c->n_keymap, size);
		return;
	}
	struct xkb_state *state = xkb_state_new(keymap);
	if (state == NULL) {
		xkb_keymap_unref(keymap);
		printf("IN keymap generation=%llu STATE_FAILED\n", (unsigned long long)c->n_keymap);
		return;
	}
	xkb_keymap_unref(c->keymap);
	xkb_state_unref(c->xkb_state);
	c->keymap = keymap;
	c->xkb_state = state;
	printf("IN keymap generation=%llu format=%u size=%u OK\n",
		(unsigned long long)c->n_keymap, format, size);
}

static void kb_enter(void *data, struct wl_keyboard *k, uint32_t serial,
		struct wl_surface *surface, struct wl_array *keys) {
	(void)k; (void)serial; (void)surface;
	struct client *c = data;
	c->n_kb_enter++;
	printf("IN keyboard_enter held=%zu\n", keys->size / sizeof(uint32_t));
}

static void kb_leave(void *data, struct wl_keyboard *k, uint32_t serial,
		struct wl_surface *surface) {
	(void)data; (void)k; (void)serial; (void)surface;
	printf("IN keyboard_leave\n");
}

static void kb_key(void *data, struct wl_keyboard *k, uint32_t serial,
		uint32_t time, uint32_t key, uint32_t state) {
	(void)k; (void)serial; (void)time;
	struct client *c = data;
	if (state == WL_KEYBOARD_KEY_STATE_PRESSED) {
		c->n_key_press++;
	} else {
		c->n_key_release++;
	}

	/* evdev keycode -> xkb keycode is the historical +8 every toolkit
	 * applies; the compositor sends evdev codes on the wire. */
	xkb_keycode_t keycode = key + 8;
	xkb_keysym_t keysym = XKB_KEY_NoSymbol;
	char utf8[16] = {0};
	char name[64] = "none";
	if (c->xkb_state != NULL) {
		keysym = xkb_state_key_get_one_sym(c->xkb_state, keycode);
		xkb_keysym_get_name(keysym, name, sizeof(name));
		xkb_state_key_get_utf8(c->xkb_state, keycode, utf8, sizeof(utf8));
	}
	printf("IN key keycode=%u state=%u keysym=0x%08x name=%s utf8=\"%s\"\n",
		key, state, keysym, name, utf8);

	/* Accumulate what a text field would have received: the UTF-8 of every
	 * PRESS, with Return and Tab spelled out as the characters the IDL says
	 * they must deliver.
	 *
	 * Control characters are excluded, and that exclusion is the point
	 * rather than tidiness: `xkb_state_key_get_utf8` happily renders Escape
	 * as U+001B and BackSpace as U+0008, so a naive accumulator would count
	 * command keys as typed text and the string comparison at the end would
	 * be comparing something no text field has ever contained. A widget
	 * routes those keysyms to actions, not to its buffer -- so this does
	 * too. */
	if (state == WL_KEYBOARD_KEY_STATE_PRESSED) {
		if (keysym == XKB_KEY_Return || keysym == XKB_KEY_KP_Enter) {
			note_text(c, "\n");
		} else if (keysym == XKB_KEY_Tab) {
			note_text(c, "\t");
		} else if (utf8[0] != '\0' && xkb_keysym_to_utf32(keysym) >= 0x20) {
			note_text(c, utf8);
		}
	}

	/* A client updates its own xkb state from `modifiers`, not from keys,
	 * so nothing is done here -- deliberately: doing both is the classic
	 * double-counting bug, and this client must model a correct app. */
}

static void kb_modifiers(void *data, struct wl_keyboard *k, uint32_t serial,
		uint32_t depressed, uint32_t latched, uint32_t locked, uint32_t group) {
	(void)k; (void)serial;
	struct client *c = data;
	c->n_modifiers++;
	if (c->xkb_state != NULL) {
		xkb_state_update_mask(c->xkb_state, depressed, latched, locked, 0, 0, group);
	}
	printf("IN modifiers depressed=0x%x latched=0x%x locked=0x%x group=%u\n",
		depressed, latched, locked, group);
}

static void kb_repeat_info(void *data, struct wl_keyboard *k, int32_t rate, int32_t delay) {
	(void)data; (void)k;
	printf("IN repeat_info rate=%d delay=%d\n", rate, delay);
}

static const struct wl_keyboard_listener keyboard_listener = {
	.keymap = kb_keymap,
	.enter = kb_enter,
	.leave = kb_leave,
	.key = kb_key,
	.modifiers = kb_modifiers,
	.repeat_info = kb_repeat_info,
};

/* ---- seat ------------------------------------------------------------ */

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
	struct client *c = data;
	printf("IN seat_capabilities caps=0x%x\n", caps);
	if ((caps & WL_SEAT_CAPABILITY_POINTER) != 0 && c->pointer == NULL) {
		c->pointer = wl_seat_get_pointer(seat);
		wl_pointer_add_listener(c->pointer, &pointer_listener, c);
	}
	if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) != 0 && c->keyboard == NULL) {
		c->keyboard = wl_seat_get_keyboard(seat);
		wl_keyboard_add_listener(c->keyboard, &keyboard_listener, c);
	}
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {
	(void)data; (void)seat;
	printf("IN seat_name name=%s\n", name);
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

/* ---- surface bring-up ------------------------------------------------ */

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
		/* Version 8 is the floor that matters: `wl_pointer.axis_value120` --
		 * the high-resolution scroll the wire's `value120` exists to carry.
		 * Binding lower would make this client unable to observe the very
		 * thing the scroll criterion is about. */
		uint32_t want = version > 9 ? 9 : version;
		c->seat = wl_registry_bind(reg, name, &wl_seat_interface, want);
		wl_seat_add_listener(c->seat, &seat_listener, c);
		printf("IN seat_bound version=%u\n", want);
	}
}

static void registry_global_remove(void *data, struct wl_registry *reg, uint32_t name) {
	(void)data; (void)reg; (void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

/* A trivial opaque buffer. The point is to have a mapped surface for input
 * to land on and for the shim's hit test to find -- the pixels are not
 * under test here. */
static bool make_buffer(struct client *c) {
	int bw = c->width + 2 * c->inset;
	int bh = c->height + 2 * c->inset;
	size_t stride = (size_t)bw * 4;
	size_t size = stride * (size_t)bh;
	int fd = memfd_create("input-echo", MFD_CLOEXEC);
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
		px[i] = 0xff203040u;
	}
	munmap(px, size);
	struct wl_shm_pool *pool = wl_shm_create_pool(c->shm, fd, (int32_t)size);
	c->buffer = wl_shm_pool_create_buffer(pool, 0, bw, bh,
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
	int inset = 0;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--inset") == 0 && i + 1 < argc) {
			inset = atoi(argv[++i]);
		} else {
			fprintf(stderr, "usage: input-echo-client [--run-ms MS] [--inset N]\n");
			return 2;
		}
	}

	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	struct client c = {.width = 320, .height = 240, .inset = inset};
	c.xkb = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	if (c.xkb == NULL) {
		fprintf(stderr, "no xkb context\n");
		return 1;
	}

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

	c.surface = wl_compositor_create_surface(c.compositor);
	c.xdg_surface = xdg_wm_base_get_xdg_surface(c.wm_base, c.surface);
	xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c);
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
	xdg_toplevel_set_title(c.toplevel, "vitrin-input-echo");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.InputEcho");
	wl_surface_commit(c.surface);

	while (!c.configured && wl_display_dispatch(c.display) != -1) {
		/* wait for the first configure */
	}
	if (!make_buffer(&c)) {
		fprintf(stderr, "cannot allocate the surface buffer\n");
		return 1;
	}
	if (c.inset > 0) {
		/* The CSD contract: the buffer is bigger than the window, and the
		 * window is the inner rectangle. The compositor positions us by
		 * THIS rectangle, which is what makes view -> surface-local a real
		 * translation rather than a copy. */
		xdg_surface_set_window_geometry(c.xdg_surface, c.inset, c.inset,
			c.width, c.height);
	}
	wl_surface_attach(c.surface, c.buffer, 0, 0);
	wl_surface_damage(c.surface, 0, 0, c.width + 2 * c.inset, c.height + 2 * c.inset);
	wl_surface_commit(c.surface);
	wl_display_roundtrip(c.display);
	c.mapped = true;
	printf("IN mapped width=%d height=%d inset=%d buffer=%dx%d\n",
		c.width, c.height, c.inset, c.width + 2 * c.inset, c.height + 2 * c.inset);
	fflush(stdout);

	int64_t deadline = now_ms() + run_ms;
	while (!g_stop && now_ms() < deadline) {
		/* The standard prepare/read/dispatch dance, so the loop can also
		 * poll a timeout instead of blocking forever on a quiet compositor. */
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
		fflush(stdout);
	}

	/* Two forms of the same bytes, because they answer different questions.
	 * `TEXT` is escaped so it is ALWAYS one line: the accumulated string can
	 * contain a newline (the IDL requires `\n` to be delivered as Return),
	 * and an assertion that read only the first line of it would silently
	 * compare a prefix. `TEXT_HEX` is the byte-exact form -- a mangled
	 * character and a correct one can render identically in a terminal with
	 * the wrong font, so the strict comparison has to be on bytes. */
	printf("TEXT ");
	for (const unsigned char *p = (const unsigned char *)c.text; *p != '\0'; p++) {
		switch (*p) {
		case '\n': fputs("\\n", stdout); break;
		case '\t': fputs("\\t", stdout); break;
		case '\\': fputs("\\\\", stdout); break;
		default: putchar(*p); break;
		}
	}
	putchar('\n');
	printf("TEXT_HEX ");
	for (const unsigned char *p = (const unsigned char *)c.text; *p != '\0'; p++) {
		printf("%02x", *p);
	}
	putchar('\n');
	printf("SUMMARY enter=%llu leave=%llu motion=%llu button=%llu axis=%llu v120=%llu "
		"frame=%llu keymaps=%llu kb_enter=%llu press=%llu release=%llu modifiers=%llu "
		"text_bytes=%zu\n",
		(unsigned long long)c.n_enter, (unsigned long long)c.n_leave,
		(unsigned long long)c.n_motion, (unsigned long long)c.n_button,
		(unsigned long long)c.n_axis, (unsigned long long)c.n_v120,
		(unsigned long long)c.n_frame, (unsigned long long)c.n_keymap,
		(unsigned long long)c.n_kb_enter, (unsigned long long)c.n_key_press,
		(unsigned long long)c.n_key_release, (unsigned long long)c.n_modifiers,
		c.text_len);
	fflush(stdout);

	xkb_state_unref(c.xkb_state);
	xkb_keymap_unref(c.keymap);
	xkb_context_unref(c.xkb);
	wl_display_disconnect(c.display);
	return 0;
}
