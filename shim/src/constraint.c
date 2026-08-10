/* constraint.c -- the shim half of pointer constraints. See constraint.h for
 * the whole design: what this relays, what it refuses to decide, and the one
 * rule that matters most (a refusal must not wedge the app).
 *
 * SPDX-License-Identifier: MPL-2.0
 */
#define _POSIX_C_SOURCE 200809L

#include <stdlib.h>
#include <string.h>

#include <pixman.h>
#include <wayland-server-core.h>

#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_pointer_constraints_v1.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/util/log.h>

#include "constraint.h"
#include "server.h"
#include "upstream.h"
#include "vitrin-protocol.h"
#include "wire.h"

/* The frame buffer for one ask. Eight u32 arguments plus the header; a
 * generous fixed buffer, sized like every other encode site in this shim. */
#define VITRIN_CONSTRAINT_FRAME_BUF 128u

/* Sanity, in the style seat.c and clipboard.c already use: this file maps the
 * wire's enum onto wlroots' own, so a renumbering on either side must fail the
 * build rather than mislabel a lock as a confinement. */
_Static_assert(VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_NONE == 0,
	"kind.none must be 0 -- it is the withdrawal and the zeroed-struct value");

static void forget_live(struct vitrin_constraint *c);

/* ---- sending the ask --------------------------------------------------- */

/* Encode and send one `pointer_constraint`.
 *
 * Fire-and-forget (conventions 6.2): there is no reply to wait for and no
 * terminal owed. A send failure is logged and nothing else -- the upstream link
 * is already dying, and the shim's own teardown funnel is what classifies that.
 */
static void send_ask(struct vitrin_shim *s, uint32_t serial, uint32_t surface,
		vitrin_shim_session_pointer_constraint_kind_t kind,
		vitrin_shim_session_pointer_constraint_lifetime_t lifetime,
		int32_t x, int32_t y, uint32_t width, uint32_t height) {
	uint8_t frame[VITRIN_CONSTRAINT_FRAME_BUF];
	vitrin_shim_session_req_pointer_constraint_t msg = {
		.serial = serial,
		.surface = surface,
		.kind = kind,
		.lifetime = lifetime,
		.x = x,
		.y = y,
		.width = width,
		.height = height,
	};
	int32_t n = vitrin_shim_session_req_pointer_constraint_encode(&msg, VITRIN_SESSION_ID,
		frame, sizeof(frame));
	if (n < 0) {
		wlr_log(WLR_ERROR, "constraint: could not encode a pointer_constraint (%d)", n);
		return;
	}
	if (!vitrin_wire_send(&s->up.wire, frame, (size_t)n, -1)) {
		wlr_log(WLR_ERROR, "constraint: could not send a pointer_constraint");
		return;
	}
	wlr_log(WLR_INFO, "constraint: asked serial=%u kind=%d region=%dx%d+%d+%d",
		serial, (int)kind, (int)width, (int)height, x, y);
}

/* Tell the core the app's constraint is gone. Sent with a fresh serial and a
 * null surface, which is what `kind=none` requires -- the core answers against
 * the WITHDRAWN record's serial, not this one, because that is the serial the
 * app's object was created under. */
static void send_withdrawal(struct vitrin_shim *s) {
	struct vitrin_constraint *c = &s->constraint;
	send_ask(s, c->next_serial++, 0u,
		VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_NONE,
		VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_ONESHOT,
		0, 0, 0u, 0u);
}

/* ---- the app's side ---------------------------------------------------- */

static void on_live_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_constraint *c = wl_container_of(listener, c, live_destroy);
	struct vitrin_shim *s = c->shim;
	forget_live(c);
	/* The app destroyed its own lock object (or its surface went away).
	 * Telling the core is not optional: the record is the CORE's, and a
	 * record left behind is a human whose pointer stays frozen and whose
	 * cursor stays hidden for an app that no longer exists. */
	send_withdrawal(s);
}

/* The region changed under a live constraint. Re-asked rather than patched:
 * the wire carries one message and a second ask replaces the first, so a
 * region change is expressed the same way every other change is. */
static void on_live_set_region(struct wl_listener *listener, void *data);

/* Resolve the region to send, in surface-local pixels.
 *
 * `0x0` means the whole surface -- Wayland's null-region meaning, which is what
 * an app that set no region asked for. Otherwise the region's BOUNDING BOX.
 *
 * THE BOUNDING BOX IS A REAL WIDENING and it is named rather than hidden:
 * Wayland allows an arbitrary `wl_region`, the wire carries one rectangle, and
 * an app whose confinement is genuinely non-rectangular gets a larger area than
 * it asked for and is not told. It is the safe direction for the human (a
 * bigger region deactivates the constraint sooner, not later) and the wrong one
 * for the app, which is the trade the IDL records. */
static void resolve_region(const struct wlr_pointer_constraint_v1 *pc,
		int32_t *x, int32_t *y, uint32_t *width, uint32_t *height) {
	*x = 0;
	*y = 0;
	*width = 0u;
	*height = 0u;
	if (!pixman_region32_not_empty((pixman_region32_t *)&pc->region)) {
		/* An empty effective region is not "the whole surface": it is a
		 * region that intersects the surface nowhere. Sending 0x0 would
		 * turn "constrain me to nothing" into "constrain me to
		 * everything", so it is sent as a degenerate 0-sized rectangle at
		 * the origin, which the core answers `inactive` forever. */
		return;
	}
	pixman_box32_t box = *pixman_region32_extents((pixman_region32_t *)&pc->region);
	int32_t w = box.x2 - box.x1;
	int32_t h = box.y2 - box.y1;
	if (w <= 0 || h <= 0) {
		return;
	}
	*x = box.x1;
	*y = box.y1;
	*width = (uint32_t)w;
	*height = (uint32_t)h;
}

/* Ask for `pc`, adopting it as this shim's one live constraint. */
static void adopt(struct vitrin_shim *s, struct wlr_pointer_constraint_v1 *pc) {
	struct vitrin_constraint *c = &s->constraint;
	c->live = pc;
	c->serial = c->next_serial++;
	c->activated = false;
	c->live_destroy.notify = on_live_destroy;
	wl_signal_add(&pc->events.destroy, &c->live_destroy);
	c->live_set_region.notify = on_live_set_region;
	wl_signal_add(&pc->events.set_region, &c->live_set_region);

	int32_t x, y;
	uint32_t width, height;
	resolve_region(pc, &x, &y, &width, &height);
	send_ask(s, c->serial,
		/* This shim composites its app into ONE upstream surface, so the
		 * ask always names that one. The app's own surface-local
		 * coordinates equal this surface's while the toplevel fills the
		 * output, which is what the core's `configure` makes it do
		 * (xdg.c); a toplevel the app sized differently gets a region
		 * offset by the letterbox, which is a real imprecision and is why
		 * the core re-judges the hit test against its own placement
		 * rather than trusting this rectangle. */
		VITRIN_SURFACE_ID,
		pc->type == WLR_POINTER_CONSTRAINT_V1_LOCKED
			? VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_LOCK
			: VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_CONFINE,
		pc->lifetime == ZWP_POINTER_CONSTRAINTS_V1_LIFETIME_PERSISTENT
			? VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_PERSISTENT
			: VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_ONESHOT,
		x, y, width, height);
}

static void on_live_set_region(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_constraint *c = wl_container_of(listener, c, live_set_region);
	if (c->live == NULL) {
		return;
	}
	int32_t x, y;
	uint32_t width, height;
	resolve_region(c->live, &x, &y, &width, &height);
	/* A NEW serial, because it is a new ask: the core answers the old one
	 * `superseded` and this file forgets it below. */
	c->serial = c->next_serial++;
	c->activated = false;
	send_ask(c->shim, c->serial, VITRIN_SURFACE_ID,
		c->live->type == WLR_POINTER_CONSTRAINT_V1_LOCKED
			? VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_LOCK
			: VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_CONFINE,
		c->live->lifetime == ZWP_POINTER_CONSTRAINTS_V1_LIFETIME_PERSISTENT
			? VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_PERSISTENT
			: VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_ONESHOT,
		x, y, width, height);
}

static void forget_live(struct vitrin_constraint *c) {
	if (c->live == NULL) {
		return;
	}
	wl_list_remove(&c->live_destroy.link);
	wl_list_remove(&c->live_set_region.link);
	c->live = NULL;
	c->serial = 0u;
	c->activated = false;
}

static void on_new_constraint(struct wl_listener *listener, void *data) {
	struct vitrin_constraint *c = wl_container_of(listener, c, new_constraint);
	struct wlr_pointer_constraint_v1 *pc = data;
	struct vitrin_shim *s = c->shim;

	/* A second constraint while one is live. The wire carries one record per
	 * realm and a second ask replaces the first, so this side does the same:
	 * the older object is deactivated (it is no longer the one the core will
	 * answer about) and the new one is asked for. It is NOT destroyed --
	 * destroying a client's object out from under it is the wedge this file
	 * exists to avoid, and an inert constraint is a legal Wayland state. */
	if (c->live != NULL) {
		if (c->activated) {
			wlr_pointer_constraint_v1_send_deactivated(c->live);
		}
		forget_live(c);
	}
	adopt(s, pc);
}

/* ---- the core's side --------------------------------------------------- */

void vitrin_constraint_handle_state(struct vitrin_shim *s, const uint8_t *frame, size_t len) {
	struct vitrin_constraint *c = &s->constraint;
	uint32_t object_id = 0;
	vitrin_shim_session_evt_pointer_constraint_state_t ev;
	vitrin_decode_status_t st =
		vitrin_shim_session_evt_pointer_constraint_state_decode(frame, len, -1,
			&object_id, &ev);
	if (st != VITRIN_DECODE_OK) {
		wlr_log(WLR_ERROR, "malformed pointer_constraint_state: %s",
			vitrin_decode_status_string(st));
		return;
	}
	/* A serial this side does not recognise is stale and is ignored -- the
	 * same rule `request_selection` states for its own serial. It happens
	 * legitimately: a verdict about a record the app has already destroyed
	 * crosses the withdrawal on the wire. */
	if (c->live == NULL || ev.serial != c->serial) {
		wlr_log(WLR_DEBUG,
			"pointer_constraint_state serial=%u state=%d ignored: not the live ask",
			ev.serial, (int)ev.state);
		return;
	}

	switch (ev.state) {
	case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_ACTIVE:
		if (!c->activated) {
			wlr_pointer_constraint_v1_send_activated(c->live);
			c->activated = true;
		}
		break;
	case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_INACTIVE:
		if (c->activated) {
			/* May destroy the constraint on a oneshot lifetime, which
			 * is wlroots' documented behaviour and exactly what the
			 * app asked for -- `on_live_destroy` then forgets the
			 * record and withdraws upstream. */
			c->activated = false;
			wlr_pointer_constraint_v1_send_deactivated(c->live);
		}
		break;
	case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_WITHDRAWN:
		if (c->activated) {
			c->activated = false;
			wlr_pointer_constraint_v1_send_deactivated(c->live);
		}
		/* The record is gone core-side. Forget it here too, so a later
		 * verdict against this serial is ignored -- but leave the app's
		 * object alone: it is the app's to destroy. */
		forget_live(c);
		break;
	case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_REFUSED:
		/* NOTHING. Not an error, not a destroy, not a retry. See
		 * constraint.h: an inert `zwp_locked_pointer_v1` is a legal
		 * Wayland state, and it is the only answer that leaves the app
		 * working. */
		wlr_log(WLR_INFO,
			"constraint: the core declined serial=%u; the app's object stays inert",
			ev.serial);
		forget_live(c);
		break;
	case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_SUPERSEDED:
		/* A later ask owns the state. This serial gets nothing further,
		 * and the later ask has already replaced `c->live`, so reaching
		 * here at all means the serial matched a record that is no longer
		 * current -- which the stale check above already excludes. Kept
		 * exhaustive by intent: a new status must be classified here
		 * rather than defaulting to touching the app's object. */
		break;
	}
}

/* ---- bring-up and teardown -------------------------------------------- */

void vitrin_constraint_create(struct vitrin_shim *s) {
	struct vitrin_constraint *c = &s->constraint;
	c->shim = s;
	c->live = NULL;
	c->serial = 0u;
	c->activated = false;
	/* 1, never 0: a zeroed struct must not look like a live record. */
	c->next_serial = 1u;

	c->manager = wlr_pointer_constraints_v1_create(s->display);
	if (c->manager == NULL) {
		wlr_log(WLR_ERROR,
			"zwp_pointer_constraints_v1 could not be created; an app that wants a "
			"pointer lock will never see one activate and keeps an unconstrained "
			"pointer");
		return;
	}
	c->new_constraint.notify = on_new_constraint;
	wl_signal_add(&c->manager->events.new_constraint, &c->new_constraint);
	c->wired = true;
}

void vitrin_constraint_finish(struct vitrin_shim *s) {
	struct vitrin_constraint *c = &s->constraint;
	forget_live(c);
	if (c->wired) {
		wl_list_remove(&c->new_constraint.link);
		c->wired = false;
	}
	c->manager = NULL;
}
