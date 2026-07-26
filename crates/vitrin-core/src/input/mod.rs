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
//! device at all**, and that absence is compiler-enforced, not a runtime
//! check: [`SeatInput::physical`] is private to this module, so the only
//! producer of `Origin::Physical` events in the crate is
//! [`intake_physical`], whose only caller is the nested backend's winit
//! event handler — a call site anywhere else (the headless backend, a
//! P1.4.x actuation path, a replay helper) is a compile error, so no
//! phantom physical-input path can exist there.
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
//!   [`intake_physical`] can mint the physical tag) and
//!   [`SeatInput::emulated`] (crate-visible for the P1.4.x actuation
//!   intake) — an untagged event is unrepresentable, and a
//!   physical-origin masquerade outside nested intake is a compile
//!   error.
//!   `SeatInput` is not the only producer of a wire-ready event, so the
//!   compile-time half of the guarantee needs the next bullet to be
//!   complete.
//! - A [`SeatDelivery`] is constructed at exactly two sites, both inside
//!   `InputRouter`, and **neither invents an origin**. `InputRouter::route`
//!   *moves* the tag out of the `SeatInput` it consumed.
//!   `InputRouter::release_physical_keys` — the focus-loss drain, the one
//!   place a wire event exists with no intake event behind it — *copies* the
//!   tag off the pairing-table entry that key's own press recorded
//!   (`InputRouter::pressed_keys` stores `(keysym, origin)` for exactly this
//!   reason) and drains only the entries a human's press put there. The tag
//!   is therefore never re-derived downstream and never minted, so it cannot
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
//! [`layout::place`] placement the compositor painted with, so that (0, 0)
//! on the wire is always the top-left of the app content the shim
//! forwarded — the origin of the shim's own view space. A click on the
//! letterbox matte is *not* a click in the app: pointer events outside the
//! placed surface rectangle are dropped, except during an implicit grab
//! (below). With no committed surface there is nothing to point at, and
//! the pointer path simply yields nothing (keys and text still flow — the
//! shim holds keyboard focus on its app and owns that judgement).
//!
//! Implicit grab: while any button the router delivered is still pressed,
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
//! **Keys pair the same way, per keysym.** Keys carry no geometry, so they
//! never route on position — but the razor above is not about geometry, it
//! is about the app's press/release accounting, and a keyboard has exactly
//! the same accounting. Without pairing, any policy that stops a key
//! *release* (P1.7.2's consent grab is the first, and [`Gate::Consume`]
//! promises to reconcile what it stops) would leave the app believing a
//! key is still held: a latched `Ctrl`/`Alt`/`Super` silently rewrites the
//! meaning of every keystroke the app receives afterwards, and a latched
//! letter autorepeats. So the router tracks the keysyms whose press it
//! delivered and drops any release that does not pair with one, exactly as
//! it does for button codes. Pairing by keysym rather than by scancode is
//! sound here because [`invariant_keysym`] is a fixed scancode→keysym
//! table with no modifier resolution — the same physical key yields the
//! same keysym on press and on release, by construction. If the core ever
//! grows a real keymap, pairing must move to the scancode.
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
//! `InputRouter<ConsentGate<DeadManHook<NoopHook>>>` — the stacking this
//! hook point was designed for, reached without restructuring anything.
//!
//! [`crate::consent::grab::ConsentGate`] (P1.7.2) is the gate. Its policy
//! lives entirely in that module; what matters here is that it honours the
//! pairing contract below by never consuming a release, and that it judges by
//! the origin tag intake bound rather than by any authority question — the
//! router still holds no authority check, and must not grow one.
//!
//! [`crate::deadman::DeadManHook`] (P1.7.3) is innermost, and it implements
//! **both** halves — which is the resolution of a real tension rather than an
//! oversight, argued in full in [`crate::deadman`]. The two halves do two
//! different jobs: `observe` detects the chord and owns every piece of switch
//! state, so no other policy can stop the human's off-switch from firing;
//! `gate` only decides what the confined app sees, withholding the chord key's
//! press until a tap can be told from a hold. It therefore *does* consume a
//! release — the one place in this core that does — and that is sound only
//! because it consumed that release's press too, so the pair is atomic and the
//! app's accounting is never split. The router's per-keysym pairing is what
//! proves it: the reconciliation finds no delivered press and does nothing.
//!
//! The first real observe-side consumer was [`PresenceHook`] (P1.4.4): it
//! feeds [`PhysicalPresence`], the physical-activity state behind the
//! enforcement chokepoint's `preempted` refusal — attached in the same shape
//! (non-consuming, wraps an inner hook), so the chokepoint's "does physical
//! input own the target" judgement rides the same single tap point as every
//! other preemption policy.
//!
//! # What arrives later (deliberately not here)
//!
//! - **Agent actuation intake (P1.4.x):** `vitrin_actuator_pointer` /
//!   `vitrin_actuator_text` requests pass the enforcement chokepoint
//!   (P1.4.4 — grant, verbs, constraints, and the token-bucket rate limit
//!   of PRD Doc 2 §8) *before* being wrapped by [`SeatInput::emulated`]
//!   and routed here. The router is not an authority check and must never
//!   grow one: by the time an event reaches it, the authority question is
//!   settled (prose page 11). One pointer state serves both origins in v0
//!   (one seat, one cursor per realm); multi-principal routing is the
//!   Phase-2+ generalization.
//! - **Keyboard interpretation never lands here.** Keys travel as
//!   xkbcommon keysyms and the core does *no keymap interpretation* (IDL
//!   `vitrin_shim_seat.key`); see [`invariant_keysym`] for what nested
//!   intake can honestly translate today and why.

use smithay::backend::input as host;
use smithay::backend::input::{
    AbsolutePositionEvent, InputBackend, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent,
};
use vitrin_protocol::generated::vitrin_actuator_pointer::{Axis, ButtonState};
use vitrin_protocol::generated::vitrin_shim_seat as seat;
use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};
use vitrin_protocol::Fixed;

use crate::scene::layout;

/// xkb keycodes are evdev scancodes offset by 8 (the historical X11
/// convention); Smithay's `KeyboardKeyEvent::key_code` is in the xkb
/// domain, the kernel's `KEY_*` constants are not.
const XKB_KEYCODE_OFFSET: u32 = 8;

/// Continuous (pixel-delta) scroll converted to wire `value120`: one wheel
/// notch = 120 = 15 pixels, the conventional libinput/toolkit equivalence.
/// Any fixed choice works; this one keeps a three-notch wheel and a 45 px
/// touchpad fling the same size in the app.
const V120_PER_SCROLL_PIXEL: f64 = 120.0 / 15.0;

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
    /// A key as an xkbcommon keysym, already modifier-resolved.
    Key { keysym: u32, state: KeyState },
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
/// wire's 24.8 fixed-point. Constructed only by [`InputRouter::route`],
/// which moves the origin from the [`SeatInput`] it consumed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeatDelivery {
    origin: Origin,
    kind: SeatDeliveryKind,
}

/// The wire-shaped payload of a [`SeatDelivery`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeatDeliveryKind {
    Motion { x: Fixed, y: Fixed },
    Button { button: u32, state: ButtonState },
    Scroll { axis: Axis, value120: i32 },
    Key { keysym: u32, state: KeyState },
    Text { text: String },
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
    /// ([`crate::recorder::Event::SeatDelivered`]): `motion`/`button`/
    /// `scroll`/`key`/`text`. Shape only, deliberately no payload -- an audit
    /// entry carrying keysyms or typed bytes would be a keylogger.
    pub fn event_label(&self) -> &'static str {
        match &self.kind {
            SeatDeliveryKind::Motion { .. } => "motion",
            SeatDeliveryKind::Button { .. } => "button",
            SeatDeliveryKind::Scroll { .. } => "scroll",
            SeatDeliveryKind::Key { .. } => "key",
            SeatDeliveryKind::Text { .. } => "text",
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

/// Journal one delivered seat event with its origin (issue #83).
///
/// The **single funnel** both delivery paths call — `session::route_seat` for
/// agent actuations and the nested backend's physical input — so the two
/// cannot record the B2 audit differently, and one test (in this module)
/// covers the code both run.
///
/// Pointer **motion is deliberately not journaled**. On the physical path it
/// arrives at raw device rate with no chokepoint token bucket to bound it (an
/// agent's actuations are rate-limited before they reach here; a human's are
/// not), and a delivery's coordinates are never recorded anyway — only its kind
/// and origin — so a per-event motion line would flood the recorder against its
/// own bounded-write discipline while adding no auditable fact the surrounding
/// button/scroll/key/text lines do not. The physical-vs-emulated distinction
/// B2 exists for is carried in full by those discrete events.
pub(crate) fn record_seat_delivery(
    recorder: &mut crate::recorder::Recorder,
    delivery: &SeatDelivery,
) {
    if matches!(delivery.kind(), SeatDeliveryKind::Motion { .. }) {
        return;
    }
    recorder.record(crate::recorder::Event::SeatDelivered {
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
/// holding anything. [`crate::deadman`] takes exactly this exception for
/// the chord key and nothing else. The rule above is otherwise absolute:
/// a gate must not consume a release whose press the *router* delivered.
///
/// [`observe`]: PreemptionHook::observe
/// [`gate`]: PreemptionHook::gate
pub(crate) trait PreemptionHook {
    /// Non-consuming tap: sees every event at intake, in view coordinates,
    /// before and regardless of gating.
    fn observe(&mut self, input: &SeatInput);

    /// Consuming gate: runs after [`observe`](Self::observe); a
    /// [`Gate::Consume`] verdict stops the event before routing.
    fn gate(&mut self, input: &SeatInput) -> Gate;
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

/// Physical-input presence: the state behind the enforcement chokepoint's
/// `preempted` refusal (P1.4.4). "Physical human input owns the target
/// right now" (IDL) holds while either
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
/// One tracker serves the process in v0 (one realm, one seat, one human);
/// per-realm presence is the Phase-2 multi-realm generalization.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Default)]
pub(crate) struct PhysicalPresence {
    /// Button codes of physically pressed, not-yet-released buttons (a
    /// multiset, mirroring the router's implicit-grab bookkeeping).
    held_buttons: Vec<u32>,
    /// When the most recent physical event was observed.
    last_activity: Option<std::time::Instant>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl PhysicalPresence {
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
    /// [`PresenceHook::observe`], which passes the tag intake bound.
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

/// The [`PhysicalPresence`] tracker attached at THE preemption hook point:
/// an observe-side tap -- non-consuming, sees every event even while the
/// consent grab consumes delivery, the same shape P1.7.3's dead-man watcher
/// detects with -- wrapping an inner hook so the P1.7.x consumers stack
/// beside it rather than displacing one another. Because
/// the hook trait deliberately carries no clock (the router never reads
/// one), the embedder that drives `route` shares a clock cell with this
/// hook and advances it to the dispatch turn's injected `now` -- the same
/// single-sample instant the rest of the turn uses.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PresenceHook<H: PreemptionHook> {
    presence: std::rc::Rc<std::cell::RefCell<PhysicalPresence>>,
    now: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
    inner: H,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<H: PreemptionHook> PresenceHook<H> {
    pub fn new(
        presence: std::rc::Rc<std::cell::RefCell<PhysicalPresence>>,
        now: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
        inner: H,
    ) -> Self {
        Self {
            presence,
            now,
            inner,
        }
    }
}

impl<H: PreemptionHook> PreemptionHook for PresenceHook<H> {
    fn observe(&mut self, input: &SeatInput) {
        self.presence
            .borrow_mut()
            .note(input.origin, &input.kind, self.now.get());
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        self.inner.gate(input)
    }
}

/// The per-realm input router: the one path from tagged intake events to
/// wire-ready seat deliveries. Holds the seat's pointer state (v0: one
/// seat, one cursor, shared by both origins) and the preemption hook.
pub(crate) struct InputRouter<H: PreemptionHook> {
    hook: H,
    /// Last known pointer position in view coordinates — buttons and
    /// scroll carry no position of their own and hit-test against this.
    /// Updated at intake (a physical fact), even for events the gate
    /// consumes, so a released grab never hit-tests a stale position.
    pointer: Option<(f64, f64)>,
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
    /// reason: where a pointer *is* is a fact, not a delivery outcome. Cleared
    /// by [`Self::reset`] with the rest of the per-shim-generation state.
    ///
    /// **This is display state and nothing else.** It feeds no hit test, no
    /// routing decision, and no wire event; delivery to the shim remains one
    /// shared position per realm view (module docs; D-019 supersedes only the
    /// "composites no cursor" half of D-017, never the delivery half).
    ///
    /// [`ConsentGrab::pointer`]: crate::consent::grab::ConsentGrab
    agent_pointer: Option<(f64, f64)>,
    /// Button codes of delivered-and-unreleased presses, in press order
    /// (a multiset: a pathological double-press pairs with a double-
    /// release). Nonempty means an implicit grab holds the pointer on
    /// the surface; a release is delivered iff its code is present here —
    /// per-button pairing, Wayland-style, never a bare count.
    pressed: Vec<u32>,
    /// Delivered-and-unreleased key presses as `(keysym, origin)`, same
    /// multiset discipline as [`Self::pressed`] and for the same razor: a key
    /// release is delivered iff its own press was (module docs). Keys hold
    /// no implicit grab — they carry no geometry — so this is *only*
    /// pairing, and it is what lets a consuming gate stop a key release
    /// without leaving a latched modifier in the app.
    ///
    /// **Why the origin is stored and not assumed.** Both origins share this
    /// router on purpose (`session::route_seat` routes chokepoint-admitted
    /// actuations through the very same instance, which is what makes the
    /// preemption hook meaningful), so the table holds an agent's held keys
    /// beside a human's. Anything that synthesises a release from an entry
    /// therefore has to read the tag back off the entry rather than assume
    /// one: minting `Origin::Physical` for an agent's key would forge the
    /// physical-vs-emulated distinction (B2) on the wire *and* in the flight
    /// recorder — the exact thing [`SeatInput::physical`]'s privacy makes a
    /// compile error at intake. Pairing itself stays per-keysym, not
    /// per-`(keysym, origin)`: the app has one keyboard, and its latch state
    /// counts presses, not who made them.
    pressed_keys: Vec<(u32, Origin)>,
}

impl<H: PreemptionHook> InputRouter<H> {
    pub fn new(hook: H) -> Self {
        Self {
            hook,
            pointer: None,
            agent_pointer: None,
            pressed: Vec::new(),
            pressed_keys: Vec::new(),
        }
    }

    /// Where the **agent's** pointer last was, in view coordinates, or `None`
    /// before its first motion (and after [`Self::reset`]).
    ///
    /// Read only by the presentation side, to draw the sprite: the backends
    /// pull it once per dispatch round through
    /// [`crate::session::Presenter::set_agent_cursor`]. See
    /// [`Self::agent_pointer`] for why this is not [`Self::pointer`].
    pub fn agent_pointer(&self) -> Option<(f64, f64)> {
        self.agent_pointer
    }

    /// Forget all per-shim-generation seat state: the implicit-grab
    /// bookkeeping, the key pairing, and the last known pointer position.
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
    pub fn reset(&mut self) {
        self.pressed.clear();
        self.pressed_keys.clear();
        self.pointer = None;
        self.agent_pointer = None;
    }

    /// Release every key the router believes the app is holding **on the
    /// human's behalf**, in press order, and forget those — one wire-ready
    /// [`SeatDelivery`] per key, for the caller to deliver. Keys an agent is
    /// holding are left exactly as they were.
    ///
    /// This is [`Self::pressed_keys`]'s pairing invariant read out loud: the
    /// table holds exactly the keysyms whose **press this router delivered**,
    /// so one release per physical entry is precisely what the app is owed
    /// and never more. A key whose press a gate consumed (the dead-man
    /// chord's) is not in there and gets nothing, because the app never saw
    /// it go down.
    ///
    /// **Why only the physical entries.** The caller's whole warrant is that
    /// host-window focus loss is the moment the core knows *the human's*
    /// release will land somewhere else. Nothing about an agent's actuation
    /// channel changed — it does not run through the host window's keyboard
    /// focus at all — so draining an agent's held key here would invent a
    /// release the principal never sent, drop a modifier it is deliberately
    /// holding without telling it, and (since the delivery and the flight
    /// recorder both carry the tag) attribute that to the human. Both
    /// origins share this router by design (`session::route_seat`), so the
    /// filter is load-bearing, not defensive.
    ///
    /// Each release carries the tag its own entry recorded, so no origin is
    /// minted here; the filter is what makes that tag always `Physical` in
    /// practice. Entries are removed in place, so press order survives.
    ///
    /// **Why the preemption hook is bypassed.** These are not new physical
    /// events to judge — no key moved — they are the delivery debt the router
    /// itself is recording. Routing them back through [`Self::route`] would
    /// hand the dead-man watcher a release with no press, and a gate that
    /// consumed one would strand exactly the key this exists to free.
    ///
    /// Called on host-window focus loss (`crate::backend::winit`'s
    /// `handle_focus`), the one moment the core knows the physical release
    /// will be delivered somewhere else: winit's Wayland backend emits no key
    /// events at all on `wl_keyboard.leave`, so without this a key held
    /// across an alt-tab stays latched down in the confined app forever.
    ///
    /// Distinct from [`Self::reset`] on purpose. `reset` forgets *all*
    /// per-shim-generation state for a shim that is gone, where no delivery
    /// is possible or wanted; this pays the app what it is owed while the
    /// shim is very much alive, and leaves the pointer and the implicit grab
    /// untouched — a focus change is not a new shim generation.
    ///
    /// Known imprecision, bounded and deliberate: pairing is per-keysym (see
    /// [`Self::pressed_keys`]), so if both origins hold the *same* keysym at
    /// once, a release consumes the oldest entry regardless of tag and the
    /// surviving entry's tag can name the other origin. The number of
    /// outstanding presses is always right, and no release is ever emitted
    /// with a tag the table did not record — but in that one overlap the
    /// drain can skip a human's key (leaving it latched until the agent
    /// releases) or keep an agent's. Making the table exact would mean
    /// pairing per-`(keysym, origin)`, which would drop a human's release of
    /// an agent-pressed key as unpaired — a latched modifier, the worse of
    /// the two failures.
    pub fn release_physical_keys(&mut self) -> Vec<SeatDelivery> {
        let mut released = Vec::new();
        self.pressed_keys.retain(|&(keysym, origin)| {
            if origin != Origin::Physical {
                return true;
            }
            released.push(SeatDelivery {
                // The tag is read back off the entry, never minted: see the
                // origin argument above.
                origin,
                kind: SeatDeliveryKind::Key {
                    keysym,
                    state: KeyState::Released,
                },
            });
            false
        });
        released
    }

    /// Route one tagged event against the current geometry: `view` is the
    /// composed realm-view size (nested: the host window size the scene
    /// composes at), `surface` the committed client surface size
    /// ([`Scene::surface_size`](crate::scene::Scene::surface_size)), if
    /// any.
    ///
    /// Returns the wire-ready delivery, or `None` if the event was
    /// consumed by the gate or had no destination under the module's
    /// routing rules (matte hit, no committed surface, unpaired release).
    pub fn route(
        &mut self,
        input: SeatInput,
        view: (u32, u32),
        surface: Option<(u32, u32)>,
    ) -> Option<SeatDelivery> {
        // Position is recorded before gating: where the pointer *is* is a
        // physical fact, not a delivery outcome.
        if let SeatInputKind::Motion { x, y } = input.kind {
            self.pointer = Some((x, y));
            // ...and the agent-owned position beside it, for the display
            // sprite only, written by this one origin (see
            // `Self::agent_pointer`). Not an `else` branch: the shared
            // position keeps taking both origins, because that is what the
            // app is delivered.
            if input.origin == Origin::Emulated {
                self.agent_pointer = Some((x, y));
            }
        }

        // THE preemption hook point: observe unconditionally, then gate.
        self.hook.observe(&input);
        if self.hook.gate(&input) == Gate::Consume {
            // Delivered-state reconciliation: a consumed release still
            // physically ended a hold the app saw begin. Clear the
            // press's bookkeeping — nothing is delivered — so a consuming
            // gate can never wedge the implicit grab or strand a keysym in
            // the pairing table; the app-side pairing debt belongs to the
            // gate implementor (`Gate::Consume` docs). Both release kinds
            // are reconciled, because both are paired.
            match &input.kind {
                SeatInputKind::Button {
                    button,
                    state: ButtonState::Released,
                } => {
                    self.release_pressed(*button);
                }
                SeatInputKind::Key {
                    keysym,
                    state: KeyState::Released,
                } => {
                    self.release_pressed_key(*keysym);
                }
                _ => {}
            }
            return None;
        }

        let SeatInput { origin, kind } = input;
        let kind = match kind {
            // Keyboard focus is held on the app shim-side (IDL: focus is
            // synthesized in the shim in v1), so keys route without
            // geometry — but not without pairing: a release is delivered
            // iff its own press was, per keysym, so no policy that stops a
            // key release can leave a latched modifier behind in the app
            // (module docs).
            SeatInputKind::Key { keysym, state } => match state {
                KeyState::Pressed => {
                    // The tag rides into the pairing table with the press, so
                    // whatever synthesises this key's release later reads it
                    // back rather than assuming one ([`Self::pressed_keys`]).
                    self.pressed_keys.push((keysym, origin));
                    SeatDeliveryKind::Key { keysym, state }
                }
                KeyState::Released => {
                    if !self.release_pressed_key(keysym) {
                        return None;
                    }
                    SeatDeliveryKind::Key { keysym, state }
                }
            },
            SeatInputKind::Text { text } => SeatDeliveryKind::Text { text },

            SeatInputKind::Motion { x, y } => {
                // No committed surface: nothing to point at (and no
                // placement to map through) — not deliverable.
                let surface = surface?;
                let (sx, sy) = surface_local((x, y), view, surface);
                if self.pressed.is_empty() && !inside(sx, sy, surface) {
                    return None; // the matte is not the app
                }
                SeatDeliveryKind::Motion {
                    x: Fixed::from_f64(sx),
                    y: Fixed::from_f64(sy),
                }
            }

            SeatInputKind::Button { button, state } => match state {
                ButtonState::Pressed => {
                    if self.pressed.is_empty() && !self.pointer_over_surface(view, surface) {
                        return None; // press on the matte starts nothing
                    }
                    self.pressed.push(button);
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
                    if !self.release_pressed(button) {
                        return None;
                    }
                    SeatDeliveryKind::Button { button, state }
                }
            },

            SeatInputKind::Scroll { axis, value120 } => {
                if self.pressed.is_empty() && !self.pointer_over_surface(view, surface) {
                    return None;
                }
                SeatDeliveryKind::Scroll { axis, value120 }
            }
        };
        Some(SeatDelivery { origin, kind })
    }

    /// Remove one delivered-press entry for `button`, if any; returns
    /// whether one existed (i.e. whether a release of this code pairs
    /// with a delivered press).
    fn release_pressed(&mut self, button: u32) -> bool {
        match self.pressed.iter().position(|&b| b == button) {
            Some(i) => {
                self.pressed.remove(i);
                true
            }
            None => false,
        }
    }

    /// Remove one delivered-press entry for `keysym`, if any; returns
    /// whether one existed (i.e. whether a release of this keysym pairs
    /// with a delivered press). The keyboard twin of
    /// [`Self::release_pressed`].
    ///
    /// Matches on the keysym alone, never on the origin: the app has one
    /// keyboard and its latch state counts presses, so a release must be able
    /// to pay down any outstanding press of that keysym. The entry's tag is
    /// carried for the benefit of [`Self::release_physical_keys`], which is
    /// the only reader of it — see [`Self::pressed_keys`] for the bound on
    /// how far a mixed-origin overlap can skew it.
    fn release_pressed_key(&mut self, keysym: u32) -> bool {
        match self.pressed_keys.iter().position(|&(k, _)| k == keysym) {
            Some(i) => {
                self.pressed_keys.remove(i);
                true
            }
            None => false,
        }
    }

    /// Whether the last known pointer position lies over the placed
    /// surface rectangle (never true with no position or no surface).
    fn pointer_over_surface(&self, view: (u32, u32), surface: Option<(u32, u32)>) -> bool {
        let (Some(pointer), Some(surface)) = (self.pointer, surface) else {
            return false;
        };
        let (sx, sy) = surface_local(pointer, view, surface);
        inside(sx, sy, surface)
    }
}

/// View coordinates → surface-local coordinates, through the same
/// deterministic placement the compositor paints with ([`layout::place`]:
/// centered, 1:1, negative when the surface is center-cropped). Router and
/// scene can never disagree about where the surface is, because they ask
/// the same function.
fn surface_local(point: (f64, f64), view: (u32, u32), surface: (u32, u32)) -> (f64, f64) {
    let placement = layout::place(view, surface);
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
/// Unhandled event classes (touch, gestures, tablet, switches, relative
/// motion) are dropped here: v0's seat vocabulary is pointer + keyboard
/// (IDL `vitrin_shim_seat`), and inventing a lossy translation at intake
/// would bypass the protocol's own growth path.
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
        _ => Vec::new(),
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
    Some(SeatInputKind::Key { keysym, state })
}

/// Resolve winit's interpreted `logical_key` to an X keysym — the #118 seam
/// that lets nested typing carry layout-*dependent* keys (letters, digits,
/// punctuation) at all: producing their keysym needs the interpretation the
/// host's keymap already performed, which the core is forbidden to redo (no
/// keymap lives here; see [`invariant_keysym`]).
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
/// already speaks — **not** keymap interpretation, which the IDL keeps out
/// of the core (`vitrin_shim_seat.key`: keysyms travel the wire precisely
/// so no keymap lives here).
///
/// Layout-*dependent* keys (letters, digits, punctuation) return `None`
/// here, because producing their keysym requires the interpreted key the
/// host already computed — [`host_keysym`] resolves those, and
/// [`resolve_key_seat`] prefers it over this table. This subset is chosen so
/// the consent / revocation paths never depend on the host resolving
/// anything: Escape — P1.7.3's hold-Esc chord — is layout-invariant and
/// always translates, from either path.
fn invariant_keysym(evdev_code: u32) -> Option<u32> {
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
        _ => return None,
    })
}

/// Whether nested intake can ever produce `keysym` — i.e. whether some evdev
/// scancode maps to it through [`invariant_keysym`].
///
/// Exists for one caller and one hazard: [`crate::deadman::Chord::parse`]
/// validates the configured dead-man chord against it, so a session can
/// never come up with an off-switch whose key intake silently drops. Asking
/// the real table rather than restating it is the whole point — a
/// hand-maintained copy of the mapping would be free to drift, and the
/// symptom of that drift is a dead-man switch that never fires, which is the
/// worst possible thing to discover late.
///
/// The scan is over the byte-wide evdev keycode space the kernel defines
/// (`input-event-codes.h` keeps `KEY_*` under 256, and [`invariant_keysym`]'s
/// table is entirely within it); it runs once per process, at argument
/// parsing.
pub(crate) fn keysym_is_intakeable(keysym: u32) -> bool {
    (0u32..256).any(|code| invariant_keysym(code) == Some(keysym))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::os::fd::AsFd;
    use std::rc::Rc;

    use smithay::backend::input::{
        AxisRelativeDirection, AxisSource, Device, DeviceCapability, Event, Keycode, UnusedEvent,
    };
    use vitrin_ipc::{Connection, TransportError};
    use vitrin_mock_shim::{MockShim, SeatEvent};

    use super::*;
    use crate::scene::Scene;
    use crate::shim::{ShimConfig, ShimServer};

    // ------------------------------------------------------------------
    // A synthetic Smithay input backend: winit cannot run in CI, so the
    // generic [`intake_physical`] is driven with handcrafted host events
    // through the same `InputBackend` trait surface `WinitInput` provides.
    // ------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SyntheticDevice;

    impl Device for SyntheticDevice {
        fn id(&self) -> String {
            "synthetic".into()
        }
        fn name(&self) -> String {
            "synthetic host device".into()
        }
        fn has_capability(&self, _capability: DeviceCapability) -> bool {
            true
        }
        fn usb_id(&self) -> Option<(u32, u32)> {
            None
        }
        fn syspath(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    /// Absolute pointer motion already in target coordinates (the
    /// transform is the identity, mirroring how the nested window's
    /// physical pixels are the view's pixels).
    struct SyntheticMotion {
        x: f64,
        y: f64,
    }

    impl Event<SyntheticHost> for SyntheticMotion {
        fn time(&self) -> u64 {
            0
        }
        fn device(&self) -> SyntheticDevice {
            SyntheticDevice
        }
    }

    impl AbsolutePositionEvent<SyntheticHost> for SyntheticMotion {
        fn x(&self) -> f64 {
            self.x
        }
        fn y(&self) -> f64 {
            self.y
        }
        fn x_transformed(&self, _width: i32) -> f64 {
            self.x
        }
        fn y_transformed(&self, _height: i32) -> f64 {
            self.y
        }
    }

    impl smithay::backend::input::PointerMotionAbsoluteEvent<SyntheticHost> for SyntheticMotion {}

    struct SyntheticButton {
        code: u32,
        state: host::ButtonState,
    }

    impl Event<SyntheticHost> for SyntheticButton {
        fn time(&self) -> u64 {
            0
        }
        fn device(&self) -> SyntheticDevice {
            SyntheticDevice
        }
    }

    impl PointerButtonEvent<SyntheticHost> for SyntheticButton {
        fn button_code(&self) -> u32 {
            self.code
        }
        fn state(&self) -> host::ButtonState {
            self.state
        }
    }

    /// Scroll with independent per-axis discrete (`v120`) and continuous
    /// (pixel) amounts, `(vertical, horizontal)`.
    struct SyntheticScroll {
        v120: (Option<f64>, Option<f64>),
        pixels: (Option<f64>, Option<f64>),
    }

    impl Event<SyntheticHost> for SyntheticScroll {
        fn time(&self) -> u64 {
            0
        }
        fn device(&self) -> SyntheticDevice {
            SyntheticDevice
        }
    }

    impl PointerAxisEvent<SyntheticHost> for SyntheticScroll {
        fn amount(&self, axis: host::Axis) -> Option<f64> {
            match axis {
                host::Axis::Vertical => self.pixels.0,
                host::Axis::Horizontal => self.pixels.1,
            }
        }
        fn amount_v120(&self, axis: host::Axis) -> Option<f64> {
            match axis {
                host::Axis::Vertical => self.v120.0,
                host::Axis::Horizontal => self.v120.1,
            }
        }
        fn source(&self) -> AxisSource {
            if self.v120.0.is_some() || self.v120.1.is_some() {
                AxisSource::Wheel
            } else {
                AxisSource::Continuous
            }
        }
        fn relative_direction(&self, _axis: host::Axis) -> AxisRelativeDirection {
            AxisRelativeDirection::Identical
        }
    }

    struct SyntheticKey {
        evdev: u32,
        state: host::KeyState,
    }

    impl Event<SyntheticHost> for SyntheticKey {
        fn time(&self) -> u64 {
            0
        }
        fn device(&self) -> SyntheticDevice {
            SyntheticDevice
        }
    }

    impl KeyboardKeyEvent<SyntheticHost> for SyntheticKey {
        fn key_code(&self) -> Keycode {
            // The xkb-domain keycode, exactly as Smithay's winit backend
            // produces it (evdev scancode + 8).
            (self.evdev + XKB_KEYCODE_OFFSET).into()
        }
        fn state(&self) -> host::KeyState {
            self.state
        }
        fn count(&self) -> u32 {
            1
        }
    }

    struct SyntheticHost;

    impl InputBackend for SyntheticHost {
        type Device = SyntheticDevice;
        type KeyboardKeyEvent = SyntheticKey;
        type PointerAxisEvent = SyntheticScroll;
        type PointerButtonEvent = SyntheticButton;
        type PointerMotionEvent = UnusedEvent;
        type PointerMotionAbsoluteEvent = SyntheticMotion;
        type GestureSwipeBeginEvent = UnusedEvent;
        type GestureSwipeUpdateEvent = UnusedEvent;
        type GestureSwipeEndEvent = UnusedEvent;
        type GesturePinchBeginEvent = UnusedEvent;
        type GesturePinchUpdateEvent = UnusedEvent;
        type GesturePinchEndEvent = UnusedEvent;
        type GestureHoldBeginEvent = UnusedEvent;
        type GestureHoldEndEvent = UnusedEvent;
        type TouchDownEvent = UnusedEvent;
        type TouchUpEvent = UnusedEvent;
        type TouchMotionEvent = UnusedEvent;
        type TouchCancelEvent = UnusedEvent;
        type TouchFrameEvent = UnusedEvent;
        type TabletToolAxisEvent = UnusedEvent;
        type TabletToolProximityEvent = UnusedEvent;
        type TabletToolTipEvent = UnusedEvent;
        type TabletToolButtonEvent = UnusedEvent;
        type SwitchToggleEvent = UnusedEvent;
        type SpecialEvent = ();
    }

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

    /// XK_Escape — the default dead-man chord.
    pub(crate) const CHORD_KEYSYM: u32 = 0xff1b;
    /// XK_Return — a layout-invariant key that is *not* the chord.
    const NON_CHORD_KEYSYM: u32 = 0xff0d;

    /// The human pressing the dead-man chord key.
    pub(crate) fn chord_press() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
            keysym: CHORD_KEYSYM,
            state: KeyState::Pressed,
        })
    }

    /// The human releasing it.
    pub(crate) fn chord_release() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
            keysym: CHORD_KEYSYM,
            state: KeyState::Released,
        })
    }

    /// An **agent** actuating the chord key: must never arm the human's
    /// switch.
    pub(crate) fn emulated_chord_press() -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
            keysym: CHORD_KEYSYM,
            state: KeyState::Pressed,
        })
    }

    /// An agent actuating the chord key's release: must never disarm a hold
    /// the human has in progress.
    pub(crate) fn emulated_chord_release() -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
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
            keysym: NON_CHORD_KEYSYM,
            state: KeyState::Pressed,
        })
    }

    /// Its release.
    pub(crate) fn other_key_release() -> SeatInput {
        SeatInput::physical(SeatInputKind::Key {
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
                keysym: 0x0061,
                state: KeyState::Pressed,
            },
        );
        // It overrides the invariant table for the same scancode, too: once the
        // host has interpreted a key, its interpretation is authoritative.
        assert_eq!(
            resolve_key_seat(1, Some(0x0041), KeyState::Pressed),
            Some(SeatInputKind::Key {
                keysym: 0x0041,
                state: KeyState::Pressed,
            }),
        );
        // With no host keysym, the invariant table still resolves Escape...
        assert_eq!(
            resolve_key_seat(1, None, KeyState::Released),
            Some(SeatInputKind::Key {
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
        let view = (VIEW_W, VIEW_H);
        let surface = Some(view);
        let mut router = InputRouter::new(NoopHook);

        // KEY_H (scancode 35) is layout-dependent — dropped with no host
        // keysym; given winit's `logical_key` ('h' = 0x0068) it flows.
        for input in physical_key(35, Some(0x0068), KeyState::Pressed) {
            if let Some(delivery) = router.route(input, view, surface) {
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
        assert_eq!(invariant_keysym(105), Some(0xff51)); // Left
        assert_eq!(invariant_keysym(125), Some(0xffeb)); // Super_L
                                                         // Layout-dependent: letters, digits, punctuation.
        assert_eq!(invariant_keysym(30), None); // KEY_A
        assert_eq!(invariant_keysym(2), None); // KEY_1
        assert_eq!(invariant_keysym(51), None); // KEY_COMMA
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

    fn router() -> InputRouter<NoopHook> {
        InputRouter::new(NoopHook)
    }

    fn phys(kind: SeatInputKind) -> SeatInput {
        SeatInput::physical(kind)
    }

    fn motion(x: f64, y: f64) -> SeatInputKind {
        SeatInputKind::Motion { x, y }
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
        let view = (64, 48);
        let surface = Some((64, 48));
        assert_eq!(router.agent_pointer(), None, "nothing has moved yet");

        // A human's motion moves the shared position and NOT the agent's.
        router.route(phys(motion(10.0, 20.0)), view, surface);
        assert_eq!(
            router.agent_pointer(),
            None,
            "physical motion must never move the agent's cursor"
        );

        // An agent's motion moves both: the shared one because that is what
        // the app is delivered, the agent-owned one because that is what the
        // sprite is drawn at.
        router.route(SeatInput::emulated(motion(30.0, 40.0)), view, surface);
        assert_eq!(router.agent_pointer(), Some((30.0, 40.0)));

        // A later human motion does not drag the sprite along with it.
        router.route(phys(motion(1.0, 2.0)), view, surface);
        assert_eq!(
            router.agent_pointer(),
            Some((30.0, 40.0)),
            "the agent's cursor moved because the human's mouse did"
        );

        // Recorded at intake, before gating and before hit-testing: a motion
        // onto the letterbox matte is not delivered, but the pointer is still
        // there and the sprite must be drawn there.
        router.route(
            SeatInput::emulated(motion(200.0, 200.0)),
            view,
            Some((10, 10)),
        );
        assert_eq!(router.agent_pointer(), Some((200.0, 200.0)));

        // Realm teardown forgets it, so no sprite hovers over a realm that is
        // gone.
        router.reset();
        assert_eq!(router.agent_pointer(), None);
    }

    #[test]
    fn exact_fit_motion_is_the_identity_mapping() {
        // The steady state of single-maximized: surface == view, placement
        // zero — wire coordinates equal view coordinates, matching the
        // IDL's realm-view-pixel flows number for number.
        let mut router = router();
        let out = router
            .route(phys(motion(10.25, 47.5)), (64, 48), Some((64, 48)))
            .expect("inside the surface");
        assert_motion(&out, 10.25, 47.5);
        assert_eq!(out.origin(), Origin::Physical);
    }

    #[test]
    fn letterboxed_motion_subtracts_the_placement_offset() {
        // View 100x80, surface 40x20: placed at (30, 30) — the same
        // placement the compositor paints (scene::layout::place).
        let mut router = router();
        let view = (100, 80);
        let surface = Some((40, 20));

        // Top-left surface pixel.
        let out = router.route(phys(motion(30.0, 30.0)), view, surface);
        assert_motion(&out.expect("surface origin"), 0.0, 0.0);
        // Bottom-right interior point.
        let out = router.route(phys(motion(69.5, 49.5)), view, surface);
        assert_motion(&out.expect("inside"), 39.5, 19.5);
        // One pixel past the right edge: matte, not app.
        assert!(router
            .route(phys(motion(70.0, 40.0)), view, surface)
            .is_none());
        // Just left of the placed rectangle: matte.
        assert!(router
            .route(phys(motion(29.5, 40.0)), view, surface)
            .is_none());
    }

    #[test]
    fn center_cropped_motion_adds_the_crop_offset() {
        // View 40x30, surface 60x50: placement (-10, -10) — the negative-
        // offset case. The view's origin shows surface pixel (10, 10), and
        // every view point lies over the surface.
        let mut router = router();
        let view = (40, 30);
        let surface = Some((60, 50));

        let out = router.route(phys(motion(0.0, 0.0)), view, surface);
        assert_motion(&out.expect("crop origin"), 10.0, 10.0);
        let out = router.route(phys(motion(39.0, 29.0)), view, surface);
        assert_motion(&out.expect("crop interior"), 49.0, 39.0);
    }

    #[test]
    fn mixed_letterbox_and_crop_axes_map_independently() {
        // View 100x20, surface 40x50 (the #19 mixed case): x letterboxed
        // (+30), y center-cropped (-15).
        let mut router = router();
        let view = (100, 20);
        let surface = Some((40, 50));

        let out = router.route(phys(motion(30.0, 0.0)), view, surface);
        assert_motion(&out.expect("placed origin"), 0.0, 15.0);
        let out = router.route(phys(motion(69.0, 19.0)), view, surface);
        assert_motion(&out.expect("placed interior"), 39.0, 34.0);
        // Left of the placed rectangle: matte on the x axis.
        assert!(router
            .route(phys(motion(29.0, 10.0)), view, surface)
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
        let out = router
            .route(phys(motion(0.5, 0.25)), (10, 10), Some((10, 10)))
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
        let view = (100, 80);
        let surface = Some((40, 20)); // placed at (30, 30)

        assert!(router
            .route(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
        assert!(router.route(phys(press()), view, surface).is_none());
        assert!(router.route(phys(scroll()), view, surface).is_none());
        assert!(router.route(phys(release()), view, surface).is_none());
        let key = router.route(
            phys(SeatInputKind::Key {
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
        let view = (100, 80);
        let surface = Some((40, 20)); // placed at (30, 30)

        // Move onto the surface and press: both delivered.
        assert!(router
            .route(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router.route(phys(press()), view, surface).is_some());

        // Drag off the surface: motion keeps flowing under the implicit
        // grab, with out-of-bounds (here negative) surface-local
        // coordinates — the wire's fixed is signed for exactly this.
        let out = router.route(phys(motion(0.0, 0.0)), view, surface);
        assert_motion(&out.expect("grabbed motion"), -30.0, -30.0);
        // Scroll during the grab flows too.
        assert!(router.route(phys(scroll()), view, surface).is_some());
        // The release lands wherever the drag ended: delivered, so the app
        // never holds a stuck button.
        assert!(router.route(phys(release()), view, surface).is_some());

        // Grab over: the same off-surface events are matte again.
        assert!(router
            .route(phys(motion(0.0, 0.0)), view, surface)
            .is_none());
        assert!(router.route(phys(scroll()), view, surface).is_none());
    }

    #[test]
    fn releases_pair_per_button_code_not_by_count() {
        // Issue #24 review: pairing is per button code, never a bare
        // count. A BTN_LEFT press dropped on the matte must not pair with
        // a later delivered BTN_RIGHT press — the wire must never see a
        // release for a button the app never saw pressed, and the app
        // must never be left holding a stranded button.
        let mut router = router();
        let view = (100, 80);
        let surface = Some((40, 20)); // placed at (30, 30)
        let button = |code, state| SeatInputKind::Button {
            button: code,
            state,
        };

        // BTN_LEFT pressed on the matte: dropped, no grab.
        assert!(router
            .route(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
        assert!(router
            .route(phys(button(0x110, ButtonState::Pressed)), view, surface)
            .is_none());
        // Drag onto the surface (the host keeps reporting motion while
        // the button stays physically held) and press BTN_RIGHT there.
        assert!(router
            .route(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router
            .route(phys(button(0x111, ButtonState::Pressed)), view, surface)
            .is_some());
        // BTN_LEFT's release: its press was never delivered — dropped,
        // it must not consume BTN_RIGHT's grab.
        assert!(router
            .route(phys(button(0x110, ButtonState::Released)), view, surface)
            .is_none());
        // BTN_RIGHT's release still pairs: no stuck button in the app.
        assert!(router
            .route(phys(button(0x111, ButtonState::Released)), view, surface)
            .is_some());
        // The grab is fully over: off-surface motion is matte again.
        assert!(router
            .route(phys(motion(0.0, 0.0)), view, surface)
            .is_none());
    }

    #[test]
    fn no_committed_surface_means_no_pointer_path_but_keys_flow() {
        // Headless-at-startup / pre-first-commit: there is nothing to
        // point at, so the pointer path yields nothing — no phantom
        // deliveries. Keys and text still route; the shim owns that
        // judgement.
        let mut router = router();
        let view = (100, 80);

        assert!(router.route(phys(motion(10.0, 10.0)), view, None).is_none());
        assert!(router.route(phys(press()), view, None).is_none());
        assert!(router.route(phys(scroll()), view, None).is_none());
        let key = router.route(
            phys(SeatInputKind::Key {
                keysym: 0xff1b,
                state: KeyState::Pressed,
            }),
            view,
            None,
        );
        assert!(key.is_some());
        let text = router.route(
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
        let mut router = InputRouter::new(RecordingHook {
            log: Rc::clone(&log),
            consume: Rc::clone(&consume),
        });
        let view = (64, 48);
        let surface = Some((64, 48));

        // Delivering: observe precedes gate.
        assert!(router
            .route(phys(motion(1.0, 1.0)), view, surface)
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
            .route(phys(motion(2.0, 2.0)), view, surface)
            .is_none());
        assert!(router
            .route(SeatInput::emulated(scroll()), view, surface)
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
        let mut router = InputRouter::new(RecordingHook {
            log,
            consume: Rc::clone(&consume),
        });
        let view = (64, 48);
        let surface = Some((64, 48));

        // Pointer on the surface; a grab-holder consumes the press.
        assert!(router
            .route(phys(motion(5.0, 5.0)), view, surface)
            .is_some());
        consume.set(true);
        assert!(router.route(phys(press()), view, surface).is_none());

        // Gate reopens: no implicit grab exists, so off-surface motion is
        // matte and the (never-pressed) release is dropped unpaired.
        consume.set(false);
        assert!(router
            .route(phys(motion(1000.0, 1000.0)), view, surface)
            .is_none());
        assert!(router.route(phys(release()), view, surface).is_none());
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
        let mut router = InputRouter::new(RecordingHook {
            log,
            consume: Rc::clone(&consume),
        });
        let view = (100, 80);
        let surface = Some((40, 20)); // placed at (30, 30)

        // Press delivered on the surface: implicit grab begins.
        assert!(router
            .route(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router.route(phys(press()), view, surface).is_some());

        // The grab consumes the user's release: not delivered ...
        consume.set(true);
        assert!(router.route(phys(release()), view, surface).is_none());

        // ... but the router did not wedge. With the gate open again,
        // matte hit-testing works (no phantom grab force-delivers
        // off-surface events to the app), an unpaired release stays
        // dropped, and a fresh press/release pair over the surface
        // behaves normally.
        consume.set(false);
        assert!(router
            .route(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
        assert!(router.route(phys(press()), view, surface).is_none());
        assert!(router.route(phys(scroll()), view, surface).is_none());
        assert!(router.route(phys(release()), view, surface).is_none());
        assert!(router
            .route(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router.route(phys(press()), view, surface).is_some());
        assert!(router.route(phys(release()), view, surface).is_some());
        assert!(router
            .route(phys(motion(5.0, 5.0)), view, surface)
            .is_none());
    }

    #[test]
    fn reset_forgets_grab_and_pointer_across_shim_generations() {
        // Issue #24 review: a grab held at shim death must not become a
        // phantom grab against the next shim generation. `reset` is what
        // the realm teardown funnel (`ShimServer::connection_closed`)
        // invokes alongside `Scene::clear_surface`.
        let mut router = router();
        let view = (100, 80);
        let surface = Some((40, 20)); // placed at (30, 30)

        assert!(router
            .route(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router.route(phys(press()), view, surface).is_some());

        router.reset();

        // The stale release from the dead generation is unpaired at the
        // fresh seat: dropped.
        assert!(router.route(phys(release()), view, surface).is_none());
        // The pointer position was forgotten too: a press cannot
        // hit-test against a pre-teardown position (which sat over the
        // surface) — first motion must re-establish it.
        assert!(router.route(phys(press()), view, surface).is_none());
        // No phantom grab: off-surface events are matte again.
        assert!(router
            .route(phys(motion(0.0, 0.0)), view, surface)
            .is_none());
        assert!(router.route(phys(scroll()), view, surface).is_none());
        // The fresh generation then works normally.
        assert!(router
            .route(phys(motion(35.0, 35.0)), view, surface)
            .is_some());
        assert!(router.route(phys(press()), view, surface).is_some());
        assert!(router.route(phys(release()), view, surface).is_some());
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
            let view = (64, 48);
            let surface = Some((64, 48));

            let deliver = |router: &mut InputRouter<NoopHook>, kind| {
                router
                    .route(wrap(kind), view, surface)
                    .expect("routable by construction")
            };

            let bytes = deliver(&mut router, motion(7.0, 9.0)).encode(SEAT_ID);
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
            let view = (64, 48);
            let surface = Some((64, 48));
            let mut deliver = |kind| {
                router
                    .route(wrap(kind), view, surface)
                    .expect("routable by construction")
            };

            // Ordered as the router tolerates (a press leaves an implicit grab
            // the rest ride), mirroring the encode test above.
            for (kind, label) in [
                (motion(1.0, 2.0), "motion"),
                (press(), "button"),
                (scroll(), "scroll"),
                (
                    SeatInputKind::Key {
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
        let view = (64, 48);
        let surface = Some((64, 48));
        let motion_delivery = router
            .route(SeatInput::physical(motion(1.0, 2.0)), view, surface)
            .expect("motion routes");
        let button_delivery = router
            .route(SeatInput::physical(press()), view, surface)
            .expect("button routes");
        super::record_seat_delivery(&mut rec, &motion_delivery);
        super::record_seat_delivery(&mut rec, &button_delivery);

        let entries = crate::recorder::tests::read_log(&path);
        let delivered = crate::recorder::tests::of_kind(&entries, "seat_delivered");
        assert_eq!(delivered.len(), 1, "motion is not journaled; the button is");
        assert_eq!(delivered[0].str("event"), "button");
        assert_eq!(delivered[0].str("origin"), "physical");
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
            width: VIEW_W,
            height: VIEW_H,
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
        let view = (VIEW_W, VIEW_H);
        let surface = Some((VIEW_W, VIEW_H)); // steady state: surface fills the view

        let mut send_all = |inputs: Vec<SeatInput>, router: &mut InputRouter<NoopHook>| {
            for input in inputs {
                let delivery = router.route(input, view, surface).expect("routable");
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
            intake_physical(&motion_ev(10.0, 20.0), (VIEW_W as i32, VIEW_H as i32)),
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
                .route(
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
            width: VIEW_W,
            height: VIEW_H,
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
            .route(
                phys(SeatInputKind::Key {
                    keysym: 0xff1b,
                    state: KeyState::Pressed,
                }),
                (VIEW_W, VIEW_H),
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
            let router = InputRouter::new(crate::consent::grab::ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                SpyHook(Rc::clone(&spy)),
            ));
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
        ) else {
            panic!("an interactive petition must pend");
        };
        (registry, petition)
    }

    #[test]
    fn a_raised_prompt_stops_human_input_at_the_router() {
        // The acceptance criterion "all human input routes exclusively to
        // the prompt", asserted where it actually has to hold: the router,
        // whose output is the only thing that can reach a shim seat.
        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view = (900, 700);
        let surface_size = Some(view);
        rig.grab.borrow_mut().set_view(view);

        // Before the prompt: ordinary routing.
        assert!(rig
            .router
            .route(phys(motion(10.0, 10.0)), view, surface_size)
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
                keysym: 0xff0d,
                state: KeyState::Pressed,
            },
            SeatInputKind::Key {
                keysym: 0xff1b, // Escape, reserved for P1.7.3
                state: KeyState::Pressed,
            },
        ] {
            assert!(
                rig.router
                    .route(phys(kind.clone()), view, surface_size)
                    .is_none(),
                "{kind:?} reached the app while a prompt was up"
            );
        }

        // Lowering the prompt restores routing.
        rig.lower(&mut registry);
        assert!(rig
            .router
            .route(phys(motion(30.0, 30.0)), view, surface_size)
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
        let view = (900, 700);
        let surface_size = Some(view);
        rig.grab.borrow_mut().set_view(view);
        rig.raise_awake(petition, &mut registry);

        let gated_before = rig.spy.gated.get();
        let consumed = [
            motion(20.0, 20.0),
            press(),
            scroll(),
            SeatInputKind::Key {
                keysym: 0xff1b, // Escape: the revocation chord itself
                state: KeyState::Pressed,
            },
        ];
        let observed_before = rig.spy.observed.get();
        for kind in consumed.iter() {
            assert!(
                rig.router
                    .route(phys(kind.clone()), view, surface_size)
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
        let view = (900, 700);
        let surface_size = Some(view);
        rig.grab.borrow_mut().set_view(view);

        // A drag begins in the app.
        assert!(rig
            .router
            .route(phys(motion(40.0, 40.0)), view, surface_size)
            .is_some());
        assert!(rig
            .router
            .route(phys(press()), view, surface_size)
            .is_some());

        rig.raise_awake(petition, &mut registry);

        // Mid-drag motion is consumed (the app must not track the pointer
        // across a security decision) ...
        assert!(rig
            .router
            .route(phys(motion(50.0, 50.0)), view, surface_size)
            .is_none());
        // ... but the release still reaches the app: no stuck button.
        assert!(
            rig.router
                .route(phys(release()), view, surface_size)
                .is_some(),
            "hold-until-release: the drag's own release must land"
        );
        // A fresh press aimed at the prompt is consumed, and its release
        // is dropped unpaired by the router rather than leaking.
        assert!(rig
            .router
            .route(phys(press()), view, surface_size)
            .is_none());
        assert!(rig
            .router
            .route(phys(release()), view, surface_size)
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
        let key = |keysym, state| SeatInputKind::Key { keysym, state };

        let mut rig = Grabbed::new();
        let (mut registry, petition) = pending_petition();
        let view = (900, 700);
        let surface_size = Some(view);
        rig.grab.borrow_mut().set_view(view);

        // The human is holding Shift when the prompt appears.
        assert!(rig
            .router
            .route(phys(key(SHIFT_L, KeyState::Pressed)), view, surface_size)
            .is_some());
        rig.raise_awake(petition, &mut registry);

        // Key presses during the prompt are consumed ...
        assert!(rig
            .router
            .route(phys(key(CTRL_L, KeyState::Pressed)), view, surface_size)
            .is_none());
        // ... and that press's own release is dropped by the router's
        // pairing rather than arriving unpaired at the app.
        assert!(
            rig.router
                .route(phys(key(CTRL_L, KeyState::Released)), view, surface_size)
                .is_none(),
            "a release whose press the grab consumed must not reach the app"
        );
        // But the Shift the app already saw go down must come back up.
        assert!(
            rig.router
                .route(phys(key(SHIFT_L, KeyState::Released)), view, surface_size)
                .is_some(),
            "hold-until-release: a modifier held from before the prompt must be released"
        );

        // With the prompt down, typing is unmodified again -- and a second
        // Shift release, now unpaired, is dropped rather than doubled.
        rig.lower(&mut registry);
        assert!(rig
            .router
            .route(phys(key(SHIFT_L, KeyState::Released)), view, surface_size)
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
        let view = (900, 700);
        let surface_size = Some(view);
        rig.grab.borrow_mut().set_view(view);
        rig.raise_awake(petition, &mut registry);

        // Aim at Deny, using the same placement the compositor draws with.
        let card = rig
            .surface
            .card_origin(view.0, view.1)
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
                rig.router.route(phys(kind), view, surface_size).is_none(),
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
        let view = (VIEW_W, VIEW_H);
        let surface_size = Some(view);
        rig.grab.borrow_mut().set_view(view);

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
        let presence = PhysicalPresence::new();
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
            let mut routed: Vec<SeatInput> = Vec::new();
            let outcome = chokepoint
                .enforce_use(
                    UseRequest {
                        facet_id: 20,
                        grant_wire_id: 10,
                        grant_row: Some(row),
                        principal: &identity,
                        kind: UseKind::Pointer(SeatInputKind::Motion { x: 5.0, y: 6.0 }),
                    },
                    grants,
                    registry,
                    UseEnv {
                        realm_view: Some(&frame),
                        presence: &presence,
                        actuations: &mut |input| routed.push(input),
                    },
                    now,
                    &mut |_principal_frame, _fd| Ok(()),
                )
                .expect("transport is healthy");
            for input in routed {
                if let Some(delivery) = router.route(input, view, surface_size) {
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
            let router = InputRouter::new(crate::consent::grab::ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                crate::deadman::DeadManHook::new(
                    Rc::clone(&deadman),
                    Rc::clone(&now),
                    SpyHook(Rc::clone(&spy)),
                ),
            ));
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
        let view = (900, 700);
        let surface = Some(view);
        rig.grab.borrow_mut().set_view(view);

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
            .route(phys(chord_press().kind().clone()), view, surface)
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
        let view = (900, 700);
        let surface = Some(view);
        let t0 = rig.now.get();

        assert!(rig
            .router
            .route(phys(chord_press().kind().clone()), view, surface)
            .is_none());
        for step in 0..200 {
            rig.now.set(t0 + std::time::Duration::from_millis(step * 5));
            let _ = rig.router.route(emulated_chord_press(), view, surface);
            let _ = rig.router.route(emulated_chord_release(), view, surface);
            let _ = rig.router.route(emulated_motion(), view, surface);
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
        let view = (900, 700);
        let surface = Some(view);
        rig.grab.borrow_mut().set_view(view);
        let t0 = rig.now.get();

        // 1. The tap's press, no prompt up: withheld.
        assert!(rig
            .router
            .route(phys(chord_press().kind().clone()), view, surface)
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
            .route(phys(chord_release().kind().clone()), view, surface)
            .is_none());
        let replay = rig.deadman.borrow_mut().take_replay();
        assert_eq!(replay.len(), 2, "fixture check: a tap replays a pair");
        for input in replay {
            let _ = rig.router.route(input, view, surface);
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
                .route(phys(chord_press().kind().clone()), view, surface)
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
                .route(phys(chord_release().kind().clone()), view, surface)
                .is_none(),
            "the chord's release reached the app"
        );
        // And nothing is latched: the router holds no key it delivered a
        // press for, which is the property a split pair violates.
        assert!(
            rig.router.pressed_keys.is_empty(),
            "a chord left a key latched down in the confined app: {:?}",
            rig.router.pressed_keys
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
        let view = (VIEW_W, VIEW_H);
        let surface = Some(view);
        let host_view = (VIEW_W as i32, VIEW_H as i32);

        let deadman = Rc::new(RefCell::new(crate::deadman::DeadManSwitch::new(
            crate::deadman::DeadManConfig::default(),
        )));
        let now = Rc::new(Cell::new(std::time::Instant::now()));
        let mut router = InputRouter::new(crate::deadman::DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            NoopHook,
        ));

        // The embedder's loop, condensed: route each intake event, then
        // drain whatever the watcher decided the app is owed.
        let pump = |router: &mut InputRouter<crate::deadman::DeadManHook<NoopHook>>,
                    core: &mut Connection,
                    inputs: Vec<SeatInput>| {
            let send = |router: &mut InputRouter<crate::deadman::DeadManHook<NoopHook>>,
                        core: &mut Connection,
                        input| {
                if let Some(delivery) = router.route(input, view, surface) {
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
        let mut router = InputRouter::new(crate::deadman::DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            NoopHook,
        ));
        let view = (64, 48);
        let surface = Some(view);
        let t0 = now.get();

        assert!(router.route(chord_press(), view, surface).is_none());
        deadman
            .borrow_mut()
            .fire_if_due(t0 + crate::deadman::DEFAULT_HOLD);
        assert!(deadman.borrow_mut().take_trigger().is_some());
        assert!(router.route(chord_release(), view, surface).is_none());
        assert!(
            router.pressed_keys.is_empty(),
            "the chord latched a key in the router's pairing table: {:?}",
            router.pressed_keys
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
        let mut router = InputRouter::new(crate::deadman::DeadManHook::new(
            Rc::clone(&deadman),
            Rc::clone(&now),
            NoopHook,
        ));
        let view = (64, 48);
        let surface = Some(view);

        // Two ordinary keys go down and are delivered, then the chord goes
        // down and is withheld by the gate.
        for keysym in [NON_CHORD_KEYSYM, 0x061] {
            assert!(
                router
                    .route(
                        phys(SeatInputKind::Key {
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
            router.route(chord_press(), view, surface).is_none(),
            "fixture check: the chord's press is withheld"
        );

        // Focus leaves. Every key the app is holding — and only those — is
        // released, in press order.
        let released = router.release_physical_keys();
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
            router.pressed_keys.is_empty(),
            "the router still believes a key is down after focus loss: {:?}",
            router.pressed_keys
        );

        // Idempotent: a second focus-out (or a `handle_focus` racing the
        // synthetic releases) owes nothing.
        assert!(router.release_physical_keys().is_empty());

        // And X11's synthetic release, arriving after the drain, pairs with
        // nothing and is dropped -- the app is never told a key came up twice.
        assert!(
            router
                .route(
                    phys(SeatInputKind::Key {
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
        // not a new shim generation, which is what `reset` is for.
        let mut router = InputRouter::new(NoopHook);
        assert!(router
            .route(phys(motion(10.0, 10.0)), view, surface)
            .is_some());
        assert!(router.route(phys(press()), view, surface).is_some());
        assert!(router.release_physical_keys().is_empty());
        assert!(
            router.route(phys(release()), view, surface).is_some(),
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
        let mut router = InputRouter::new(NoopHook);
        let view = (64, 48);
        let surface = Some(view);
        const AGENT_KEY: u32 = 0xffe1; // Shift_L, an agent holding a modifier
        const HUMAN_KEY: u32 = 0x062; // 'b'

        for input in [
            SeatInput::emulated(SeatInputKind::Key {
                keysym: AGENT_KEY,
                state: KeyState::Pressed,
            }),
            phys(SeatInputKind::Key {
                keysym: HUMAN_KEY,
                state: KeyState::Pressed,
            }),
        ] {
            assert!(
                router.route(input, view, surface).is_some(),
                "fixture check: both origins' presses reach the app"
            );
        }

        // The human alt-tabs. Exactly one release goes out, it is the human's
        // key, and it carries the human's tag.
        let released = router.release_physical_keys();
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
            router.pressed_keys,
            vec![(AGENT_KEY, Origin::Emulated)],
            "the agent's press must survive a human's focus change, with its own tag"
        );

        // The agent's own release still pairs and still goes out as the
        // agent's -- the drain neither paid this debt nor forgot it.
        let delivery = router
            .route(
                SeatInput::emulated(SeatInputKind::Key {
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
        assert!(router.pressed_keys.is_empty());
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
        let presence = Rc::new(RefCell::new(PhysicalPresence::new()));
        let clock = Rc::new(Cell::new(t0));
        let consume = Rc::new(Cell::new(false));
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut router = InputRouter::new(PresenceHook::new(
            Rc::clone(&presence),
            Rc::clone(&clock),
            RecordingHook {
                log: Rc::clone(&log),
                consume: Rc::clone(&consume),
            },
        ));
        let view = (64, 48);
        let surface = Some((64, 48));

        // A delivered physical motion is observed and recorded at the
        // clock cell's injected instant.
        assert!(router
            .route(phys(motion(2.0, 2.0)), view, surface)
            .is_some());
        assert!(presence.borrow().owns_target(t0));
        assert!(!presence.borrow().owns_target(t0 + PHYSICAL_HOLD_WINDOW));

        // A consumed event still reaches the tap (observe precedes the
        // gate), at the advanced clock.
        let t1 = t0 + std::time::Duration::from_secs(10);
        clock.set(t1);
        consume.set(true);
        assert!(router
            .route(phys(motion(3.0, 3.0)), view, surface)
            .is_none());
        assert!(presence.borrow().owns_target(t1));

        // Emulated events pass the tap without arming it.
        let t2 = t1 + std::time::Duration::from_secs(10);
        clock.set(t2);
        consume.set(false);
        assert!(router
            .route(SeatInput::emulated(scroll()), view, surface)
            .is_some());
        assert!(!presence.borrow().owns_target(t2));

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
}
