/* seat.c -- virtual-seat input replay. See seat.h for the whole design:
 * B2's structural origin tag, D7's dynamic keymap, D10's coordinate rules,
 * and the two open questions this task settles (keymap caching, pointer
 * batching).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * The order of the file follows the order an event meets it:
 *
 *   tag       the wire's origin becomes a `struct vitrin_origin`
 *   bind      a keysym becomes a keycode in the dynamic keymap
 *   upload    a changed keymap reaches the app
 *   replay    the event reaches the app through the shim's wl_seat
 *   trace     what was delivered, with its tag, is written down
 */
#define _POSIX_C_SOURCE 200809L

#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <wayland-server-core.h>
#include <wayland-server-protocol.h>

#include <xkbcommon/xkbcommon.h>

#include <wlr/interfaces/wlr_keyboard.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_output_layout.h>
#include <wlr/types/wlr_pointer_gestures_v1.h>
#include <wlr/types/wlr_relative_pointer_v1.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/util/box.h>
#include <wlr/util/log.h>

#include "seat.h"
#include "server.h"
#include "upstream.h"
#include "vitrin-protocol.h"

/* B2, argument 2 (seat.h): the bias is what makes the all-zeroes tag mean
 * `unset` rather than `physical`. These two assertions are what stop the
 * bias and the wire from drifting apart -- if the IDL ever renumbered
 * `origin`, this file would refuse to compile rather than mislabel a
 * delivery. */
_Static_assert(VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL == 0,
	"origin.physical must be 0, which is why the tag is stored biased");
_Static_assert(VITRIN_ORIGIN_UNSET == 0,
	"the unset tag must be the all-zeroes value");

/* `delivered[]` is indexed by opcode and sized from gesture_end's, so every
 * opcode this file counts must fit. Spelled out per opcode rather than as one
 * `max()`: an opcode that outgrows the array fails at the line that names it,
 * instead of writing past the end of the struct at runtime. An opcode appended
 * AFTER gesture_end is caught by the message-count pin below.
 *
 * This is the check that was missing when the array was a literal `5` -- the
 * version-2 append would have made the first replayed relative_motion
 * corrupt whatever follows `delivered` in `struct vitrin_seat_replay`. */
_Static_assert(VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_KEY_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE < VITRIN_SEAT_EVENT_SLOTS,
	"opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE < VITRIN_SEAT_EVENT_SLOTS,
	"opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE < VITRIN_SEAT_EVENT_SLOTS,
	"opcode fits");
_Static_assert(VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE < VITRIN_SEAT_EVENT_SLOTS, "opcode fits");

/* THE ASSERTS ABOVE COVER ONLY OPCODES THAT ALREADY EXIST, and the list is as
 * hand-maintained as the literal `5` it replaced. This is what catches an event
 * appended AFTER gesture_end.
 *
 * The comments here used to claim the per-opcode asserts did that. THEY DID
 * NOT, and it was proved rather than reasoned about: a probe event appended
 * after gesture_end took opcode 10, `delivered[10]` was written in a 10-element
 * array, and `src_seat.c.o` compiled clean under -Wall -Wextra -Werror. That is
 * a real out-of-bounds write, and a comment telling the next maintainer a guard
 * exists is worse than no comment when the guard does not.
 *
 * The pin works because the generated header emits no per-interface event
 * count, so "is gesture_end still the last vitrin_shim_seat event" is not
 * expressible here -- but ANY new message raises VITRIN_MESSAGE_COUNT, so this
 * fails first and sends the maintainer to the paragraph above.
 *
 * WHEN THIS FAILS: bump the number. Then, iff the new message is a
 * vitrin_shim_seat EVENT, re-derive VITRIN_SEAT_EVENT_SLOTS (seat.h) from the
 * new last opcode and add its `_Static_assert` above -- BEFORE any replay
 * indexes delivered[] with it. The core's own `session.rs` pins the same
 * constant for the same reason, so a message added to the IDL turns two
 * unrelated files red on purpose.
 *
 * Re-pinned 47 -> 48 by D-042 (issue #306), with the check the paragraph
 * above asks for actually made: the added message is
 * `vitrin_shim_session.idle_inhibit`, a REQUEST on the session bootstrap object.
 * It is not a `vitrin_shim_seat` event, so `VITRIN_SEAT_EVENT_SLOTS` is
 * unchanged, `gesture_end` is still that interface's last event, and
 * `delivered[]` is untouched.
 *
 * Re-pinned 48 -> 54 by P2.6.5 (issue #189), same check made rather than
 * waved through, and this is the largest single append the pin has taken.
 * The six added messages are `vitrin_grant.get_powerbox`,
 * `vitrin_shim_session.designation`, and the four messages of the new
 * `vitrin_powerbox` interface. NONE is a `vitrin_shim_seat` event:
 * `gesture_end` is still that interface's last event, so
 * `VITRIN_SEAT_EVENT_SLOTS` is unchanged, no new `_Static_assert` belongs
 * above, and `delivered[]` is untouched.
 *
 * One of the six IS a core -> shim event carrying an fd -- `designation`, the
 * first in this protocol. It does not reach this file (it is addressed to
 * VITRIN_SESSION_ID, not to the seat), and this transport does not implement
 * receiving an fd at all; see `wire.h` for what P2.6.7 owes there. */
_Static_assert(VITRIN_MESSAGE_COUNT == 54,
	"a message was appended to the IDL -- read the paragraph above before touching "
	"delivered[]");

/* The keysym the IDL mandates for a newline in `text`: "a newline (\\n)
 * MUST be rendered as Return and a tab (\\t) as Tab". This is NOT what
 * xkbcommon would pick -- `xkb_utf32_to_keysym('\n')` yields Linefeed
 * (0xff0a), which no toolkit treats as Enter -- so the mapping is written
 * here rather than delegated. `\t` needs no override: xkbcommon already
 * yields Tab. */
#define VITRIN_KEYSYM_RETURN 0xff0du
#define VITRIN_KEYSYM_TAB 0xff09u

/* Continuous scroll amount per notch, mirroring the core's
 * V120_PER_SCROLL_PIXEL (= 120/15) in
 * crates/vitrin-core/src/input/mod.rs. The core converts a touchpad's
 * pixels INTO value120 at 15 px per notch; converting back at the same rate
 * is what makes a pixel-scroll that crossed the wire arrive at the app the
 * size it started. Any other constant here would silently rescale
 * touchpad scrolling on its way through the realm. */
#define VITRIN_SCROLL_PIXELS_PER_NOTCH 15.0

/* Client-side key repeat, disabled. Every key the app sees here was
 * synthesized from a discrete wire event: the core forwards each host key
 * event individually (the host's own auto-repeat included), and agent text
 * is a run of explicit press/release pairs. A client repeat timer running
 * on top of that would invent keystrokes nobody sent -- which is precisely
 * the "no duplicates" the acceptance criteria demand.
 *
 * This stays 0 on a bare-metal core too, and that is a decision rather than
 * an omission (vitrin decision D-028(5), issue #217). Off a host there IS no
 * host auto-repeat: libinput synthesizes none, so a held key would not repeat
 * at all. Re-enabling `wlr_keyboard_set_repeat_info` here is the
 * architecturally correct Wayland answer and is refused because repeat is
 * SEAT-WIDE: this seat carries an agent's actuations beside the human's, the
 * repeat machinery cannot see the per-event `origin` tag, and repeating an
 * agent's held key is exactly the invented keystroke the paragraph above
 * forbids. The core is the only side that still has the tag, so the core is
 * where a physical-origin-only repeat would have to live.
 *
 * IT DOES NOT LIVE THERE YET. This comment used to read "the core repeats
 * instead", in the present tense, and no such code has ever existed --
 * corrected 2026-08-12 by the WS-E.4.4 honesty sweep (issue #224). The
 * consequence is user-visible on `--drm` and is now published as its own
 * limit on docs/book/src/limits.md and README.md: a held key produces exactly
 * one character. Nested is unaffected, because the host compositor repeats
 * and the core forwards each repeated event individually. Do not restore the
 * present tense here without the core-side implementation and a hardware run
 * behind it. */
#define VITRIN_REPEAT_RATE_HZ 0
#define VITRIN_REPEAT_DELAY_MS 0

/* ---- the generated keymap's one key type ------------------------------ */

/* Every generated key carries this type. Two clauses live in it, and they
 * suppress two DIFFERENT xkbcommon mechanisms -- the normative
 * modifier-suppression rule needs both, and neither implies the other:
 *
 *   LEVEL SELECTION. Only `Level1` is ever mapped, so no modifier
 *   combination can select any other level. That is what stops a Shift the
 *   app is holding from shifting an already-resolved keysym -- the classic
 *   VNC double-shift bug the prose page rules out normatively.
 *
 *   THE CAPS-LOCK KEYSYM TRANSFORMATION. Level selection is not the only
 *   way a modifier changes the delivered character: libxkbcommon applies a
 *   separate capitalization on top of the level lookup
 *   (`should_do_caps_transformation`), which fires whenever Lock is
 *   effective AND the key's type does not CONSUME Lock. A type with
 *   `modifiers = none` consumes nothing, so it does not suppress this at
 *   all -- it is strictly worse than a stock keymap, whose ALPHABETIC type
 *   consumes Lock. `modifiers = Lock` here is what turns it off. Without it,
 *   a human toggling Caps Lock in the host window (the core translates
 *   evdev 58 to XK_Caps_Lock, so this is the ordinary physical path)
 *   silently upper-cases every subsequent agent `text` payload: "hello"
 *   delivered as "HELLO", "héllo→世界" as "HÉLLO→世界". Both
 *   `xkb_state_key_get_one_sym` and `xkb_state_key_get_syms` apply it, so no
 *   toolkit escapes it.
 *
 * Lock, and NOTHING else, is consumed. Control and Mod1 are deliberately
 * left out: consuming Control would suppress `should_do_ctrl_transformation`
 * (Ctrl+C would stop producing U+0003) and consuming either would make
 * toolkits that build accelerator masks from
 * `xkb_state_mod_index_is_consumed` stop recognising Ctrl/Alt chords. Shift
 * is left out for a different reason -- it drives level selection, which the
 * `map` clauses have already neutralised, and it triggers no keysym
 * transformation, so consuming it would only hide it from accelerators.
 *
 * The type is deliberately NOT called "ONE_LEVEL": canonical XKB's
 * ONE_LEVEL is `modifiers = none`, and redefining a canonical name to mean
 * something else would mislead anyone reading the generated keymap. */
#define VITRIN_KEY_TYPE "VITRIN_PLAIN"

/* ---- the fixed keycode regions --------------------------------------- */

/* Modifier keysyms and the xkb modifier each one drives. These get real
 * `SetMods`/`LockMods` actions and `modifier_map` entries, so a chord
 * (Ctrl+C, Shift+Tab) reaches the app as a chord, and so a Caps Lock the
 * human toggled is visible to the app as locked state. They are the ONLY
 * keys in the keymap that touch modifier state, and because every key --
 * these included -- carries VITRIN_KEY_TYPE, the state they set can never
 * re-resolve an already-resolved keysym (the normative
 * modifier-suppression rule).
 *
 * The set is a superset of the core's layout-invariant table
 * (`invariant_keysym`, crates/vitrin-core/src/input/mod.rs) so that every
 * modifier the core can currently send is already bound, plus the ones a
 * later intake will send (AltGr, Meta, Num_Lock) so that adding them costs
 * no keymap regeneration. */
static const struct {
	uint32_t keysym;
	const char *modifier;
	bool locking;
} MODIFIER_KEYS[] = {
	{0xffe1u, "Shift", false},   /* Shift_L */
	{0xffe2u, "Shift", false},   /* Shift_R */
	{0xffe3u, "Control", false}, /* Control_L */
	{0xffe4u, "Control", false}, /* Control_R */
	{0xffe9u, "Mod1", false},    /* Alt_L */
	{0xffeau, "Mod1", false},    /* Alt_R */
	{0xffe7u, "Mod1", false},    /* Meta_L */
	{0xffe8u, "Mod1", false},    /* Meta_R */
	{0xffebu, "Mod4", false},    /* Super_L */
	{0xffecu, "Mod4", false},    /* Super_R */
	{0xfe03u, "Mod5", false},    /* ISO_Level3_Shift (AltGr) */
	{0xffe5u, "Lock", true},     /* Caps_Lock */
	{0xff7fu, "Mod2", true},     /* Num_Lock */
};

/* Non-printable keysyms pre-bound at startup: exactly the layout-invariant
 * editing / navigation / function keys the core's nested intake can produce
 * (`invariant_keysym`), minus the modifiers above, which have their own
 * region. Pre-binding them is what makes the ENTIRE human key path
 * regeneration-free: an app that reads the keymap once has every one of
 * these forever. */
static const uint32_t WARM_KEYSYMS[] = {
	0xff1bu, /* Escape    */
	0xff08u, /* BackSpace */
	0xff09u, /* Tab       */
	0xff0du, /* Return    */
	0xff8du, /* KP_Enter  */
	0xffffu, /* Delete    */
	0xff63u, /* Insert    */
	0xff50u, /* Home      */
	0xff57u, /* End       */
	0xff55u, /* Prior     */
	0xff56u, /* Next      */
	0xff51u, /* Left      */
	0xff52u, /* Up        */
	0xff53u, /* Right     */
	0xff54u, /* Down      */
	0xffbeu, 0xffbfu, 0xffc0u, 0xffc1u, 0xffc2u, 0xffc3u, /* F1..F6  */
	0xffc4u, 0xffc5u, 0xffc6u, 0xffc7u, 0xffc8u, 0xffc9u, /* F7..F12 */
};

/* Printable ASCII, whose keysym IS its codepoint. 95 slots that cover every
 * character an all-ASCII `text` payload can contain. */
#define ASCII_FIRST 0x20u
#define ASCII_LAST 0x7eu
#define ASCII_COUNT (ASCII_LAST - ASCII_FIRST + 1u)

#define MODIFIER_COUNT (sizeof(MODIFIER_KEYS) / sizeof(MODIFIER_KEYS[0]))
#define WARM_COUNT (sizeof(WARM_KEYSYMS) / sizeof(WARM_KEYSYMS[0]))

/* The whole fixed prefix must leave a usable dynamic ring; asserting it
 * here means a future addition to either table that squeezed the ring flat
 * is a build failure rather than a mystery at runtime. */
_Static_assert(MODIFIER_COUNT + WARM_COUNT + ASCII_COUNT + 64u <= VITRIN_KEY_SLOTS,
	"the fixed keycode regions must leave at least 64 dynamic slots");

/* `text_slots` holds a slot index per codepoint in a uint16_t. */
_Static_assert(VITRIN_KEY_SLOTS <= UINT16_MAX,
	"a keycode slot index must fit the type text_slots stores it in");

/* ---- small helpers ---------------------------------------------------- */

const char *vitrin_origin_name(struct vitrin_origin o) {
	if (!vitrin_origin_is_tagged(o)) {
		return "unset";
	}
	return vitrin_origin_wire(o) == VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL
		? "physical" : "emulated";
}

static uint32_t now_msec(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint32_t)((uint64_t)ts.tv_sec * 1000u + (uint64_t)ts.tv_nsec / 1000000u);
}

/* The evdev keycode that reaches the app for a slot. `wl_keyboard.key`
 * carries evdev codes, which are xkb keycodes minus the historical X11
 * offset of 8. */
static uint32_t slot_evdev(unsigned slot) {
	return VITRIN_KEYCODE_BASE + slot - 8u;
}

/* ---- the delivery trace ---------------------------------------------- */

/* One line per event, carrying the origin tag verbatim (seat.h, "where the
 * tag becomes observable"). Machine-readable on purpose: the acceptance
 * harness parses these and correlates them against what the core says it
 * sent, which is the available proof that the tag survived the hop.
 *
 * `delivered` is 0 for an event the shim deliberately did not replay (no
 * surface under the pointer, an unpaired release), and `reason` says which
 * -- a dropped event that left no trace would be indistinguishable from one
 * that was never routed. */
static void trace(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		const char *event, bool delivered, const char *reason, const char *fmt, ...)
	__attribute__((format(printf, 6, 7)));

static void trace(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		const char *event, bool delivered, const char *reason, const char *fmt, ...) {
	/* B2's runtime backstop. Every replay helper takes the tag as a
	 * mandatory parameter and every one of them funnels here, so an
	 * untagged delivery is caught at the single point it could pass
	 * through -- loudly, rather than by being printed as `physical`. */
	assert(vitrin_origin_is_tagged(origin));
	if (!vitrin_origin_is_tagged(origin)) {
		wlr_log(WLR_ERROR, "seat-replay: %s delivered with NO origin tag (B2 violation)", event);
	}

	char detail[256];
	va_list ap;
	va_start(ap, fmt);
	vsnprintf(detail, sizeof(detail), fmt, ap);
	va_end(ap);
	wlr_log(WLR_DEBUG, "seat-replay: seq=%llu event=%s origin=%s delivered=%d reason=%s %s",
		(unsigned long long)++r->seq, event, vitrin_origin_name(origin),
		delivered ? 1 : 0, reason, detail);
}

/* ---- keycode binding -------------------------------------------------- */

/* Find the slot carrying `keysym` in one of the two held states.
 *
 * The distinction is not bookkeeping, it is the whole point. A keysym can be
 * bound at more than one keycode at once (`slot_bind` mints a second one
 * precisely when the first is held), and the two callers want opposite
 * answers:
 *
 *   a PRESS needs a keycode the app is NOT already holding, or the app would
 *   receive a second `wl_keyboard.key` PRESSED for a keycode already down --
 *   a stream no real compositor produces;
 *
 *   a RELEASE needs the keycode the app IS holding, and specifically the one
 *   its own press went down on, or the press would be left stranded.
 *
 * `XKB_KEY_NoSymbol` is excluded from both. Unbound slots carry keysym 0, so
 * a lookup that treated 0 as searchable would "find" the first free dynamic
 * slot for any event whose keysym argument is 0 -- which the IDL permits,
 * `keysym` being a bare `uint` the decoder does not constrain -- and deliver
 * a real press on a keycode that carries no symbol at all. */
static int slot_find(const struct vitrin_seat_replay *r, uint32_t keysym, bool held) {
	if (keysym == XKB_KEY_NoSymbol) {
		return -1;
	}
	for (unsigned i = 0; i < VITRIN_KEY_SLOTS; i++) {
		if (r->slots[i].keysym == keysym && r->slots[i].held == held) {
			return (int)i;
		}
	}
	return -1;
}

/* A keycode carrying `keysym` that the app is not holding down. */
static int slot_free_of(const struct vitrin_seat_replay *r, uint32_t keysym) {
	return slot_find(r, keysym, false);
}

/* The keycode carrying `keysym` that the app IS holding down. */
static int slot_held_of(const struct vitrin_seat_replay *r, uint32_t keysym) {
	return slot_find(r, keysym, true);
}

/* Fill one of the two fixed regions. Only ever called from init. */
static void slot_pin(struct vitrin_seat_replay *r, unsigned slot, uint32_t keysym) {
	r->slots[slot].keysym = keysym;
	r->slots[slot].bound_seq = 0;
	r->keymap_dirty = true;
}

/* Bind `keysym` to a keycode and return its slot, or -1 if the dynamic ring
 * cannot take it right now.
 *
 * `chunk_floor` is the bind sequence at the start of the current delivery
 * chunk. A dynamic slot bound at or after it belongs to the string being
 * typed right now and must not be recycled out from under it; a slot whose
 * key the app currently holds down must not be recycled either, or the app
 * would receive a release for a keycode that has since changed meaning.
 * Everything else is fair game, oldest first.
 *
 * A keysym that is ALREADY bound but whose keycode the app is holding down
 * does not short-circuit: it falls through to the ring and gets a second
 * keycode of its own. That is the collision between a key a human is
 * physically holding and the same character appearing in an agent's `text`
 * payload -- space, Tab, Return, Escape and the arrows are all reachable
 * both ways, and the IDL's own scenario types a string ending in "\n" while
 * nothing stops a human from resting a finger on Enter. Reusing the held
 * keycode would emit a duplicate PRESSED and then swallow the human's real
 * release; a second keycode keeps the two presses independent, each paired
 * with its own release. */
static int slot_bind(struct vitrin_seat_replay *r, uint32_t keysym, uint64_t chunk_floor) {
	int existing = slot_free_of(r, keysym);
	if (existing >= 0) {
		return existing;
	}

	unsigned dyn_first = r->warm_slots;
	unsigned dyn_count = VITRIN_KEY_SLOTS - dyn_first;
	for (unsigned tried = 0; tried < dyn_count; tried++) {
		unsigned i = dyn_first + (r->ring_next + tried) % dyn_count;
		struct vitrin_key_slot *slot = &r->slots[i];
		if (slot->held) {
			continue;
		}
		if (slot->keysym != XKB_KEY_NoSymbol && slot->bound_seq >= chunk_floor) {
			continue;
		}
		r->ring_next = (r->ring_next + tried + 1u) % dyn_count;
		if (slot->keysym != XKB_KEY_NoSymbol) {
			wlr_log(WLR_DEBUG,
				"dynamic keycode %u recycled: keysym 0x%08x -> 0x%08x "
				"(an app that never re-reads the keymap loses the new one)",
				VITRIN_KEYCODE_BASE + i, slot->keysym, keysym);
		}
		slot->keysym = keysym;
		slot->bound_seq = ++r->bind_seq;
		r->keymap_dirty = true;
		return (int)i;
	}
	return -1;
}

/* ---- keymap generation ------------------------------------------------ */

/* Render the current binding table as an xkb keymap string.
 *
 * Self-contained: no `include "complete"`, so nothing here depends on
 * xkeyboard-config being installed or on which version of it is (seat.h,
 * D7). The keycodes section declares every slot unconditionally, bound or
 * not, so the keycode NAMES never move between generations -- only the
 * symbols attached to them -- which is what keeps regeneration additive.
 *
 * Every key carries an explicit VITRIN_KEY_TYPE. That is the
 * modifier-suppression rule written into the keymap rather than inferred
 * from it -- see the type's own comment above for what each of its two
 * clauses suppresses, and why Lock is the only modifier it consumes. */
static char *keymap_text(const struct vitrin_seat_replay *r) {
	char *buf = NULL;
	size_t len = 0;
	FILE *f = open_memstream(&buf, &len);
	if (f == NULL) {
		return NULL;
	}

	fprintf(f, "xkb_keymap {\n");

	fprintf(f, "xkb_keycodes \"vitrin\" {\n  minimum = %u;\n  maximum = %u;\n",
		VITRIN_KEYCODE_BASE, VITRIN_KEYCODE_BASE + VITRIN_KEY_SLOTS - 1u);
	for (unsigned i = 0; i < VITRIN_KEY_SLOTS; i++) {
		fprintf(f, "  <K%u> = %u;\n", i, VITRIN_KEYCODE_BASE + i);
	}
	fprintf(f, "};\n");

	/* The one key type the whole keymap uses, spelled out rather than
	 * included, and the virtual modifiers xkbcommon expects to exist. */
	fprintf(f,
		"xkb_types \"vitrin\" {\n"
		"  virtual_modifiers NumLock,Alt,LevelThree,Super,LevelFive,Meta,Hyper,ScrollLock;\n"
		"  type \"" VITRIN_KEY_TYPE "\" {\n"
		"    modifiers = Lock;\n"
		"    map[none] = Level1;\n"
		"    map[Lock] = Level1;\n"
		"    level_name[Level1] = \"Any\";\n"
		"  };\n"
		"};\n");

	/* Empty: modifier behaviour is expressed as explicit actions on the
	 * modifier keys below, so there is no interpretation left for a compat
	 * section to supply. An empty one is also the only kind that cannot
	 * disagree with the actions. */
	fprintf(f, "xkb_compat \"vitrin\" { };\n");

	fprintf(f, "xkb_symbols \"vitrin\" {\n  name[Group1] = \"vitrin\";\n");
	for (unsigned i = 0; i < VITRIN_KEY_SLOTS; i++) {
		uint32_t keysym = r->slots[i].keysym;
		if (keysym == XKB_KEY_NoSymbol) {
			continue; /* a declared keycode with no symbol: NoSymbol */
		}
		if (i < r->mod_slots) {
			const char *mod = MODIFIER_KEYS[i].modifier;
			fprintf(f,
				"  key <K%u> { type[Group1]=\"" VITRIN_KEY_TYPE "\", symbols[Group1]=[0x%08x], "
				"actions[Group1]=[%s(modifiers=%s)] };\n",
				i, keysym, MODIFIER_KEYS[i].locking ? "LockMods" : "SetMods", mod);
		} else {
			fprintf(f,
				"  key <K%u> { type[Group1]=\"" VITRIN_KEY_TYPE "\", symbols[Group1]=[0x%08x] };\n",
				i, keysym);
		}
	}
	for (unsigned i = 0; i < r->mod_slots; i++) {
		fprintf(f, "  modifier_map %s { <K%u> };\n", MODIFIER_KEYS[i].modifier, i);
	}
	fprintf(f, "};\n};\n");

	if (fclose(f) != 0) {
		free(buf);
		return NULL;
	}
	return buf;
}

/* Compile the current table and hand it to the seat, which forwards
 * `wl_keyboard.keymap` to every bound client (wlroots wires that relay in
 * `wlr_seat_set_keyboard`). A no-op when nothing has changed, which is the
 * steady state for ASCII text: the cheapest regeneration is the one that
 * does not happen. */
static bool keymap_sync(struct vitrin_seat_replay *r) {
	if (!r->keymap_dirty || r->keyboard == NULL) {
		return true;
	}
	char *text = keymap_text(r);
	if (text == NULL) {
		wlr_log(WLR_ERROR, "cannot render the dynamic keymap");
		return false;
	}
	struct xkb_keymap *keymap = xkb_keymap_new_from_string(r->xkb, text,
		XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
	free(text);
	if (keymap == NULL) {
		wlr_log(WLR_ERROR, "the dynamic keymap did not compile");
		return false;
	}
	bool ok = wlr_keyboard_set_keymap(r->keyboard, keymap);
	xkb_keymap_unref(keymap);
	if (!ok) {
		wlr_log(WLR_ERROR, "wlr_keyboard_set_keymap refused the dynamic keymap");
		return false;
	}

	/* Re-announce modifier state, unconditionally, right behind the keymap.
	 *
	 * A client that receives `wl_keyboard.keymap` must rebuild its own
	 * `xkb_state` from the new keymap, and a fresh state has NO modifiers
	 * set. wlroots does not tell it otherwise: `wlr_keyboard_set_keymap`
	 * (0.19.3) builds a fresh `xkb_state`, replays the held keycodes into
	 * it, calls `keyboard_modifier_update` DISCARDING the return value, and
	 * emits only `events.keymap` -- so `wl_keyboard.modifiers` is never
	 * sent. Real compositors never notice, because they do not regenerate
	 * keymaps mid-session; this one does it whenever a codepoint is new.
	 *
	 * Left unsent, a human holding Control while an agent types one
	 * previously-unseen character would have every chord after it broken
	 * until they released and re-pressed -- Ctrl+C arriving at a text editor
	 * as a literal 'c'. That is exactly the concurrent human+agent operation
	 * this product exists for.
	 *
	 * It has to be unconditional rather than routed through the
	 * `events.modifiers` relay, because in both failing cases that signal is
	 * silent: for a HELD modifier wlroots correctly sees no change (it
	 * replayed the keycodes), and for a LOCKED one (Caps, Num) the change is
	 * real but the discarded return value swallows it. Our side is right
	 * either way; it is the app's side that has just been reset. */
	wlr_seat_keyboard_notify_modifiers(r->shim->seat, &r->keyboard->modifiers);

	r->keymap_dirty = false;
	r->keymap_generations++;
	wlr_log(WLR_DEBUG, "dynamic keymap generation %llu uploaded",
		(unsigned long long)r->keymap_generations);
	return true;
}

/* ---- key synthesis ---------------------------------------------------- */

/* Press or release one keycode on the app's keyboard.
 *
 * Two calls, in this order, because they do two different things and the
 * order is what the app observes:
 *
 *   wlr_seat_keyboard_notify_key    sends `wl_keyboard.key` to the app
 *   wlr_keyboard_notify_key         updates OUR xkb state, which fires the
 *                                   modifiers relay -> `wl_keyboard.modifiers`
 *
 * So a modifier press arrives as key-then-modifiers, which is the ordering
 * every wlroots compositor produces from a real device, and by the time the
 * next key arrives the app's modifier state is already correct. */
static void key_send(struct vitrin_seat_replay *r, unsigned slot, bool pressed) {
	struct vitrin_shim *s = r->shim;
	uint32_t time = now_msec();
	uint32_t evdev = slot_evdev(slot);

	wlr_seat_keyboard_notify_key(s->seat, time, evdev,
		pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);

	struct wlr_keyboard_key_event ev = {
		.time_msec = time,
		.keycode = evdev,
		/* True: this keyboard has no backend to compute modifier state for
		 * it, so the state must be derived from the keys we inject. */
		.update_state = true,
		.state = pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED,
	};
	wlr_keyboard_notify_key(r->keyboard, &ev);

	r->slots[slot].held = pressed;
	r->keys_synthesized++;
}

/* ---- pointer replay --------------------------------------------------- */

static bool button_is_pressed(const struct vitrin_seat_replay *r, uint32_t button) {
	for (unsigned i = 0; i < r->npressed; i++) {
		if (r->pressed[i] == button) {
			return true;
		}
	}
	return false;
}

static bool button_release(struct vitrin_seat_replay *r, uint32_t button) {
	for (unsigned i = 0; i < r->npressed; i++) {
		if (r->pressed[i] == button) {
			memmove(&r->pressed[i], &r->pressed[i + 1],
				(r->npressed - i - 1) * sizeof(r->pressed[0]));
			r->npressed--;
			return true;
		}
	}
	return false;
}

/* Realm-view coordinates -> the shim's output-layout space.
 *
 * D10: the core has ALREADY compensated for its own letterboxing, so (0, 0)
 * on the wire is the top-left of the content this shim forwarded, and no
 * second placement offset belongs here. What does belong is the output's
 * own position in the layout, asked of the layout rather than assumed to be
 * (0, 0) -- the same discipline the core follows by routing through the
 * very `layout::place` its scene paints with. */
static bool view_to_layout(struct vitrin_shim *s, double vx, double vy,
		double *lx, double *ly) {
	if (s->layout == NULL || s->output == NULL) {
		return false;
	}
	struct wlr_box box;
	wlr_output_layout_get_box(s->layout, s->output, &box);
	if (wlr_box_empty(&box)) {
		return false;
	}
	*lx = vx + (double)box.x;
	*ly = vy + (double)box.y;
	return true;
}

/* The surface under a layout point, and that point in its surface-local
 * coordinates.
 *
 * `wlr_scene_node_at` walks the very scene graph the compositor composited,
 * so the replayer and the renderer cannot disagree about where a surface
 * is. It is also the only reason popups work: `wlr_scene_xdg_surface_create`
 * (xdg.c) puts a toplevel's menus and tooltips in the scene as separate
 * surfaces, and a shim that mapped view coordinates straight onto the
 * toplevel would send every menu click to the window behind the menu. */
static struct wlr_surface *surface_at(struct vitrin_shim *s, double lx, double ly,
		double *sx, double *sy) {
	if (s->scene == NULL) {
		return NULL;
	}
	struct wlr_scene_node *node = wlr_scene_node_at(&s->scene->tree.node, lx, ly, sx, sy);
	if (node == NULL || node->type != WLR_SCENE_NODE_BUFFER) {
		return NULL;
	}
	struct wlr_scene_surface *scene_surface =
		wlr_scene_surface_try_from_buffer(wlr_scene_buffer_from_node(node));
	return scene_surface != NULL ? scene_surface->surface : NULL;
}

static void replay_motion(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		double vx, double vy) {
	struct vitrin_shim *s = r->shim;
	double lx = 0, ly = 0;
	if (!view_to_layout(s, vx, vy, &lx, &ly)) {
		r->dropped++;
		trace(r, origin, "motion", false, "no-output", "view=%.3f,%.3f", vx, vy);
		return;
	}

	struct wlr_surface *focused = s->seat->pointer_state.focused_surface;

	/* Implicit grab: while a press the app has seen is still outstanding,
	 * the pointer stays on the surface that received it and the coordinates
	 * are computed from the offset captured at focus time -- they may leave
	 * [0, size), which is exactly what a drag off the edge of a window looks
	 * like in Wayland. Re-hit-testing here instead would hand the drag to
	 * whatever is under the cursor now, or to nothing, and strand a pressed
	 * button in the app forever. */
	if (r->npressed > 0 && focused != NULL) {
		double sx = lx - r->surface_ox;
		double sy = ly - r->surface_oy;
		wlr_seat_pointer_notify_motion(s->seat, now_msec(), sx, sy);
		r->frame_pending = true;
		r->delivered[VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE]++;
		trace(r, origin, "motion", true, "grab",
			"view=%.3f,%.3f surface=%.3f,%.3f", vx, vy, sx, sy);
		return;
	}

	double sx = 0, sy = 0;
	struct wlr_surface *surface = surface_at(s, lx, ly, &sx, &sy);
	if (surface == NULL) {
		/* Nothing to point at. The core has already dropped matte clicks, so
		 * this is the shim's own view having a hole in it: the app has not
		 * mapped yet, or its surface does not cover the whole realm view.
		 * wlroots emits the closing `wl_pointer.frame` for the leave itself,
		 * so this path must not also arm one -- see the note on enter. */
		if (focused != NULL) {
			wlr_seat_pointer_notify_clear_focus(s->seat);
		}
		r->dropped++;
		trace(r, origin, "motion", false, "no-surface", "view=%.3f,%.3f", vx, vy);
		return;
	}

	/* Focus is synthesized here: the wire carries no focus event, so
	 * pointer-enter happens on the first motion that lands on a surface
	 * (IDL: "focus in version 1 is synthesized shim-side").
	 *
	 * THE HIT TEST ABOVE IS ALSO WHAT MAKES THIS CORRECT WITH SEVERAL
	 * WINDOWS, and that used to be an accident this comment did not know
	 * about: it read "version 1's realm is single-surface", which stopped
	 * being true the first time an app opened a second toplevel (issue
	 * #268). Nothing here needed changing, because `surface_at` re-hit-tests
	 * the scene on EVERY motion instead of caching a focused surface the way
	 * the keyboard path does -- so a window that unmaps drops out of the
	 * scene and out of the answer on the next event, and a sibling under the
	 * pointer gets an enter from the same unchanged code. The full argument,
	 * including the one case that is not instant (a drag in progress, which
	 * takes the grab branch above deliberately), is in xdg.c's
	 * `toplevel_unmap`; it is not restated here. */
	r->surface_ox = lx - sx;
	r->surface_oy = ly - sy;
	if (surface != focused) {
		/* An enter carries the position, and wlroots closes it with its own
		 * `wl_pointer.frame`. Sending a motion to the same coordinates on
		 * top would be deduplicated away by wlroots and leave this shim
		 * arming a second, EMPTY frame group -- so the enter is the whole
		 * delivery, and the frame is wlroots' to send. */
		wlr_seat_pointer_notify_enter(s->seat, surface, sx, sy);
	} else {
		wlr_seat_pointer_notify_motion(s->seat, now_msec(), sx, sy);
		r->frame_pending = true;
	}
	r->delivered[VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE]++;
	trace(r, origin, "motion", true, surface != focused ? "enter" : "ok",
		"view=%.3f,%.3f surface=%.3f,%.3f", vx, vy, sx, sy);
}

static void replay_button(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		uint32_t button, vitrin_actuator_pointer_button_state_t state) {
	struct vitrin_shim *s = r->shim;
	bool pressed = state == VITRIN_ACTUATOR_POINTER_BUTTON_STATE_PRESSED;

	if (pressed) {
		/* A press with no pointer focus has no destination. The core's own
		 * razor already dropped presses on the letterbox matte; this one is
		 * about the shim's hit test, which can fail for a reason the core
		 * cannot see (no mapped surface yet). The two are not redundant. */
		if (s->seat->pointer_state.focused_surface == NULL) {
			r->dropped++;
			trace(r, origin, "button", false, "no-focus", "button=%u state=pressed", button);
			return;
		}
		if (r->npressed >= VITRIN_MAX_PRESSED_BUTTONS) {
			r->dropped++;
			trace(r, origin, "button", false, "too-many-held", "button=%u state=pressed", button);
			return;
		}
		if (!button_is_pressed(r, button)) {
			r->pressed[r->npressed++] = button;
		}
	} else if (!button_release(r, button)) {
		/* A release is replayed iff its own press was -- per button code, so
		 * a press this shim dropped can never borrow another button's grab.
		 * Delivering it anyway would tell the app a button it never saw go
		 * down has come up. */
		r->dropped++;
		trace(r, origin, "button", false, "unpaired-release", "button=%u state=released", button);
		return;
	}

	wlr_seat_pointer_notify_button(s->seat, now_msec(), button,
		pressed ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
	r->frame_pending = true;
	r->delivered[VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE]++;
	trace(r, origin, "button", true, "ok", "button=%u state=%s held=%u",
		button, pressed ? "pressed" : "released", r->npressed);
}

static void replay_scroll(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		vitrin_actuator_pointer_axis_t axis, int32_t value120) {
	struct vitrin_shim *s = r->shim;
	if (s->seat->pointer_state.focused_surface == NULL) {
		r->dropped++;
		trace(r, origin, "scroll", false, "no-focus", "axis=%u value120=%d",
			(unsigned)axis, value120);
		return;
	}

	enum wl_pointer_axis orientation =
		axis == VITRIN_ACTUATOR_POINTER_AXIS_HORIZONTAL
			? WL_POINTER_AXIS_HORIZONTAL_SCROLL
			: WL_POINTER_AXIS_VERTICAL_SCROLL;
	/* Both magnitudes travel: `value120` verbatim for a client that speaks
	 * wl_pointer v8 (which is the whole reason the wire carries v120), and
	 * the continuous equivalent for one that does not. wlroots derives the
	 * legacy `discrete` steps from `value_discrete` itself. */
	double value = (double)value120 * VITRIN_SCROLL_PIXELS_PER_NOTCH / 120.0;
	wlr_seat_pointer_notify_axis(s->seat, now_msec(), orientation, value, value120,
		WL_POINTER_AXIS_SOURCE_WHEEL, WL_POINTER_AXIS_RELATIVE_DIRECTION_IDENTICAL);
	r->frame_pending = true;
	r->delivered[VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE]++;
	trace(r, origin, "scroll", true, "ok", "axis=%s value120=%d value=%.3f",
		orientation == WL_POINTER_AXIS_VERTICAL_SCROLL ? "vertical" : "horizontal",
		value120, value);
}

/* ---- relative pointer replay (protocol version 2) --------------------- */

/* `relative_motion` -> `zwp_relative_pointer_v1.relative_motion`.
 *
 * THIS DOES NOT MOVE THE POINTER, and that is the whole shape of the event.
 * The absolute `motion` that accompanies it is what moves the cursor and
 * drives the hit test; this carries the delta an app integrates for itself.
 * Replaying it as a motion instead would double every movement.
 *
 * FOCUS IS THE GATE. `wlr_relative_pointer_manager_v1_send_relative_motion`
 * fans out to the relative-pointer objects of the seat's FOCUSED client only
 * (wlroots checks `seat->pointer_state.focused_surface`'s client), so a delta
 * arriving before the app has pointer focus reaches nobody. The check is made
 * here as well, and not left to wlroots, for the trace's sake: a silent
 * no-delivery would be recorded as `delivered=1`, and the trace is the only
 * evidence the origin tag survived the hop.
 *
 * NO FRAME. `wl_pointer.frame` groups wl_pointer events, and this is not one
 * -- the relative-pointer protocol has no frame of its own and wlroots emits
 * none. Arming `frame_pending` here would make the shim send a frame for an
 * empty group whenever a delta arrived with no wl_pointer event beside it.
 *
 * MICROSECONDS, not milliseconds: this one wlroots entry point takes usec
 * (its header says so in as many words), where every other one here takes
 * msec. Stamped with the shim's own clock, as every replay is -- the wire
 * carries no timestamp, deliberately, so there is no device clock to prefer. */
static void replay_relative_motion(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		double dx, double dy, double dx_unaccel, double dy_unaccel) {
	struct vitrin_shim *s = r->shim;
	if (r->relative_pointers == NULL) {
		r->dropped++;
		trace(r, origin, "relative_motion", false, "no-global",
			"dx=%.3f,%.3f unaccel=%.3f,%.3f", dx, dy, dx_unaccel, dy_unaccel);
		return;
	}
	if (s->seat->pointer_state.focused_surface == NULL) {
		r->dropped++;
		trace(r, origin, "relative_motion", false, "no-focus",
			"dx=%.3f,%.3f unaccel=%.3f,%.3f", dx, dy, dx_unaccel, dy_unaccel);
		return;
	}

	uint64_t time_usec = (uint64_t)now_msec() * 1000u;
	wlr_relative_pointer_manager_v1_send_relative_motion(
		r->relative_pointers, s->seat, time_usec, dx, dy, dx_unaccel, dy_unaccel);
	r->delivered[VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE]++;
	trace(r, origin, "relative_motion", true, "ok",
		"dx=%.3f,%.3f unaccel=%.3f,%.3f", dx, dy, dx_unaccel, dy_unaccel);
}

/* ---- gesture replay (protocol version 2) ------------------------------ */

static const char *gesture_kind_name(vitrin_shim_seat_gesture_kind_t kind) {
	return kind == VITRIN_SHIM_SEAT_GESTURE_KIND_PINCH ? "pinch" : "swipe";
}

/* Every gesture replay needs the same two things to be true, and reports the
 * same two failures the same way. Returns false having already traced. */
static bool gesture_ready(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		const char *event, const char *detail) {
	if (r->gestures == NULL) {
		r->dropped++;
		trace(r, origin, event, false, "no-global", "%s", detail);
		return false;
	}
	if (r->shim->seat->pointer_state.focused_surface == NULL) {
		r->dropped++;
		trace(r, origin, event, false, "no-focus", "%s", detail);
		return false;
	}
	return true;
}

static void replay_gesture_begin(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		vitrin_shim_seat_gesture_kind_t kind, uint32_t fingers) {
	char detail[64];
	snprintf(detail, sizeof(detail), "kind=%s fingers=%u", gesture_kind_name(kind), fingers);
	if (!gesture_ready(r, origin, "gesture_begin", detail)) {
		return;
	}
	/* The IDL's "at most one gesture is in flight per seat", enforced rather
	 * than trusted. A second begin is the core's bug: replaying it would give
	 * the app two begins to pair one end against, and the app's own
	 * accounting -- not this shim's -- is what would be left broken. Ignored
	 * and logged; the connection stays up, because log-and-close is the
	 * remedy for a SHIM's violation and an app must not die of the core's. */
	if (r->gesture_live) {
		r->dropped++;
		trace(r, origin, "gesture_begin", false, "already-in-flight",
			"%s in_flight=%s", detail, gesture_kind_name(r->gesture_kind));
		return;
	}

	uint32_t time = now_msec();
	if (kind == VITRIN_SHIM_SEAT_GESTURE_KIND_PINCH) {
		wlr_pointer_gestures_v1_send_pinch_begin(r->gestures, r->shim->seat, time, fingers);
	} else {
		wlr_pointer_gestures_v1_send_swipe_begin(r->gestures, r->shim->seat, time, fingers);
	}
	/* Recorded only on the delivered path, exactly as `pressed[]` is: an app
	 * that was never told a gesture began must never be sent its updates or
	 * its end. */
	r->gesture_live = true;
	r->gesture_kind = kind;
	r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE]++;
	trace(r, origin, "gesture_begin", true, "ok", "%s", detail);
}

/* Shared by both update events: an update is replayed iff a gesture of ITS
 * kind is the one this shim has in flight. `event` and `detail` are the
 * caller's so the trace names the actual event. */
static bool gesture_update_ready(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		vitrin_shim_seat_gesture_kind_t kind, const char *event, const char *detail) {
	if (!gesture_ready(r, origin, event, detail)) {
		return false;
	}
	if (!r->gesture_live) {
		r->dropped++;
		trace(r, origin, event, false, "no-gesture-in-flight", "%s", detail);
		return false;
	}
	if (r->gesture_kind != kind) {
		/* A pinch update inside a swipe. The app is tracking a swipe; handing
		 * it pinch motion would be motion for a gesture of a shape it is not
		 * making. */
		r->dropped++;
		trace(r, origin, event, false, "wrong-gesture-kind",
			"%s in_flight=%s", detail, gesture_kind_name(r->gesture_kind));
		return false;
	}
	return true;
}

static void replay_gesture_swipe_update(struct vitrin_seat_replay *r,
		struct vitrin_origin origin, double dx, double dy) {
	char detail[64];
	snprintf(detail, sizeof(detail), "dx=%.3f dy=%.3f", dx, dy);
	if (!gesture_update_ready(r, origin, VITRIN_SHIM_SEAT_GESTURE_KIND_SWIPE,
			"gesture_swipe_update", detail)) {
		return;
	}
	wlr_pointer_gestures_v1_send_swipe_update(r->gestures, r->shim->seat, now_msec(), dx, dy);
	r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE]++;
	trace(r, origin, "gesture_swipe_update", true, "ok", "%s", detail);
}

static void replay_gesture_pinch_update(struct vitrin_seat_replay *r,
		struct vitrin_origin origin, double dx, double dy, double scale, double rotation) {
	char detail[96];
	/* `scale` is absolute since the begin while the other three are deltas
	 * (IDL). Nothing here differences or accumulates it -- it is passed
	 * through, because wlroots' `send_pinch_update` takes the same absolute
	 * quantity `zwp_pointer_gestures_v1` defines. The two agree by
	 * construction, which is why this is a forward and not a conversion. */
	snprintf(detail, sizeof(detail), "dx=%.3f dy=%.3f scale=%.3f rotation=%.3f",
		dx, dy, scale, rotation);
	if (!gesture_update_ready(r, origin, VITRIN_SHIM_SEAT_GESTURE_KIND_PINCH,
			"gesture_pinch_update", detail)) {
		return;
	}
	wlr_pointer_gestures_v1_send_pinch_update(
		r->gestures, r->shim->seat, now_msec(), dx, dy, scale, rotation);
	r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE]++;
	trace(r, origin, "gesture_pinch_update", true, "ok", "%s", detail);
}

static void replay_gesture_end(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		vitrin_shim_seat_gesture_kind_t kind, vitrin_shim_seat_gesture_state_t state) {
	bool cancelled = state == VITRIN_SHIM_SEAT_GESTURE_STATE_CANCELLED;
	char detail[80];
	snprintf(detail, sizeof(detail), "kind=%s state=%s",
		gesture_kind_name(kind), cancelled ? "cancelled" : "completed");

	/* Checked BEFORE `gesture_ready`, and the order is the point. An end with
	 * nothing in flight is dropped whatever the state of the globals; but if
	 * a gesture IS live and the app has since lost pointer focus, the local
	 * bookkeeping must still be cleared -- the gesture is over either way,
	 * and leaving it live would make every later gesture in this shim's life
	 * be refused as "already in flight". Same shape as the key path's
	 * `no-keyboard-focus` release arm, which reconciles and then reports the
	 * truth. */
	if (!r->gesture_live) {
		r->dropped++;
		trace(r, origin, "gesture_end", false, "no-gesture-in-flight", "%s", detail);
		return;
	}
	/* The gesture that is actually in flight is the one that gets ended,
	 * never the one the event named. `kind` is redundant by construction and
	 * is carried so that a disagreement is a DETECTABLE core bug rather than
	 * a silent mis-replay (IDL); ending nothing on a mismatch would leave the
	 * app accumulating the real gesture forever, which is the failure the
	 * argument exists to catch. */
	vitrin_shim_seat_gesture_kind_t live = r->gesture_kind;
	if (live != kind) {
		wlr_log(WLR_ERROR,
			"gesture_end names %s but %s is in flight; ending the one in flight "
			"(a core bug -- the wire carries `kind` so this is visible rather than silent)",
			gesture_kind_name(kind), gesture_kind_name(live));
	}
	r->gesture_live = false;

	if (!gesture_ready(r, origin, "gesture_end", detail)) {
		return; /* already traced, and the bookkeeping is reconciled above */
	}
	uint32_t time = now_msec();
	if (live == VITRIN_SHIM_SEAT_GESTURE_KIND_PINCH) {
		wlr_pointer_gestures_v1_send_pinch_end(r->gestures, r->shim->seat, time, cancelled);
	} else {
		wlr_pointer_gestures_v1_send_swipe_end(r->gestures, r->shim->seat, time, cancelled);
	}
	r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE]++;
	trace(r, origin, "gesture_end", true, live == kind ? "ok" : "kind-mismatch",
		"%s ended=%s", detail, gesture_kind_name(live));
}

void vitrin_seat_frame_boundary(struct vitrin_shim *s) {
	struct vitrin_seat_replay *r = &s->replay;
	if (!r->frame_pending) {
		return;
	}
	r->frame_pending = false;
	wlr_seat_pointer_notify_frame(s->seat);
}

/* ---- keyboard replay -------------------------------------------------- */

/* Is there an app holding keyboard focus to deliver to at all?
 *
 * Focus is taken when the app's toplevel maps and given back when it
 * unmaps, so "no focus" means there is no window yet (or no longer). Keys
 * and text delivered into that gap reach nobody -- `wl_keyboard` events go
 * to the focused client and to nothing otherwise -- so the honest report is
 * a drop rather than a success. Saying `delivered=1` for an event no app
 * received would make the delivery trace a record of intent instead of a
 * record of fact, and the trace is the only evidence the origin tag
 * survived. */
static bool keyboard_focused(const struct vitrin_seat_replay *r) {
	return r->shim->seat != NULL &&
		r->shim->seat->keyboard_state.focused_surface != NULL;
}

static void replay_key(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		uint32_t keysym, vitrin_shim_seat_key_state_t state) {
	bool pressed = state == VITRIN_SHIM_SEAT_KEY_STATE_PRESSED;

	/* The IDL's `keysym` is a bare `uint`, so the decoder cannot reject
	 * NoSymbol for us and this has to. Today's core never emits it
	 * (`invariant_keysym` returns a real keysym or nothing at all), but
	 * "the core would not do that" is not a bound on what arrives -- the
	 * same reasoning that makes utf8_next decode defensively. A press with
	 * no symbol has nothing to deliver; letting it through would take a
	 * keycode, mark it held, and leave the app holding a phantom key. */
	if (keysym == XKB_KEY_NoSymbol) {
		r->dropped++;
		trace(r, origin, "key", false, "no-keysym", "keysym=0x%08x state=%s",
			keysym, pressed ? "pressed" : "released");
		return;
	}

	if (!pressed) {
		/* Release: look up the keycode this keysym is HELD on, and never
		 * bind. Binding here would mint a keycode for a key the app was
		 * never told about and then release it, and (worse) a keysym whose
		 * press predates a keymap regeneration would come back on a
		 * different keycode than it went down on. */
		int slot = slot_held_of(r, keysym);
		if (slot < 0) {
			r->dropped++;
			trace(r, origin, "key", false, "unpaired-release", "keysym=0x%08x", keysym);
			return;
		}
		if (!keyboard_focused(r)) {
			/* Focus went away between the press and its release (the app
			 * unmapped mid-chord). Reconcile the local bookkeeping anyway --
			 * the key is physically up, and leaving it marked held would
			 * pin its keycode out of the recycler forever -- but report the
			 * truth, which is that nothing was delivered. */
			r->slots[slot].held = false;
			r->dropped++;
			trace(r, origin, "key", false, "no-keyboard-focus",
				"keysym=0x%08x state=released", keysym);
			return;
		}
		key_send(r, (unsigned)slot, false);
		r->delivered[VITRIN_SHIM_SEAT_EVT_KEY_OPCODE]++;
		trace(r, origin, "key", true, "ok", "keysym=0x%08x state=released keycode=%u",
			keysym, VITRIN_KEYCODE_BASE + (unsigned)slot);
		return;
	}

	if (!keyboard_focused(r)) {
		/* Checked BEFORE binding: a keysym bound for a delivery that cannot
		 * happen would still consume a dynamic slot and still force a keymap
		 * regeneration on the app -- work and churn for nobody. */
		r->dropped++;
		trace(r, origin, "key", false, "no-keyboard-focus",
			"keysym=0x%08x state=pressed", keysym);
		return;
	}
	/* A press for a keysym the app is already holding is dropped, not
	 * doubled. Delivering it on the same keycode would be a second PRESSED
	 * with no RELEASED between -- a stream no real compositor produces, and
	 * one an Xwayland bridge would disagree with us about. Delivering it on
	 * a FRESH keycode would be worse: two keycodes down, one release to
	 * come, and a key stuck forever. The `key` path is state-carrying (a
	 * chord either is or is not held), so idempotence is the honest answer;
	 * `text` cannot do this because dropping there would silently delete a
	 * character from the string, which is why it binds a second keycode
	 * instead. */
	if (slot_held_of(r, keysym) >= 0) {
		r->dropped++;
		trace(r, origin, "key", false, "already-held",
			"keysym=0x%08x state=pressed", keysym);
		return;
	}
	int slot = slot_bind(r, keysym, r->bind_seq + 1u);
	if (slot < 0 || !keymap_sync(r)) {
		r->dropped++;
		trace(r, origin, "key", false, "no-keycode", "keysym=0x%08x", keysym);
		return;
	}
	key_send(r, (unsigned)slot, true);
	r->delivered[VITRIN_SHIM_SEAT_EVT_KEY_OPCODE]++;
	trace(r, origin, "key", true, "ok", "keysym=0x%08x state=pressed keycode=%u",
		keysym, VITRIN_KEYCODE_BASE + (unsigned)slot);
}

/* Decode one UTF-8 scalar at `p`, advancing `*used`. Returns false on any
 * malformed sequence -- overlong, truncated, surrogate, out of range -- in
 * which case the caller skips a single byte and resynchronises. The core
 * validates this string on its way in, but the shim decodes defensively
 * anyway: a decoder that trusted its input would turn a core bug into an
 * out-of-bounds read here. */
static bool utf8_next(const uint8_t *p, size_t avail, uint32_t *out, size_t *used) {
	uint8_t c = p[0];
	unsigned extra;
	uint32_t cp;
	if (c < 0x80u) {
		*out = c;
		*used = 1;
		return true;
	} else if ((c & 0xe0u) == 0xc0u) {
		extra = 1;
		cp = c & 0x1fu;
	} else if ((c & 0xf0u) == 0xe0u) {
		extra = 2;
		cp = c & 0x0fu;
	} else if ((c & 0xf8u) == 0xf0u) {
		extra = 3;
		cp = c & 0x07u;
	} else {
		return false;
	}
	if (avail < (size_t)extra + 1u) {
		return false;
	}
	for (unsigned i = 1; i <= extra; i++) {
		if ((p[i] & 0xc0u) != 0x80u) {
			return false;
		}
		cp = (cp << 6) | (p[i] & 0x3fu);
	}
	static const uint32_t MIN_FOR_LEN[4] = {0, 0x80u, 0x800u, 0x10000u};
	if (cp < MIN_FOR_LEN[extra] || cp > 0x10ffffu || (cp >= 0xd800u && cp <= 0xdfffu)) {
		return false;
	}
	*out = cp;
	*used = (size_t)extra + 1u;
	return true;
}

/* The keysym that delivers one codepoint, or NoSymbol for a codepoint that
 * must not be delivered at all.
 *
 * `\n` and `\t` are the two the IDL pins by name. EVERY OTHER CONTROL
 * CHARACTER IS DROPPED, and the set dropped here is exactly the set the IDL
 * declares illegal. `vitrin_actuator_text.type`, whose payload becomes this
 * event: "Normative control-character rule: a newline (U+000A) MUST be
 * rendered as a Return keypress and a tab (U+0009) as Tab by the delivery
 * path [...] All other C0 and C1 control characters are fatal
 * 'invalid_argument': a correct client never emits them." So `cp < 0x20 ||
 * cp == 0x7f || 0x80 <= cp <= 0x9f` -- Unicode's Cc category, C0 plus DEL
 * plus C1 -- minus the two named above.
 *
 * The rejection is the core's to make (the agent-facing chokepoint owns
 * `invalid_argument`, and this interface has no error carrier at all), so
 * this is a backstop rather than the enforcement point. It is not an
 * optional one: the shim is the side that would carry out the damage, and
 * the check cannot be left to xkbcommon, which maps every one of these to a
 * real key. U+0008 is BackSpace, U+001B is Escape, U+007F is Delete, U+000B
 * is Clear -- all pre-bound in the warm region, so they would be delivered
 * without even costing a regeneration -- and the rest come back as
 * Unicode-offset keysyms (0x0100000e and friends) that render as the control
 * characters themselves. NOT ONE C0 codepoint comes back as NoSymbol.
 *
 * So without this, an agent pasting text scraped from a terminal, an LLM
 * response, or a clipboard buffer containing one stray 0x08 byte would erase
 * the user's existing text, and a stray 0x1B would dismiss the dialog it was
 * filling in. Dropping makes the string arrive short, which is visible and
 * counted (`codepoints_unmappable`, and the delivery traces as `partial`),
 * instead of arriving having done something nobody asked for, which is not.
 *
 * The `key` event remains the way to send a real Escape or BackSpace -- it
 * is the interface for "press this key", and it is the human path. */
static uint32_t keysym_for_codepoint(uint32_t cp) {
	if (cp == '\n') {
		return VITRIN_KEYSYM_RETURN;
	}
	if (cp == '\t') {
		return VITRIN_KEYSYM_TAB;
	}
	if (cp < 0x20u || cp == 0x7fu || (cp >= 0x80u && cp <= 0x9fu)) {
		return XKB_KEY_NoSymbol; /* C0, DEL, C1 */
	}
	return (uint32_t)xkb_utf32_to_keysym(cp);
}

static void replay_text(struct vitrin_seat_replay *r, struct vitrin_origin origin,
		const uint8_t *utf8, size_t len) {
	if (!keyboard_focused(r)) {
		/* Same reasoning as the key path, and the same placement: before the
		 * decode, so a string nobody can receive neither consumes dynamic
		 * keycodes nor pushes a keymap regeneration at an app that is not
		 * there. */
		r->dropped++;
		trace(r, origin, "text", false, "no-keyboard-focus", "bytes=%zu", len);
		return;
	}

	/* Decode once, into the seat's own buffer rather than the stack: the
	 * largest legal payload is 4096 bytes and this runs on the same stack
	 * that already carries the wire's 64 KiB reassembly buffer. */
	if (len > VITRIN_TEXT_MAX_BYTES) {
		/* Unreachable through the generated decoder, which enforces the
		 * IDL's `(max 4096 bytes)` bound before this function is called. It
		 * is checked anyway because the two bounds live in different files
		 * and only one of them is hand-written: raise the IDL's bound
		 * without raising VITRIN_TEXT_MAX_BYTES and the decode loop below
		 * would run off the end of `text_keysyms`. A clamp turns that from
		 * a buffer overflow into a truncated string and a log line. */
		wlr_log(WLR_ERROR,
			"text payload of %zu bytes exceeds the %u-byte bound; truncating",
			len, VITRIN_TEXT_MAX_BYTES);
		len = VITRIN_TEXT_MAX_BYTES;
	}
	size_t n = 0;
	size_t unmappable = 0;
	for (size_t i = 0; i < len;) {
		uint32_t cp = 0;
		size_t used = 1;
		if (!utf8_next(utf8 + i, len - i, &cp, &used)) {
			unmappable++;
			i += 1; /* resynchronise on the next byte */
			continue;
		}
		i += used;
		uint32_t keysym = keysym_for_codepoint(cp);
		if (keysym == XKB_KEY_NoSymbol) {
			/* Either a control character `keysym_for_codepoint` refuses to
			 * turn into a keystroke, or a codepoint xkbcommon has no keysym
			 * for at all. Dropping is the honest option in both cases: there
			 * is no keycode that could deliver it as text, and the keys that
			 * could be pressed instead do something else entirely. */
			wlr_log(WLR_DEBUG, "text: U+%04X is not deliverable as text; dropped", cp);
			unmappable++;
			continue;
		}
		r->text_keysyms[n++] = keysym;
	}
	r->codepoints_unmappable += unmappable;

	/* Chunked delivery. Each chunk binds every keysym it needs, uploads ONE
	 * keymap, then types -- so a keycode is never recycled while the string
	 * that used it is still being typed. In practice a chunk is the whole
	 * string: the split only happens past the dynamic region's capacity of
	 * distinct new codepoints.
	 *
	 * The slot each codepoint was bound to is REMEMBERED rather than looked
	 * up again at typing time. Looking it up again would have to search by
	 * keysym, and a keysym can legitimately sit at two keycodes at once --
	 * one the human is holding, one this chunk just minted for it -- so the
	 * search would have to re-derive which of the two it meant, from state
	 * that is changing as the loop types. Recording the answer once is both
	 * shorter and impossible to get wrong. */
	size_t i = 0;
	size_t typed = 0;
	unsigned chunks = 0;
	while (i < n) {
		uint64_t chunk_floor = r->bind_seq + 1u;
		size_t j = i;
		for (;;) {
			if (j >= n) {
				break;
			}
			int slot = slot_bind(r, r->text_keysyms[j], chunk_floor);
			if (slot < 0) {
				break;
			}
			r->text_slots[j] = (uint16_t)slot;
			j++;
		}
		if (j == i) {
			wlr_log(WLR_ERROR,
				"text: no keycode is available for keysym 0x%08x; "
				"%zu of %zu codepoints delivered",
				r->text_keysyms[i], typed, n);
			break;
		}
		if (!keymap_sync(r)) {
			break;
		}
		chunks++;
		for (size_t k = i; k < j; k++) {
			key_send(r, r->text_slots[k], true);
			key_send(r, r->text_slots[k], false);
			typed++;
		}
		i = j;
	}

	r->codepoints_delivered += typed;
	if (typed > 0) {
		r->delivered[VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE]++;
	} else {
		r->dropped++;
	}
	/* `ok` means the whole string was delivered. Codepoints refused as
	 * undeliverable count against that just as a chunking failure does --
	 * the app received fewer characters than the agent asked for, and a
	 * trace that called it `ok` because the ones it kept all landed would be
	 * reporting the shim's success rather than the request's. */
	trace(r, origin, "text", typed > 0, (typed == n && unmappable == 0) ? "ok" : "partial",
		"bytes=%zu codepoints=%zu delivered=%zu unmappable=%zu chunks=%u keymaps=%llu",
		len, n, typed, unmappable, chunks,
		(unsigned long long)r->keymap_generations);
}

/* ---- focus ------------------------------------------------------------ */

void vitrin_seat_focus_keyboard(struct vitrin_shim *s, struct wlr_surface *surface) {
	struct vitrin_seat_replay *r = &s->replay;
	if (s->seat == NULL || surface == NULL || r->keyboard == NULL) {
		return;
	}
	/* WHERE THE POLICY LIVES, AND WHAT IT NOW SAYS. This function is the
	 * mechanism -- WHICH surface gets the keyboard is xdg.c's decision, and
	 * the rule there is "the most recently mapped window that is still
	 * mapped" (`toplevel_map` / `toplevel_unmap`). What this comment used to
	 * claim -- "one realm, one window, so the app has the keyboard for as
	 * long as it has a window" -- was version 1's policy and stopped being
	 * true the first time an app opened a second toplevel: the survivor of a
	 * closed dialog got nothing back, because focus is only ever TAKEN at
	 * map (issue #268).
	 *
	 * Handing over the already pressed keycodes and the current modifier
	 * state is what makes a chord that began before the map survive it -- and
	 * it is also what makes the keyboard arrive at a successor mid-chord
	 * without the app seeing a phantom release.
	 *
	 * wlroots sends the previous holder its `wl_keyboard.leave` as part of
	 * this enter, so a caller moving focus between siblings must NOT unfocus
	 * first: that would put an interval with no holder on the wire, which is
	 * the state this whole path exists to avoid.
	 *
	 * `notify_enter`, NOT `wlr_seat_keyboard_enter`: the notify form "defers
	 * to any keyboard grabs" (wlr_seat.h) and the bare form does not. The
	 * bare one would let this shim yank the keyboard out from under an open
	 * menu, which is the app's own popup grab and none of the shim's
	 * business. The price is that this call can be a no-op for as long as a
	 * menu is open, so it is FIRE-AND-FORGET and callers must treat it that
	 * way: `vitrin_seat_keyboard_focus_is` is how they check, and xdg.c's
	 * `vitrin_xdg_refocus` is what re-asserts once the grab ends. */
	wlr_seat_keyboard_notify_enter(s->seat, surface,
		r->keyboard->keycodes, r->keyboard->num_keycodes, &r->keyboard->modifiers);
	wlr_log(WLR_DEBUG, "keyboard focus taken by the app surface");
}

void vitrin_seat_clear_keyboard(struct vitrin_shim *s) {
	if (s->seat == NULL) {
		return;
	}
	wlr_seat_keyboard_notify_clear_focus(s->seat);
}

void vitrin_seat_unfocus_keyboard(struct vitrin_shim *s, struct wlr_surface *surface) {
	if (vitrin_seat_keyboard_focus_is(s, surface)) {
		vitrin_seat_clear_keyboard(s);
	}
}

/* Whether `surface` is the one holding the keyboard right now.
 *
 * Exported so xdg.c can ask BEFORE it acts, which is what the two-window
 * unmap path needs (issue #268): a window that unmaps while a sibling holds
 * the keyboard must neither clear focus nor go looking for a successor, and
 * "did my call do anything?" is not something `vitrin_seat_unfocus_keyboard`
 * can answer after the fact. Reading `seat->keyboard_state` from xdg.c
 * instead would put wlroots' seat internals in a second file for no gain;
 * this keeps the seat's state the seat's business, and it is deliberately a
 * QUERY of wlroots' own field rather than a shim-side copy of focus, for the
 * reason the pointer-state comment in seat.h gives -- a second copy is a
 * second thing to invalidate, and the one that got it wrong would be
 * pointing at freed memory. */
bool vitrin_seat_keyboard_focus_is(struct vitrin_shim *s, struct wlr_surface *surface) {
	if (s->seat == NULL || surface == NULL) {
		return false;
	}
	return s->seat->keyboard_state.focused_surface == surface;
}

/* ---- the entry point -------------------------------------------------- */

/* THE replay entry point (seat.h, B2 argument 1): the origin tag is
 * constructed here, from a decoder-validated wire value, and immediately
 * handed to a replay helper as a mandatory by-value parameter. Every helper
 * above is static, so this is the only way input can enter the app. */
void vitrin_seat_handle_event(struct vitrin_shim *s, const uint8_t *frame, size_t len) {
	struct vitrin_seat_replay *r = &s->replay;
	vitrin_frame_header_t hdr;
	if (vitrin_frame_header_decode(frame, len, &hdr) != VITRIN_DECODE_OK) {
		wlr_log(WLR_ERROR, "malformed seat event header");
		return;
	}
	if (r->keyboard == NULL) {
		/* Replay is not wired (a bring-up failure, or --no-upstream's
		 * standalone mode reached from a test). Say so rather than
		 * pretending the event was delivered. */
		wlr_log(WLR_ERROR, "seat event (opcode %u) dropped: replay is not initialised",
			hdr.opcode);
		r->dropped++;
		return;
	}

	uint32_t object_id = 0;
	switch (hdr.opcode) {
	case VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE: {
		vitrin_shim_seat_evt_motion_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_motion_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed motion: %s", vitrin_decode_status_string(st));
			return;
		}
		replay_motion(r, vitrin_origin_from_wire(ev.origin),
			vitrin_fixed_to_double(ev.x), vitrin_fixed_to_double(ev.y));
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE: {
		vitrin_shim_seat_evt_button_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_button_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed button: %s", vitrin_decode_status_string(st));
			return;
		}
		replay_button(r, vitrin_origin_from_wire(ev.origin), ev.button, ev.state);
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE: {
		vitrin_shim_seat_evt_scroll_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_scroll_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed scroll: %s", vitrin_decode_status_string(st));
			return;
		}
		replay_scroll(r, vitrin_origin_from_wire(ev.origin), ev.axis, ev.value120);
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_KEY_OPCODE: {
		vitrin_shim_seat_evt_key_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_key_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed key: %s", vitrin_decode_status_string(st));
			return;
		}
		replay_key(r, vitrin_origin_from_wire(ev.origin), ev.keysym, ev.state);
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE: {
		vitrin_shim_seat_evt_text_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_text_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed text: %s", vitrin_decode_status_string(st));
			return;
		}
		replay_text(r, vitrin_origin_from_wire(ev.origin), ev.text.data, ev.text.len);
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE: {
		vitrin_shim_seat_evt_relative_motion_t ev;
		vitrin_decode_status_t st = vitrin_shim_seat_evt_relative_motion_decode(
			frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed relative_motion: %s",
				vitrin_decode_status_string(st));
			return;
		}
		replay_relative_motion(r, vitrin_origin_from_wire(ev.origin),
			vitrin_fixed_to_double(ev.dx), vitrin_fixed_to_double(ev.dy),
			vitrin_fixed_to_double(ev.dx_unaccel), vitrin_fixed_to_double(ev.dy_unaccel));
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE: {
		vitrin_shim_seat_evt_gesture_begin_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_gesture_begin_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed gesture_begin: %s",
				vitrin_decode_status_string(st));
			return;
		}
		replay_gesture_begin(r, vitrin_origin_from_wire(ev.origin), ev.kind, ev.fingers);
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE: {
		vitrin_shim_seat_evt_gesture_swipe_update_t ev;
		vitrin_decode_status_t st = vitrin_shim_seat_evt_gesture_swipe_update_decode(
			frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed gesture_swipe_update: %s",
				vitrin_decode_status_string(st));
			return;
		}
		replay_gesture_swipe_update(r, vitrin_origin_from_wire(ev.origin),
			vitrin_fixed_to_double(ev.dx), vitrin_fixed_to_double(ev.dy));
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE: {
		vitrin_shim_seat_evt_gesture_pinch_update_t ev;
		vitrin_decode_status_t st = vitrin_shim_seat_evt_gesture_pinch_update_decode(
			frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed gesture_pinch_update: %s",
				vitrin_decode_status_string(st));
			return;
		}
		replay_gesture_pinch_update(r, vitrin_origin_from_wire(ev.origin),
			vitrin_fixed_to_double(ev.dx), vitrin_fixed_to_double(ev.dy),
			vitrin_fixed_to_double(ev.scale), vitrin_fixed_to_double(ev.rotation));
		break;
	}
	case VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE: {
		vitrin_shim_seat_evt_gesture_end_t ev;
		vitrin_decode_status_t st =
			vitrin_shim_seat_evt_gesture_end_decode(frame, len, -1, &object_id, &ev);
		if (st != VITRIN_DECODE_OK) {
			wlr_log(WLR_ERROR, "malformed gesture_end: %s",
				vitrin_decode_status_string(st));
			return;
		}
		replay_gesture_end(r, vitrin_origin_from_wire(ev.origin), ev.kind, ev.state);
		break;
	}
	default:
		/* Version skew, not an attack -- the core is the TCB. Discard and
		 * keep serving, as the conventions' tolerate-unknown-events posture
		 * asks (the same choice upstream.c makes for unknown objects). */
		wlr_log(WLR_ERROR, "unknown seat event opcode %u; discarded", hdr.opcode);
		break;
	}
}

/* ---- bring-up and teardown -------------------------------------------- */

static void handle_modifiers(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_seat_replay *r = wl_container_of(listener, r, modifiers);
	/* The relay wlroots does not do for us: `wlr_seat_set_keyboard` forwards
	 * keymap and repeat-info changes to clients, but modifier state is the
	 * compositor's to pass on. */
	wlr_seat_keyboard_notify_modifiers(r->shim->seat, &r->keyboard->modifiers);
}

/* A keyboard grab ended -- an app's menu closed, in practice, since
 * `xdg_popup.grab` is the only thing in a version-1 realm that takes one.
 * Every focus change the shim made while it was live was swallowed
 * (`wlr_seat_keyboard_notify_enter` defers to grabs), so this is the first
 * moment the seat can be made to agree with window policy again. xdg.c owns
 * that policy and re-applies the whole of it; nothing is passed in, because
 * the answer is a property of which windows are mapped and not of which grab
 * just ended. See `vitrin_xdg_refocus` (issue #268).
 *
 * wlroots resets `keyboard_state.grab` to the default grab BEFORE emitting
 * this signal, so the enter issued from here is not swallowed by the grab
 * that is ending. */
static void handle_keyboard_grab_end(struct wl_listener *listener, void *data) {
	(void)data;
	struct vitrin_seat_replay *r = wl_container_of(listener, r, keyboard_grab_end);
	vitrin_xdg_refocus(r->shim);
}

static const struct wlr_keyboard_impl keyboard_impl = {
	.name = "vitrin-virtual-keyboard",
	/* No LEDs: there is no device to light up, and a shim that pretended
	 * otherwise would be reporting hardware state it cannot have. */
	.led_update = NULL,
};

bool vitrin_seat_init(struct vitrin_shim *s) {
	struct vitrin_seat_replay *r = &s->replay;
	r->shim = s;

	r->xkb = xkb_context_new(XKB_CONTEXT_NO_DEFAULT_INCLUDES);
	if (r->xkb == NULL) {
		wlr_log(WLR_ERROR, "cannot create an xkb context");
		return false;
	}

	/* Region 1: modifiers, at fixed slots for the life of the process, so a
	 * held chord survives every keymap regeneration. */
	r->mod_slots = (unsigned)MODIFIER_COUNT;
	for (unsigned i = 0; i < r->mod_slots; i++) {
		slot_pin(r, i, MODIFIER_KEYS[i].keysym);
	}

	/* Region 2: the warm set -- printable ASCII plus every layout-invariant
	 * key the core can send. Bound once, here, BEFORE the app can connect,
	 * so that the very first keymap it reads already covers the whole human
	 * key path and all-ASCII agent text. */
	unsigned slot = r->mod_slots;
	for (uint32_t cp = ASCII_FIRST; cp <= ASCII_LAST; cp++) {
		slot_pin(r, slot++, (uint32_t)xkb_utf32_to_keysym(cp));
	}
	for (unsigned i = 0; i < WARM_COUNT; i++) {
		if (slot_free_of(r, WARM_KEYSYMS[i]) >= 0) {
			continue; /* already covered by ASCII (space) */
		}
		slot_pin(r, slot++, WARM_KEYSYMS[i]);
	}
	r->warm_slots = slot;
	r->ring_next = 0;

	r->keyboard = calloc(1, sizeof(*r->keyboard));
	if (r->keyboard == NULL) {
		wlr_log(WLR_ERROR, "out of memory creating the virtual keyboard");
		return false;
	}
	wlr_keyboard_init(r->keyboard, &keyboard_impl, keyboard_impl.name);
	wlr_keyboard_set_repeat_info(r->keyboard, VITRIN_REPEAT_RATE_HZ, VITRIN_REPEAT_DELAY_MS);

	r->modifiers.notify = handle_modifiers;
	wl_signal_add(&r->keyboard->events.modifiers, &r->modifiers);
	r->modifiers_wired = true;

	/* Attached to the SEAT, not to the keyboard: the grab is seat state.
	 * Safe this early even though xdg.c's toplevel list is built two phases
	 * later -- `vitrin_xdg_refocus` returns immediately until it exists. */
	r->keyboard_grab_end.notify = handle_keyboard_grab_end;
	wl_signal_add(&s->seat->events.keyboard_grab_end, &r->keyboard_grab_end);
	r->keyboard_grab_end_wired = true;

	if (!keymap_sync(r)) {
		return false;
	}
	/* Only now: `wlr_seat_set_keyboard` pushes the keymap to every already
	 * bound client, and a keyboard with no keymap would push nothing. */
	wlr_seat_set_keyboard(s->seat, r->keyboard);

	wlr_log(WLR_INFO,
		"seat replay ready: %u modifier + %u warm keycodes, %u dynamic, keymap generation %llu",
		r->mod_slots, r->warm_slots - r->mod_slots,
		VITRIN_KEY_SLOTS - r->warm_slots, (unsigned long long)r->keymap_generations);
	return true;
}

void vitrin_seat_finish(struct vitrin_shim *s) {
	struct vitrin_seat_replay *r = &s->replay;
	if (r->keyboard != NULL) {
		wlr_log(WLR_INFO,
			"seat replay down: motion=%llu button=%llu scroll=%llu key=%llu text=%llu "
			"relative_motion=%llu gesture_begin=%llu gesture_swipe_update=%llu "
			"gesture_pinch_update=%llu gesture_end=%llu "
			"dropped=%llu keys=%llu codepoints=%llu unmappable=%llu keymaps=%llu",
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_KEY_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE],
			(unsigned long long)
				r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE],
			(unsigned long long)
				r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE],
			(unsigned long long)r->delivered[VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE],
			(unsigned long long)r->dropped, (unsigned long long)r->keys_synthesized,
			(unsigned long long)r->codepoints_delivered,
			(unsigned long long)r->codepoints_unmappable,
			(unsigned long long)r->keymap_generations);
	}
	/* A gesture the app was told had begun, with the shim now going down.
	 * Nothing can be sent -- the seat is about to be destroyed and the client
	 * with it -- so this is a log line and not a repair. It is worth one: a
	 * shim that exits mid-gesture is either the realm dying or a bug, and the
	 * core owes exactly one end per begin, so an unpaired one here is the
	 * single place that debt becomes visible from this side. */
	if (r->gesture_live) {
		wlr_log(WLR_INFO,
			"seat replay down with a %s gesture still in flight; the app was told it "
			"began and will not be told it ended",
			gesture_kind_name(r->gesture_kind));
		r->gesture_live = false;
	}
	/* Order matters: `wlr_keyboard_finish` asserts that nothing is still
	 * listening to the keyboard's signals, and it synthesizes a release for
	 * every key still held -- which is why the seat must be detached first,
	 * so those releases do not chase a seat that is being destroyed. */
	if (r->modifiers_wired) {
		wl_list_remove(&r->modifiers.link);
		r->modifiers_wired = false;
	}
	/* The seat is a display global and must have no listeners left on its
	 * signals when `wl_display_destroy` tears it down -- the same rule the
	 * xdg-shell listeners follow in server.c, and the same `_wired` flag
	 * pattern, because bring-up can fail before this one was attached. */
	if (r->keyboard_grab_end_wired) {
		wl_list_remove(&r->keyboard_grab_end.link);
		r->keyboard_grab_end_wired = false;
	}
	if (r->keyboard != NULL) {
		if (s->seat != NULL && wlr_seat_get_keyboard(s->seat) == r->keyboard) {
			wlr_seat_set_keyboard(s->seat, NULL);
		}
		wlr_keyboard_finish(r->keyboard);
		free(r->keyboard);
		r->keyboard = NULL;
	}
	if (r->xkb != NULL) {
		xkb_context_unref(r->xkb);
		r->xkb = NULL;
	}
}
