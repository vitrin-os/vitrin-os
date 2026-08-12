/* xdg.c -- Phase D (the app's window).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * Puts the app's xdg_toplevel into the scene graph and holds it at exactly
 * the realm-view size the core announced in `configure`, and does the same
 * for the popups it opens under it. Without this the shim is a compositor
 * with nothing to composite: P1.6.1 stood up `xdg_wm_base` so the app could
 * bind it, but nothing listened for the windows it created, which is why
 * weston-terminal ran there truly blind.
 *
 * LAYOUT IS ONE RULE: single maximized, at the origin, no decorations. That
 * is the whole of version 1's policy (PRD Doc 2; the core's own scene layer
 * makes the same choice for the realm view, and globals.c already declines
 * server-side decorations). It lives here, in the untrusted shim, rather
 * than in the core -- window management is not the TCB's job.
 *
 * WHY THE APP IS TOLD IT IS MAXIMIZED AND ACTIVATED. Both are honest facts
 * about a realm, not cosmetics: the app owns the entire realm view (so it
 * is maximized) and its front window is the active one.
 * Apps draw differently when told otherwise -- inset shadows sized for a
 * floating window, a hollow "unfocused" text cursor -- and every such
 * difference would be a visible artifact of the shim rather than of the
 * app.
 *
 * `activated` USED TO BE A CONSTANT HERE, AND THAT WAS A LIE THE MOMENT A
 * REALM HAD TWO WINDOWS (issue #268). The argument above was written as "it
 * is the only window in it", and `configure_to_view` set `activated` to
 * `true` once, at the initial commit, and never revisited it -- so an app
 * that opened a second toplevel (a file chooser, a preferences window, any
 * GTK dialog) had BOTH of them told they were active, while only one of them
 * could hold the keyboard. `activated` is precisely what an app reads to
 * decide whether to draw a solid or a hollow text cursor, so the constant
 * produced exactly the artifact this paragraph exists to forbid, one level
 * down. It now names the front window: see `toplevel_wants_activated`.
 */
#define _POSIX_C_SOURCE 200809L

#include <stdlib.h>

#include <wayland-server-core.h>

#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>

#include "server.h"

struct vitrin_toplevel {
	struct vitrin_shim *shim;
	struct wlr_xdg_toplevel *toplevel;
	struct wlr_scene_tree *tree;
	/* Membership of `vitrin_shim.toplevels`, so a re-sent `configure` can
	 * reach every window (`vitrin_xdg_reconfigure_all`) and so the keyboard
	 * can find a successor when one window of several unmaps
	 * (`toplevel_unmap`). The list is kept MOST RECENTLY FOCUSED FIRST --
	 * see `focus_toplevel`, which is the only thing that reorders it. */
	struct wl_list link;
	/* On screen right now, as opposed to merely alive. The list above
	 * carries every toplevel the client has created, and an app is free to
	 * keep an unmapped-but-undestroyed one around for as long as it likes --
	 * so "still on the list" is NOT "still a window", and handing the
	 * keyboard to an unmapped surface would be a worse bug than the one the
	 * successor search fixes (issue #268).
	 *
	 * Deliberately not read from wlroots' own `base->surface->mapped`, even
	 * though that field says the same thing: the successor search runs
	 * INSIDE this toplevel's own unmap handler, so its answer for the
	 * unmapping surface depends on whether wlroots clears that field before
	 * or after it emits `events.unmap`. This flag is written by exactly two
	 * lines, both in this file, and the unmap one runs before anything reads
	 * it -- so the search cannot pick the window that is going away, and the
	 * proof of that is local to this file rather than to another project's
	 * emit order. */
	bool mapped;

	struct wl_listener commit;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener destroy;
	struct wl_listener request_maximize;
	struct wl_listener request_fullscreen;
};

/* A menu, a tooltip, a combo box drop-down: the other half of xdg-shell an app
 * cannot live without. See `on_new_popup` for what the shim owes one. */
struct vitrin_popup {
	struct wlr_xdg_popup *popup;

	struct wl_listener commit;
	struct wl_listener destroy;
};

/* WHAT THE APP IS TOLD IT MAY ASK FOR (`xdg_toplevel.wm_capabilities`, v5+).
 *
 * Exactly the two state requests this shim implements a handler for. wlroots
 * marks `request_maximize` and `request_fullscreen` as the ones a compositor
 * MUST answer with a configure (wlr_xdg_shell.h, on `wlr_xdg_toplevel.events`)
 * and both are wired below. The other two are dropped because there is no
 * mechanism behind them, not to save a round trip: `set_minimized` has nowhere
 * to minimize to -- one app, one realm view, and `request_minimize` is
 * unhandled here -- and `show_window_menu` needs decorations and a window
 * manager, neither of which version 1 has (globals.c declines server-side
 * decorations, `request_show_window_menu` is unhandled).
 *
 * THIS IS A CORRECTION, NOT AN ADDITION, and the distinction matters because
 * the opposite was written down. wlroots seeds all four capabilities into
 * `scheduled` the moment the toplevel is created (0.19.3,
 * `create_xdg_toplevel`, "The first configure event must carry WM
 * capabilities"), so a v5+ client has been receiving `wm_capabilities` from
 * this shim all along -- listing two capabilities the shim does not implement.
 * An app hides or disables the UI for a capability it is not offered
 * (xdg-shell, `xdg_toplevel.wm_capabilities`), so the effect of the wlroots
 * default was a minimize button and a window menu that do nothing: an artifact
 * of the shim showing up in the app's own chrome, which is the same failure
 * the "maximized and activated" note above exists to avoid.
 *
 * Advertising a capability is not a promise to grant every request made under
 * it -- xdg-shell puts that explicitly under compositor policy ("Whether this
 * configure actually sets the window maximized is subject to compositor
 * policies") -- it is a promise that the request is available and answered.
 * Both of these are. */
#define VITRIN_WM_CAPABILITIES \
	(WLR_XDG_TOPLEVEL_WM_CAPABILITIES_MAXIMIZE | \
	 WLR_XDG_TOPLEVEL_WM_CAPABILITIES_FULLSCREEN)

/* THE FRONT WINDOW OF THE REALM, or NULL when the realm has none on screen.
 *
 * One definition, three callers, on purpose. The list is most-recently-focused
 * first (`focus_toplevel`), so its first MAPPED entry is "the most recently
 * mapped window that is still mapped" -- which is simultaneously the window
 * that should hold the keyboard, the window that should be told it is
 * `activated`, and the successor an unmapping holder hands the keyboard to
 * (issue #268). Writing that rule out three times is how those three answers
 * would drift apart; the drift would be invisible, and it would look exactly
 * like the bug being fixed.
 *
 * Unmapped entries are skipped rather than absent because `toplevel_destroy`,
 * not `toplevel_unmap`, is what unlinks: an app may keep an unmapped toplevel
 * around indefinitely, and the caller inside `toplevel_unmap` has just
 * cleared its own `mapped` flag precisely so that this walk cannot pick the
 * window that is going away. */
static struct vitrin_toplevel *front_toplevel(struct vitrin_shim *s) {
	struct vitrin_toplevel *t;
	wl_list_for_each(t, &s->toplevels, link) {
		if (t->mapped) {
			return t;
		}
	}
	return NULL;
}

/* WHAT `activated` MEANS HERE (issue #268, task 3).
 *
 * xdg-shell: the activated state is the one an app reads to decide whether it
 * is the window the user is interacting with, and it is what drives a solid
 * versus a hollow text cursor, a lit versus a dimmed header bar. With more
 * than one toplevel in a realm there is exactly one window that can honestly
 * carry it: the front one, `front_toplevel` above. The answer is re-stated on
 * every focus change (`sync_activation`).
 *
 * IT IS THE SHIM'S POLICY THAT IS READ HERE, NOT WLROOTS' SEAT STATE, and the
 * difference is not academic. An earlier draft of this fix answered
 * `vitrin_seat_keyboard_focus_is(...)` -- "is this the surface the seat is
 * actually pointing at" -- which reads correct and is not: keyboard focus can
 * be held away from the shim's intent by anything that takes a keyboard grab,
 * and an `xdg_popup.grab` is what EVERY GTK/Qt/Firefox menu takes.
 * `wlr_seat_keyboard_notify_enter` "defers to any keyboard grabs"
 * (wlr_seat.h) and the xdg popup grab's `enter` is a deliberate no-op, so
 * with a menu open the shim's handover is swallowed -- and a version of this
 * function that asked the seat would then configure the window that is
 * physically in front, newly mapped and on screen, as DEACTIVATED. Measured:
 * open a grabbing popup, map a second toplevel, and the second toplevel is
 * told `activated=0` while it is the only thing the human can see. That is
 * the hollow-cursor artifact this file's header exists to forbid, reintroduced
 * by the fix meant to remove it.
 *
 * So `activated` states what the shim INTENDS, the MRU list is the record of
 * that intent, and making the seat agree with the intent is
 * `vitrin_xdg_refocus`'s job -- which runs the moment a grab lets go.
 *
 * THE ONE EXCEPTION IS THE PRE-MAP CONFIGURE, and it is not a fudge. An
 * xdg_toplevel cannot map until it has been configured once (xdg-shell's
 * initial-commit/configure/ack contract), so its first configure necessarily
 * goes out while it is not the front window of anything -- and the very next
 * thing that happens is `toplevel_map`, which makes it exactly that.
 * Answering `false` there would make the app paint its FIRST FRAME in its
 * unfocused colours and repaint a round trip later: a visible flash that
 * belongs to the shim, not to the app. Answering `true` is a prediction the
 * same event cycle fulfils. A toplevel that unmapped and is being brought
 * back performs the initial commit again (wlroots clears `initialized` in
 * `reset_xdg_surface`) and gets the same answer, correctly: it is also about
 * to become the front window at map. */
static bool toplevel_wants_activated(struct vitrin_toplevel *t) {
	if (!t->mapped) {
		return true;
	}
	return front_toplevel(t->shim) == t;
}

/* The realm view fills the output, so "maximized" and "fullscreen" and "the
 * window" are all the same rectangle. Sending it on the initial commit is
 * required by xdg-shell: the client cannot attach a buffer until it has been
 * configured once. */
static void configure_to_view(struct vitrin_toplevel *t) {
	struct vitrin_shim *s = t->shim;
	wlr_xdg_toplevel_set_size(t->toplevel, s->cfg.width, s->cfg.height);
	wlr_xdg_toplevel_set_maximized(t->toplevel, true);
	wlr_xdg_toplevel_set_activated(t->toplevel, toplevel_wants_activated(t));
}

/* Re-state `activated` on every window that is on screen, after the front
 * window has changed. Called from `focus_toplevel` and nowhere else:
 * `activated` is a claim about which window is at the front, and
 * `focus_toplevel` is the only thing that moves a window there.
 *
 * ONLY MAPPED WINDOWS. An unmapped toplevel has no appearance on screen to
 * get wrong, and the one this sweep would otherwise reach is the surface
 * currently unmapping -- sending a state change to a window that is going
 * away, on the way to handing its keyboard to somebody else. It gets its
 * answer from `configure_to_view` on the initial commit it must perform
 * before it can ever be seen again.
 *
 * `initialized` is the protocol guard the whole file uses: a configure
 * scheduled before a surface's first commit is not a protocol error but a
 * wlroots assert, which takes the realm down (see
 * `toplevel_request_maximize`). */
static void sync_activation(struct vitrin_shim *s) {
	struct vitrin_toplevel *t;
	wl_list_for_each(t, &s->toplevels, link) {
		if (!t->mapped || !t->toplevel->base->initialized) {
			continue;
		}
		wlr_xdg_toplevel_set_activated(t->toplevel, toplevel_wants_activated(t));
		wlr_xdg_surface_schedule_configure(t->toplevel->base);
	}
}

/* Make `t` the front window: give it the keyboard, and make every window's
 * `activated` agree.
 *
 * THE LIST IS AN MRU STACK, MAINTAINED HERE. `on_new_toplevel` inserts at the
 * head, so `wl_list_for_each` would otherwise walk newest-CREATED first;
 * moving the newly focused window back to the head makes it walk most
 * recently FOCUSED first instead. That is what turns `front_toplevel` into a
 * first-match walk with no timestamps to keep, and the two orders are not the
 * same list: an app that creates A, B and C and then raises A has changed
 * which window the human was last working in without creating anything.
 *
 * The reorder is safe for the list's other reader -- `vitrin_xdg_reconfigure_all`
 * applies the same rule to every entry and does not care in what order.
 *
 * THE REORDER HAPPENS EVEN IF THE SEAT REFUSES THE FOCUS, and that is the
 * design, not an oversight. `wlr_seat_keyboard_notify_enter` defers to any
 * keyboard grab (wlr_seat.h) and an open menu holds one, so this call can be
 * swallowed. The list records what the shim MEANT; `vitrin_xdg_refocus` is
 * what makes the seat catch up when the grab ends. Recording intent only on
 * success would leave the shim with no memory of what it wanted, which is the
 * state in which #268 becomes unrecoverable rather than merely deferred. */
static void focus_toplevel(struct vitrin_toplevel *t) {
	struct vitrin_shim *s = t->shim;
	wl_list_remove(&t->link);
	wl_list_insert(&s->toplevels, &t->link);
	vitrin_seat_focus_keyboard(s, t->toplevel->base->surface);
	if (!vitrin_seat_keyboard_focus_is(s, t->toplevel->base->surface)) {
		/* Not an error and not a repair site: the grab owns the keyboard
		 * until it ends, and ending it is the app's business. Logged because
		 * a silent deferral is indistinguishable from #268 itself when
		 * reading a session log after the fact. */
		wlr_log(WLR_DEBUG,
			"keyboard focus deferred to an active grab; will be re-asserted when it ends");
	}
	sync_activation(s);
}

/* MAKE THE SEAT AGREE WITH THE SHIM'S INTENT (issue #268, the grab case).
 *
 * Called when a keyboard grab ends (seat.c holds the listener; this is the
 * same shape as `vitrin_xdg_reconfigure_all`, which output.c calls when the
 * view resizes -- a mechanism-level event asking window policy to re-apply
 * itself). While a grab is live every focus change this file makes is
 * swallowed by wlroots, so the two can be out of step for as long as a menu
 * is open; this is the one moment they can be brought back together.
 *
 * WITHOUT IT #268 SURVIVES ITS OWN FIX. Measured on this tree: two toplevels,
 * a grabbing popup open on the one holding the keyboard, and that one
 * unmapped -- `toplevel_unmap` picks the survivor, the grab eats the enter,
 * and nothing ever tries again. The surviving window is left visible and
 * permanently untypable, which is the exact end state the issue was filed
 * for, reached through a path a menu makes ordinary.
 *
 * Idempotent by construction: when the seat already points at the front
 * window this returns without touching anything, so a grab that never
 * disturbed focus costs one list walk and one comparison. */
void vitrin_xdg_refocus(struct vitrin_shim *s) {
	/* The seat is brought up in phase B and this file's list in phase D, so
	 * a grab ending in between would otherwise walk an uninitialized list.
	 * The same flag guards teardown for the same reason (server.c). */
	if (!s->xdg_wired) {
		return;
	}
	struct vitrin_toplevel *front = front_toplevel(s);
	if (front == NULL) {
		/* No window on screen. The last unmap already asked for the clear
		 * and the grab swallowed that too -- `wlr_seat_keyboard_notify_clear_focus`
		 * defers exactly like the enter does -- so the seat may still be
		 * aimed at an unmapped surface. Condition 3 of `toplevel_unmap`
		 * exists to prevent precisely that, and this is where it is honoured
		 * on the deferred path. */
		vitrin_seat_clear_keyboard(s);
		return;
	}
	if (vitrin_seat_keyboard_focus_is(s, front->toplevel->base->surface)) {
		return;
	}
	wlr_log(WLR_INFO, "keyboard re-asserted on the front window after a grab: \"%s\" (%s)",
		front->toplevel->title ? front->toplevel->title : "(untitled)",
		front->toplevel->app_id ? front->toplevel->app_id : "(no app id)");
	focus_toplevel(front);
}

/* ON THE INITIAL COMMIT, AND NOT ONE INSTRUCTION EARLIER.
 * `wlr_xdg_toplevel_set_wm_capabilities` schedules a configure, and scheduling
 * one against a surface that has not had its initial commit aborts the whole
 * shim -- the same trap `toplevel_request_maximize` documents below. The
 * initial commit is the first legal moment and it is exactly where xdg-shell
 * wants the event: "compositors must send this event once before the first
 * xdg_surface.configure event", and the first configure is the one this same
 * commit schedules.
 *
 * Per map cycle, not per toplevel. An unmapped-then-remapped surface performs
 * the initial commit again (wlroots clears `initialized` in
 * `reset_xdg_surface`), so this runs again and the new configure cycle also
 * gets its capabilities first. wlroots' own seeding happens once, at toplevel
 * creation, and its `scheduled.fields` bit is cleared when the first configure
 * goes out -- so after a remap wlroots alone would send none. */
static void announce_wm_capabilities(struct vitrin_toplevel *t) {
	/* wlroots asserts the xdg_shell global's version is at least 5 before
	 * touching this field. globals.c creates it at 6, so this never fires
	 * today -- but an assert that takes the realm down with it is not a
	 * thing to leave resting on a constant in another file. */
	if (t->shim->xdg_shell->version < XDG_TOPLEVEL_WM_CAPABILITIES_SINCE_VERSION) {
		return;
	}
	wlr_xdg_toplevel_set_wm_capabilities(t->toplevel, VITRIN_WM_CAPABILITIES);
}

static void toplevel_commit(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, commit);
	if (t->toplevel->base->initial_commit) {
		announce_wm_capabilities(t);
		configure_to_view(t);
	}
}

static void toplevel_map(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, map);
	t->mapped = true;
	/* Single-maximized: the window's origin IS the view's origin. */
	wlr_scene_node_set_position(&t->tree->node, 0, 0);
	wlr_scene_node_set_enabled(&t->tree->node, true);
	/* Keyboard focus is synthesized shim-side -- there is no focus event on
	 * the wire, so the shim decides (seat.h, IDL `vitrin_shim_seat`) -- and
	 * the rule is: THE MOST RECENTLY MAPPED WINDOW THAT IS STILL MAPPED
	 * HOLDS THE KEYBOARD. Taken at map rather than at creation because an
	 * unmapped surface cannot legally receive input.
	 *
	 * THAT SENTENCE REPLACES "the app has the keyboard for as long as it has
	 * a window", which is what stood here, and which was true only while a
	 * realm had exactly one window (issue #268). It never said what happens
	 * to the FIRST window when a second one arrives and leaves again, and the
	 * code's answer was: nothing -- `toplevel_unmap` cleared focus to nobody
	 * and no later event could ever give it back, because focus is only ever
	 * taken at map and the survivor had already mapped. Found on bare metal
	 * with alacritty and nautilus: a session that looks alive, still scrolls
	 * under the pointer, and cannot be typed into. The half of the rule that
	 * lives at unmap is in `toplevel_unmap`. */
	focus_toplevel(t);
	wlr_log(WLR_INFO, "app window mapped: \"%s\" (%s)",
		t->toplevel->title ? t->toplevel->title : "(untitled)",
		t->toplevel->app_id ? t->toplevel->app_id : "(no app id)");
}

/* THE OTHER HALF OF THE FOCUS RULE (issue #268): the keyboard is passed on,
 * not thrown away.
 *
 * What stood here was one unconditional `vitrin_seat_unfocus_keyboard`, whose
 * body clears focus to NOBODY when the surface holds it. With one window per
 * realm that is correct and complete. With two it is the bug: close the
 * second window and the first -- still mapped, still visible, still receiving
 * pointer events -- never gets the keyboard back, because `toplevel_map` is
 * the only thing that takes it and the survivor mapped long ago.
 *
 * Three conditions:
 *
 *  1. ONLY IF THIS SURFACE HOLDS THE KEYBOARD -- and this one is a GUARD,
 *     not a repair. It is currently redundant, and saying so is the point:
 *     `focus_toplevel` moves the focused window to the head of the MRU list,
 *     so the front window and the keyboard holder are normally the same
 *     window, and a background window unmapping would find that same window
 *     as its "successor" and re-focus it -- which `wlr_seat_keyboard_enter`
 *     turns into a no-op on an already-focused surface. Deleting this line
 *     therefore does NOT move the keyboard, and the focus-succession test
 *     stays green with it gone (measured). What it does stop is a pointless
 *     `sync_activation` configure sweep aimed at every window on screen every
 *     time a background one closes, which IS observable and IS asserted --
 *     see FACT 4 in tests/focus_succession_client.c. The invariant that makes
 *     it redundant is one policy change away from being false (click-to-focus
 *     would separate "front" from "holder"), which is why the guard stays and
 *     why the test measures the traffic rather than the focus.
 *  2. ONLY A MAPPED SUCCESSOR. Every toplevel the client created is on this
 *     list, mapped or not, and `toplevel_destroy` -- not this handler -- is
 *     what unlinks. So THIS toplevel is still on the list while this runs;
 *     `t->mapped = false` above is what keeps `front_toplevel` from handing
 *     the keyboard straight back to the window that is going away.
 *  3. CLEAR WHEN NOTHING IS LEFT. The last window closing must still leave
 *     the seat pointing at nothing, or the fix would be aiming the keyboard
 *     at a surface about to be freed -- a worse bug than the one it fixes.
 *
 * NONE OF THE THREE IS GUARANTEED TO LAND IMMEDIATELY. Every one of them goes
 * through wlroots' seat, which defers to an active keyboard grab, and an open
 * menu is a keyboard grab (`xdg_popup.grab`). What this handler writes is the
 * shim's intent, into the MRU list; `vitrin_xdg_refocus` is what makes the
 * seat match it once the grab ends.
 *
 * THE POINTER NEEDS NOTHING HERE, AND THAT ARGUMENT STILL HOLDS WITH A
 * SIBLING (issue #268, task 4). It rests on `replay_motion` (seat.c)
 * re-hit-testing the scene on every motion event through `wlr_scene_node_at`,
 * rather than caching a focused surface the way the keyboard path does. The
 * line above disables this toplevel's scene node, so the very next motion
 * cannot land on it; with no sibling it finds nothing and clears pointer
 * focus, and with a sibling it finds the sibling and sends it an enter. Both
 * are what should happen, from the same unchanged code. wlroots additionally
 * drops pointer focus itself when the surface is destroyed, which covers an
 * app that closes a window and never moves the mouse again.
 *
 * The one case that is not instant is a drag: while a button the app has seen
 * is still down, `replay_motion` deliberately keeps the pointer on the
 * surface that received the press instead of re-hit-testing (a drag that
 * wanders off a window must not tear focus away mid-gesture), so a window
 * unmapping mid-drag keeps receiving motion until the button releases or the
 * surface is destroyed. That is bounded, self-healing, and strictly better
 * than stranding a pressed button in the app -- and it is the pre-existing
 * behaviour of the grab path, not something a second window introduces. */
static void toplevel_unmap(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, unmap);
	struct vitrin_shim *s = t->shim;
	t->mapped = false;
	wlr_scene_node_set_enabled(&t->tree->node, false);

	struct wlr_surface *surface = t->toplevel->base->surface;
	if (!vitrin_seat_keyboard_focus_is(s, surface)) {
		return; /* condition 1 */
	}

	/* Condition 2. `front_toplevel` is the successor a human expects when a
	 * dialog closes -- the window they were in before it opened -- and it is
	 * the SAME function that decides who is `activated`, so the two answers
	 * cannot disagree. */
	struct vitrin_toplevel *successor = front_toplevel(s);
	if (successor == NULL) {
		vitrin_seat_unfocus_keyboard(s, surface); /* condition 3 */
		return;
	}
	focus_toplevel(successor);
	wlr_log(WLR_INFO, "keyboard passed to the surviving window: \"%s\" (%s)",
		successor->toplevel->title ? successor->toplevel->title : "(untitled)",
		successor->toplevel->app_id ? successor->toplevel->app_id : "(no app id)");
}

static void toplevel_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, destroy);
	/* The scene tree is owned by wlr_scene_xdg_surface_create and torn down
	 * with the xdg surface; only our listeners and this record are ours. */
	wl_list_remove(&t->commit.link);
	wl_list_remove(&t->map.link);
	wl_list_remove(&t->unmap.link);
	wl_list_remove(&t->destroy.link);
	wl_list_remove(&t->request_maximize.link);
	wl_list_remove(&t->request_fullscreen.link);
	wl_list_remove(&t->link);
	free(t);
}

/* xdg-shell requires a configure in reply to a state request even when the
 * compositor changes nothing -- not answering is a protocol violation. The
 * answer is always the same here: the window already is the whole view.
 *
 * THE `initialized` GUARD IS NOT DEFENSIVE PROGRAMMING, IT IS THE PROTOCOL.
 * xdg-shell lets a client set its initial state -- `set_maximized`,
 * `set_fullscreen`, `set_title` -- on a brand-new toplevel BEFORE the first
 * commit that makes the surface configurable, and Firefox does exactly that
 * (found by the P1.6.4 bring-up: 140.12.0esr requests maximize during window
 * construction). wlroots answers a configure scheduled against a surface that
 * has not had its initial commit with `assert(surface->initialized)`, so this
 * handler running one instruction too early does not misbehave -- IT ABORTS
 * THE WHOLE SHIM, killing the realm, from an ordinary and legal client
 * request.
 *
 * Doing nothing here is not skipping the answer, it is deferring it to the
 * one place that can send it: `toplevel_commit` configures every toplevel to
 * the view on its initial commit, which is the first moment a configure is
 * legal and is guaranteed to come. So the client's request is honoured with
 * the same geometry either way, one round trip later.
 *
 * Neither weston-terminal nor any test client in this tree reaches this path,
 * which is precisely why the ladder ends at a real browser. */
static void toplevel_request_maximize(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, request_maximize);
	if (!t->toplevel->base->initialized) {
		return;
	}
	configure_to_view(t);
	wlr_xdg_surface_schedule_configure(t->toplevel->base);
}

static void toplevel_request_fullscreen(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_toplevel *t = wl_container_of(listener, t, request_fullscreen);
	if (!t->toplevel->base->initialized) {
		return;
	}
	configure_to_view(t);
	wlr_xdg_surface_schedule_configure(t->toplevel->base);
}

static void on_new_toplevel(struct wl_listener *listener, void *data) {
	struct vitrin_shim *s = wl_container_of(listener, s, new_toplevel);
	struct wlr_xdg_toplevel *toplevel = data;

	struct vitrin_toplevel *t = calloc(1, sizeof(*t));
	if (t == NULL) {
		wlr_log(WLR_ERROR, "out of memory tracking a toplevel");
		return;
	}
	t->shim = s;
	t->toplevel = toplevel;
	/* Linked before anything can fail below, so the destroy handler's
	 * `wl_list_remove` always has an inserted link to remove. */
	wl_list_insert(&s->toplevels, &t->link);

	/* Covers the toplevel and its subsurfaces, and applies the client's
	 * window geometry as a position offset -- so a client with CSD shadow
	 * insets is placed by its geometry rectangle rather than by its buffer,
	 * which is what makes view coordinates land on the right pixels for
	 * every GTK/Qt app.
	 *
	 * This call covers exactly ONE xdg surface -- this one, plus its
	 * subsurfaces. A popup the client creates later is a separate xdg
	 * surface and needs its own node, hung under this one; `on_new_popup`
	 * makes it and finds this tree through `base->data`, set below. */
	t->tree = wlr_scene_xdg_surface_create(&s->scene->tree, toplevel->base);
	if (t->tree == NULL) {
		wlr_log(WLR_ERROR, "cannot add the toplevel to the scene");
		/* Unlinked before the free, or the list keeps a node pointing into
		 * freed memory and every later walk of it dereferences that. The
		 * comment above promises the link is always removable; this is the
		 * one path that used to break the promise. Latent until issue #268
		 * gave the list a second reader -- `toplevel_unmap`'s successor
		 * search now runs whenever the keyboard-holding window closes, so a
		 * failure here would turn the next window close into a
		 * use-after-free rather than into nothing at all. */
		wl_list_remove(&t->link);
		free(t);
		return;
	}
	wlr_scene_node_set_enabled(&t->tree->node, false); /* enabled on map */
	/* The scene tree, published on the xdg surface itself. tinywl's
	 * convention, kept for one reason: a popup is handed its parent as a
	 * bare `wlr_surface`, and the only route from there back to the node it
	 * must hang under is a pointer the compositor left behind. Nested popups
	 * (a submenu off a menu) resolve through the same field on the popup's
	 * own xdg surface, which `on_new_popup` sets. */
	toplevel->base->data = t->tree;

	t->commit.notify = toplevel_commit;
	wl_signal_add(&toplevel->base->surface->events.commit, &t->commit);
	t->map.notify = toplevel_map;
	wl_signal_add(&toplevel->base->surface->events.map, &t->map);
	t->unmap.notify = toplevel_unmap;
	wl_signal_add(&toplevel->base->surface->events.unmap, &t->unmap);
	t->destroy.notify = toplevel_destroy;
	wl_signal_add(&toplevel->events.destroy, &t->destroy);
	t->request_maximize.notify = toplevel_request_maximize;
	wl_signal_add(&toplevel->events.request_maximize, &t->request_maximize);
	t->request_fullscreen.notify = toplevel_request_fullscreen;
	wl_signal_add(&toplevel->events.request_fullscreen, &t->request_fullscreen);
}

/* THE CONFIGURE A POPUP IS OWED, AND WHAT HAPPENS WITHOUT IT.
 *
 * `xdg_surface`'s initial-commit/configure/ack contract binds the ROLE object,
 * not just toplevels: a popup that attaches a buffer having never been
 * configured is killed with `xdg_surface` error 3 (`unconfigured_buffer`).
 * wlroots does not send that configure on the compositor's behalf -- for
 * popups it only rejects a parentless one (`handle_xdg_popup_client_commit`)
 * -- so a compositor with no `new_popup` listener does not merely fail to
 * render menus, it DISCONNECTS every app that opens one. Measured against
 * this shim before this listener existed: `globals-error: seq=13 code=3
 * message="xdg_surface has never been configured"`, and the client gone.
 * `tests/xdg_conformance_client.c` FACT 3 is that measurement, kept.
 *
 * An empty configure is the right answer here for the same reason the
 * toplevel's is a fixed one: version 1 has no window manager and the popup's
 * own positioner already resolved its geometry against the parent. A
 * compositor with a screen to keep popups on would instead reposition here. */
static void popup_commit(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_popup *p = wl_container_of(listener, p, commit);
	if (p->popup->base->initial_commit) {
		wlr_xdg_surface_schedule_configure(p->popup->base);
	}
}

static void popup_destroy(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_popup *p = wl_container_of(listener, p, destroy);
	/* Same ownership split as the toplevel: the scene tree belongs to
	 * wlr_scene_xdg_surface_create and dies with the xdg surface. */
	wl_list_remove(&p->commit.link);
	wl_list_remove(&p->destroy.link);
	free(p);
}

static void on_new_popup(struct wl_listener *listener, void *data) {
	/* No `wl_container_of` back to the shim, unlike `on_new_toplevel`: a
	 * popup's geometry comes from its own positioner and its parent, never
	 * from the realm view, and it is placed under the parent's scene node
	 * rather than under the root. Nothing here needs shim state. */
	(void)listener;
	struct wlr_xdg_popup *popup = data;

	struct vitrin_popup *p = calloc(1, sizeof(*p));
	if (p == NULL) {
		wlr_log(WLR_ERROR, "out of memory tracking a popup");
		return;
	}
	p->popup = popup;

	/* Under the parent's node, never under the scene root: a popup is
	 * positioned relative to its parent, and `wlr_scene_xdg_surface_create`
	 * implements exactly that for a popup xdg surface (it tracks
	 * `popup->current.geometry` on every commit). Hanging it off the root
	 * would place a menu at the view origin instead of at its anchor. */
	struct wlr_xdg_surface *parent = popup->parent != NULL
		? wlr_xdg_surface_try_from_wlr_surface(popup->parent) : NULL;
	struct wlr_scene_tree *parent_tree = parent != NULL ? parent->data : NULL;
	if (parent_tree != NULL) {
		popup->base->data = wlr_scene_xdg_surface_create(parent_tree, popup->base);
		if (popup->base->data == NULL) {
			wlr_log(WLR_ERROR, "cannot add the popup to the scene");
		}
	} else {
		/* Only reachable if the parent is a surface this shim never put in
		 * the scene -- there is no such role in the v0 global set (no
		 * layer-shell), so this is a "cannot happen" that MUST NOT be an
		 * assert: tinywl asserts here, and an assert in a shim takes the
		 * realm down over a client's malformed request. The popup is
		 * configured anyway, one line below, so the app keeps running with
		 * an invisible menu rather than being disconnected. */
		wlr_log(WLR_ERROR, "popup with no parent scene node; not rendered");
	}

	/* Wired even when the scene node is missing: the configure is what keeps
	 * the client alive, and it is owed regardless of what gets rendered. */
	p->commit.notify = popup_commit;
	wl_signal_add(&popup->base->surface->events.commit, &p->commit);
	p->destroy.notify = popup_destroy;
	wl_signal_add(&popup->events.destroy, &p->destroy);
}

/* Re-configure every live toplevel to the current view size -- the
 * single-maximized rule applied again after the core moved the view.
 *
 * `configure_to_view` is the *same* function the initial commit and the
 * client's own maximize/fullscreen requests go through, deliberately: this
 * shim has one layout rule and this must not become a second statement of it.
 *
 * `initialized` is the protocol guard, not defensive programming: a configure
 * before the surface's first commit is illegal (see `toplevel_request_maximize`
 * for the full argument), and a toplevel created mid-resize is in exactly that
 * state. It gets the new size anyway, on its initial commit, from
 * `toplevel_commit` -- which reads `cfg` and has already been updated. */
void vitrin_xdg_reconfigure_all(struct vitrin_shim *s) {
	struct vitrin_toplevel *t;
	wl_list_for_each(t, &s->toplevels, link) {
		if (!t->toplevel->base->initialized) {
			continue;
		}
		configure_to_view(t);
		wlr_xdg_surface_schedule_configure(t->toplevel->base);
	}
}

bool vitrin_setup_xdg(struct vitrin_shim *s) {
	wl_list_init(&s->toplevels);
	s->new_toplevel.notify = on_new_toplevel;
	wl_signal_add(&s->xdg_shell->events.new_toplevel, &s->new_toplevel);
	s->new_popup.notify = on_new_popup;
	wl_signal_add(&s->xdg_shell->events.new_popup, &s->new_popup);
	s->xdg_wired = true;
	return true;
}
