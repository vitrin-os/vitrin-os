/* server.c -- Phase A (core bring-up) and Phase E (teardown).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * Stands up the wl_display, its event loop, the headless backend, the
 * renderer/allocator, and the single wl_shm global. Headless is chosen
 * explicitly (not wlr_backend_autocreate) so the shim can never grab a DRM
 * device or a seat -- the trusted core owns the real screen.
 */
#include <wayland-server-core.h>

#include <wlr/backend.h>
#include <wlr/backend/headless.h>
#include <wlr/render/allocator.h>
#include <wlr/render/wlr_renderer.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/util/log.h>

#include "ledger.h"
#include "clipboard.h"
#include "server.h"
#include "upstream.h"

bool vitrin_backend_bringup(struct vitrin_shim *s) {
	s->display = wl_display_create();
	if (s->display == NULL) {
		return false;
	}
	/* In wlroots 0.19 the backend is fed the event loop, not the display. */
	s->loop = wl_display_get_event_loop(s->display);

	s->backend = wlr_headless_backend_create(s->loop);
	if (s->backend == NULL) {
		wlr_log(WLR_ERROR, "failed to create headless backend");
		return false;
	}

	s->renderer = wlr_renderer_autocreate(s->backend);
	if (s->renderer == NULL) {
		wlr_log(WLR_ERROR, "failed to create renderer");
		return false;
	}

	s->allocator = wlr_allocator_autocreate(s->backend, s->renderer);
	if (s->allocator == NULL) {
		wlr_log(WLR_ERROR, "failed to create allocator");
		return false;
	}

	/* wl_shm ONLY. wlr_renderer_init_wl_display would additionally advertise
	 * linux-dmabuf and legacy wl_drm globals, which would violate the exact
	 * v0 global contract (dmabuf is opt-in via --dmabuf in globals.c). */
	if (!wlr_renderer_init_wl_shm(s->renderer, s->display)) {
		wlr_log(WLR_ERROR, "wlr_renderer_init_wl_shm failed");
		return false;
	}

	return true;
}

void vitrin_shim_finish(struct vitrin_shim *s) {
	/* wl_display_destroy frees display-owned state -- clients, the wl_shm /
	 * wl_output / xdg_shell / seat / decoration globals, the output layout,
	 * and (via the event loop it owns) the headless backend and its outputs,
	 * which is why we must NOT call wlr_backend_destroy ourselves (backend.h:
	 * "Normally called automatically when the event loop is destroyed"). The
	 * outputs' swapchains are released during that teardown, so the renderer
	 * and allocator must still be alive here -- destroy them, and the
	 * standalone scene graph, only afterwards. Every field is NULL-guarded so
	 * the err path in main() can call this after a partial bring-up. */
	/* Detach our decoration listener first: the manager is a display global,
	 * and wlr asserts its new_toplevel_decoration signal has no listeners left
	 * when wl_display_destroy tears it down. (The output.frame/destroy
	 * listeners are removed by output_destroy when the output is destroyed
	 * during wl_display_destroy, so they need no handling here.) */
	if (s->xdg_decoration != NULL) {
		wl_list_remove(&s->new_deco.link);
		s->xdg_decoration = NULL;
	}
	/* Same reasoning for the xdg-shell listeners: xdg_shell is a display
	 * global, and its new_toplevel and new_popup signals must have no
	 * listeners left when wl_display_destroy tears it down. (Per-toplevel and
	 * per-popup listeners are removed by toplevel_destroy/popup_destroy,
	 * which wl_display_destroy_clients triggers.) Gated on `xdg_wired`, not
	 * on the global's existence: bring-up can fail between the two, and
	 * removing a never-inserted link dereferences NULL. */
	if (s->xdg_wired) {
		wl_list_remove(&s->new_toplevel.link);
		wl_list_remove(&s->new_popup.link);
		s->xdg_wired = false;
	}
	/* The virtual keyboard, likewise before the display goes: it is attached
	 * to the seat (a display global) and holds a listener on its own signals
	 * that `wlr_keyboard_finish` asserts is gone. seat.c owns the ordering;
	 * calling it here is what guarantees it runs on every exit path,
	 * including main()'s partial-bring-up one. */
	vitrin_clipboard_finish(s);
	vitrin_seat_finish(s);
	/* Release the core connection and the frame pool BEFORE the display
	 * goes: the wire's event source belongs to the display's event loop, so
	 * removing it afterwards would touch freed memory. Closing our end also
	 * gives the core its EOF -- the shim-death signal its lifecycle layer
	 * (P1.5.3) reads -- as early as we can honestly send it. */
	vitrin_upstream_finish(s);
	if (s->display != NULL) {
		wl_display_destroy_clients(s->display);
		/* The globals ledger dumps between the two: its client-destroy
		 * listeners fire during destroy_clients (so dumping earlier would
		 * omit the "the app disconnected" records that are half the point),
		 * while its protocol logger and client-created listener are owned by
		 * the display (so dumping later would touch freed memory). */
		vitrin_ledger_finish(s);
		wl_display_destroy(s->display);
		s->display = NULL;
	}
	if (s->scene != NULL) {
		wlr_scene_node_destroy(&s->scene->tree.node);
		s->scene = NULL;
	}
	if (s->allocator != NULL) {
		wlr_allocator_destroy(s->allocator);
		s->allocator = NULL;
	}
	if (s->renderer != NULL) {
		wlr_renderer_destroy(s->renderer);
		s->renderer = NULL;
	}
	/* s->backend was destroyed by wl_display_destroy above; just drop it. */
	s->backend = NULL;
}
