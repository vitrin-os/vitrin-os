// SPDX-License-Identifier: MPL-2.0
//! Input intake & routing v0 (P1.3.7, issue #24): where input *enters* the
//! trusted core, gets its origin tag, and is routed to the realm's shim
//! seat.
//!
//! In nested mode, the host compositor's keyboard/pointer events arriving
//! at the core's winit window **are the human principal's input** — the
//! human principal is implicit/built-in in the MVP, with no identity
//! ceremony (plan P1.3.7; `docs/protocol/11-vitrin_shim_seat.md` flow (i)).
//! Trusting nested host input as human is a documented limitation; real
//! physical-origin verification is Phase 3. Headless mode has **no human
//! device at all**, and in a shipping build that absence is compiler-enforced,
//! not a runtime check: [`SeatInput::physical`] is private to this module, so
//! the only producer of `Origin::Physical` events in the crate is
//! [`intake_physical`], whose only caller is the nested backend's winit
//! event handler — a call site anywhere else (the headless backend, a
//! P1.4.x actuation path, a replay helper) is a compile error, so no
//! phantom physical-input path can exist there.
//!
//! **One build weakens that sentence, and it is named rather than implied.**
//! A `physical-input-injector` build (issue #212, WS-E.1.6) adds a second
//! caller of [`intake_physical`]: [`injector`], fed by an inherited
//! socketpair the headless backend adopts only when `--physical-input-fd N`
//! is also passed. WS-E.1.6's whole claim is about where physical input goes,
//! and headless is the only backend CI runs (D-019(4)), so without it that
//! claim has no mock-free gate at all. The privacy above is unchanged — the
//! injector cannot construct a `SeatInput`, it can only hand host events to
//! the same intake the nested backend uses — but the *guarantee* is not: in a
//! shipping build "nothing but a human's device produces a physical tag here"
//! is enforced by the compiler, and in an instrumented build it is enforced
//! by the feature, the flag and possession of a descriptor. That is a weaker
//! guarantee than the one this paragraph otherwise claims, and it is the same
//! trade `dead-man-injector` and `consent-injector` already make.
//!
//! # B2: the origin tag is bound at intake, structurally
//!
//! Backward requirement B2 (plan §1) demands that every input event carry
//! its origin — physical device vs. agent actuator — from intake through
//! core → shim (`vitrin_shim_seat` carries it on the wire, per-event) →
//! app, the libei/EIS model (PRD Doc 2 §4.4). Here that is a property of
//! the type system, not a convention:
//!
//! - [`SeatInput`] (an event at intake) and [`SeatDelivery`] (an event
//!   ready for the wire) keep `origin` **private**, with no setter and no
//!   `Default`. The only ways to obtain a `SeatInput` are the two
//!   constructors [`SeatInput::physical`] (private to this module — only
//!   [`intake_physical`] and [`physical_key`] can mint the physical tag) and
//!   [`SeatInput::emulated`] (crate-visible for the P1.4.x actuation
//!   intake) — an untagged event is unrepresentable, and a
//!   physical-origin masquerade outside nested intake is a compile
//!   error.
//!   `SeatInput` is not the only producer of a wire-ready event, so the
//!   compile-time half of the guarantee needs the next bullet to be
//!   complete.
//! - A [`SeatDelivery`] is constructed at exactly three sites, all inside
//!   `InputRouter`, and **none invents an origin**. `InputRouter::route_into`
//!   (behind both addressing rules) *moves* the tag out of the `SeatInput` it
//!   consumed. [`InputRouter::release_physical_keys`] and
//!   [`InputRouter::release_physical_buttons`] — the drains, the only places a
//!   wire event exists with no intake event behind it — *copy* the tag off the
//!   pairing-table entry that press recorded ([`RealmSeat::pressed_keys`] and
//!   [`RealmSeat::pressed`] store `(code, origin)` for exactly this reason)
//!   and drain only the entries a human's press put there. The tag is
//!   therefore never re-derived downstream and never minted, so it cannot
//!   drift between intake and the wire.
//! - [`SeatDelivery::encode`] is the single seat-event encoder, an
//!   exhaustive match with no catch-all: every arm feeds the origin into
//!   the generated event type, whose `origin` field the IDL (and the RNG
//!   schema's last-arg-is-origin rule) makes mandatory. Adding a seat
//!   event kind without encoding its origin is a compile error.
//!
//! # Coordinate mapping (the settled mismatch-case decision)
//!
//! The wire's pointer coordinates are **realm-view pixels** (IDL
//! `vitrin_shim_seat`; decision D10), and the shim maps view → surface-
//! local for replay. The shim's realm view is the space `configure` told
//! it about — a view-sized output with its app maximized at the origin.
//! When the committed surface exactly fills the core's view (the intended
//! steady state of single-maximized), core-view coordinates and the shim's
//! view coordinates are the same space and the router's mapping is the
//! identity, matching the IDL's flows number for number.
//!
//! When the sizes mismatch (mid-resize, or a misbehaving shim), the core
//! letterboxes/center-crops the surface at 1:1 (`scene`, the P1.3.3
//! decision) — a **core-private presentation choice** the shim cannot know
//! about (the IDL deliberately leaves letterbox-vs-reconfigure open). The
//! router therefore compensates for it at routing time: host-window/view
//! coordinates are translated by the same deterministic
//! [`ViewGeometry::place`] placement the compositor painted with, so that
//! (0, 0) on the wire is always the top-left of the app content the shim
//! forwarded — the origin of the shim's own view space. Since issue #304
//! that placement starts below the rows the core reserves for the trusted
//! band and the status strip, so the translation carries the inset too and
//! a pointer over the band maps outside the app rather than into its first
//! row. A click on the
//! letterbox matte is *not* a click in the app: pointer events outside the
//! placed surface rectangle are dropped, except during an implicit grab
//! (below). With no committed surface there is nothing to point at, and
//! the pointer path simply yields nothing (keys and text still flow — the
//! shim holds keyboard focus on its app and owns that judgement).
//!
//! Implicit grab (**per realm**, like everything else in [`RealmSeat`]):
//! while any button the router delivered is still pressed,
//! all pointer events keep flowing regardless of position — coordinates
//! may leave [0, surface) (the wire's `fixed` is signed) — so drags that
//! stray off the surface never strand a stuck button in the app, mirroring
//! Wayland's implicit-grab semantics. A press that lands on the matte is
//! dropped and starts no grab; its release is likewise dropped (the razor:
//! a release is delivered iff its own press was). Pairing is tracked
//! **per button code**, Wayland-style, not as a bare count: a
//! matte-dropped `BTN_LEFT` press can never borrow the grab a delivered
//! `BTN_RIGHT` press holds, so the wire never sees a release for a button
//! the app never saw pressed, and no delivered press is ever left
//! stranded by someone else's release.
//!
//! **Keys pair the same way, per key.** Keys carry no geometry, so they
//! never route on position — but the razor above is not about geometry, it
//! is about the app's press/release accounting, and a keyboard has exactly
//! the same accounting. Without pairing, any policy that stops a key
//! *release* (P1.7.2's consent grab is the first, and [`Gate::Consume`]
//! promises to reconcile what it stops) would leave the app believing a
//! key is still held: a latched `Ctrl`/`Alt`/`Super` silently rewrites the
//! meaning of every keystroke the app receives afterwards, and a latched
//! letter autorepeats. So the router tracks the keys whose press it
//! delivered and drops any release that does not pair with one, exactly as
//! it does for button codes.
//!
//! **What a press and its release pair BY is now the key, not the keysym**
//! (WS-E.3.1, issue #217, decision D-028(3) — this paragraph is the
//! discharge of the warning that used to stand here). It was the keysym for
//! as long as every key in this core came from a fixed scancode→keysym table
//! or from a host that had already resolved it, so the same physical key
//! yielded the same keysym on press and on release by construction. A real
//! keymap ([`keymap::CoreKeymap`]) breaks that: press `a` with Shift held,
//! release it after Shift is up, and the release resolves to `a` where the
//! press resolved to `A`. Pairing by keysym would find nothing, drop the
//! release, and leave the app holding a key forever. So an event carries a
//! [`KeySource`] — the evdev scancode where a device is behind it, the
//! keysym where none is (an agent's actuation names a keysym and nothing
//! else) — and [`RealmSeat::pressed_keys`] pairs on that.
//!
//! **And the release delivers the PRESS's keysym**, which is the half that
//! is easy to stop one step short of. Pairing correctly inside the core is
//! worth nothing if the wire then carries `a` for a release whose press
//! carried `A`: the shim binds a dynamic keycode per keysym (`shim/src/
//! seat.c`), so the app would be told to release a keycode it never held
//! while the one it *is* holding stays down — the same latched key, one
//! layer further out, where the core can no longer see it. The pairing
//! entry therefore remembers the delivered keysym ([`HeldKey`]) and both the
//! ordinary release path and the drains send that one back. The drains
//! always did; the ordinary path had no reason to until now.
//!
//! # The preemption hook (defined now, consumed by P1.7.x)
//!
//! [`PreemptionHook`] is **the single point** in the router where policy
//! interposes between intake and delivery, placed *after* origin binding
//! and *before* coordinate mapping / hit-testing:
//!
//! - **after origin binding**, because preemption policy is origin policy:
//!   the consent grab captures *physical* input, and PRD Doc 2 §8's
//!   physical-preempts-agent arbitration (Phase 2+, multi-agent) needs
//!   both origins visible as data at one point;
//! - **before mapping/hit-testing**, because the consent surface (P1.7.1)
//!   is core-rendered in *view* space: a grab must see view coordinates to
//!   hit-test its own buttons, and must capture clicks on the matte too —
//!   a gate that ran after the app hit-test could be dodged by parking the
//!   pointer off the surface.
//!
//! Two attachment shapes, one hook point, called in a fixed order for
//! every event:
//!
//! 1. [`PreemptionHook::observe`] — an unconditional, non-consuming tap.
//!    P1.7.3's hold-chord revocation watcher **detects** here: it must see
//!    raw physical events (Escape press/release — covered by the
//!    layout-invariant key table below) *even while a consent grab is
//!    consuming all delivery*.
//! 2. [`PreemptionHook::gate`] — the consuming gate. P1.7.2's consent
//!    input grab attaches here: returning [`Gate::Consume`] stops the
//!    event before mapping and before the wire. A consumed press starts
//!    no implicit grab; a consumed **release** of a delivered press still
//!    clears that press's grab bookkeeping — reconciled without delivery
//!    — so a consuming gate can never wedge the router's grab (see
//!    [`Gate::Consume`] and the pairing contract on [`PreemptionHook`]).
//!
//! Both sides are now taken, and the nested backend's router is
//! [`crate::backend::winit::NestedHook`] —
//! `InputRouter<LockGate<ConsentGate<DeadManHook<ClipboardHook<ScreenshotHook<BacklightHook<AttentionHook<NoopHook>>>>>>>>`,
//! the stacking this hook point was designed for, reached without restructuring
//! anything. That alias is the **one** place the order is written down, because
//! the order is the decision and a decision written twice drifts. A consent
//! prompt consumes an event before the dead-man's *gate* half sees it, both are
//! outside the clipboard, screenshot, brightness and attention keys, and a
//! raised lock is outside every one of them — so the human's off-switch and the
//! human's security question each
//! short-circuit a mechanism whose worst failure is a convenience that does not
//! happen, and none of them can be answered by somebody who is not at the
//! keyboard.
//!
//! [`crate::consent::grab::ConsentGate`] (P1.7.2) is the gate. Its policy
//! lives entirely in that module; what matters here is that it honours the
//! pairing contract below by never consuming a release, and that it judges by
//! the origin tag intake bound rather than by any authority question — the
//! router still holds no authority check, and must not grow one.
//!
//! [`crate::deadman::DeadManHook`] (P1.7.3) sits between them, and it
//! implements **both** halves — which is the resolution of a real tension rather than an
//! oversight, argued in full in [`crate::deadman`]. The two halves do two
//! different jobs: `observe` detects the chord and owns every piece of switch
//! state, so no other policy can stop the human's off-switch from firing;
//! `gate` only decides what the confined app sees, withholding the chord key's
//! press until a tap can be told from a hold. It therefore *does* consume a
//! release — one of the three places in this core that do, the others being the
//! attention hook below and the brightness gate beside it — and that is sound only because it consumed that
//! release's press too, so the pair is atomic and the app's accounting is never
//! split. The router's per-keysym pairing is what
//! proves it: the reconciliation finds no delivered press and does nothing.
//!
//! [`crate::lock::LockGate`] (WS-E.2.2, issue #214) is **outermost**, and it is
//! the one gate in this stack that consumes *all* physical input rather than
//! one key's worth. It is expressed through [`ConsumingGate`] rather than
//! [`PreemptionHook`] precisely so it cannot reach [`observe`] — see that
//! trait for the whole argument, which is that the human's off-switch detects
//! in a tap nothing may be allowed to blind, and the sharpest possible way for
//! a lock to be wrong is to blind it.
//!
//! [`crate::backlight::BacklightHook`] (D-041, issue #303) sits between the
//! screenshot key and the attention hook, and it is `gate`-only too. It
//! consumes both halves of `XF86MonBrightnessUp`/`Down` — the same sound
//! exception, for a third core-owned key — but **only on a session that passed
//! `--backlight`**; without the flag it decides nothing and both keys reach the
//! app exactly as WS-E.4.3 left them. Its position is deliberately
//! unobservable: neither keysym is a modifier or a member of any chord's
//! vocabulary, so nothing above or below it can have its matcher state changed
//! by what it takes.
//!
//! [`crate::attention::AttentionHook`] (WS-E.1.7, issue #232) is innermost, and
//! it implements `gate` **only**. It consumes the human's attention chord —
//! both halves of the pair, taking the same sound exception for the same
//! reason — and records a press the embedder turns into a short exemption
//! window for the two layout verbs. Being innermost is what makes it
//! *suppressible*: a raised prompt or a dead-man chord press never reaches it.
//! That is the opposite posture from the dead-man's `observe`, deliberately —
//! see [`crate::attention`]'s "neighbour, never sibling" table for why the two
//! core-owned chords are kept apart at every level.
//!
//! The physical-activity state behind the enforcement chokepoint's
//! `preempted` refusal ([`PhysicalPresenceMap`]) rides the same single tap
//! point, but it is **not** a stackable hook: the router records it itself,
//! above the stack, because it is the one per-realm fact here and no policy
//! hook may be told a realm ([`PreemptionHook`], [`InputRouter`]). It was a
//! hook from P1.4.4 until issue #212's review, and in that whole time no
//! shipping backend stacked it — so `preempted` could not fire in any
//! `vitrind` ever built.
//!
//! # What arrives later (deliberately not here)
//!
//! - **Agent actuation intake (P1.4.x):** `vitrin_actuator_pointer` /
//!   `vitrin_actuator_text` requests pass the enforcement chokepoint
//!   (P1.4.4 — grant, verbs, constraints, and the token-bucket rate limit
//!   of PRD Doc 2 §8) *before* being wrapped by [`SeatInput::emulated`]
//!   and routed here, **naming the realm the grant is over**
//!   ([`InputRouter::route_emulated`]). The router is not an authority check
//!   and must never grow one: by the time an event reaches it, the authority
//!   question is settled (prose page 11), and naming the realm at the entry
//!   point is addressing, not authorization. Within a realm one pointer state
//!   still serves both origins in v0 (one seat, one cursor per realm);
//!   multi-*principal* routing is the Phase-2+ generalization.
//!
//! # Two addressing rules (WS-E.1.6, issue #212)
//!
//! A session holds up to [`crate::realm::MAX_REALMS`] realms and every one of
//! them may be receiving input at once. Which realm an event reaches is
//! answered by two different rules, and [`InputRouter`] carries both without
//! either becoming an authority check:
//!
//! - **physical input follows the human's attention** — the bound realm,
//!   which a `layout_focus` holder moves ([`InputRouter::route_physical`]);
//! - **an agent's actuation follows its grant** — the realm named by the
//!   grant it was admitted under, watched or not
//!   ([`InputRouter::route_emulated`]).
//!
//! The full argument, including what stays session-wide and what a bind
//! change costs the app it leaves, is on [`InputRouter`] itself.
//! - **Keyboard interpretation is a per-backend property, not a core
//!   property** — and saying otherwise was the substance of what WS-E.3.1
//!   (issue #217, decision D-028) had to correct in the IDL. Keys travel as
//!   xkbcommon keysyms in every configuration, and the wire is unchanged.
//!   *Where the keysym came from* differs: nested mode gets it from winit's
//!   already-interpreted `logical_key` ([`host_keysym`], issue #118) and the
//!   core interprets nothing; headless has no keyboard; and a bare-metal
//!   backend gets an evdev scancode from libinput and nothing else, so the
//!   core resolves it itself through [`keymap::CoreKeymap`]. See
//!   [`invariant_keysym`] for the fixed subset every backend can translate
//!   with no keymap at all, which is the one the core's own chords are
//!   drawn from.

use smithay::backend::input as host;
use smithay::backend::input::{
    AbsolutePositionEvent, GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent,
    GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};
use vitrin_protocol::generated::vitrin_actuator_pointer::{Axis, ButtonState};
use vitrin_protocol::generated::vitrin_shim_seat as seat;
use vitrin_protocol::generated::vitrin_shim_seat::{GestureKind, GestureState, KeyState, Origin};
use vitrin_protocol::Fixed;

use crate::view::ViewGeometry;

/// **Pointer constraints** (WS-E.4.2, issue #222): the record a shim's
/// `pointer_constraint` ask makes, the derived state its `pointer_constraint_state`
/// reports, and the one predicate that decides whether the core hides its own
/// human cursor sprite.
///
/// A submodule of this one rather than a sibling of it, because the constraint
/// table lives inside [`InputRouter`] and is reachable only through
/// [`InputRouter::constraints`] — the same discipline [`InputRouter::presence`]
/// follows, and for the same reason: a backend composing against a table its
/// own router does not write would be a sprite decided from stale state.
pub(crate) mod constraint;

#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        unused_imports,
        reason = "`hides_human_sprite`'s one production caller is the bare-metal composite, \
                  which is behind `drm-backend`"
    )
)]
pub(crate) use constraint::{
    hides_human_sprite, OutputGates, PointerConstraintAsk, PointerConstraintTable,
    PresentationGates, Region as ConstraintRegion, Verdict as ConstraintVerdict,
};

/// xkb keycodes are evdev scancodes offset by 8 (the historical X11
/// convention); Smithay's `KeyboardKeyEvent::key_code` is in the xkb
/// domain, the kernel's `KEY_*` constants are not.
pub(crate) const XKB_KEYCODE_OFFSET: u32 = 8;

/// The `keysymdef.h` convention for a keysym that is not its own codepoint:
/// codepoints below `0x100` are their own keysym, everything else is
/// `0x0100_0000 | codepoint`.
///
/// One definition for the whole core, because three places now depend on the
/// same convention and a convention spelled three times drifts:
/// [`host_keysym`] encodes nested input with it, [`keymap::CoreKeymap`]
/// normalises a real keymap's legacy keysyms into it (D-028(1)), and
/// [`crate::lock::gate`]'s `printable` decodes the lock passphrase with it.
pub(crate) const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;

/// Continuous (pixel-delta) scroll converted to wire `value120`: one wheel
/// notch = 120 = 15 pixels, the conventional libinput/toolkit equivalence.
/// Any fixed choice works; this one keeps a three-notch wheel and a 45 px
/// touchpad fling the same size in the app.
const V120_PER_SCROLL_PIXEL: f64 = 120.0 / 15.0;

/// What is behind a key event — the question "what does this release pay
/// down?" (D-028(3), issue #217).
///
/// Until the core grew a keymap, the answer was always "the keysym", and
/// that was sound because [`invariant_keysym`] is a fixed table with no
/// modifier resolution: the same physical key produced the same keysym on
/// press and on release, by construction. A real keymap breaks it — press
/// `a` with Shift down, release it after Shift is up, and the release
/// resolves to `a` where the press resolved to `A` — so the identity has to
/// come from the device instead. This enum is that identity, and it is a
/// type rather than an `Option<u32>` so that "which of the two keys does
/// this event pair by" has one answer per event and no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeySource {
    /// A real key on a real keyboard, named by its **evdev** scancode (the
    /// kernel's `KEY_*` domain, xkb's minus [`XKB_KEYCODE_OFFSET`]). Pairs
    /// by that number, so a press and its release pair however the modifier
    /// state moved in between.
    Device(u32),
    /// No device behind it: an agent's chokepoint-admitted key actuation,
    /// or a core-synthesised one. Pairs by keysym, which is the only
    /// identity such an event has — and it is sound for the same reason it
    /// used to be sound for everything: an actuator names a keysym directly,
    /// so it names the same one on the way back up.
    ///
    /// Unconstructed outside tests today, and carrying the same
    /// `allow(dead_code)` [`SeatInput::emulated`] carries for the same
    /// reason: v0's agent actuation path is `vitrin_shim_seat.text`, not
    /// `key` (the IDL says so in as many words), so every key event a
    /// shipping build produces has a real key behind it. The variant is here
    /// because [`RealmSeat::pressed_keys`]'s whole mixed-origin argument is
    /// about the day that stops being true, and because the alternative — a
    /// bare `u32` scancode — would force an agent's keysym-only actuation to
    /// invent one.
    #[cfg_attr(not(test), allow(dead_code))]
    Keysym,
}

impl KeySource {
    /// The identity this event's press and release pair by.
    fn pairing(self, keysym: u32) -> KeyPairing {
        match self {
            Self::Device(scancode) => KeyPairing::Scancode(scancode),
            Self::Keysym => KeyPairing::Keysym(keysym),
        }
    }
}

/// A key's pairing identity, as stored in [`RealmSeat::pressed_keys`].
///
/// Two variants rather than one `u32` because the two spaces genuinely do
/// not overlap and must never be compared: evdev 30 is `KEY_A` and keysym 30
/// is nothing at all, so a `Scancode(30)` press must not be paid down by a
/// `Keysym(30)` release. Deriving it from [`KeySource`] rather than storing
/// it at construction is what keeps the two consistent — there is no way to
/// hand-build an entry whose pairing disagrees with its event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyPairing {
    Scancode(u32),
    Keysym(u32),
}

/// One delivered-and-unreleased key press ([`RealmSeat::pressed_keys`]).
///
/// Three fields where there used to be two, and the third is the correction
/// D-028(3) forces: the entry remembers **the keysym the app was actually
/// told about**, so both the ordinary release path and the drains can send
/// that one back. Sending the *release's own* keysym would hand the shim a
/// keysym it never bound a keycode for, and the app would keep the press's
/// keycode held forever — the same latched-modifier failure the pairing
/// table exists to prevent, moved one layer down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeldKey {
    pairing: KeyPairing,
    /// The keysym delivered with the press.
    keysym: u32,
    origin: Origin,
}

/// What one input event *does*, in realm-view coordinates, with no origin
/// attached — the payload half of [`SeatInput`]. Only ever travels wrapped
/// in a [`SeatInput`], whose constructors demand the origin.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeatInputKind {
    /// Absolute pointer position in view pixels (nested mode: host-window
    /// physical pixels — the window presents the composed view 1:1).
    Motion { x: f64, y: f64 },
    /// Pointer button, Linux evdev button code.
    Button { button: u32, state: ButtonState },
    /// High-resolution scroll; one wheel notch = ±120.
    Scroll { axis: Axis, value120: i32 },
    /// A key as an xkbcommon keysym, already modifier-resolved — plus what
    /// the router pairs its press and release by ([`KeySource`]). The keysym
    /// is what goes on the wire; the source is what makes the release find
    /// its own press once a keymap can resolve the two differently.
    Key {
        source: KeySource,
        keysym: u32,
        state: KeyState,
    },
    /// A Unicode string (the agent text-actuation path in v0; human
    /// input-method text becomes its physical twin in a later phase).
    /// Constructed by the enforcement chokepoint's actuation intake
    /// (P1.4.4, [`crate::enforcement`]) — physical intake never produces
    /// it, and that asymmetry is the point: this variant is the wire's
    /// text-actuation verb, so anything holding one is acting on an agent's
    /// authority and must have passed the chokepoint to get it.
    ///
    /// Runtime-reachable since the M1.1 wiring: an agent's `actuate.text`
    /// reaches `session::route_seat` and the shim's virtual seat.
    Text { text: String },

    /// Pointer motion as a **delta**, in view pixels — the class every
    /// pointing device on a bare-metal seat actually produces (WS-E.4.2,
    /// issue #222).
    ///
    /// It **accompanies** [`Self::Motion`] rather than replacing it (IDL
    /// `vitrin_shim_seat.relative_motion`): one physical movement mints both,
    /// because an app binds whichever of the two it understands. The absolute
    /// twin is minted by the backend that owns the accumulated position
    /// ([`crate::backend::drm`]'s `accumulate_pointer`); this half is what
    /// [`intake_physical`] translates, since a delta needs no position to
    /// interpret.
    ///
    /// Both deltas travel because `zwp_relative_pointer_v1` defines both and
    /// libinput supplies both: `dx`/`dy` are the movement after pointer
    /// acceleration (it agrees with where the pointer visibly went),
    /// `dx_unaccel`/`dy_unaccel` the same movement before it (what a camera
    /// control or a drawing tool wants). Carrying one alone would force the
    /// shim to copy it into a field it would then be lying about.
    RelativeMotion {
        dx: f64,
        dy: f64,
        dx_unaccel: f64,
        dy_unaccel: f64,
    },
    /// A multi-finger touchpad gesture began. Swipe and pinch share this
    /// event and share [`Self::GestureEnd`], because their begin and end
    /// signatures are identical in the pointer-gesture vocabulary these
    /// serve; `kind` says which.
    ///
    /// `fingers` is fixed for the gesture's life — a gesture that gains or
    /// loses a finger ends and a new one begins, which is libinput's own
    /// behaviour and the IDL's rule.
    GestureBegin { kind: GestureKind, fingers: u32 },
    /// How far the finger group moved since this gesture's previous event,
    /// in view pixels. A delta, never a position and never cumulative.
    GestureSwipeUpdate { dx: f64, dy: f64 },
    /// A pinch's motion. **The four quantities do not share a reference
    /// point** and the IDL spells the asymmetry out because reading one as
    /// another is undetectable app-side: `dx`/`dy` and `rotation` (degrees,
    /// positive clockwise) are deltas since this gesture's previous event,
    /// while `scale` is absolute — relative to this gesture's own begin,
    /// 1.0 at the begin. Multiplying successive `scale`s into a running
    /// product zooms toward zero.
    GesturePinchUpdate {
        dx: f64,
        dy: f64,
        scale: f64,
        rotation: f64,
    },
    /// The gesture in flight ended. `state` distinguishes the two ways it
    /// can stop, and that is what lets an app commit rather than guess: a
    /// completed pinch keeps its zoom, a cancelled one puts it back.
    ///
    /// `kind` is what *intake* believes ended. The router answers with the
    /// kind **it** has in flight (see [`RealmSeat::gesture`]), so a
    /// disagreement is a logged, detectable bug rather than a silent
    /// mis-replay.
    GestureEnd {
        kind: GestureKind,
        state: GestureState,
    },
}

/// One input event at intake: origin bound at construction (B2), view
/// coordinates. `origin` is private and there is no setter — an untagged
/// event cannot be expressed, and nothing downstream can re-tag one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeatInput {
    origin: Origin,
    kind: SeatInputKind,
}

impl SeatInput {
    /// Tag input from a physical human device. Private to this module by
    /// design: the only producer of physical-tagged *intake* in the crate
    /// is [`intake_physical`] (nested mode's single point of entry), so a
    /// physical-origin masquerade anywhere else — the headless backend, a
    /// P1.4.x actuation path, a replay helper — is a compile error, not a
    /// convention. Headless mode has no physical source, structurally.
    ///
    /// One physical-tagged *delivery* does not pass through here:
    /// [`InputRouter::release_physical_keys`] pays down presses this router
    /// already delivered, so it reads each tag back off its own pairing
    /// table rather than minting one. That is inside the same module and
    /// under the same privacy, and it can only ever emit a tag a real intake
    /// event recorded — see the module docs' B2 section.
    fn physical(kind: SeatInputKind) -> Self {
        Self {
            origin: Origin::Physical,
            kind,
        }
    }

    /// Tag input from a principal's actuator. The enforcement chokepoint
    /// (P1.4.4, [`crate::enforcement`]) wraps chokepoint-**admitted**
    /// requests — and only those — with this constructor; the single-path
    /// test there pins that no other non-test site mints emulated events.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn emulated(kind: SeatInputKind) -> Self {
        Self {
            origin: Origin::Emulated,
            kind,
        }
    }

    /// Who caused this event. Readable everywhere, writable nowhere.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// The event payload (hook implementations inspect it; they cannot
    /// alter it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn kind(&self) -> &SeatInputKind {
        &self.kind
    }
}

/// A routed seat event, wire-ready: pointer coordinates already mapped to
/// the shim's view space (surface top-left = origin) and widened to the
/// wire's 24.8 fixed-point. Constructed only inside [`InputRouter`] — by the
/// shared routing body behind [`InputRouter::route_physical`] and
/// [`InputRouter::route_emulated`], which moves the origin from the
/// [`SeatInput`] it consumed, and by the two drains, which copy it off the
/// pairing-table entry that press recorded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeatDelivery {
    origin: Origin,
    kind: SeatDeliveryKind,
}

/// The wire-shaped payload of a [`SeatDelivery`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeatDeliveryKind {
    Motion {
        x: Fixed,
        y: Fixed,
    },
    Button {
        button: u32,
        state: ButtonState,
    },
    Scroll {
        axis: Axis,
        value120: i32,
    },
    Key {
        keysym: u32,
        state: KeyState,
    },
    Text {
        text: String,
    },
    /// The wire twin of [`SeatInputKind::RelativeMotion`], widened to 24.8
    /// fixed-point for [`Self::Motion`]'s sub-pixel reason.
    RelativeMotion {
        dx: Fixed,
        dy: Fixed,
        dx_unaccel: Fixed,
        dy_unaccel: Fixed,
    },
    GestureBegin {
        kind: GestureKind,
        fingers: u32,
    },
    GestureSwipeUpdate {
        dx: Fixed,
        dy: Fixed,
    },
    GesturePinchUpdate {
        dx: Fixed,
        dy: Fixed,
        scale: Fixed,
        rotation: Fixed,
    },
    GestureEnd {
        kind: GestureKind,
        state: GestureState,
    },
}

impl SeatDelivery {
    /// Who caused this event (the tag bound at intake, moved — never
    /// recomputed — through routing).
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// The wire-shaped payload.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn kind(&self) -> &SeatDeliveryKind {
        &self.kind
    }

    /// The seat event kind as the stable label the flight recorder writes
    /// ([`crate::recorder::Event::SeatDelivered`]): the IDL's own event name,
    /// one per [`SeatDeliveryKind`] variant. Shape only, deliberately no
    /// payload -- an audit entry carrying keysyms or typed bytes would be a
    /// keylogger, and one carrying gesture deltas would be a coarse one.
    ///
    /// Exhaustive with no catch-all, deliberately: a new delivery kind that
    /// forgot its label would otherwise be journaled as an existing one, and
    /// the recorder is the only record of what the core sent.
    pub fn event_label(&self) -> &'static str {
        match &self.kind {
            SeatDeliveryKind::Motion { .. } => "motion",
            SeatDeliveryKind::Button { .. } => "button",
            SeatDeliveryKind::Scroll { .. } => "scroll",
            SeatDeliveryKind::Key { .. } => "key",
            SeatDeliveryKind::Text { .. } => "text",
            SeatDeliveryKind::RelativeMotion { .. } => "relative_motion",
            SeatDeliveryKind::GestureBegin { .. } => "gesture_begin",
            SeatDeliveryKind::GestureSwipeUpdate { .. } => "gesture_swipe_update",
            SeatDeliveryKind::GesturePinchUpdate { .. } => "gesture_pinch_update",
            SeatDeliveryKind::GestureEnd { .. } => "gesture_end",
        }
    }

    /// The origin tag as the recorder's stable label: `physical` or
    /// `emulated`. The tag is read straight off the delivery, never
    /// recomputed (B2); the wire encoding keeps [`Origin`]'s own numbering
    /// ([`Self::encode`]).
    pub fn origin_label(&self) -> &'static str {
        origin_label(self.origin)
    }

    /// Encode as the corresponding `vitrin_shim_seat` event toward
    /// `seat_object_id`.
    ///
    /// This is the **only** seat-event encoder in the core, and the match
    /// is exhaustive with no catch-all: every arm passes `self.origin`
    /// into the generated event struct, whose mandatory `origin` field the
    /// IDL pins as the final argument of every seat event (B2's schema
    /// half). No untagged wire path exists.
    pub fn encode(&self, seat_object_id: u32) -> Vec<u8> {
        match &self.kind {
            SeatDeliveryKind::Motion { x, y } => seat::events::Motion {
                x: *x,
                y: *y,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::Button { button, state } => seat::events::Button {
                button: *button,
                state: *state,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::Scroll { axis, value120 } => seat::events::Scroll {
                axis: *axis,
                value120: *value120,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::Key { keysym, state } => seat::events::Key {
                keysym: *keysym,
                state: *state,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::Text { text } => seat::events::Text {
                text: text.clone(),
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::RelativeMotion {
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
            } => seat::events::RelativeMotion {
                dx: *dx,
                dy: *dy,
                dx_unaccel: *dx_unaccel,
                dy_unaccel: *dy_unaccel,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::GestureBegin { kind, fingers } => seat::events::GestureBegin {
                kind: *kind,
                fingers: *fingers,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::GestureSwipeUpdate { dx, dy } => seat::events::GestureSwipeUpdate {
                dx: *dx,
                dy: *dy,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::GesturePinchUpdate {
                dx,
                dy,
                scale,
                rotation,
            } => seat::events::GesturePinchUpdate {
                dx: *dx,
                dy: *dy,
                scale: *scale,
                rotation: *rotation,
                origin: self.origin,
            }
            .encode(seat_object_id),
            SeatDeliveryKind::GestureEnd { kind, state } => seat::events::GestureEnd {
                kind: *kind,
                state: *state,
                origin: self.origin,
            }
            .encode(seat_object_id),
        }
    }
}

/// Project an [`Origin`] to the flight recorder's stable audit label. The one
/// place the enum is mapped to the recorder vocabulary (`physical`/`emulated`),
/// kept exhaustive with no catch-all so a future origin cannot be logged as an
/// old one by default.
pub(crate) fn origin_label(origin: Origin) -> &'static str {
    match origin {
        Origin::Physical => "physical",
        Origin::Emulated => "emulated",
    }
}

/// Journal one delivered seat event with its origin and **the realm whose app
/// received it** (issue #83).
///
/// The **single funnel** both delivery paths call — `session::route_seat` for
/// agent actuations and the nested backend's physical input — so the two
/// cannot record the B2 audit differently, and one test (in this module)
/// covers the code both run.
///
/// `realm` is a parameter rather than something this function derives: both
/// call sites pick the delivery target themselves — the physical path from
/// `session::physical_seat_target`, an agent's from the grant row its use was
/// admitted under — and the journal has to record the realm that was actually
/// addressed, not one this funnel re-derived and could disagree about.
///
/// Pointer **motion is deliberately not journaled**. On the physical path it
/// arrives at raw device rate with no chokepoint token bucket to bound it (an
/// agent's actuations are rate-limited before they reach here; a human's are
/// not), and a delivery's coordinates are never recorded anyway — only its kind
/// and origin — so a per-event motion line would flood the recorder against its
/// own bounded-write discipline while adding no auditable fact the surrounding
/// button/scroll/key/text lines do not. The physical-vs-emulated distinction
/// B2 exists for is carried in full by those discrete events.
///
/// **`relative_motion` and the two gesture updates join it** (WS-E.4.2), for
/// exactly that reason and no other: all three arrive at raw device rate, and
/// `relative_motion` arrives *beside* the very motion already excluded, so
/// journaling it would flood the recorder at double the rate the argument
/// above rejects. `gesture_begin` and `gesture_end` are journaled — they are
/// discrete, bounded by the human's fingers, and they are the pair whose
/// balance an audit can actually check.
pub(crate) fn record_seat_delivery(
    recorder: &mut crate::recorder::Recorder,
    realm: &crate::grants::RealmId,
    delivery: &SeatDelivery,
) {
    if matches!(
        delivery.kind(),
        SeatDeliveryKind::Motion { .. }
            | SeatDeliveryKind::RelativeMotion { .. }
            | SeatDeliveryKind::GestureSwipeUpdate { .. }
            | SeatDeliveryKind::GesturePinchUpdate { .. }
    ) {
        return;
    }
    recorder.record(crate::recorder::Event::SeatDelivered {
        realm,
        event: delivery.event_label(),
        origin: delivery.origin_label(),
    });
}

/// Verdict of [`PreemptionHook::gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Route and deliver normally.
    Deliver,
    /// Consume: the event stops here — no mapping, no wire. A consumed
    /// press starts no implicit grab. A consumed **release** whose press
    /// the router delivered still clears that press's bookkeeping
    /// (delivered-state reconciliation, nothing on the wire) — for button
    /// codes *and* keysyms alike: the router's grab can never wedge on a
    /// consuming gate, but the app is left holding the press — the gate
    /// implementor's debt. A grab that wants clean app-side pairing simply
    /// never consumes releases of either kind: the router's per-button-code
    /// and per-keysym pairing already drops unpaired ones, so answering
    /// [`Gate::Deliver`] for a release can never leak input the app should
    /// not see.
    ///
    /// **A consumed [`SeatInputKind::GestureEnd`] is reconciled the same
    /// way** (WS-E.4.2): the router forgets the gesture it had in flight,
    /// nothing goes on the wire, and the app is left accumulating the
    /// gesture — the latched-modifier failure in a new shape, and the gate
    /// implementor's debt exactly as a consumed release is. It is stated
    /// here rather than left implicit because a gesture has no second chance:
    /// once the router has forgotten it, no later drain can pay it down. Do
    /// not consume a `gesture_end` whose begin the router delivered; see the
    /// pairing contract on [`PreemptionHook`].
    Consume,
}

/// The single preemption hook point of the input router (module docs lay
/// out the placement rationale). For every event, [`observe`] is called
/// first and unconditionally, then [`gate`] may consume it.
///
/// A hook may implement both halves, and P1.7.3's dead-man watcher does:
/// `observe` is where it detects the chord (unconditional, so no other
/// policy can blind the human's off-switch) and `gate` is where it keeps
/// that chord from reaching the confined app. What `observe` can never do
/// is *stop* an event — that is the gate's job, and the split is the point.
///
/// **Pairing contract for gate implementors.** A gate that begins
/// consuming while router-delivered presses are outstanding (a consent
/// grab seizing input mid-drag, or mid-keystroke) should keep answering
/// [`Gate::Deliver`] for *every* release — pointer button and key alike —
/// hold-until-release. That is always safe: the router's per-button-code
/// and per-keysym pairing drops any release whose press was not
/// delivered, so a blanket-delivered release can never leak input the app
/// should not see, and the app's press/release accounting stays intact. A
/// gate that consumes such a release instead does not wedge the router
/// (the bookkeeping is reconciled; see [`Gate::Consume`]) but strands the
/// press app-side — its debt to settle, and on the keyboard that debt is
/// a latched modifier that changes the meaning of everything typed after.
///
/// **The one sound exception**, and the only one in this core: a gate that
/// consumed a press may consume that press's release, because then nothing
/// is stranded — the app never saw the press begin, so it is not left
/// holding anything. Exactly two gates take it, each for its own core-owned
/// chord key and nothing else: [`crate::deadman`] for the dead-man chord,
/// [`crate::attention`] for the attention chord (which consumes *both* halves
/// unconditionally, so the pair is atomic by construction rather than by a
/// classification), and [`crate::backlight`] for the two `XF86MonBrightness*`
/// keys on a session that passed `--backlight`, which takes the exception the
/// same way the attention hook does and for the same reason. The rule above is
/// otherwise absolute: a gate must not consume a release whose press the
/// *router* delivered.
///
/// **[`SeatInputKind::GestureEnd`] is a release for this purpose**
/// (WS-E.4.2, issue #222), and the rule is word for word the same. A gate
/// that begins consuming mid-gesture — a consent prompt raised while a human
/// is pinching, a lock — should keep answering [`Gate::Deliver`] for
/// `gesture_end`, for the reason it already gives for releases: the router
/// delivers an end **iff** it delivered that gesture's own begin, so a
/// blanket-delivered end can never leak input the app should not see. A gate
/// that consumes one instead does not wedge the router (the in-flight record
/// is cleared; see [`Gate::Consume`]) but leaves the app accumulating a
/// gesture with no end ever coming — the same failure a latched modifier is,
/// and one no later drain can repair, because the router has already
/// forgotten there was a gesture. `gesture_begin` and the two updates are
/// ordinary consumable input: a consumed begin starts nothing, so its updates
/// and its end are dropped by the router's own pairing.
///
/// # Session-wide by construction: **no** hook is ever told a realm
///
/// Since WS-E.1.6 the router addresses each event to a realm (physical input
/// to the bound realm, an agent's actuation to the realm its grant names),
/// and the whole hook stack sits **above** that split: one stack for the
/// session, called for every event of every realm.
///
/// Neither [`observe`] nor [`gate`] receives the realm, and that is the whole
/// of the guarantee: the consent grab is the trusted path and the dead-man
/// watcher is the human's off-switch, and neither may ever apply to some
/// realms and not others. A policy that is never told which realm an event is
/// for cannot scope itself to one — "the prompt consumes input for every
/// realm" and "the chord revokes every realm's grants" are then
/// inexpressibly-otherwise rather than merely tested, which is the shape
/// D-018(2) asks for.
///
/// `observe` **did** take the realm for one review cycle, because
/// per-realm physical presence needed it — and that made the guarantee a
/// convention (a hook could simply have ignored events for other realms)
/// while three doc comments described it as structural. Presence is now
/// recorded by [`InputRouter`] itself, above the stack, so the argument is
/// sound again: see [`InputRouter`]'s "Presence is not a stackable hook".
///
/// [`observe`]: PreemptionHook::observe
/// [`gate`]: PreemptionHook::gate
pub(crate) trait PreemptionHook {
    /// Non-consuming tap: sees every event at intake, in view coordinates,
    /// before and regardless of gating.
    ///
    /// Sees every event of **every** realm, and is not told which — the tap
    /// runs even for physical input arriving while no realm is bound at all,
    /// because the human's off-switch must work when nothing is on screen.
    fn observe(&mut self, input: &SeatInput);

    /// Consuming gate: runs after [`observe`](Self::observe); a
    /// [`Gate::Consume`] verdict stops the event before routing.
    ///
    /// Not given the realm either — see the trait docs.
    fn gate(&mut self, input: &SeatInput) -> Gate;

    /// The attention signal this stack carries, if any (WS-E.1.7, issue #232).
    ///
    /// **Not a third policy point** — it makes no decision and sees no event.
    /// It is a *wiring* accessor, and it is on the trait for exactly the reason
    /// [`InputRouter::presence`] is on the router: the capability kernel has to
    /// read the same signal the hook writes, and a kernel whose signal is not
    /// its router's would judge the exemption against something nothing opens.
    /// That was constructible for presence until issue #212's review, and every
    /// shipped `vitrind` was in that state. Reaching the signal *through* the
    /// stack makes the mistake unconstructible instead: `Runtime::new` takes it
    /// out of the router it is handed, so no backend can pass a second one.
    ///
    /// **Required, deliberately — there is no default.** `None` is a perfectly
    /// honest answer (a plain headless build has no physical input device and so
    /// no attention key, and [`NoopHook`] says exactly that), but it must be
    /// *said* rather than inherited. A defaulted `None` would mean a wrapping
    /// hook that forgot to forward its inner hook's answer silently returns "no
    /// attention key", `Runtime::new` falls back to a detached signal, and the
    /// chord opens a window nothing reads — the key simply stops working, with
    /// every test still green.
    ///
    /// That is not a hypothetical failure mode, it is the one this codebase
    /// already shipped: `PresenceHook` was an optional member of this same stack
    /// that no backend included, so `preempted` could not fire in any `vitrind`
    /// ever built while the book described it as live (issue #212's review). The
    /// lesson taken from it was to make the omission unconstructible, and a
    /// defaulted method here would have re-opened precisely that hole one
    /// release later. Every wrapping hook must now forward explicitly or fail to
    /// compile.
    fn attention(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>>;

    /// The clipboard chord signal this stack carries, if any (WS-E.2.1, issue
    /// #213).
    ///
    /// The same wiring accessor [`Self::attention`] is, for the same reason and
    /// with the same **deliberate absence of a default**: a wrapping hook that
    /// forgot to forward its inner hook's answer would silently report "no
    /// clipboard chords", `Runtime::new` would fall back to a detached signal,
    /// and the human's Ctrl-Shift-Insert would queue gestures into a `RefCell`
    /// the embedder never drains — the key simply stops working, with every test
    /// still green. That is `PresenceHook`'s failure exactly, and a defaulted
    /// method here would re-open it a third time.
    fn clipboard(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>>;

    /// The screenshot chord signal this stack carries, if any (WS-E.2.4, issue
    /// #216).
    ///
    /// The same wiring accessor the two above are, for the same reason and with
    /// the same **deliberate absence of a default**: a wrapping hook that forgot
    /// to forward its inner hook's answer would silently report "no screenshot
    /// chord", `Runtime::new` would fall back to a detached signal, and the
    /// human's key would queue gestures into a `RefCell` the embedder never
    /// drains — the key simply stops working, with every test still green. That
    /// is `PresenceHook`'s failure exactly, and a defaulted method here would
    /// re-open it a fourth time.
    fn screenshot(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>>;

    /// The brightness-key queue this stack carries, if any (D-041, issue #303).
    ///
    /// The same wiring accessor the three above are, for the same reason and
    /// with the same **deliberate absence of a default**: a wrapping hook that
    /// forgot to forward its inner hook's answer would silently report "no
    /// brightness keys", `Runtime::new` would fall back to a detached signal,
    /// and the human's brightness key would be consumed by a gate whose presses
    /// nothing ever drains — a key that went from *doing nothing* to *doing
    /// nothing and no longer reaching the app*, with every test still green.
    /// That is `PresenceHook`'s failure exactly, and a defaulted method here
    /// would re-open it a fifth time.
    fn backlight(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>>;
}

/// A policy that may **only** consume, and is never handed the observe tap
/// (WS-E.2.2, issue #214).
///
/// # What this exists to make unconstructible
///
/// Every hook in this stack must forward [`PreemptionHook::observe`]
/// unconditionally, because the human's off-switch detects there
/// ([`crate::deadman`]). Every hook that gets it wrong disables the off-switch
/// silently, with every other test still green — and the failure is one
/// keystroke to write: `if self.locked { return; }` at the top of an `observe`
/// body. [`crate::lock::LockGate`] is the sharpest case in the tree, because it
/// is the one gate that consumes **all** physical input: a lock that could
/// swallow the dead-man chord would leave a human who cannot revoke *worse off*
/// locked than unlocked, which inverts the whole safety argument for having a
/// lock.
///
/// So the lock's policy does not implement [`PreemptionHook`] at all. It
/// implements this — a trait with **no observation method** — and
/// [`GateOnlyHook`] supplies the hook impl, forwarding `observe`, `attention`
/// and `clipboard` verbatim. The tap is therefore implemented *in this module*,
/// which has no `use crate::lock` and no notion of a lock existing; an edit
/// inside `crate::lock` cannot make observation conditional because the code
/// that observes is not reachable from there and the trait it calls through
/// cannot express an observation.
///
/// This is the [#210](https://github.com/vitrin-os/vitrin-os/issues/210) /
/// [#232](https://github.com/vitrin-os/vitrin-os/issues/232) shape — make the
/// function private, make the method non-defaulted — applied one level up: make
/// the *capability to get it wrong* absent from the type.
///
/// It is deliberately **not** retrofitted onto the existing hooks.
/// [`crate::deadman::DeadManHook`] and [`crate::chord`]'s consumers genuinely
/// need `observe` (the dead-man detects there; the chord matcher tracks
/// modifiers there so a release a prompt swallowed still clears its bit), and a
/// blanket rewrite would take that away from the two policies that must have
/// it. What this says is narrower and true: **a policy that needs no
/// observation must not be able to touch one.**
pub(crate) trait ConsumingGate {
    /// Judge one intake event. [`Gate::Consume`] stops it before mapping and
    /// before the wire; [`Gate::Deliver`] passes it to the inner hook.
    ///
    /// Bound by the same pairing contract [`PreemptionHook`] states, with the
    /// same razor: consuming a release whose press the *router* delivered
    /// strands that press in the confined app.
    fn judge(&mut self, input: &SeatInput) -> Gate;
}

/// A [`PreemptionHook`] built from a [`ConsumingGate`]: gates through `G`,
/// forwards everything else to `H` **unconditionally and unconditionally-ably**
/// (see [`ConsumingGate`] for what that buys).
///
/// Precedence matches the rest of the stack: when `G` consumes, the inner hook's
/// gate is not consulted. The observe tap is passed through in every case,
/// which is what keeps the dead-man watcher alive while this gate swallows the
/// event stream whole.
pub(crate) struct GateOnlyHook<G: ConsumingGate, H: PreemptionHook> {
    gate: G,
    inner: H,
}

impl<G: ConsumingGate, H: PreemptionHook> GateOnlyHook<G, H> {
    pub fn new(gate: G, inner: H) -> Self {
        Self { gate, inner }
    }
}

impl<G: ConsumingGate, H: PreemptionHook> PreemptionHook for GateOnlyHook<G, H> {
    /// **Unconditional, and unconditional by construction.** `G` has no
    /// observation method, so no policy stacked here can be told about this
    /// event at all, let alone stop it reaching the hooks below. The dead-man
    /// switch's detection therefore survives any gate expressed this way.
    fn observe(&mut self, input: &SeatInput) {
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        match self.gate.judge(input) {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }

    /// Forwarded, never answered here — a gate-only policy owns no wiring
    /// signal. Spelled rather than defaulted for the reason
    /// [`PreemptionHook::attention`] gives at length.
    fn attention(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>> {
        self.inner.attention()
    }

    /// Forwarded, for the reason above.
    fn clipboard(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>> {
        self.inner.clipboard()
    }

    /// Forwarded, for the reason above.
    fn screenshot(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>> {
        self.inner.screenshot()
    }

    /// Forwarded, for the reason above.
    fn backlight(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>> {
        self.inner.backlight()
    }
}

/// The terminal hook: observes nothing, consumes nothing.
///
/// No longer "the MVP placeholder" — the P1.7.x policies (the consent grab
/// and the dead-man watcher) each *wrap* an inner hook, so this is what the
/// stack bottoms out in, and the nested backend really carries it under both.
pub(crate) struct NoopHook;

impl PreemptionHook for NoopHook {
    fn observe(&mut self, _input: &SeatInput) {}

    fn gate(&mut self, _input: &SeatInput) -> Gate {
        Gate::Deliver
    }

    /// The terminal hook owns no attention signal, and says so out loud
    /// rather than inheriting it: a stack that bottoms out here and is not
    /// wrapped by an [`AttentionHook`](crate::attention::AttentionHook) has
    /// no attention key. See [`PreemptionHook::attention`] for why this is
    /// spelled rather than defaulted.
    fn attention(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>> {
        None
    }

    /// The terminal hook owns no clipboard chords either, and says so out loud
    /// for the reason above.
    fn clipboard(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>> {
        None
    }

    /// The terminal hook owns no screenshot chord either, and says so out loud
    /// for the reason above.
    fn screenshot(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>> {
        None
    }

    /// The terminal hook owns no brightness keys either, and says so out loud
    /// for the reason above. A stack that bottoms out here and is not wrapped
    /// by a [`BacklightHook`](crate::backlight::BacklightHook) delivers both
    /// `XF86MonBrightness*` keys to the app, which is every checkout's
    /// behaviour before D-041 and every session that did not pass
    /// `--backlight`.
    fn backlight(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>> {
        None
    }
}

/// How long after the last physical input event the human still "owns the
/// target" for the enforcement chokepoint's `preempted` judgement (below).
/// PRD Doc 2 SS8: "when a physical event arrives, in-flight agent
/// actuations to the same focus are preempted and the agent's actuator is
/// **transiently suspended**" -- this constant is the transient. 500 ms is
/// long enough that an agent can never interleave into the middle of a
/// human's click or keystroke burst, short enough that the human merely
/// resting their hands does not wedge the agent; deployment tuning can
/// join the M1.1 configuration surface if field experience demands it.
pub(crate) const PHYSICAL_HOLD_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

/// Staleness ceiling on the held-button hold: a physically held button
/// owns the target however long the hold lasts -- but only while the
/// device shows *any* sign of life within this ceiling. A hold whose
/// device has been completely silent for the whole ceiling is stale and
/// stops owning the target, and the next physical event purges it rather
/// than resurrecting it. The realistic cause of a minute of total device
/// silence with a button "down" is an unpaired press -- the device
/// unplugged mid-hold, seat/device teardown, or an intake feeder that
/// never synthesized the release -- and without an expiry one lost
/// release would refuse every actuation `preempted` for the life of the
/// process (PRD Doc 2 SS8 calls preemption a **transient** suspension; a
/// permanent wedge is an availability bug, not fail-closed caution).
/// Real drags survive far past the ceiling because any motion -- hand
/// tremor included -- refreshes the activity clock. The M1.1 intake
/// wiring SHOULD additionally synthesize releases on device removal (the
/// libinput convention); this ceiling is the backstop that keeps a
/// feeder gap from becoming a process-lifetime denial of actuation.
pub(crate) const PHYSICAL_HOLD_CEILING: std::time::Duration = std::time::Duration::from_secs(60);

/// Physical-input presence **in one realm**: the state behind the
/// enforcement chokepoint's `preempted` refusal (P1.4.4). "Physical human
/// input owns the target right now" (IDL) holds while either
///
/// - a **physically pressed button is still down** (a human mid-click or
///   mid-drag owns the target however long that takes, provided the
///   device has shown life within [`PHYSICAL_HOLD_CEILING`] -- the
///   stale-hold backstop documented on that constant), or
/// - **any physical event arrived within [`PHYSICAL_HOLD_WINDOW`]** (the
///   PRD's transient suspension after human activity).
///
/// Fed exclusively with the origin tag bound at intake (B2): emulated
/// events -- including the chokepoint's own admitted actuations -- never
/// count, so an agent can never extend its own preemption window. Time is
/// injected ([`crate::grants`]' clock discipline): `note` records the
/// caller's `now`, `owns_target` judges at the caller's `now`, and the
/// chokepoint samples one instant per request for both.
///
/// **One tracker per realm** since WS-E.1.6 (issue #212), held in a
/// [`PhysicalPresenceMap`]. There is still one seat and one human, but
/// physical input is addressed to the **bound** realm while an agent's
/// actuation is addressed to the realm its grant names, so "the target" is a
/// realm rather than the session — see [`PhysicalPresenceMap`] for what that
/// narrows and why the narrowing is the correct direction rather than a
/// relaxation for convenience.
#[derive(Debug, Default)]
pub(crate) struct PhysicalPresence {
    /// Button codes of physically pressed, not-yet-released buttons (a
    /// multiset, mirroring the router's implicit-grab bookkeeping).
    held_buttons: Vec<u32>,
    /// When the most recent physical event was observed.
    last_activity: Option<std::time::Instant>,
}

impl PhysicalPresence {
    /// A test's way to build one tracker in isolation. Production never calls
    /// it: entries are minted through [`PhysicalPresenceMap::note`]'s
    /// `or_default`, which is the only place a realm may acquire one.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observed intake event at `now`. Emulated events are
    /// ignored -- only the physical origin bound at intake counts (B2),
    /// so an agent's own admitted actuations can never extend its
    /// preemption window. Takes `(origin, kind)` rather than a
    /// [`SeatInput`] so that tests outside this module can model an
    /// observed physical event without a physical-origin *constructor*
    /// leaking out of intake; the only runtime feeder is
    /// [`InputRouter::route_into`], which passes the tag intake bound.
    pub fn note(&mut self, origin: Origin, kind: &SeatInputKind, now: std::time::Instant) {
        if origin != Origin::Physical {
            return;
        }
        // Self-heal before recording: if the device sat silent past the
        // stale ceiling, `owns_target` already stopped honoring the held
        // set ([`PHYSICAL_HOLD_CEILING`]), and fresh activity must not
        // resurrect a hold whose release was lost during the gap.
        if self
            .last_activity
            .is_some_and(|at| now.saturating_duration_since(at) >= PHYSICAL_HOLD_CEILING)
        {
            self.held_buttons.clear();
        }
        self.last_activity = Some(now);
        if let SeatInputKind::Button { button, state } = kind {
            match state {
                ButtonState::Pressed => self.held_buttons.push(*button),
                ButtonState::Released => {
                    if let Some(i) = self.held_buttons.iter().position(|b| b == button) {
                        self.held_buttons.remove(i);
                    }
                }
            }
        }
    }

    /// The chokepoint's judgement: does physical human input own the
    /// target at `now`? A held button owns while the device has shown
    /// life within [`PHYSICAL_HOLD_CEILING`] (the stale-hold backstop);
    /// any physical event owns within [`PHYSICAL_HOLD_WINDOW`].
    pub fn owns_target(&self, now: std::time::Instant) -> bool {
        let within = |window: std::time::Duration| {
            self.last_activity
                .is_some_and(|at| now.saturating_duration_since(at) < window)
        };
        (!self.held_buttons.is_empty() && within(PHYSICAL_HOLD_CEILING))
            || within(PHYSICAL_HOLD_WINDOW)
    }
}

/// **Which realms the human's hand is in**: one [`PhysicalPresence`] per
/// realm, and the whole of decision 4 of issue #212 (WS-E.1.6).
///
/// # Why this is per realm, and what per realm narrows
///
/// The chokepoint's `preempted` refusal means "physical human input owns
/// **the target** right now". While a session held one realm the target and
/// the session were the same thing, so one tracker was exact. With several
/// realms the target of an actuation is the realm its **grant** names, and a
/// session-wide tracker answers a different question than the one the refusal
/// asks: a human typing in realm A would preempt an agent working in realm B,
/// which is the concurrent-operation claim the whole project rests on being
/// refused for no reason a human could see.
///
/// **So this narrows a blanket safety behaviour, and that is disclosed rather
/// than smoothed over.** Before WS-E.1.6, a human touching anything muted
/// every agent everywhere. After it, a human in realm A mutes agents in realm
/// A. Anyone who was relying on the old *breadth* — as a crude session-wide
/// "hands off while I work" — loses it here and is not told by any wire
/// event. It is published in `docs/book/src/limits.md`. The narrowing is
/// still the correct direction: the old breadth refused uses whose target no
/// human was anywhere near, which is not caution, it is a wrong answer that
/// happened to be conservative.
///
/// # Entries only ever exist for realms a human has actually been in
///
/// [`Self::note`] is fed from the router's observe-side tap with **the realm
/// the event was addressed to**, and physical input is addressed to the bound
/// realm alone. So a realm that has never held the human's attention has no
/// entry at all and [`Self::owns_target`] answers `false` for it — the honest
/// answer, not a default. [`Self::forget`] drops a realm's entry when the
/// human's attention leaves it (`session::route_physical_turn` and
/// `session::apply_layout`, at the same moment the router drains that realm's
/// held physical presses), so a button the human was holding when the binding
/// moved cannot keep that realm "owned" for the whole
/// [`PHYSICAL_HOLD_CEILING`] with nobody touching it.
#[derive(Debug, Default)]
pub(crate) struct PhysicalPresenceMap {
    per_realm: std::collections::BTreeMap<crate::grants::RealmId, PhysicalPresence>,
}

impl PhysicalPresenceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observed intake event at `now`, against **the realm it was
    /// addressed to**. Emulated events are ignored by [`PhysicalPresence::note`]
    /// itself (B2), and an event addressed to no realm — physical input while
    /// nothing is bound — is recorded nowhere, because there is no target for
    /// a human to own.
    ///
    /// An entry is minted only by a *physical* event, so the map cannot grow
    /// an entry per realm an agent actuates into.
    pub fn note(
        &mut self,
        realm: Option<&crate::grants::RealmId>,
        origin: Origin,
        kind: &SeatInputKind,
        now: std::time::Instant,
    ) {
        if origin != Origin::Physical {
            return;
        }
        let Some(realm) = realm else {
            return;
        };
        self.per_realm
            .entry(realm.clone())
            .or_default()
            .note(origin, kind, now);
    }

    /// Does physical human input own `realm` at `now`?
    ///
    /// `None` — a use that names no realm — answers `false`, and the two
    /// callers that can produce it are both correct with that answer: a
    /// layout request judged while **no** realm is bound has no human
    /// attention to steal, and a seat-delivered use whose grant row has
    /// vanished is unreachable here (step 3 refused it `not_granted`) and is
    /// refused `internal` at the delivery arm regardless. This is not the
    /// fail-open direction it looks like: there is no realm whose presence
    /// could be consulted, so answering `true` would refuse every agent in
    /// the session for a human who is demonstrably nowhere.
    pub fn owns_target(
        &self,
        realm: Option<&crate::grants::RealmId>,
        now: std::time::Instant,
    ) -> bool {
        realm
            .and_then(|realm| self.per_realm.get(realm))
            .is_some_and(|presence| presence.owns_target(now))
    }

    /// Forget `realm`'s presence entirely — the human is no longer in it.
    ///
    /// Called at exactly the moments the router drains that realm's held
    /// physical presses: the human's attention moved somewhere else, so their
    /// next physical event will be addressed to another realm and this one's
    /// held-button set can never be paid down by an intake release. Without
    /// this, a bind change mid-drag leaves a realm "owned" for
    /// [`PHYSICAL_HOLD_CEILING`] — a full minute of refusing every agent
    /// actuating there, on the strength of a button the human let go of into
    /// a different realm.
    pub fn forget(&mut self, realm: &crate::grants::RealmId) {
        self.per_realm.remove(realm);
    }
}

/// One realm's seat state: everything the router believes **that realm's
/// app** was told, and nothing else.
///
/// Split out of [`InputRouter`] by WS-E.1.6 (issue #212). It was one copy per
/// session, which was exact while a session held one realm and became a bug
/// the moment it held two: the pairing tables are the *app's* press/release
/// accounting, so sharing them across realms means a focus switch mid-chord
/// leaves a latched modifier that silently rewrites every subsequent
/// keystroke in an app the human can no longer even see.
///
/// Minted lazily, on the first event addressed to a realm, and dropped by
/// [`InputRouter::reset_for`] when that realm's shim generation ends.
#[derive(Debug, Default)]
pub(crate) struct RealmSeat {
    /// Last known pointer position in view coordinates — buttons and
    /// scroll carry no position of their own and hit-test against this.
    /// Updated at intake (a physical fact), even for events the gate
    /// consumes, so a released grab never hit-tests a stale position.
    pointer: Option<(f64, f64)>,
    /// Last known **physical** pointer position, in view coordinates — the
    /// position a pointer constraint is judged against (WS-E.4.2, issue #222).
    ///
    /// The exact mirror of [`Self::agent_pointer`], written by the other origin
    /// and for the same reason read the other way round. [`Self::pointer`] takes
    /// both origins, because the app has one pointer; judging a **human's**
    /// cursor visibility against it would let an agent's actuation activate a
    /// lock — and so hide the human's own sprite — by moving the shared position
    /// onto the app's surface. `agent_pointer` exists to stop a sprite following
    /// the human's mouse; this exists to stop a hide following the agent's.
    ///
    /// Updated at intake beside [`Self::pointer`], before gating, because where
    /// a pointer *is* is a fact rather than a delivery outcome.
    human_pointer: Option<(f64, f64)>,
    /// Last known **emulated** pointer position, in view coordinates — the
    /// position the agent cursor sprite is drawn at
    /// ([`crate::cursor::composite_agent_cursor`]).
    ///
    /// **The mirror image of [`ConsentGrab::pointer`], one origin over.** That
    /// field is deliberately physical-only, so an agent holding a pointer grant
    /// cannot slide the hit target under the human's finger; this one is
    /// deliberately emulated-only, for the same kind of reason read the other
    /// way round: [`Self::pointer`] is written by *both* origins (the app has
    /// one pointer, which is exactly D-017's deferred per-principal delivery),
    /// so a sprite drawn on it would follow the human's physical mouse and tell
    /// the human an agent is pointing where the human is pointing.
    ///
    /// Updated at intake beside [`Self::pointer`], before gating, for the same
    /// reason: where a pointer *is* is a fact, not a delivery outcome.
    ///
    /// **This is display state and nothing else.** It feeds no hit test, no
    /// routing decision, and no wire event; delivery to the shim remains one
    /// shared position per realm view (module docs; D-019 supersedes only the
    /// "composites no cursor" half of D-017, never the delivery half). Now
    /// one such position **per realm**, which multiplies the drawn-vs-delivered
    /// gap D-017 defers rather than closing any of it — published as a limit.
    ///
    /// [`ConsentGrab::pointer`]: crate::consent::grab::ConsentGrab
    agent_pointer: Option<(f64, f64)>,
    /// Delivered-and-unreleased button presses as `(code, origin)`, in press
    /// order (a multiset: a pathological double-press pairs with a double-
    /// release). Nonempty means an implicit grab holds the pointer on
    /// the surface; a release is delivered iff its code is present here —
    /// per-button pairing, Wayland-style, never a bare count.
    ///
    /// The origin is carried for the same reason [`Self::pressed_keys`]
    /// carries it and is read by the same kind of caller: anything that
    /// synthesises a release from an entry reads the tag back off the entry
    /// rather than minting one, because minting `Origin::Physical` for an
    /// agent's button would forge the physical-vs-emulated distinction (B2)
    /// on the wire and in the flight recorder. Pairing itself stays
    /// per-code, never per-`(code, origin)`: the app has one pointer, and
    /// its grab state counts presses, not who made them.
    pressed: Vec<(u32, Origin)>,
    /// Delivered-and-unreleased key presses as [`HeldKey`], same
    /// multiset discipline as [`Self::pressed`] and for the same razor: a key
    /// release is delivered iff its own press was (module docs). Keys hold
    /// no implicit grab — they carry no geometry — so this is *only*
    /// pairing, and it is what lets a consuming gate stop a key release
    /// without leaving a latched modifier in the app.
    ///
    /// **Why the origin is stored and not assumed.** Both origins share this
    /// realm's seat on purpose (`session::route_seat` routes chokepoint-admitted
    /// actuations through the very same router, which is what makes the
    /// preemption hook meaningful), so the table holds an agent's held keys
    /// beside a human's. Anything that synthesises a release from an entry
    /// therefore has to read the tag back off the entry rather than assume
    /// one: minting `Origin::Physical` for an agent's key would forge the
    /// physical-vs-emulated distinction (B2) on the wire *and* in the flight
    /// recorder — the exact thing [`SeatInput::physical`]'s privacy makes a
    /// compile error at intake. Pairing itself stays per-[`KeyPairing`], not
    /// per-`(pairing, origin)`: the app has one keyboard, and its latch state
    /// counts presses, not who made them.
    pressed_keys: Vec<HeldKey>,
    /// **The one gesture this realm's app was told had begun**, or `None`
    /// (WS-E.4.2, issue #222). The gesture twin of [`Self::pressed`] and
    /// [`Self::pressed_keys`], and the same razor: an update or an end is
    /// delivered **iff** its own begin was.
    ///
    /// An `Option`, not a `Vec`, because the IDL pins at most one gesture in
    /// flight per seat — a second `gesture_begin` while one is live is a core
    /// bug, so the router drops and traces it rather than trusting libinput
    /// not to produce one. Per realm like everything else here: a gesture
    /// begun in realm A must not be endable by an event addressed to realm B.
    ///
    /// The tag rides along for [`Self::pressed`]'s reason — the drain
    /// ([`InputRouter::end_physical_gesture`]) reads it back off the record
    /// rather than minting one, and drains only what a human began.
    gesture: Option<InFlightGesture>,
}

/// One delivered-and-unended gesture ([`RealmSeat::gesture`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InFlightGesture {
    /// The kind the app was told began. The router answers a `gesture_end`
    /// with **this** kind rather than the one intake claimed, so a
    /// disagreement is a logged core bug instead of a silent mis-replay
    /// (IDL `gesture_end`: "kind repeats the in-flight gesture's kind").
    kind: GestureKind,
    origin: Origin,
}

impl RealmSeat {
    /// Remove one delivered-press entry for `button`, if any; returns
    /// whether one existed (i.e. whether a release of this code pairs
    /// with a delivered press).
    fn release_pressed(&mut self, button: u32) -> bool {
        match self.pressed.iter().position(|&(b, _)| b == button) {
            Some(i) => {
                self.pressed.remove(i);
                true
            }
            None => false,
        }
    }

    /// Remove one delivered-press entry matching `pairing`, if any, and
    /// return **the keysym that press delivered**. `None` means this release
    /// pairs with nothing the app was told about. The keyboard twin of
    /// [`Self::release_pressed`].
    ///
    /// Two things it deliberately does not match on.
    ///
    /// Not the **origin**: the app has one keyboard and its latch state
    /// counts presses, so a release must be able to pay down any outstanding
    /// press of that key. The entry's tag is carried for the benefit of the
    /// drains, which are the only readers of it — see [`Self::pressed_keys`]
    /// for the bound on how far a mixed-origin overlap can skew it.
    ///
    /// Not the release's own **keysym** (D-028(3)): under a real keymap the
    /// release of a key pressed with Shift down resolves to a different
    /// keysym than its press did, so matching on it would drop the release,
    /// leave a phantom entry here, and latch a key in the app. The keysym
    /// the caller wants is the one this returns — the press's — because
    /// that is the one the shim bound a keycode for.
    fn release_pressed_key(&mut self, pairing: KeyPairing) -> Option<u32> {
        let i = self
            .pressed_keys
            .iter()
            .position(|e| e.pairing == pairing)?;
        Some(self.pressed_keys.remove(i).keysym)
    }

    /// Whether the last known pointer position lies over the placed
    /// surface rectangle (never true with no position or no surface).
    fn pointer_over_surface(&self, view: ViewGeometry, surface: Option<(u32, u32)>) -> bool {
        let (Some(pointer), Some(surface)) = (self.pointer, surface) else {
            return false;
        };
        let (sx, sy) = surface_local(pointer, view, surface);
        inside(sx, sy, surface)
    }
}

/// The session's input router: the one path from tagged intake events to
/// wire-ready seat deliveries. Holds **one seat's worth of state per realm**
/// (v0: one seat, one cursor per realm, shared by both origins) and the one
/// session-wide preemption hook.
///
/// # Two addressing rules, named as two entry points
///
/// A session holds up to [`crate::realm::MAX_REALMS`] realms (WS-E.1.2) and
/// every one of them may be receiving input at once, from two different
/// sources answering to two different rules (WS-E.1.6, issue #212):
///
/// - **physical input follows the human's attention** — the *bound* realm,
///   the one [`crate::session::physical_seat_target`] names, which a
///   `layout_focus` holder moves ([`Self::route_physical`]);
/// - **an agent's actuation follows its grant** — the realm the grant it was
///   admitted under names, whether or not a human is looking at it
///   ([`Self::route_emulated`]).
///
/// Collapsing the two onto the bound realm would break the project's own
/// premise: an agent must be able to work in a realm the human is not
/// watching, which is the whole of the headless-fleet and concurrent-operation
/// claim. So the rules are two **named entry points** rather than one function
/// that inspects the origin tag and decides: a call site has to say which rule
/// it is following, and switching a path from one to the other is a visible
/// edit rather than a silent one.
///
/// **This is still not an authority check, and must never grow one.** Neither
/// entry point asks whether the caller may address that realm — by the time an
/// agent's event arrives the chokepoint has settled that (prose page 11), and
/// physical input answers to no grant at all. What the split buys is that the
/// two *addresses* cannot be confused, not that either is authorized here.
///
/// # What stays session-wide
///
/// The hook stack ([`Self::hook`]): the consent grab is the trusted path and
/// the dead-man watcher is the human's off-switch, and both must see every
/// physical event regardless of which realm is bound. [`PreemptionHook::gate`]
/// is not even *told* the realm, so a gate scoped to one realm is
/// inexpressible rather than merely untested.
///
/// # Presence is not a stackable hook, it is the router's own record
///
/// [`PhysicalPresenceMap`] is what the chokepoint's `preempted` refusal reads,
/// so a build in which it is never written does not refuse a synthetic click
/// under the human's hand — it *admits* it. From P1.4.4 until issue #212's
/// review, a `PresenceHook` was an optional member of the stack that every
/// shipping backend forgot to include, and the `preempted` step was therefore
/// unreachable in every `vitrind` ever built while the book described it as
/// live behaviour.
///
/// So presence is not stacked at all now: this struct holds the map and writes
/// it in [`Self::route_into`], **above** the hook stack, and [`Self::presence`]
/// is the handle `session::Runtime::new` puts in [`crate::session::Kernel`]. A
/// router that does not feed presence, or a kernel whose presence is not its
/// router's, is unconstructible rather than a mistake nobody made on purpose.
///
/// This is also what restores the hook stack's session-wide guarantee to a
/// structural one. Presence was the *only* reason [`PreemptionHook::observe`]
/// was ever handed a realm, and while it was, "the off-switch cannot be scoped
/// to one realm" was a convention (a `return` away from being false) that three
/// doc comments called inexpressible. With the realm gone from the trait
/// entirely, no policy hook can see one.
///
/// The router still makes **no judgement** with a clock: `now` is a cell the
/// embedder sets through [`Self::observe_at`], and it is used for one thing,
/// timestamping what the human just did.
pub(crate) struct InputRouter<H: PreemptionHook> {
    /// The session-wide policy stack: the consent grab, the dead-man watcher,
    /// and whatever they bottom out in. Told no realm, ever.
    hook: H,
    /// **Where the human's hand is**, per realm — read by the chokepoint
    /// through [`crate::session::Kernel::presence`], which is this same
    /// object.
    presence: std::rc::Rc<std::cell::RefCell<PhysicalPresenceMap>>,
    /// The dispatch turn's instant, set by the embedder through
    /// [`Self::observe_at`]. Shared with the grab and the watcher on the
    /// nested backend, so the whole turn is judged against one sample.
    now: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
    /// **The realm physical input follows**: whichever realm
    /// [`Self::bind_to`] last named, or `None` before the first bind and
    /// after the bound realm's death.
    ///
    /// Not a routing *policy* and not a second copy of one — the binding is
    /// chosen by `session::physical_seat_target` (which follows the output,
    /// which a `layout_focus` holder moves) and merely recorded here, so the
    /// policy lives in one function and this field follows it.
    bound: Option<crate::grants::RealmId>,
    /// **One seat's state per realm**, minted on that realm's first event and
    /// dropped when its shim generation ends ([`Self::reset_for`]).
    ///
    /// The map is what makes a sibling's death, and a focus change,
    /// inconsequential to a realm that was not involved: there is no shared
    /// pairing table left to clear by accident.
    ///
    /// **Bounded, and the bound is small enough to state exactly.** At most
    /// [`crate::realm::MAX_REALMS`] entries, because a realm can only be
    /// addressed while it exists. `size_of::<RealmSeat>()` is **96 bytes**
    /// (two `Option<(f64, f64)>` at 24 each, two `Vec`s at 24 each — measured,
    /// x86-64), plus each `Vec`'s heap for the presses actually outstanding,
    /// which a human's ten fingers and an agent's rate limit both bound in
    /// practice. Sixteen realms is therefore ~1.5 KiB of router state against
    /// the ~590 MiB of per-realm pixels `MAX_REALMS` is actually justified
    /// against ([`crate::realm::MAX_REALMS`]); this changes that accounting by
    /// nothing measurable and is stated so nobody has to wonder.
    seats: std::collections::BTreeMap<crate::grants::RealmId, RealmSeat>,
    /// **The session's pointer-constraint records** (WS-E.4.2, issue #222),
    /// shared with the presentation side by an `Rc` clone.
    ///
    /// Held here rather than beside the router for the reason `presence` is:
    /// the bare-metal composite has to *read* the same table this router writes
    /// in order to decide whether to draw the human's cursor, and only the view
    /// is reachable from the panel's own clock. A second table would be a
    /// sprite decided from state nothing updates. [`Self::constraints`] is the
    /// only way to obtain the handle, and `PointerConstraintTable::new` is
    /// private to `crate::input`, so a backend cannot mint a rival one.
    constraints: std::rc::Rc<std::cell::RefCell<PointerConstraintTable>>,
}

/// The four things [`InputRouter::route_into`] needs from `self` besides the
/// seats: the session-wide policy stack, the per-realm presence map, the
/// per-realm constraint table, and this turn's instant.
///
/// A struct rather than four parameters so the two addressing rules
/// ([`InputRouter::route_physical`] and [`InputRouter::route_emulated`]) build
/// it identically, and so a future field cannot be silently passed by only one
/// of them. `now` is copied out of the cell here, once per event, because the
/// borrow of `self` that holds `hook` and `seats` mutably cannot also hold the
/// cell.
struct Tap<'a, H: PreemptionHook> {
    hook: &'a mut H,
    presence: &'a std::rc::Rc<std::cell::RefCell<PhysicalPresenceMap>>,
    constraints: &'a std::rc::Rc<std::cell::RefCell<PointerConstraintTable>>,
    now: std::time::Instant,
}

impl<H: PreemptionHook> InputRouter<H> {
    /// Build the session's router around `hook`, feeding `presence` at the
    /// observe-side tap on the `now` cell the embedder advances.
    ///
    /// `presence` is a parameter rather than something this constructor mints
    /// because the chokepoint has to read the same map: `Runtime::new` takes it
    /// straight back out through [`Self::presence`], so the kernel's presence
    /// and the router's are one object by construction (struct docs).
    pub fn new(
        presence: std::rc::Rc<std::cell::RefCell<PhysicalPresenceMap>>,
        now: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
        hook: H,
    ) -> Self {
        Self {
            hook,
            presence,
            now,
            bound: None,
            seats: std::collections::BTreeMap::new(),
            constraints: std::rc::Rc::new(std::cell::RefCell::new(PointerConstraintTable::new())),
        }
    }

    /// The presence map this router feeds, for the kernel to judge `preempted`
    /// against. Cloning the handle is the *only* way to obtain it, which is
    /// what makes "the kernel reads the map the router writes" structural.
    pub fn presence(&self) -> std::rc::Rc<std::cell::RefCell<PhysicalPresenceMap>> {
        std::rc::Rc::clone(&self.presence)
    }

    /// **The pointer-constraint table this router owns** (WS-E.4.2), for the
    /// backend that composites the human's cursor to read once per frame.
    ///
    /// Minted by [`Self::new`] rather than taken as a parameter, unlike
    /// `presence`, because the *only* other reader is the view the same
    /// backend constructs a few lines later — and the structural guarantee is
    /// bought the other way here: `PointerConstraintTable::new` is private to
    /// `crate::input`, so an `Rc` clone from this method is the only table a
    /// backend can obtain at all.
    pub fn constraints(&self) -> std::rc::Rc<std::cell::RefCell<PointerConstraintTable>> {
        std::rc::Rc::clone(&self.constraints)
    }

    /// The attention signal this router's hook stack carries, if any
    /// (WS-E.1.7): `None` when nothing in the stack is an
    /// [`AttentionHook`](crate::attention::AttentionHook).
    ///
    /// The same discipline [`Self::presence`] follows and for the same reason:
    /// `Runtime::new` takes the signal *out of the router it is handed* rather
    /// than minting one beside it, so "the kernel judges the exemption against
    /// the signal the hook opens" is structural rather than a wiring step a
    /// backend can forget.
    pub fn attention(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>> {
        self.hook.attention()
    }

    /// The clipboard chord signal this router's hook stack carries (WS-E.2.1),
    /// on the same terms as [`Self::attention`]: `Runtime::new` takes it *out of
    /// the router it is handed*, so the embedder that drains the gestures and
    /// the hook that queues them cannot be two different signals.
    pub fn clipboard(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>> {
        self.hook.clipboard()
    }

    /// The screenshot chord signal this router's hook stack carries (WS-E.2.4),
    /// on the same terms as [`Self::attention`]: `Runtime::new` takes it *out of
    /// the router it is handed*, so the embedder that drains the gestures and
    /// the hook that queues them cannot be two different signals.
    pub fn screenshot(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>> {
        self.hook.screenshot()
    }

    /// The brightness-key queue this router's hook stack carries (D-041), on
    /// the same terms as [`Self::attention`]: `Runtime::new` takes it *out of
    /// the router it is handed*, so the embedder that actuates the presses and
    /// the hook that consumed the keys cannot be two different signals.
    pub fn backlight(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>> {
        self.hook.backlight()
    }

    /// Set the presence tap's clock to this dispatch turn's instant.
    ///
    /// The router still reads no clock of its own — it is *told* one, by
    /// `session::route_physical_turn`, which is the single funnel every
    /// backend's physical input passes through. Putting it there rather than
    /// leaving each embedder to advance a shared cell is deliberate: a tap
    /// whose clock never moved would record every physical event at process
    /// start and `owns_target` would answer `false` forever, which is the same
    /// silent failure as not stacking the tap at all.
    pub fn observe_at(&self, now: std::time::Instant) {
        self.now.set(now);
    }

    /// A router whose presence map nothing else can see, for unit tests that
    /// are about routing rather than about preemption.
    ///
    /// `#[cfg(test)]`, deliberately: a backend cannot call it, so no shipping
    /// build can end up with a router whose presence the kernel never reads —
    /// the exact defect issue #212's review found in P1.4.4's wiring.
    #[cfg(test)]
    pub fn detached(hook: H) -> Self {
        Self::new(
            std::rc::Rc::new(std::cell::RefCell::new(PhysicalPresenceMap::new())),
            std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now())),
            hook,
        )
    }

    /// `realm`'s seat state, minted empty if this is its first event.
    ///
    /// A free function over the map rather than a method on `self`, so the
    /// routing body can hold `&mut self.hook` and `&mut self.seats` at once
    /// (the hook runs for every event; the seat is only the addressed
    /// realm's). `contains_key` then `get_mut` rather than `entry`, because
    /// `entry` demands an owned key and would clone a realm id on **every**
    /// pointer motion rather than on the first one.
    fn seat_mut<'s>(
        seats: &'s mut std::collections::BTreeMap<crate::grants::RealmId, RealmSeat>,
        realm: &crate::grants::RealmId,
    ) -> &'s mut RealmSeat {
        if !seats.contains_key(realm) {
            seats.insert(realm.clone(), RealmSeat::default());
        }
        seats
            .get_mut(realm)
            .expect("the entry was just inserted if it was missing")
    }

    /// Where the **agent's** pointer last was in `realm`, in view coordinates,
    /// or `None` before its first motion there (and after that realm's
    /// [`Self::reset_for`]).
    ///
    /// Read only by the presentation side, to draw the sprite: the backends
    /// pull it once per dispatch round through
    /// [`crate::session::Presenter::set_agent_cursor`], for the realm the
    /// output is showing. See [`RealmSeat::agent_pointer`] for why this is not
    /// [`RealmSeat::pointer`].
    pub fn agent_pointer(&self, realm: &crate::grants::RealmId) -> Option<(f64, f64)> {
        self.seats.get(realm).and_then(|seat| seat.agent_pointer)
    }

    /// Where the **human's** pointer last was in `realm`, in view coordinates
    /// (WS-E.4.2, issue #222), or `None` before their first motion there.
    ///
    /// Read by `session::reconcile_pointer_constraints`, which judges a
    /// constraint's region against it. The human's position specifically, for
    /// the reason [`RealmSeat::human_pointer`] gives: an agent's motion must
    /// not be able to activate a lock and so hide the human's own sprite.
    pub fn human_pointer(&self, realm: &crate::grants::RealmId) -> Option<(f64, f64)> {
        self.seats.get(realm).and_then(|seat| seat.human_pointer)
    }

    /// **Move the human's attention to `realm`**, and hand back what the realm
    /// being left is owed.
    ///
    /// Called by the physical delivery path with the realm
    /// [`crate::session::physical_seat_target`] names, and by
    /// `session::apply_layout` the moment a `layout_focus` holder moves the
    /// output. Re-binding the realm already bound is a no-op returning `None`
    /// — the ordinary case, once per physical dispatch round.
    ///
    /// # The drain, and why the caller cannot skip it
    ///
    /// Binding elsewhere **drains the losing realm's held physical presses**:
    /// every key and every button whose press this router delivered *for the
    /// human*, in press order, keys first. Those come back as
    /// `(losing realm, deliveries)` for the caller to send, and the method is
    /// `#[must_use]` so a call site that ignores them is a warning rather than
    /// a latched `Ctrl` in an app the human has just stopped being able to
    /// see. This is `release_physical_keys`'s reason one level down: host-window
    /// focus loss is the moment the core knows *the human's* release will be
    /// delivered somewhere else, and a realm switch is exactly the same fact —
    /// the next release goes to the realm that just gained the binding.
    ///
    /// **The buttons are new here** (issue #212, decision 3). A press held
    /// across a switch used to be forgotten with no delivery at all, which
    /// wedges an implicit pointer grab in the losing app for good. **And the
    /// gesture in flight is newer still** (WS-E.4.2, issue #222): it is ended
    /// `cancelled`, because a begin with no end leaves the losing app
    /// accumulating a pinch forever — see [`Self::end_physical_gesture`].
    ///
    /// **The agent's held presses are deliberately left alone**, and that is
    /// the whole difference this change makes: an agent addressed to the
    /// losing realm still reaches it, so it can still release its own key on
    /// its next request. Draining those would invent a release the principal
    /// never sent — and, since the delivery and the flight recorder both carry
    /// the tag, attribute it to the human.
    ///
    /// **What the drain costs the app**, stated rather than hidden: a release
    /// the app reads as real. Telling an app the human let go when the human
    /// did not is a lie the app cannot detect; the alternative is a latched
    /// modifier forever, which is worse, and this is the same trade
    /// `release_physical_keys` already argues for focus loss. It now happens
    /// on every switcher keypress rather than only on alt-tab.
    ///
    /// # The losing realm's presence goes with the drain, here rather than in
    /// the caller
    ///
    /// The human's hand has left `losing`, so their next release is addressed
    /// elsewhere and the held set recorded against `losing` can never be paid
    /// down by an intake release. Left behind it would keep the realm "owned"
    /// for [`PHYSICAL_HOLD_CEILING`] — a full minute of refusing every agent
    /// actuating there, on the strength of a button the human let go of in a
    /// different realm. [`Self::forget_presence_of`] therefore runs *inside*
    /// this method, not beside its two call sites: it was a caller's
    /// obligation for exactly one review cycle and the third caller (the realm
    /// death funnel) did not know it existed.
    #[must_use = "the losing realm is owed these releases; dropping them latches a key or \
                  wedges a pointer grab in an app the human can no longer see"]
    pub fn bind_to(
        &mut self,
        realm: &crate::grants::RealmId,
    ) -> Option<(crate::grants::RealmId, Vec<SeatDelivery>)> {
        if self.bound.as_ref() == Some(realm) {
            return None;
        }
        let losing = self.bound.replace(realm.clone())?;
        let mut owed = self.release_physical_keys(&losing);
        owed.extend(self.release_physical_buttons(&losing));
        // Third, so an app that is mid-drag *and* mid-gesture sees the
        // pointer settle before the gesture is called off. Order within a
        // kind is press order; order between kinds is keys, buttons, gesture.
        owed.extend(self.end_physical_gesture(&losing));
        self.forget_presence_of(&losing);
        Some((losing, owed))
    }

    /// Drop `realm`'s physical-presence entry, at the two moments the human's
    /// attention provably leaves it: a bind change ([`Self::bind_to`]) and the
    /// realm's death ([`Self::reset_for`]).
    ///
    /// Private, and called only from those two, because the invariant
    /// [`PhysicalPresenceMap::forget`] documents — an entry exists only while
    /// the human is in that realm — is exactly "the router's per-realm seat
    /// state and the human's presence in that realm are forgotten in one act".
    /// Splitting them is what let a dead realm keep a held button, and a
    /// `preempted` refusal, for the stale-hold ceiling after its shim was
    /// gone.
    fn forget_presence_of(&mut self, realm: &crate::grants::RealmId) {
        self.presence.borrow_mut().forget(realm);
    }

    /// Forget `realm`'s seat state — the implicit-grab bookkeeping, the key
    /// pairing, and the last known pointer position. Returns whether there was
    /// anything to forget.
    ///
    /// Invoked by the realm teardown funnel
    /// ([`ShimServer::connection_closed`](crate::shim::ShimServer::connection_closed)),
    /// so no state can survive into the next shim generation: a stale
    /// grab would route off-surface input to an app that never saw the
    /// press, a stale release (button or key) would arrive unpaired at the
    /// fresh seat, and a stale pointer position would let a press hit-test
    /// geometry the new shim never produced. First motion re-establishes
    /// the pointer.
    ///
    /// The agent-owned position goes with it, so the sprite does not hover
    /// over a realm that no longer exists: the next composite draws none.
    ///
    /// **Scoped by construction now.** One router serves a session holding
    /// several realms, and an unconditional clear on *any* realm's death used
    /// to be one realm reaching into another's state — the survivor's app
    /// keeps holding a key whose release the router has just forgotten it
    /// owes. Since the state is a map keyed by realm there is no shared table
    /// left to clear by accident; this removes one entry and nothing else.
    ///
    /// The binding goes too when the dying realm held it, because physical
    /// input has nowhere to follow until `session::rebind_output_after_death`
    /// picks a survivor. **No drain accompanies that**, unlike
    /// [`Self::bind_to`]: the shim is gone, so there is nobody left to deliver
    /// a release to.
    ///
    /// **The presence entry goes too**, for the same reason it goes on a bind
    /// change: a human mid-drag when the shim dies would otherwise leave that
    /// realm "owned" — every agent actuating there refused `preempted` — for
    /// [`PHYSICAL_HOLD_CEILING`], with the app that could have paid the
    /// release down already dead. It is unobservable today only by an accident
    /// of ordering (step 5a refuses a seat-delivered use over a dead realm
    /// `no_surface` before step 5c is reached), which is not a property to
    /// rest on: [`Self::forget_presence_of`] is called here so the ordering
    /// never has to hold.
    ///
    /// **And the pointer-constraint record goes too** (WS-E.4.2, issue #222),
    /// silently: the shim is gone, so there is nobody to send a
    /// `pointer_constraint_state` to. It is cleared *here* rather than beside
    /// this method's callers for the reason [`Self::forget_presence_of`] is —
    /// the realm death funnel reaches teardown two ways
    /// (`ShimServer::connection_closed` for a realm whose shim came up, and a
    /// bare `router.reset_for` for one that died before it did), and a clear
    /// written at the first would have a hole at exactly the second.
    pub fn reset_for(&mut self, realm: &crate::grants::RealmId) -> bool {
        let had_seat = self.seats.remove(realm).is_some();
        let was_bound = self.bound.as_ref() == Some(realm);
        if was_bound {
            self.bound = None;
        }
        self.forget_presence_of(realm);
        let had_constraint = self.constraints.borrow_mut().forget(realm);
        had_seat || was_bound || had_constraint
    }

    /// The realm physical input currently follows, if any.
    ///
    /// Every *emulated* delivery site names its own realm (that is what
    /// [`Self::route_emulated`] is for) and never asks this. The one reader is
    /// the physical path: `crate::backend::winit`'s focus-loss drain, which
    /// needs the realm the human's held keys are owed to. `post_dispatch`'s
    /// agent-cursor gate used to be the second, asking the opposite question
    /// — "is the output showing the realm whose agent pointer I am about to
    /// draw" — and WS-E.1.6 removed it: the sprite position is looked up by
    /// the focused realm through [`Self::agent_pointer`], so there is no
    /// binding to compare against.
    pub fn bound_realm(&self) -> Option<&crate::grants::RealmId> {
        self.bound.as_ref()
    }

    /// The presses the router believes `realm`'s app is holding, as
    /// `(keysym, origin)` in press order. Test-only read of
    /// [`RealmSeat::pressed_keys`]: the pairing table is the state a sibling
    /// realm's death and a focus change must not disturb, and nothing outside
    /// this module can otherwise observe it without consuming it (the drains
    /// drain).
    #[cfg(test)]
    pub fn held_keys(&self, realm: &crate::grants::RealmId) -> Vec<(u32, Origin)> {
        self.seats
            .get(realm)
            .map(|seat| {
                seat.pressed_keys
                    .iter()
                    .map(|e| (e.keysym, e.origin))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The buttons the router believes `realm`'s app is holding, same
    /// discipline as [`Self::held_keys`].
    #[cfg(test)]
    pub fn held_buttons(&self, realm: &crate::grants::RealmId) -> &[(u32, Origin)] {
        self.seats
            .get(realm)
            .map(|seat| seat.pressed.as_slice())
            .unwrap_or(&[])
    }

    /// Release every key the router believes **`realm`'s** app is holding **on
    /// the human's behalf**, in press order, and forget those — one wire-ready
    /// [`SeatDelivery`] per key, for the caller to deliver. Keys an agent is
    /// holding are left exactly as they were.
    ///
    /// This is [`RealmSeat::pressed_keys`]'s pairing invariant read out loud:
    /// the table holds exactly the keysyms whose **press this router
    /// delivered**, so one release per physical entry is precisely what the
    /// app is owed and never more. A key whose press a gate consumed (the
    /// dead-man chord's) is not in there and gets nothing, because the app
    /// never saw it go down.
    ///
    /// **Why only the physical entries.** Both callers' warrant is that the
    /// human's release will land somewhere else — host-window focus loss
    /// (`crate::backend::winit`'s `handle_focus`), or the binding moving to
    /// another realm ([`Self::bind_to`]). Nothing about an agent's actuation
    /// channel changed in either case — it does not run through the host
    /// window's keyboard focus, and since WS-E.1.6 it does not run through the
    /// binding either — so draining an agent's held key here would invent a
    /// release the principal never sent, drop a modifier it is deliberately
    /// holding without telling it, and (since the delivery and the flight
    /// recorder both carry the tag) attribute that to the human. Both origins
    /// share this realm's seat by design, so the filter is load-bearing, not
    /// defensive.
    ///
    /// Each release carries the tag its own entry recorded, so no origin is
    /// minted here; the filter is what makes that tag always `Physical` in
    /// practice. Entries are removed in place, so press order survives.
    ///
    /// **Why the preemption hook is bypassed.** These are not new physical
    /// events to judge — no key moved — they are the delivery debt the router
    /// itself is recording. Routing them back through [`Self::route_physical`]
    /// would hand the dead-man watcher a release with no press, and a gate
    /// that consumed one would strand exactly the key this exists to free.
    ///
    /// On host-window focus loss this is the one moment the core knows the
    /// physical release will be delivered somewhere else: winit's Wayland
    /// backend emits no key events at all on `wl_keyboard.leave`, so without
    /// this a key held across an alt-tab stays latched down in the confined
    /// app forever.
    ///
    /// Distinct from [`Self::reset_for`] on purpose. That forgets *all* of a
    /// realm's seat state for a shim that is gone, where no delivery is
    /// possible or wanted; this pays the app what it is owed while the shim is
    /// very much alive, and leaves the pointer position untouched — a focus
    /// change is not a new shim generation.
    ///
    /// Known imprecision, bounded and deliberate: pairing is per-keysym (see
    /// [`RealmSeat::pressed_keys`]), so if both origins hold the *same* keysym
    /// in the same realm at once, a release consumes the oldest entry
    /// regardless of tag and the surviving entry's tag can name the other
    /// origin. The number of outstanding presses is always right, and no
    /// release is ever emitted with a tag the table did not record — but in
    /// that one overlap the drain can skip a human's key (leaving it latched
    /// until the agent releases) or keep an agent's. Making the table exact
    /// would mean pairing per-`(keysym, origin)`, which would drop a human's
    /// release of an agent-pressed key as unpaired — a latched modifier, the
    /// worse of the two failures.
    pub fn release_physical_keys(&mut self, realm: &crate::grants::RealmId) -> Vec<SeatDelivery> {
        let mut released = Vec::new();
        let Some(seat) = self.seats.get_mut(realm) else {
            return released;
        };
        seat.pressed_keys.retain(|entry| {
            if entry.origin != Origin::Physical {
                return true;
            }
            released.push(SeatDelivery {
                // The tag is read back off the entry, never minted: see the
                // origin argument above.
                origin: entry.origin,
                kind: SeatDeliveryKind::Key {
                    // And so is the keysym — the one the PRESS delivered, so
                    // the app releases the keycode it is actually holding
                    // (D-028(3)). This was already true here and is now true
                    // of the ordinary release path too.
                    keysym: entry.keysym,
                    state: KeyState::Released,
                },
            });
            false
        });
        released
    }

    /// The pointer twin of [`Self::release_physical_keys`]: release every
    /// button the router believes **`realm`'s** app is holding on the human's
    /// behalf, in press order, and forget those.
    ///
    /// Added by WS-E.1.6 (issue #212, decision 3), and it is the half that was
    /// missing rather than a symmetry for its own sake. A key held across a
    /// bind change latches a modifier; a *button* held across one wedges the
    /// app's implicit pointer grab, and until this existed the only treatment
    /// a held button got was being forgotten — correct for a dead shim,
    /// wrong for a live one, and a mid-drag focus switch is exactly the live
    /// case.
    ///
    /// Everything [`Self::release_physical_keys`] says about origins, minted
    /// tags, the bypassed hook and the bounded per-code imprecision applies
    /// here word for word; the only difference is which table is drained.
    ///
    /// **Keys before buttons** where both are drained ([`Self::bind_to`]),
    /// which is the order Wayland clients are least surprised by (a keyboard
    /// latch is the state that misbehaves worst) and is otherwise immaterial:
    /// within each kind, press order is preserved.
    pub fn release_physical_buttons(
        &mut self,
        realm: &crate::grants::RealmId,
    ) -> Vec<SeatDelivery> {
        let mut released = Vec::new();
        let Some(seat) = self.seats.get_mut(realm) else {
            return released;
        };
        seat.pressed.retain(|&(button, origin)| {
            if origin != Origin::Physical {
                return true;
            }
            released.push(SeatDelivery {
                origin,
                kind: SeatDeliveryKind::Button {
                    button,
                    state: ButtonState::Released,
                },
            });
            false
        });
        released
    }

    /// **End the gesture `realm`'s app was told had begun**, `cancelled`, and
    /// forget it — the third drain, added by WS-E.4.2 (issue #222).
    ///
    /// Everything [`Self::release_physical_keys`] says about origins, minted
    /// tags and the bypassed preemption hook applies here word for word. The
    /// difference is what the debt *is*: a key latches a modifier and a button
    /// wedges an implicit grab, and a gesture with no end leaves the app
    /// accumulating a pinch forever — the same failure in a third shape, and
    /// the one the IDL names outright ("what the core owes in return is
    /// exactly one gesture_end for every gesture_begin it sent, without
    /// exception").
    ///
    /// **`cancelled`, never `completed`**, because the human did not finish
    /// it: something took the input away mid-gesture, which is precisely the
    /// distinction `gesture_state` exists to carry. An app that was previewing
    /// a zoom puts it back rather than committing one the human never
    /// confirmed. `docs/plan/14-workstream-session-mode.md` §6 already
    /// publishes this trade for keys and buttons — "a realm switch mid-gesture
    /// tells the app the human let go" — and a touchpad gesture joins that
    /// published limit rather than inventing a new one.
    ///
    /// Returns at most one delivery: the IDL pins at most one gesture in
    /// flight per seat. A `Vec` anyway, so the call sites can `extend` it into
    /// the same owed list the other two drains fill, in the same order every
    /// time.
    ///
    /// **Only the human's gesture**, by the filter both siblings use — though
    /// today no actuator can mint one, so the filter is a guarantee about the
    /// future rather than a live branch: version 2 defines no emulated source
    /// of a gesture, so every record this drains was tagged physical at
    /// intake. It is written as a filter rather than an assertion for exactly
    /// the reason the other two are: when an emulated source does arrive,
    /// draining it here would invent an end the principal never sent and
    /// attribute it to the human.
    ///
    /// # Where this is called, and the one place it deliberately is not
    ///
    /// Beside [`Self::release_physical_buttons`], at both of its call sites
    /// and no others: [`Self::bind_to`] (the human's attention moved to
    /// another realm) and `session::suspend_physical_seat` (the seat gave the
    /// devices away on a VT switch). **Not** `backend::winit`'s
    /// `handle_focus`, for the same reason that site drains keys and not
    /// buttons: it is the host reporting *keyboard* focus loss, pointer state
    /// is reported separately, and a gesture is pointer-side. That exclusion
    /// costs nothing today for a second, sharper reason — smithay's winit
    /// backend produces no gesture events at all, so a nested session can
    /// never have one in flight to drain.
    pub fn end_physical_gesture(&mut self, realm: &crate::grants::RealmId) -> Vec<SeatDelivery> {
        let Some(seat) = self.seats.get_mut(realm) else {
            return Vec::new();
        };
        let Some(live) = seat.gesture else {
            return Vec::new();
        };
        if live.origin != Origin::Physical {
            return Vec::new();
        }
        seat.gesture = None;
        vec![SeatDelivery {
            // Read back off the record, never minted — see above.
            origin: live.origin,
            kind: SeatDeliveryKind::GestureEnd {
                kind: live.kind,
                state: GestureState::Cancelled,
            },
        }]
    }

    /// **Route one physical event to the realm the human's attention is bound
    /// to** — the first of the router's two addressing rules.
    ///
    /// `view` is the composed realm-view size (nested: the host window size
    /// the scene composes at), `surface` the committed client surface size of
    /// **that same bound realm**
    /// ([`Scene::surface_size`](crate::scene::Scene::surface_size)), if any;
    /// `session::route_physical_turn` resolves both from one call to
    /// `physical_seat_target` so they cannot name different realms.
    ///
    /// Returns the wire-ready delivery, or `None` if the event was consumed by
    /// the gate, had no destination under the module's routing rules (matte
    /// hit, no committed surface, unpaired release), or arrived while **no
    /// realm is bound**. The hook still runs in that last case: a human must
    /// be able to hold the dead-man chord when there is nothing on screen.
    pub fn route_physical(
        &mut self,
        input: SeatInput,
        view: ViewGeometry,
        surface: Option<(u32, u32)>,
    ) -> Option<SeatDelivery> {
        debug_assert_eq!(
            input.origin,
            Origin::Physical,
            "route_physical addresses the human's attention; an agent's actuation is \
             addressed by its grant (route_emulated)"
        );
        let Self {
            hook,
            presence,
            constraints,
            now,
            bound,
            seats,
        } = self;
        Self::route_into(
            Tap {
                hook,
                presence,
                constraints,
                now: now.get(),
            },
            seats,
            bound.as_ref(),
            input,
            view,
            surface,
        )
    }

    /// **Route one chokepoint-admitted actuation to the realm its grant
    /// names** — the router's second addressing rule, and the one that makes
    /// an agent able to work in a realm nobody is looking at.
    ///
    /// `realm` comes from the grant row the use was admitted under, carried
    /// here by `session::route_seat`; `view`/`surface` are that realm's
    /// geometry. Nothing here re-checks the authority that named it — a
    /// delivery site that made its own authority judgement would be the second
    /// enforcement site this crate does not have.
    pub fn route_emulated(
        &mut self,
        realm: &crate::grants::RealmId,
        input: SeatInput,
        view: ViewGeometry,
        surface: Option<(u32, u32)>,
    ) -> Option<SeatDelivery> {
        debug_assert_eq!(
            input.origin,
            Origin::Emulated,
            "route_emulated addresses a grant's realm; the human's own input follows the \
             binding (route_physical)"
        );
        let Self {
            hook,
            presence,
            constraints,
            now,
            seats,
            ..
        } = self;
        Self::route_into(
            Tap {
                hook,
                presence,
                constraints,
                now: now.get(),
            },
            seats,
            Some(realm),
            input,
            view,
            surface,
        )
    }

    /// The routing body both addressing rules share: hook, then this realm's
    /// pairing and geometry.
    ///
    /// Takes the fields rather than `&mut self` so the caller can pick the
    /// realm out of `self.bound` without a clone and without the borrow
    /// checker seeing two mutable borrows of `self`.
    fn route_into(
        tap: Tap<'_, H>,
        seats: &mut std::collections::BTreeMap<crate::grants::RealmId, RealmSeat>,
        realm: Option<&crate::grants::RealmId>,
        input: SeatInput,
        view: ViewGeometry,
        surface: Option<(u32, u32)>,
    ) -> Option<SeatDelivery> {
        // Position is recorded before gating: where the pointer *is* is a
        // physical fact, not a delivery outcome. Into the addressed realm's
        // own seat -- with several realms, one shared position would let a
        // press in one realm hit-test against a pointer another realm moved.
        // **THE FREEZE**, sampled before anything is recorded (WS-E.4.2, issue
        // #222). The IDL says a lock "stops absolute motion and freezes the
        // position the core's own hit tests use", and the second half is why
        // this is here rather than only down in the `Motion` arm: recording the
        // position anyway would let the pointer drift out of the constraint's
        // own region while the app is told nothing — and the region test is
        // what keeps the constraint active. The lock would end because of
        // motion it was supposed to stop.
        //
        // **`Origin::Physical` only**, and the position `RealmSeat::pointer`
        // holds keeps taking an agent's motion: `route_emulated` is a separate
        // named entry point precisely so an agent's actuation follows its grant,
        // and an app that locks the pointer must not thereby confine an agent
        // (D-032).
        //
        // Sampled here rather than acted on here, so nothing below moves: the
        // preemption hook, the presence tap and every gate still see the event
        // exactly as they did. A locked pointer must not cost the human their
        // off-switch, and the way that is guaranteed is that this predicate is
        // read at two points strictly *below* the tap and never at it.
        let frozen = input.origin == Origin::Physical
            && matches!(input.kind, SeatInputKind::Motion { .. })
            && realm.is_some_and(|realm| tap.constraints.borrow().is_active(realm));

        if let (Some(realm), false, SeatInputKind::Motion { x, y }) = (realm, frozen, &input.kind) {
            let seat = Self::seat_mut(seats, realm);
            seat.pointer = Some((*x, *y));
            // ...and the two origin-owned positions beside it, each written by
            // exactly one origin (see `RealmSeat::agent_pointer` and
            // `RealmSeat::human_pointer`). Not `else` branches on the shared
            // position: that keeps taking both origins, because that is what
            // the app is delivered.
            match input.origin {
                Origin::Emulated => seat.agent_pointer = Some((*x, *y)),
                Origin::Physical => seat.human_pointer = Some((*x, *y)),
            }
        }

        // THE preemption point. Two things happen here, in this order, and
        // the split between them is the whole of the module's session-wide
        // guarantee:
        //
        //   1. the router records **where the human's hand is**, per realm,
        //      against this turn's instant -- above the stack, because it is
        //      the one per-realm fact and no policy hook may see a realm;
        //   2. the session-wide stack observes unconditionally, then gates.
        //
        // Both are reached even when no realm is bound: `note` discards an
        // event addressed to nothing, and the human's off-switch does not
        // depend on anything being on screen.
        let Tap {
            hook,
            presence,
            constraints: _,
            now,
        } = tap;
        presence
            .borrow_mut()
            .note(realm, input.origin, &input.kind, now);
        hook.observe(&input);
        if hook.gate(&input) == Gate::Consume {
            // Delivered-state reconciliation: a consumed release still
            // physically ended a hold the app saw begin. Clear the
            // press's bookkeeping — nothing is delivered — so a consuming
            // gate can never wedge the implicit grab or strand a keysym in
            // the pairing table; the app-side pairing debt belongs to the
            // gate implementor (`Gate::Consume` docs). Both release kinds
            // are reconciled, because both are paired.
            if let Some(seat) = realm.and_then(|realm| seats.get_mut(realm)) {
                match &input.kind {
                    SeatInputKind::Button {
                        button,
                        state: ButtonState::Released,
                    } => {
                        seat.release_pressed(*button);
                    }
                    SeatInputKind::Key {
                        source,
                        keysym,
                        state: KeyState::Released,
                    } => {
                        seat.release_pressed_key(source.pairing(*keysym));
                    }
                    // A gesture's end is a release for this purpose
                    // (WS-E.4.2): the in-flight record is the router's
                    // bookkeeping for a delivered begin, so a consumed end
                    // clears it exactly as a consumed button release clears
                    // the grab. Nothing goes on the wire, and the app is
                    // left accumulating the gesture -- the gate
                    // implementor's debt, and the reason `PreemptionHook`'s
                    // pairing contract asks gates to deliver this one.
                    SeatInputKind::GestureEnd { .. } => {
                        seat.gesture = None;
                    }
                    _ => {}
                }
            }
            return None;
        }

        // Nothing bound: physical input has no destination this round. The
        // hook above has already seen it, which is the half that must not
        // depend on a realm being on screen.
        let realm = realm?;
        let seat = Self::seat_mut(seats, realm);

        let SeatInput { origin, kind } = input;
        let kind = match kind {
            // Keyboard focus is held on the app shim-side (IDL: focus is
            // synthesized in the shim in v1), so keys route without
            // geometry — but not without pairing: a release is delivered
            // iff its own press was, per keysym, so no policy that stops a
            // key release can leave a latched modifier behind in the app
            // (module docs).
            SeatInputKind::Key {
                source,
                keysym,
                state,
            } => match state {
                KeyState::Pressed => {
                    // The tag rides into the pairing table with the press, so
                    // whatever synthesises this key's release later reads it
                    // back rather than assuming one ([`RealmSeat::pressed_keys`]).
                    // So does the keysym, for the release below.
                    seat.pressed_keys.push(HeldKey {
                        pairing: source.pairing(keysym),
                        keysym,
                        origin,
                    });
                    SeatDeliveryKind::Key { keysym, state }
                }
                KeyState::Released => {
                    // Two things happen here, and the second is D-028(3).
                    // The release pairs by the KEY (scancode where a device
                    // is behind it), so a modifier that moved between press
                    // and release cannot make it miss. And what goes on the
                    // wire is the keysym the PRESS delivered, not this
                    // event's own: the shim bound a keycode for that one, so
                    // sending any other would leave the app holding a key it
                    // is never told to release.
                    let pressed_keysym = seat.release_pressed_key(source.pairing(keysym))?;
                    SeatDeliveryKind::Key {
                        keysym: pressed_keysym,
                        state,
                    }
                }
            },
            SeatInputKind::Text { text } => SeatDeliveryKind::Text { text },

            SeatInputKind::Motion { x, y } => {
                // The delivery half of the freeze sampled at the top of this
                // function: while this realm's constraint is in force the app
                // gets deltas (`relative_motion`, minted by `intake_physical`
                // from the very same host event) and no absolute motion at all
                // — which is what a lock IS.
                //
                // Ranked explicitly BELOW the sprite. A frozen but visible
                // cursor looks like a hung compositor and leaves the human every
                // escape they had; a hidden one does not. So this arm is the
                // backstop that makes the rule true for any future backend, and
                // the bare-metal backend additionally declines to move its own
                // accumulated position — see `backend::drm`'s `handle_libinput`.
                if frozen {
                    return None;
                }
                // No committed surface: nothing to point at (and no
                // placement to map through) — not deliverable.
                let surface = surface?;
                let (sx, sy) = surface_local((x, y), view, surface);
                if seat.pressed.is_empty() && !inside(sx, sy, surface) {
                    return None; // the matte is not the app
                }
                SeatDeliveryKind::Motion {
                    x: Fixed::from_f64(sx),
                    y: Fixed::from_f64(sy),
                }
            }

            SeatInputKind::Button { button, state } => match state {
                ButtonState::Pressed => {
                    if seat.pressed.is_empty() && !seat.pointer_over_surface(view, surface) {
                        return None; // press on the matte starts nothing
                    }
                    // The tag rides into the pairing table with the press,
                    // so whatever synthesises this button's release later
                    // reads it back rather than assuming one.
                    seat.pressed.push((button, origin));
                    SeatDeliveryKind::Button { button, state }
                }
                ButtonState::Released => {
                    // A release is delivered iff its own press was — per
                    // button code, so a matte-dropped press can never
                    // borrow another button's grab, the wire never sees
                    // a release for a button the app never saw pressed,
                    // and the implicit grab guarantees the app never
                    // holds a stuck button, wherever the pointer
                    // wandered meanwhile.
                    if !seat.release_pressed(button) {
                        return None;
                    }
                    SeatDeliveryKind::Button { button, state }
                }
            },

            SeatInputKind::Scroll { axis, value120 } => {
                if seat.pressed.is_empty() && !seat.pointer_over_surface(view, surface) {
                    return None;
                }
                SeatDeliveryKind::Scroll { axis, value120 }
            }

            // A delta carries no position of its own, so it hit-tests
            // against the same stored pointer `scroll` does — the existing
            // rule for a position-less pointer event, not a new one. Getting
            // this wrong in the *other* direction is what matters: an app
            // that was never told the pointer entered it must not be handed
            // the movement of a pointer that is over the letterbox matte, and
            // an app mid-drag must keep receiving deltas wherever the pointer
            // wandered, which is what the implicit-grab clause below gives it.
            SeatInputKind::RelativeMotion {
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
            } => {
                if seat.pressed.is_empty() && !seat.pointer_over_surface(view, surface) {
                    return None;
                }
                SeatDeliveryKind::RelativeMotion {
                    dx: Fixed::from_f64(dx),
                    dy: Fixed::from_f64(dy),
                    dx_unaccel: Fixed::from_f64(dx_unaccel),
                    dy_unaccel: Fixed::from_f64(dy_unaccel),
                }
            }

            SeatInputKind::GestureBegin { kind, fingers } => {
                if seat.pressed.is_empty() && !seat.pointer_over_surface(view, surface) {
                    return None; // a gesture over the matte is not the app's
                }
                // At most one gesture in flight per seat (IDL). A second
                // begin is a core bug or a device the layer below
                // mis-reported; either way, believing it would owe the app
                // two ends for two begins it can only pair one of. Dropped
                // and traced -- never trusted away.
                if let Some(live) = seat.gesture {
                    tracing::debug!(
                        in_flight = ?live.kind,
                        arriving = ?kind,
                        "gesture_begin dropped: one gesture is already in flight for this realm"
                    );
                    return None;
                }
                // The tag rides into the record with the begin, so the drain
                // reads it back rather than assuming one.
                seat.gesture = Some(InFlightGesture { kind, origin });
                SeatDeliveryKind::GestureBegin { kind, fingers }
            }

            SeatInputKind::GestureSwipeUpdate { dx, dy } => {
                // Delivered iff its own begin was, and iff that begin was a
                // swipe: a pinch update inside a swipe would hand the app
                // motion for a gesture of a shape it is not tracking.
                if seat.gesture.map(|g| g.kind) != Some(GestureKind::Swipe) {
                    return None;
                }
                SeatDeliveryKind::GestureSwipeUpdate {
                    dx: Fixed::from_f64(dx),
                    dy: Fixed::from_f64(dy),
                }
            }

            SeatInputKind::GesturePinchUpdate {
                dx,
                dy,
                scale,
                rotation,
            } => {
                if seat.gesture.map(|g| g.kind) != Some(GestureKind::Pinch) {
                    return None;
                }
                SeatDeliveryKind::GesturePinchUpdate {
                    dx: Fixed::from_f64(dx),
                    dy: Fixed::from_f64(dy),
                    scale: Fixed::from_f64(scale),
                    rotation: Fixed::from_f64(rotation),
                }
            }

            SeatInputKind::GestureEnd { kind, state } => {
                // Unpaired ends are dropped, exactly as unpaired releases
                // are: an end the app never saw a begin for would tell it to
                // finish a gesture it is not making.
                let live = seat.gesture.take()?;
                if live.kind != kind {
                    // The gesture that is actually in flight is the one that
                    // gets ended -- never the one the event claimed, or the
                    // app would be left accumulating the real one forever.
                    tracing::warn!(
                        in_flight = ?live.kind,
                        arriving = ?kind,
                        "gesture_end names a different gesture than the one in flight; \
                         ending the one in flight"
                    );
                }
                SeatDeliveryKind::GestureEnd {
                    kind: live.kind,
                    state,
                }
            }
        };
        Some(SeatDelivery { origin, kind })
    }
}

/// View coordinates → surface-local coordinates, through the same
/// deterministic placement the compositor paints with
/// ([`ViewGeometry::place`]: centered and 1:1 inside the *usable* rectangle,
/// negative when the surface is center-cropped). Router and scene can never
/// disagree about where the surface is, because they ask the same function.
///
/// **This is one of the three placement consumers issue #304 threaded**: a
/// pointer at the top of the output is over the trusted band or the status
/// strip, not over the app's first row, and before the inset the router said
/// otherwise.
fn surface_local(point: (f64, f64), view: ViewGeometry, surface: (u32, u32)) -> (f64, f64) {
    let placement = view.place(surface);
    (point.0 - placement.x as f64, point.1 - placement.y as f64)
}

/// Whether surface-local coordinates fall inside the surface.
fn inside(sx: f64, sy: f64, surface: (u32, u32)) -> bool {
    sx >= 0.0 && sy >= 0.0 && sx < f64::from(surface.0) && sy < f64::from(surface.1)
}

/// Nested-mode intake: translate one host-compositor input event into
/// origin-tagged [`SeatInput`]s (0, 1, or — for a diagonal scroll — 2).
/// **This is the only producer of `Origin::Physical` events in the core**;
/// it is generic over Smithay's [`InputBackend`] so tests drive it with a
/// synthetic backend (winit cannot run in CI), and the nested backend
/// instantiates it with `WinitInput`.
///
/// `view` is the host window size in physical pixels — the space
/// [`AbsolutePositionEvent::x_transformed`] resolves absolute positions
/// into, which is the same space the scene composes the view at (the
/// window presents the composed view 1:1), so no further conversion
/// applies at intake.
///
/// # What this translates, and what it still drops
///
/// Version 2 of `vitrin_shim_seat` grew five events (WS-E.4.2, issue #222),
/// so three classes that used to fall through the drop arm are served here
/// now: **relative pointer motion**, **multi-finger swipe** and **pinch**.
///
/// Relative motion is the one with a subtlety. The IDL says `relative_motion`
/// *accompanies* `motion` rather than replacing it — one physical movement
/// produces both — but this function holds no pointer position and so cannot
/// mint the absolute half. The backend that owns the accumulated position
/// does: [`crate::backend::drm`]'s libinput arm mints
/// [`physical_motion`] and calls this for the delta, so both halves reach the
/// wire from the one place that can compute both. A caller that only forwards
/// `PointerMotion` here therefore gets the delta alone, which is honest — it
/// is all the event itself contains.
///
/// **Still dropped, and each for its own reason.** Touch and tablet are *not
/// yet served*: the wire has no event for either, and inventing a lossy
/// translation at intake would bypass the protocol's own growth path. That is
/// a not-yet rather than a never — what would reopen each is recorded in this
/// repository's decision log, not here. Switch events (a lid closing) and
/// hold gestures have no wire event either. Device add/remove are not input.
pub(crate) fn intake_physical<B: InputBackend>(
    event: &InputEvent<B>,
    view: (i32, i32),
) -> Vec<SeatInput> {
    match event {
        InputEvent::PointerMotionAbsolute { event } => {
            vec![SeatInput::physical(SeatInputKind::Motion {
                x: event.x_transformed(view.0),
                y: event.y_transformed(view.1),
            })]
        }
        InputEvent::PointerButton { event } => {
            let state = match event.state() {
                host::ButtonState::Pressed => ButtonState::Pressed,
                host::ButtonState::Released => ButtonState::Released,
            };
            vec![SeatInput::physical(SeatInputKind::Button {
                button: event.button_code(),
                state,
            })]
        }
        InputEvent::PointerAxis { event } => {
            let mut out = Vec::new();
            for axis in [host::Axis::Vertical, host::Axis::Horizontal] {
                // Discrete wheels report v120 directly; continuous sources
                // (touchpads) report pixels, converted at the documented
                // fixed rate. Zero after rounding means "no scroll on this
                // axis", not a zero-valued event.
                let v120 = event
                    .amount_v120(axis)
                    .or_else(|| event.amount(axis).map(|px| px * V120_PER_SCROLL_PIXEL));
                let Some(v120) = v120 else { continue };
                let value120 = v120.round() as i32;
                if value120 == 0 {
                    continue;
                }
                let axis = match axis {
                    host::Axis::Vertical => Axis::Vertical,
                    host::Axis::Horizontal => Axis::Horizontal,
                };
                out.push(SeatInput::physical(SeatInputKind::Scroll {
                    axis,
                    value120,
                }));
            }
            out
        }
        InputEvent::Keyboard { event } => {
            let state = match event.state() {
                host::KeyState::Pressed => KeyState::Pressed,
                host::KeyState::Released => KeyState::Released,
            };
            let evdev = event.key_code().raw().saturating_sub(XKB_KEYCODE_OFFSET);
            // Always `None` here: this arm exists for the *generic*
            // `InputBackend` the unit tests drive (a synthetic backend has
            // no winit `logical_key` to resolve), so it can only ever
            // resolve the layout-invariant subset from the scancode alone.
            // The real nested path never reaches this arm at all — the
            // winit backend's own event pump resolves `logical_key` to a
            // host keysym itself ([`host_keysym`], called from
            // `crate::backend::winit::NestedWinitEventsApp::window_event`)
            // and calls [`physical_key`] directly with `Some(keysym)`
            // (`crate::backend::winit::NestedState::handle_key`), so a
            // text key flows without ever passing through this
            // scancode-only translation (issue #118).
            physical_key(evdev, None, state)
        }

        // Relative motion. Both deltas are taken from the device rather than
        // derived from one another: `zwp_relative_pointer_v1` defines both,
        // libinput supplies both, and copying one into the other would put a
        // number on the wire the app would be entitled to believe.
        InputEvent::PointerMotion { event } => {
            vec![SeatInput::physical(SeatInputKind::RelativeMotion {
                dx: event.delta_x(),
                dy: event.delta_y(),
                dx_unaccel: event.delta_x_unaccel(),
                dy_unaccel: event.delta_y_unaccel(),
            })]
        }

        // Gestures. Swipe and pinch share `gesture_begin`/`gesture_end` on
        // the wire and differ only in their update, which is exactly how
        // libinput reports them, so the six host events map onto four wire
        // events with no dead argument in any of them.
        InputEvent::GestureSwipeBegin { event } => {
            vec![SeatInput::physical(SeatInputKind::GestureBegin {
                kind: GestureKind::Swipe,
                fingers: event.fingers(),
            })]
        }
        InputEvent::GestureSwipeUpdate { event } => {
            vec![SeatInput::physical(SeatInputKind::GestureSwipeUpdate {
                dx: event.delta_x(),
                dy: event.delta_y(),
            })]
        }
        InputEvent::GestureSwipeEnd { event } => {
            vec![SeatInput::physical(SeatInputKind::GestureEnd {
                kind: GestureKind::Swipe,
                state: gesture_state(event.cancelled()),
            })]
        }
        InputEvent::GesturePinchBegin { event } => {
            vec![SeatInput::physical(SeatInputKind::GestureBegin {
                kind: GestureKind::Pinch,
                fingers: event.fingers(),
            })]
        }
        InputEvent::GesturePinchUpdate { event } => {
            vec![SeatInput::physical(SeatInputKind::GesturePinchUpdate {
                dx: event.delta_x(),
                dy: event.delta_y(),
                // Absolute since this gesture's begin, unlike the three
                // beside it -- carried through verbatim rather than
                // differenced, because reinterpreting it is the one mistake
                // the IDL says an app cannot detect.
                scale: event.scale(),
                rotation: event.rotation(),
            })]
        }
        InputEvent::GesturePinchEnd { event } => {
            vec![SeatInput::physical(SeatInputKind::GestureEnd {
                kind: GestureKind::Pinch,
                state: gesture_state(event.cancelled()),
            })]
        }

        _ => Vec::new(),
    }
}

/// libinput's `cancelled` flag as the wire's [`GestureState`]. One line, and
/// a named one, because the polarity is the kind of thing that inverts
/// silently in a refactor and an app cannot tell a wrongly-committed pinch
/// from a rightly-committed one.
fn gesture_state(cancelled: bool) -> GestureState {
    if cancelled {
        GestureState::Cancelled
    } else {
        GestureState::Completed
    }
}

/// Mint the physical [`SeatInput`]s for one keyboard event, given the host's
/// interpreted keysym when there is one — issue #118's nested-typing fix.
///
/// `host_keysym` is winit's `logical_key` resolved to an X keysym by
/// [`host_keysym`]: the layout-*dependent* interpretation the host already
/// computed, which the core is forbidden to recompute (see
/// [`invariant_keysym`]). Prefer it; fall back to the layout-invariant
/// scancode table for the fixed subset (Escape, Enter, arrows, modifiers…)
/// the core *can* resolve without a keymap; drop the key otherwise, tracing
/// why. The real call site is
/// `crate::backend::winit::NestedState::handle_key`, which calls this
/// directly with the keysym its own winit event pump resolved — bypassing
/// [`intake_physical`]'s scancode-only `Keyboard` arm entirely, which exists
/// only for the generic-`InputBackend` tests below. Kept in this module so
/// the physical-origin minting stays in its one trusted spot (B2), and
/// separated from the pure [`resolve_key_seat`] so the resolution can be
/// unit-tested without minting an origin.
pub(crate) fn physical_key(
    evdev: u32,
    host_keysym: Option<u32>,
    state: KeyState,
) -> Vec<SeatInput> {
    match resolve_key_seat(evdev, host_keysym, state) {
        Some(kind) => vec![SeatInput::physical(kind)],
        None => {
            tracing::trace!(
                evdev,
                "layout-dependent key dropped at intake: no host keysym and not in the \
                 layout-invariant table"
            );
            Vec::new()
        }
    }
}

/// Mint the physical [`SeatInput`]s for one keyboard event a **libinput
/// backend** delivered, resolving it through the core's own keymap
/// (WS-E.3.1, decision D-028(1)).
///
/// The bare-metal twin of [`physical_key`], and the third of the three key
/// paths this crate now has: the synthetic-backend table, nested's
/// `logical_key`, and this. It mints the same `Origin::Physical` tag through
/// the same [`SeatInput::physical`] constructor and produces the same
/// [`KeySource::Device`] pairing identity, so nothing downstream — the
/// router, the chord matchers, the drains, the recorder — can tell which
/// backend a key came from, which is the whole reason the wire needed no
/// change.
///
/// Feature-gated with `CoreKeymap` itself: see `Cargo.toml`'s
/// `session-keymap` block for why nested and headless must not pay for it.
/// `crate::backend::drm`'s libinput `Keyboard` arm is the caller (WS-E.3.2,
/// issue #218).
#[cfg(feature = "session-keymap")]
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the caller is the DRM backend's libinput arm; a `session-keymap`-only \
                  build compiles this path and runs its tests without one"
    )
)]
pub(crate) fn keymap_key(
    keymap: &mut keymap::CoreKeymap,
    evdev: u32,
    state: KeyState,
) -> Vec<SeatInput> {
    match keymap.resolve(evdev, state) {
        Some(keysym) => vec![SeatInput::physical(SeatInputKind::Key {
            source: KeySource::Device(evdev),
            keysym,
            state,
        })],
        None => {
            tracing::trace!(
                evdev,
                "key dropped at intake: the keymap binds no symbol to it"
            );
            Vec::new()
        }
    }
}

/// Mint the physical [`SeatInput`] for one **absolute pointer position a
/// bare-metal backend has already resolved** (WS-E.3.2, issue #218).
///
/// The pointer twin of [`keymap_key`], and it exists for a reason of the same
/// shape. [`intake_physical`] translates `PointerMotion` into the *delta*
/// event `relative_motion` (WS-E.4.2), which is all the host event itself
/// contains; `SeatInputKind::Motion` is an *absolute* view coordinate, so
/// somebody has to hold the accumulated position and clamp it to the output,
/// and that somebody is the backend that owns the output
/// ([`crate::backend::drm`]'s `accumulate_pointer`) — the position it
/// accumulates is also the one it draws the human's cursor sprite at, so the
/// two cannot disagree about where the pointer is. The DRM backend's
/// `PointerMotion` arm mints both halves for one host event, which is what the
/// IDL's "`relative_motion` accompanies `motion`" requires.
///
/// What stays here is the *minting*: `Origin::Physical` is bound by the same
/// private [`SeatInput::physical`] constructor every other intake path uses
/// (B2), so a backend cannot forge a physical origin and nothing downstream
/// can tell which backend a motion came from.
#[cfg(feature = "drm-backend")]
pub(crate) fn physical_motion(x: f64, y: f64) -> Vec<SeatInput> {
    vec![SeatInput::physical(SeatInputKind::Motion { x, y })]
}

/// Resolve one keyboard event to its wire [`SeatInputKind`], preferring the
/// host's interpreted `host_keysym` and falling back to the layout-invariant
/// scancode table ([`invariant_keysym`]). `None` when neither yields a keysym —
/// the key is dropped at intake rather than guessed. Pure and origin-free, so
/// it is exhaustively unit-testable without a synthetic input backend.
fn resolve_key_seat(
    evdev: u32,
    host_keysym: Option<u32>,
    state: KeyState,
) -> Option<SeatInputKind> {
    let keysym = host_keysym.or_else(|| invariant_keysym(evdev))?;
    Some(SeatInputKind::Key {
        // There is a real key behind this, so it pairs by the scancode
        // (D-028(3)) — even in nested mode, where the host's interpretation
        // happens to be stable across a press/release pair today. Making the
        // nested path depend on that stability would be relying on the
        // *host's* keymap not changing under it, which is not the core's to
        // promise.
        source: KeySource::Device(evdev),
        keysym,
        state,
    })
}

/// Resolve winit's interpreted `logical_key` to an X keysym — the #118 seam
/// that lets nested typing carry layout-*dependent* keys (letters, digits,
/// punctuation) at all: producing their keysym needs an interpretation, and
/// under a host the host has already done it — redoing it here would mean
/// disagreeing with the compositor the human is actually typing into. A
/// *bare-metal* core has no such host and does its own, through
/// [`keymap::CoreKeymap`] (D-028); this seam is nested's, and
/// [`invariant_keysym`] is the subset both share.
///
/// Only [`smithay::reexports::winit::keyboard::Key::Character`] resolves to
/// something — a named key (`Enter`, `ArrowLeft`, a bare modifier) or a dead
/// key mid-composition carries no character yet, so those return `None` and
/// [`resolve_key_seat`] falls back to the layout-invariant table instead
/// (which is exactly how Escape/Enter/arrows keep working for dead-man
/// regardless of which path resolved them).
///
/// Called from `crate::backend::winit::NestedWinitEventsApp::window_event`,
/// the one real producer of a `logical_key` in this crate (a synthetic test
/// backend has none to offer).
pub(crate) fn host_keysym(key: &smithay::reexports::winit::keyboard::Key) -> Option<u32> {
    match key {
        smithay::reexports::winit::keyboard::Key::Character(text) => {
            char_keysym(text.chars().next()?)
        }
        _ => None,
    }
}

/// One Unicode character to its X11 keysym, by the `keysymdef.h` convention:
/// codepoints below `0x100` (ASCII + the Latin-1 supplement) are their own
/// keysym; every other codepoint is `0x0100_0000 | codepoint` (the "Unicode
/// keysym" range every X11/xkbcommon implementation recognizes). Control
/// characters (`0x00`-`0x1f`, `0x7f`) are not valid keysyms under either
/// convention and are dropped defensively — winit should never surface one
/// through `Key::Character` (they arrive as `Key::Named` instead), but
/// encoding one verbatim would mint a keysym that means something else
/// entirely rather than the intended key.
///
/// A multi-character `Key::Character` (composed input, ligatures) is
/// approximated by its first character in [`host_keysym`], which is the
/// only caller — documented there, not here, since this function only ever
/// sees one `char` at a time.
fn char_keysym(ch: char) -> Option<u32> {
    if ch.is_control() {
        return None;
    }
    let code = ch as u32;
    Some(if code < 0x100 {
        code
    } else {
        0x0100_0000 | code
    })
}

/// The layout-*invariant* evdev-scancode → keysym subset: editing,
/// navigation, function, and modifier keys whose meaning is the same under
/// every keyboard layout. This is a fixed constant table (kernel
/// `input-event-codes.h` on the left, X11 `keysymdef.h` on the right) —
/// the keyboard analogue of the evdev `BTN_*` codes the button path
/// already speaks — **not** keymap interpretation. It is what every backend
/// can name *without* a keymap, which is a narrower and more durable claim
/// than the one the IDL used to make (D-028 corrected "no keymap
/// interpretation happens inside the core": a bare-metal backend does its
/// own, through [`keymap::CoreKeymap`]). This table is what the core's own
/// chords are drawn from precisely because it is the part no keymap moves.
///
/// Layout-*dependent* keys (letters, digits, punctuation) return `None`
/// here. Under a host, [`host_keysym`] resolves those and
/// [`resolve_key_seat`] prefers it over this table; on bare metal
/// [`keymap_key`] resolves them instead. This table never guesses at one. This subset is chosen so
/// the consent / revocation paths never depend on the host resolving
/// anything: Escape — P1.7.3's hold-Esc chord — is layout-invariant and
/// always translates, from either path, and so are the two Super keys
/// WS-E.1.7's attention chord is drawn from (`125`/`126` below).
///
/// `KEY_SYSRQ` (99 → `XK_Print`) joined at WS-E.2.4 (issue #216) and belongs
/// here **on this table's own terms**, not as a favour to the screenshot key:
/// PrintScreen is one physical key at one scancode on every layout this table
/// serves, and its keysym is not a function of any keymap. It is the only key
/// added to the table since the core's first chord, and it is worth saying why
/// the bar is that high — every row here is a key the core can name without a
/// keymap, so a row that is *not* layout-invariant would let a chord mean one
/// thing on a US layout and another on a Turkish one, silently.
///
/// `pub(crate)` for the two core-owned chord vocabularies, which assert against
/// this table rather than restating it: [`crate::deadman::Chord::parse`]
/// through [`keysym_is_intakeable`], and
/// [`crate::attention::AttentionChord::parse`] through both — it carries the
/// scancode as well, so it checks this row itself.
pub(crate) fn invariant_keysym(evdev_code: u32) -> Option<u32> {
    Some(match evdev_code {
        1 => 0xff1b,                           // KEY_ESC        -> XK_Escape
        14 => 0xff08,                          // KEY_BACKSPACE  -> XK_BackSpace
        15 => 0xff09,                          // KEY_TAB        -> XK_Tab
        28 => 0xff0d,                          // KEY_ENTER      -> XK_Return
        29 => 0xffe3,                          // KEY_LEFTCTRL   -> XK_Control_L
        42 => 0xffe1,                          // KEY_LEFTSHIFT  -> XK_Shift_L
        54 => 0xffe2,                          // KEY_RIGHTSHIFT -> XK_Shift_R
        56 => 0xffe9,                          // KEY_LEFTALT    -> XK_Alt_L
        57 => 0x0020,                          // KEY_SPACE      -> XK_space
        58 => 0xffe5,                          // KEY_CAPSLOCK   -> XK_Caps_Lock
        59..=68 => 0xffbe + (evdev_code - 59), // KEY_F1..F10    -> XK_F1..XK_F10
        87 => 0xffc8,                          // KEY_F11        -> XK_F11
        88 => 0xffc9,                          // KEY_F12        -> XK_F12
        96 => 0xff8d,                          // KEY_KPENTER    -> XK_KP_Enter
        97 => 0xffe4,                          // KEY_RIGHTCTRL  -> XK_Control_R
        99 => 0xff61,                          // KEY_SYSRQ      -> XK_Print
        100 => 0xffea,                         // KEY_RIGHTALT   -> XK_Alt_R
        102 => 0xff50,                         // KEY_HOME       -> XK_Home
        103 => 0xff52,                         // KEY_UP         -> XK_Up
        104 => 0xff55,                         // KEY_PAGEUP     -> XK_Prior
        105 => 0xff51,                         // KEY_LEFT       -> XK_Left
        106 => 0xff53,                         // KEY_RIGHT      -> XK_Right
        107 => 0xff57,                         // KEY_END        -> XK_End
        108 => 0xff54,                         // KEY_DOWN       -> XK_Down
        109 => 0xff56,                         // KEY_PAGEDOWN   -> XK_Next
        110 => 0xff63,                         // KEY_INSERT     -> XK_Insert
        111 => 0xffff,                         // KEY_DELETE     -> XK_Delete
        125 => 0xffeb,                         // KEY_LEFTMETA   -> XK_Super_L
        126 => 0xffec,                         // KEY_RIGHTMETA  -> XK_Super_R
        // ---- The XF86 media and brightness block (WS-E.4.3, issue #223) ----
        //
        // These belong here on THIS TABLE'S OWN CRITERION, not as a favour to a
        // laptop: each is one physical key at one scancode whose keysym is not
        // a function of any layout. Verified structurally rather than assumed --
        // `grep -lE 'XF86Audio(Mute|RaiseVolume|LowerVolume)'
        // /usr/share/X11/xkb/symbols/*` returns only `inet` and two vendor
        // files; `us`, `tr`, `de`, `fr`, `gb` and `ru` bind none of them. They
        // come from the MODEL/inet compatibility component, which is precisely
        // what "layout-invariant" means.
        //
        // **Where they actually matter**, which is narrower than "media keys
        // were dropped": on bare metal WITH `--keymap` these resolve through
        // `keymap::CoreKeymap` and xkb's own `inet(evdev)` already delivered
        // them, so nothing was being lost there. The two paths these rows fix
        // are bare metal WITHOUT `--keymap` (which falls back to this table) and
        // NESTED, where winit reports them as `Key::Named` so `host_keysym`
        // answers `None` and `resolve_key_seat` falls through to here.
        //
        // **And the honest residual, which is now half the size it was**
        // (D-041, issue #303). The owner decided that the core writes
        // `/sys/class/backlight`, so the two BRIGHTNESS rows below are no
        // longer delivered on a session that passed `--backlight`: they are
        // consumed by [`crate::backlight`] and turned into a bounded write.
        // The rows stay here because that is where the keysym comes from --
        // the gate matches on it, and `backlight::tests::
        // the_gate_matches_the_scancode_table` pins the two together.
        //
        // The rest of this block is unchanged and the residual is unchanged
        // for it:
        // **The volume keys are still delivered to an app that cannot act on them.**
        // There is no one-file sysfs equivalent for a mixer and every route to
        // one runs through a sound server -- a bus or
        // socket client inside the TCB, which is the dependency D-033(4)
        // refused for logind. Volume actuation stays DEFERRED, reopened by a
        // shell client holding a named verb (WS-E Stage 2's design, which
        // D-039 makes newly plausible) or by an owner decision of the kind
        // D-041 records for the backlight. And on a session WITHOUT
        // `--backlight` -- which is every nested and headless one, where the
        // flag is refused -- the brightness rows behave exactly as they did
        // before: delivered, and unactionable.
        113 => 0x1008ff12, // KEY_MUTE          -> XF86AudioMute
        114 => 0x1008ff11, // KEY_VOLUMEDOWN    -> XF86AudioLowerVolume
        115 => 0x1008ff13, // KEY_VOLUMEUP      -> XF86AudioRaiseVolume
        163 => 0x1008ff17, // KEY_NEXTSONG      -> XF86AudioNext
        // `XF86AudioPlay`, NOT `XF86MediaPlayPause` (0x1008_10a4): the live
        // binding in `symbols/inet` is `key <I172> { [ XF86AudioPlay,
        // XF86AudioPause ] }` and the MediaPlayPause alternative in the same
        // file is commented out, so this is what every other desktop delivers.
        // The level-2 divergence is precedented rather than new -- this table is
        // single-level, so Shift+PlayPause stays `XF86AudioPlay` here while a
        // keymap build resolves `XF86AudioPause`, exactly as KEY_TAB already
        // stays `XK_Tab` where xkb says `ISO_Left_Tab`.
        164 => 0x1008ff14, // KEY_PLAYPAUSE     -> XF86AudioPlay
        165 => 0x1008ff16, // KEY_PREVIOUSSONG  -> XF86AudioPrev
        166 => 0x1008ff15, // KEY_STOPCD        -> XF86AudioStop
        224 => 0x1008ff03, // KEY_BRIGHTNESSDOWN-> XF86MonBrightnessDown
        225 => 0x1008ff02, // KEY_BRIGHTNESSUP  -> XF86MonBrightnessUp
        // The highest code `keysym_is_intakeable`'s `0..256` scan still reaches,
        // and worth the note because 249 would not be. If a row above 255 is
        // ever wanted, that scan has to widen with it or the chord vocabulary
        // and the table silently stop agreeing.
        248 => 0x1008ffb2, // KEY_MICMUTE       -> XF86AudioMicMute
        //
        // **Excluded on POLICY, not on this table's criterion**: KEY_POWER
        // (116), KEY_SLEEP (142), KEY_WAKEUP (143), KEY_SWITCHVIDEOMODE (227)
        // and KEY_RFKILL (247) are all in range and all layout-invariant, so the
        // criterion above would admit them. #223's key decision is that suspend
        // and power policy belong to systemd-logind rather than to this core, so
        // adding those rows would forward a system-power key into a confined
        // app's shim seat instead of leaving it where the policy lives. That is
        // why the refusal is written down here rather than left as an absence.
        //
        // **DEFERRED, with the evidence named**: KEY_KBDILLUMTOGGLE (228),
        // KEY_KBDILLUMDOWN (229) and KEY_KBDILLUMUP (230) are in range and
        // layout-invariant and would pass on the criterion. They are left out
        // because the target machine has no keyboard backlight -- `/sys/class/
        // leds` on it holds only capslock/numlock/scrolllock, the NIC's and the
        // wifi phy's -- so adding them would be three rows nothing could ever
        // press. A `*kbd_backlight*` entry under `/sys/class/leds` on the
        // machine this runs on is the evidence that reopens them.
        _ => return None,
    })
}

/// Whether nested intake can ever produce `keysym` — i.e. whether some evdev
/// scancode maps to it through [`invariant_keysym`].
///
/// Exists for several callers and one hazard, and the extra callers are why the
/// hazard is stated as a class rather than as one switch:
/// [`crate::deadman::Chord::parse`] validates the configured dead-man chord
/// against it, and [`crate::attention::AttentionChord::parse`] the attention
/// chord (WS-E.1.7), so a session can never come up with a core-owned chord
/// whose key intake silently drops.
///
/// # What it does NOT promise on bare metal (WS-E.3.1)
///
/// This answers about [`invariant_keysym`] — the fixed scancode table the
/// NESTED and headless paths use. A `session-keymap` build resolves keys
/// through the operator's keymap instead, where a keysym is whatever the layout
/// says, so "intakeable" here is not a statement about what that build will
/// deliver. Answering `true` and then never firing is exactly the late
/// discovery this function exists to prevent, so the bare-metal half is checked
/// somewhere else and by construction:
/// [`crate::input::keymap::CoreKeymap`] refuses to be built from a keymap that
/// does not deliver every [`crate::chord::Trigger::VOCABULARY`] entry. Two
/// checks, two paths, neither standing in for the other. Asking the real table rather than restating
/// it is the whole point — a hand-maintained copy of the mapping would be free
/// to drift, and the symptom of that drift is a chord that never fires, which
/// is the worst possible thing to discover late. The two failures are not the
/// same size (a dead off-switch versus a focus change that never happens) and
/// the modules keep them apart; the *check* is identical and lives here once.
///
/// The scan is over the byte-wide evdev keycode space the kernel defines
/// (`input-event-codes.h` keeps `KEY_*` under 256, and [`invariant_keysym`]'s
/// table is entirely within it); it runs once per process, at argument
/// parsing.
pub(crate) fn keysym_is_intakeable(keysym: u32) -> bool {
    (0u32..256).any(|code| invariant_keysym(code) == Some(keysym))
}

/// The core's own keymap (WS-E.3.1, issue #217; decision D-028) — the state
/// machine that turns an evdev scancode into a keysym on a backend that has
/// no host to have done it already. Gated on `session-keymap` in full,
/// because linking `libxkbcommon.so.0` into the TCB is exactly what a nested
/// or headless build must not do; see the feature's block in `Cargo.toml`
/// for the measured cost and the two structural bounds on what it may read.
#[cfg(feature = "session-keymap")]
pub(crate) mod keymap;

/// A synthetic Smithay input backend, shared by this module's unit tests and
/// by the `physical-input-injector` build — see the module's own docs for why
/// there is exactly one copy.
#[cfg(any(test, feature = "physical-input-injector"))]
pub(crate) mod synthetic;

/// The `physical-input-injector` channel (issue #212): the line vocabulary a
/// harness one process boundary away uses to make physical-tagged input happen
/// in a headless core. Feature-gated in full; see [`injector`]'s docs and the
/// feature's own block in `Cargo.toml` for what it widens.
#[cfg(any(test, feature = "physical-input-injector"))]
pub(crate) mod injector;

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::os::fd::AsFd;
    use std::rc::Rc;

    use vitrin_ipc::{Connection, TransportError};
    use vitrin_mock_shim::{MockShim, SeatEvent};

    use super::synthetic::{
        SyntheticButton, SyntheticGestureBegin, SyntheticGestureEnd, SyntheticHost, SyntheticKey,
        SyntheticMotion, SyntheticPinchUpdate, SyntheticRelativeMotion, SyntheticScroll,
        SyntheticSwipeUpdate,
    };
    use super::*;
    use crate::scene::Scene;
    use crate::shim::{ShimConfig, ShimServer};

    const VIEW: (i32, i32) = (100, 80);

    // ------------------------------------------------------------------
    // Chord fixtures for `crate::deadman`'s tests
    //
    // These live here, not there, for the reason the P1.7.2 section below
    // already records: [`SeatInput::physical`] is private to this module by
    // design (B2), so this is the only place in the crate where a
    // physical-origin event can be minted at all. A module that needs to
    // model "the human pressed a key" borrows it from here rather than
    // gaining the ability to forge an origin tag.
    // ------------------------------------------------------------------

    /// **The general form**: one physical-origin event of any kind, for a
    /// module whose fixtures are not a fixed pair of chord keys
    /// ([`crate::lock`] types a whole alphabet at its gate, and
    /// [`crate::chord`]'s consumers hold arbitrary modifier sets).
    ///
    /// Same warrant as the chord fixtures below and the same reason it lives
    /// here: [`SeatInput::physical`] is private to this module, so this is the
    /// only place in the crate that can mint a physical origin tag at all. A
    /// module borrows one from here rather than gaining the ability to forge
    /// one, and the production path stays what it was — the nested backend's
    /// intake, and nothing else.
    pub(crate) fn physical_for_test(kind: SeatInputKind) -> SeatInput {
        SeatInput::physical(kind)
    }

    /// XK_Escape — the default dead-man chord.
    pub(crate) const CHORD_KEYSYM: u32 = 0xff1b;
    /// The scancode that keysym comes off — `KEY_ESC`. Named because the
    /// fixtures below now carry a [`KeySource::Device`], and a scancode that
    /// did not match its keysym would make every one of them a fiction.
    const CHORD_EVDEV: u32 = 1;
    /// XK_Return — a layout-invariant key that is *not* the chord.
    const NON_CHORD_KEYSYM: u32 = 0xff0d;
    /// `KEY_ENTER`, the scancode `NON_CHORD_KEYSYM` comes off.
    const NON_CHORD_EVDEV: u32 = 28;

    /// The human pressing the dead-man chord key.
    pub(crate) fn chord_press() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
            source: KeySource::Device(CHORD_EVDEV),
            keysym: CHORD_KEYSYM,
            state: KeyState::Pressed,
        })
    }

    /// The human releasing it.
    pub(crate) fn chord_release() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
            source: KeySource::Device(CHORD_EVDEV),
            keysym: CHORD_KEYSYM,
            state: KeyState::Released,
        })
    }

    /// An **agent** actuating the chord key: must never arm the human's
    /// switch.
    pub(crate) fn emulated_chord_press() -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: CHORD_KEYSYM,
            state: KeyState::Pressed,
        })
    }

    /// An agent actuating the chord key's release: must never disarm a hold
    /// the human has in progress.
    pub(crate) fn emulated_chord_release() -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: CHORD_KEYSYM,
            state: KeyState::Released,
        })
    }

    /// Agent pointer traffic, for flooding an armed hold.
    pub(crate) fn emulated_motion() -> SeatInput {
        SeatInput::emulated(SeatInputKind::Motion { x: 1.0, y: 2.0 })
    }

    /// A physical key that is not the chord.
    pub(crate) fn other_key_press() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
            source: KeySource::Device(NON_CHORD_EVDEV),
            keysym: NON_CHORD_KEYSYM,
            state: KeyState::Pressed,
        })
    }

    /// Its release.
    pub(crate) fn other_key_release() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
            source: KeySource::Device(NON_CHORD_EVDEV),
            keysym: NON_CHORD_KEYSYM,
            state: KeyState::Released,
        })
    }

    fn motion_ev(x: f64, y: f64) -> InputEvent<SyntheticHost> {
        InputEvent::PointerMotionAbsolute {
            event: SyntheticMotion { x, y },
        }
    }

    fn button_ev(code: u32, state: host::ButtonState) -> InputEvent<SyntheticHost> {
        InputEvent::PointerButton {
            event: SyntheticButton { code, state },
        }
    }

    fn key_ev(evdev: u32, state: host::KeyState) -> InputEvent<SyntheticHost> {
        InputEvent::Keyboard {
            event: SyntheticKey { evdev, state },
        }
    }

    // The version-2 host events (WS-E.4.2, issue #222). Constructed through
    // the same synthetic `InputBackend` the five above use, so every one of
    // these tests enters the core at `intake_physical` -- the single point
    // where `Origin::Physical` can be minted -- and never at a weaker seam.

    fn relative_ev(
        dx: f64,
        dy: f64,
        dx_unaccel: f64,
        dy_unaccel: f64,
    ) -> InputEvent<SyntheticHost> {
        InputEvent::PointerMotion {
            event: SyntheticRelativeMotion {
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
            },
        }
    }

    fn swipe_begin_ev(fingers: u32) -> InputEvent<SyntheticHost> {
        InputEvent::GestureSwipeBegin {
            event: SyntheticGestureBegin { fingers },
        }
    }

    fn swipe_update_ev(dx: f64, dy: f64) -> InputEvent<SyntheticHost> {
        InputEvent::GestureSwipeUpdate {
            event: SyntheticSwipeUpdate { dx, dy },
        }
    }

    fn swipe_end_ev(cancelled: bool) -> InputEvent<SyntheticHost> {
        InputEvent::GestureSwipeEnd {
            event: SyntheticGestureEnd { cancelled },
        }
    }

    fn pinch_begin_ev(fingers: u32) -> InputEvent<SyntheticHost> {
        InputEvent::GesturePinchBegin {
            event: SyntheticGestureBegin { fingers },
        }
    }

    fn pinch_update_ev(dx: f64, dy: f64, scale: f64, rotation: f64) -> InputEvent<SyntheticHost> {
        InputEvent::GesturePinchUpdate {
            event: SyntheticPinchUpdate {
                dx,
                dy,
                scale,
                rotation,
            },
        }
    }

    fn pinch_end_ev(cancelled: bool) -> InputEvent<SyntheticHost> {
        InputEvent::GesturePinchEnd {
            event: SyntheticGestureEnd { cancelled },
        }
    }

    /// Route one host event through a router and hand back what the wire
    /// would carry, or `None` if nothing was delivered.
    ///
    /// `intake_physical` yields a `Vec`, and every version-2 class yields
    /// exactly one element; asserting that here rather than in each test is
    /// what keeps a translation that silently started emitting two from
    /// passing as one.
    fn route_host<H: PreemptionHook>(
        router: &mut InputRouter<H>,
        event: &InputEvent<SyntheticHost>,
        view: ViewGeometry,
        surface: Option<(u32, u32)>,
    ) -> Option<SeatDelivery> {
        let mut inputs = intake_physical(event, (view.output().0 as i32, view.output().1 as i32));
        assert_eq!(
            inputs.len(),
            1,
            "a version-2 host event translates to exactly one seat input"
        );
        let input = inputs.remove(0);
        assert_eq!(
            input.origin(),
            Origin::Physical,
            "intake is the one place the physical tag is minted (B2)"
        );
        router.route_physical(input, view, surface)
    }

    // ------------------------------------------------------------------
    // Intake: origin binding + translation
    // ------------------------------------------------------------------

    #[test]
    fn intake_binds_physical_origin_at_the_point_of_entry() {
        // Every translated host event carries Origin::Physical from the
        // constructor onward (B2: applied AT intake, never inferred
        // downstream) — and the payloads survive translation exactly.
        let events = [
            intake_physical(&motion_ev(12.5, 8.0), VIEW),
            intake_physical(&button_ev(0x110, host::ButtonState::Pressed), VIEW),
            intake_physical(&key_ev(28, host::KeyState::Pressed), VIEW),
        ];
        let flat: Vec<&SeatInput> = events.iter().flatten().collect();
        assert_eq!(flat.len(), 3);
        for input in &flat {
            assert_eq!(input.origin(), Origin::Physical);
        }
        assert_eq!(flat[0].kind(), &SeatInputKind::Motion { x: 12.5, y: 8.0 });
        assert_eq!(
            flat[1].kind(),
            &SeatInputKind::Button {
                button: 0x110,
                state: ButtonState::Pressed,
            }
        );
        assert_eq!(
            flat[2].kind(),
            &SeatInputKind::Key {
                source: KeySource::Device(28),
                keysym: 0xff0d, // KEY_ENTER -> XK_Return
                state: KeyState::Pressed,
            }
        );
    }

    #[test]
    fn intake_translates_wheel_and_pixel_scroll_to_value120() {
        // A discrete wheel reports v120 directly; a diagonal wheel event
        // yields one tagged event per axis, vertical first.
        let wheel = InputEvent::PointerAxis::<SyntheticHost> {
            event: SyntheticScroll {
                v120: (Some(-120.0), Some(240.0)),
                pixels: (None, None),
            },
        };
        let out = intake_physical(&wheel, VIEW);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].kind(),
            &SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: -120,
            }
        );
        assert_eq!(
            out[1].kind(),
            &SeatInputKind::Scroll {
                axis: Axis::Horizontal,
                value120: 240,
            }
        );
        for input in &out {
            assert_eq!(input.origin(), Origin::Physical);
        }

        // Continuous (pixel) scroll converts at 15 px per notch: 30 px ->
        // 240; a sub-rounding residue (0.05 px) yields no event at all
        // rather than a zero-valued one.
        let pixels = InputEvent::PointerAxis::<SyntheticHost> {
            event: SyntheticScroll {
                v120: (None, None),
                pixels: (Some(30.0), Some(0.05)),
            },
        };
        let out = intake_physical(&pixels, VIEW);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].kind(),
            &SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: 240,
            }
        );
    }

    #[test]
    fn intake_drops_layout_dependent_keys_and_maps_invariant_ones() {
        // KEY_A's keysym depends on the active layout, which only the host
        // knows — dropped until interpreted host keysyms are available
        // (see invariant_keysym docs). Escape is layout-invariant and must
        // always translate: P1.7.3's hold-Esc revocation depends on it.
        assert!(intake_physical(&key_ev(30, host::KeyState::Pressed), VIEW).is_empty());
        let esc = intake_physical(&key_ev(1, host::KeyState::Released), VIEW);
        assert_eq!(
            esc[0].kind(),
            &SeatInputKind::Key {
                source: KeySource::Device(1),
                keysym: 0xff1b,
                state: KeyState::Released,
            }
        );
    }

    #[test]
    fn resolve_prefers_the_host_keysym_then_falls_back_to_the_invariant_table() {
        // The #118 seam. `resolve_key_seat`/`physical_key` prefer the host's
        // interpreted keysym — winit's `logical_key`, resolved by
        // `host_keysym` and supplied by `crate::backend::winit`'s owned
        // event pump — and fall back to the layout-invariant scancode table
        // only when there is none.

        // A host keysym wins for a layout-*dependent* scancode (KEY_A = 30),
        // which is exactly the text-key path the wiring turns on.
        assert_eq!(
            physical_key(30, Some(0x0061), KeyState::Pressed)[0].kind(),
            &SeatInputKind::Key {
                source: KeySource::Device(30),
                keysym: 0x0061,
                state: KeyState::Pressed,
            },
        );
        // It overrides the invariant table for the same scancode, too: once the
        // host has interpreted a key, its interpretation is authoritative.
        assert_eq!(
            resolve_key_seat(1, Some(0x0041), KeyState::Pressed),
            Some(SeatInputKind::Key {
                source: KeySource::Device(1),
                keysym: 0x0041,
                state: KeyState::Pressed,
            }),
        );
        // With no host keysym, the invariant table still resolves Escape...
        assert_eq!(
            resolve_key_seat(1, None, KeyState::Released),
            Some(SeatInputKind::Key {
                source: KeySource::Device(1),
                keysym: 0xff1b,
                state: KeyState::Released,
            }),
        );
        // ...and a layout-dependent key is dropped when winit genuinely has
        // no interpretation to offer (e.g. a bare modifier chord with no
        // character result) — `physical_key`/`resolve_key_seat` never guess.
        assert_eq!(resolve_key_seat(30, None, KeyState::Pressed), None);
        assert!(physical_key(30, None, KeyState::Pressed).is_empty());
    }

    #[test]
    fn a_text_key_given_a_host_keysym_reaches_the_app_as_physical_input() {
        // The delivery half of #118: given the interpreted keysym
        // `crate::backend::winit`'s owned pump resolves via `host_keysym`
        // and passes to `physical_key` directly (bypassing `intake_physical`
        // — see both functions' docs), the app receives the text key over
        // the real wire, origin=physical.
        let _fd = crate::capture::tests::fd_lock();
        let (server, _scene, mut core, mut mock) = wire_setup();
        let view: ViewGeometry = (VIEW_W, VIEW_H).into();
        let surface = Some(view.usable());
        let mut router = router();

        // KEY_H (scancode 35) is layout-dependent — dropped with no host
        // keysym; given winit's `logical_key` ('h' = 0x0068) it flows.
        for input in physical_key(35, Some(0x0068), KeyState::Pressed) {
            if let Some(delivery) = router.route_physical(input, view, surface) {
                server
                    .deliver_seat_event(&delivery, &mut |frame| core.send_message(frame, None))
                    .expect("send");
            }
        }
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Key {
                keysym: 0x0068,
                state: KeyState::Pressed,
                origin: Origin::Physical,
            },
            "a text key, given the host keysym, must reach the app over the wire as physical input",
        );
    }

    #[test]
    fn invariant_keysym_covers_the_documented_subset() {
        // Spot checks against keysymdef.h, including the F-key range math.
        assert_eq!(invariant_keysym(1), Some(0xff1b)); // Escape
        assert_eq!(invariant_keysym(28), Some(0xff0d)); // Return
        assert_eq!(invariant_keysym(57), Some(0x0020)); // space
        assert_eq!(invariant_keysym(59), Some(0xffbe)); // F1
        assert_eq!(invariant_keysym(68), Some(0xffc7)); // F10
        assert_eq!(invariant_keysym(87), Some(0xffc8)); // F11
        assert_eq!(invariant_keysym(88), Some(0xffc9)); // F12
        assert_eq!(invariant_keysym(99), Some(0xff61)); // KEY_SYSRQ -> Print
        assert_eq!(invariant_keysym(105), Some(0xff51)); // Left
        assert_eq!(invariant_keysym(125), Some(0xffeb)); // Super_L
                                                         // Layout-dependent: letters, digits, punctuation.
        assert_eq!(invariant_keysym(30), None); // KEY_A
        assert_eq!(invariant_keysym(2), None); // KEY_1
        assert_eq!(invariant_keysym(51), None); // KEY_COMMA
    }

    /// **The XF86 media and brightness block** (WS-E.4.3, issue #223), pinned
    /// value by value against `linux/input-event-codes.h` and
    /// `X11/XF86keysym.h`.
    ///
    /// Spelled out rather than spot-checked, because every one of these is a
    /// two-number pair a typo would silently corrupt into some *other* real
    /// keysym: `0x1008ff11` and `0x1008ff13` are volume down and up, and
    /// swapping them gives an app a working key that does the opposite thing.
    ///
    /// The three assertions after the table are the ones that keep this a
    /// principled extension rather than a list somebody grew:
    ///
    /// * every row is inside the `0..256` window `keysym_is_intakeable` scans,
    ///   so the chord vocabularies that consult it see a consistent table;
    /// * no row is layout-dependent -- the negatives above (`KEY_A`, `KEY_1`,
    ///   `KEY_COMMA`) still answer `None`;
    /// * the system-power keys are **excluded on policy**, not on the table's
    ///   criterion, and that refusal is asserted so it cannot be quietly
    ///   reversed by someone applying the criterion mechanically.
    #[test]
    fn the_xf86_media_block_matches_the_kernel_and_x11_headers() {
        for (code, keysym, name) in [
            (113u32, 0x1008ff12u32, "KEY_MUTE -> XF86AudioMute"),
            (114, 0x1008ff11, "KEY_VOLUMEDOWN -> XF86AudioLowerVolume"),
            (115, 0x1008ff13, "KEY_VOLUMEUP -> XF86AudioRaiseVolume"),
            (163, 0x1008ff17, "KEY_NEXTSONG -> XF86AudioNext"),
            (164, 0x1008ff14, "KEY_PLAYPAUSE -> XF86AudioPlay"),
            (165, 0x1008ff16, "KEY_PREVIOUSSONG -> XF86AudioPrev"),
            (166, 0x1008ff15, "KEY_STOPCD -> XF86AudioStop"),
            (
                224,
                0x1008ff03,
                "KEY_BRIGHTNESSDOWN -> XF86MonBrightnessDown",
            ),
            (225, 0x1008ff02, "KEY_BRIGHTNESSUP -> XF86MonBrightnessUp"),
            (248, 0x1008ffb2, "KEY_MICMUTE -> XF86AudioMicMute"),
        ] {
            assert_eq!(invariant_keysym(code), Some(keysym), "{name}");
            assert!(
                code < 256,
                "{name} is outside the 0..256 window `keysym_is_intakeable` scans, so the \
                 table and the chord vocabularies would stop agreeing about it"
            );
            assert!(
                keysym_is_intakeable(keysym),
                "{name} must be reachable through the same predicate the dead-man and \
                 attention chords validate against"
            );
        }

        // **KEY_PLAYPAUSE is XF86AudioPlay, not XF86MediaPlayPause.** The live
        // binding is `symbols/inet`'s `key <I172> { [ XF86AudioPlay,
        // XF86AudioPause ] }`; the MediaPlayPause alternative in the same file
        // is commented out. Getting this one wrong produces a keysym no other
        // desktop sends, which is the quietest possible failure.
        assert_ne!(invariant_keysym(164), Some(0x1008_10a4));

        // **Excluded on POLICY, not on the criterion** (#223's key decision:
        // suspend and power policy are logind's, not this core's). All five are
        // in range and layout-invariant, so the criterion alone would admit
        // them -- and admitting them would forward a system-power key into a
        // confined app's shim seat instead of leaving it where the policy lives.
        for (code, name) in [
            (116u32, "KEY_POWER"),
            (142, "KEY_SLEEP"),
            (143, "KEY_WAKEUP"),
            (227, "KEY_SWITCHVIDEOMODE"),
            (247, "KEY_RFKILL"),
        ] {
            assert_eq!(
                invariant_keysym(code),
                None,
                "{name} must NOT be in this table. It passes the layout-invariance criterion, \
                 which is exactly why the refusal is a decision rather than an omission: \
                 delegating power and lid policy to systemd-logind means this core does not \
                 hand those keys to a confined app either"
            );
        }

        // **DEFERRED**, with the evidence that reopens it named: the keyboard
        // backlight keys pass the criterion and are left out because the target
        // machine has no keyboard backlight at all (`/sys/class/leds` holds only
        // the lock LEDs, the NIC's and the wifi phy's). A `*kbd_backlight*`
        // entry there is what would reopen them.
        for code in [228u32, 229, 230] {
            assert_eq!(invariant_keysym(code), None);
        }
    }

    /// **The media block widens no chord vocabulary**, which is the property
    /// that keeps it a table extension rather than a configuration change.
    ///
    /// Adding rows widens [`keysym_is_intakeable`], which
    /// `deadman::Chord::parse` and `attention::AttentionChord::parse` consult —
    /// but `chord::Trigger::parse` *also* requires the name to be in
    /// `Trigger::VOCABULARY`, which is unchanged. So no new chord became
    /// configurable, no operator's `--keymap` became invalid through
    /// `CoreKeymap`'s `DisarmsChord` check, and nothing an operator can type on
    /// a command line changed meaning.
    #[test]
    fn the_media_block_makes_no_new_chord_configurable() {
        for name in ["xf86audiomute", "volumeup", "brightnessup", "playpause"] {
            assert!(
                crate::chord::Trigger::parse(name).is_err(),
                "`{name}` became a configurable chord trigger: the media block is a delivery \
                 table, not a vocabulary, and a gesture nobody designed must not become \
                 spellable by widening it"
            );
        }
        // ...and the keys that WERE in the vocabulary still are, so this is not
        // passing because `Trigger::parse` broke.
        assert!(crate::chord::Trigger::parse("delete").is_ok());
        assert!(crate::chord::Trigger::parse("print").is_ok());
    }

    /// WS-E.2.4 (issue #216): the screenshot chord's trigger key is reachable
    /// from **both** halves of the table's contract — the scancode resolves,
    /// and intake can actually deliver the keysym.
    ///
    /// The second half is the one that matters and the one that is easy to get
    /// wrong by adding a `Trigger::VOCABULARY` row and forgetting this table:
    /// `chord::Trigger::parse` refuses a key `keysym_is_intakeable` says no to,
    /// so a session configured with such a chord would refuse to start rather
    /// than come up with a gesture that silently never fires — but only because
    /// this row exists.
    #[test]
    fn the_screenshot_trigger_is_intakeable() {
        assert_eq!(invariant_keysym(99), Some(0xff61));
        assert!(keysym_is_intakeable(0xff61));
    }

    // ------------------------------------------------------------------
    // #118: resolving winit's `logical_key` to a host keysym
    // ------------------------------------------------------------------
    //
    // `winit::keyboard::Key` is a plain public enum -- unlike the
    // `KeyEvent` that carries it in a real event, it can be constructed
    // right here with no display, no backend, and no
    // `crate::backend::winit` event pump at all. These tests drive
    // `host_keysym` directly with it, which is as close to "feed the real
    // nested backend's winit glue a keystroke" as a display-free test gets;
    // the rest of the path from there (`physical_key`/`resolve_key_seat`
    // preferring the resolved keysym, delivery over the wire) is already
    // covered above and unchanged by this function's existence.

    use smithay::reexports::winit::keyboard::{Key, NamedKey};

    #[test]
    fn host_keysym_resolves_ascii_characters_to_their_own_codepoint() {
        // ASCII/Latin-1 keysyms equal their Unicode codepoint by
        // construction (keysymdef.h) -- this is the exact case that reaches
        // the app in `a_text_key_given_a_host_keysym_reaches_the_app_as_physical_input`
        // above ('h' = 0x0068).
        assert_eq!(host_keysym(&Key::Character("h".into())), Some(0x0068));
        assert_eq!(host_keysym(&Key::Character("H".into())), Some(0x0048));
        assert_eq!(host_keysym(&Key::Character("1".into())), Some(0x0031));
        assert_eq!(host_keysym(&Key::Character(",".into())), Some(0x002c));
        // Space is layout-invariant too, but `host_keysym` resolves it just
        // the same -- the two paths agree rather than disagree, which
        // `resolve_key_seat` relies on (the host keysym takes priority).
        assert_eq!(host_keysym(&Key::Character(" ".into())), Some(0x0020));
    }

    #[test]
    fn host_keysym_resolves_latin1_supplement_and_wider_unicode() {
        // 'é' (U+00E9) is within the Latin-1 supplement: keysym ==
        // codepoint, same as ASCII.
        assert_eq!(host_keysym(&Key::Character("é".into())), Some(0x00e9));
        // '€' (U+20AC) is outside Latin-1: the 24-bit-Unicode convention
        // applies (0x0100_0000 | codepoint).
        assert_eq!(host_keysym(&Key::Character("€".into())), Some(0x0100_20ac));
        // An emoji (U+1F600, above the Basic Multilingual Plane) takes the
        // same convention -- the formula is codepoint-width-agnostic.
        assert_eq!(
            host_keysym(&Key::Character("😀".into())),
            Some(0x0100_0000 | 0x1F600)
        );
    }

    #[test]
    fn host_keysym_leaves_named_and_dead_keys_to_the_invariant_table() {
        // Named keys (arrows, Enter, function keys, modifiers, ...) are not
        // resolved here at all -- `invariant_keysym`'s scancode table
        // already covers the layout-invariant ones, and `None` is exactly
        // what makes `resolve_key_seat` fall back to it instead of (wrongly)
        // preferring an unresolved "host keysym".
        assert_eq!(host_keysym(&Key::Named(NamedKey::Enter)), None);
        assert_eq!(host_keysym(&Key::Named(NamedKey::ArrowLeft)), None);
        assert_eq!(host_keysym(&Key::Named(NamedKey::Escape)), None);
        // A dead key mid-composition (e.g. a standalone '^' awaiting the
        // next keystroke on an AZERTY/international layout) has no final
        // character yet -- the combined character arrives as a later,
        // separate `Character` event, which resolves normally.
        assert_eq!(host_keysym(&Key::Dead(Some('^'))), None);
        assert_eq!(host_keysym(&Key::Dead(None)), None);
    }

    #[test]
    fn host_keysym_drops_control_characters_rather_than_encode_them() {
        // Winit should never surface a control character through
        // `Key::Character` (they arrive as `Key::Named` instead), but if one
        // ever did, encoding it verbatim would mint an invalid X11 keysym
        // (0x00-0x1f, 0x7f are not keysyms at all) rather than the intended
        // key -- so this is dropped defensively, the same "drop rather than
        // guess" posture `resolve_key_seat` takes for an unresolvable key.
        assert_eq!(host_keysym(&Key::Character("\u{7}".into())), None); // BEL
        assert_eq!(host_keysym(&Key::Character("\x7f".into())), None); // DEL
        assert_eq!(host_keysym(&Key::Character("\r".into())), None);
        assert_eq!(host_keysym(&Key::Character("\t".into())), None);
    }

    #[test]
    fn host_keysym_takes_only_the_first_character_of_a_composed_string() {
        // A multi-character `Key::Character` (composed input, ligatures) is
        // approximated by its first character rather than dropped outright
        // -- an approximation, documented on `host_keysym`, not a silent
        // truncation a caller could be surprised by.
        assert_eq!(host_keysym(&Key::Character("fi".into())), Some(0x0066));
    }

    // ------------------------------------------------------------------
    // Routing: letterbox / crop coordinate mapping and hit-testing
    // ------------------------------------------------------------------

    /// The realm every router fixture below addresses, unless a test names a
    /// second one on purpose. A constant rather than a parameter because the
    /// coordinate-mapping and pairing tests are about one realm's seat and
    /// would say nothing extra for being spread over two.
    pub(crate) fn test_realm() -> crate::grants::RealmId {
        crate::grants::RealmId::new("realm-0")
    }

    /// A router with the human's attention already on [`test_realm`], which
    /// is what makes `route_physical` below have a destination at all.
    ///
    /// The bind's drain is `None` here by construction (nothing was bound, so
    /// nothing can be owed) and is asserted rather than discarded, so this
    /// fixture cannot quietly start swallowing a debt if `bind_to` changes.
    fn router() -> InputRouter<NoopHook> {
        let mut router = InputRouter::detached(NoopHook);
        assert!(
            router.bind_to(&test_realm()).is_none(),
            "the first bind can owe nothing: no realm was bound before it"
        );
        router
    }

    pub(crate) fn phys(kind: SeatInputKind) -> SeatInput {
        SeatInput::physical(kind)
    }

    /// Route one event **by the rule its own origin implies**, to
    /// [`test_realm`] either way.
    ///
    /// For the tests that sweep `Origin::ALL` and are about the *tag* rather
    /// than the addressing: they need one call that takes either origin, and
    /// picking the entry point from the tag here keeps the production
    /// signatures from having to. Everything that is actually about which
    /// realm an event reaches calls `route_physical`/`route_emulated`
    /// directly, which is the whole point of there being two.
    fn route_by_origin<H: PreemptionHook>(
        router: &mut InputRouter<H>,
        input: SeatInput,
        view: ViewGeometry,
        surface: Option<(u32, u32)>,
    ) -> Option<SeatDelivery> {
        match input.origin() {
            Origin::Physical => router.route_physical(input, view, surface),
            Origin::Emulated => router.route_emulated(&test_realm(), input, view, surface),
        }
    }

    fn motion(x: f64, y: f64) -> SeatInputKind {
        SeatInputKind::Motion { x, y }
    }

    /// A pointer position over the app's own pixel `(x, y)`, expressed in view
    /// coordinates.
    ///
    /// Since issue #304 the app's origin is [`ViewGeometry::reserved_top`] rows
    /// down — the trusted band's rows, plus the status strip's when `--status`
    /// is on — so a test that means "somewhere inside the surface" says so here
    /// instead of hard-coding a view coordinate that used to be inside the app
    /// and is now over the core's own chrome. Only valid for a surface placed
    /// at the usable rectangle's origin, which is what an app committing the
    /// size `configure` gave it does.
    fn over(view: ViewGeometry, x: f64, y: f64) -> SeatInputKind {
        motion(x, y + f64::from(view.reserved_top()))
    }

    fn press() -> SeatInputKind {
        SeatInputKind::Button {
            button: 0x110,
            state: ButtonState::Pressed,
        }
    }

    fn release() -> SeatInputKind {
        SeatInputKind::Button {
            button: 0x110,
            state: ButtonState::Released,
        }
    }

    fn scroll() -> SeatInputKind {
        SeatInputKind::Scroll {
            axis: Axis::Vertical,
            value120: -120,
        }
    }

    /// Assert a delivery is a motion at exactly `(x, y)` surface-local.
    fn assert_motion(delivery: &SeatDelivery, x: f64, y: f64) {
        assert_eq!(
            delivery.kind(),
            &SeatDeliveryKind::Motion {
                x: Fixed::from_f64(x),
                y: Fixed::from_f64(y),
            }
        );
    }

    /// **The agent-owned position follows the emulated origin alone** (D-019).
    ///
    /// This is the whole reason [`InputRouter::agent_pointer`] is a second
    /// field rather than a read of [`InputRouter::pointer`]: the shared field
    /// is written by *both* origins, so a sprite drawn on it would track the
    /// human's physical mouse and tell the human an agent is pointing wherever
    /// the human is. The mirror image of `ConsentGrab::pointer`, which is
    /// deliberately physical-only for the mirror-image reason.
    ///
    /// It is display state only: nothing here changes what the shim is
    /// delivered, which stays one shared position per realm view.
    #[test]
    fn the_agent_pointer_follows_the_emulated_origin_and_nothing_else() {
        let mut router = router();
        let realm = test_realm();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        assert_eq!(router.agent_pointer(&realm), None, "nothing has moved yet");

        // A human's motion moves the shared position and NOT the agent's.
        router.route_physical(phys(motion(10.0, 20.0)), view, surface);
        assert_eq!(
            router.agent_pointer(&test_realm()),
            None,
            "physical motion must never move the agent's cursor"
        );

        // An agent's motion moves both: the shared one because that is what
        // the app is delivered, the agent-owned one because that is what the
        // sprite is drawn at.
        router.route_emulated(
            &test_realm(),
            SeatInput::emulated(motion(30.0, 40.0)),
            view,
            surface,
        );
        assert_eq!(router.agent_pointer(&test_realm()), Some((30.0, 40.0)));

        // A later human motion does not drag the sprite along with it.
        router.route_physical(phys(motion(1.0, 2.0)), view, surface);
        assert_eq!(
            router.agent_pointer(&test_realm()),
            Some((30.0, 40.0)),
            "the agent's cursor moved because the human's mouse did"
        );

        // Recorded at intake, before gating and before hit-testing: a motion
        // onto the letterbox matte is not delivered, but the pointer is still
        // there and the sprite must be drawn there.
        router.route_emulated(
            &test_realm(),
            SeatInput::emulated(motion(200.0, 200.0)),
            view,
            Some((10, 10)),
        );
        assert_eq!(router.agent_pointer(&test_realm()), Some((200.0, 200.0)));

        // Realm teardown forgets it, so no sprite hovers over a realm that is
        // gone -- through the scoped entry point the teardown funnel uses,
        // so the private clear has no caller outside this module's own API.
        assert!(router.reset_for(&realm));
        assert_eq!(router.agent_pointer(&test_realm()), None);
    }

    #[test]
    fn exact_fit_motion_subtracts_only_the_reservation() {
        // The steady state of single-maximized **since issue #304**: the app
        // commits the size `configure` gave it, which is the USABLE view, so
        // the placement is `(0, reserved_top)` and wire coordinates equal view
        // coordinates shifted by the rows the core keeps. Before the inset the
        // app was configured at the output's full height and this mapping was
        // the identity; the shift is not a change of rule but the same
        // `ViewGeometry::place` the compositor paints with.
        let mut router = router();
        let view: ViewGeometry = (64u32, 48u32).into();
        let inset = f64::from(view.reserved_top());
        assert!(inset > 0.0, "every session reserves the band's rows");
        let out = router
            .route_physical(phys(motion(10.25, inset + 7.5)), view, Some(view.usable()))
            .expect("inside the surface");
        assert_motion(&out, 10.25, 7.5);
        assert_eq!(out.origin(), Origin::Physical);

        // ...and the app's own origin is the first row it was configured for.
        let origin = router
            .route_physical(phys(motion(0.0, inset)), view, Some(view.usable()))
            .expect("the app's top-left pixel");
        assert_motion(&origin, 0.0, 0.0);

        // A pointer ON the reserved rows is over the core's chrome, not the
        // app: it maps to a negative surface coordinate and is dropped.
        assert!(
            router
                .route_physical(phys(motion(0.0, inset - 1.0)), view, Some(view.usable()))
                .is_none(),
            "the trusted band is not the app"
        );
    }

    #[test]
    fn letterboxed_motion_subtracts_the_placement_offset() {
        // Output 100x80 with the band's 8 rows reserved, so the usable view is
        // 100x72; surface 40x20 is centred in THAT — placed at (30, 8 + 26) —
        // the same placement the compositor paints (`ViewGeometry::place`).
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        let (ox, oy) = (30.0, f64::from(view.reserved_top()) + (72.0 - 20.0) / 2.0);
        assert_eq!((ox, oy), (30.0, 34.0), "the fixture's own arithmetic");

        // Top-left surface pixel.
        let out = router.route_physical(phys(motion(ox, oy)), view, surface);
        assert_motion(&out.expect("surface origin"), 0.0, 0.0);
        // Bottom-right interior point.
        let out = router.route_physical(phys(motion(ox + 39.5, oy + 19.5)), view, surface);
        assert_motion(&out.expect("inside"), 39.5, 19.5);
        // One pixel past the right edge: matte, not app.
        assert!(router
            .route_physical(phys(motion(ox + 40.0, oy + 6.0)), view, surface)
            .is_none());
        // Just left of the placed rectangle: matte.
        assert!(router
            .route_physical(phys(motion(ox - 0.5, oy + 6.0)), view, surface)
            .is_none());
    }

    #[test]
    fn center_cropped_motion_adds_the_crop_offset() {
        // Output 40x30, usable 40x22 after the band's 8 rows, surface 60x50:
        // placement (-10, 8 + (22 - 50) / 2) = (-10, -6) — the negative-offset
        // case. The view's origin shows surface pixel (10, 6), and every view
        // point lies over the surface.
        let mut router = router();
        let view: ViewGeometry = (40, 30).into();
        let surface = Some((60, 50));

        let out = router.route_physical(phys(motion(0.0, 0.0)), view, surface);
        assert_motion(&out.expect("crop origin"), 10.0, 6.0);
        let out = router.route_physical(phys(motion(39.0, 29.0)), view, surface);
        assert_motion(&out.expect("crop interior"), 49.0, 35.0);
    }

    #[test]
    fn mixed_letterbox_and_crop_axes_map_independently() {
        // Output 100x20, usable 100x12 after the band's 8 rows, surface 40x50
        // (the #19 mixed case): x letterboxed (+30), y center-cropped
        // (8 + (12 - 50) / 2 = -11).
        let mut router = router();
        let view: ViewGeometry = (100, 20).into();
        let surface = Some((40, 50));

        let out = router.route_physical(phys(motion(30.0, 0.0)), view, surface);
        assert_motion(&out.expect("placed origin"), 0.0, 11.0);
        let out = router.route_physical(phys(motion(69.0, 19.0)), view, surface);
        assert_motion(&out.expect("placed interior"), 39.0, 30.0);
        // Left of the placed rectangle: matte on the x axis.
        assert!(router
            .route_physical(phys(motion(29.0, 10.0)), view, surface)
            .is_none());
    }

    #[test]
    fn sub_pixel_motion_survives_in_fixed_point() {
        // Host positions are f64 (HiDPI hosts report fractions); the wire
        // is 24.8 fixed-point exactly so sub-pixel survives.
        //
        // Design note (issue #24 review): the prose page
        // docs/protocol/11-vitrin_shim_seat.md still says v0 motion is
        // whole-pixel — written for the actuator path before nested
        // physical intake existed. The IDL <description>, which wins,
        // only motivates fixed-point via later synthesis and does not
        // forbid fractional v0 values; the prose sentence is flagged for
        // a protocol-track amendment rather than rounding away real host
        // precision here.
        let mut router = router();
        // An output of 10x18 leaves a 10x10 usable view after the band's rows,
        // so the app's own origin is at y = 8 and the fractions below are
        // measured from it.
        let view: ViewGeometry = (10u32, 18u32).into();
        let inset = f64::from(view.reserved_top());
        let out = router
            .route_physical(phys(motion(0.5, inset + 0.25)), view, Some(view.usable()))
            .expect("inside");
        match out.kind() {
            SeatDeliveryKind::Motion { x, y } => {
                assert_eq!(x.to_bits(), 128); // 0.5 * 256
                assert_eq!(y.to_bits(), 64); // 0.25 * 256
            }
            other => panic!("expected motion, got {other:?}"),
        }
    }

    #[test]
    fn click_on_the_matte_is_not_a_click_in_the_app() {
        // Pointer parked on the letterbox matte: motion, press, release,
        // scroll — none of it reaches the app, and the unpaired release
        // does not leak either. Keys still flow (focus is held on the app
        // shim-side).
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20)); // placed at (30, 34): centred in the usable view

        assert!(router
            .route_physical(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(scroll()), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_none());
        let key = router.route_physical(
            phys(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff0d,
                state: KeyState::Pressed,
            }),
            view,
            surface,
        );
        assert!(key.is_some(), "keys route regardless of pointer position");
    }

    #[test]
    fn implicit_grab_holds_the_pointer_through_an_off_surface_drag() {
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20)); // placed at (30, 34): centred in the usable view

        // Move onto the surface and press: both delivered.
        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());

        // Drag off the surface: motion keeps flowing under the implicit
        // grab, with out-of-bounds (here negative) surface-local
        // coordinates — the wire's fixed is signed for exactly this.
        let out = router.route_physical(phys(motion(0.0, 0.0)), view, surface);
        assert_motion(&out.expect("grabbed motion"), -30.0, -34.0);
        // Scroll during the grab flows too.
        assert!(router
            .route_physical(phys(scroll()), view, surface)
            .is_some());
        // The release lands wherever the drag ended: delivered, so the app
        // never holds a stuck button.
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_some());

        // Grab over: the same off-surface events are matte again.
        assert!(router
            .route_physical(phys(motion(0.0, 0.0)), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(scroll()), view, surface)
            .is_none());
    }

    #[test]
    fn releases_pair_per_button_code_not_by_count() {
        // Issue #24 review: pairing is per button code, never a bare
        // count. A BTN_LEFT press dropped on the matte must not pair with
        // a later delivered BTN_RIGHT press — the wire must never see a
        // release for a button the app never saw pressed, and the app
        // must never be left holding a stranded button.
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20)); // placed at (30, 34): centred in the usable view
        let button = |code, state| SeatInputKind::Button {
            button: code,
            state,
        };

        // BTN_LEFT pressed on the matte: dropped, no grab.
        assert!(router
            .route_physical(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(button(0x110, ButtonState::Pressed)), view, surface)
            .is_none());
        // Drag onto the surface (the host keeps reporting motion while
        // the button stays physically held) and press BTN_RIGHT there.
        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(button(0x111, ButtonState::Pressed)), view, surface)
            .is_some());
        // BTN_LEFT's release: its press was never delivered — dropped,
        // it must not consume BTN_RIGHT's grab.
        assert!(router
            .route_physical(phys(button(0x110, ButtonState::Released)), view, surface)
            .is_none());
        // BTN_RIGHT's release still pairs: no stuck button in the app.
        assert!(router
            .route_physical(phys(button(0x111, ButtonState::Released)), view, surface)
            .is_some());
        // The grab is fully over: off-surface motion is matte again.
        assert!(router
            .route_physical(phys(motion(0.0, 0.0)), view, surface)
            .is_none());
    }

    #[test]
    fn no_committed_surface_means_no_pointer_path_but_keys_flow() {
        // Headless-at-startup / pre-first-commit: there is nothing to
        // point at, so the pointer path yields nothing — no phantom
        // deliveries. Keys and text still route; the shim owns that
        // judgement.
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();

        assert!(router
            .route_physical(phys(motion(10.0, 10.0)), view, None)
            .is_none());
        assert!(router.route_physical(phys(press()), view, None).is_none());
        assert!(router.route_physical(phys(scroll()), view, None).is_none());
        let key = router.route_physical(
            phys(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff1b,
                state: KeyState::Pressed,
            }),
            view,
            None,
        );
        assert!(key.is_some());
        let text = router.route_emulated(
            &test_realm(),
            SeatInput::emulated(SeatInputKind::Text {
                text: "hello".into(),
            }),
            view,
            None,
        );
        assert!(text.is_some());
    }

    // ------------------------------------------------------------------
    // The preemption hook point
    // ------------------------------------------------------------------

    /// A recording hook: logs `observe`/`gate` calls in order and consumes
    /// while `consume` is set — the shape P1.7.3 (observer) and P1.7.2
    /// (gate) attach with.
    struct RecordingHook {
        log: Rc<RefCell<Vec<(&'static str, Origin)>>>,
        consume: Rc<Cell<bool>>,
    }

    impl PreemptionHook for RecordingHook {
        fn observe(&mut self, input: &SeatInput) {
            self.log.borrow_mut().push(("observe", input.origin()));
        }

        // A terminal test double owns no attention signal. Spelled out because
        // `PreemptionHook::attention` deliberately has no default: a wrapping
        // hook that inherited `None` would silently disable the human's
        // attention key, which is the shape of the bug #212's review found in
        // `PresenceHook`.
        fn attention(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>> {
            None
        }

        fn clipboard(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>> {
            None
        }

        // Not defaulted either, for `PreemptionHook::screenshot`'s reason.
        fn screenshot(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>> {
            None
        }
        fn backlight(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>> {
            None
        }
        fn gate(&mut self, input: &SeatInput) -> Gate {
            self.log.borrow_mut().push(("gate", input.origin()));
            if self.consume.get() {
                Gate::Consume
            } else {
                Gate::Deliver
            }
        }
    }

    #[test]
    fn hook_observes_every_event_before_the_gate_and_consumption_blocks_delivery() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let consume = Rc::new(Cell::new(false));
        let mut router = InputRouter::detached(RecordingHook {
            log: Rc::clone(&log),
            consume: Rc::clone(&consume),
        });
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());

        // Delivering: observe precedes gate.
        assert!(router
            .route_physical(phys(over(view, 1.0, 1.0)), view, surface)
            .is_some());
        assert_eq!(
            log.borrow().as_slice(),
            &[("observe", Origin::Physical), ("gate", Origin::Physical)]
        );

        // Consuming: nothing is delivered, but the observer still saw the
        // raw event — P1.7.3's revocation watcher works mid-grab.
        log.borrow_mut().clear();
        consume.set(true);
        assert!(router
            .route_physical(phys(over(view, 2.0, 2.0)), view, surface)
            .is_none());
        assert!(router
            .route_emulated(&test_realm(), SeatInput::emulated(scroll()), view, surface)
            .is_none());
        assert_eq!(
            log.borrow().as_slice(),
            &[
                ("observe", Origin::Physical),
                ("gate", Origin::Physical),
                ("observe", Origin::Emulated),
                ("gate", Origin::Emulated),
            ]
        );
    }

    #[test]
    fn consumed_press_starts_no_implicit_grab() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let consume = Rc::new(Cell::new(false));
        let mut router = InputRouter::detached(RecordingHook {
            log,
            consume: Rc::clone(&consume),
        });
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));

        // Pointer on the surface; a grab-holder consumes the press.
        assert!(router
            .route_physical(phys(motion(5.0, 5.0)), view, surface)
            .is_some());
        consume.set(true);
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_none());

        // Gate reopens: no implicit grab exists, so off-surface motion is
        // matte and the (never-pressed) release is dropped unpaired.
        consume.set(false);
        assert!(router
            .route_physical(phys(motion(1000.0, 1000.0)), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_none());
    }

    #[test]
    fn consumed_release_reconciles_the_grab_without_delivering() {
        // Issue #24 review: a gate that consumes the release of a
        // delivered press (the P1.7.2 consent grab seizing input
        // mid-drag) must not wedge the implicit grab forever. The router
        // reconciles its own bookkeeping — nothing is delivered; the
        // app-side stranded press is the gate implementor's documented
        // debt (see `Gate::Consume`).
        let log = Rc::new(RefCell::new(Vec::new()));
        let consume = Rc::new(Cell::new(false));
        let mut router = InputRouter::detached(RecordingHook {
            log,
            consume: Rc::clone(&consume),
        });
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20)); // placed at (30, 34): centred in the usable view

        // Press delivered on the surface: implicit grab begins.
        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());

        // The grab consumes the user's release: not delivered ...
        consume.set(true);
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_none());

        // ... but the router did not wedge. With the gate open again,
        // matte hit-testing works (no phantom grab force-delivers
        // off-surface events to the app), an unpaired release stays
        // dropped, and a fresh press/release pair over the surface
        // behaves normally.
        consume.set(false);
        assert!(router
            .route_physical(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(scroll()), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
    }

    #[test]
    fn reset_forgets_grab_and_pointer_across_shim_generations() {
        // Issue #24 review: a grab held at shim death must not become a
        // phantom grab against the next shim generation. `reset_for` is what
        // the realm teardown funnel (`ShimServer::connection_closed`)
        // invokes alongside `Scene::clear_surface`.
        let mut router = router();
        let realm = test_realm();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20)); // placed at (30, 34): centred in the usable view

        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());

        assert!(router.reset_for(&realm), "the bound realm's death clears");
        assert_eq!(
            router.bound_realm(),
            None,
            "the human's input has nowhere to go until a realm is bound again"
        );
        // The next shim generation attaches and the binding comes back to it
        // (`session::rebind_output_after_death`, then the next physical
        // turn's `bind_to`). Without this the assertions below would all pass
        // vacuously on "nothing is bound" rather than on the reset.
        assert!(
            router.bind_to(&realm).is_none(),
            "a re-bind after a death owes nothing: the reset already dropped the seat"
        );

        // The stale release from the dead generation is unpaired at the
        // fresh seat: dropped.
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_none());
        // The pointer position was forgotten too: a press cannot
        // hit-test against a pre-teardown position (which sat over the
        // surface) — first motion must re-establish it.
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_none());
        // No phantom grab: off-surface events are matte again.
        assert!(router
            .route_physical(phys(motion(0.0, 0.0)), view, surface)
            .is_none());
        assert!(router
            .route_physical(phys(scroll()), view, surface)
            .is_none());
        // The fresh generation then works normally.
        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_some());
    }

    /// **A sibling realm's death must not clear the bound realm's state**
    /// (WS-E.1.2 review, HIGH 2).
    ///
    /// One router serves a session that may now hold several realms, so an
    /// unconditional `reset` on *any* realm's death is one realm reaching
    /// into another's. The consequence is not abstract: the router's pairing
    /// table is the record of which presses the app was told about, and
    /// forgetting an entry means the matching release is dropped as unpaired
    /// — the key stays down in a surviving app forever, and the journal
    /// records nothing, because nothing was delivered.
    ///
    /// Both directions are asserted. A `reset_for` that cleared nothing at
    /// all would satisfy the first half and lose the phantom-grab guarantee
    /// [`reset_forgets_grab_and_pointer_across_shim_generations`] pins.
    #[test]
    fn a_siblings_death_leaves_the_bound_realms_held_key_alone() {
        use crate::grants::RealmId;
        let mut router = InputRouter::detached(NoopHook);
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        let bound = RealmId::new("browser");
        let sibling = RealmId::new("realm-0");

        // The generation belongs to `browser`: everything below is what
        // *its* app was told. Nothing was bound before, so the bind can owe
        // nothing -- asserted rather than discarded.
        assert!(router.bind_to(&bound).is_none());
        assert!(router
            .route_physical(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
        let key_down = || SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: 0xffe1, // Shift_L: the modifier a latch is worst for
            state: KeyState::Pressed,
        };
        assert!(router
            .route_physical(phys(key_down()), view, surface)
            .is_some());
        assert_eq!(router.held_keys(&bound), [(0xffe1, Origin::Physical)]);

        // An unrelated realm dies. Nothing of this generation is its to
        // forget.
        assert!(
            !router.reset_for(&sibling),
            "a realm that never held the generation clears nothing"
        );
        assert_eq!(
            router.bound_realm(),
            Some(&bound),
            "and it does not steal the binding either"
        );
        assert_eq!(
            router.held_keys(&bound),
            [(0xffe1, Origin::Physical)],
            "the survivor's app is still holding this key, so the router still owes its release"
        );
        // The proof that matters at the app: the release still pairs and is
        // still delivered. Under an unscoped reset it would be dropped as
        // unpaired and Shift would latch down for good.
        let key_up = SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: 0xffe1,
            state: KeyState::Released,
        };
        assert!(
            router.route_physical(phys(key_up), view, surface).is_some(),
            "the held key's release must still be deliverable"
        );
        // ...and the implicit grab survived too, so an off-surface release
        // still reaches the app that saw the press.
        assert!(router
            .route_physical(phys(motion(0.0, 0.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(release()), view, surface)
            .is_some());

        // The bound realm's own death still clears everything, which is the
        // half `reset_forgets_grab_and_pointer_across_shim_generations`
        // covers in full.
        assert!(router
            .route_physical(phys(key_down()), view, surface)
            .is_some());
        assert!(router.reset_for(&bound), "its own realm's death clears");
        assert_eq!(router.bound_realm(), None);
        assert!(router.held_keys(&bound).is_empty());
    }

    // ------------------------------------------------------------------
    // B2 on the wire: every delivery kind encodes its origin
    // ------------------------------------------------------------------

    #[test]
    fn every_delivery_kind_encodes_its_origin() {
        // For every seat event kind and every origin value the protocol
        // defines (Origin::ALL — an appended origin fails this loudly):
        // encode, then decode with the generated types the shim uses, and
        // the tag must round-trip. Together with SeatDelivery::encode's
        // exhaustive match this is the no-untagged-path proof at the wire.
        const SEAT_ID: u32 = 3;
        for &origin in Origin::ALL {
            let wrap = |kind: SeatInputKind| match origin {
                Origin::Physical => SeatInput::physical(kind),
                Origin::Emulated => SeatInput::emulated(kind),
            };
            let mut router = router();
            let view: ViewGeometry = (64, 48).into();
            let surface = Some(view.usable());

            let deliver = |router: &mut InputRouter<NoopHook>, kind| {
                route_by_origin(router, wrap(kind), view, surface)
                    .expect("routable by construction")
            };

            let bytes = deliver(&mut router, over(view, 7.0, 9.0)).encode(SEAT_ID);
            let (oid, ev) = seat::events::Motion::decode(&bytes, None).unwrap();
            assert_eq!((oid, ev.origin), (SEAT_ID, origin));
            assert_eq!((ev.x, ev.y), (Fixed::from_f64(7.0), Fixed::from_f64(9.0)));

            let bytes = deliver(&mut router, press()).encode(SEAT_ID);
            let (_, ev) = seat::events::Button::decode(&bytes, None).unwrap();
            assert_eq!(ev.origin, origin);
            assert_eq!((ev.button, ev.state), (0x110, ButtonState::Pressed));

            let bytes = deliver(&mut router, scroll()).encode(SEAT_ID);
            let (_, ev) = seat::events::Scroll::decode(&bytes, None).unwrap();
            assert_eq!(ev.origin, origin);
            assert_eq!((ev.axis, ev.value120), (Axis::Vertical, -120));

            let bytes = deliver(
                &mut router,
                SeatInputKind::Key {
                    source: KeySource::Keysym,
                    keysym: 0xff0d,
                    state: KeyState::Pressed,
                },
            )
            .encode(SEAT_ID);
            let (_, ev) = seat::events::Key::decode(&bytes, None).unwrap();
            assert_eq!(ev.origin, origin);
            assert_eq!((ev.keysym, ev.state), (0xff0d, KeyState::Pressed));

            let bytes = deliver(
                &mut router,
                SeatInputKind::Text {
                    text: "héllo→世界".into(),
                },
            )
            .encode(SEAT_ID);
            let (_, ev) = seat::events::Text::decode(&bytes, None).unwrap();
            assert_eq!(ev.origin, origin);
            assert_eq!(ev.text, "héllo→世界");
        }
    }

    #[test]
    fn every_delivery_labels_its_kind_and_origin_for_the_recorder() {
        // The projection the flight recorder writes (issue #83,
        // `recorder::Event::SeatDelivered`): every kind maps to its own
        // stable label and every origin to its own. Over `Origin::ALL`, so an
        // appended origin fails here loudly rather than being logged as an old
        // one -- the same guard `every_delivery_kind_encodes_its_origin` puts
        // on the wire, applied to the audit label.
        for &origin in Origin::ALL {
            let wrap = |kind: SeatInputKind| match origin {
                Origin::Physical => SeatInput::physical(kind),
                Origin::Emulated => SeatInput::emulated(kind),
            };
            let expected_origin = match origin {
                Origin::Physical => "physical",
                Origin::Emulated => "emulated",
            };
            let mut router = router();
            let view: ViewGeometry = (64, 48).into();
            let surface = Some(view.usable());
            let mut deliver = |kind| {
                route_by_origin(&mut router, wrap(kind), view, surface)
                    .expect("routable by construction")
            };

            // Ordered as the router tolerates (a press leaves an implicit grab
            // the rest ride), mirroring the encode test above.
            for (kind, label) in [
                (over(view, 1.0, 2.0), "motion"),
                (press(), "button"),
                (scroll(), "scroll"),
                (
                    SeatInputKind::Key {
                        source: KeySource::Keysym,
                        keysym: 0xff0d,
                        state: KeyState::Pressed,
                    },
                    "key",
                ),
                (SeatInputKind::Text { text: "x".into() }, "text"),
            ] {
                let delivery = deliver(kind);
                assert_eq!(delivery.event_label(), label);
                assert_eq!(delivery.origin_label(), expected_origin);
                // The free projection and the method agree, and neither leaks
                // any payload -- label strings only.
                assert_eq!(super::origin_label(delivery.origin()), expected_origin);
            }
        }
    }

    #[test]
    fn recording_a_delivery_skips_motion_and_keeps_the_origin() {
        // The funnel both delivery paths call (issue #83): a discrete event is
        // journaled with its origin; pointer motion is not (raw device rate on
        // the physical path, coordinates never recorded anyway — a per-event
        // motion line would only flood the recorder). This is the code the
        // nested backend's physical path runs, exercised here where routing is
        // cheap.
        let _fd = crate::capture::tests::fd_lock();
        let (mut rec, path) = crate::recorder::tests::scratch_recorder("seat-delivery-funnel");
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());
        let motion_delivery = router
            .route_physical(SeatInput::physical(over(view, 1.0, 2.0)), view, surface)
            .expect("motion routes");
        let button_delivery = router
            .route_physical(SeatInput::physical(press()), view, surface)
            .expect("button routes");
        let realm = crate::grants::RealmId::new("realm-7");
        super::record_seat_delivery(&mut rec, &realm, &motion_delivery);
        super::record_seat_delivery(&mut rec, &realm, &button_delivery);

        let entries = crate::recorder::tests::read_log(&path);
        let delivered = crate::recorder::tests::of_kind(&entries, "seat_delivered");
        assert_eq!(delivered.len(), 1, "motion is not journaled; the button is");
        assert_eq!(delivered[0].str("event"), "button");
        assert_eq!(delivered[0].str("origin"), "physical");
        // Which app received it: not derivable from the grant row, because
        // the delivery target is chosen at runtime.
        assert_eq!(delivered[0].str("realm"), "realm-7");
        crate::recorder::tests::cleanup(&path);
    }

    // ------------------------------------------------------------------
    // End-to-end: intake -> router -> ShimServer -> wire -> mock shim
    // ------------------------------------------------------------------

    const VIEW_W: u32 = 64;
    const VIEW_H: u32 = 48;

    /// A live core/shim pair with the seat minted: configure sent,
    /// `create_surface` + `get_seat` processed by the server.
    fn wire_setup() -> (ShimServer, Scene, Connection, MockShim) {
        let (mut core, shim) = Connection::pair().expect("socketpair");
        let server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            geom: (VIEW_W, VIEW_H).into(),
        });
        server
            .send_configure(&mut |frame| core.send_message(frame, None))
            .expect("configure");
        let mut mock = MockShim::start(shim).expect("bring-up");
        mock.get_seat().expect("get_seat");
        let mut scene = Scene::new();
        let mut server = server;
        process(&mut server, &mut scene, &mut core, 2); // create_surface, get_seat
        (server, scene, core, mock)
    }

    fn process(server: &mut ShimServer, scene: &mut Scene, core: &mut Connection, n: usize) {
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            server
                .handle_message(msg, scene, None, &mut |frame| {
                    core.send_message(frame, None)
                })
                .expect("compliant traffic");
        }
    }

    #[test]
    fn human_click_and_type_reach_the_mock_shim_seat_origin_tagged() {
        // The core-side half of the acceptance criterion "human can
        // click/type into the shimmed app through the nested window":
        // synthetic host events tagged physical at intake, routed through
        // the view->surface mapping, delivered by the shim server, decoded
        // off the real wire by the mock shim — origin intact end to end.
        let _fd = crate::capture::tests::fd_lock();
        let (server, _scene, mut core, mut mock) = wire_setup();
        let mut router = router();
        let view: ViewGeometry = (VIEW_W, VIEW_H).into();
        // Steady state: the surface is the size `configure` gave it, so it
        // fills the usable view and its origin is `reserved_top()` rows down.
        let surface = Some(view.usable());
        let inset = f64::from(view.reserved_top());

        let mut send_all = |inputs: Vec<SeatInput>, router: &mut InputRouter<NoopHook>| {
            for input in inputs {
                let delivery = router
                    .route_physical(input, view, surface)
                    .expect("routable");
                let sent = server
                    .deliver_seat_event(&delivery, &mut |frame| core.send_message(frame, None))
                    .expect("send");
                assert!(sent, "seat exists, so delivery must go out");
            }
        };

        // A click: move onto the app, press, release — through the real
        // generic intake (synthetic host backend), so the physical tag is
        // bound where nested-mode input enters the core.
        send_all(
            intake_physical(
                &motion_ev(10.0, 20.0 + inset),
                (VIEW_W as i32, VIEW_H as i32),
            ),
            &mut router,
        );
        send_all(
            intake_physical(
                &button_ev(0x110, host::ButtonState::Pressed),
                (VIEW_W as i32, VIEW_H as i32),
            ),
            &mut router,
        );
        send_all(
            intake_physical(
                &button_ev(0x110, host::ButtonState::Released),
                (VIEW_W as i32, VIEW_H as i32),
            ),
            &mut router,
        );
        // Typing: Enter (a layout-invariant key the human path carries as
        // a keysym), and agent text for the emulated contrast.
        send_all(
            intake_physical(
                &key_ev(28, host::KeyState::Pressed),
                (VIEW_W as i32, VIEW_H as i32),
            ),
            &mut router,
        );
        {
            let delivery = router
                .route_emulated(
                    &test_realm(),
                    SeatInput::emulated(SeatInputKind::Text {
                        text: "vitrin".into(),
                    }),
                    view,
                    surface,
                )
                .expect("text routes");
            assert!(server
                .deliver_seat_event(&delivery, &mut |frame| core.send_message(frame, None))
                .unwrap());
        }

        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Motion {
                x: Fixed::from_f64(10.0),
                y: Fixed::from_f64(20.0),
                origin: Origin::Physical,
            }
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Button {
                button: 0x110,
                state: ButtonState::Pressed,
                origin: Origin::Physical,
            }
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Button {
                button: 0x110,
                state: ButtonState::Released,
                origin: Origin::Physical,
            }
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Key {
                keysym: 0xff0d,
                state: KeyState::Pressed,
                origin: Origin::Physical,
            }
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Text {
                text: "vitrin".into(),
                origin: Origin::Emulated,
            }
        );
    }

    #[test]
    fn seat_event_before_get_seat_is_dropped_not_an_error() {
        // IDL: input routed to the realm before the shim has created its
        // seat has no destination and is dropped. No error, no wire bytes.
        let _fd = crate::capture::tests::fd_lock();
        let (mut core, shim) = Connection::pair().expect("socketpair");
        let server = ShimServer::new(ShimConfig {
            realm: "realm-0".into(),
            geom: (VIEW_W, VIEW_H).into(),
        });
        server
            .send_configure(&mut |frame| core.send_message(frame, None))
            .expect("configure");
        let mut mock = MockShim::start(shim).expect("bring-up");
        let mut scene = Scene::new();
        let mut server = server;
        process(&mut server, &mut scene, &mut core, 1); // create_surface only

        let mut router = router();
        let delivery = router
            .route_physical(
                phys(SeatInputKind::Key {
                    source: KeySource::Keysym,
                    keysym: 0xff1b,
                    state: KeyState::Pressed,
                }),
                (VIEW_W, VIEW_H).into(),
                None,
            )
            .expect("keys route");
        let sent = server
            .deliver_seat_event(&delivery, &mut |frame| core.send_message(frame, None))
            .expect("dropping is not an error");
        assert!(!sent, "no seat minted: the event has no destination");

        // Nothing rode the wire: a probe on the (nonblocking) shim side
        // sees silence.
        let conn = mock.connection_mut();
        let flags = rustix::fs::fcntl_getfl(conn.as_fd()).unwrap();
        rustix::fs::fcntl_setfl(conn.as_fd(), flags | rustix::fs::OFlags::NONBLOCK).unwrap();
        match conn.recv_message() {
            Err(TransportError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("expected a silent wire, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // The consent grab through the real router (P1.7.2)
    //
    // These live here rather than in `crate::consent::grab` because
    // `SeatInput::physical` is private to this module by design (B2), so
    // this is the only place a physical-origin event can be minted and
    // pushed through the router the way nested intake does.
    // ------------------------------------------------------------------

    /// What the *innermost* hook saw, counted separately for each half of
    /// the trait.
    ///
    /// The observe tap is P1.7.3's attachment point, and its whole reason
    /// to exist is that it keeps seeing raw physical events while a consent
    /// grab consumes every one of them — the moment revocation matters most
    /// is the moment a prompt is on screen. That property was asserted in
    /// three doc comments and tested nowhere: the only wrapper test fed an
    /// emulated event, which the grab passes through anyway, so it never
    /// exercised the short-circuit path at all.
    #[derive(Default)]
    struct HookSpy {
        observed: std::cell::Cell<usize>,
        gated: std::cell::Cell<usize>,
    }

    struct SpyHook(Rc<HookSpy>);

    impl PreemptionHook for SpyHook {
        fn observe(&mut self, _input: &SeatInput) {
            self.0.observed.set(self.0.observed.get() + 1);
        }

        // A terminal test double owns no attention signal. Spelled out because
        // `PreemptionHook::attention` deliberately has no default: a wrapping
        // hook that inherited `None` would silently disable the human's
        // attention key, which is the shape of the bug #212's review found in
        // `PresenceHook`.
        fn attention(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::attention::AttentionSignal>>> {
            None
        }

        fn clipboard(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>> {
            None
        }

        // Not defaulted either, for `PreemptionHook::screenshot`'s reason.
        fn screenshot(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>> {
            None
        }

        fn backlight(
            &self,
        ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>> {
            None
        }

        fn gate(&mut self, _input: &SeatInput) -> Gate {
            self.0.gated.set(self.0.gated.get() + 1);
            Gate::Deliver
        }
    }

    /// A router carrying the consent grab over a spy hook, plus everything
    /// a test needs to raise a prompt, advance the clock, and read
    /// decisions back.
    struct Grabbed {
        router: InputRouter<crate::consent::grab::ConsentGate<SpyHook>>,
        grab: Rc<RefCell<crate::consent::grab::ConsentGrab>>,
        surface: crate::consent::ConsentSurface,
        /// The clock cell the embedder owns; `raise_at` and the tests
        /// advance it exactly as the nested backend's `handle_input` does.
        now: Rc<std::cell::Cell<std::time::Instant>>,
        spy: Rc<HookSpy>,
        /// Kept alive for the length of a test: `raise` records to it, and
        /// dropping it removes the scratch log.
        recorder: crate::recorder::Recorder,
        log_path: std::path::PathBuf,
    }

    impl Grabbed {
        fn new() -> Self {
            let grab = Rc::new(RefCell::new(crate::consent::grab::ConsentGrab::new()));
            let now = Rc::new(std::cell::Cell::new(std::time::Instant::now()));
            let spy = Rc::new(HookSpy::default());
            let mut router = InputRouter::detached(crate::consent::grab::ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                SpyHook(Rc::clone(&spy)),
            ));
            // The human's attention is on the rig's one realm, so
            // `route_physical` below has somewhere to deliver.
            assert!(router.bind_to(&test_realm()).is_none());
            let (recorder, log_path) = crate::recorder::tests::scratch_recorder("input-grab");
            Self {
                router,
                grab,
                surface: crate::consent::ConsentSurface::new(
                    crate::consent::TrustedIndicator::for_test(),
                ),
                now,
                spy,
                recorder,
                log_path,
            }
        }

        /// Raise `petition`'s prompt and step the clock past the guard
        /// interval, which is what all but the guard's own test want: they
        /// are asserting routing, not the anti-tapjacking delay.
        fn raise_awake(
            &mut self,
            petition: crate::petitions::PetitionId,
            registry: &mut crate::petitions::PetitionRegistry,
        ) {
            let raised_at = self.now.get();
            self.grab
                .borrow_mut()
                .raise(
                    petition,
                    raised_at,
                    registry,
                    &mut self.surface,
                    &mut self.recorder,
                )
                .expect("pending");
            self.now
                .set(raised_at + crate::consent::grab::GUARD_INTERVAL);
        }

        fn lower(&mut self, registry: &mut crate::petitions::PetitionRegistry) {
            self.grab.borrow_mut().lower(registry, &mut self.surface);
        }
    }

    impl Drop for Grabbed {
        fn drop(&mut self) {
            crate::recorder::tests::cleanup(&self.log_path);
        }
    }

    /// One pending petition in a fresh interactive registry, wired to the
    /// realm `wire_setup` serves.
    fn pending_petition() -> (
        crate::petitions::PetitionRegistry,
        crate::petitions::PetitionId,
    ) {
        use crate::petitions::{
            Admission, ConsentPolicy, PetitionConfig, PetitionRegistry, PetitionRequest,
        };
        use vitrin_protocol::generated::vitrin_grant::{Persistence as WirePersistence, Verb};

        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let connection = registry.register_connection();
        let realms = crate::realm::tests::registry_with(&["realm-0"]);
        let Admission::Pending { petition } = registry.admit(
            PetitionRequest {
                connection,
                identity: crate::identity::PrincipalIdentity::parse(
                    crate::consent::tests::PROMPT_IDENTITY,
                )
                .expect("fixture identity"),
                realm_name: "realm-0".into(),
                grant_wire_id: 10,
                consent_wire_id: 11,
                resource: String::new(),
                verbs: Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT,
                expiry_ms: 60_000,
                max_event_rate: 0,
                persistence: WirePersistence::WhileRunning,
                flags: 0,
            },
            std::time::Instant::now(),
            &realms,
            false,
        ) else {
            panic!("an interactive petition must pend");
        };
        (registry, petition)
    }

    /// **The trusted path is session-wide, and the per-realm split did not
    /// scope it** (WS-E.1.6, issue #212, acceptance criterion 4, first half).
    ///
    /// A consent prompt consumes physical input for **every** realm, not only
    /// the one the human's attention is bound to. That has to be asserted
    /// rather than assumed, because a per-realm router is exactly the change
    /// that could quietly make a grab apply to one realm's events and not
    /// another's — and a prompt that stopped consuming the moment the binding
    /// moved would be answerable by whatever was pointing at the card.
    ///
    /// It is also structural, not merely tested:
    /// [`PreemptionHook::gate`] is never told which realm an event was
    /// addressed to, so a realm-scoped gate is *inexpressible* rather than
    /// forbidden. This test is what proves the structure was not worked
    /// around at the router.
    #[test]
    fn a_prompt_consumes_physical_input_for_every_realm_not_only_the_bound_one() {
        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());
        let a = test_realm();
        let b = crate::grants::RealmId::new("realm-b");

        // Fixture: ordinary routing in realm A, before any prompt.
        assert!(rig
            .router
            .route_physical(phys(motion(10.0, 10.0)), view, surface_size)
            .is_some());

        rig.raise_awake(petition, &mut registry);

        // Realm A is bound: consumed, as `a_raised_prompt_stops_human_input_at_the_router`
        // already pins.
        assert!(rig
            .router
            .route_physical(phys(motion(20.0, 20.0)), view, surface_size)
            .is_none());

        // The human's attention moves to realm B *while the prompt is up*.
        // Everything must still be consumed. A gate scoped to the realm that
        // was bound when the prompt went up would deliver here.
        let (losing, owed) = rig
            .router
            .bind_to(&b)
            .expect("the binding moved, so the realm being left is owed its drain");
        assert_eq!(losing, a);
        assert!(
            owed.is_empty(),
            "nothing was held: the prompt consumed the presses, so no app saw one begin"
        );
        for input in [
            phys(motion(30.0, 30.0)),
            phys(press()),
            phys(scroll()),
            phys(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff0d,
                state: KeyState::Pressed,
            }),
        ] {
            assert!(
                rig.router
                    .route_physical(input, view, surface_size)
                    .is_none(),
                "a raised prompt must consume physical input for EVERY realm: a grab that \
                 stopped at the realm it was raised over would let the human's next click \
                 reach an app instead of the card"
            );
        }
        // Neither realm's app was told anything at all.
        assert!(rig.router.held_keys(&a).is_empty() && rig.router.held_buttons(&a).is_empty());
        assert!(rig.router.held_keys(&b).is_empty() && rig.router.held_buttons(&b).is_empty());

        // An agent's actuation into a *third* realm passes through, exactly as
        // it does with one realm: the grab consumes `physical` only
        // (`crate::consent::grab`, "why agent input is not consumed here" --
        // the petitioning principal's own actuations are already refused
        // `consent_held` at the chokepoint, and another principal's are none
        // of this prompt's business). The realm is not part of that
        // judgement, and the point of asserting it here is that it did not
        // *become* part of one.
        assert!(rig
            .router
            .route_emulated(
                &crate::grants::RealmId::new("realm-c"),
                SeatInput::emulated(motion(40.0, 40.0)),
                view,
                surface_size,
            )
            .is_some());

        // Lowering restores delivery, in the realm now bound.
        rig.lower(&mut registry);
        assert!(rig
            .router
            .route_physical(phys(motion(50.0, 50.0)), view, surface_size)
            .is_some());
    }

    #[test]
    fn a_raised_prompt_stops_human_input_at_the_router() {
        // The acceptance criterion "all human input routes exclusively to
        // the prompt", asserted where it actually has to hold: the router,
        // whose output is the only thing that can reach a shim seat.
        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());

        // Before the prompt: ordinary routing.
        assert!(rig
            .router
            .route_physical(phys(motion(10.0, 10.0)), view, surface_size)
            .is_some());

        rig.raise_awake(petition, &mut registry);

        // Motion, presses, scroll, and key presses all stop here. Keys are
        // the sharpest case: they route without geometry otherwise (focus
        // is held shim-side), so only the grab can stop them.
        for kind in [
            motion(20.0, 20.0),
            press(),
            scroll(),
            SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff0d,
                state: KeyState::Pressed,
            },
            SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff1b, // Escape, reserved for P1.7.3
                state: KeyState::Pressed,
            },
        ] {
            assert!(
                rig.router
                    .route_physical(phys(kind.clone()), view, surface_size)
                    .is_none(),
                "{kind:?} reached the app while a prompt was up"
            );
        }

        // Lowering the prompt restores routing.
        rig.lower(&mut registry);
        assert!(rig
            .router
            .route_physical(phys(motion(30.0, 30.0)), view, surface_size)
            .is_some());
    }

    #[test]
    fn the_observe_tap_still_sees_everything_a_consent_grab_swallows() {
        // P1.7.3's revocation watcher rides `observe`, and the one moment
        // it must not go deaf is the one moment a consent prompt is up.
        // Asserted through the real router with a real grab: for each
        // consumed event the tap's count rises and the inner *gate*'s does
        // not, which is the short-circuit the wrapper documents.
        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());
        rig.raise_awake(petition, &mut registry);

        let gated_before = rig.spy.gated.get();
        let consumed = [
            motion(20.0, 20.0),
            press(),
            scroll(),
            SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff1b, // Escape: the revocation chord itself
                state: KeyState::Pressed,
            },
        ];
        let observed_before = rig.spy.observed.get();
        for kind in consumed.iter() {
            assert!(
                rig.router
                    .route_physical(phys(kind.clone()), view, surface_size)
                    .is_none(),
                "fixture check: {kind:?} must be consumed here"
            );
        }
        assert_eq!(
            rig.spy.observed.get() - observed_before,
            consumed.len(),
            "the tap must see every event the grab swallowed"
        );
        assert_eq!(
            rig.spy.gated.get(),
            gated_before,
            "a consumed event must not reach the inner gate"
        );
    }

    #[test]
    fn a_prompt_raised_mid_drag_leaves_no_stuck_button_in_the_app() {
        // The pairing contract (`PreemptionHook`), exercised against the
        // real grab: a prompt that appears mid-drag must not strand the
        // press the app already saw. The release is delivered; the press
        // that answers the prompt is not.
        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());

        // A drag begins in the app.
        assert!(rig
            .router
            .route_physical(phys(motion(40.0, 40.0)), view, surface_size)
            .is_some());
        assert!(rig
            .router
            .route_physical(phys(press()), view, surface_size)
            .is_some());

        rig.raise_awake(petition, &mut registry);

        // Mid-drag motion is consumed (the app must not track the pointer
        // across a security decision) ...
        assert!(rig
            .router
            .route_physical(phys(motion(50.0, 50.0)), view, surface_size)
            .is_none());
        // ... but the release still reaches the app: no stuck button.
        assert!(
            rig.router
                .route_physical(phys(release()), view, surface_size)
                .is_some(),
            "hold-until-release: the drag's own release must land"
        );
        // A fresh press aimed at the prompt is consumed, and its release
        // is dropped unpaired by the router rather than leaking.
        assert!(rig
            .router
            .route_physical(phys(press()), view, surface_size)
            .is_none());
        assert!(rig
            .router
            .route_physical(phys(release()), view, surface_size)
            .is_none());
    }

    #[test]
    fn a_prompt_raised_mid_keystroke_leaves_no_latched_modifier_in_the_app() {
        // The keyboard twin of the test above, and the sharper of the two.
        // A prompt appearing while the human holds Shift must not strand
        // that key down in the confined app: everything the human typed
        // afterwards would silently arrive shifted. The grab never
        // consumes a release, and the router's per-keysym pairing is what
        // makes that safe -- a release whose press the grab DID consume is
        // dropped here, not leaked.
        const SHIFT_L: u32 = 0xffe1;
        const CTRL_L: u32 = 0xffe3;
        let key = |keysym, state| SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym,
            state,
        };

        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());

        // The human is holding Shift when the prompt appears.
        assert!(rig
            .router
            .route_physical(phys(key(SHIFT_L, KeyState::Pressed)), view, surface_size)
            .is_some());
        rig.raise_awake(petition, &mut registry);

        // Key presses during the prompt are consumed ...
        assert!(rig
            .router
            .route_physical(phys(key(CTRL_L, KeyState::Pressed)), view, surface_size)
            .is_none());
        // ... and that press's own release is dropped by the router's
        // pairing rather than arriving unpaired at the app.
        assert!(
            rig.router
                .route_physical(phys(key(CTRL_L, KeyState::Released)), view, surface_size)
                .is_none(),
            "a release whose press the grab consumed must not reach the app"
        );
        // But the Shift the app already saw go down must come back up.
        assert!(
            rig.router
                .route_physical(phys(key(SHIFT_L, KeyState::Released)), view, surface_size)
                .is_some(),
            "hold-until-release: a modifier held from before the prompt must be released"
        );

        // With the prompt down, typing is unmodified again -- and a second
        // Shift release, now unpaired, is dropped rather than doubled.
        rig.lower(&mut registry);
        assert!(rig
            .router
            .route_physical(phys(key(SHIFT_L, KeyState::Released)), view, surface_size)
            .is_none());
    }

    #[test]
    fn clicking_a_prompt_button_produces_a_decision_and_no_app_input() {
        // The whole grab, end to end through the router: the click that
        // answers the prompt yields a decision and delivers nothing to the
        // app -- not the press, not the release, not the motion.
        use crate::consent::Choice;

        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());
        rig.raise_awake(petition, &mut registry);

        // Aim at Deny, using the same placement the compositor draws with.
        let card = rig
            .surface
            .card_origin(view.output().0, view.output().1)
            .expect("a prompt is up");
        let rendered = crate::consent::render::rasterize(
            registry.prompt_content(petition).as_ref().expect("pending"),
        );
        let deny = rendered
            .buttons
            .iter()
            .find(|b| b.choice == Choice::Deny)
            .expect("Deny is always offered");
        let target = (
            f64::from(card.0 + deny.rect.x) + f64::from(deny.rect.w) / 2.0,
            f64::from(card.1 + deny.rect.y) + f64::from(deny.rect.h) / 2.0,
        );

        for kind in [motion(target.0, target.1), press(), release()] {
            assert!(
                rig.router
                    .route_physical(phys(kind), view, surface_size)
                    .is_none(),
                "answering the prompt must deliver nothing to the app"
            );
        }
        assert_eq!(
            rig.grab.borrow_mut().take_decision().map(|d| d.choice),
            Some(Choice::Deny)
        );

        // And the decision resolves the petition through the production
        // path, with the protocol's clean denial.
        let resolution = registry
            .resolve_human(petition, Choice::Deny)
            .expect("the petition is still pending");
        assert!(matches!(
            resolution.verdict,
            crate::petitions::Verdict::Declined {
                outcome: vitrin_protocol::generated::vitrin_grant::Outcome::Denied
            }
        ));
    }

    #[test]
    fn an_actuation_sent_mid_prompt_never_reaches_the_app() {
        // The M1.4 input-echo acceptance criterion, closed end to end: a
        // real chokepoint decision, a real router, a real wire, and a real
        // mock shim reading its seat. While the agent's own prompt is up
        // the chokepoint refuses `consent_held` and the sink is never
        // called, so nothing rides the wire; once the prompt closes the
        // same actuation arrives at the shim, origin-tagged.
        use crate::enforcement::{Chokepoint, UseEnv, UseKind, UseOutcome, UseRequest};
        use crate::grants::{GrantSpec, GrantTable, Issuer, PersistenceRung, RealmId, ResourceRef};
        use vitrin_protocol::generated::vitrin_grant::Refusal;
        use vitrin_protocol::generated::vitrin_grant::Verb;

        let _fd = crate::capture::tests::fd_lock();
        let (server, _scene, mut core, mut mock) = wire_setup();
        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (VIEW_W, VIEW_H).into();
        let surface_size = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());

        let now = std::time::Instant::now();
        let identity =
            crate::identity::PrincipalIdentity::parse(crate::consent::tests::PROMPT_IDENTITY)
                .expect("fixture identity");
        let mut grants = GrantTable::new();
        let row = grants
            .insert(
                GrantSpec {
                    principal_id: identity.clone(),
                    realm_id: RealmId::new("realm-0"),
                    resource_ref: ResourceRef::WholeRealm,
                    verbs: Verb::ACTUATE_POINTER,
                    expiry: None,
                    max_event_rate: std::num::NonZeroU32::new(20).unwrap(),
                    persistence: PersistenceRung::WhileRunning,
                    issuer: Issuer::HumanConsent,
                },
                now,
            )
            .expect("a valid row");

        let rgba = vec![0u8; VIEW_W as usize * VIEW_H as usize * 4];
        let frame = crate::capture::RealmViewFrame {
            rgba: &rgba,
            width: VIEW_W,
            height: VIEW_H,
        };
        let presence = PhysicalPresenceMap::new();
        // The realm the grant row names, and so the realm an admitted
        // actuation is addressed to (WS-E.1.6).
        let grant_realm = crate::grants::RealmId::new("realm-0");
        let mut chokepoint = Chokepoint::new();

        // One actuation attempt: chokepoint, then -- only if it admitted
        // -- the router, then the shim wire. The M1.1 shape in miniature.
        //
        // The chokepoint's own `send` is discarded: it addresses the
        // *principal* connection (this is where `refused` would go), which
        // is a different socket from the shim's. Routing it into `core`
        // would put principal-protocol bytes on the shim wire and prove
        // nothing about the seat. What this test asserts is what the app
        // sees, and `UseOutcome` already reports the refusal.
        let actuate = |chokepoint: &mut Chokepoint,
                       grants: &mut GrantTable,
                       registry: &crate::petitions::PetitionRegistry,
                       router: &mut InputRouter<crate::consent::grab::ConsentGate<SpyHook>>,
                       core: &mut Connection| {
            let mut routed: Vec<(crate::grants::RealmId, SeatInput)> = Vec::new();
            let outcome = chokepoint
                .enforce_use(
                    UseRequest {
                        facet_id: 20,
                        grant_wire_id: 10,
                        grant_row: Some(row),
                        principal: &identity,
                        // View coordinates -- what an agent reads off a
                        // capture -- so the row it names must be one the app
                        // actually has: the app's origin is
                        // `reserved_top()` rows down (issue #304), and the
                        // wire assertion below is the surface-local answer.
                        kind: UseKind::Pointer(SeatInputKind::Motion {
                            x: 5.0,
                            y: 6.0 + f64::from(view.reserved_top()),
                        }),
                    },
                    grants,
                    registry,
                    UseEnv {
                        realm_view: Some(&frame),
                        presence: &presence,
                        attention: &std::cell::RefCell::new(
                            crate::attention::AttentionSignal::detached(),
                        ),
                        // No human anywhere: preemption is not this test's
                        // subject and must not silently supply its refusals.
                        physical_realm: None,
                        actuations: &mut |realm, input| routed.push((realm.clone(), input)),
                        grant_realm: Some(&grant_realm),
                        layout: &mut |act| panic!("no layout act expected: {act:?}"),
                        // Not a launch test; a launch reaching this sink
                        // would be the bug, so it panics rather than
                        // silently answering.
                        launch: &mut |ask| panic!("no launch expected: {ask:?}"),
                    },
                    now,
                    &mut |_principal_frame, _fd| Ok(()),
                )
                .expect("transport is healthy");
            for (realm, input) in routed {
                if let Some(delivery) = router.route_emulated(&realm, input, view, surface_size) {
                    server
                        .deliver_seat_event(&delivery, &mut |frame| core.send_message(frame, None))
                        .expect("send");
                }
            }
            outcome
        };

        // Prompt up: refused `consent_held`, nothing routed, nothing sent.
        rig.raise_awake(petition, &mut registry);
        let outcome = actuate(
            &mut chokepoint,
            &mut grants,
            &registry,
            &mut rig.router,
            &mut core,
        );
        assert!(
            matches!(
                outcome,
                UseOutcome::Refused {
                    code: Refusal::ConsentHeld,
                    ..
                }
            ),
            "expected consent_held, got {outcome:?}"
        );

        // The refusal rode the wire; the seat did not. Drain the refusal
        // the chokepoint voiced, then assert the next thing the shim sees
        // is the post-prompt actuation and not a mid-prompt one.
        rig.lower(&mut registry);
        registry
            .resolve_human(petition, crate::consent::Choice::Deny)
            .expect("still pending");
        let outcome = actuate(
            &mut chokepoint,
            &mut grants,
            &registry,
            &mut rig.router,
            &mut core,
        );
        assert!(
            matches!(outcome, UseOutcome::Admitted { .. }),
            "the prompt is down: the same actuation must now be admitted"
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Motion {
                x: Fixed::from_f64(5.0),
                y: Fixed::from_f64(6.0),
                origin: Origin::Emulated,
            },
            "the FIRST seat event the app ever sees is the post-prompt one"
        );
    }

    // ------------------------------------------------------------------
    // The dead-man switch through the real router (P1.7.3)
    //
    // Here for the same reason the P1.7.2 block above is: this is the only
    // module that can mint a physical-origin event, so it is the only place
    // the human's off-switch can be driven the way nested intake drives it.
    // ------------------------------------------------------------------

    /// A router carrying the production hook stack — P1.7.2's consent grab
    /// over P1.7.3's dead-man watcher over a spy — plus everything needed to
    /// raise a prompt and inspect the switch. This is exactly what
    /// `backend::winit` builds, with a spy in place of the `NoopHook`.
    struct Guarded {
        router:
            InputRouter<crate::consent::grab::ConsentGate<crate::deadman::DeadManHook<SpyHook>>>,
        grab: Rc<RefCell<crate::consent::grab::ConsentGrab>>,
        deadman: Rc<RefCell<crate::deadman::DeadManSwitch>>,
        surface: crate::consent::ConsentSurface,
        now: Rc<Cell<std::time::Instant>>,
        spy: Rc<HookSpy>,
        recorder: crate::recorder::Recorder,
        log_path: std::path::PathBuf,
    }

    impl Guarded {
        fn new() -> Self {
            let grab = Rc::new(RefCell::new(crate::consent::grab::ConsentGrab::new()));
            let deadman = Rc::new(RefCell::new(crate::deadman::DeadManSwitch::new(
                crate::deadman::DeadManConfig::default(),
            )));
            let now = Rc::new(Cell::new(std::time::Instant::now()));
            let spy = Rc::new(HookSpy::default());
            let mut router = InputRouter::detached(crate::consent::grab::ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                crate::deadman::DeadManHook::new(
                    Rc::clone(&deadman),
                    Rc::clone(&now),
                    SpyHook(Rc::clone(&spy)),
                ),
            ));
            // The human's attention is on the rig's one realm.
            assert!(router.bind_to(&test_realm()).is_none());
            let (recorder, log_path) = crate::recorder::tests::scratch_recorder("input-deadman");
            Self {
                router,
                grab,
                deadman,
                surface: crate::consent::ConsentSurface::new(
                    crate::consent::TrustedIndicator::for_test(),
                ),
                now,
                spy,
                recorder,
                log_path,
            }
        }
    }

    impl Drop for Guarded {
        fn drop(&mut self) {
            crate::recorder::tests::cleanup(&self.log_path);
        }
    }

    #[test]
    fn the_dead_man_chord_completes_while_a_consent_prompt_owns_all_input() {
        // The whole reason the watcher rides `observe` rather than `gate`.
        // A consent prompt consumes every physical event before the inner
        // gate is even consulted -- and the moment a prompt is on screen is
        // exactly the moment a human might most want to end everything. The
        // off-switch must survive it, and it must survive it *through the
        // real grab*, not a stand-in.
        let mut rig = Guarded::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());

        let t0 = rig.now.get();
        rig.grab
            .borrow_mut()
            .raise(
                petition,
                t0,
                &mut registry,
                &mut rig.surface,
                &mut rig.recorder,
            )
            .expect("the petition is pending");
        rig.now.set(t0 + crate::consent::grab::GUARD_INTERVAL);

        // The human presses the chord while the prompt owns input. Nothing
        // is delivered -- the grab consumed it, and the inner gate was never
        // even reached.
        let gated_before = rig.spy.gated.get();
        assert!(rig
            .router
            .route_physical(phys(chord_press().kind().clone()), view, surface)
            .is_none());
        assert_eq!(
            rig.spy.gated.get(),
            gated_before,
            "fixture check: the grab must short-circuit the inner gate here"
        );

        // ... and the switch armed anyway, because `observe` is forwarded
        // unconditionally.
        let armed_at = rig.now.get();
        assert_eq!(
            rig.deadman.borrow().deadline(),
            Some(armed_at + crate::deadman::DEFAULT_HOLD),
            "the chord did not arm under a consent grab"
        );

        // The hold elapses with no further input of any kind.
        rig.deadman
            .borrow_mut()
            .fire_if_due(armed_at + crate::deadman::DEFAULT_HOLD);
        let trigger = rig
            .deadman
            .borrow_mut()
            .take_trigger()
            .expect("a consent prompt must not be able to veto the off-switch");
        assert_eq!(trigger.chord, "esc");
    }

    #[test]
    fn an_agent_flooding_the_router_cannot_defeat_a_hold_in_progress() {
        // The adversary with an actuation grant: it cannot arm the switch,
        // and -- the sharper direction -- it cannot disarm one the human has
        // armed by flooding the router with emulated input, including a
        // forged chord release. Driven through `route`, so the flood takes
        // the real path an admitted actuation takes.
        let mut rig = Guarded::new();
        let view: ViewGeometry = (900, 700).into();
        let surface = Some(view.usable());
        let t0 = rig.now.get();

        assert!(rig
            .router
            .route_physical(phys(chord_press().kind().clone()), view, surface)
            .is_none());
        for step in 0..200 {
            rig.now.set(t0 + std::time::Duration::from_millis(step * 5));
            let _ = rig
                .router
                .route_emulated(&test_realm(), emulated_chord_press(), view, surface);
            let _ =
                rig.router
                    .route_emulated(&test_realm(), emulated_chord_release(), view, surface);
            let _ = rig
                .router
                .route_emulated(&test_realm(), emulated_motion(), view, surface);
        }
        rig.deadman
            .borrow_mut()
            .fire_if_due(t0 + crate::deadman::DEFAULT_HOLD);
        assert!(
            rig.deadman.borrow_mut().take_trigger().is_some(),
            "an agent flooding the router defeated the human's off-switch"
        );
    }

    #[test]
    fn a_prompt_between_a_taps_press_and_release_never_splits_a_later_chord() {
        // The replay-accounting bug this rig exists to catch, driven through
        // the exact production hook stack (`ConsentGate<DeadManHook<..>>`)
        // with a real grab and a real petition.
        //
        // The sequence is ordinary, and the agent chooses its timing (it
        // decides when to petition, hence when the prompt goes up):
        //
        //   1. the human taps Esc with no prompt up -- the press is withheld;
        //   2. a petition raises a prompt before the release arrives;
        //   3. the tap is classified and its pair drained and re-routed. The
        //      grab CONSUMES the replayed press but DELIVERS the replayed
        //      release (its documented hold-until-release exception), so only
        //      one half of the pair ever reaches the dead-man gate.
        //
        // With suppression modelled as a bare counter, step 3 left an odd
        // credit, and the human's next real chord was split down the middle:
        // press delivered to the app, release consumed -- Escape latched down
        // in the confined app immediately after the panic button. That breaks
        // both "the shim never sees the held chord" and `PreemptionHook`'s
        // pairing exception at once.
        let mut rig = Guarded::new();
        let (mut registry, petition) = pending_petition();
        let view: ViewGeometry = (900, 700).into();
        let surface = Some(view.usable());
        rig.grab.borrow_mut().set_view(view.output());
        let t0 = rig.now.get();

        // 1. The tap's press, no prompt up: withheld.
        assert!(rig
            .router
            .route_physical(phys(chord_press().kind().clone()), view, surface)
            .is_none());

        // 2. A prompt goes up mid-tap.
        rig.grab
            .borrow_mut()
            .raise(
                petition,
                t0,
                &mut registry,
                &mut rig.surface,
                &mut rig.recorder,
            )
            .expect("the petition is pending");

        // 3. The release lands, classifying the tap; drain and re-route the
        //    pair exactly as the backend does.
        rig.now.set(t0 + std::time::Duration::from_millis(80));
        assert!(rig
            .router
            .route_physical(phys(chord_release().kind().clone()), view, surface)
            .is_none());
        let replay = rig.deadman.borrow_mut().take_replay();
        assert_eq!(replay.len(), 2, "fixture check: a tap replays a pair");
        for input in replay {
            let _ = rig.router.route_physical(input, view, surface);
        }

        // The prompt is answered and lowered; input is ordinary again.
        rig.grab.borrow_mut().lower(&mut registry, &mut rig.surface);

        // Now the human holds the chord for real. The app must see NEITHER
        // half -- not the press on a stale pass, and so never a release
        // without it.
        let t1 = t0 + std::time::Duration::from_secs(5);
        rig.now.set(t1);
        assert!(
            rig.router
                .route_physical(phys(chord_press().kind().clone()), view, surface)
                .is_none(),
            "the held chord's press reached the app on a stale replay pass"
        );
        rig.deadman
            .borrow_mut()
            .fire_if_due(t1 + crate::deadman::DEFAULT_HOLD);
        assert!(
            rig.deadman.borrow_mut().take_trigger().is_some(),
            "fixture check: the hold must have completed"
        );
        rig.now
            .set(t1 + crate::deadman::DEFAULT_HOLD + std::time::Duration::from_millis(10));
        assert!(
            rig.router
                .route_physical(phys(chord_release().kind().clone()), view, surface)
                .is_none(),
            "the chord's release reached the app"
        );
        // And nothing is latched: the router holds no key it delivered a
        // press for, which is the property a split pair violates.
        assert!(
            rig.router.held_keys(&test_realm()).is_empty(),
            "a chord left a key latched down in the confined app: {:?}",
            rig.router.held_keys(&test_realm())
        );
    }

    #[test]
    fn the_held_chord_never_reaches_the_shim_but_a_tap_does() {
        // Design tension 1, end to end over the real wire: the app must keep
        // its Escape key (Firefox closes dialogs with it) and must never see
        // the chord that revoked the session's authority.
        //
        // Both halves are asserted against one mock shim in one sequence, so
        // "the chord was suppressed" cannot pass by the seat being broken:
        // a sentinel key proves the chord's silence positively -- see the
        // comment at the sentinel below for why the earlier version of this
        // test could not tell the chord and the tap apart at all.
        let _fd = crate::capture::tests::fd_lock();
        let (server, _scene, mut core, mut mock) = wire_setup();
        let view: ViewGeometry = (VIEW_W, VIEW_H).into();
        let surface = Some(view.usable());
        let host_view = (VIEW_W as i32, VIEW_H as i32);

        let deadman = Rc::new(RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        )));
        let now = Rc::new(Cell::new(std::time::Instant::now()));
        let mut router = InputRouter::detached(crate::deadman::DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            NoopHook,
        ));
        assert!(router.bind_to(&test_realm()).is_none());

        // The embedder's loop, condensed: route each intake event, then
        // drain whatever the watcher decided the app is owed.
        let pump = |router: &mut InputRouter<crate::deadman::DeadManHook<NoopHook>>,
                    core: &mut Connection,
                    inputs: Vec<SeatInput>| {
            let send = |router: &mut InputRouter<crate::deadman::DeadManHook<NoopHook>>,
                        core: &mut Connection,
                        input| {
                if let Some(delivery) = router.route_physical(input, view, surface) {
                    server
                        .deliver_seat_event(&delivery, &mut |frame| core.send_message(frame, None))
                        .expect("send");
                }
            };
            for input in inputs {
                send(router, core, input);
            }
            let replay = deadman.borrow_mut().take_replay();
            for input in replay {
                send(router, core, input);
            }
        };

        // --- A held chord: press, hold past the deadline, release. ---
        let t0 = now.get();
        pump(
            &mut router,
            &mut core,
            intake_physical(&key_ev(1, host::KeyState::Pressed), host_view),
        );
        deadman
            .borrow_mut()
            .fire_if_due(t0 + crate::deadman::DEFAULT_HOLD);
        assert!(
            deadman.borrow_mut().take_trigger().is_some(),
            "fixture check: the chord must have completed"
        );
        let released_at = t0 + crate::deadman::DEFAULT_HOLD + std::time::Duration::from_millis(300);
        now.set(released_at);
        pump(
            &mut router,
            &mut core,
            intake_physical(&key_ev(1, host::KeyState::Released), host_view),
        );

        // --- A sentinel the chord cannot be mistaken for. ---
        //
        // Without this the test proved nothing. The chord's pair and the tap's
        // pair are byte-identical on the wire (same keysym, same states, same
        // origin), and the assertions below cannot tell which pair they are
        // reading -- so gutting `gate_event` to deliver everything left this
        // test green while the app received the whole revocation gesture. A
        // key the chord path never touches makes the two discriminable: if the
        // chord leaked, the first event the app sees is Escape, not Return.
        now.set(released_at + std::time::Duration::from_millis(1));
        pump(
            &mut router,
            &mut core,
            intake_physical(&key_ev(28, host::KeyState::Pressed), host_view),
        );

        // --- Then an ordinary tap of the same key. ---
        let tapped_at = released_at + std::time::Duration::from_secs(1);
        now.set(tapped_at);
        pump(
            &mut router,
            &mut core,
            intake_physical(&key_ev(1, host::KeyState::Pressed), host_view),
        );
        now.set(tapped_at + std::time::Duration::from_millis(80));
        pump(
            &mut router,
            &mut core,
            intake_physical(&key_ev(1, host::KeyState::Released), host_view),
        );

        // The app's whole view of the session, in order. The FIRST event is
        // the sentinel, which is only possible if the chord's press and its
        // release both produced nothing -- so this asserts the chord's silence
        // positively, without a non-blocking read, and fails if `gate_event`
        // ever stops withholding.
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Key {
                keysym: 0xff0d,
                state: KeyState::Pressed,
                origin: Origin::Physical,
            },
            "the FIRST thing the app ever sees must be the sentinel: the chord left no trace"
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Key {
                keysym: 0xff1b,
                state: KeyState::Pressed,
                origin: Origin::Physical,
            },
            "then the tap's press, replayed after the gate classified it as a tap"
        );
        assert_eq!(
            mock.next_seat_event().unwrap(),
            SeatEvent::Key {
                keysym: 0xff1b,
                state: KeyState::Released,
                origin: Origin::Physical,
            },
            "a tap must arrive as a complete pair: a press with no release latches Escape"
        );
    }

    #[test]
    fn a_chord_press_leaves_no_latched_key_in_the_router() {
        // The harm this design exists to avoid, checked at the router's own
        // pairing table: the chord's press is never delivered, so its
        // release cannot strand anything -- and a later, ordinary tap still
        // pairs normally rather than being eaten as a stale release.
        let deadman = Rc::new(RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        )));
        let now = Rc::new(Cell::new(std::time::Instant::now()));
        let mut router = InputRouter::detached(crate::deadman::DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            NoopHook,
        ));
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());
        let t0 = now.get();

        assert!(router
            .route_physical(chord_press(), view, surface)
            .is_none());
        deadman
            .borrow_mut()
            .fire_if_due(t0 + crate::deadman::DEFAULT_HOLD);
        assert!(deadman.borrow_mut().take_trigger().is_some());
        assert!(router
            .route_physical(chord_release(), view, surface)
            .is_none());
        assert!(
            router.held_keys(&test_realm()).is_empty(),
            "the chord latched a key in the router's pairing table: {:?}",
            router.held_keys(&test_realm())
        );
        assert!(
            deadman.borrow_mut().take_replay().is_empty(),
            "a completed chord owes the app nothing"
        );
    }

    /// **A key held across a focus change comes up in the app, not latched
    /// down in it.**
    ///
    /// The hazard the nested backend's `handle_focus` reaches for this
    /// method to close: on Wayland, `wl_keyboard.leave` produces no key
    /// events at all, so the physical release the human eventually performs
    /// is delivered to whatever window took focus and the core never hears
    /// about it. Left alone, the confined app holds that key forever — a
    /// stuck Ctrl or Shift the human has no way to clear.
    ///
    /// The pairing invariant is the whole safety argument, so it is asserted
    /// on all three sides: exactly the delivered presses are released, in
    /// press order; a press the gate consumed (the dead-man chord's) gets
    /// nothing, because the app never saw it go down; and a physical release
    /// that arrives afterwards — X11's synthetic focus-out release, which
    /// `admits_key_event` now lets through — finds nothing to pair with and
    /// is dropped rather than double-releasing.
    ///
    /// All presses here are physical; the drain's *other* half — that an
    /// agent's held key is neither released nor re-tagged as the human's —
    /// is [`a_focus_drain_never_speaks_for_an_agents_held_key`].
    #[test]
    fn a_key_held_across_a_focus_change_is_released_not_latched() {
        let deadman = Rc::new(RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        )));
        let now = Rc::new(Cell::new(std::time::Instant::now()));
        let mut router = InputRouter::detached(crate::deadman::DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            NoopHook,
        ));
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());

        // Two ordinary keys go down and are delivered, then the chord goes
        // down and is withheld by the gate.
        for keysym in [NON_CHORD_KEYSYM, 0x061] {
            assert!(
                router
                    .route_physical(
                        phys(SeatInputKind::Key {
                            source: KeySource::Keysym,
                            keysym,
                            state: KeyState::Pressed
                        }),
                        view,
                        surface,
                    )
                    .is_some(),
                "fixture check: an ordinary key press reaches the app"
            );
        }
        assert!(
            router
                .route_physical(chord_press(), view, surface)
                .is_none(),
            "fixture check: the chord's press is withheld"
        );

        // Focus leaves. Every key the app is holding — and only those — is
        // released, in press order.
        let released = router.release_physical_keys(&test_realm());
        assert_eq!(
            released
                .iter()
                .map(|d| (d.origin(), d.kind().clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    Origin::Physical,
                    SeatDeliveryKind::Key {
                        keysym: NON_CHORD_KEYSYM,
                        state: KeyState::Released
                    }
                ),
                (
                    Origin::Physical,
                    SeatDeliveryKind::Key {
                        keysym: 0x061,
                        state: KeyState::Released
                    }
                ),
            ],
            "focus loss must release exactly the keys whose press the app saw, in order -- \
             a missing one stays latched down in the confined app forever"
        );
        assert!(
            router.held_keys(&test_realm()).is_empty(),
            "the router still believes a key is down after focus loss: {:?}",
            router.held_keys(&test_realm())
        );

        // Idempotent: a second focus-out (or a `handle_focus` racing the
        // synthetic releases) owes nothing.
        assert!(router.release_physical_keys(&test_realm()).is_empty());

        // And X11's synthetic release, arriving after the drain, pairs with
        // nothing and is dropped -- the app is never told a key came up twice.
        assert!(
            router
                .route_physical(
                    phys(SeatInputKind::Key {
                        source: KeySource::Keysym,
                        keysym: NON_CHORD_KEYSYM,
                        state: KeyState::Released
                    }),
                    view,
                    surface,
                )
                .is_none(),
            "an already-paid release must not reach the app a second time"
        );

        // The pointer and its implicit grab are untouched: a focus change is
        // not a new shim generation, which is what `reset_for` is for.
        let mut router = InputRouter::detached(NoopHook);
        assert!(router.bind_to(&test_realm()).is_none());
        assert!(router
            .route_physical(phys(motion(10.0, 10.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
        assert!(router.release_physical_keys(&test_realm()).is_empty());
        assert!(
            router
                .route_physical(phys(release()), view, surface)
                .is_some(),
            "releasing held keys must not drop the pointer's implicit grab"
        );
    }

    /// **A human's focus change never speaks for an agent's keyboard.**
    ///
    /// `session::route_seat` routes chokepoint-admitted actuations through
    /// this same router, deliberately, so the pairing table holds an agent's
    /// held keys next to a human's. The first shape of the focus drain
    /// stamped `Origin::Physical` on everything it drained, which is two
    /// defects at once:
    ///
    /// 1. **Origin forgery inside the TCB.** The wire event and the flight
    ///    recorder's `SeatDelivered` entry would both say a human released a
    ///    key no human touched — the physical-vs-emulated distinction (PRD
    ///    Doc 2 §8, requirement B2) that [`SeatInput::physical`]'s privacy
    ///    makes a compile error at intake, laundered back in on the way out.
    /// 2. **Silently dropping an agent's modifier.** The agent is told
    ///    nothing and its model of the keyboard diverges from the app's.
    ///
    /// Both are closed by draining only the entries the table recorded as
    /// physical, so this asserts the emulated press is neither released nor
    /// re-tagged, that its own release still pairs afterwards, and that the
    /// human's key sitting beside it is still released (the fix must not
    /// have traded one stuck key for another).
    #[test]
    fn a_focus_drain_never_speaks_for_an_agents_held_key() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());
        const AGENT_KEY: u32 = 0xffe1; // Shift_L, an agent holding a modifier
        const HUMAN_KEY: u32 = 0x062; // 'b'

        for input in [
            SeatInput::emulated(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: AGENT_KEY,
                state: KeyState::Pressed,
            }),
            phys(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: HUMAN_KEY,
                state: KeyState::Pressed,
            }),
        ] {
            assert!(
                route_by_origin(&mut router, input, view, surface).is_some(),
                "fixture check: both origins' presses reach the app"
            );
        }

        // The human alt-tabs. Exactly one release goes out, it is the human's
        // key, and it carries the human's tag.
        let released = router.release_physical_keys(&test_realm());
        assert_eq!(
            released
                .iter()
                .map(|d| (d.origin(), d.kind().clone()))
                .collect::<Vec<_>>(),
            vec![(
                Origin::Physical,
                SeatDeliveryKind::Key {
                    keysym: HUMAN_KEY,
                    state: KeyState::Released
                }
            )],
            "a focus drain must release the human's key and only the human's: an agent's \
             held key released here is an origin forgery on the wire and in the journal"
        );
        assert_eq!(
            router.held_keys(&test_realm()),
            [(AGENT_KEY, Origin::Emulated)],
            "the agent's press must survive a human's focus change, with its own tag"
        );

        // The agent's own release still pairs and still goes out as the
        // agent's -- the drain neither paid this debt nor forgot it.
        let delivery = router
            .route_emulated(
                &test_realm(),
                SeatInput::emulated(SeatInputKind::Key {
                    source: KeySource::Keysym,
                    keysym: AGENT_KEY,
                    state: KeyState::Released,
                }),
                view,
                surface,
            )
            .expect("an agent's release of its own held key must reach the app");
        assert_eq!(delivery.origin(), Origin::Emulated);
        assert_eq!(
            delivery.kind(),
            &SeatDeliveryKind::Key {
                keysym: AGENT_KEY,
                state: KeyState::Released
            }
        );
        assert!(router.held_keys(&test_realm()).is_empty());
    }

    /// **One dispatch round, two realms, two rules** (WS-E.1.6, issue #212,
    /// acceptance criterion 2).
    ///
    /// Realm A is bound. In the same round an agent's motion under a grant
    /// over realm **B** is delivered against B's geometry, and the human's
    /// motion is delivered against A's — and neither realm's seat state is
    /// touched by the other's event. The geometries are deliberately
    /// *different*, which is what makes the mapping assertions discriminate:
    /// with equal surfaces every coordinate would arrive unchanged whichever
    /// realm the router picked.
    ///
    /// (Per-realm **view** sizes do not exist — one output, `scene::realms`
    /// — so it is the two realms' committed *surfaces* that differ here,
    /// which is exactly what `route_physical`/`route_emulated` are handed and
    /// exactly what a router picking the wrong realm would get wrong.)
    #[test]
    fn a_round_delivers_the_agent_to_its_grants_realm_and_the_human_to_the_bound_one() {
        let mut router = InputRouter::detached(NoopHook);
        let a = crate::grants::RealmId::new("realm-a");
        let b = crate::grants::RealmId::new("realm-b");
        let view: ViewGeometry = (100, 80).into();
        // A fills its usable view -- the size `configure` gave it; B is
        // letterboxed inside the same rectangle, placed at (30, 34).
        let surface_a = Some(view.usable());
        let surface_b = Some((40, 20));
        assert!(router.bind_to(&a).is_none());

        // The agent, into the realm nobody is watching, through B's placement.
        let to_b = router
            .route_emulated(&b, SeatInput::emulated(motion(35.0, 35.0)), view, surface_b)
            .expect("an agent's motion must reach the realm its grant names");
        assert_motion(&to_b, 5.0, 1.0);
        assert_eq!(to_b.origin(), Origin::Emulated);

        // The human, in the same round, into the bound realm through A's.
        let to_a = router
            .route_physical(phys(motion(35.0, 35.0)), view, surface_a)
            .expect("the human's motion must reach the bound realm");
        assert_motion(&to_a, 35.0, 35.0 - f64::from(view.reserved_top()));
        assert_eq!(to_a.origin(), Origin::Physical);

        // Neither realm's pointer state is the other's: the agent's sprite
        // position exists only for B, and A's implicit-grab hit test uses
        // only what the human moved.
        assert_eq!(router.agent_pointer(&b), Some((35.0, 35.0)));
        assert_eq!(
            router.agent_pointer(&a),
            None,
            "an agent working in B must not put a cursor position into A"
        );

        // A press in each realm pairs only in its own realm.
        assert!(router
            .route_emulated(&b, SeatInput::emulated(press()), view, surface_b)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface_a)
            .is_some());
        assert_eq!(router.held_buttons(&a), [(0x110, Origin::Physical)]);
        assert_eq!(router.held_buttons(&b), [(0x110, Origin::Emulated)]);

        // ...and the human's release pays down only A's.
        assert!(router
            .route_physical(phys(release()), view, surface_a)
            .is_some());
        assert!(router.held_buttons(&a).is_empty());
        assert_eq!(
            router.held_buttons(&b),
            [(0x110, Origin::Emulated)],
            "a human's release in the bound realm must not end an agent's drag in another"
        );
    }

    /// **A binding change drains the human's held presses out of the realm
    /// being left — keys and buttons — and leaves the agent's alone.**
    ///
    /// The acceptance criterion of issue #212, first bullet, and the sibling
    /// of [`a_focus_drain_never_speaks_for_an_agents_held_key`] — now with the
    /// *same* filter rather than the opposite one, which is the change
    /// WS-E.1.6 makes. Before it, a `layout_focus` holder moving the output
    /// closed the agent's channel too (the session had one delivery target, and
    /// the chokepoint refused an actuation naming any other realm), so an
    /// agent's entry left behind was stranded for good and the drain paid it.
    /// Seat delivery is per realm now: the agent still reaches the realm it
    /// holds a grant over, so releasing its key here would invent an act it
    /// never performed and put the human's tag on it.
    ///
    /// Buttons are drained here and not on host-window focus loss because a
    /// realm switch really does move the human's pointer away, grab and all,
    /// where a lost keyboard focus says nothing about a drag still in progress
    /// (see `backend::winit`'s `handle_focus`).
    #[test]
    fn a_binding_change_drains_the_humans_held_presses_and_only_the_humans() {
        let mut router = InputRouter::detached(NoopHook);
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());
        let a = crate::grants::RealmId::new("realm-a");
        let b = crate::grants::RealmId::new("realm-b");
        const HUMAN_KEY: u32 = 0xffe3; // Control_L
        const AGENT_KEY: u32 = 0xffe1; // Shift_L
        const HUMAN_BUTTON: u32 = 0x110; // BTN_LEFT

        assert!(router.bind_to(&a).is_none(), "nothing was bound before");
        assert!(router
            .route_physical(phys(motion(10.0, 10.0)), view, surface)
            .is_some());
        for input in [
            phys(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: HUMAN_KEY,
                state: KeyState::Pressed,
            }),
            phys(SeatInputKind::Button {
                button: HUMAN_BUTTON,
                state: ButtonState::Pressed,
            }),
        ] {
            assert!(
                router.route_physical(input, view, surface).is_some(),
                "fixture check: every press must reach the app, or there is nothing to strand"
            );
        }
        assert!(
            router
                .route_emulated(
                    &a,
                    SeatInput::emulated(SeatInputKind::Key {
                        source: KeySource::Keysym,
                        keysym: AGENT_KEY,
                        state: KeyState::Pressed,
                    }),
                    view,
                    surface,
                )
                .is_some(),
            "fixture check: the agent's press must reach the app too"
        );

        // The human's attention moves. Everything *they* left holding comes
        // back, keys first, each carrying the tag its own entry recorded, and
        // addressed to the realm being left rather than the one gained.
        let (losing, released) = router
            .bind_to(&b)
            .expect("moving the binding owes the realm being left its releases");
        assert_eq!(losing, a, "the debt is owed to the realm the human left");
        assert_eq!(
            released
                .iter()
                .map(|d| (d.origin(), d.kind().clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    Origin::Physical,
                    SeatDeliveryKind::Key {
                        keysym: HUMAN_KEY,
                        state: KeyState::Released
                    }
                ),
                (
                    Origin::Physical,
                    SeatDeliveryKind::Button {
                        button: HUMAN_BUTTON,
                        state: ButtonState::Released
                    }
                ),
            ],
            "a binding change must pay the losing app every press THE HUMAN is holding, keys              before buttons, with the tag the table recorded and never a minted one (B2) --              and must not speak for the agent, which still reaches that realm"
        );
        assert_eq!(
            router.held_keys(&a),
            [(AGENT_KEY, Origin::Emulated)],
            "the agent's press must survive the human's binding change, with its own tag"
        );
        assert!(
            router.held_buttons(&a).is_empty(),
            "the human's implicit grab in the losing realm is over"
        );

        // Nothing was delivered *to* the realm the human moved to.
        assert!(router.held_keys(&b).is_empty() && router.held_buttons(&b).is_empty());

        // The agent's own release still pairs in realm A, still goes out as
        // the agent's, and reaches A while B is bound -- which is the whole
        // point of the drain being one-sided.
        let delivery = router
            .route_emulated(
                &a,
                SeatInput::emulated(SeatInputKind::Key {
                    source: KeySource::Keysym,
                    keysym: AGENT_KEY,
                    state: KeyState::Released,
                }),
                view,
                surface,
            )
            .expect("an agent's release of its own held key must still reach its realm");
        assert_eq!(delivery.origin(), Origin::Emulated);
        assert!(router.held_keys(&a).is_empty());

        // Idempotent, and the human's debt is really gone.
        assert!(router.release_physical_keys(&a).is_empty());
        assert!(router.release_physical_buttons(&a).is_empty());
        assert!(
            router
                .route_physical(
                    phys(SeatInputKind::Key {
                        source: KeySource::Keysym,
                        keysym: HUMAN_KEY,
                        state: KeyState::Released
                    }),
                    view,
                    surface,
                )
                .is_none(),
            "a release arriving after the drain pairs with nothing in the realm now bound,              and must not reach an app that never saw the press"
        );
    }

    // ------------------------------------------------------------------
    // Physical presence (the chokepoint's `preempted` state, P1.4.4)
    // ------------------------------------------------------------------

    #[test]
    fn physical_presence_holds_while_buttons_are_down_and_for_the_window_after() {
        let t0 = std::time::Instant::now();
        let mut presence = PhysicalPresence::new();
        assert!(!presence.owns_target(t0), "idle by default");

        // Any physical activity opens the transient window...
        presence.note(Origin::Physical, &motion(1.0, 1.0), t0);
        assert!(presence.owns_target(t0));
        assert!(
            presence.owns_target(t0 + PHYSICAL_HOLD_WINDOW - std::time::Duration::from_millis(1))
        );
        // ...which closes (half-open, fail-closed toward the human's side
        // ending) exactly at the window bound.
        assert!(!presence.owns_target(t0 + PHYSICAL_HOLD_WINDOW));

        // A held physical button owns the target however long the hold
        // lasts -- far past the activity window, up to the stale-hold
        // ceiling (the dedicated test below).
        presence.note(Origin::Physical, &press(), t0);
        let much_later = t0 + PHYSICAL_HOLD_CEILING - std::time::Duration::from_millis(1);
        assert!(presence.owns_target(much_later));
        // Release at that instant: the hold ends, but the release is
        // itself physical activity, so the window re-arms from it.
        presence.note(Origin::Physical, &release(), much_later);
        assert!(presence.owns_target(much_later));
        assert!(!presence.owns_target(much_later + PHYSICAL_HOLD_WINDOW));

        // Per-button-code pairing, mirroring the router: releasing a
        // never-pressed code does not end another button's hold.
        presence.note(Origin::Physical, &press(), much_later);
        presence.note(
            Origin::Physical,
            &SeatInputKind::Button {
                button: 0x111,
                state: ButtonState::Released,
            },
            much_later,
        );
        assert!(presence.owns_target(much_later + PHYSICAL_HOLD_WINDOW * 2));
    }

    #[test]
    fn stale_hold_expires_at_the_ceiling_and_is_never_resurrected() {
        // The unpaired-press backstop (PHYSICAL_HOLD_CEILING docs): a
        // Pressed whose Released was lost -- device unplugged mid-hold,
        // seat teardown, a feeder that never synthesized the release --
        // must not refuse `preempted` for the life of the process.
        let t0 = std::time::Instant::now();
        let mut presence = PhysicalPresence::new();
        presence.note(Origin::Physical, &press(), t0);

        // Held and alive: owns far past the transient window...
        assert!(presence.owns_target(t0 + PHYSICAL_HOLD_WINDOW * 4));
        assert!(
            presence.owns_target(t0 + PHYSICAL_HOLD_CEILING - std::time::Duration::from_millis(1))
        );
        // ...but a full ceiling of device silence disowns it (half-open
        // bound, mirroring the window's).
        assert!(!presence.owns_target(t0 + PHYSICAL_HOLD_CEILING));

        // Fresh activity after the stale gap re-arms only the transient
        // window; the phantom hold was purged, so the agent is unblocked
        // one window later -- never wedged forever.
        let resumed = t0 + PHYSICAL_HOLD_CEILING + std::time::Duration::from_secs(1);
        presence.note(Origin::Physical, &motion(5.0, 5.0), resumed);
        assert!(presence.owns_target(resumed));
        assert!(!presence.owns_target(resumed + PHYSICAL_HOLD_WINDOW));

        // A real drag never lapses: each event refreshes the activity
        // clock while the button stays held, across any total span.
        let mut t = resumed;
        presence.note(Origin::Physical, &press(), t);
        for _ in 0..4 {
            t += PHYSICAL_HOLD_CEILING / 2;
            presence.note(Origin::Physical, &motion(6.0, 6.0), t);
        }
        assert!(
            presence.owns_target(t + PHYSICAL_HOLD_WINDOW * 2),
            "a live hold outlasts the ceiling as long as the device shows life"
        );
    }

    #[test]
    fn emulated_events_never_extend_the_preemption_window() {
        // An agent's own admitted actuations are origin-tagged emulated;
        // they must not let the agent preempt itself (or another agent).
        let t0 = std::time::Instant::now();
        let mut presence = PhysicalPresence::new();
        presence.note(Origin::Emulated, &motion(1.0, 1.0), t0);
        presence.note(Origin::Emulated, &press(), t0);
        assert!(!presence.owns_target(t0));
    }

    #[test]
    fn presence_hook_taps_the_router_at_the_b2_hook_point() {
        // The tracker attaches exactly like P1.7.3's watcher: an
        // observe-side tap that sees every event -- including ones a
        // consuming gate stops -- while delivery proceeds through the
        // inner hook untouched.
        let t0 = std::time::Instant::now();
        let presence = Rc::new(RefCell::new(PhysicalPresenceMap::new()));
        let clock = Rc::new(Cell::new(t0));
        let consume = Rc::new(Cell::new(false));
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut router = InputRouter::new(
            Rc::clone(&presence),
            Rc::clone(&clock),
            RecordingHook {
                log: Rc::clone(&log),
                consume: Rc::clone(&consume),
            },
        );
        // The handle the kernel gets is the very map this test holds — the
        // property `Runtime::new` relies on (`InputRouter::presence`).
        assert!(Rc::ptr_eq(&router.presence(), &presence));
        assert!(router.bind_to(&test_realm()).is_none());
        let bound = test_realm();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());

        // A delivered physical motion is observed and recorded at the
        // clock cell's injected instant.
        assert!(router
            .route_physical(phys(over(view, 2.0, 2.0)), view, surface)
            .is_some());
        assert!(presence.borrow().owns_target(Some(&bound), t0));
        assert!(!presence
            .borrow()
            .owns_target(Some(&bound), t0 + PHYSICAL_HOLD_WINDOW));

        // A consumed event still reaches the tap (observe precedes the
        // gate), at the advanced clock.
        let t1 = t0 + std::time::Duration::from_secs(10);
        clock.set(t1);
        consume.set(true);
        assert!(router
            .route_physical(phys(over(view, 3.0, 3.0)), view, surface)
            .is_none());
        assert!(presence.borrow().owns_target(Some(&bound), t1));

        // Emulated events pass the tap without arming it.
        let t2 = t1 + std::time::Duration::from_secs(10);
        clock.set(t2);
        consume.set(false);
        assert!(router
            .route_emulated(&test_realm(), SeatInput::emulated(scroll()), view, surface)
            .is_some());
        assert!(!presence.borrow().owns_target(Some(&bound), t2));

        // The inner hook saw everything, in observe-then-gate order.
        assert_eq!(
            log.borrow().as_slice(),
            &[
                ("observe", Origin::Physical),
                ("gate", Origin::Physical),
                ("observe", Origin::Physical),
                ("gate", Origin::Physical),
                ("observe", Origin::Emulated),
                ("gate", Origin::Emulated),
            ]
        );
    }

    /// **A realm the human's hand has left stops being "owned" — whether it
    /// was left by a bind change or by dying.**
    ///
    /// `PhysicalPresenceMap::forget`'s own doc says an entry is dropped "at
    /// exactly the moments the router drains that realm's held physical
    /// presses". `bind_to` was one of those moments and the realm death funnel
    /// was the other, and only the first was wired: a human mid-drag when a
    /// shim died left a held button recorded against a realm with no app,
    /// which `owns_target` honours for [`PHYSICAL_HOLD_CEILING`] — a full
    /// minute in which every agent actuating there is refused `preempted` with
    /// nobody touching anything.
    ///
    /// It is unobservable in today's chokepoint only because step 5a refuses a
    /// seat-delivered use over a dead realm `no_surface` before step 5c is
    /// reached. That is an accident of step order, not a property, so both
    /// moments now run through `InputRouter::forget_presence_of` and neither
    /// is a caller's obligation.
    ///
    /// A **held button** rather than bare motion in both halves, deliberately:
    /// motion alone expires after [`PHYSICAL_HOLD_WINDOW`] (500 ms) and would
    /// make the assertion pass on the clock instead of on the forget.
    #[test]
    fn a_realm_the_human_has_left_is_not_owned_by_the_human_whether_it_was_switched_or_died() {
        let t0 = std::time::Instant::now();
        let much_later = t0 + PHYSICAL_HOLD_WINDOW * 4;
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());
        let (a, b) = (
            crate::grants::RealmId::new("realm-a"),
            crate::grants::RealmId::new("realm-b"),
        );

        // (1) The realm dies under a held button.
        let presence = Rc::new(RefCell::new(PhysicalPresenceMap::new()));
        let mut router = InputRouter::new(Rc::clone(&presence), Rc::new(Cell::new(t0)), NoopHook);
        assert!(router.bind_to(&a).is_none());
        assert!(router
            .route_physical(phys(over(view, 2.0, 2.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
        assert!(
            presence.borrow().owns_target(Some(&a), much_later),
            "fixture check: a held button owns its realm well past the motion window, or \
             the assertion below would pass on the clock"
        );
        assert!(router.reset_for(&a));
        assert!(
            !presence.borrow().owns_target(Some(&a), much_later),
            "a dead realm's app can never pay the release down, so its presence must die \
             with it rather than refusing every agent there for the stale-hold ceiling"
        );

        // (2) The binding moves away under a held button.
        let presence = Rc::new(RefCell::new(PhysicalPresenceMap::new()));
        let mut router = InputRouter::new(Rc::clone(&presence), Rc::new(Cell::new(t0)), NoopHook);
        assert!(router.bind_to(&a).is_none());
        assert!(router
            .route_physical(phys(over(view, 2.0, 2.0)), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
        assert!(presence.borrow().owns_target(Some(&a), much_later));
        let (losing, owed) = router.bind_to(&b).expect("the binding moved");
        assert_eq!(losing, a);
        assert!(
            !owed.is_empty(),
            "fixture check: the losing realm is owed its release, which is the act the \
             forget travels with"
        );
        assert!(
            !presence.borrow().owns_target(Some(&a), much_later),
            "the human's next release is addressed to realm-b, so realm-a's held set can \
             never be paid down and must not keep it owned"
        );
    }

    // ==================================================================
    // WS-E.3.1 (issue #217, decision D-028): the core can interpret a
    // keymap, so a press and its release no longer resolve to the same
    // keysym — and pairing had to move off it.
    //
    // Two of the four criteria are here and two are in
    // `super::keymap::tests`, and the split is the point rather than an
    // accident: these two are properties the DEFAULT build must keep true,
    // so they run with `session-keymap` OFF, where there is no keymap at
    // all. The two that need one cannot.
    // ==================================================================

    /// #217 acceptance criterion (b): a key whose press and release resolve
    /// to **different** keysyms leaves the pairing table empty.
    ///
    /// This is the router half of `super::keymap::tests::
    /// a_real_keymap_can_resolve_one_keys_press_and_release_to_different_syms`,
    /// which demonstrates on a real `us+lv3:caps_switch_latch` keyboard that
    /// such a key exists. Here the two syms are supplied directly, because
    /// what is under test is the router with no keymap in the build.
    ///
    /// Three assertions, and the third is the one a keysym-paired router
    /// would fail even if it somehow passed the first two: the release the
    /// APP receives must carry the **press's** keysym. The shim binds a
    /// dynamic keycode per keysym, so a release carrying the other sym would
    /// release a keycode the app never held and leave the real one down.
    #[test]
    fn a_key_whose_press_and_release_resolve_to_different_syms_leaves_no_held_key() {
        const KEY_A: u32 = 30;
        const SHIFTED: u32 = 0x0041; // `A`, what the press resolved to
        const UNSHIFTED: u32 = 0x0061; // `a`, what the release resolved to

        let mut router = router();
        let view: ViewGeometry = (100u32, 80u32).into();
        let surface = Some((100u32, 80u32));

        let down = router
            .route_physical(
                phys(SeatInputKind::Key {
                    source: KeySource::Device(KEY_A),
                    keysym: SHIFTED,
                    state: KeyState::Pressed,
                }),
                view,
                surface,
            )
            .expect("a key press routes");
        assert_eq!(
            down.kind(),
            &SeatDeliveryKind::Key {
                keysym: SHIFTED,
                state: KeyState::Pressed
            }
        );
        assert_eq!(
            router.held_keys(&test_realm()),
            [(SHIFTED, Origin::Physical)]
        );

        let up = router
            .route_physical(
                phys(SeatInputKind::Key {
                    source: KeySource::Device(KEY_A),
                    keysym: UNSHIFTED,
                    state: KeyState::Released,
                }),
                view,
                surface,
            )
            .expect("the release pairs by scancode, not by keysym");
        assert_eq!(
            up.kind(),
            &SeatDeliveryKind::Key {
                keysym: SHIFTED,
                state: KeyState::Released
            },
            "the release must carry the keysym the PRESS delivered, or the shim releases a \
             keycode the app never held"
        );
        assert!(
            router.held_keys(&test_realm()).is_empty(),
            "nothing may be left held: a latched modifier rewrites every later keystroke"
        );
    }

    /// The other side of the same razor, so criterion (b) cannot be met by a
    /// router that simply stopped pairing at all: a release of a **different
    /// key** must still be dropped, and the first key must still be held.
    #[test]
    fn pairing_by_scancode_still_drops_a_release_whose_own_press_was_never_delivered() {
        let mut router = router();
        let view: ViewGeometry = (100u32, 80u32).into();
        let surface = Some((100u32, 80u32));

        assert!(router
            .route_physical(
                phys(SeatInputKind::Key {
                    source: KeySource::Device(30),
                    keysym: 0x0041,
                    state: KeyState::Pressed,
                }),
                view,
                surface,
            )
            .is_some());
        // Same keysym, different key. Under keysym pairing this would have
        // paid down the press above.
        assert!(
            router
                .route_physical(
                    phys(SeatInputKind::Key {
                        source: KeySource::Device(48),
                        keysym: 0x0041,
                        state: KeyState::Released,
                    }),
                    view,
                    surface,
                )
                .is_none(),
            "a release must pair with ITS OWN key's press, not with any press of that keysym"
        );
        assert_eq!(
            router.held_keys(&test_realm()),
            [(0x0041, Origin::Physical)]
        );
        // And an agent's keysym-identified release cannot pay down a
        // device's press either: the two identities do not overlap.
        assert!(router
            .route_emulated(
                &test_realm(),
                SeatInput::emulated(SeatInputKind::Key {
                    source: KeySource::Keysym,
                    keysym: 0x0041,
                    state: KeyState::Released,
                }),
                view,
                surface,
            )
            .is_none());
        assert_eq!(
            router.held_keys(&test_realm()),
            [(0x0041, Origin::Physical)]
        );
    }

    /// #217 acceptance criterion (d): `keysym_is_intakeable` still answers
    /// `true` for the **default dead-man chord**.
    ///
    /// The claim it makes changed even though the answer did not, and that
    /// is why this is a test rather than a comment. Before D-028 the
    /// intakeable set was the whole set of keysyms any backend could
    /// produce. Now a bare-metal backend can produce a keysym for every key
    /// on the board, so the invariant table is a *subset* — the check still
    /// holds (everything in the table is still deliverable) but it no longer
    /// bounds what is deliverable. What it must never stop doing is admitting
    /// the human's off-switch, on every backend, whatever the layout.
    #[test]
    fn the_default_dead_man_chord_is_still_intakeable_under_a_core_that_owns_a_keymap() {
        let chord = crate::deadman::Chord::parse(crate::deadman::DEFAULT_CHORD)
            .expect("the default chord is in the vocabulary");
        assert!(
            keysym_is_intakeable(chord.keysym()),
            "the human's off-switch must be deliverable on every backend"
        );
        // And it is layout-invariant for the reason that matters: it comes
        // out of the fixed scancode table, which no keymap moves.
        assert_eq!(invariant_keysym(1), Some(chord.keysym()));
        // The other two core-owned chord vocabularies get the same check
        // one module over, where their tables live:
        // `crate::chord::tests::every_chord_vocabulary_keysym_survives_a_core_that_owns_a_keymap`.
    }

    /// The keymap is reached by **file**, never by layout name — D-028(2),
    /// enforced against the source rather than against the API.
    ///
    /// `xkb::Keymap::new_from_names` searches `~/.config/xkb` before
    /// `/usr/share/X11/xkb` and honours `XKB_*`, and a realm's app runs as
    /// the core's uid with confinement still ahead of us — so a name-resolved
    /// keymap is an app-writable file the TCB parses. `crate::input::keymap`
    /// is built so it cannot happen (`NO_DEFAULT_INCLUDES |
    /// NO_ENVIRONMENT_NAMES`, and only a `from_string` constructor), and this
    /// is the guard that keeps a later edit from quietly reintroducing it.
    ///
    /// It runs in the **default** build on purpose: that is the
    /// configuration in which `crate::input::keymap` is not even compiled,
    /// so nothing else in CI would notice.
    #[test]
    fn no_source_file_resolves_a_keymap_by_name() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the crate's src/ is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("a readable source file");
                checked += 1;
                // Spelled split so this test is not its own violation, and
                // the name-constructor pattern carries its `(` so
                // `crate::input::keymap`'s docs can still say what they
                // refuse to call.
                for forbidden in [concat!("new_from_", "names("), concat!("XKB_", "DEFAULT_")] {
                    assert!(
                        !text.contains(forbidden),
                        "{} mentions `{forbidden}`: the core resolves its keymap from a file \
                         the operator names, never from a layout name (D-028(2))",
                        path.display()
                    );
                }
            }
        }
        assert!(
            checked > 20,
            "fixture check: the sweep found only {checked} source files, so it is not \
             actually looking at the crate"
        );
    }

    /// #217 acceptance criterion (a): a press taken with Shift held resolves
    /// to `A`, and its release — taken after Shift is up, so the keymap now
    /// says `a` — still pairs and is still delivered.
    ///
    /// End to end over a real `us` keymap and the real router, because the
    /// two halves are only interesting together: the keymap is what makes
    /// the two keysyms differ, and the router is what has to survive it.
    #[cfg(feature = "session-keymap")]
    #[test]
    fn a_shift_held_press_resolves_to_capital_a_and_its_release_still_pairs() {
        use super::keymap::CoreKeymap;
        const KEY_A: u32 = 30;
        const KEY_LEFTSHIFT: u32 = 42;

        let mut keymap = CoreKeymap::from_text(include_str!("../../tests/fixtures/keymap-us.xkb"))
            .expect("the us fixture compiles");
        let mut router = router();
        let view: ViewGeometry = (100u32, 80u32).into();
        let surface = Some((100u32, 80u32));

        let route = |keymap: &mut CoreKeymap, router: &mut InputRouter<NoopHook>, evdev, state| {
            let inputs = keymap_key(keymap, evdev, state);
            assert_eq!(inputs.len(), 1, "one key event in, one out");
            inputs
                .into_iter()
                .map(|i| router.route_physical(i, view, surface))
                .next()
                .unwrap()
        };

        assert!(route(&mut keymap, &mut router, KEY_LEFTSHIFT, KeyState::Pressed).is_some());
        let down = route(&mut keymap, &mut router, KEY_A, KeyState::Pressed)
            .expect("the shifted press is delivered");
        assert_eq!(
            down.kind(),
            &SeatDeliveryKind::Key {
                keysym: 0x0041,
                state: KeyState::Pressed
            },
            "Shift held must resolve KEY_A to `A`, which is the whole reason the core \
             grew a keymap"
        );

        // Shift up FIRST, so the keymap now resolves KEY_A to `a`.
        assert!(route(&mut keymap, &mut router, KEY_LEFTSHIFT, KeyState::Released).is_some());
        assert_eq!(
            keymap.resolve(KEY_A, KeyState::Released),
            Some(0x0061),
            "fixture check: without Shift the SAME key resolves to `a`, or this test is \
             not testing anything"
        );
        let up = route(&mut keymap, &mut router, KEY_A, KeyState::Released)
            .expect("the release must still pair — by scancode");
        assert_eq!(
            up.kind(),
            &SeatDeliveryKind::Key {
                keysym: 0x0041,
                state: KeyState::Released
            },
            "and must carry the press's keysym, so the app releases the key it is holding"
        );
        assert!(router.held_keys(&test_realm()).is_empty());
    }

    // ------------------------------------------------------------------
    // WS-E.4.2 (issue #222): the version-2 seat classes
    //
    // Three classes are served -- relative motion, multi-finger swipe and
    // pinch -- and every test below drives them through the SAME synthetic
    // `InputBackend` the version-1 classes use, because `intake_physical` is
    // the only place `Origin::Physical` can be minted and a test that
    // constructed a `SeatInput` directly would be proving something about a
    // path production does not have.
    //
    // Touch and tablet are NOT YET SERVED, which is a different claim from
    // refused: the machine these were measured on has neither device, and a
    // measurement of one laptop is not a property of a class. What would
    // reopen each is recorded in the decision log, not here.
    // ------------------------------------------------------------------

    /// Decode a delivery the way the shim will: through the generated
    /// decoder, off the very bytes `encode` produces. Asserting on
    /// `SeatDeliveryKind` alone would prove the router agrees with itself;
    /// this proves the wire carries it, and it is the only way to check the
    /// `origin` argument the IDL pins as every seat event's last one.
    const SEAT_ID: u32 = 42;

    /// Every served version-2 class reaches the wire, through the hook, with
    /// its origin intact and its payload unmangled.
    ///
    /// Four properties in one pass, because they are one path:
    ///
    /// 1. **It passes the hook.** The `RecordingHook` log must show
    ///    `observe` then `gate` for every event -- so a new class cannot
    ///    route around the dead-man tap or the consent grab.
    /// 2. **The origin survives.** Decoded off the frame bytes, not read
    ///    back off the struct the encoder was handed.
    /// 3. **The arguments survive, each in its own field.** The pinch's four
    ///    quantities are all distinct and none is derivable from another, so
    ///    a translation that swapped `scale` and `rotation`, or copied `dx`
    ///    into `dx_unaccel`, fails here.
    /// 4. **`gesture_kind` is right.** A swipe reported as a pinch would be
    ///    replayed on the wrong wlroots gesture object.
    #[test]
    fn every_version_2_class_reaches_the_wire_through_the_hook_with_origin_intact() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut router = InputRouter::detached(RecordingHook {
            log: Rc::clone(&log),
            consume: Rc::new(Cell::new(false)),
        });
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        // A pointer-position-less class hit-tests against the stored
        // pointer, so put the pointer on the app first -- the same
        // precondition `scroll` has had since v0.
        assert!(route_host(&mut router, &motion_ev(10.0, 10.0), view, surface).is_some());
        log.borrow_mut().clear();

        // -- relative_motion -------------------------------------------
        let d = route_host(
            &mut router,
            &relative_ev(3.5, -2.25, 7.0, -4.5),
            view,
            surface,
        )
        .expect("relative motion is deliverable while the pointer is on the app");
        let (obj, ev) =
            seat::events::RelativeMotion::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!(obj, SEAT_ID);
        assert_eq!(ev.dx, Fixed::from_f64(3.5));
        assert_eq!(ev.dy, Fixed::from_f64(-2.25));
        assert_eq!(
            ev.dx_unaccel,
            Fixed::from_f64(7.0),
            "the unaccelerated delta must be the device's own, never a copy of the \
             accelerated one -- it is most of the reason this event exists"
        );
        assert_eq!(ev.dy_unaccel, Fixed::from_f64(-4.5));
        assert_eq!(ev.origin, Origin::Physical);

        // -- a whole swipe ---------------------------------------------
        let d = route_host(&mut router, &swipe_begin_ev(3), view, surface).expect("swipe begins");
        let (_, ev) =
            seat::events::GestureBegin::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!(ev.kind, GestureKind::Swipe);
        assert_eq!(ev.fingers, 3);
        assert_eq!(ev.origin, Origin::Physical);

        let d = route_host(&mut router, &swipe_update_ev(12.0, -3.0), view, surface)
            .expect("an update after a delivered begin is delivered");
        let (_, ev) =
            seat::events::GestureSwipeUpdate::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!(
            (ev.dx, ev.dy),
            (Fixed::from_f64(12.0), Fixed::from_f64(-3.0))
        );
        assert_eq!(ev.origin, Origin::Physical);

        let d = route_host(&mut router, &swipe_end_ev(false), view, surface).expect("swipe ends");
        let (_, ev) = seat::events::GestureEnd::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!(ev.kind, GestureKind::Swipe);
        assert_eq!(
            ev.state,
            GestureState::Completed,
            "libinput's `cancelled == false` is the wire's `completed`; inverting this \
             makes an app undo a gesture the human finished"
        );
        assert_eq!(ev.origin, Origin::Physical);

        // -- a whole pinch ---------------------------------------------
        let d = route_host(&mut router, &pinch_begin_ev(2), view, surface).expect("pinch begins");
        let (_, ev) =
            seat::events::GestureBegin::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!(ev.kind, GestureKind::Pinch);
        assert_eq!(ev.fingers, 2);

        let d = route_host(
            &mut router,
            &pinch_update_ev(1.0, 2.0, 1.75, -30.0),
            view,
            surface,
        )
        .expect("pinch update");
        let (_, ev) =
            seat::events::GesturePinchUpdate::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!((ev.dx, ev.dy), (Fixed::from_f64(1.0), Fixed::from_f64(2.0)));
        assert_eq!(
            ev.scale,
            Fixed::from_f64(1.75),
            "scale is absolute since the begin; carrying it in the rotation field, or \
             differencing it, is the mistake apps cannot detect"
        );
        assert_eq!(ev.rotation, Fixed::from_f64(-30.0));
        assert_eq!(ev.origin, Origin::Physical);

        let d = route_host(&mut router, &pinch_end_ev(true), view, surface).expect("pinch ends");
        let (_, ev) = seat::events::GestureEnd::decode(&d.encode(SEAT_ID), None).expect("decodes");
        assert_eq!(ev.kind, GestureKind::Pinch);
        assert_eq!(ev.state, GestureState::Cancelled);

        // 1: the hook saw every one of the seven, observe before gate, and
        // nothing routed around it.
        let seen = log.borrow();
        assert_eq!(
            seen.len(),
            14,
            "seven events, each observed then gated: {seen:?}"
        );
        for pair in seen.chunks(2) {
            assert_eq!(pair[0], ("observe", Origin::Physical));
            assert_eq!(pair[1], ("gate", Origin::Physical));
        }
    }

    /// Each of the five new deliveries carries a distinct recorder label, and
    /// only the two discrete ones are journaled.
    ///
    /// The label is the whole of what an audit entry says about a seat event,
    /// so two kinds sharing one would make the journal lie by omission. The
    /// exclusions are the flip side of the same judgement: `relative_motion`
    /// arrives *beside* the `motion` already excluded for flooding the
    /// recorder, so journaling it would double the rate the existing argument
    /// rejects -- while `gesture_begin`/`gesture_end` are bounded by the
    /// human's fingers and are the pair whose balance an audit can check.
    #[test]
    fn the_version_2_deliveries_have_their_own_labels_and_only_the_discrete_ones_are_journaled() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rec, path) = crate::recorder::tests::scratch_recorder("seat-v2-labels");
        let realm = crate::grants::RealmId::new("realm-9");

        let cases = [
            (
                SeatDeliveryKind::RelativeMotion {
                    dx: Fixed::from_f64(1.0),
                    dy: Fixed::from_f64(2.0),
                    dx_unaccel: Fixed::from_f64(3.0),
                    dy_unaccel: Fixed::from_f64(4.0),
                },
                "relative_motion",
            ),
            (
                SeatDeliveryKind::GestureBegin {
                    kind: GestureKind::Swipe,
                    fingers: 3,
                },
                "gesture_begin",
            ),
            (
                SeatDeliveryKind::GestureSwipeUpdate {
                    dx: Fixed::from_f64(1.0),
                    dy: Fixed::from_f64(1.0),
                },
                "gesture_swipe_update",
            ),
            (
                SeatDeliveryKind::GesturePinchUpdate {
                    dx: Fixed::from_f64(1.0),
                    dy: Fixed::from_f64(1.0),
                    scale: Fixed::from_f64(2.0),
                    rotation: Fixed::from_f64(15.0),
                },
                "gesture_pinch_update",
            ),
            (
                SeatDeliveryKind::GestureEnd {
                    kind: GestureKind::Swipe,
                    state: GestureState::Cancelled,
                },
                "gesture_end",
            ),
        ];
        let mut labels = Vec::new();
        for (kind, label) in cases {
            let delivery = SeatDelivery {
                origin: Origin::Physical,
                kind,
            };
            assert_eq!(delivery.event_label(), label);
            labels.push(label);
            super::record_seat_delivery(&mut rec, &realm, &delivery);
        }
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "no two kinds may share a label");

        let entries = crate::recorder::tests::read_log(&path);
        let delivered = crate::recorder::tests::of_kind(&entries, "seat_delivered");
        let journaled: Vec<&str> = delivered.iter().map(|e| e.str("event")).collect();
        assert_eq!(
            journaled,
            ["gesture_begin", "gesture_end"],
            "the three raw-device-rate classes must not reach the log at all"
        );
        for entry in &delivered {
            assert_eq!(entry.str("origin"), "physical");
            assert_eq!(entry.str("realm"), "realm-9");
        }
        crate::recorder::tests::cleanup(&path);
    }

    /// **The latch test, and the one that matters.** A realm switch mid-swipe
    /// ends the gesture the losing app was told about -- `cancelled`, exactly
    /// once, and never twice.
    ///
    /// A `gesture_begin` with no `gesture_end` is the latched-modifier failure
    /// in a new shape: the app accumulates a pinch or a swipe forever, and no
    /// later event can pay it down because the human's fingers are now
    /// producing events addressed to a different realm. Delete
    /// `end_physical_gesture` from `bind_to` and this test fails on its first
    /// assertion.
    ///
    /// `cancelled` rather than `completed` is the load-bearing half: the human
    /// did not finish the gesture, so an app previewing a zoom must put it
    /// back rather than commit one nobody confirmed.
    #[test]
    fn a_realm_switch_mid_gesture_ends_it_cancelled_and_owes_the_app_nothing_after() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        assert!(route_host(&mut router, &motion_ev(10.0, 10.0), view, surface).is_some());
        assert!(route_host(&mut router, &swipe_begin_ev(4), view, surface).is_some());
        assert!(route_host(&mut router, &swipe_update_ev(5.0, 0.0), view, surface).is_some());

        let other = crate::grants::RealmId::new("realm-1");
        let (losing, owed) = router
            .bind_to(&other)
            .expect("the binding moved, so the losing realm is owed its debt");
        assert_eq!(losing, test_realm());
        assert_eq!(
            owed.len(),
            1,
            "one gesture in flight, so exactly one end is owed: {owed:?}"
        );
        assert_eq!(owed[0].origin(), Origin::Physical);
        assert_eq!(
            owed[0].kind(),
            &SeatDeliveryKind::GestureEnd {
                kind: GestureKind::Swipe,
                state: GestureState::Cancelled,
            },
            "the app must be told the gesture it was told about ended, and told the \
             truth about how"
        );

        // Draining twice must not owe twice: an app told a swipe ended and
        // then told it ended again would pair the second with nothing.
        assert!(router.end_physical_gesture(&test_realm()).is_empty());
        // And the gesture's own events, arriving after the switch, are now
        // unpaired for the realm they were addressed to -- so the human's
        // fingers finishing the swipe cannot reach the app it left.
        assert!(router.bind_to(&test_realm()).is_some());
        assert!(route_host(&mut router, &swipe_update_ev(5.0, 0.0), view, surface).is_none());
        assert!(route_host(&mut router, &swipe_end_ev(false), view, surface).is_none());
    }

    /// A seat pause (a VT switch) owes the same debt, through the same drain.
    ///
    /// Its own call site is `session::suspend_physical_seat`; what is asserted
    /// here is the router half it depends on, so the two cannot drift.
    #[test]
    fn the_gesture_drain_reads_the_kind_back_off_the_record_it_delivered() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some(view.usable());
        let inset = f64::from(view.reserved_top());
        assert!(route_host(&mut router, &motion_ev(1.0, 1.0 + inset), view, surface).is_some());
        assert!(route_host(&mut router, &pinch_begin_ev(2), view, surface).is_some());

        let owed = router.end_physical_gesture(&test_realm());
        assert_eq!(
            owed.len(),
            1,
            "the pinch the app was told about is owed an end"
        );
        assert_eq!(
            owed[0].kind(),
            &SeatDeliveryKind::GestureEnd {
                kind: GestureKind::Pinch,
                state: GestureState::Cancelled,
            },
            "the kind comes off the router's own record of what it delivered, so a \
             pinch is never ended as a swipe"
        );
        assert!(
            router.end_physical_gesture(&test_realm()).is_empty(),
            "the record is consumed by the drain"
        );
    }

    /// **A consumed begin starts nothing**, so its updates and its end can
    /// never leak -- the gesture twin of "a consumed press starts no implicit
    /// grab".
    ///
    /// This is the second half of the latch protection, and it is the half
    /// that would fail *open*: if a gate consumed a begin while the router
    /// still recorded it, the app would receive updates and an end for a
    /// gesture it was never told had started.
    #[test]
    fn a_gate_consumed_gesture_begin_leaks_no_update_and_no_end() {
        let consume = Rc::new(Cell::new(false));
        let mut router = InputRouter::detached(RecordingHook {
            log: Rc::new(RefCell::new(Vec::new())),
            consume: Rc::clone(&consume),
        });
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        assert!(route_host(&mut router, &motion_ev(5.0, 5.0), view, surface).is_some());

        consume.set(true);
        assert!(
            route_host(&mut router, &pinch_begin_ev(2), view, surface).is_none(),
            "a consumed begin is not delivered"
        );
        consume.set(false);
        assert!(
            route_host(
                &mut router,
                &pinch_update_ev(1.0, 1.0, 2.0, 0.0),
                view,
                surface
            )
            .is_none(),
            "an update whose begin was consumed must not reach an app that was never \
             told a pinch started"
        );
        assert!(
            route_host(&mut router, &pinch_end_ev(false), view, surface).is_none(),
            "and neither must its end"
        );
        assert!(
            router.end_physical_gesture(&test_realm()).is_empty(),
            "nothing was ever in flight, so the drain owes nothing either"
        );
    }

    /// A gate that consumes a `gesture_end` clears the router's record, so the
    /// router cannot wedge -- the next gesture still works.
    ///
    /// The app-side debt this leaves is real and is the gate implementor's,
    /// exactly as it is for a consumed key release (`Gate::Consume`). What is
    /// pinned here is that the *router* recovers: keeping the record instead
    /// would make "at most one gesture in flight" drop every subsequent begin
    /// forever, which is a wedge rather than a debt.
    #[test]
    fn a_consumed_gesture_end_clears_the_record_so_the_next_gesture_still_begins() {
        let consume = Rc::new(Cell::new(false));
        let mut router = InputRouter::detached(RecordingHook {
            log: Rc::new(RefCell::new(Vec::new())),
            consume: Rc::clone(&consume),
        });
        assert!(router.bind_to(&test_realm()).is_none());
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        assert!(route_host(&mut router, &motion_ev(5.0, 5.0), view, surface).is_some());
        assert!(route_host(&mut router, &swipe_begin_ev(3), view, surface).is_some());

        consume.set(true);
        assert!(route_host(&mut router, &swipe_end_ev(false), view, surface).is_none());
        consume.set(false);

        assert!(
            router.end_physical_gesture(&test_realm()).is_empty(),
            "the consumed end already reconciled the record; a drain must not now \
             invent a second end for a gesture the router has forgotten"
        );
        assert!(
            route_host(&mut router, &pinch_begin_ev(2), view, surface).is_some(),
            "and the next gesture must still begin -- a consuming gate may leave a \
             debt, but it may never wedge the router"
        );
    }

    /// The IDL's "at most one gesture in flight per seat", enforced rather
    /// than assumed, plus the three shapes of unpaired event.
    #[test]
    fn unpaired_gesture_events_are_dropped_and_a_second_begin_is_refused() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        assert!(route_host(&mut router, &motion_ev(5.0, 5.0), view, surface).is_some());

        // Nothing in flight: an update and an end have nothing to belong to.
        assert!(route_host(&mut router, &swipe_update_ev(1.0, 1.0), view, surface).is_none());
        assert!(route_host(
            &mut router,
            &pinch_update_ev(1.0, 1.0, 1.0, 0.0),
            view,
            surface
        )
        .is_none());
        assert!(route_host(&mut router, &swipe_end_ev(false), view, surface).is_none());

        assert!(route_host(&mut router, &swipe_begin_ev(3), view, surface).is_some());
        assert!(
            route_host(&mut router, &pinch_begin_ev(2), view, surface).is_none(),
            "a second begin while one is in flight would owe the app two ends it can \
             only pair one of"
        );
        assert!(
            route_host(
                &mut router,
                &pinch_update_ev(1.0, 1.0, 2.0, 0.0),
                view,
                surface
            )
            .is_none(),
            "a pinch update inside a swipe belongs to no gesture the app is tracking"
        );
        assert!(
            route_host(&mut router, &swipe_update_ev(1.0, 1.0), view, surface).is_some(),
            "the swipe that IS in flight keeps moving"
        );

        // An end naming the wrong kind still ends the one in flight: leaving
        // it live would be the latch this whole section exists to prevent.
        let d = route_host(&mut router, &pinch_end_ev(false), view, surface)
            .expect("the gesture in flight is ended regardless of what the end names");
        assert_eq!(
            d.kind(),
            &SeatDeliveryKind::GestureEnd {
                kind: GestureKind::Swipe,
                state: GestureState::Completed,
            }
        );
        assert!(router.end_physical_gesture(&test_realm()).is_empty());
    }

    /// The matte is not the app, for the version-2 classes too.
    ///
    /// A delta and a gesture carry no position of their own, so they
    /// hit-test against the stored pointer exactly as `scroll` does. Without
    /// this an app whose surface does not fill the view would receive the
    /// movement of a pointer that is demonstrably not over it.
    #[test]
    fn relative_motion_and_gestures_are_dropped_when_the_pointer_is_on_the_matte() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((32, 24)); // centred, so (2, 2) is matte
        assert!(
            route_host(&mut router, &motion_ev(2.0, 2.0), view, surface).is_none(),
            "fixture check: (2, 2) must be matte, or this test asserts nothing"
        );
        assert!(route_host(&mut router, &relative_ev(1.0, 1.0, 1.0, 1.0), view, surface).is_none());
        assert!(route_host(&mut router, &swipe_begin_ev(3), view, surface).is_none());
        assert!(
            router.end_physical_gesture(&test_realm()).is_empty(),
            "a begin that was never delivered leaves nothing to drain"
        );
    }

    /// A realm's gesture is its own: a sibling realm's drain must not end it,
    /// and a sibling's death must not either.
    #[test]
    fn one_realms_gesture_is_untouched_by_another_realms_drain_or_death() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        let other = crate::grants::RealmId::new("realm-1");
        assert!(route_host(&mut router, &motion_ev(5.0, 5.0), view, surface).is_some());
        assert!(route_host(&mut router, &swipe_begin_ev(3), view, surface).is_some());

        assert!(
            router.end_physical_gesture(&other).is_empty(),
            "a realm with no seat at all owes nothing"
        );
        router.reset_for(&other);
        assert_eq!(
            router.end_physical_gesture(&test_realm()).len(),
            1,
            "the sibling's death must leave this realm's in-flight gesture exactly \
             where it was"
        );
    }

    /// An agent's actuation cannot mint a gesture today, and if one ever can,
    /// the drain must leave it alone -- the same filter the key and button
    /// drains apply, for the same reason.
    ///
    /// Written against the router directly because `intake_physical` cannot
    /// produce an emulated event by construction (B2), which is the property
    /// this test relies on rather than works around.
    #[test]
    fn the_gesture_drain_leaves_an_emulated_gesture_alone() {
        let mut router = router();
        let view: ViewGeometry = (64, 48).into();
        let surface = Some((64, 48));
        assert!(route_host(&mut router, &motion_ev(5.0, 5.0), view, surface).is_some());
        assert!(router
            .route_emulated(
                &test_realm(),
                SeatInput::emulated(SeatInputKind::GestureBegin {
                    kind: GestureKind::Pinch,
                    fingers: 2,
                }),
                view,
                surface,
            )
            .is_some());
        assert!(
            router.end_physical_gesture(&test_realm()).is_empty(),
            "draining it would invent an end the principal never sent and, since the \
             delivery carries the tag, attribute it to the human"
        );
    }

    // ---- pointer constraints (WS-E.4.2, issue #222) ------------------------

    /// Put `realm`'s constraint into force, as the reconciler would.
    ///
    /// A test helper rather than a production entry point, deliberately: only
    /// `PointerConstraintTable::reconcile` may set the state in a shipping
    /// build, and it derives it. This drives the same function with gates that
    /// say "focused, nothing in the way, pointer on the surface".
    fn arm_constraint<H: PreemptionHook>(
        router: &InputRouter<H>,
        realm: &crate::grants::RealmId,
        view: ViewGeometry,
        surface: Option<(u32, u32)>,
    ) {
        let table = router.constraints();
        let mut table = table.borrow_mut();
        let _ = table.ask(
            realm,
            PointerConstraintAsk {
                serial: 1,
                surface: Some(7),
                kind: vitrin_protocol::generated::vitrin_shim_session::PointerConstraintKind::Lock,
                lifetime:
                    vitrin_protocol::generated::vitrin_shim_session::PointerConstraintLifetime::Persistent,
                region: ConstraintRegion {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
            },
        );
        let owed = table.reconcile(&PresentationGates {
            focused: Some(realm),
            output: OutputGates::default(),
            surface,
            view,
            pointer: Some((
                f64::from(view.output().0) / 2.0,
                f64::from(view.output().1) / 2.0,
            )),
        });
        assert!(
            owed.iter().any(|v| v.realm == *realm),
            "the fixture must actually reconcile the realm it names"
        );
        assert!(
            table.is_active(realm),
            "the fixture must actually put the constraint into force"
        );
    }

    /// **An active lock stops absolute motion and keeps the delta** — which is
    /// what a lock IS on this wire (WS-E.4.2, issue #222).
    ///
    /// The IDL promises that "a lock stops absolute motion and freezes the
    /// position the core's own hit tests use", and this is the router's half of
    /// it. The bare-metal backend additionally declines to move its own
    /// accumulated position, so the two together mean the app is told nothing
    /// about where the pointer is and everything about how far it moved.
    #[test]
    fn an_active_pointer_constraint_stops_absolute_motion_and_keeps_relative_motion() {
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        // Establish the pointer over the surface first: without a position the
        // relative arm would be dropped by the matte rule for its own reasons,
        // and the test would pass for the wrong one.
        assert!(router
            .route_physical(phys(motion(50.0, 40.0)), view, surface)
            .is_some());
        arm_constraint(&router, &test_realm(), view, surface);

        assert!(
            router
                .route_physical(phys(motion(51.0, 41.0)), view, surface)
                .is_none(),
            "absolute motion must not reach a locked app"
        );
        let delta = router.route_physical(
            phys(SeatInputKind::RelativeMotion {
                dx: 1.0,
                dy: 1.0,
                dx_unaccel: 1.0,
                dy_unaccel: 1.0,
            }),
            view,
            surface,
        );
        assert!(
            delta.is_some(),
            "the delta is the whole point of a lock and must keep flowing"
        );
        // ...and the other pointer classes are untouched: a lock is about
        // position, not about the app going deaf.
        assert!(router
            .route_physical(phys(scroll()), view, surface)
            .is_some());
        assert!(router
            .route_physical(phys(press()), view, surface)
            .is_some());
    }

    /// **A locked app cannot confine an agent's actuation** (D-032).
    ///
    /// The sharpest of the freeze tests. `route_physical` and `route_emulated`
    /// are two named entry points precisely so an agent's actuation follows its
    /// grant rather than the human's attention, and an app that acquires a way
    /// to trap a principal's pointer through a lock would have turned a
    /// confinement primitive into a capability over somebody else.
    #[test]
    fn a_locked_app_cannot_confine_an_agents_actuation() {
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        assert!(router
            .route_physical(phys(motion(50.0, 40.0)), view, surface)
            .is_some());
        arm_constraint(&router, &test_realm(), view, surface);

        let delivered = router.route_emulated(
            &test_realm(),
            SeatInput::emulated(motion(52.0, 42.0)),
            view,
            surface,
        );
        let delivered = delivered.expect("an agent's absolute motion is not the app's to stop");
        assert_motion(&delivered, 22.0, 8.0);
        assert_eq!(delivered.origin(), Origin::Emulated);
    }

    /// **A realm's death forgets its constraint, and only its own** — path 4 of
    /// the deactivation list.
    ///
    /// Modelled on [`a_siblings_death_leaves_the_bound_realms_held_key_alone`],
    /// because the failure that matters here is scope rather than existence:
    /// the clear lives *inside* `reset_for`, which is what makes it reach both
    /// arms of the death funnel — the one that goes through
    /// `ShimServer::connection_closed` and the one for a realm that died before
    /// its shim came up.
    #[test]
    fn a_realms_death_forgets_its_pointer_constraint_and_leaves_a_siblings_alone() {
        let mut router = InputRouter::detached(NoopHook);
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        let bound = test_realm();
        let sibling = crate::grants::RealmId::new("browser");
        assert!(router.bind_to(&bound).is_none());
        assert!(router
            .route_physical(phys(motion(50.0, 40.0)), view, surface)
            .is_some());
        // The sibling's is armed FIRST, so arming the bound realm's leaves it
        // recorded-but-inactive -- which is exactly the state a hidden realm's
        // constraint is in, and the one whose removal must not disturb the
        // realm on screen.
        arm_constraint(&router, &sibling, view, surface);
        arm_constraint(&router, &bound, view, surface);
        assert!(router.constraints().borrow().get(&sibling).is_some());

        assert!(router.reset_for(&sibling));
        assert!(
            router.constraints().borrow().get(&sibling).is_none(),
            "the dying realm's record goes, silently: there is no shim left to tell"
        );
        assert!(
            router.constraints().borrow().is_active(&bound),
            "a sibling's death must not forget the bound realm's lock"
        );
        assert!(router.reset_for(&bound));
        assert!(!router.constraints().borrow().is_active(&bound));
        assert!(
            router
                .route_physical(phys(motion(51.0, 41.0)), view, surface)
                .is_none(),
            "and with the record gone the realm has no seat either, so nothing is delivered \
             to a shim generation that ended"
        );
    }

    /// **The table the router writes is the only one obtainable**, which is
    /// what makes "the composite reads the router's records" structural rather
    /// than a wiring step a backend can get wrong.
    #[test]
    fn the_constraint_table_handle_is_the_routers_own() {
        let router = router();
        let a = router.constraints();
        let b = router.constraints();
        assert!(
            std::rc::Rc::ptr_eq(&a, &b),
            "every handle must be a clone of the one table"
        );
        let realm = test_realm();
        let _ = a.borrow_mut().ask(
            &realm,
            PointerConstraintAsk {
                serial: 1,
                surface: Some(7),
                kind: vitrin_protocol::generated::vitrin_shim_session::PointerConstraintKind::Lock,
                lifetime:
                    vitrin_protocol::generated::vitrin_shim_session::PointerConstraintLifetime::Persistent,
                region: ConstraintRegion {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
            },
        );
        assert!(
            b.borrow().get(&realm).is_some(),
            "a second handle must see the first handle's write"
        );
    }

    /// **A locked pointer does not stop the dead-man chord, and does not stop
    /// the human typing.**
    ///
    /// The single property this whole change must not cost. Structurally it
    /// cannot: the preemption hook's observe tap runs strictly above the
    /// per-kind match the freeze lives in — `hook.observe` precedes
    /// `let realm = realm?`, which precedes every arm — and the freeze only ever
    /// touches absolute `Motion`. This asserts it against a **real**
    /// `DeadManHook` stacked on a real router with a lock genuinely in force,
    /// because "the off-switch still works" is not a property to leave to a
    /// comment.
    #[test]
    fn a_locked_pointer_does_not_stop_the_dead_man_chord() {
        let deadman = std::rc::Rc::new(std::cell::RefCell::new(
            crate::deadman::DeadManSwitch::new(crate::deadman::DeadManConfig::default()),
        ));
        let now = std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now()));
        let mut router = InputRouter::detached(crate::deadman::DeadManHook::new(
            std::rc::Rc::clone(&deadman),
            std::rc::Rc::clone(&now),
            NoopHook,
        ));
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        assert!(router.bind_to(&test_realm()).is_none());
        assert!(router
            .route_physical(phys(motion(50.0, 40.0)), view, surface)
            .is_some());
        arm_constraint(&router, &test_realm(), view, surface);

        // The control: the freeze really is on, so the assertions below are
        // about a session that is genuinely locked.
        assert!(router
            .route_physical(phys(motion(51.0, 41.0)), view, surface)
            .is_none());

        router.route_physical(chord_press(), view, surface);
        assert!(
            deadman.borrow().deadline().is_some(),
            "the human's off-switch must arm while an app holds a pointer lock"
        );
        // ...and an ordinary key still reaches the app: a lock is about
        // position, not about the human going mute.
        assert!(router
            .route_physical(
                phys(SeatInputKind::Key {
                    source: KeySource::Keysym,
                    keysym: 0xffe1,
                    state: KeyState::Pressed,
                }),
                view,
                surface,
            )
            .is_some());
    }

    /// **A frozen pointer really is frozen** — the position the core's own hit
    /// tests use does not move either (WS-E.4.2, issue #222).
    ///
    /// Found by a test, not by reading: the first version of the freeze lived
    /// only in the `Motion` delivery arm, so `RealmSeat::pointer` and
    /// `RealmSeat::human_pointer` kept tracking the device. That is not a
    /// cosmetic gap — the constraint's own region test is judged against
    /// `human_pointer`, so the pointer would have drifted off the surface and
    /// **ended the lock** using exactly the motion the lock exists to stop,
    /// while the app was told nothing at all.
    #[test]
    fn an_active_constraint_freezes_the_position_the_core_hit_tests_with() {
        let mut router = router();
        let view: ViewGeometry = (100, 80).into();
        let surface = Some((40, 20));
        assert!(router
            .route_physical(phys(motion(50.0, 40.0)), view, surface)
            .is_some());
        arm_constraint(&router, &test_realm(), view, surface);

        assert!(router
            .route_physical(phys(motion(4.0, 4.0)), view, surface)
            .is_none());
        assert_eq!(
            router.human_pointer(&test_realm()),
            Some((50.0, 40.0)),
            "the human's recorded position must not move under a lock"
        );

        // ...but an AGENT's motion still moves the shared position, because an
        // app's lock is not authority over a principal.
        assert!(router
            .route_emulated(
                &test_realm(),
                SeatInput::emulated(motion(52.0, 42.0)),
                view,
                surface,
            )
            .is_some());
        assert_eq!(router.agent_pointer(&test_realm()), Some((52.0, 42.0)));
        assert_eq!(
            router.human_pointer(&test_realm()),
            Some((50.0, 40.0)),
            "and it does not become the human's position by doing so"
        );
    }
}
