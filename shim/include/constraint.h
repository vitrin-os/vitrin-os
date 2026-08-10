/* constraint.h -- the shim half of pointer constraints (WS-E.4.2, issue #222).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * ====================================================================
 * WHAT THIS IS
 * ====================================================================
 *
 * `zwp_pointer_constraints_v1` is the interface an app uses to say "pin the
 * pointer to my surface" (a lock) or "keep it inside this region" (a
 * confinement). A first-person game, a 3-D viewport, a drawing tool with a
 * pressure-sensitive canvas: every one of them needs it, and none of them can
 * express the need any other way.
 *
 * THE SHIM CANNOT ANSWER THAT ASK. It owns no cursor, no seat hardware and no
 * screen; the core does, and the core also owns the human's own pointer sprite
 * (`wp_cursor_shape_manager_v1` is deliberately not served, so the app cannot
 * hide it). So this file is a *relay*, and the wire is an ask-and-verdict pair
 * on `vitrin_shim_session`:
 *
 *   pointer_constraint(serial, surface, kind, lifetime, x, y, w, h)
 *                                  shim -> core   "my app asked for this"
 *   pointer_constraint_state(serial, state)
 *                                  core -> shim   "here is what is in force"
 *
 * The verdict is NOT a reply. A constraint's state changes for reasons the app
 * never asked about -- the human switched realms, a consent card went up, the
 * screen locked -- so the core sends one of these per transition, forever,
 * against the serial that named the ask.
 *
 * ====================================================================
 * THE ONE RULE THAT MATTERS MOST: A REFUSAL MUST NOT WEDGE THE APP
 * ====================================================================
 *
 * On `refused` this file does NOTHING AT ALL. It does not destroy the app's
 * object, it does not post an error, and it does not retry. An inert
 * `zwp_locked_pointer_v1` -- created, never activated -- is a legal Wayland
 * state: the protocol explicitly permits a compositor to decline to activate a
 * lock, and an app's own state machine handles that with no vitrin-specific
 * behaviour. Sending an error instead would break the app in a way no core test
 * could see.
 *
 * The same holds for `inactive`, which is not a refusal at all: the pointer is
 * simply not inside the region yet, or the human is looking at another realm.
 * The app waits, and `activated` arrives by itself when the core says so.
 *
 * ====================================================================
 * WHAT THE SHIM DOES NOT DECIDE
 * ====================================================================
 *
 * Whether a constraint is in force, whether the human's cursor is drawn, and
 * whether the app's absolute motion stops. All three are the core's, derived
 * from state this process cannot see. This file's whole judgement is which
 * wlroots call to make on which verdict:
 *
 *   active     -> wlr_pointer_constraint_v1_send_activated
 *   inactive   -> wlr_pointer_constraint_v1_send_deactivated
 *   withdrawn  -> deactivate, and forget the record
 *   refused    -> nothing whatsoever (above)
 *   superseded -> forget the record; a later ask already owns the state
 *
 * ONE CONSTRAINT AT A TIME. The wire carries one record per realm and a second
 * ask replaces the first, so this side keeps one too -- the same discipline
 * seat.c keeps for the single in-flight gesture, and for the same reason: this
 * side must be able to tell a mis-sequenced verdict from a legitimate one
 * WITHOUT trusting the core to be correct. A verdict whose serial is not the
 * live one is stale and is ignored.
 */

#ifndef VITRIN_SHIM_CONSTRAINT_H
#define VITRIN_SHIM_CONSTRAINT_H

#include <stdbool.h>
#include <stdint.h>

#include <wayland-server-core.h>

#include "vitrin-protocol.h"

struct vitrin_shim;
struct wlr_pointer_constraints_v1;
struct wlr_pointer_constraint_v1;

/* The shim's pointer-constraint state. Lives in `struct vitrin_shim`. */
struct vitrin_constraint {
	/* Back-pointer: the wlroots listeners here are reached by
	 * `wl_container_of` from their own links, but the shim is needed to
	 * reach the upstream wire. */
	struct vitrin_shim *shim;

	/* The global, or NULL when it could not be created. Best-effort, like
	 * the relative-pointer manager: an app that never binds it is
	 * unaffected, and one that does simply never sees a lock activate. */
	struct wlr_pointer_constraints_v1 *manager;

	/* Whether the `new_constraint` listener was attached; teardown must not
	 * `wl_list_remove` a link that was never inserted. */
	bool wired;
	struct wl_listener new_constraint;

	/* The one constraint this shim is relaying, or NULL. */
	struct wlr_pointer_constraint_v1 *live;
	struct wl_listener live_destroy;
	struct wl_listener live_set_region;
	/* The serial the live constraint was asked under. A verdict naming any
	 * other serial is stale. */
	uint32_t serial;
	/* Whether the live constraint has been told `activated` and not yet
	 * `deactivated`, so a repeated verdict does not re-send. */
	bool activated;
	/* The next serial to mint. Strictly increasing within the connection,
	 * never reused -- the wire's own rule. Starts at 1 so a zero serial in a
	 * verdict can never match a live record. */
	uint32_t next_serial;
};

/* Create `zwp_pointer_constraints_v1` and attach the `new_constraint`
 * listener. Called from `vitrin_create_globals`.
 *
 * Never fatal: like the two WS-E.4.2 pointer extensions beside it, a shim that
 * cannot create this global keeps serving the whole v0 pointer path, and the
 * app degrades to an unconstrained pointer. */
void vitrin_constraint_create(struct vitrin_shim *s);

/* Handle one `vitrin_shim_session.pointer_constraint_state` from the core.
 * `frame`/`len` are the whole undecoded frame, as upstream.c holds it. */
void vitrin_constraint_handle_state(struct vitrin_shim *s, const uint8_t *frame, size_t len);

/* Release every listener. Idempotent, and safe on a shim whose global was
 * never created. */
void vitrin_constraint_finish(struct vitrin_shim *s);

#endif /* VITRIN_SHIM_CONSTRAINT_H */
