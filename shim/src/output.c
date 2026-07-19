/* output.c -- Phase C (output + scene) and the frame path.
 *
 * Adds one headless output, publishes its wl_output global, wires it into a
 * wlr_scene via a wlr_output_layout, and drives the loop that turns the
 * app's commits into frames on the core's wire.
 *
 * THE FRAME PATH, per tick:
 *
 *   1. Ask upstream whether a frame may be forwarded AT ALL. If not, skip
 *      the tick without compositing (see the backpressure note below).
 *   2. Build the output state: the scene renders into a swapchain buffer and
 *      reports the damage that buffer accumulated.
 *   3. Forward that buffer and that damage to the core.
 *   4. Commit the output state, which completes the headless frame and
 *      re-arms its timer.
 *
 * WHAT IS DELIBERATELY MISSING FROM THAT LIST: sending the app its
 * `wl_surface.frame` callbacks. P1.6.1 sent them here, from the shim's own
 * headless timer, which let the app free-run at whatever rate this process
 * happened to tick. They now come from `frame_done` (upstream.c), so the
 * app's redraw clock is the core's presentation clock -- the "frame-callback
 * relay, not a local timer" decision of PRD Doc 2 section 4.4, and the
 * reason a slow core slows the app instead of burying it.
 *
 * BACKPRESSURE, and why the check comes BEFORE the composite. The obvious
 * shape -- composite every tick, queue what the core has not taken -- grows
 * a queue of multi-megabyte frames exactly when the core is least able to
 * drain it. The shape here cannot: a tick that may not be forwarded is not
 * composited at all, and nothing is lost by skipping it, because the scene's
 * damage ring still holds every pixel involved and hands it to the next
 * composite that does run. So the shim's worst case is one frame in flight
 * plus one pool slot -- O(1) buffers, regardless of how far behind the core
 * falls -- and intermediate frames are coalesced by composition rather than
 * queued. The cost of skipping is re-arming: the headless output's frame
 * timer is re-armed by a commit, so a skipped tick must be asked for again
 * once the core answers (upstream.c's resume path).
 *
 * WHEN STEP 3 FAILS. Steps 2 and 3 are not independent: by the time the
 * forwarder can fail (no memfd, no free slot, an unreadable composed buffer)
 * the scene's damage has already been spent building the frame. Dropping
 * such a frame silently would not cost one frame, it would freeze the realm
 * for good -- the app is blocked on a `wl_surface.frame` callback that only a
 * `frame_done` can produce, so it damages nothing further, so the output
 * never ticks again. Step 4 is therefore conditional: a frame that did not
 * reach the core is NOT committed, and the tick is asked for again. See
 * output_frame_lost.
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
#include "upstream.h"

/* Consecutive lost frames tolerated before the realm is shut down.
 *
 * The retry below is driven by an idle source, so it is immediate: a fault
 * that outlives a handful of retries is a standing condition (the memfd
 * allocation is refused, the renderer cannot read back), not the one-off
 * glitch a retry exists to absorb. Retrying such a condition forever would
 * spin a core at 100% and flood the log while presenting nothing -- the
 * "shim alive, showing nothing" state that is invisible to the core. Exiting
 * makes it visible: the core's lifecycle layer (P1.5.3) already reads shim
 * death, and a dead realm is diagnosable where a wedged one is not. */
#define VITRIN_MAX_CONSECUTIVE_LOST_FRAMES 8

/* A composed frame did not reach the core (or the output rejected it). Put
 * the tick back rather than letting the frame vanish.
 *
 * The recovery is "do not commit, ask again", and it is exact rather than
 * best-effort, because of where wlroots clears its damage bookkeeping:
 * `wlr_scene_output_build_state` only COPIES the scene's
 * `pending_commit_damage` into the output state; what clears it is the
 * scene's own listener on the output's commit event. So skipping the commit
 * leaves the damage intact, and the retry composites and forwards exactly
 * the frame that was lost -- same pixels, same damage rectangles, nothing
 * over-approximated. (The rendered buffer goes back to the swapchain with
 * its contents, which the damage ring has already accounted for, so the
 * retry's render is a no-op rather than a repeat of the work.)
 *
 * `wlr_output_schedule_frame` is what asks again. It sets `needs_frame` --
 * which is one of the two things `wlr_scene_output_needs_frame` consults, so
 * the retry tick is not dismissed as "nothing changed" -- and, since a frame
 * handler runs with `frame_pending` already cleared, it queues an idle
 * source rather than waiting on the headless output's timer, which only a
 * commit re-arms. Both matter: without the first the next tick returns
 * early, without the second there is no next tick at all. */
static void output_frame_lost(struct vitrin_shim *s, const char *why) {
	s->up.frames_lost++;
	s->up.consecutive_frames_lost++;
	if (s->up.consecutive_frames_lost >= VITRIN_MAX_CONSECUTIVE_LOST_FRAMES) {
		wlr_log(WLR_ERROR,
			"%s, %u frames in a row; the shim cannot present this realm, shutting down",
			why, s->up.consecutive_frames_lost);
		if (s->display != NULL) {
			wl_display_terminate(s->display);
		}
		return;
	}
	wlr_log(WLR_ERROR, "%s; recompositing (attempt %u of %u)",
		why, s->up.consecutive_frames_lost, VITRIN_MAX_CONSECUTIVE_LOST_FRAMES);
	wlr_output_schedule_frame(s->output);
}

static void output_frame(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_shim *s = wl_container_of(listener, s, frame);
	if (s->scene_output == NULL) {
		return;
	}

	if (!wlr_scene_output_needs_frame(s->scene_output)) {
		/* Nothing changed. wlroots re-schedules the output itself when the
		 * scene is damaged again, so returning here idles the shim instead
		 * of spinning it at the headless refresh rate. */
		return;
	}

	if (s->up.active && !vitrin_upstream_can_forward(s)) {
		/* Backpressure. See the header comment: skipping costs nothing
		 * because the damage ring keeps the pixels, and it is what bounds
		 * the shim's memory when the core is slow. */
		s->up.composite_deferred = true;
		s->up.frames_deferred++;
		return;
	}

	struct wlr_output_state st;
	wlr_output_state_init(&st);
	if (!wlr_scene_output_build_state(s->scene_output, &st, NULL)) {
		/* The same shape as a failed forward, and the same fix: the scene
		 * kept its damage (nothing was committed), but nothing re-arms the
		 * output either, so without asking again this tick would be the last
		 * one and the app would wait forever on its frame callback. */
		output_frame_lost(s, "the scene could not build a frame");
		wlr_output_state_finish(&st);
		return;
	}

	bool forwarded = true;
	if ((st.committed & WLR_OUTPUT_STATE_BUFFER) != 0) {
		/* The scene reports damage in output-buffer-local coordinates --
		 * the same space as the buffer we are about to attach -- so it
		 * travels upstream verbatim. When it is absent the forwarder falls
		 * back to full-surface damage rather than guessing. */
		const pixman_region32_t *damage =
			(st.committed & WLR_OUTPUT_STATE_DAMAGE) != 0 ? &st.damage : NULL;
		forwarded = vitrin_upstream_forward(s, st.buffer, damage);
	} else if (s->up.active) {
		/* The scene wanted a frame but produced no new buffer -- an app
		 * commit that requested a frame callback while changing no pixels.
		 * There is nothing for the core to present, so there is no
		 * `frame_done` coming; withholding the callback would deadlock an
		 * app waiting on it. This cannot reintroduce free-running: an app
		 * that is actually drawing produces damage, and damage always takes
		 * the forwarded, core-paced branch above. */
		vitrin_upstream_pace_locally(s);
	}

	if (!forwarded) {
		/* Deliberately NOT committed: the commit is what would discard the
		 * scene damage this frame was built from, and that damage is the
		 * only remaining copy of it. */
		output_frame_lost(s, "the composed frame did not reach the core");
	} else if (st.committed != 0 && !wlr_output_commit_state(s->output, &st)) {
		/* The local presentation failed. Whatever went upstream is already
		 * the core's, but the headless output's frame timer is re-armed by a
		 * successful commit and by nothing else, so without asking again
		 * this output would simply stop ticking. */
		output_frame_lost(s, "the output rejected the frame commit");
	} else {
		s->up.consecutive_frames_lost = 0;
	}
	wlr_output_state_finish(&st);

	if (!s->up.active) {
		/* Standalone (--no-upstream): there is no core to pace the app, so
		 * the shim keeps P1.6.1's self-sustaining loop. Without it the
		 * client throttles itself off after one frame and looks hung. */
		vitrin_upstream_pace_locally(s);
	}
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
	/* cfg.width/height is the core's `configure` geometry whenever there is
	 * an upstream link (upstream.c overwrites it), so the headless output IS
	 * the realm view: one composed buffer, forwarded 1:1, no scaling. */
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
