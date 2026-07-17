/* server.h -- shared state and phase prototypes for the Vitrin OS Wayland
 * shim skeleton (P1.6.1).
 *
 * The shim is a tiny wlroots headless-backend compositor that serves exactly
 * one app over one private Wayland socket. It never touches real hardware --
 * the trusted core owns the screen; a later task (P1.6.2) forwards the app's
 * buffers upstream to the core. This skeleton only stands up the Wayland
 * environment (globals, one headless output, a self-sustaining scene frame
 * loop) so `weston-terminal` runs blind against it.
 *
 * Structural prior art: cage (kiosk compositor) and tinywl, adapted to the
 * wlroots 0.19 API. See docs/plan/01-phase-1-mvp.md (E6, D3, D11).
 */
#ifndef VITRIN_SERVER_H
#define VITRIN_SERVER_H

#include <stdbool.h>
#include <stdint.h>

#include <wayland-server-core.h>

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
	int width, height; /* headless output size; default 1280x720 */
};

struct vitrin_shim {
	struct vitrin_config cfg;

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
};

/* Phase A (server.c): wl_display, event loop, headless backend, renderer,
 * allocator, and the single wl_shm global. */
bool vitrin_backend_bringup(struct vitrin_shim *s);

/* Phase B (globals.c): every protocol global except wl_shm/wl_output, plus
 * the decoration listener that declines server-side decorations. */
bool vitrin_create_globals(struct vitrin_shim *s);

/* Phase C (output.c): one headless output, its wl_output global, the
 * scene<->output-layout wiring, and the self-sustaining frame loop. */
bool vitrin_setup_output(struct vitrin_shim *s);

/* Phase E (server.c): tear everything down (idempotent). */
void vitrin_shim_finish(struct vitrin_shim *s);

#endif /* VITRIN_SERVER_H */
