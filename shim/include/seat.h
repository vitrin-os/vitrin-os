/* seat.h -- virtual-seat input replay (P1.6.3): origin-tagged
 * `vitrin_shim_seat` events from the core, replayed into the app through the
 * shim's own `wl_seat`.
 *
 * This is the actuation half of artifact A5. `upstream.c` receives the five
 * seat events off the wire; everything that happens to them afterwards lives
 * here. The core is a router, this file is the replayer, and the app is an
 * ordinary Wayland client that cannot tell it is being driven.
 *
 * WHAT ARRIVES, AND WHAT IT BECOMES:
 *
 *   motion(x, y, origin)        -> wl_pointer enter/motion, surface-local
 *   button(code, state, origin) -> wl_pointer button (evdev codes, verbatim)
 *   scroll(axis, v120, origin)  -> wl_pointer axis (high-resolution v120)
 *   key(keysym, state, origin)  -> wl_keyboard key against a dynamic keymap
 *   text(string, origin)        -> a run of key press/release pairs, ditto
 *
 * ====================================================================
 * B2: HOW THE ORIGIN TAG SURVIVES DELIVERY, STRUCTURALLY, IN C
 * ====================================================================
 *
 * Backward requirement B2 says every input event carries its origin --
 * physical human device vs. agent actuator -- from core intake through the
 * wire to app delivery, and that the tag is "preserved structurally through
 * delivery, not dropped at the shim boundary". Phase 3's
 * physically-originated consent is unbuildable retroactively if replay
 * flattens the distinction.
 *
 * The core makes that unforgeable through the Rust type system
 * (crates/vitrin-core/src/input/mod.rs: private `origin` field, no setter,
 * no `Default`, two constructors one of which is module-private, so an
 * untagged event is unrepresentable and a physical-origin masquerade is a
 * compile error). C has weaker tools. These are the four it does have, and
 * this file uses all four:
 *
 *   1. ONE ENTRY POINT, AND IT DECODES. `vitrin_seat_handle_event` is the
 *      only non-static replay function this translation unit exports, and
 *      its input is a raw wire frame. Every internal replay helper is
 *      `static`. So there is no linkable symbol anywhere in the shim that
 *      replays input without first decoding a wire event -- an untagged
 *      replay path cannot be *called*, because it cannot be *named*. This
 *      is the C analogue of "SeatDelivery is constructed only by
 *      InputRouter::route".
 *
 *   2. THE ALL-ZEROES VALUE IS NOT `physical`. On the wire `physical` is 0,
 *      so a C struct left at `{0}` -- what `= {0}`, `calloc`, and a
 *      forgotten initializer all produce -- would decode as the MOST
 *      privileged tag. `struct vitrin_origin` therefore stores the tag
 *      BIASED BY ONE, so the zero value is `unset` and is not any valid
 *      origin at all. "Forgot to tag" is then a loud abort in the accessor
 *      rather than a silent physical-origin masquerade. The bias is bound
 *      to the wire values by _Static_assert in seat.c, so the two cannot
 *      drift.
 *
 *   3. THE TAG IS `const` INSIDE THE STRUCT. C does enforce this one:
 *      assigning to a const member is a constraint violation, and the
 *      struct as a whole becomes non-assignable. Once constructed, a tag
 *      cannot be overwritten, and one delivery's tag cannot be copied over
 *      another's. There is no setter, and the single constructor
 *      (`vitrin_origin_from_wire`) takes the decoder's validated enum --
 *      which the generated header has already checked against the enum's
 *      whole-value membership rule -- so a tag that never came off a wire
 *      cannot be minted.
 *
 *   4. IT IS A MANDATORY PARAMETER, NOT A FIELD SOMEONE MAY FORGET.
 *      Every replay helper takes `struct vitrin_origin` by value. Dropping
 *      the tag means deleting a parameter, which breaks every call site --
 *      the C equivalent of the core's "adding a seat event kind without
 *      encoding its origin is a compile error".
 *
 * WHERE THE TAG BECOMES OBSERVABLE. There is no shim -> core message on
 * `vitrin_shim_seat` (0 requests, 5 events) and none anywhere else for
 * input acknowledgement, so the core's flight recorder
 * (crates/vitrin-core/src/recorder.rs) structurally cannot record what the
 * shim *delivered* -- only what it *sent*. The shim therefore emits its own
 * structured, machine-readable delivery trace, one line per event, carrying
 * the tag verbatim:
 *
 *   seat-replay: seq=N event=KIND origin=physical|emulated ... delivered=0|1
 *
 * The acceptance harness correlates that trace one-to-one against the
 * core's record of what it emitted; the two agreeing on origin, order and
 * count is the available proof that the tag survived the hop. See
 * tests/acceptance/seat_input_replay.sh and the README for the limits of
 * that claim.
 *
 * ====================================================================
 * D7: THE DYNAMIC KEYMAP (the wtype technique) -- R5's hot spot
 * ====================================================================
 *
 * `actuate.text` means "deliver this Unicode string", never "press these
 * keys", and Firefox has no reliable text-input-v3, so text is delivered by
 * synthesizing a keymap that binds each needed codepoint to its own
 * keycode and then pressing those keycodes -- the wtype technique, settled
 * as decision D7. `key` uses the same machinery: a keysym is a keysym.
 *
 * THE MODIFIER-SUPPRESSION RULE, MADE STRUCTURAL. Keysyms arrive already
 * modifier-resolved, so the shim MUST bind each non-modifier keysym at an
 * unmodified level, or a modifier the app is already holding would be
 * applied a second time (the classic VNC double-shift bug, which the prose
 * page rules out normatively). Every generated key carries one explicit
 * type, `VITRIN_KEY_TYPE`, and that type suppresses the TWO separate
 * xkbcommon mechanisms by which a modifier can change a delivered
 * character -- neither of which implies the other, and only the first of
 * which is obvious:
 *
 *   LEVEL SELECTION. The type maps only Level1, so no modifier combination
 *   can select any other level, for any key, ever. This is the Shift case.
 *   Writing it into the keymap is stronger than emitting one symbol per key
 *   and trusting xkbcommon's type inference: the suppression is stated,
 *   not inferred.
 *
 *   THE CAPS-LOCK KEYSYM TRANSFORMATION. libxkbcommon capitalises the
 *   resolved keysym, AFTER level selection, whenever Lock is effective and
 *   the key's type does not CONSUME Lock -- a mechanism a one-level type
 *   does not touch. So the type consumes Lock (and only Lock: consuming
 *   Control would break Ctrl+C, and consuming anything would hide it from
 *   toolkit accelerator masks). Without this, a human toggling Caps Lock in
 *   the host window would silently upper-case every subsequent agent `text`
 *   payload, which is the same double-application the rule forbids, wearing
 *   a different mechanism.
 *
 * Modifier keysyms still carry real `SetMods`/`LockMods` actions and
 * `modifier_map` entries, so a chord (Ctrl+C) reaches the app as a chord and
 * a locked Caps is visible to the app as locked state -- they convey state
 * and nothing else, exactly as the rule requires.
 *
 * WHAT `text` REFUSES TO SEND. `actuate.text` is "deliver this Unicode
 * string", never "press these keys", and the IDL names exactly two control
 * characters that become keystrokes (`\n` -> Return, `\t` -> Tab). Every
 * other control codepoint is dropped rather than handed to xkbcommon, which
 * maps them to real and destructive keys -- U+0008 to BackSpace, U+001B to
 * Escape, U+007F to Delete. A stray byte in a scraped or pasted payload must
 * not erase the user's text or dismiss their dialog. `key` remains the way
 * to press those keys deliberately.
 *
 * THE KEYMAP IS SELF-CONTAINED. It includes no system xkb data ("complete"
 * types/compat), so the shim compiles it with
 * XKB_CONTEXT_NO_DEFAULT_INCLUDES and never reads /usr/share/X11/xkb. One
 * less thing a confined process touches, and one less way two machines'
 * xkeyboard-config versions can make the same string arrive differently.
 *
 * CACHING AND REGENERATION (the two open questions the issue names).
 * A keycode's meaning must be as stable as possible, because the known
 * failure mode is an app that reads the keymap once and never re-reads it.
 * The keycode space is therefore split into three regions:
 *
 *   modifier region  fixed for the life of the process
 *   warm region      fixed too: all of printable ASCII plus every
 *                    layout-invariant editing/navigation/function keysym
 *                    the core can produce, bound ONCE at startup, before
 *                    the app has even connected
 *   dynamic region   bind-on-demand, FIFO recycling, never evicting a
 *                    keysym that is currently held down
 *
 * Three consequences, all deliberate:
 *
 *   - The app's FIRST keymap read already covers the entire human key path
 *     and all-ASCII agent text. An app that never re-reads is fully correct
 *     for both.
 *   - Binding is ADDITIVE: a keycode already in the app's copy of the
 *     keymap never changes meaning until the dynamic ring wraps. So a stale
 *     reader degrades by missing characters it never had, not by typing
 *     the WRONG ones -- which is the difference between a visible gap and
 *     silent corruption.
 *   - The keymap is regenerated only when a codepoint is genuinely new.
 *     Steady-state ASCII typing regenerates it exactly zero times; the R5
 *     gate string costs one regeneration for its four non-ASCII codepoints
 *     and zero on every repeat.
 *
 * A single `text` event whose distinct new codepoints exceed the dynamic
 * region is delivered in chunks, one keymap per chunk, so a keycode is
 * never recycled while the string that used it is still being typed. The
 * clean fix for all of this is text-input-v3 (E2.8); this is the version-1
 * technique that works against apps that do not have it.
 *
 * ====================================================================
 * D10: COORDINATES, AND WHY THE SHIM DOES NOT COMPENSATE TWICE
 * ====================================================================
 *
 * The core sends realm-view pixels and the shim maps view -> surface-local.
 * The nuance already settled core-side (crates/vitrin-core/src/input/mod.rs,
 * "Coordinate mapping"): the core ALREADY compensates for its own
 * letterboxing at routing time, so (0, 0) on the wire is the top-left of
 * the app content this shim forwarded -- the origin of the shim's own view
 * space -- and matte clicks never arrive at all. The shim must therefore
 * NOT subtract a placement offset of its own; doing so would double-count.
 *
 * What the shim does instead is the mapping the core cannot do: view space
 * -> the particular surface under that point, in that surface's own
 * coordinates. That is a hit test, not an offset, and it is emphatically
 * not the identity: a client with CSD shadow insets (every GTK and Qt app)
 * sets a window geometry, which places its surface at a negative offset
 * inside its own tree, so view (0, 0) is some interior pixel of its buffer.
 * A shim that assumed identity would put every click a shadow's width away
 * from where the agent aimed.
 *
 * The hit test is answered by `wlr_scene_node_at` against the very scene
 * graph the compositor painted, so the replayer and the renderer can never
 * disagree about where a surface is -- the same discipline the core's
 * router follows by sharing `layout::place` with its scene. It is also what
 * makes subsurfaces and popups addressable for free: whatever ends up in
 * the scene is what input hit-tests against. (Popups are not in the scene
 * yet -- see the note in xdg.c -- but nothing here will need to change when
 * they are.)
 *
 * Focus in version 1 is synthesized here: pointer enter on the first motion
 * that lands on a surface, keyboard focus held on the app from the moment
 * its toplevel maps. There is no focus event on the wire; an explicit
 * tagged one is a later version's addition.
 */
#ifndef VITRIN_SEAT_H
#define VITRIN_SEAT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "vitrin-protocol.h"

#include <wayland-server-core.h> /* struct wl_listener */

struct vitrin_shim;
struct wlr_keyboard;
struct wlr_surface;
struct xkb_context;

/* ---- B2: the origin tag ---------------------------------------------- */

/* Who caused an input event, carried from the wire to the point of
 * delivery. See the B2 section above for the whole argument; the short
 * version is that this type exists so that "delivered untagged" and
 * "delivered as physical when it was emulated" are both unrepresentable
 * rather than merely avoided.
 *
 * `biased` is the wire enum value PLUS ONE, so that zero -- the value every
 * uninitialised C aggregate has -- means `unset` and not `physical`. It is
 * `const` so nothing can retag a constructed value: C enforces that much,
 * and it also makes the struct non-assignable, which is why it travels as a
 * by-value parameter and is never stored. */
struct vitrin_origin {
	const uint8_t biased;
};

/* The zero value: not a valid origin. Never produced by the constructor. */
#define VITRIN_ORIGIN_UNSET 0u

/* THE constructor. Takes the value the generated decoder has already
 * validated against the enum's membership rule, so a tag that did not come
 * off a wire cannot be minted through it. There is deliberately no
 * `vitrin_origin_physical()` convenience: the core made physical-origin
 * minting module-private for exactly this reason, and the shim -- which is
 * the untrusted side -- has even less business inventing one. */
static inline struct vitrin_origin vitrin_origin_from_wire(vitrin_shim_seat_origin_t o) {
	return (struct vitrin_origin){.biased = (uint8_t)((unsigned)o + 1u)};
}

/* Whether this tag was constructed rather than zero-initialised. */
static inline bool vitrin_origin_is_tagged(struct vitrin_origin o) {
	return o.biased != VITRIN_ORIGIN_UNSET;
}

/* The wire value. Callers must have checked `vitrin_origin_is_tagged`
 * first; the delivery path asserts it once, at the single entry point. */
static inline vitrin_shim_seat_origin_t vitrin_origin_wire(struct vitrin_origin o) {
	return (vitrin_shim_seat_origin_t)(o.biased - 1u);
}

/* The tag as it appears in the delivery trace. "unset" can only be printed
 * by a bug, and printing it rather than guessing is the point. */
const char *vitrin_origin_name(struct vitrin_origin o);

/* ---- the keymap ------------------------------------------------------ */

/* The first xkb keycode the shim uses. 9, not 8, so the evdev code that
 * reaches the app on the wire (xkb keycode - 8) starts at 1 and never
 * collides with KEY_RESERVED. */
#define VITRIN_KEYCODE_BASE 9u

/* Keycode slots, i.e. xkb keycodes VITRIN_KEYCODE_BASE .. 255. The ceiling
 * is 255 because that is where the X11 keycode range ends: xkbcommon itself
 * would take more, but an Xwayland-hosted app (Phase 3) would not, and a
 * keymap that only works for native clients is a trap laid for later. 247
 * slots is already far more than the technique needs -- see the region
 * split below. */
#define VITRIN_KEY_SLOTS (256u - VITRIN_KEYCODE_BASE)

/* Largest `text` payload, from the IDL (`text` is "max 4096 bytes"). Every
 * codepoint is at least one byte, so this also bounds the codepoint count. */
#define VITRIN_TEXT_MAX_BYTES 4096u

/* Delivered presses whose release is still owed, per device. Wayland's own
 * `wlr_keyboard` tracks at most 32 simultaneously pressed keys, and a
 * pointer has five buttons a human can hold; both caps are generous. */
#define VITRIN_MAX_PRESSED_BUTTONS 8

/* One keycode slot's binding. `held` is what keeps the FIFO recycler from
 * pulling a keycode out from under a key the app still believes is down. */
struct vitrin_key_slot {
	uint32_t keysym; /* 0 (XKB_KEY_NoSymbol) = unbound */
	bool held;
	/* Insertion order in the dynamic ring; unused for the fixed regions. */
	uint64_t bound_seq;
};

struct vitrin_seat_replay {
	/* Back-pointer, so the wlroots listeners below can reach the shim. */
	struct vitrin_shim *shim;

	/* The virtual keyboard device. The seat needs a `wlr_keyboard` to have a
	 * keymap at all -- capabilities alone advertise child objects, they do
	 * not make a keymap exist -- and this one is backed by no hardware: its
	 * key events are the ones this file synthesizes. */
	struct wlr_keyboard *keyboard;
	/* wlroots does not relay modifier state for us: `wlr_seat_set_keyboard`
	 * wires up keymap and repeat-info forwarding but not modifiers, so the
	 * compositor must. This listener is that relay. */
	struct wl_listener modifiers;
	bool modifiers_wired;

	/* The xkb context, kept for the life of the shim so keymap
	 * regeneration is a string compile and nothing more. Created with
	 * XKB_CONTEXT_NO_DEFAULT_INCLUDES: the generated keymap is
	 * self-contained (see D7 above), so the shim never reads xkb data
	 * files. */
	struct xkb_context *xkb;

	/* The keycode table. Regions, in slot order:
	 *   [0, mod_slots)          modifier keysyms, fixed
	 *   [mod_slots, warm_slots) ASCII + layout-invariant keys, fixed
	 *   [warm_slots, VITRIN_KEY_SLOTS) the dynamic ring
	 * The two fixed regions are filled once, at init, and never change. */
	struct vitrin_key_slot slots[VITRIN_KEY_SLOTS];
	unsigned mod_slots;
	unsigned warm_slots;
	unsigned ring_next;   /* next dynamic slot the FIFO will recycle */
	uint64_t bind_seq;    /* monotonic, orders dynamic bindings */
	bool keymap_dirty;    /* a binding changed since the last upload */
	uint64_t keymap_generations; /* uploads so far (1 = the warm one) */

	/* Decoded `text` payload, kept out of the stack: one keysym per
	 * codepoint of the largest legal string, and the keycode slot each was
	 * bound to. The slots are recorded at bind time rather than searched for
	 * again at typing time, because a keysym can legitimately occupy two
	 * keycodes at once -- one the app is holding, one minted for this
	 * delivery -- and a search would have to disambiguate them from state
	 * the typing loop is itself changing. */
	uint32_t text_keysyms[VITRIN_TEXT_MAX_BYTES];
	uint16_t text_slots[VITRIN_TEXT_MAX_BYTES];

	/* ---- pointer state ----
	 * Pointer and keyboard FOCUS are not tracked here on purpose: wlroots
	 * already holds them (`seat->pointer_state.focused_surface`,
	 * `seat->keyboard_state.focused_surface`) and, more importantly,
	 * already clears them when the surface is destroyed. A second copy
	 * would be a second thing to invalidate, and the one that got it wrong
	 * would be pointing at freed memory. */

	/* Layout-space origin of the surface holding pointer focus, captured
	 * when focus was taken. During an implicit grab this offset is used
	 * instead of a fresh hit test, which is what keeps a drag that wanders
	 * off the surface from tearing the focus away mid-drag. */
	double surface_ox, surface_oy;
	/* Delivered-and-unreleased button codes, in press order. A release is
	 * replayed iff its own press was -- per button code, Wayland-style. */
	uint32_t pressed[VITRIN_MAX_PRESSED_BUTTONS];
	unsigned npressed;
	/* A pointer event has been sent since the last `wl_pointer.frame`. */
	bool frame_pending;

	/* ---- instrumentation, and the delivery trace's sequence number ---- */
	uint64_t seq;
	uint64_t delivered[5]; /* indexed by opcode: motion/button/scroll/key/text */
	uint64_t dropped;
	uint64_t keys_synthesized;
	uint64_t codepoints_delivered;
	uint64_t codepoints_unmappable;
};

/* Create the virtual keyboard, compile the warm keymap, and attach both to
 * the shim's `wl_seat`. Runs in phase B (globals.c), immediately after
 * `wlr_seat_create` and BEFORE the private Wayland socket is bound -- the
 * app's first `wl_keyboard` bind must already find a keymap waiting, or its
 * first read is of nothing at all. Returns false only on an allocation or
 * keymap-compilation failure, both of which are fatal to bring-up. */
bool vitrin_seat_init(struct vitrin_shim *s);

/* Take keyboard focus for `surface`, and hold it. Called when the app's
 * toplevel maps (xdg.c). Version 1's realm is single-surface, so this is
 * the whole of focus policy: the app has the keyboard whenever it has a
 * window. */
void vitrin_seat_focus_keyboard(struct vitrin_shim *s, struct wlr_surface *surface);

/* Release keyboard focus if `surface` holds it (the toplevel unmapped or
 * was destroyed). */
void vitrin_seat_unfocus_keyboard(struct vitrin_shim *s, struct wlr_surface *surface);

/* THE replay entry point, and the only exported one: decode one
 * `vitrin_shim_seat` event and replay it. `frame` is a complete wire frame
 * already routed to the seat object by `upstream.c`.
 *
 * There is deliberately no `vitrin_seat_replay_motion(x, y, origin)` in
 * this header, nor any other per-event export. Replay is reachable only by
 * decoding a wire event, so no caller anywhere in the shim can inject input
 * that never carried an origin tag (B2, argument 1 above). */
void vitrin_seat_handle_event(struct vitrin_shim *s, const uint8_t *frame, size_t len);

/* End the current pointer event batch: emit `wl_pointer.frame` if any
 * pointer event was replayed since the last one.
 *
 * POINTER BATCHING, the second open question the issue names. A
 * `wl_pointer.frame` marks the end of a group of pointer events that belong
 * together, and the group the protocol cares about is the one the core
 * routed from a single physical event -- a diagonal scroll is two `scroll`
 * events for one wheel motion (the core's intake produces exactly that),
 * and an agent's move-then-click arrives as a run. Sending a frame after
 * every event would tell the app those were unrelated; batching per wire
 * dispatch reproduces the grouping the core actually had. So the frame is
 * emitted once per drained batch of core traffic, which is what this
 * function is called at the end of. */
void vitrin_seat_frame_boundary(struct vitrin_shim *s);

/* Detach listeners and free the keymap machinery. Idempotent, and safe
 * after a partial bring-up. Must run BEFORE `wl_display_destroy`, which
 * tears down the seat this keyboard is attached to. */
void vitrin_seat_finish(struct vitrin_shim *s);

#endif /* VITRIN_SEAT_H */
