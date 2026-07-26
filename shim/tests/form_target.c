/* form_target.c -- the headless venue's app for the goal-directed demo: a
 * two-field form that echoes what it was told, and paints a checksum of it.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS EXISTS. `click_target.c` proves a click lands on an observed
 * feature; `input_echo_client.c` proves a resolved key becomes the character
 * a text field would have received. Neither is a *task*. The demo agent
 * (`examples/agent-demo/run_demo.py`) is handed a task record it did not
 * author -- field names and values -- fills the form, submits it, and then has
 * to prove FROM PIXELS ALONE that the confirmation reflects exactly the values
 * it was told to enter. That needs an app which
 *
 *   1. has two locatable input fields and a locatable submit button, all
 *      addressable by a marker colour rather than by geometry told twice;
 *   2. accumulates typed text per field, so the click that focused a field
 *      and the text that followed it are causally linked in one surface;
 *   3. on submit, paints a receipt whose colours are a pure function of the
 *      whole record -- so a frame either carries this record's checksum or it
 *      does not, with no "enough pixels moved" heuristic anywhere;
 *   4. prints the bytes it received, so the pixel claim has an independent,
 *      byte-exact ground truth beside it (the role `gtk_entry_probe.c`'s
 *      `ENTRY_HEX` plays for the D7 text gate).
 *
 * THE RECEIPT ENCODING IS NORMATIVE, AND IT IS NOT DEFINED HERE.
 * `examples/agent-demo/README.md` defines it; the Python in `run_demo.py` is
 * the reference implementation. This file restates it in C and is pinned
 * against the reference by a unit test (`--bands`, below, driven from
 * `tests/integration/test_demo.py`). If the two ever disagree, the README and
 * the Python win and this file is the bug.
 *
 * Briefly, so a reader of this file can follow the paint code:
 *
 *     canon    = "k0=v0\nk1=v1"     (the field NAMES come from --field, the
 *                                    values from what was typed)
 *     band i   = fnv1a32(canon + "#" + i) -> r=((h>>8)&0xF)*0x11,
 *                                           g=((h>>4)&0xF)*0x11,
 *                                           b=( h    &0xF)*0x11
 *
 * Channels are forced to multiples of 0x11 because that is this repo's
 * established convention for a colour that survives the capture path and a
 * 4-bit-per-channel histogram EXACTLY, with no tolerance
 * (`tests/integration/harness.py`'s `dominant_colour`/`locate_colour`,
 * `click_target.c`'s three colours). Three bands are 36 bits, so a *wrong*
 * record painting all three correctly is a ~1.5e-11 coincidence.
 *
 * THE KEYMAP IS INTERPRETED, NOT INSPECTED -- the same D7 point
 * `input_echo_client.c` exists to make, and the reason its keyboard handling
 * is copied here rather than simplified. The shim's keymap is generated at
 * runtime (`shim/src/seat.c`), so resolving a `wl_keyboard.key` to a
 * character is the APP's job, through xkbcommon, exactly as GTK/Qt/Firefox do
 * it. An app that got this subtly wrong would look fine and silently drop
 * characters -- which the receipt would then, correctly, refuse to match.
 *
 * NO ENTER HANDLER, DELIBERATELY. Submission happens only through a click on
 * the located button. So per-field typing carries no trailing newline, and
 * "the form was submitted" is itself a pointer-actuation proof rather than a
 * side effect of the text payload.
 *
 * A NOTE ON GLYPHS. This client rasterises no font. Typed bytes are drawn as
 * one filled ink cell per received UTF-8 byte (`ink_text`), which is enough
 * for "ink landed inside the field I clicked" and nothing more. The demo's
 * proof of *content* is the receipt checksum and this program's own `SUBMIT`
 * line -- never glyph recognition -- so a rasteriser here would add a font
 * dependency and prove nothing extra. `run_demo.py` says the same thing in
 * the same words, on purpose.
 *
 * Buffer handling and the pointer listener follow `click_target.c`; the
 * keyboard and xkbcommon handling follows `input_echo_client.c`. Neither is
 * re-derived, so neither can drift.
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

#include <linux/input-event-codes.h>
#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>

#include "xdg-shell-client-protocol.h"

#define BUFFER_COUNT 2

/* The realm view this layout is authored against. Kept at the size the rest
 * of the real-app ladder uses (`tests/integration/test_real_app.py`'s
 * REALM_SIZE, `crates/xtask`'s HEADLESS_SIZE): every measured threshold in
 * the demo and its gate is derived at this size, so enlarging the view would
 * break size parity with all of them. */
#define VIEW_W 640
#define VIEW_H 480

/* Every colour's channels are multiples of 0x11, so a 4-bit-per-channel
 * histogram reads them back exactly (see the header comment). */
#define COLOUR_BG 0xffffffu     /* the form's paper */
#define COLOUR_FIELD0 0x00ff00u /* field 0's marker: green  */
#define COLOUR_FIELD1 0x00ffffu /* field 1's marker: cyan   */
#define COLOUR_SUBMIT 0xffff00u /* the submit button:  yellow */
#define COLOUR_BORDER 0x000000u /* borders, focus ring, ink   */

/* Border drawn OUTSIDE each marker rectangle, so the rectangle the agent
 * locates by colour is exactly the marker's own extent. */
#define BORDER_W 4

/* The focus ring is drawn just INSIDE the field's marker rectangle, which is
 * deliberate: it puts a change *inside* the field's bounding box that no
 * typing produced. That is the trap `run_demo.py` has to survive (it
 * baselines the per-field ink profile AFTER the click and before the type,
 * and additionally insets the rectangle past this ring), and a gate whose app
 * cannot spring the trap would not be testing the mitigation at all. */
#define FOCUS_W 2

#define FIELD_COUNT 2

/* Text capacity per field: the IDL caps one `vitrin_actuator_text.type`
 * payload at 4096 bytes, and a scripted run may type more than once. */
#define TEXT_CAP 16384

/* Ink geometry: one filled INK_CELL_W x INK_CELL_H cell per received byte,
 * at INK_PITCH horizontal spacing, inset INK_PAD_X/INK_PAD_Y into the
 * region. INK_PAD_Y clears the focus ring above. */
#define INK_CELL_W 4
#define INK_CELL_H 12
#define INK_PITCH 6
#define INK_PAD_X 8
#define INK_PAD_Y 16
#define INK_LINE_H 16

/* The confirmation view: an echo strip above BAND_TOP, then the bands filling
 * everything below it. At the pinned 480-row view that is three 128-row
 * bands. `form.html` uses the same split, so the agent's decoder is the same
 * code in both venues. */
#define BAND_TOP 96
#define BAND_COUNT 3

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

static inline uint32_t pack(uint32_t rgb) {
	return 0xff000000u | rgb; /* XRGB8888, opaque */
}

struct rect {
	int x0, y0, x1, y1; /* half-open: [x0, x1) x [y0, y1) */
};

/* Surface-local layout, in pixels, at the pinned VIEW_W x VIEW_H view. The
 * same reading order and the same colours as `examples/agent-demo/form.html`,
 * so the agent's locator code is literally identical in both venues. */
static const struct rect FIELD_RECT[FIELD_COUNT] = {
	{40, 96, 600, 140},
	{40, 176, 600, 220},
};
static const uint32_t FIELD_COLOUR[FIELD_COUNT] = {COLOUR_FIELD0, COLOUR_FIELD1};
static const struct rect SUBMIT_RECT = {40, 256, 600, 312};

struct buffer {
	struct wl_buffer *wl;
	uint32_t *pixels;
	size_t size;
	bool busy;
};

struct field {
	const char *name;
	char text[TEXT_CAP];
	size_t len;
};

struct client {
	struct wl_display *display;
	struct wl_registry *registry;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	struct wl_seat *seat;
	struct wl_pointer *pointer;
	struct wl_keyboard *keyboard;

	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;

	int width, height;
	bool configured;
	bool closed;

	struct xkb_context *xkb;
	struct xkb_keymap *keymap;
	struct xkb_state *xkb_state;

	/* Latest surface-local pointer position (from enter/motion); a
	 * `wl_pointer.button` event carries none of its own. */
	int px, py;
	bool have_pos;

	struct field fields[FIELD_COUNT];
	int field_count;
	int focus; /* -1 = nothing focused */
	bool submitted;

	struct buffer buffers[BUFFER_COUNT];
	bool buffers_ready;
	bool dirty;

	uint64_t commits;
};

/* ---- the receipt encoding (restated from the README; Python is the
 * reference) --------------------------------------------------------- */

#define FNV_OFFSET_32 0x811c9dc5u
#define FNV_PRIME_32 0x01000193u

static uint32_t fnv1a32_update(uint32_t hash, const void *data, size_t len) {
	const unsigned char *p = data;
	for (size_t i = 0; i < len; i++) {
		hash ^= p[i];
		hash *= FNV_PRIME_32;
	}
	return hash;
}

/* Band `index`'s colour for `canon`, hashed incrementally so no intermediate
 * buffer bounds the record's length. */
static uint32_t band_rgb(const char *canon, size_t canon_len, int index) {
	char suffix[16];
	int n = snprintf(suffix, sizeof(suffix), "#%d", index);
	uint32_t h = fnv1a32_update(FNV_OFFSET_32, canon, canon_len);
	h = fnv1a32_update(h, suffix, (size_t)(n < 0 ? 0 : n));
	uint32_t r = ((h >> 8) & 0xFu) * 0x11u;
	uint32_t g = ((h >> 4) & 0xFu) * 0x11u;
	uint32_t b = (h & 0xFu) * 0x11u;
	return (r << 16) | (g << 8) | b;
}

/* `canon` for the current field contents: "k0=v0\nk1=v1". Returns the length
 * written, or 0 if it would not fit (which cannot happen for IDL-legal
 * payloads, but is checked rather than assumed). */
static size_t build_canon(const struct client *c, char *out, size_t cap) {
	size_t len = 0;
	for (int i = 0; i < c->field_count; i++) {
		const char *name = c->fields[i].name;
		size_t need = (i > 0 ? 1u : 0u) + strlen(name) + 1u + c->fields[i].len;
		if (len + need + 1u > cap) {
			return 0;
		}
		if (i > 0) {
			out[len++] = '\n';
		}
		size_t nlen = strlen(name);
		memcpy(out + len, name, nlen);
		len += nlen;
		out[len++] = '=';
		memcpy(out + len, c->fields[i].text, c->fields[i].len);
		len += c->fields[i].len;
	}
	out[len] = '\0';
	return len;
}

static void print_hex(const char *data, size_t len) {
	for (size_t i = 0; i < len; i++) {
		printf("%02x", (unsigned char)data[i]);
	}
}

/* ---- drawing --------------------------------------------------------- */

static void fill_rect(struct client *c, struct buffer *b, int x0, int y0, int x1,
		int y1, uint32_t rgb) {
	if (x0 < 0) { x0 = 0; }
	if (y0 < 0) { y0 = 0; }
	if (x1 > c->width) { x1 = c->width; }
	if (y1 > c->height) { y1 = c->height; }
	uint32_t px = pack(rgb);
	for (int y = y0; y < y1; y++) {
		uint32_t *row = b->pixels + (size_t)y * (size_t)c->width;
		for (int x = x0; x < x1; x++) {
			row[x] = px;
		}
	}
}

/* A ring of thickness `w` drawn OUTSIDE `r` (a border) or INSIDE it (a focus
 * ring), selected by the sign of `w`. */
static void ring(struct client *c, struct buffer *b, const struct rect *r, int w,
		bool outside, uint32_t rgb) {
	int x0 = outside ? r->x0 - w : r->x0;
	int y0 = outside ? r->y0 - w : r->y0;
	int x1 = outside ? r->x1 + w : r->x1;
	int y1 = outside ? r->y1 + w : r->y1;
	fill_rect(c, b, x0, y0, x1, y0 + w, rgb);
	fill_rect(c, b, x0, y1 - w, x1, y1, rgb);
	fill_rect(c, b, x0, y0, x0 + w, y1, rgb);
	fill_rect(c, b, x1 - w, y0, x1, y1, rgb);
}

/* One filled ink cell per byte of `text`, left to right, wrapping at the
 * region's right edge and breaking on '\n'. Not a font: see the header. */
static void ink_text(struct client *c, struct buffer *b, const struct rect *region,
		const char *text, size_t len) {
	int x = region->x0 + INK_PAD_X;
	int y = region->y0 + INK_PAD_Y;
	for (size_t i = 0; i < len; i++) {
		if (text[i] == '\n') {
			x = region->x0 + INK_PAD_X;
			y += INK_LINE_H;
			continue;
		}
		if (x + INK_CELL_W > region->x1 - INK_PAD_X) {
			x = region->x0 + INK_PAD_X;
			y += INK_LINE_H;
		}
		if (y + INK_CELL_H > region->y1) {
			return;
		}
		fill_rect(c, b, x, y, x + INK_CELL_W, y + INK_CELL_H, COLOUR_BORDER);
		x += INK_PITCH;
	}
}

static void paint(struct client *c, struct buffer *b) {
	fill_rect(c, b, 0, 0, c->width, c->height, COLOUR_BG);

	if (!c->submitted) {
		for (int i = 0; i < c->field_count; i++) {
			const struct rect *r = &FIELD_RECT[i];
			ring(c, b, r, BORDER_W, true, COLOUR_BORDER);
			fill_rect(c, b, r->x0, r->y0, r->x1, r->y1, FIELD_COLOUR[i]);
			if (c->focus == i) {
				ring(c, b, r, FOCUS_W, false, COLOUR_BORDER);
			}
			ink_text(c, b, r, c->fields[i].text, c->fields[i].len);
		}
		ring(c, b, &SUBMIT_RECT, BORDER_W, true, COLOUR_BORDER);
		fill_rect(c, b, SUBMIT_RECT.x0, SUBMIT_RECT.y0, SUBMIT_RECT.x1,
			SUBMIT_RECT.y1, COLOUR_SUBMIT);
		return;
	}

	/* The confirmation view: the echo strip, then the bands. */
	char canon[TEXT_CAP * 2];
	size_t canon_len = build_canon(c, canon, sizeof(canon));
	struct rect strip = {0, 0, c->width, BAND_TOP};
	ink_text(c, b, &strip, canon, canon_len);

	int span = c->height - BAND_TOP;
	int band_h = span / BAND_COUNT;
	for (int i = 0; i < BAND_COUNT; i++) {
		int y0 = BAND_TOP + i * band_h;
		/* The last band absorbs the remainder, so no unpainted strip is
		 * left when the view height is not divisible by BAND_COUNT. */
		int y1 = (i == BAND_COUNT - 1) ? c->height : y0 + band_h;
		fill_rect(c, b, 0, y0, c->width, y1, band_rgb(canon, canon_len, i));
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
		c->compositor = wl_registry_bind(reg, name, &wl_compositor_interface,
			want > 6 ? 6 : want);
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

static inline bool inside(const struct rect *r, int x, int y) {
	return x >= r->x0 && x < r->x1 && y >= r->y0 && y < r->y1;
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

static void submit(struct client *c) {
	char canon[TEXT_CAP * 2];
	size_t canon_len = build_canon(c, canon, sizeof(canon));
	c->submitted = true;

	/* The byte-exact ground truth, out of band from the pixels: what this
	 * app actually received, hex-encoded so a mangled character and a
	 * correct one cannot render identically (the reason
	 * `input_echo_client.c` prints `TEXT_HEX` beside `TEXT`, and
	 * `gtk_entry_probe.c` prints `ENTRY_HEX`). The band colours are printed
	 * too, so a receipt mismatch is diagnosable without decoding a PNG. */
	printf("SUBMIT fields=%d canon=", c->field_count);
	print_hex(canon, canon_len);
	for (int i = 0; i < c->field_count; i++) {
		printf(" f%d=", i);
		print_hex(c->fields[i].text, c->fields[i].len);
	}
	for (int i = 0; i < BAND_COUNT; i++) {
		printf(" band%d=%06x", i, band_rgb(canon, canon_len, i));
	}
	putchar('\n');
	fflush(stdout);
}

static void ptr_button(void *data, struct wl_pointer *p, uint32_t serial,
		uint32_t time, uint32_t button, uint32_t state) {
	(void)p;
	(void)serial;
	(void)time;
	struct client *c = data;
	if (state != WL_POINTER_BUTTON_STATE_PRESSED || button != BTN_LEFT ||
			!c->have_pos || c->submitted) {
		return;
	}
	for (int i = 0; i < c->field_count; i++) {
		if (inside(&FIELD_RECT[i], c->px, c->py)) {
			c->focus = i;
			c->dirty = true;
			printf("FOCUS field=%d sx=%d sy=%d\n", i, c->px, c->py);
			fflush(stdout);
			return;
		}
	}
	if (inside(&SUBMIT_RECT, c->px, c->py)) {
		submit(c);
		c->dirty = true;
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

/* ---- keyboard (the D7 dynamic keymap, resolved the way a toolkit does) -- */

static void kb_keymap(void *data, struct wl_keyboard *k, uint32_t format,
		int32_t fd, uint32_t size) {
	(void)k;
	struct client *c = data;
	if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1) {
		fprintf(stderr, "form-target: unsupported keymap format %u\n", format);
		close(fd);
		return;
	}
	char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
	if (map == MAP_FAILED) {
		fprintf(stderr, "form-target: cannot mmap the keymap\n");
		close(fd);
		return;
	}
	/* Recompile from scratch on every keymap event -- which is what a
	 * correct toolkit does, and what an app that "reads the keymap once"
	 * does NOT. The shim regenerates its keymap whenever a codepoint is
	 * new (`shim/src/seat.c`), so an app that cached the first one would
	 * silently drop every later character. */
	struct xkb_keymap *keymap = xkb_keymap_new_from_string(c->xkb, map,
		XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
	munmap(map, size);
	close(fd);
	if (keymap == NULL) {
		fprintf(stderr, "form-target: the dynamic keymap did not compile\n");
		return;
	}
	struct xkb_state *state = xkb_state_new(keymap);
	if (state == NULL) {
		xkb_keymap_unref(keymap);
		fprintf(stderr, "form-target: cannot create an xkb state\n");
		return;
	}
	xkb_keymap_unref(c->keymap);
	xkb_state_unref(c->xkb_state);
	c->keymap = keymap;
	c->xkb_state = state;
}

static void kb_enter(void *data, struct wl_keyboard *k, uint32_t serial,
		struct wl_surface *surface, struct wl_array *keys) {
	(void)data;
	(void)k;
	(void)serial;
	(void)surface;
	(void)keys;
}

static void kb_leave(void *data, struct wl_keyboard *k, uint32_t serial,
		struct wl_surface *surface) {
	(void)data;
	(void)k;
	(void)serial;
	(void)surface;
}

static void kb_key(void *data, struct wl_keyboard *k, uint32_t serial,
		uint32_t time, uint32_t key, uint32_t state) {
	(void)k;
	(void)serial;
	(void)time;
	struct client *c = data;
	if (state != WL_KEYBOARD_KEY_STATE_PRESSED || c->xkb_state == NULL) {
		return;
	}
	/* evdev keycode -> xkb keycode is the historical +8 every toolkit
	 * applies; the compositor sends evdev codes on the wire. */
	xkb_keycode_t keycode = key + 8;
	xkb_keysym_t keysym = xkb_state_key_get_one_sym(c->xkb_state, keycode);
	char utf8[16] = {0};
	int n = xkb_state_key_get_utf8(c->xkb_state, keycode, utf8, sizeof(utf8));
	uint32_t cp = xkb_keysym_to_utf32(keysym);

	/* Control characters are routed to actions by a real widget, never into
	 * its buffer -- and `xkb_state_key_get_utf8` renders Escape as U+001B
	 * and BackSpace as U+0008 quite happily, so a naive accumulator would
	 * count command keys as typed text. There is deliberately no Return
	 * handler: submission is a click on the located button (header comment),
	 * so Return here is simply not a character this form accepts. */
	if (n <= 0 || cp < 0x20u || cp == 0x7fu || (cp >= 0x80u && cp <= 0x9fu)) {
		return;
	}
	if (c->focus < 0 || c->submitted) {
		return;
	}
	struct field *f = &c->fields[c->focus];
	size_t add = strlen(utf8);
	if (f->len + add + 1 >= sizeof(f->text)) {
		return;
	}
	memcpy(f->text + f->len, utf8, add);
	f->len += add;
	f->text[f->len] = '\0';
	c->dirty = true;
}

static void kb_modifiers(void *data, struct wl_keyboard *k, uint32_t serial,
		uint32_t depressed, uint32_t latched, uint32_t locked, uint32_t group) {
	(void)k;
	(void)serial;
	struct client *c = data;
	/* A client updates its xkb state from `modifiers`, never from keys;
	 * doing both is the classic double-counting bug. */
	if (c->xkb_state != NULL) {
		xkb_state_update_mask(c->xkb_state, depressed, latched, locked, 0, 0, group);
	}
}

static void kb_repeat_info(void *data, struct wl_keyboard *k, int32_t rate, int32_t delay) {
	(void)data;
	(void)k;
	(void)rate;
	(void)delay;
}

static const struct wl_keyboard_listener keyboard_listener = {
	.keymap = kb_keymap,
	.enter = kb_enter,
	.leave = kb_leave,
	.key = kb_key,
	.modifiers = kb_modifiers,
	.repeat_info = kb_repeat_info,
};

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
	struct client *c = data;
	if ((caps & WL_SEAT_CAPABILITY_POINTER) != 0 && c->pointer == NULL) {
		c->pointer = wl_seat_get_pointer(seat);
		wl_pointer_add_listener(c->pointer, &pointer_listener, c);
	}
	if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) != 0 && c->keyboard == NULL) {
		c->keyboard = wl_seat_get_keyboard(seat);
		wl_keyboard_add_listener(c->keyboard, &keyboard_listener, c);
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

	int fd = memfd_create("form-target", MFD_CLOEXEC);
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
	/* Re-commit each cadence, like `click_target.c`: the content changes
	 * only on a click or a resolved key, and this picks the repaint up on
	 * the next frame after it. */
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
	c->dirty = false;
	wl_surface_attach(c->surface, b->wl, 0, 0);
	wl_surface_damage_buffer(c->surface, 0, 0, c->width, c->height);
	b->busy = true;
	c->commits++;

	struct wl_callback *cb = wl_surface_frame(c->surface);
	wl_callback_add_listener(cb, &frame_listener, c);
	wl_surface_commit(c->surface);
}

static int64_t now_ms(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void usage(const char *argv0) {
	fprintf(stderr,
		"usage: %s [--run-ms MS] [--field NAME]... \n"
		"       %s --bands CANON\n",
		argv0, argv0);
}

int main(int argc, char **argv) {
	int run_ms = 60000;
	struct client c = {.width = 0, .height = 0, .focus = -1};

	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--field") == 0 && i + 1 < argc) {
			if (c.field_count >= FIELD_COUNT) {
				fprintf(stderr, "form-target: at most %d --field names\n", FIELD_COUNT);
				return 2;
			}
			c.fields[c.field_count++].name = argv[++i];
		} else if (strcmp(argv[i], "--bands") == 0 && i + 1 < argc) {
			/* The self-test entry point: compute this canonical record's
			 * three band colours and exit, touching no Wayland at all. It
			 * calls the same `band_rgb` the paint path calls, which is what
			 * lets `tests/integration/test_demo.py` pin this C
			 * implementation against the Python reference on a runner with
			 * no compositor. */
			const char *canon = argv[++i];
			size_t len = strlen(canon);
			printf("BANDS");
			for (int j = 0; j < BAND_COUNT; j++) {
				printf(" %06x", band_rgb(canon, len, j));
			}
			putchar('\n');
			return 0;
		} else {
			usage(argv[0]);
			return 2;
		}
	}
	if (c.field_count == 0) {
		fprintf(stderr, "form-target: needs at least one --field NAME\n");
		usage(argv[0]);
		return 2;
	}

	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	c.xkb = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	if (c.xkb == NULL) {
		fprintf(stderr, "form-target: no xkb context\n");
		return 1;
	}

	c.display = wl_display_connect(NULL);
	if (c.display == NULL) {
		fprintf(stderr, "form-target: cannot connect to the Wayland display\n");
		return 1;
	}
	c.registry = wl_display_get_registry(c.display);
	wl_registry_add_listener(c.registry, &registry_listener, &c);
	wl_display_roundtrip(c.display); /* globals */
	wl_display_roundtrip(c.display); /* seat capabilities */

	if (c.compositor == NULL || c.shm == NULL || c.wm_base == NULL) {
		fprintf(stderr, "form-target: missing wl_compositor, wl_shm or xdg_wm_base\n");
		return 1;
	}
	if (c.seat == NULL) {
		fprintf(stderr, "form-target: the compositor advertised no wl_seat\n");
		return 1;
	}
	xdg_wm_base_add_listener(c.wm_base, &wm_base_listener, &c);

	c.surface = wl_compositor_create_surface(c.compositor);
	c.xdg_surface = xdg_wm_base_get_xdg_surface(c.wm_base, c.surface);
	xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c);
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
	xdg_toplevel_set_title(c.toplevel, "vitrin-form-target");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.form-target");
	wl_surface_commit(c.surface);

	while (!c.configured && wl_display_dispatch(c.display) != -1) {
		if (g_stop) {
			return 0;
		}
	}
	if (c.width <= 0 || c.height <= 0) {
		fprintf(stderr, "form-target: the compositor configured no size\n");
		return 1;
	}
	if (c.width < VIEW_W || c.height < VIEW_H) {
		/* The layout is absolute, authored at VIEW_W x VIEW_H. A smaller
		 * view clips it, which the agent would see as a missing marker --
		 * so say so here rather than let it read as "the app never
		 * painted". */
		fprintf(stderr,
			"form-target: view %dx%d is smaller than the %dx%d this layout is "
			"authored at; the form will be clipped\n",
			c.width, c.height, VIEW_W, VIEW_H);
	}

	printf("FORM size=%dx%d fields=%d", c.width, c.height, c.field_count);
	for (int i = 0; i < c.field_count; i++) {
		printf(" f%d=%d,%d,%d,%d", i, FIELD_RECT[i].x0, FIELD_RECT[i].y0,
			FIELD_RECT[i].x1, FIELD_RECT[i].y1);
	}
	printf(" submit=%d,%d,%d,%d band_top=%d bands=%d\n", SUBMIT_RECT.x0,
		SUBMIT_RECT.y0, SUBMIT_RECT.x1, SUBMIT_RECT.y1, BAND_TOP, BAND_COUNT);
	fflush(stdout);

	if (!buffers_create(&c)) {
		return 1;
	}

	draw(&c);

	int64_t deadline = now_ms() + run_ms;
	while (!g_stop && !c.closed && now_ms() < deadline) {
		/* The prepare/read/dispatch dance (input_echo_client.c's loop) so
		 * the deadline is honoured even when the compositor is quiet. */
		while (wl_display_prepare_read(c.display) != 0) {
			wl_display_dispatch_pending(c.display);
		}
		wl_display_flush(c.display);
		struct pollfd pfd = {.fd = wl_display_get_fd(c.display), .events = POLLIN};
		int64_t remaining = deadline - now_ms();
		int wait = remaining > 100 ? 100 : (remaining > 0 ? (int)remaining : 0);
		if (poll(&pfd, 1, wait) > 0) {
			if (wl_display_read_events(c.display) == -1) {
				break;
			}
		} else {
			wl_display_cancel_read(c.display);
		}
		if (wl_display_dispatch_pending(c.display) == -1) {
			break;
		}
	}

	printf("CLIENT submitted=%d focus=%d commits=%llu size=%dx%d\n",
		c.submitted ? 1 : 0, c.focus, (unsigned long long)c.commits, c.width, c.height);
	fflush(stdout);

	xkb_state_unref(c.xkb_state);
	xkb_keymap_unref(c.keymap);
	xkb_context_unref(c.xkb);
	wl_display_disconnect(c.display);
	return 0;
}
