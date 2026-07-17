/* globals.c -- Phase B (protocol globals).
 *
 * Advertises exactly the v0 global set (wl_shm and wl_output are created
 * elsewhere -- server.c and output.c respectively):
 *
 *   wl_compositor, xdg_wm_base, wl_seat, zxdg_decoration_manager_v1
 *   (+ zwp_linux_dmabuf_v1 iff --dmabuf)
 *
 * The global list is a contract, not a floor (see the issue / plan E6): each
 * constructor below creates exactly one wl_global, and every constructor that
 * would sneak in an extra global (subcompositor, data-device, viewporter,
 * presentation-time, xdg-output, wl_drm) is deliberately NOT called. Additions
 * are driven empirically later by the "globals touched" log (P1.6.4).
 */
#include <wayland-server-core.h>
#include <wayland-server-protocol.h> /* WL_SEAT_CAPABILITY_* */

#include <wlr/render/wlr_renderer.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_linux_dmabuf_v1.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_xdg_decoration_v1.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>

#include "server.h"

/* Decline server-side decorations: whenever a client asks for a decoration
 * object, immediately pin it to client-side (D3/E6: no SSD in v0). */
static void on_new_deco(struct wl_listener *listener, void *data) {
	(void)listener;
	struct wlr_xdg_toplevel_decoration_v1 *deco = data;
	wlr_xdg_toplevel_decoration_v1_set_mode(
		deco, WLR_XDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE);
}

bool vitrin_create_globals(struct vitrin_shim *s) {
	/* wl_compositor (creates only this global; wl_subcompositor is a separate
	 * wlr_subcompositor_create we intentionally never call). */
	s->compositor = wlr_compositor_create(s->display, 6, s->renderer);
	if (s->compositor == NULL) {
		return false;
	}

	/* xdg_wm_base (advertised as "xdg_wm_base" on the wire). */
	s->xdg_shell = wlr_xdg_shell_create(s->display, 6);
	if (s->xdg_shell == NULL) {
		return false;
	}

	/* wl_seat. Capabilities describe child objects only; they do not change
	 * the registry. Virtual-seat input replay lands in P1.6.3. */
	s->seat = wlr_seat_create(s->display, "seat0");
	if (s->seat == NULL) {
		return false;
	}
	wlr_seat_set_capabilities(
		s->seat, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);

	/* zxdg_decoration_manager_v1 + the decline-SSD listener. */
	s->xdg_decoration = wlr_xdg_decoration_manager_v1_create(s->display);
	if (s->xdg_decoration == NULL) {
		return false;
	}
	s->new_deco.notify = on_new_deco;
	wl_signal_add(&s->xdg_decoration->events.new_toplevel_decoration,
		&s->new_deco);

	/* zwp_linux_dmabuf_v1 -- opt-in only (D3: shm is the mandatory v0 path,
	 * dmabuf is a GPU-only optimization). Best-effort: a software renderer
	 * (pixman) has no DRM fd to back it, so we warn and keep serving over shm
	 * rather than killing the shim over an optional accelerated global. */
	if (s->cfg.dmabuf) {
		if (wlr_linux_dmabuf_v1_create_with_renderer(
				s->display, 4, s->renderer) == NULL) {
			wlr_log(WLR_ERROR,
				"linux-dmabuf-v1 requested but the renderer cannot back it "
				"(no DRM fd); continuing with shm only");
		}
	}

	return true;
}
