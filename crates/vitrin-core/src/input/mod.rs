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
//! device at all**, and that absence is structural, not a runtime check:
//! the only producer of `Origin::Physical` events is [`intake_physical`],
//! whose only caller is the nested backend's winit event handler — the
//! headless backend contains no call site, so no phantom physical-input
//! path can exist there.
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
//!   constructors [`SeatInput::physical`] and [`SeatInput::emulated`] —
//!   an untagged event is unrepresentable.
//! - A [`SeatDelivery`] is constructed **only** by [`InputRouter::route`],
//!   which *moves* the origin from the `SeatInput` it consumed. The tag is
//!   never re-derived downstream, so it provably cannot drift between
//!   intake and the wire.
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
//! a release is delivered iff its press was).
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
//!    P1.7.3's hold-Esc revocation watcher attaches here: it must see raw
//!    physical events (Escape press/release — covered by the
//!    layout-invariant key table below) *even while a consent grab is
//!    consuming all delivery*.
//! 2. [`PreemptionHook::gate`] — the consuming gate. P1.7.2's consent
//!    input grab attaches here: returning [`Gate::Consume`] stops the
//!    event before mapping and before the wire. Consumed events update no
//!    grab state (a consumed press starts no implicit grab).
//!
//! The MVP implementation is [`NoopHook`] (observe nothing, deliver
//! everything) — the shape, placement, and ordering are the deliverable;
//! both P1.7.x consumers attach without restructuring the router.
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
    /// Runtime construction arrives with the P1.4.x actuation intake —
    /// physical intake never produces it.
    #[cfg_attr(not(test), allow(dead_code))]
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
    /// Tag input from a physical human device. The only call sites are the
    /// nested backend's intake ([`intake_physical`]) — headless mode has
    /// no physical source, structurally.
    pub fn physical(kind: SeatInputKind) -> Self {
        Self {
            origin: Origin::Physical,
            kind,
        }
    }

    /// Tag input from a principal's actuator. The P1.4.x actuation path
    /// wraps chokepoint-approved requests with this constructor.
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

/// Verdict of [`PreemptionHook::gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Route and deliver normally.
    Deliver,
    /// Consume: the event stops here — no mapping, no wire, no grab-state
    /// change.
    Consume,
}

/// The single preemption hook point of the input router (module docs lay
/// out the placement rationale). For every event, [`observe`] is called
/// first and unconditionally (P1.7.3's revocation watcher — cannot
/// consume), then [`gate`] may consume it (P1.7.2's consent grab).
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

/// The MVP hook: observes nothing, consumes nothing. Replaced (wrapped) by
/// the P1.7.x consent grab + revocation watcher.
pub(crate) struct NoopHook;

impl PreemptionHook for NoopHook {
    fn observe(&mut self, _input: &SeatInput) {}

    fn gate(&mut self, _input: &SeatInput) -> Gate {
        Gate::Deliver
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
    /// Number of delivered-and-unreleased button presses: nonzero means an
    /// implicit grab holds the pointer on the surface.
    pressed_buttons: u32,
}

impl<H: PreemptionHook> InputRouter<H> {
    pub fn new(hook: H) -> Self {
        Self {
            hook,
            pointer: None,
            pressed_buttons: 0,
        }
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
        }

        // THE preemption hook point: observe unconditionally, then gate.
        self.hook.observe(&input);
        if self.hook.gate(&input) == Gate::Consume {
            return None;
        }

        let SeatInput { origin, kind } = input;
        let kind = match kind {
            // Keyboard focus is held on the app shim-side (IDL: focus is
            // synthesized in the shim in v1), so keys and text route
            // unconditionally — no geometry involved.
            SeatInputKind::Key { keysym, state } => SeatDeliveryKind::Key { keysym, state },
            SeatInputKind::Text { text } => SeatDeliveryKind::Text { text },

            SeatInputKind::Motion { x, y } => {
                // No committed surface: nothing to point at (and no
                // placement to map through) — not deliverable.
                let surface = surface?;
                let (sx, sy) = surface_local((x, y), view, surface);
                if self.pressed_buttons == 0 && !inside(sx, sy, surface) {
                    return None; // the matte is not the app
                }
                SeatDeliveryKind::Motion {
                    x: Fixed::from_f64(sx),
                    y: Fixed::from_f64(sy),
                }
            }

            SeatInputKind::Button { button, state } => match state {
                ButtonState::Pressed => {
                    if self.pressed_buttons == 0 && !self.pointer_over_surface(view, surface) {
                        return None; // press on the matte starts nothing
                    }
                    self.pressed_buttons = self.pressed_buttons.saturating_add(1);
                    SeatDeliveryKind::Button { button, state }
                }
                ButtonState::Released => {
                    // A release is delivered iff its press was — the
                    // implicit grab guarantees the app never holds a stuck
                    // button, wherever the pointer wandered meanwhile.
                    if self.pressed_buttons == 0 {
                        return None;
                    }
                    self.pressed_buttons -= 1;
                    SeatDeliveryKind::Button { button, state }
                }
            },

            SeatInputKind::Scroll { axis, value120 } => {
                if self.pressed_buttons == 0 && !self.pointer_over_surface(view, surface) {
                    return None;
                }
                SeatDeliveryKind::Scroll { axis, value120 }
            }
        };
        Some(SeatDelivery { origin, kind })
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
            match invariant_keysym(evdev) {
                Some(keysym) => vec![SeatInput::physical(SeatInputKind::Key { keysym, state })],
                None => {
                    tracing::trace!(
                        evdev,
                        "layout-dependent key dropped at intake (see invariant_keysym docs)"
                    );
                    Vec::new()
                }
            }
        }
        _ => Vec::new(),
    }
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
/// and are dropped at intake, because producing their keysym requires the
/// interpreted key the host already computed. Winit delivers it
/// (`KeyEvent::logical_key`), but Smithay 0.7.0's winit wrapper reduces
/// the event to a raw scancode before the core sees it — a pinned-
/// dependency gap, not a protocol gap: full nested typing needs the
/// wrapper to surface winit's interpreted keysym (flagged for the M1.2
/// wiring; interpreting scancodes through a keymap in the core is the one
/// forbidden workaround). The subset below is chosen so the consent /
/// revocation paths never depend on the gap: Escape — P1.7.3's hold-Esc
/// chord — is layout-invariant and always translates.
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

#[cfg(test)]
mod tests {
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
                .handle_message(msg, scene, &mut |frame| core.send_message(frame, None))
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
}
