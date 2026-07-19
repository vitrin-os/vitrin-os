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
	wlr_log(WLR_INFO, "app window mapped: \"%s\" (%s)",
		t->toplevel->title ? t->toplevel->title : "(untitled)",
		t->toplevel->app_id ? t->toplevel->app_id : "(no app id)");
}

static void toplevel_unmap(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, unmap);
	wlr_scene_node_set_enabled(&t->tree->node, false);
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
 * answer is always the same here: the window already is the whole view. */
static void toplevel_request_maximize(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, request_maximize);
	configure_to_view(t);
	wlr_xdg_surface_schedule_configure(t->toplevel->base);
}

static void toplevel_request_fullscreen(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, request_fullscreen);
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

	/* One call covers the toplevel, its subsurfaces AND its popups, so menus
	 * and tooltips reach the realm view without any extra plumbing -- which
	 * matters for the apps on the P1.6.4 ladder far more than for the
	 * single-rectangle case. */
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
