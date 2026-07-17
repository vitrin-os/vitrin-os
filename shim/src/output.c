/* output.c -- Phase C (output + scene).
 *
 * Adds one headless output, publishes its wl_output global, wires it into a
 * wlr_scene via a wlr_output_layout, and runs a self-sustaining frame loop.
 * The headless backend drives .frame on a timer; on each frame we commit the
 * (empty, for now) scene and send frame-done so clients keep drawing rather
 * than freezing after their first buffer. Real upstream buffer forwarding is
 * P1.6.2.
 */
#define _POSIX_C_SOURCE 200809L

#include <time.h>

#include <wayland-server-core.h>

#include <wlr/backend/headless.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_output_layout.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/util/log.h>

#include "server.h"

static void output_frame(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_shim *s = wl_container_of(listener, s, frame);
	if (s->scene_output == NULL) {
		return;
	}
	wlr_scene_output_commit(s->scene_output, NULL);

	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	/* Without this the client throttles itself off after one frame and the
	 * whole thing looks hung. */
	wlr_scene_output_send_frame_done(s->scene_output, &now);
}

static void output_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_shim *s = wl_container_of(listener, s, destroy);
	wl_list_remove(&s->frame.link);
	wl_list_remove(&s->destroy.link);
	s->output = NULL;
	s->scene_output = NULL;
}

bool vitrin_setup_output(struct vitrin_shim *s) {
	s->output = wlr_headless_add_output(s->backend, s->cfg.width, s->cfg.height);
	if (s->output == NULL) {
		wlr_log(WLR_ERROR, "failed to add headless output");
		return false;
	}

	/* Must precede the first commit -- it attaches the swapchain the commit
	 * renders into. Arg order is (output, allocator, renderer). */
	if (!wlr_output_init_render(s->output, s->allocator, s->renderer)) {
		wlr_log(WLR_ERROR, "wlr_output_init_render failed");
		return false;
	}

	/* Enable the output at a custom mode: a headless output has no mode list,
	 * so set_custom_mode (not set_mode) is the only way to give it a size. */
	struct wlr_output_state st;
	wlr_output_state_init(&st);
	wlr_output_state_set_enabled(&st, true);
	wlr_output_state_set_custom_mode(&st, s->cfg.width, s->cfg.height, 0);
	bool committed = wlr_output_commit_state(s->output, &st);
	wlr_output_state_finish(&st);
	if (!committed) {
		wlr_log(WLR_ERROR, "initial output commit failed");
		return false;
	}

	/* Publish wl_output only after the enabling commit (not automatic). */
	wlr_output_create_global(s->output, s->display);

	s->layout = wlr_output_layout_create(s->display);
	if (s->layout == NULL) {
		return false;
	}
	s->scene = wlr_scene_create();
	if (s->scene == NULL) {
		return false;
	}
	s->scene_layout = wlr_scene_attach_output_layout(s->scene, s->layout);
	if (s->scene_layout == NULL) {
		return false;
	}

	struct wlr_output_layout_output *lo =
		wlr_output_layout_add_auto(s->layout, s->output);
	s->scene_output = wlr_scene_output_create(s->scene, s->output);
	if (lo == NULL || s->scene_output == NULL) {
		return false;
	}
	wlr_scene_output_layout_add_output(s->scene_layout, lo, s->scene_output);

	s->frame.notify = output_frame;
	wl_signal_add(&s->output->events.frame, &s->frame);
	s->destroy.notify = output_destroy;
	wl_signal_add(&s->output->events.destroy, &s->destroy);

	return true;
}
