/* xdg.c -- Phase D (the app's window).
 *
 * Puts the app's xdg_toplevel into the scene graph and holds it at exactly
 * the realm-view size the core announced in `configure`. Without this the
 * shim is a compositor with nothing to composite: P1.6.1 stood up
 * `xdg_wm_base` so the app could bind it, but nothing listened for the
 * windows it created, which is why weston-terminal ran there truly blind.
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

static void toplevel_commit(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, commit);
	if (t->toplevel->base->initial_commit) {
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
	 * NOT popups: `wlr_scene_xdg_surface_create` positions a popup xdg
	 * surface but does not create scene nodes for popups a client makes
	 * later -- that needs an `xdg_shell.events.new_popup` listener, which
	 * this shim does not have yet (tinywl's `server_new_xdg_popup` is the
	 * shape it will take). Until it does, a menu or tooltip is neither
	 * rendered nor clickable. The input path is already ready for them:
	 * seat.c routes by hit-testing this scene, so a popup becomes
	 * addressable the moment it becomes visible, with no change there. */
	t->tree = wlr_scene_xdg_surface_create(&s->scene->tree, toplevel->base);
	if (t->tree == NULL) {
		wlr_log(WLR_ERROR, "cannot add the toplevel to the scene");
		free(t);
		return;
	}
	wlr_scene_node_set_enabled(&t->tree->node, false); /* enabled on map */

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

bool vitrin_setup_xdg(struct vitrin_shim *s) {
	s->new_toplevel.notify = on_new_toplevel;
	wl_signal_add(&s->xdg_shell->events.new_toplevel, &s->new_toplevel);
	s->xdg_wired = true;
	return true;
}
