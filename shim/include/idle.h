/* idle.h -- the shim half of idle inhibition (D-042, issue #306).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * ====================================================================
 * WHAT THIS IS
 * ====================================================================
 *
 * `zwp_idle_inhibit_manager_v1` is the interface an app uses to say "do not
 * blank the screen while this plays". A video player, a slide presenter, a
 * long-running progress view: every one of them needs it, and none of them can
 * express the need any other way -- there is no fallback at all, which is why
 * an app that wants one and cannot have one simply watches the screen go black
 * mid-film.
 *
 * THE SHIM CANNOT ANSWER THAT ASK. A realm has no screen, no idle timer and no
 * display controller; the core has all three. So this file is a *relay*, and
 * the wire is one request on `vitrin_shim_session`:
 *
 *   idle_inhibit(surface, state)   shim -> core   "my app is asking / has stopped"
 *
 * ====================================================================
 * WHY THERE IS NO VERDICT, AND WHY THAT IS NOT A GAP
 * ====================================================================
 *
 * `constraint.c`'s sibling relay has an ask-and-verdict pair, because
 * `zwp_locked_pointer_v1` waits for a `locked` event and would latch forever
 * without one. `zwp_idle_inhibitor_v1` DEFINES NO EVENTS AT ALL: there is
 * nothing to deliver a verdict to, and an app's only observable is whether its
 * screen blanked. A refusal is therefore both unobservable and harmless -- a
 * deployment that blanks nothing has satisfied the ask vacuously -- so this
 * file has no receive half. That is the whole difference between the two
 * relays, and it is the protocol's shape rather than this file's choice.
 *
 * ====================================================================
 * THIS FILE AGGREGATES; THE WIRE CARRIES ONE BIT
 * ====================================================================
 *
 * An app may hold any number of inhibitor objects at once (one per window, one
 * per video element -- Firefox does exactly this), and the core has no use for
 * the count: it is deciding one question about one panel. So this file keeps
 * the count and sends only the EDGES:
 *
 *   0 -> 1 inhibitors   ->   idle_inhibit(surface, held)
 *   1 -> 0 inhibitors   ->   idle_inhibit(null,   released)
 *
 * The bookkeeping belongs on this side because this is the side that can see
 * object lifetimes. Which brings the one rule that matters most.
 *
 * ====================================================================
 * THE ONE RULE THAT MATTERS MOST: A LEAK MUST NOT PIN THE PANEL AWAKE
 * ====================================================================
 *
 * The worst failure this feature can cause is a human whose screen never
 * blanks again because an app that no longer exists is still "asking". So the
 * release is driven by OBJECT DESTRUCTION and never by app cooperation:
 * wlroots destroys an inhibitor when the app destroys it, when its surface goes
 * away, AND when the client disconnects without doing either -- and all three
 * arrive on the same `events.destroy` signal this file listens to. An app that
 * is killed mid-film therefore releases.
 *
 * That is one layer. The core keeps the other: it drops a realm's inhibit when
 * the realm dies, on the funnel every death path reaches. Neither is a
 * substitute for the other -- if the SHIM dies there is nobody left to send a
 * release, and if the shim lives while its app dies the core sees no realm
 * death.
 *
 * ====================================================================
 * A NAMED GAP: THIS FILE COUNTS OBJECTS, NOT VISIBLE SURFACES
 * ====================================================================
 *
 * Wayland's own advice is that "inhibitors should only be in effect while this
 * surface is visible". This file does not implement that sentence, and the core's
 * realm gate does not discharge it either -- the gate asks whether the human is
 * looking at this REALM, never whether a surface is visible.
 *
 * What is counted is live inhibitor OBJECTS. There is no map/unmap listener:
 * wlroots destroys an inhibitor when its surface is DESTROYED, and this file
 * hears that, but a surface that is merely unmapped and still alive keeps its
 * inhibitor and keeps this count above zero. So an app that hides the window it
 * was playing in, without destroying it or its inhibitor, still holds the blank
 * off inside a realm the human is watching.
 *
 * Documented rather than fixed, deliberately. Wiring `map`/`unmap` would add a
 * second lifetime per object to a file whose whole correctness argument is that
 * there is exactly one, and no app in this repo's bring-up evidence
 * (`shim/docs/firefox.md`) has been observed doing it. The IDL states the same
 * gap on the wire, which is where a third-party shim author will read it; this
 * comment states it for whoever changes this file. Reopens on an app measured
 * doing it.
 *
 * ====================================================================
 * A WLROOTS DETAIL THAT IS NOT OPTIONAL
 * ====================================================================
 *
 * `wlr_idle_inhibit_v1.c` emits an inhibitor's `destroy` signal and then
 * asserts its listener list is EMPTY. A handler that does not remove its own
 * link aborts the shim on the next inhibitor teardown. Hence one heap-allocated
 * watcher per inhibitor rather than one listener on the manager: each watcher
 * unhooks itself inside its own handler, which is what that assertion demands.
 */

#ifndef VITRIN_SHIM_IDLE_H
#define VITRIN_SHIM_IDLE_H

#include <stdbool.h>
#include <stddef.h>

#include <wayland-server-core.h>

struct vitrin_shim;
struct wlr_idle_inhibit_manager_v1;

/* The shim's idle-inhibit state. Lives in `struct vitrin_shim`. */
struct vitrin_idle {
	/* Back-pointer: the watchers below are reached by `wl_container_of`
	 * from their own links, but the shim is needed to reach the upstream
	 * wire. */
	struct vitrin_shim *shim;

	/* The global, or NULL when it could not be created. Best-effort, like
	 * every other WS-E global: an app that never binds it is unaffected,
	 * and one that does simply has its screen blank as it did before. */
	struct wlr_idle_inhibit_manager_v1 *manager;

	/* Whether the `new_inhibitor` listener was attached; teardown must not
	 * `wl_list_remove` a link that was never inserted. */
	bool wired;
	struct wl_listener new_inhibitor;

	/* How many inhibitor objects this app is holding. The wire carries the
	 * EDGES of this number against zero and nothing else. */
	size_t live;

	/* Whether `held` has been sent and no `released` has followed, so the
	 * edge is a property of what the CORE was told rather than of a count
	 * this file could recompute differently. */
	bool announced;
};

/* Create `zwp_idle_inhibit_manager_v1` and attach the `new_inhibitor`
 * listener. Called from `vitrin_create_globals`.
 *
 * Never fatal: a shim that cannot create this global keeps serving everything
 * else, and its app's screen blanks on the core's timer as it did before the
 * global existed. */
void vitrin_idle_create(struct vitrin_shim *s);

/* Release every listener and, if one was announced, tell the core the inhibit
 * is gone. Idempotent, and safe on a shim whose global was never created.
 *
 * The release is attempted rather than assumed to arrive: on an orderly exit
 * the core hears it, and on a crash the core's own realm-death path is what
 * drops the record. */
void vitrin_idle_finish(struct vitrin_shim *s);

#endif /* VITRIN_SHIM_IDLE_H */
