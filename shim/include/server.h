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
#include <sys/types.h> /* pid_t (the app child, spawned by main.c) */

#include <wayland-server-core.h>

#include "ledger.h"
#include "seat.h"
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
	/* Also write the globals ledger (ledger.h) to this path, one record per
	 * line with no log prefix. NULL = shim log only. */
	const char *globals_log;
	/* Advertise the probe catalogue: interfaces the shim does NOT implement,
	 * offered purely so that an app's demand for one becomes an observable
	 * `wl_registry.bind`. A DIAGNOSTIC MODE -- it lies to the client, so it
	 * is off by default and announced loudly when on (ledger.h). */
	bool probe_globals;
	/* Restrict the probe catalogue to this comma-separated list of interface
	 * names, or NULL for all of it. This is what turns the ledger from "what
	 * did the app ask for" into "which of those did it actually NEED": a
	 * whole-catalogue run answers the first question, and bisecting with this
	 * answers the second, without recompiling and without adding anything to
	 * the real global set on a guess. */
	const char *probe_filter;
	/* The app command the core conveyed after `--` (the P1.5.4 / #103 argv
	 * contract: `<shim> [shim-args] -- <app> <app-args>`): the program this
	 * shim forks and execs once its session is up (#104). Points into main()'s
	 * argv -- `argv[argc]` is a guaranteed NULL, so `app_argv` is already the
	 * NULL-terminated vector execv wants; not owned. NULL = spawn nothing (no
	 * `--`, or an empty tail: --no-upstream dev mode and the globals/upstream
	 * acceptance tests run the shim with no app). */
	char **app_argv;
	int app_argc;
};

struct vitrin_shim {
	struct vitrin_config cfg;

	/* The app child (main.c app-spawn, #104): the pid of the forked app, or
	 * -1 when none was spawned. The SIGCHLD handler reaps it and brings the
	 * realm down when it exits (one shim, one app, one universe); SIGTERM/SIGINT
	 * teardown SIGTERMs and reaps it first, so killing the shim never orphans
	 * the app (P1.5.2 verification). */
	pid_t app_pid;

	/* Phase A0 -- the core link (upstream.c/wire.c). Opened before any
	 * wlroots object exists, because its `configure` sizes everything. */
	struct vitrin_upstream up;

	/* The "globals touched" ledger (P1.6.4, ledger.h): what the app was
	 * offered, what it bound, and -- via the probe catalogue -- what it
	 * wanted that the v0 set does not provide. Armed before the socket is
	 * bound, because all the interesting traffic is a client's first
	 * roundtrip. */
	struct vitrin_ledger ledger;

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
	/* Virtual-seat input replay (P1.6.3, seat.h): the dynamic keymap and
	 * the pointer/keyboard state the core's `vitrin_shim_seat` events are
	 * replayed through. Lives beside the `wl_seat` it drives. */
	struct vitrin_seat_replay replay;
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
