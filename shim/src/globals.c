/* globals.c -- Phase B (protocol globals).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * Advertises exactly the v0 global set (wl_shm and wl_output are created
 * elsewhere -- server.c and output.c respectively):
 *
 *   wl_compositor, wl_subcompositor, xdg_wm_base, wl_seat,
 *   wl_data_device_manager, zxdg_decoration_manager_v1
 *   (+ zwp_linux_dmabuf_v1 iff --dmabuf)
 *
 * The global list is a contract, not a floor (see the issue / plan E6): each
 * constructor below creates exactly one wl_global, and every constructor that
 * would sneak in an extra global (viewporter, presentation-time, xdg-output,
 * wl_drm, primary-selection) is deliberately NOT called.
 *
 * ADDITIONS ARE DRIVEN EMPIRICALLY, AND EACH ONE CITES ITS EVIDENCE.
 * `wl_data_device_manager` was the first (P1.6.3, because no GTK app can
 * receive keyboard input without it) and was argued from a failure
 * reconstructed by hand. `wl_subcompositor` is the second (P1.6.4) and was
 * argued from a log line: the "globals touched" ledger (ledger.h) makes an
 * app's demand for an interface we do not advertise observable, which is what
 * turned "Firefox segfaults" into "Firefox binds wl_subcompositor". Every
 * further addition is expected to cite a `globals-demand` line the same way.
 *
 * THE EVIDENCE FOR AN ADDITION IS A PRE-ADDITION RUN, and it has to be kept as
 * one: `shim/docs/globals-demand-wl_subcompositor-140.12.0esr.log` is that run
 * for this one. A ledger from the shim as it ships cannot contain the demand
 * line, because a probe is never armed for an interface already in the
 * contract (ledger.c, in_v0_contract) -- there, the same interface shows up
 * only as a successful `class=v0` bind, which is the weaker signal ledger.h
 * exists to argue is insufficient. `globals-touched-firefox-140.12.0esr.log`
 * is that shipping-state run, and the argument for every interface
 * deliberately REFUSED is in their companion notes (shim/docs/firefox.md).
 */
#include <wayland-server-core.h>
#include <wayland-server-protocol.h> /* WL_SEAT_CAPABILITY_* */

#include <stdlib.h>

#include <wlr/render/wlr_renderer.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_data_device.h>
#include <wlr/types/wlr_linux_dmabuf_v1.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_subcompositor.h>
#include <wlr/types/wlr_xdg_decoration_v1.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>

#include "ledger.h"
#include "clipboard.h"
#include "server.h"

/* Decline server-side decorations: whenever a client asks for a decoration
 * object, pin it to client-side (D3/E6: no SSD in v0).
 *
 * THE ANSWER CANNOT BE GIVEN WHEN THE QUESTION ARRIVES, which is why this is
 * three functions rather than one. `wlr_xdg_toplevel_decoration_v1_set_mode`
 * schedules a configure, and `wlr_xdg_surface_schedule_configure` asserts
 * `surface->initialized` -- but xdg-decoration REQUIRES the client to create
 * its decoration object before the surface's first commit, so at
 * `new_toplevel_decoration` time the surface is, correctly, not yet
 * initialized. Answering immediately therefore aborts the shim on
 * protocol-correct clients: every real toolkit terminal (alacritty, kitty)
 * dies at startup with
 *
 *     wlr_xdg_surface_schedule_configure: Assertion `surface->initialized'
 *
 * Firefox never binds `zxdg_decoration_manager_v1` at all, which is the only
 * reason the acceptance gates never saw this.
 *
 * So the mode is set at the initial commit instead, exactly where
 * `toplevel_commit` in xdg.c already sends this surface's first configure.
 * A client that creates its decoration AFTER the initial commit -- also legal
 * -- is answered inline, since the assertion's precondition already holds. */
struct vitrin_decoration {
	struct wlr_xdg_toplevel_decoration_v1 *deco;
	struct wl_listener commit;
	struct wl_listener destroy;
};

static void decline_ssd(struct wlr_xdg_toplevel_decoration_v1 *deco) {
	wlr_xdg_toplevel_decoration_v1_set_mode(
		deco, WLR_XDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE);
}

static void deco_commit(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_decoration *d = wl_container_of(listener, d, commit);
	/* Only the initial commit: the mode is pinned once, and a later commit
	 * must not re-send a configure the client did not ask for. */
	if (d->deco->toplevel->base->initial_commit) {
		decline_ssd(d->deco);
	}
}

static void deco_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_decoration *d = wl_container_of(listener, d, destroy);
	wl_list_remove(&d->commit.link);
	wl_list_remove(&d->destroy.link);
	free(d);
}

static void on_new_deco(struct wl_listener *listener, void *data) {
	(void)listener;
	struct wlr_xdg_toplevel_decoration_v1 *deco = data;

	if (deco->toplevel->base->initialized) {
		decline_ssd(deco);
		return;
	}

	struct vitrin_decoration *d = calloc(1, sizeof(*d));
	if (d == NULL) {
		/* Out of memory. Say nothing rather than aborting: the client's
		 * own default is client-side decorations, so the window still
		 * comes up correctly-drawn; it merely never hears it from us. */
		wlr_log(WLR_ERROR, "decoration record allocation failed");
		return;
	}
	d->deco = deco;
	d->commit.notify = deco_commit;
	wl_signal_add(&deco->toplevel->base->surface->events.commit, &d->commit);
	d->destroy.notify = deco_destroy;
	wl_signal_add(&deco->events.destroy, &d->destroy);
}

bool vitrin_create_globals(struct vitrin_shim *s) {
	/* wl_compositor. Creates only this global -- `wl_subcompositor` is a
	 * separate constructor, called deliberately just below rather than
	 * arriving as a side effect of this one. */
	s->compositor = wlr_compositor_create(s->display, 6, s->renderer);
	if (s->compositor == NULL) {
		return false;
	}

	/* wl_subcompositor.
	 *
	 * ADDED EMPIRICALLY IN P1.6.4, and this one was found by the machinery
	 * rather than reconstructed by hand: it traces to the two
	 * `globals-demand: interface=wl_subcompositor` lines (seq=15 and seq=22,
	 * one per connection) in the pre-addition probe run checked in as
	 * shim/docs/globals-demand-wl_subcompositor-140.12.0esr.log. That file's
	 * header says how to regenerate it, and ledger.h explains why a demand for
	 * a global we do not advertise is otherwise invisible.
	 *
	 * IT IS LOAD-BEARING, not a nicety, and the bisection says so in three
	 * measurements of the same build. With the global absent entirely, Firefox
	 * 140.12.0esr creates two `wl_surface`s and SEGFAULTS before it ever makes
	 * an `xdg_surface` -- exit 139, no window mapped, one commit. With it
	 * advertised as an INERT probe, the window maps but only 3 frames are
	 * forwarded, because the content subsurface never composites. Implemented
	 * for real, the same build forwards ~57 and repaints. Both GDK and
	 * Firefox's own `nsWaylandDisplay` bind it, and Firefox's rendering path
	 * puts the page content in a subsurface of the toplevel, so the failure is
	 * not "the browser looks wrong" but "there is no window at all".
	 *
	 * IT GRANTS NOTHING ACROSS THE REALM BOUNDARY, by the same argument that
	 * admitted `wl_data_device_manager` below and for a stronger reason: the
	 * protocol REQUIRES a subsurface and its parent to belong to the same
	 * client, and a shim serves exactly one. So this composes one app's own
	 * surfaces into one window, which is the shim's whole job. It also needs
	 * no change to the frame path -- `wlr_scene_xdg_surface_create` (xdg.c)
	 * already covers a toplevel's subsurfaces, so they composite into the
	 * same buffer and travel upstream as ordinary damage. */
	if (wlr_subcompositor_create(s->display) == NULL) {
		wlr_log(WLR_ERROR, "wlr_subcompositor_create failed");
		return false;
	}

	/* xdg_wm_base (advertised as "xdg_wm_base" on the wire). */
	s->xdg_shell = wlr_xdg_shell_create(s->display, 6);
	if (s->xdg_shell == NULL) {
		return false;
	}

	/* wl_seat. Capabilities describe child objects only; they do not change
	 * the registry.
	 *
	 * POINTER and KEYBOARD, and neither more nor less. Both are now backed
	 * by real replay (P1.6.3, seat.c): pointer motion/button/scroll and key
	 * are exactly the five `vitrin_shim_seat` events, and each maps onto one
	 * of these two capabilities. TOUCH is deliberately absent -- v0's seat
	 * vocabulary has no touch event, so advertising it would invite clients
	 * to bind a `wl_touch` that could never produce anything. */
	s->seat = wlr_seat_create(s->display, "seat0");
	if (s->seat == NULL) {
		return false;
	}
	wlr_seat_set_capabilities(
		s->seat, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);

	/* The virtual keyboard and its dynamic keymap, before the socket is
	 * bound: the app's first `wl_keyboard` bind must already find a keymap
	 * waiting (wlroots sends it at bind time), or its first read is of
	 * nothing at all and every key until the next regeneration is lost. */
	if (!vitrin_seat_init(s)) {
		wlr_log(WLR_ERROR, "seat input replay setup failed");
		return false;
	}

	/* wl_data_device_manager.
	 *
	 * ADDED EMPIRICALLY IN P1.6.3, and this is the empiricism the v0 global
	 * set was always meant to be driven by ("a contract, not a floor";
	 * P1.6.4's "globals touched" log is the systematic version of what
	 * happened here). It is not a nice-to-have: GDK treats it as a hard
	 * prerequisite of having a seat at all. GTK 4 refuses to open the
	 * display outright ("the Wayland compositor does not provide one or more
	 * of the required interfaces"), and GTK 3 opens the display but never
	 * constructs a `GdkSeat`, so its windows appear and receive no keyboard
	 * input whatsoever. Without this global the P1.6.3 acceptance criterion
	 * -- text arriving in a GTK text field -- is not merely unproven, it is
	 * unreachable.
	 *
	 * THIS GLOBAL STILL GRANTS NOTHING ACROSS THE REALM BOUNDARY. A shim
	 * serves exactly one client on exactly one private socket, and
	 * `wl_data_device_manager` mediates selection and drag-and-drop BETWEEN
	 * CLIENTS OF THIS SEAT -- of which there is one. Both ends of any
	 * transfer through it are the same app, so this is app-internal
	 * clipboard: the cut/copy/paste a text entry's own context menu needs.
	 *
	 * WHAT CHANGED (WS-E.2.1, issue #213, D-024). This comment used to add
	 * that "cross-realm clipboard remains exactly where it belongs, unbuilt,
	 * and the core's to mediate when it is built". It is built, and the core
	 * does mediate it -- so that sentence is gone rather than left standing.
	 * The channel does NOT run through this global: it is three
	 * `vitrin_shim_session` messages the CORE initiates, driven by two
	 * physical human chords and reachable by no client at any verb set (see
	 * clipboard.h). What this global gives it is only an app-side clipboard
	 * to read from and to install into.
	 *
	 * The listener that makes even the app-internal half work is wired
	 * immediately below: without a `request_set_selection` handler wlroots
	 * never installs a selection at all, so this global advertised a
	 * clipboard that did nothing even within one app. */
	if (wlr_data_device_manager_create(s->display) == NULL) {
		wlr_log(WLR_ERROR, "wlr_data_device_manager_create failed");
		return false;
	}
	if (!vitrin_clipboard_wire(s)) {
		return false;
	}

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

	/* Last, and only under --probe-globals: the catalogue of interfaces we
	 * advertise WITHOUT implementing, so an app's demand for one is an
	 * observable bind rather than silence (ledger.h). Last so that everything
	 * above it is the real contract and everything after it is not, which is
	 * also the order the ledger reports them in. */
	vitrin_ledger_create_probes(s);

	return true;
}
