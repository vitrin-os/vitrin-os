/* xdg.c -- Phase D (the app's window).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * Puts the app's xdg_toplevel into the scene graph and holds it at exactly
 * the realm-view size the core announced in `configure`, and does the same
 * for the popups it opens under it. Without this the shim is a compositor
 * with nothing to composite: P1.6.1 stood up `xdg_wm_base` so the app could
 * bind it, but nothing listened for the windows it created, which is why
 * weston-terminal ran there truly blind.
 *
 * LAYOUT IS ONE RULE: single maximized, at the origin, no decorations. That
 * is the whole of version 1's policy (PRD Doc 2; the core's own scene layer
 * makes the same choice for the realm view, and globals.c already declines
 * server-side decorations). It lives here, in the untrusted shim, rather
 * than in the core -- window management is not the TCB's job.
 *
 * WHY THE APP IS TOLD IT IS MAXIMIZED AND ACTIVATED. Both are honest facts
 * about a realm, not cosmetics: the app owns the entire realm view (so it
 * is maximized) and it is the only window in it (so it is the active one).
 * Apps draw differently when told otherwise -- inset shadows sized for a
 * floating window, a hollow "unfocused" text cursor -- and every such
 * difference would be a visible artifact of the shim rather than of the
 * app.
 */
#define _POSIX_C_SOURCE 200809L

#include <stdlib.h>

#include <wayland-server-core.h>

#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>

#include "server.h"

struct vitrin_toplevel {
	struct vitrin_shim *shim;
	struct wlr_xdg_toplevel *toplevel;
	struct wlr_scene_tree *tree;

	struct wl_listener commit;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener destroy;
	struct wl_listener request_maximize;
	struct wl_listener request_fullscreen;
};

/* A menu, a tooltip, a combo box drop-down: the other half of xdg-shell an app
 * cannot live without. See `on_new_popup` for what the shim owes one. */
struct vitrin_popup {
	struct wlr_xdg_popup *popup;

	struct wl_listener commit;
	struct wl_listener destroy;
};

/* WHAT THE APP IS TOLD IT MAY ASK FOR (`xdg_toplevel.wm_capabilities`, v5+).
 *
 * Exactly the two state requests this shim implements a handler for. wlroots
 * marks `request_maximize` and `request_fullscreen` as the ones a compositor
 * MUST answer with a configure (wlr_xdg_shell.h, on `wlr_xdg_toplevel.events`)
 * and both are wired below. The other two are dropped because there is no
 * mechanism behind them, not to save a round trip: `set_minimized` has nowhere
 * to minimize to -- one app, one realm view, and `request_minimize` is
 * unhandled here -- and `show_window_menu` needs decorations and a window
 * manager, neither of which version 1 has (globals.c declines server-side
 * decorations, `request_show_window_menu` is unhandled).
 *
 * THIS IS A CORRECTION, NOT AN ADDITION, and the distinction matters because
 * the opposite was written down. wlroots seeds all four capabilities into
 * `scheduled` the moment the toplevel is created (0.19.3,
 * `create_xdg_toplevel`, "The first configure event must carry WM
 * capabilities"), so a v5+ client has been receiving `wm_capabilities` from
 * this shim all along -- listing two capabilities the shim does not implement.
 * An app hides or disables the UI for a capability it is not offered
 * (xdg-shell, `xdg_toplevel.wm_capabilities`), so the effect of the wlroots
 * default was a minimize button and a window menu that do nothing: an artifact
 * of the shim showing up in the app's own chrome, which is the same failure
 * the "maximized and activated" note above exists to avoid.
 *
 * Advertising a capability is not a promise to grant every request made under
 * it -- xdg-shell puts that explicitly under compositor policy ("Whether this
 * configure actually sets the window maximized is subject to compositor
 * policies") -- it is a promise that the request is available and answered.
 * Both of these are. */
#define VITRIN_WM_CAPABILITIES \
	(WLR_XDG_TOPLEVEL_WM_CAPABILITIES_MAXIMIZE | \
	 WLR_XDG_TOPLEVEL_WM_CAPABILITIES_FULLSCREEN)

/* The realm view fills the output, so "maximized" and "fullscreen" and "the
 * window" are all the same rectangle. Sending it on the initial commit is
 * required by xdg-shell: the client cannot attach a buffer until it has been
 * configured once. */
static void configure_to_view(struct vitrin_toplevel *t) {
	struct vitrin_shim *s = t->shim;
	wlr_xdg_toplevel_set_size(t->toplevel, s->cfg.width, s->cfg.height);
	wlr_xdg_toplevel_set_maximized(t->toplevel, true);
	wlr_xdg_toplevel_set_activated(t->toplevel, true);
}

/* ON THE INITIAL COMMIT, AND NOT ONE INSTRUCTION EARLIER.
 * `wlr_xdg_toplevel_set_wm_capabilities` schedules a configure, and scheduling
 * one against a surface that has not had its initial commit aborts the whole
 * shim -- the same trap `toplevel_request_maximize` documents below. The
 * initial commit is the first legal moment and it is exactly where xdg-shell
 * wants the event: "compositors must send this event once before the first
 * xdg_surface.configure event", and the first configure is the one this same
 * commit schedules.
 *
 * Per map cycle, not per toplevel. An unmapped-then-remapped surface performs
 * the initial commit again (wlroots clears `initialized` in
 * `reset_xdg_surface`), so this runs again and the new configure cycle also
 * gets its capabilities first. wlroots' own seeding happens once, at toplevel
 * creation, and its `scheduled.fields` bit is cleared when the first configure
 * goes out -- so after a remap wlroots alone would send none. */
static void announce_wm_capabilities(struct vitrin_toplevel *t) {
	/* wlroots asserts the xdg_shell global's version is at least 5 before
	 * touching this field. globals.c creates it at 6, so this never fires
	 * today -- but an assert that takes the realm down with it is not a
	 * thing to leave resting on a constant in another file. */
	if (t->shim->xdg_shell->version < XDG_TOPLEVEL_WM_CAPABILITIES_SINCE_VERSION) {
		return;
	}
	wlr_xdg_toplevel_set_wm_capabilities(t->toplevel, VITRIN_WM_CAPABILITIES);
}

static void toplevel_commit(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, commit);
	if (t->toplevel->base->initial_commit) {
		announce_wm_capabilities(t);
		configure_to_view(t);
	}
}

static void toplevel_map(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, map);
	/* Single-maximized: the window's origin IS the view's origin. */
	wlr_scene_node_set_position(&t->tree->node, 0, 0);
	wlr_scene_node_set_enabled(&t->tree->node, true);
	/* Keyboard focus is synthesized shim-side and held on the app for as
	 * long as it has a window -- version 1's whole focus policy, and the
	 * reason there is no focus event on the wire (seat.h, IDL
	 * `vitrin_shim_seat`). Taken at map rather than at creation because an
	 * unmapped surface cannot legally receive input. */
	vitrin_seat_focus_keyboard(t->shim, t->toplevel->base->surface);
	wlr_log(WLR_INFO, "app window mapped: \"%s\" (%s)",
		t->toplevel->title ? t->toplevel->title : "(untitled)",
		t->toplevel->app_id ? t->toplevel->app_id : "(no app id)");
}

static void toplevel_unmap(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, unmap);
	wlr_scene_node_set_enabled(&t->tree->node, false);
	/* Give the keyboard back before the surface stops being able to hold
	 * it. (Pointer focus needs no such call: wlroots drops it itself when
	 * the surface is destroyed, and an unmapped surface is out of the scene
	 * so the next hit test cannot find it.) */
	vitrin_seat_unfocus_keyboard(t->shim, t->toplevel->base->surface);
}

static void toplevel_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, destroy);
	/* The scene tree is owned by wlr_scene_xdg_surface_create and torn down
	 * with the xdg surface; only our listeners and this record are ours. */
	wl_list_remove(&t->commit.link);
	wl_list_remove(&t->map.link);
	wl_list_remove(&t->unmap.link);
	wl_list_remove(&t->destroy.link);
	wl_list_remove(&t->request_maximize.link);
	wl_list_remove(&t->request_fullscreen.link);
	free(t);
}

/* xdg-shell requires a configure in reply to a state request even when the
 * compositor changes nothing -- not answering is a protocol violation. The
 * answer is always the same here: the window already is the whole view.
 *
 * THE `initialized` GUARD IS NOT DEFENSIVE PROGRAMMING, IT IS THE PROTOCOL.
 * xdg-shell lets a client set its initial state -- `set_maximized`,
 * `set_fullscreen`, `set_title` -- on a brand-new toplevel BEFORE the first
 * commit that makes the surface configurable, and Firefox does exactly that
 * (found by the P1.6.4 bring-up: 140.12.0esr requests maximize during window
 * construction). wlroots answers a configure scheduled against a surface that
 * has not had its initial commit with `assert(surface->initialized)`, so this
 * handler running one instruction too early does not misbehave -- IT ABORTS
 * THE WHOLE SHIM, killing the realm, from an ordinary and legal client
 * request.
 *
 * Doing nothing here is not skipping the answer, it is deferring it to the
 * one place that can send it: `toplevel_commit` configures every toplevel to
 * the view on its initial commit, which is the first moment a configure is
 * legal and is guaranteed to come. So the client's request is honoured with
 * the same geometry either way, one round trip later.
 *
 * Neither weston-terminal nor any test client in this tree reaches this path,
 * which is precisely why the ladder ends at a real browser. */
static void toplevel_request_maximize(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, request_maximize);
	if (!t->toplevel->base->initialized) {
		return;
	}
	configure_to_view(t);
	wlr_xdg_surface_schedule_configure(t->toplevel->base);
}

static void toplevel_request_fullscreen(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, request_fullscreen);
	if (!t->toplevel->base->initialized) {
		return;
	}
	configure_to_view(t);
	wlr_xdg_surface_schedule_configure(t->toplevel->base);
}

static void on_new_toplevel(struct wl_listener *listener, void *data) {
	struct vitrin_shim *s = wl_container_of(listener, s, new_toplevel);
	struct wlr_xdg_toplevel *toplevel = data;

	struct vitrin_toplevel *t = calloc(1, sizeof(*t));
	if (t == NULL) {
		wlr_log(WLR_ERROR, "out of memory tracking a toplevel");
		return;
	}
	t->shim = s;
	t->toplevel = toplevel;

	/* Covers the toplevel and its subsurfaces, and applies the client's
	 * window geometry as a position offset -- so a client with CSD shadow
	 * insets is placed by its geometry rectangle rather than by its buffer,
	 * which is what makes view coordinates land on the right pixels for
	 * every GTK/Qt app.
	 *
	 * This call covers exactly ONE xdg surface -- this one, plus its
	 * subsurfaces. A popup the client creates later is a separate xdg
	 * surface and needs its own node, hung under this one; `on_new_popup`
	 * makes it and finds this tree through `base->data`, set below. */
	t->tree = wlr_scene_xdg_surface_create(&s->scene->tree, toplevel->base);
	if (t->tree == NULL) {
		wlr_log(WLR_ERROR, "cannot add the toplevel to the scene");
		free(t);
		return;
	}
	wlr_scene_node_set_enabled(&t->tree->node, false); /* enabled on map */
	/* The scene tree, published on the xdg surface itself. tinywl's
	 * convention, kept for one reason: a popup is handed its parent as a
	 * bare `wlr_surface`, and the only route from there back to the node it
	 * must hang under is a pointer the compositor left behind. Nested popups
	 * (a submenu off a menu) resolve through the same field on the popup's
	 * own xdg surface, which `on_new_popup` sets. */
	toplevel->base->data = t->tree;

	t->commit.notify = toplevel_commit;
	wl_signal_add(&toplevel->base->surface->events.commit, &t->commit);
	t->map.notify = toplevel_map;
	wl_signal_add(&toplevel->base->surface->events.map, &t->map);
	t->unmap.notify = toplevel_unmap;
	wl_signal_add(&toplevel->base->surface->events.unmap, &t->unmap);
	t->destroy.notify = toplevel_destroy;
	wl_signal_add(&toplevel->events.destroy, &t->destroy);
	t->request_maximize.notify = toplevel_request_maximize;
	wl_signal_add(&toplevel->events.request_maximize, &t->request_maximize);
	t->request_fullscreen.notify = toplevel_request_fullscreen;
	wl_signal_add(&toplevel->events.request_fullscreen, &t->request_fullscreen);
}

/* THE CONFIGURE A POPUP IS OWED, AND WHAT HAPPENS WITHOUT IT.
 *
 * `xdg_surface`'s initial-commit/configure/ack contract binds the ROLE object,
 * not just toplevels: a popup that attaches a buffer having never been
 * configured is killed with `xdg_surface` error 3 (`unconfigured_buffer`).
 * wlroots does not send that configure on the compositor's behalf -- for
 * popups it only rejects a parentless one (`handle_xdg_popup_client_commit`)
 * -- so a compositor with no `new_popup` listener does not merely fail to
 * render menus, it DISCONNECTS every app that opens one. Measured against
 * this shim before this listener existed: `globals-error: seq=13 code=3
 * message="xdg_surface has never been configured"`, and the client gone.
 * `tests/xdg_conformance_client.c` FACT 3 is that measurement, kept.
 *
 * An empty configure is the right answer here for the same reason the
 * toplevel's is a fixed one: version 1 has no window manager and the popup's
 * own positioner already resolved its geometry against the parent. A
 * compositor with a screen to keep popups on would instead reposition here. */
static void popup_commit(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_popup *p = wl_container_of(listener, p, commit);
	if (p->popup->base->initial_commit) {
		wlr_xdg_surface_schedule_configure(p->popup->base);
	}
}

static void popup_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_popup *p = wl_container_of(listener, p, destroy);
	/* Same ownership split as the toplevel: the scene tree belongs to
	 * wlr_scene_xdg_surface_create and dies with the xdg surface. */
	wl_list_remove(&p->commit.link);
	wl_list_remove(&p->destroy.link);
	free(p);
}

static void on_new_popup(struct wl_listener *listener, void *data) {
	/* No `wl_container_of` back to the shim, unlike `on_new_toplevel`: a
	 * popup's geometry comes from its own positioner and its parent, never
	 * from the realm view, and it is placed under the parent's scene node
	 * rather than under the root. Nothing here needs shim state. */
	(void)listener;
	struct wlr_xdg_popup *popup = data;

	struct vitrin_popup *p = calloc(1, sizeof(*p));
	if (p == NULL) {
		wlr_log(WLR_ERROR, "out of memory tracking a popup");
		return;
	}
	p->popup = popup;

	/* Under the parent's node, never under the scene root: a popup is
	 * positioned relative to its parent, and `wlr_scene_xdg_surface_create`
	 * implements exactly that for a popup xdg surface (it tracks
	 * `popup->current.geometry` on every commit). Hanging it off the root
	 * would place a menu at the view origin instead of at its anchor. */
	struct wlr_xdg_surface *parent = popup->parent != NULL
		? wlr_xdg_surface_try_from_wlr_surface(popup->parent) : NULL;
	struct wlr_scene_tree *parent_tree = parent != NULL ? parent->data : NULL;
	if (parent_tree != NULL) {
		popup->base->data = wlr_scene_xdg_surface_create(parent_tree, popup->base);
		if (popup->base->data == NULL) {
			wlr_log(WLR_ERROR, "cannot add the popup to the scene");
		}
	} else {
		/* Only reachable if the parent is a surface this shim never put in
		 * the scene -- there is no such role in the v0 global set (no
		 * layer-shell), so this is a "cannot happen" that MUST NOT be an
		 * assert: tinywl asserts here, and an assert in a shim takes the
		 * realm down over a client's malformed request. The popup is
		 * configured anyway, one line below, so the app keeps running with
		 * an invisible menu rather than being disconnected. */
		wlr_log(WLR_ERROR, "popup with no parent scene node; not rendered");
	}

	/* Wired even when the scene node is missing: the configure is what keeps
	 * the client alive, and it is owed regardless of what gets rendered. */
	p->commit.notify = popup_commit;
	wl_signal_add(&popup->base->surface->events.commit, &p->commit);
	p->destroy.notify = popup_destroy;
	wl_signal_add(&popup->events.destroy, &p->destroy);
}

bool vitrin_setup_xdg(struct vitrin_shim *s) {
	s->new_toplevel.notify = on_new_toplevel;
	wl_signal_add(&s->xdg_shell->events.new_toplevel, &s->new_toplevel);
	s->new_popup.notify = on_new_popup;
	wl_signal_add(&s->xdg_shell->events.new_popup, &s->new_popup);
	s->xdg_wired = true;
	return true;
}
