/* focus_succession_client.c -- proves, from the app's side of the socket, that
 * a realm with SEVERAL windows hands the keyboard on when one of them closes,
 * hands it to the right one, and still lets go of it when the last one goes.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS EXISTS. Issue #268, found on bare metal (DRM/KMS) on 2026-08-11
 * with alacritty and nautilus running as ONE app in ONE realm: nautilus was
 * launched from the terminal's prompt, mapped over it, took the keyboard, and
 * when it was closed `toplevel_unmap` (src/xdg.c) cleared seat keyboard focus
 * to NOBODY. alacritty was still mapped, still visible, still scrolled under
 * the pointer -- and could not be typed into ever again, because focus is
 * only ever TAKEN at map and the survivor had mapped long before. A session
 * that looks alive and refuses the keyboard is close to session-ending on
 * bare metal, where there is no host compositor to click away to.
 *
 * WHY A CLIENT AND NOT A UNIT TEST. The same argument xdg_conformance_client.c
 * and input_echo_client.c make: what is being asserted is which events cross
 * the wire, in which order, naming which surface -- and only the receiving end
 * can settle that. A test that read `seat->keyboard_state.focused_surface`
 * inside the shim would assert the shim's opinion of itself; one that counts
 * `wl_keyboard.enter`/`leave` asserts what the app was actually told, which is
 * the thing that was broken. It needs no core, no DRM, no GPU and no seat
 * device: `vitrin_seat_init` runs unconditionally at bring-up
 * (src/globals.c), so the virtual keyboard exists and the seat advertises
 * WL_SEAT_CAPABILITY_KEYBOARD even under `--no-upstream`.
 *
 * WHY THREE WINDOWS AND NOT TWO. The bug needs only two, but the FIX names a
 * policy -- "the most recently mapped window that is still mapped" -- and with
 * two windows every candidate policy gives the same answer, so a two-window
 * test would pass against a shim that picked the oldest window, the
 * first-created one, or whatever the list happened to hold first. Three
 * windows CREATED in one order and MAPPED in another separate them: this
 * client creates A, B, C and then maps C, A, B, so the two orders disagree
 * about who should inherit the keyboard when B closes (creation says C,
 * recency says A) and FACT 3 can assert which one the shim actually
 * implements.
 *
 * FACT 1 -- MAPPING A WINDOW MOVES THE KEYBOARD TO IT. `leave` on the old
 * holder, `enter` on the new one. This is the step that made alacritty's block
 * cursor go hollow when nautilus appeared, and it is CORRECT; it is asserted
 * so that a later "fix" cannot make focus sticky and call the bug fixed.
 *
 * FACT 2 -- CLOSING A WINDOW GIVES THE KEYBOARD TO A SURVIVOR, NOT TO NOBODY.
 * The bug itself, inverted into an assertion. The window that unmaps is left
 * ALIVE (unmapped, never destroyed), because the shim unlinks its record in
 * the DESTROY handler, not the unmap one -- so it is still on the list the
 * successor search walks, and handing the keyboard to an unmapped surface
 * would be a worse bug than the one being fixed.
 *
 * FACT 3 -- THE SURVIVOR IS THE MOST RECENTLY MAPPED ONE. See "why three
 * windows" above.
 *
 * FACT 4 -- A BACKGROUND WINDOW CLOSING IS INVISIBLE TO THE FRONT ONE. No
 * keyboard event AND NO CONFIGURE.
 *
 * The configure half is the load-bearing one, and it is worth saying why,
 * because the obvious phrasing of this check is vacuous. It is tempting to
 * write FACT 4 as "the keyboard does not move", justified by "a shim that ran
 * the successor search unconditionally would yank the keyboard out of the
 * window the human is typing into". That justification is FALSE against this
 * shim: `focus_toplevel` keeps the front window at the head of the MRU list,
 * so an unconditional search would find THE CURRENT HOLDER and re-focus it,
 * and `wlr_seat_keyboard_enter` early-returns on an already-focused surface.
 * Measured: delete the guard in `toplevel_unmap` (src/xdg.c, condition 1) and
 * a keyboard-only version of this check stays green. What the guard actually
 * prevents is the `activated` sweep that a successor search drags behind it --
 * a configure sent to every window on screen every time a background one
 * closes, waking apps to re-read state that did not change. So this asserts
 * the configure count too, and that is the assertion that goes red when the
 * guard is removed.
 *
 * FACT 5 -- THE LAST WINDOW CLOSING STILL CLEARS FOCUS. The fix must not
 * become "always keep the keyboard somewhere": when the last mapped window
 * goes, the seat has to let go, or the shim is aiming a keyboard at a surface
 * about to be freed.
 *
 * FACT 6 -- `activated` NAMES THE FRONT WINDOW (issue #268, task 3).
 * `activated` is what an app reads to decide whether to draw a solid or a
 * hollow text cursor. Before the fix, `configure_to_view` set it once at the
 * initial commit and never cleared it, so with two windows BOTH were activated
 * and the flag was lying to one of them -- the same class of shim-visible
 * artifact the header of src/xdg.c exists to forbid. So every
 * `xdg_toplevel.configure` is parsed here and its activated bit applied at the
 * ack, exactly as a real toolkit applies double-buffered configure state.
 *
 * FACT 7 -- AN OPEN MENU DOES NOT BREAK ANY OF THE ABOVE. This is the one
 * lifecycle input that makes the whole mechanism a no-op if it is not handled,
 * and it is not exotic: `xdg_popup.grab` is what every GTK, Qt and Firefox
 * menu takes, and wlroots' `wlr_seat_keyboard_notify_enter` "defers to any
 * keyboard grabs" (wlr_seat.h) with the xdg popup grab's `enter` being a
 * deliberate no-op. So while a menu is open the shim's focus changes are
 * SWALLOWED. Two consequences, one per half of the fix, both measured failing
 * on the tree before the code that makes them pass:
 *
 *   7a. A window mapped while a menu is open is still told it is `activated`.
 *       A version of `toplevel_wants_activated` that asked the seat "does this
 *       surface hold the keyboard" instead of asking the shim's own policy
 *       "is this the front window" configured the newly mapped, on-screen
 *       window as DEACTIVATED -- a hollow cursor on the only window the human
 *       can see, produced by the fix for hollow cursors.
 *
 *   7b. Closing the keyboard holder while a menu is open still hands the
 *       keyboard to the survivor. The handover is deferred, not lost:
 *       `vitrin_xdg_refocus` re-asserts it when the grab ends. Without that,
 *       the survivor never receives `wl_keyboard.enter` and is visible and
 *       untypable forever -- #268's exact end state, reached through a path
 *       the successor search alone does not cover.
 *
 * Prints one line per check and exits non-zero if any of them failed.
 */
#define _GNU_SOURCE /* memfd_create, for the buffers that map the toplevels */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

/* Bind high enough that the modern configure states are reachable, capped at
 * what the shim advertises -- which is what a real toolkit does. */
#define WANT_WM_BASE_VERSION 6
/* `wl_keyboard.enter`/`leave` exist at version 1; nothing here needs more.
 * Capped rather than demanded, so this client cannot fail for a reason that
 * has nothing to do with focus. */
#define WANT_SEAT_VERSION 9

#define WIN_W 200
#define WIN_H 150
/* A, B and C carry FACTS 1-6; D and E carry FACT 7. Fresh toplevels rather
 * than a re-map of A and B on purpose: an unmapped xdg surface goes back to
 * unconfigured (wlroots clears `initialized` in `reset_xdg_surface`), so
 * showing one again means performing the whole initial-commit/configure/ack
 * dance a second time, and getting that wrong is an `unconfigured_buffer`
 * disconnect that would look like a focus failure. */
#define WIN_COUNT 5

struct client;

/* One window. The realm the bug was found in had exactly this shape: one app,
 * one shim, several toplevels. */
struct win {
	const char *name;
	struct wl_surface *surface;
	struct xdg_surface *xdg;
	struct xdg_toplevel *toplevel;
	struct wl_buffer *buffer;

	int surface_configures;
	/* The activated bit as it arrived in the last `xdg_toplevel.configure`
	 * (`pending`) and as it stands after the ack that committed it
	 * (`activated`). Kept apart because configure state is double-buffered:
	 * collapsing them would be this client inventing a protocol of its own. */
	bool pending_activated;
	bool activated;

	int kb_enters;
	int kb_leaves;
};

struct client {
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	struct wl_seat *seat;
	struct wl_keyboard *keyboard;
	uint32_t seat_caps;

	struct win win[WIN_COUNT];
	/* The window named by the last `wl_keyboard.enter`, cleared by the
	 * matching `leave`. NULL means the seat is pointing at nobody -- which is
	 * both the bug's end state and FACT 5's correct one, the difference being
	 * entirely whether another window was still mapped. */
	struct win *kb_focus;
	/* An enter naming a surface this client does not own. Impossible unless
	 * the shim invents one; counted rather than ignored so that it can never
	 * be mistaken for "focus went nowhere". */
	int kb_enters_unknown;
	/* The most recent input-event serial the shim sent, kept for
	 * `xdg_popup.grab` (FACT 7), which xdg-shell requires be made "in
	 * response to a user action". A keyboard enter is the only input serial
	 * this client ever receives. */
	uint32_t last_serial;
};

/* Named indices, because "map C, then A, then B" is the whole point of the
 * layout and `win[2]` would hide it. */
enum { WIN_A = 0, WIN_B = 1, WIN_C = 2, WIN_D = 3, WIN_E = 4 };

static int failures = 0;

static void check(bool ok, const char *what) {
	printf("%s %s\n", ok ? "ok  " : "FAIL", what);
	if (!ok) {
		failures++;
	}
}

/* Two round trips, never one: a configure the shim schedules on the
 * compositor's idle loop -- which is every configure it sends, including the
 * `activated` ones a focus change produces -- lands on the second.
 * xdg_conformance_client.c takes the same precaution for the same reason. */
static bool sync2(struct wl_display *display, const char *where) {
	if (wl_display_roundtrip(display) < 0 || wl_display_roundtrip(display) < 0) {
		fprintf(stderr, "FAIL roundtrip %s; wl_display error %d\n",
			where, wl_display_get_error(display));
		return false;
	}
	return true;
}

/* ---- seat and keyboard ------------------------------------------------ */

static void kb_keymap(void *data, struct wl_keyboard *k, uint32_t format,
		int32_t fd, uint32_t size) {
	(void)data; (void)k; (void)format; (void)size;
	/* Not interpreted here -- input_echo_client.c is the test that compiles
	 * the keymap and resolves keysyms through it. This one only cares who the
	 * keyboard is pointed at. The fd is still closed: a shim that regenerates
	 * its keymap would otherwise leak one per generation into this process. */
	close(fd);
}

static struct win *win_of_surface(struct client *c, struct wl_surface *surface) {
	for (int i = 0; i < WIN_COUNT; i++) {
		if (c->win[i].surface != NULL && c->win[i].surface == surface) {
			return &c->win[i];
		}
	}
	return NULL;
}

static void kb_enter(void *data, struct wl_keyboard *k, uint32_t serial,
		struct wl_surface *surface, struct wl_array *keys) {
	(void)k; (void)keys;
	struct client *c = data;
	c->last_serial = serial;
	struct win *w = win_of_surface(c, surface);
	if (w == NULL) {
		c->kb_enters_unknown++;
		printf("IN keyboard_enter surface=UNKNOWN\n");
		return;
	}
	w->kb_enters++;
	c->kb_focus = w;
	printf("IN keyboard_enter surface=%s\n", w->name);
}

static void kb_leave(void *data, struct wl_keyboard *k, uint32_t serial,
		struct wl_surface *surface) {
	(void)k; (void)serial;
	struct client *c = data;
	struct win *w = win_of_surface(c, surface);
	printf("IN keyboard_leave surface=%s\n", w != NULL ? w->name : "UNKNOWN");
	if (w != NULL) {
		w->kb_leaves++;
	}
	/* Only the holder's leave clears the holder: a `leave` delivered after the
	 * `enter` that replaced it must not blank out the new holder. */
	if (c->kb_focus == w) {
		c->kb_focus = NULL;
	}
}

static void kb_key(void *data, struct wl_keyboard *k, uint32_t serial,
		uint32_t time, uint32_t key, uint32_t state) {
	(void)data; (void)k; (void)serial; (void)time; (void)key; (void)state;
}

static void kb_modifiers(void *data, struct wl_keyboard *k, uint32_t serial,
		uint32_t depressed, uint32_t latched, uint32_t locked, uint32_t group) {
	(void)data; (void)k; (void)serial;
	(void)depressed; (void)latched; (void)locked; (void)group;
}

static void kb_repeat_info(void *data, struct wl_keyboard *k, int32_t rate,
		int32_t delay) {
	(void)data; (void)k; (void)rate; (void)delay;
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
	c->seat_caps = caps;
	if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) != 0 && c->keyboard == NULL) {
		c->keyboard = wl_seat_get_keyboard(seat);
		wl_keyboard_add_listener(c->keyboard, &keyboard_listener, c);
	}
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {
	(void)data; (void)seat; (void)name;
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

/* ---- registry --------------------------------------------------------- */

static void registry_global(void *data, struct wl_registry *registry,
		uint32_t name, const char *iface, uint32_t version) {
	struct client *c = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		c->compositor = wl_registry_bind(registry, name,
			&wl_compositor_interface, version > 4 ? 4 : version);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		c->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		uint32_t v = version > WANT_WM_BASE_VERSION
			? WANT_WM_BASE_VERSION : version;
		c->wm_base = wl_registry_bind(registry, name, &xdg_wm_base_interface, v);
	} else if (strcmp(iface, wl_seat_interface.name) == 0) {
		uint32_t v = version > WANT_SEAT_VERSION ? WANT_SEAT_VERSION : version;
		c->seat = wl_registry_bind(registry, name, &wl_seat_interface, v);
		wl_seat_add_listener(c->seat, &seat_listener, c);
	}
}

static void registry_global_remove(void *data, struct wl_registry *registry,
		uint32_t name) {
	(void)data; (void)registry; (void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

static void wm_base_ping(void *data, struct xdg_wm_base *wm_base,
		uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(wm_base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {
	.ping = wm_base_ping,
};

/* ---- the windows ------------------------------------------------------ */

static void xdg_surface_configure(void *data, struct xdg_surface *xdg_surface,
		uint32_t serial) {
	struct win *w = data;
	w->surface_configures++;
	/* The ack is what commits the double-buffered state that came with this
	 * configure -- so FACT 6's bit becomes current exactly here. */
	xdg_surface_ack_configure(xdg_surface, serial);
	w->activated = w->pending_activated;
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

static void toplevel_configure(void *data, struct xdg_toplevel *toplevel,
		int32_t width, int32_t height, struct wl_array *states) {
	(void)toplevel; (void)width; (void)height;
	struct win *w = data;
	bool activated = false;
	uint32_t *state;
	wl_array_for_each(state, states) {
		if (*state == XDG_TOPLEVEL_STATE_ACTIVATED) {
			activated = true;
		}
	}
	/* ABSENCE IS MEANINGFUL in this array: a configure that does not list
	 * ACTIVATED is the compositor saying the window is not active. So this
	 * assigns rather than ORs -- a client that only ever set the flag could
	 * never observe the half of the bug that is a missing deactivation. */
	w->pending_activated = activated;
}

static void toplevel_close(void *data, struct xdg_toplevel *toplevel) {
	(void)data; (void)toplevel;
}

static void toplevel_configure_bounds(void *data, struct xdg_toplevel *toplevel,
		int32_t width, int32_t height) {
	(void)data; (void)toplevel; (void)width; (void)height;
}

static void toplevel_wm_capabilities(void *data, struct xdg_toplevel *toplevel,
		struct wl_array *capabilities) {
	(void)data; (void)toplevel; (void)capabilities;
}

static const struct xdg_toplevel_listener toplevel_listener = {
	.configure = toplevel_configure,
	.close = toplevel_close,
	.configure_bounds = toplevel_configure_bounds,
	.wm_capabilities = toplevel_wm_capabilities,
};

/* Contents are never inspected -- this test reads events, not pixels -- so
 * this is the smallest correct wl_shm buffer and no more. It exists because a
 * toplevel only MAPS once a buffer is attached and committed, and mapping is
 * what moves the keyboard. */
static struct wl_buffer *make_buffer(struct wl_shm *shm, int w, int h) {
	size_t stride = (size_t)w * 4;
	size_t size = stride * (size_t)h;
	int fd = memfd_create("vitrin-focus", MFD_CLOEXEC);
	if (fd < 0) {
		return NULL;
	}
	if (ftruncate(fd, (off_t)size) < 0) {
		close(fd);
		return NULL;
	}
	void *px = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (px == MAP_FAILED) {
		close(fd);
		return NULL;
	}
	memset(px, 0xff, size); /* opaque white; never inspected */
	munmap(px, size);
	struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)size);
	struct wl_buffer *buf = wl_shm_pool_create_buffer(pool, 0, w, h,
		(int32_t)stride, WL_SHM_FORMAT_XRGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);
	return buf;
}

/* Create the role and perform the buffer-less initial commit xdg-shell
 * requires -- but do NOT map. Creation and mapping are separated on purpose:
 * mapping in a different order from creating is what makes FACT 3 able to tell
 * "most recently mapped" apart from "most recently created", and it is also
 * what a real toolkit does when it builds a dialog before showing it.
 *
 * Setup failures are reported as hard errors rather than failed checks:
 * nothing about focus has been observed yet, so there is no verdict to give. */
static bool win_create(struct win *w, struct client *c, struct wl_display *display,
		const char *name) {
	w->name = name;
	w->surface = wl_compositor_create_surface(c->compositor);
	w->xdg = xdg_wm_base_get_xdg_surface(c->wm_base, w->surface);
	xdg_surface_add_listener(w->xdg, &xdg_surface_listener, w);
	w->toplevel = xdg_surface_get_toplevel(w->xdg);
	xdg_toplevel_add_listener(w->toplevel, &toplevel_listener, w);
	xdg_toplevel_set_title(w->toplevel, name);
	xdg_toplevel_set_app_id(w->toplevel, "org.vitrin.focus-succession");

	wl_surface_commit(w->surface); /* the initial commit: no buffer */
	if (!sync2(display, "after the initial commit")) {
		return false;
	}
	if (w->surface_configures < 1) {
		fprintf(stderr, "FAIL window %s was never configured\n", name);
		return false;
	}
	return true;
}

/* Map: attach a buffer to a configured surface and commit. */
static bool win_map(struct win *w, struct client *c, struct wl_display *display) {
	w->buffer = make_buffer(c->shm, WIN_W, WIN_H);
	if (w->buffer == NULL) {
		fprintf(stderr, "FAIL cannot allocate a wl_shm buffer for %s\n", w->name);
		return false;
	}
	wl_surface_attach(w->surface, w->buffer, 0, 0);
	wl_surface_damage_buffer(w->surface, 0, 0, WIN_W, WIN_H);
	wl_surface_commit(w->surface);
	return sync2(display, "after mapping a window");
}

/* Unmap without destroying: attach a NULL buffer and commit, which is
 * xdg-shell's own way to take a window off screen while keeping the role
 * object alive. Deliberately not `xdg_toplevel_destroy`: the shim unlinks its
 * toplevel record in the DESTROY handler, so an unmapped-but-alive window is
 * still on the list the successor search walks -- which is exactly the state
 * that must not fool it. */
static bool win_unmap(struct win *w, struct wl_display *display) {
	wl_surface_attach(w->surface, NULL, 0, 0);
	wl_surface_commit(w->surface);
	return sync2(display, "after unmapping a window");
}

/* ---- the menu (FACT 7) ------------------------------------------------ */

/* A grabbing popup: a menu, in every toolkit that has one. Only three things
 * about it matter here, and none of them is what it looks like:
 *
 *  - `xdg_popup.grab` takes a KEYBOARD grab on the seat, and from then until
 *    the popup goes away wlroots swallows every focus change the shim asks
 *    for. That is the whole point of the fact.
 *  - the grab must be requested BEFORE the popup maps (wlroots answers a
 *    grab on a mapped popup with `xdg_popup` error 0), so the request goes
 *    between the role creation and the buffer.
 *  - it still needs its own initial commit and its own ack, like every other
 *    xdg surface. `tests/xdg_conformance_client.c` FACT 3 is the test that
 *    this shim sends that configure at all; here it is only a prerequisite. */
struct menu {
	struct wl_surface *surface;
	struct xdg_surface *xdg;
	struct xdg_popup *popup;
	struct wl_buffer *buffer;
	int done;
};

static void menu_xdg_configure(void *data, struct xdg_surface *xdg_surface,
		uint32_t serial) {
	(void)data;
	xdg_surface_ack_configure(xdg_surface, serial);
}

static const struct xdg_surface_listener menu_xdg_listener = {
	.configure = menu_xdg_configure,
};

static void menu_configure(void *data, struct xdg_popup *popup,
		int32_t x, int32_t y, int32_t width, int32_t height) {
	(void)data; (void)popup; (void)x; (void)y; (void)width; (void)height;
}

/* The compositor dismissing the menu. Counted, not acted on: FACT 7b unmaps
 * the popup's parent, and wlroots dismisses a popup whose parent goes away. */
static void menu_popup_done(void *data, struct xdg_popup *popup) {
	(void)popup;
	struct menu *m = data;
	m->done++;
}

static void menu_repositioned(void *data, struct xdg_popup *popup, uint32_t token) {
	(void)data; (void)popup; (void)token;
}

static const struct xdg_popup_listener menu_popup_listener = {
	.configure = menu_configure,
	.popup_done = menu_popup_done,
	.repositioned = menu_repositioned,
};

static bool menu_open(struct menu *m, struct client *c, struct wl_display *display,
		struct win *parent) {
	memset(m, 0, sizeof(*m));
	struct xdg_positioner *pos = xdg_wm_base_create_positioner(c->wm_base);
	xdg_positioner_set_size(pos, 40, 40);
	xdg_positioner_set_anchor_rect(pos, 0, 0, 10, 10);

	m->surface = wl_compositor_create_surface(c->compositor);
	m->xdg = xdg_wm_base_get_xdg_surface(c->wm_base, m->surface);
	xdg_surface_add_listener(m->xdg, &menu_xdg_listener, m);
	m->popup = xdg_surface_get_popup(m->xdg, parent->xdg, pos);
	xdg_popup_add_listener(m->popup, &menu_popup_listener, m);
	xdg_positioner_destroy(pos);

	xdg_popup_grab(m->popup, c->seat, c->last_serial);
	wl_surface_commit(m->surface); /* the initial commit: no buffer */
	if (!sync2(display, "after opening a menu")) {
		return false;
	}
	m->buffer = make_buffer(c->shm, 40, 40);
	if (m->buffer == NULL) {
		fprintf(stderr, "FAIL cannot allocate a wl_shm buffer for the menu\n");
		return false;
	}
	wl_surface_attach(m->surface, m->buffer, 0, 0);
	wl_surface_damage_buffer(m->surface, 0, 0, 40, 40);
	wl_surface_commit(m->surface);
	return sync2(display, "after mapping a menu");
}

/* Destroying the popup is what ends the grab -- it is not enough for the
 * compositor to have sent `popup_done`, which only asks. */
static bool menu_close(struct menu *m, struct wl_display *display) {
	xdg_popup_destroy(m->popup);
	xdg_surface_destroy(m->xdg);
	wl_surface_destroy(m->surface);
	m->popup = NULL;
	return sync2(display, "after closing a menu");
}

int main(void) {
	struct client c = {0};
	struct win *a = &c.win[WIN_A];
	struct win *b = &c.win[WIN_B];
	struct win *w_c = &c.win[WIN_C];

	struct wl_display *display = wl_display_connect(NULL);
	if (display == NULL) {
		fprintf(stderr, "FAIL cannot connect to WAYLAND_DISPLAY\n");
		return 2;
	}

	struct wl_registry *registry = wl_display_get_registry(display);
	wl_registry_add_listener(registry, &registry_listener, &c);
	/* Twice: the first trip delivers the globals, the second the `wl_seat`
	 * capabilities event the keyboard object is created from. */
	if (!sync2(display, "on the registry")) {
		return 2;
	}
	if (c.compositor == NULL || c.shm == NULL || c.wm_base == NULL || c.seat == NULL) {
		fprintf(stderr, "FAIL missing wl_compositor, wl_shm, xdg_wm_base or wl_seat\n");
		return 2;
	}
	xdg_wm_base_add_listener(c.wm_base, &wm_base_listener, &c);
	printf("info seat capabilities=0x%x\n", c.seat_caps);
	check(c.keyboard != NULL,
		"the seat advertises a keyboard with no core attached (--no-upstream)");
	if (c.keyboard == NULL) {
		printf("FAIL (%d failed)\n", failures);
		return 1;
	}

	/* Created A, B, C -- and none of them mapped, so none of them has the
	 * keyboard yet. */
	if (!win_create(a, &c, display, "A") ||
			!win_create(b, &c, display, "B") ||
			!win_create(w_c, &c, display, "C")) {
		return 2;
	}
	check(c.kb_focus == NULL,
		"no window holds the keyboard before any of them has mapped");

	/* ---- FACT 1: mapped C, then A, then B ---- */
	if (!win_map(w_c, &c, display)) {
		return 2;
	}
	check(c.kb_focus == w_c, "C takes the keyboard when it maps (first window)");
	check(w_c->activated, "C is configured activated while it holds the keyboard");

	if (!win_map(a, &c, display)) {
		return 2;
	}
	check(c.kb_focus == a, "A takes the keyboard from C when it maps");
	check(w_c->kb_leaves == 1, "C was told it lost the keyboard");
	/* FACT 6, the half that was broken by omission: before the fix,
	 * `activated` was set once at the initial commit and never cleared, so
	 * this check failed with every window claiming to be the active one. */
	check(!w_c->activated, "C is configured DEactivated once A holds the keyboard");
	check(a->activated, "A is configured activated while it holds the keyboard");

	if (!win_map(b, &c, display)) {
		return 2;
	}
	check(c.kb_focus == b, "B takes the keyboard from A when it maps");
	check(a->kb_leaves == 1, "A was told it lost the keyboard");
	check(!a->activated, "A is configured DEactivated once B holds the keyboard");
	check(b->activated, "B is configured activated while it holds the keyboard");

	/* ---- FACTS 2 and 3: B closes ----
	 * Mapped: A and C. Most recently MAPPED is A; most recently CREATED is C.
	 * The keyboard must go to A. */
	if (!win_unmap(b, display)) {
		return 2;
	}
	check(b->kb_leaves == 1, "B was told it lost the keyboard");
	check(c.kb_focus != NULL,
		"THE BUG (#268): closing one window of several leaves the keyboard "
		"with a survivor, not with nobody");
	check(c.kb_focus == a,
		"the survivor is the most recently MAPPED window (A), not the most "
		"recently created one (C)");
	check(a->kb_enters == 2,
		"A received a second keyboard enter naming its own surface");
	check(w_c->kb_enters == 1, "C was not given the keyboard");
	check(a->activated, "A is configured activated again when it inherits the keyboard");
	check(!w_c->activated, "C stays deactivated");
	check(c.kb_enters_unknown == 0,
		"no keyboard enter named a surface this client does not own");

	/* ---- FACT 4: C closes while A holds the keyboard ---- */
	int a_enters_before = a->kb_enters;
	int a_leaves_before = a->kb_leaves;
	int a_configures_before = a->surface_configures;
	if (!win_unmap(w_c, display)) {
		return 2;
	}
	check(c.kb_focus == a,
		"closing a BACKGROUND window does not disturb the keyboard");
	check(a->kb_enters == a_enters_before && a->kb_leaves == a_leaves_before,
		"the focused window received no keyboard event at all");
	/* The discriminating half of FACT 4 -- see the header. The keyboard
	 * checks above pass with or without `toplevel_unmap`'s condition 1; this
	 * one does not. */
	check(a->surface_configures == a_configures_before,
		"the front window received no CONFIGURE either: a background window "
		"closing does not run the successor search");
	check(a->activated, "the focused window stays activated");

	/* ---- FACT 5: the last mapped window closes ----
	 * B and C are still alive on the shim's list, unmapped. Neither may
	 * inherit anything. */
	if (!win_unmap(a, display)) {
		return 2;
	}
	check(a->kb_leaves == 2, "A was told it lost the keyboard");
	check(c.kb_focus == NULL,
		"unmapping the LAST MAPPED window clears focus (the unmapped-but-alive "
		"windows must not inherit it)");
	check(a->kb_enters == 2 && b->kb_enters == 1 && w_c->kb_enters == 1,
		"no enter was ever sent to an unmapped surface");

	/* ---- FACT 7: everything above, with a menu open ----
	 * A, B and C are all unmapped now, so D and E start from a realm with no
	 * window in it -- the same clean slate the run began with. */
	struct win *d = &c.win[WIN_D];
	struct win *e = &c.win[WIN_E];
	if (!win_create(d, &c, display, "D") || !win_create(e, &c, display, "E")) {
		return 2;
	}
	if (!win_map(d, &c, display)) {
		return 2;
	}
	check(c.kb_focus == d, "D takes the keyboard when it maps");

	/* 7a: a window mapped while a menu is open. The grab keeps the keyboard
	 * on D -- that is wlroots' business and is deliberately NOT asserted
	 * here, because it is not this shim's promise. What IS this shim's
	 * promise is that E, now the front window and the only thing on screen,
	 * is not told it is inactive. */
	struct menu menu;
	if (!menu_open(&menu, &c, display, d)) {
		return 2;
	}
	int e_enters_before = e->kb_enters;
	if (!win_map(e, &c, display)) {
		return 2;
	}
	check(e->activated,
		"a window mapped while a menu holds a keyboard grab is still "
		"configured ACTIVATED (it is the front window)");
	check(!d->activated, "the window behind it is configured DEactivated");

	/* Closing the menu ends the grab, and the deferred handover lands. */
	if (!menu_close(&menu, display)) {
		return 2;
	}
	check(c.kb_focus == e,
		"the keyboard is re-asserted on the front window once the grab ends");
	check(e->kb_enters == e_enters_before + 1,
		"the front window received exactly one enter for that handover");

	/* 7b: #268 itself, with a menu open. E holds the keyboard, a menu is
	 * grabbing on E, and E closes -- the successor search runs, its enter is
	 * swallowed by the grab, and the re-assert has to finish the job. */
	if (!menu_open(&menu, &c, display, e)) {
		return 2;
	}
	int d_enters_before = d->kb_enters;
	if (!win_unmap(e, display)) {
		return 2;
	}
	/* One check, not two, and the enter count is half of it on purpose: this
	 * runs after 7a, so "D holds the keyboard" would ALSO be true of a shim
	 * that never moved the keyboard to E in the first place. Requiring a new
	 * enter is what makes this assert a handover rather than an accident. */
	check(c.kb_focus == d && d->kb_enters == d_enters_before + 1,
		"THE BUG (#268) WITH A MENU OPEN: closing the keyboard holder while "
		"an xdg_popup.grab is live still hands the keyboard to the survivor, "
		"with exactly one enter and not one per deferral");
	check(d->activated, "the survivor is configured activated");
	check(c.kb_enters_unknown == 0,
		"no keyboard enter named a surface this client does not own");
	/* wlroots dismisses a popup whose parent unmapped, so the menu is
	 * already gone; the resource is not, and destroying it is what a real
	 * toolkit does on `popup_done`. */
	if (menu.done > 0 && !menu_close(&menu, display)) {
		return 2;
	}

	int err = wl_display_get_error(display);
	if (err != 0) {
		/* wl_display_get_protocol_error RETURNS the error code and writes the
		 * offending interface and object id through its out-parameters -- in
		 * that order, which is easy to transpose and prints a plausible lie
		 * when you do. */
		uint32_t id = 0;
		const struct wl_interface *iface = NULL;
		uint32_t code = wl_display_get_protocol_error(display, &iface, &id);
		fprintf(stderr, "info disconnected: errno=%d protocol iface=%s id=%u code=%u\n",
			err, iface != NULL ? iface->name : "(none)", id, code);
	}
	check(err == 0, "the whole sequence ran without a protocol error");

	wl_display_disconnect(display);

	printf("%s (%d failed)\n", failures == 0 ? "PASS" : "FAIL", failures);
	return failures == 0 ? 0 : 1;
}
