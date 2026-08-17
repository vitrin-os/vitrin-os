/* idle.c -- the shim half of idle inhibition. See idle.h for the whole design:
 * what this relays, why there is no verdict to receive, and the one rule that
 * matters most (a leak must not pin the human's panel awake).
 *
 * SPDX-License-Identifier: MPL-2.0
 */
#define _POSIX_C_SOURCE 200809L

#include <stdlib.h>

#include <wayland-server-core.h>

#include <wlr/types/wlr_idle_inhibit_v1.h>
#include <wlr/util/log.h>

#include "idle.h"
#include "server.h"
#include "upstream.h"
#include "vitrin-protocol.h"
#include "wire.h"

/* The frame buffer for one ask: two u32 arguments plus the header. Generous and
 * fixed, sized like every other encode site in this shim. */
#define VITRIN_IDLE_FRAME_BUF 64u

/* Sanity, in the style constraint.c and seat.c already use: `released` is the
 * zero value on the wire AND the safe reading of a byte nobody can trust, so a
 * renumbering on either side must fail the build rather than turn a release into
 * a hold. */
_Static_assert(VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_RELEASED == 0,
	"released must be 0 -- it is the withdrawal and the zeroed-struct value");

/* One watcher per inhibitor object.
 *
 * Heap-allocated rather than a single listener on the manager, and that is
 * wlroots' requirement rather than a preference: `wlr_idle_inhibit_v1.c` emits
 * an inhibitor's destroy signal and then ASSERTS its listener list is empty, so
 * a handler must remove its own link. One link per object is the only shape
 * that can. */
struct vitrin_idle_watch {
	struct vitrin_idle *idle;
	struct wl_listener destroy;
};

/* ---- sending the ask --------------------------------------------------- */

/* Encode and send one `idle_inhibit`.
 *
 * Fire-and-forget (conventions 6.2): there is no reply to wait for, no terminal
 * owed, and -- unlike the pointer constraint -- no verdict to expect either. A
 * send failure is logged and nothing else: the upstream link is already dying
 * and the shim's own teardown funnel is what classifies that. */
static void send_state(struct vitrin_shim *s, uint32_t surface,
		vitrin_shim_session_idle_inhibit_state_t state) {
	uint8_t frame[VITRIN_IDLE_FRAME_BUF];
	vitrin_shim_session_req_idle_inhibit_t msg = {
		.surface = surface,
		.state = state,
	};
	int32_t n = vitrin_shim_session_req_idle_inhibit_encode(&msg, VITRIN_SESSION_ID,
		frame, sizeof(frame));
	if (n < 0) {
		wlr_log(WLR_ERROR, "idle: could not encode an idle_inhibit (%d)", n);
		return;
	}
	if (!vitrin_wire_send(&s->up.wire, frame, (size_t)n, -1)) {
		wlr_log(WLR_ERROR, "idle: could not send an idle_inhibit");
		return;
	}
	wlr_log(WLR_INFO, "idle: told the core state=%d (live inhibitors: %zu)",
		(int)state, s->idle.live);
}

/* Send whatever edge the current count implies, and nothing when there is no
 * edge.
 *
 * The edge is taken against `announced` -- what the CORE was told -- rather than
 * against a previous count, so a path that changed the count twice before
 * reaching here cannot emit two `held`s, and a path that changed it and came
 * back cannot emit none. */
static void announce(struct vitrin_idle *i) {
	bool want = i->live > 0;
	if (want == i->announced) {
		return;
	}
	i->announced = want;
	if (want) {
		/* This shim composites its app into ONE upstream surface, so the
		 * ask always names that one -- the same resolution constraint.c
		 * makes, and for the same reason: the app's own surfaces are
		 * this process's business and the core knows only the one it is
		 * handed frames on. */
		send_state(i->shim, VITRIN_SURFACE_ID,
			VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_HELD);
	} else {
		/* Null surface, which is what `released` requires. */
		send_state(i->shim, 0u,
			VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_RELEASED);
	}
}

/* ---- the app's side ---------------------------------------------------- */

static void on_inhibitor_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_idle_watch *w = wl_container_of(listener, w, destroy);
	struct vitrin_idle *i = w->idle;
	/* FIRST, and not optionally: wlroots asserts this list is empty the
	 * moment this handler returns (idle.h). */
	wl_list_remove(&w->destroy.link);
	if (i->live > 0) {
		i->live--;
	}
	free(w);
	/* Reached on every way an inhibitor can end -- the app destroyed it, its
	 * surface went away, or the client disconnected holding it. The third is
	 * the one that matters: a leaked inhibit releases here rather than
	 * pinning the human's panel awake. */
	announce(i);
}

static void on_new_inhibitor(struct wl_listener *listener, void *data) {
	struct vitrin_idle *i = wl_container_of(listener, i, new_inhibitor);
	struct wlr_idle_inhibitor_v1 *inhibitor = data;

	struct vitrin_idle_watch *w = calloc(1, sizeof(*w));
	if (w == NULL) {
		/* Not fatal, and not silent. Refusing to track the object means
		 * the count would drift low, so the honest thing is to not count
		 * it at all: the app's screen blanks as it did before this global
		 * existed, which is the same degradation a failed
		 * `wlr_idle_inhibit_v1_create` produces. */
		wlr_log(WLR_ERROR,
			"idle: out of memory tracking an inhibitor; this one will not hold the "
			"screen awake");
		return;
	}
	w->idle = i;
	w->destroy.notify = on_inhibitor_destroy;
	wl_signal_add(&inhibitor->events.destroy, &w->destroy);
	i->live++;
	announce(i);
}

/* ---- bring-up and teardown -------------------------------------------- */

void vitrin_idle_create(struct vitrin_shim *s) {
	struct vitrin_idle *i = &s->idle;
	i->shim = s;
	i->live = 0;
	i->announced = false;
	i->wired = false;

	i->manager = wlr_idle_inhibit_v1_create(s->display);
	if (i->manager == NULL) {
		wlr_log(WLR_ERROR,
			"zwp_idle_inhibit_manager_v1 could not be created; an app playing a video "
			"will have the screen blank under it exactly as it did before");
		return;
	}
	i->new_inhibitor.notify = on_new_inhibitor;
	wl_signal_add(&i->manager->events.new_inhibitor, &i->new_inhibitor);
	i->wired = true;
}

void vitrin_idle_finish(struct vitrin_shim *s) {
	struct vitrin_idle *i = &s->idle;
	if (i->wired) {
		wl_list_remove(&i->new_inhibitor.link);
		i->wired = false;
	}
	/* The per-inhibitor watchers are NOT freed here, deliberately: they are
	 * freed by their own handlers, and `wl_display_destroy_clients` (which
	 * server.c calls after this) destroys every inhibitor resource and so
	 * fires every one of them. Freeing them here would leave the signal
	 * holding dangling links for exactly that call.
	 *
	 * What IS done here is telling the core, while the wire is still open:
	 * this shim is going away and its inhibit with it. The core would drop
	 * the record on realm death regardless -- this is the orderly half of a
	 * two-layer defence, not the load-bearing one. */
	i->live = 0;
	announce(i);
	i->manager = NULL;
}
