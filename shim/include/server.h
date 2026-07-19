/* server.h -- shared state and phase prototypes for the Vitrin OS Wayland
 * shim (P1.6.1 skeleton, P1.6.2 upstream link).
 *
 * The shim is a tiny wlroots headless-backend compositor that serves exactly
 * one app over one private Wayland socket. It never touches real hardware --
 * the trusted core owns the screen. What it composites, it forwards up to
 * the core over the socketpair it inherited at fork (upstream.h), and the
 * core's frame-done answers are what pace the app.
 *
 * Structural prior art: cage (kiosk compositor) and tinywl, adapted to the
 * wlroots 0.19 API. See docs/plan/01-phase-1-mvp.md (E6, D3, D11).
 */
#ifndef VITRIN_SERVER_H
#define VITRIN_SERVER_H

#include <stdbool.h>
#include <stdint.h>

#include <wayland-server-core.h>

#include "upstream.h"

/* Forward-declare the wlroots types we hold pointers to, so this header
 * stays cheap to include and does not force -DWLR_USE_UNSTABLE on its
 * consumers that only need the struct layout. */
struct wlr_backend;
struct wlr_renderer;
struct wlr_allocator;
struct wlr_output;
struct wlr_output_layout;
struct wlr_scene;
struct wlr_scene_output;
struct wlr_scene_output_layout;
struct wlr_compositor;
struct wlr_xdg_shell;
struct wlr_seat;
struct wlr_xdg_decoration_manager_v1;

struct vitrin_config {
	/* Resolved socket name: --socket NAME > $WAYLAND_DISPLAY > "vitrin-shim-0". */
	const char *socket_name;
	/* Advertise linux-dmabuf-v1 (D3: shm is the mandatory v0 path; dmabuf is
	 * an opt-in, kept out of the default global set). */
	bool dmabuf;
	/* Run with no core connection at all (--no-upstream). Development and
	 * the P1.6.1 globals acceptance test only: with no core there is nobody
	 * to forward frames to and nobody to pace the app, so the shim falls
	 * back to pacing it locally. Holding fd 3 is what makes a process this
	 * realm's shim, so refusing to start without it is the default. */
	bool standalone;
	/* Headless output size. Defaults to 1280x720 and is OVERWRITTEN by the
	 * core's `configure` whenever there is an upstream link -- the realm-view
	 * geometry is the core's to decide, not this process's. */
	int width, height;
};

struct vitrin_shim {
	struct vitrin_config cfg;

	/* Phase A0 -- the core link (upstream.c/wire.c). Opened before any
	 * wlroots object exists, because its `configure` sizes everything. */
	struct vitrin_upstream up;

	/* Phase A -- core. */
	struct wl_display *display;
	struct wl_event_loop *loop;
	struct wlr_backend *backend;
	struct wlr_renderer *renderer;
	struct wlr_allocator *allocator;

	/* Phase B -- protocol globals. */
	struct wlr_compositor *compositor;
	struct wlr_xdg_shell *xdg_shell;
	struct wlr_seat *seat;
	struct wlr_xdg_decoration_manager_v1 *xdg_decoration;
	struct wl_listener new_deco; /* xdg_decoration.new_toplevel_decoration */

	/* Phase C -- output + scene. */
	struct wlr_output *output;
	struct wlr_output_layout *layout;
	struct wlr_scene *scene;
	struct wlr_scene_output *scene_output;
	struct wlr_scene_output_layout *scene_layout;
	struct wl_listener frame;   /* output.frame */
	struct wl_listener destroy; /* output.destroy */

	/* Phase D -- the app's window. `xdg_wired` records that the listener
	 * below was actually attached: bring-up can fail between creating the
	 * xdg_shell global (phase B) and attaching to it (phase D), and teardown
	 * must not `wl_list_remove` a link that was never inserted. */
	bool xdg_wired;
	struct wl_listener new_toplevel; /* xdg_shell.new_toplevel */
};

/* Phase A (server.c): wl_display, event loop, headless backend, renderer,
 * allocator, and the single wl_shm global. */
bool vitrin_backend_bringup(struct vitrin_shim *s);

/* Phase B (globals.c): every protocol global except wl_shm/wl_output, plus
 * the decoration listener that declines server-side decorations. */
bool vitrin_create_globals(struct vitrin_shim *s);

/* Phase C (output.c): one headless output, its wl_output global, the
 * scene<->output-layout wiring, and the frame loop that forwards upstream. */
bool vitrin_setup_output(struct vitrin_shim *s);

/* Phase D (xdg.c): the app's toplevel -> scene wiring and the
 * single-maximized layout. */
bool vitrin_setup_xdg(struct vitrin_shim *s);

/* Phase E (server.c): tear everything down (idempotent). */
void vitrin_shim_finish(struct vitrin_shim *s);

#endif /* VITRIN_SERVER_H */
