// SPDX-License-Identifier: MPL-2.0
//! **The VT escape** (WS-E.3.5, D-031): `Ctrl-Alt-F1` … `Ctrl-Alt-F12`, matched
//! on physical input only, consumed in every realm, and turned into the one
//! `Session::change_vt` call this workspace contains.
//!
//! # Why a compositor has to implement the chord it is not supposed to touch
//!
//! [D-030(1)] refused to call `change_vt`, reasoning that *"a display server
//! that traps the human on its own VT is a hostile display server"*. The
//! reasoning is right and the conclusion inverted the effect, because of one
//! fact about the kernel: **once a process holds DRM master and the VT is in
//! graphics mode, the kernel stops handling `Ctrl-Alt-F<n>`.** The chord is not
//! something a compositor declines to interfere with; it is something a
//! compositor either implements or abolishes.
//!
//! That was not deduced, it was observed. On 2026-08-09 the bare-metal backend
//! ran on hardware for the first time: `Ctrl-Alt-F1` and `Ctrl-Alt-F2` did
//! nothing, the repo owner could not leave his own session, and it ended only
//! when he killed it from another machine-local shell. The flight recorder for
//! that run contains 88 `seat_delivered` rows and **not one entry about the
//! chords he pressed** — nothing in this core could consume an F-key, so the
//! presses either went to the confined app (telling it the human was trying to
//! leave) or were swallowed by the lock cover, and in neither case did anything
//! act on them or write them down.
//!
//! # No principal can move the human's VT, and it is a compile error
//!
//! Four facts stack, and three of them predate this module:
//!
//! 1. **[`crate::input::SeatInput::physical`] is module-private.** A principal
//!    holding every verb in `Verb::VALID_MASK` cannot *construct* a
//!    physical-origin event. Not "is refused one" — cannot express one.
//! 2. **[`ChordMatcher`] refuses non-physical input at both halves**
//!    ([`crate::chord`]'s rule 1). The `gate` guard stops an agent's emulated
//!    `ctrl+alt+f2` matching; the `observe` guard stops the subtler attack the
//!    chord module names by hand — an agent holding a *synthetic* Ctrl and Alt
//!    so that the human's next ordinary `F2` throws them off their own screen.
//!    Both are pinned adversarially below.
//! 3. **The hook stack really does see emulated events**
//!    ([`crate::input::InputRouter::route_emulated`] funnels into the same
//!    `route_into`), so fact 2 is doing work rather than guarding an
//!    unreachable path.
//! 4. **[`VtRequest`] is the [`crate::screenshot::HumanGesture`] shape.** Its
//!    constructor is private to this module, its only producer is a
//!    physical-origin match in [`VtHook::gate`], and
//!    `DrmState::drain_vt_requests` — the only code in the workspace that names
//!    `change_vt` — takes one **by value**. So *"no principal can switch the
//!    human's VT"* is a compile error rather than the current absence of such a
//!    call.
//!
//! The one honest widening: a `physical-input-injector` build can mint
//! physical-origin keys, so there "only the human" degrades to "the human, or
//! whoever holds the inherited descriptor" — which `input::injector` already
//! publishes. It is moot here, because `main` refuses `--physical-input-fd`
//! outside `--headless` and this hook is stacked only on `--drm`.
//!
//! # Why it consumes [`crate::chord::ChordMatcher`] rather than growing a matcher
//!
//! [D-024] refused a second modifier matcher in this hook stack: *"a second
//! place to get the gate wrong, in the stack the human's off-switch lives in"*.
//! A hand-rolled VT matcher would be a second place to get **the origin guard**
//! wrong, in the one gesture that moves the human's whole attention off the
//! machine.
//!
//! # Where it is stacked: the first hook ever outside the lock gate
//!
//! ```text
//! InputRouter<VtHook<LockGate<ConsentGate<DeadManHook<ClipboardHook<ScreenshotHook<AttentionHook<NoopHook>>>>>>>>
//! ```
//!
//! * **Outside [`crate::lock::LockGate`]**, because the escape must work while
//!   locked (D-031(2)) and `LockGate::judge` consumes *every* physical press
//!   while the lock is up. Any inner position makes the chord dead exactly when
//!   the human most needs it.
//! * **Outside [`crate::consent::grab::ConsentGate`]**, because a consent grab
//!   seizes the whole input stream and a human who cannot answer a card must
//!   still be able to leave.
//! * **Outside [`crate::deadman::DeadManHook`]**, which costs the off-switch
//!   nothing: the dead-man detects in `observe`, which this hook forwards
//!   unconditionally, and `DeadManHook::gate` exists only to keep its own chord
//!   key from the app.
//! * **Never as a carve-out inside [`crate::lock::LockPolicy`]**, which would
//!   put a second chord matcher inside the one gate that swallows the whole
//!   device — the thing D-024 refuses by name.
//!
//! `LockGate`'s own docs say it tracks chord modifiers in `judge` and that this
//! is sound *"only because it is outermost, where `gate` sees exactly the events
//! `observe` does"*. That stays sound rather than merely unaffected: a
//! `ChordMatcher` **never consumes a modifier key**, so `LockGate::judge` still
//! sees every Ctrl/Alt/Shift/Super transition it saw before, and the only events
//! it can now be denied are F-key presses that matched a `ctrl+alt` chord —
//! which `main`'s startup refusal reserves against every other core chord on
//! this backend.
//!
//! # It is off the [`PreemptionHook`] trait, deliberately
//!
//! The three wiring accessors exist because the *kernel* must read a signal the
//! hook writes across `Runtime::new`. The VT signal has no such gap: `run_session`
//! builds it, clones it into [`VtHook`], and keeps the original as a `DrmState`
//! field. A fourth accessor would put a handle on "switch the human's VT" on a
//! trait `crate::session` — backend-agnostic, shared with nested and headless —
//! can reach. Keeping it off the trait keeps `change_vt` reachable from exactly
//! one file.
//!
//! [D-024]: ../../../docs/plan/20-decision-log.md
//! [D-030(1)]: ../../../docs/plan/20-decision-log.md

use std::cell::RefCell;
use std::rc::Rc;

use crate::chord::{ChordMatcher, ChordSpecError, ModChord, Trigger};
use crate::input::{Gate, KeySource, PreemptionHook, SeatInput, SeatInputKind};
use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

/// How many virtual terminals the escape binds: `F1` … `F12`.
///
/// Twelve because that is the kernel's own binding, and the reflex being
/// rescued is the kernel's. **Deliberately not configurable in this change**:
/// the whole defect is an escape hatch that was not there, and shipping a
/// `--vt-chord` or a `--no-vt-escape` alongside the fix is how it ends up off
/// or misspelled. The kiosk case is a separate issue.
pub(crate) const VT_COUNT: i32 = 12;

/// The virtual terminal a matched chord asks for.
///
/// A newtype rather than a bare integer, following [`crate::lock::LockGesture`]
/// and [`crate::screenshot::ScreenshotGesture`]: the matcher's payload should
/// name what a match *is*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetVt(i32);

impl TargetVt {
    pub fn get(self) -> i32 {
        self.0
    }
}

/// `XF86Switch_VT_1`, the first of the twelve.
const XF86_SWITCH_VT_1: u32 = 0x1008_FE01;
/// `XF86Switch_VT_12`.
const XF86_SWITCH_VT_12: u32 = 0x1008_FE0C;

/// The VT a resolved keysym asks for, or `None`.
///
/// **This is the path that actually fires on hardware, and the `ctrl+alt+fN`
/// matcher below is not.** Adversarial review caught this before a second
/// bring-up: once a `--keymap` is loaded, `CoreKeymap::resolve` applies the
/// live modifier state, and the standard xkb `"CTRL+ALT"` key type puts
/// `XF86Switch_VT_n` on level 5 of every F-key — the level Ctrl+Alt selects.
/// Verified against the very keymap the 2026-08-09 run used:
///
/// ```text
/// key <FK01> { type= "CTRL+ALT",
///              symbols[1]= [ F1, F1, F1, F1, XF86Switch_VT_1 ] };
/// ```
///
/// So `ctrl+alt+F1` never arrives as `XK_F1` (`0xffbe`) with modifiers — it
/// arrives as `XF86Switch_VT_1` (`0x1008fe01`) and matches no chord at all.
/// A matcher keyed on the F-row would have compiled, passed every test, and
/// left the human trapped a second time.
///
/// **No modifier check accompanies this, deliberately.** The keysym is not "the
/// F1 key"; it is the layout's own word for *switch to VT 1*, and a layout only
/// emits it when the human held the modifiers that mean it. Re-checking Ctrl
/// and Alt here would re-derive, from a shadow copy of the modifier state, a
/// fact the keymap has already settled — the drift-prone shape `set_view`'s
/// docs warn about.
fn vt_from_resolved_keysym(keysym: u32) -> Option<TargetVt> {
    (XF86_SWITCH_VT_1..=XF86_SWITCH_VT_12)
        .contains(&keysym)
        .then(|| TargetVt((keysym - XF86_SWITCH_VT_1 + 1) as i32))
}

/// **Proof that a human's physical chord fired**, and the only argument through
/// which `Session::change_vt` can be reached.
///
/// The field is private to this module and there is no `pub` constructor, no
/// `Copy` and no `Clone`, so the only way to obtain one is [`Self::of`] — a
/// private function whose only caller is [`VtSignal::request`], which only
/// [`VtHook::gate`] reaches, and only for a **physical-origin** chord match.
///
/// That chain is the module docs' fourth fact stated as a type. The shape is
/// [`crate::screenshot::HumanGesture`]'s, which is `spawn::SpawnRecord`'s
/// (#207) and `clipboard::PendingPromotion`'s (#213).
pub(crate) struct VtRequest {
    target: TargetVt,
    /// Private, unconstructible from outside: widening this to `pub` is the one
    /// edit that would undo everything the type says.
    _human: (),
}

impl VtRequest {
    fn of(target: TargetVt) -> Self {
        Self { target, _human: () }
    }

    pub fn target(&self) -> TargetVt {
        self.target
    }
}

/// The twelve trigger keys the escape reserves on bare metal.
///
/// `main`'s startup collision check reads this: the dead-man watcher detects in
/// the router's **unconditional** observe tap, so a core chord sharing a key
/// with the VT escape would arm the human's off-switch every time they left the
/// VT, and no hook ordering can prevent it.
pub(crate) fn reserved_triggers() -> Vec<Trigger> {
    (1..=VT_COUNT)
        .map(|n| {
            Trigger::parse(&format!("f{n}"))
                .expect("f1..f12 are in the chord vocabulary and in the invariant table")
        })
        .collect()
}

/// The twelve bindings, built **mechanically**: the `n` that spells the key and
/// the `n` that names the VT are the same `n`, so an off-by-one is
/// unconstructible rather than tested.
fn bindings() -> Result<Vec<(ModChord, TargetVt)>, ChordSpecError> {
    (1..=VT_COUNT)
        .map(|n| Ok((ModChord::parse(&format!("ctrl+alt+f{n}"))?, TargetVt(n))))
        .collect()
}

/// The chord queue the hook writes and the bare-metal backend drains.
///
/// The [`crate::screenshot::ScreenshotSignal`] shape and for its reason: the
/// hook runs inside `InputRouter::route_physical`, which holds no seat and must
/// not grow one, so the gate can only record *that the human asked to leave*.
/// Performing the switch is the embedder's.
pub(crate) struct VtSignal {
    matcher: ChordMatcher<TargetVt>,
    /// **An `Option`, not a `Vec`.** A human cannot chord twice in one dispatch
    /// turn, so a second request in one turn means something is wrong; the
    /// first is kept and the second is warned about and dropped. A queue would
    /// mean switching twice in a row for one gesture.
    pending: Option<VtRequest>,
    /// The **evdev scancode** whose press this signal consumed as an
    /// `XF86Switch_VT_n`, so its release can be consumed too.
    ///
    /// Keyed on the scancode rather than the keysym because that is what
    /// [`crate::input::KeySource::Device`] pairs by, and it is the only
    /// identity that survives the modifier state changing between press and
    /// release — which it always does here: the human lets go of Ctrl and Alt
    /// around the same instant, so the release very often resolves to plain
    /// `XK_F2` rather than `XF86Switch_VT_2`. Pairing by keysym would leak
    /// exactly that release to the app, which is the latch this project has
    /// already fixed twice (P1.7.2, and again for the consent grab).
    switch_held: Option<u32>,
}

impl VtSignal {
    pub fn new() -> Result<Self, ChordSpecError> {
        Ok(Self {
            matcher: ChordMatcher::new(bindings()?)?,
            pending: None,
            switch_held: None,
        })
    }

    /// Record a request. Private: the argument is a [`VtRequest`], and only
    /// [`VtHook::gate`] can produce the [`TargetVt`] one is minted from.
    fn request(&mut self, target: TargetVt) {
        if self.pending.is_some() {
            tracing::warn!(
                vt = target.get(),
                "a second VT switch was chorded in one dispatch turn; keeping the first. A \
                 human cannot press two chords in one turn, so this means something is feeding \
                 the router faster than it dispatches"
            );
            return;
        }
        self.pending = Some(VtRequest::of(target));
    }

    /// Judge a **resolved** `XF86Switch_VT_n`. `None` means "not mine, keep
    /// looking" — the `ctrl+alt+fN` matcher still runs after this, and is what
    /// serves a session started with no `--keymap`, where
    /// [`crate::input::invariant_keysym`] genuinely does answer `XK_F1..F12`
    /// and no layout is present to remap them.
    ///
    /// Physical origin only, checked here as well as in the matcher: an agent
    /// that could name this keysym must not move the human between VTs, and
    /// this path does not go through [`ChordMatcher`]'s own origin guard.
    fn judge_resolved(&mut self, input: &SeatInput) -> Option<Gate> {
        if input.origin() != Origin::Physical {
            return None;
        }
        let SeatInputKind::Key {
            source,
            keysym,
            state,
        } = input.kind()
        else {
            return None;
        };
        // Pair by scancode; see `switch_held`. A keysym-paired release would
        // resolve differently once Ctrl and Alt come up and would leak.
        let KeySource::Device(scancode) = source else {
            return None;
        };
        match state {
            KeyState::Pressed => {
                let target = vt_from_resolved_keysym(*keysym)?;
                self.switch_held = Some(*scancode);
                self.request(target);
                Some(Gate::Consume)
            }
            // Consumed **only** if we consumed its press. An F-key released
            // without our press is the app's, and swallowing it is the exact
            // latch the pairing contract exists to prevent.
            KeyState::Released if self.switch_held == Some(*scancode) => {
                self.switch_held = None;
                Some(Gate::Consume)
            }
            KeyState::Released => None,
        }
    }

    /// Take this turn's request, if the human made one.
    pub fn take_pending(&mut self) -> Option<VtRequest> {
        self.pending.take()
    }

    /// Forget every modifier bit and every consumed trigger, because physical
    /// input has ended without releases.
    ///
    /// See [`ChordMatcher::forget_physical_state`]: at the instant of a VT
    /// switch the human is holding **Ctrl and Alt by construction**, and
    /// libinput is suspended before either release can arrive.
    pub fn forget_physical_state(&mut self) {
        self.matcher.forget_physical_state();
        // ...and the resolved-keysym path's half of the same latch. At the
        // instant of a switch the human is holding the F-key too, and its
        // release arrives on the far side of a `libinput.suspend()` or never.
        // A stale `switch_held` would swallow the next unrelated release of
        // that same scancode.
        self.switch_held = None;
    }
}

/// The VT escape at the router's preemption hook point.
///
/// An ordinary [`PreemptionHook`] rather than a
/// [`crate::input::ConsumingGate`], because it genuinely needs `observe`: the
/// matcher tracks modifiers there, for [`crate::chord`]'s rule 2's reason. It
/// forwards `observe` unconditionally and forwards all three wiring accessors
/// verbatim, exactly as [`crate::screenshot::ScreenshotHook`] does.
pub(crate) struct VtHook<H: PreemptionHook> {
    signal: Rc<RefCell<VtSignal>>,
    inner: H,
}

impl<H: PreemptionHook> VtHook<H> {
    pub fn new(signal: Rc<RefCell<VtSignal>>, inner: H) -> Self {
        Self { signal, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for VtHook<H> {
    /// Modifier tracking only — unconditional, so a release an inner gate
    /// swallowed still clears its bit ([`crate::chord`]'s rule 2). It decides
    /// nothing and can stop nothing.
    fn observe(&mut self, input: &SeatInput) {
        self.signal.borrow_mut().matcher.observe(input);
        self.inner.observe(input);
    }

    /// A matched press is **consumed and never delivered to the app**, and so
    /// is its release — the one sound exception [`PreemptionHook`]'s pairing
    /// contract names, since the app never saw the press begin. Modifier keys
    /// are never consumed (the app needs its Ctrl), and an F-key that matched
    /// nothing — bare `f2`, `ctrl+f2`, `ctrl+shift+alt+f2` — is delivered
    /// untouched, so an app that binds F2 keeps it.
    fn gate(&mut self, input: &SeatInput) -> Gate {
        let verdict = {
            let mut signal = self.signal.borrow_mut();
            // The resolved-keysym path FIRST, because it is the one that fires
            // whenever a keymap is loaded — which is every bare-metal session
            // that can type. The matcher below serves the keymap-less session.
            if let Some(gate) = signal.judge_resolved(input) {
                gate
            } else {
                let (gate, target) = signal.matcher.gate(input);
                if let Some(target) = target {
                    signal.request(target);
                }
                gate
            }
        };
        match verdict {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }

    fn attention(&self) -> Option<Rc<RefCell<crate::attention::AttentionSignal>>> {
        self.inner.attention()
    }

    fn clipboard(&self) -> Option<Rc<RefCell<crate::clipboard::ClipboardSignal>>> {
        self.inner.clipboard()
    }

    fn screenshot(&self) -> Option<Rc<RefCell<crate::screenshot::ScreenshotSignal>>> {
        self.inner.screenshot()
    }

    fn backlight(&self) -> Option<Rc<RefCell<crate::backlight::BacklightSignal>>> {
        self.inner.backlight()
    }
}

/// This session's own virtual terminal, or `None` if it cannot be resolved.
///
/// Two sources, in order, and **no new privilege for either**: `XDG_VTNR`,
/// which logind sets for a seat session, then `/sys/class/tty/tty0/active`,
/// which is world-readable and names the foreground VT — at startup, before any
/// switch, that is ours.
///
/// `None` is a real answer and is handled as one: the chord still works and the
/// stall watchdog is disabled, because a watchdog that cannot tell "we are
/// already here" from "the switch never happened" would raise a false alarm on
/// a working session and teach the human to ignore the one indicator that
/// matters. Fail honest — a silent watchdog beats a lying one, and the escape
/// must never be gated on a diagnostic.
pub(crate) fn resolve_own_vt() -> Option<i32> {
    if let Some(vt) = std::env::var("XDG_VTNR")
        .ok()
        .and_then(|raw| raw.trim().parse::<i32>().ok())
        .filter(|vt| *vt > 0)
    {
        return Some(vt);
    }
    let active = std::fs::read_to_string("/sys/class/tty/tty0/active").ok()?;
    parse_active_tty(&active)
}

/// Parse `/sys/class/tty/tty0/active`'s `ttyN` into `N`.
///
/// A free function so it is testable without the file: the format is the
/// kernel's and a session that misparsed it would silently lose its watchdog.
fn parse_active_tty(raw: &str) -> Option<i32> {
    raw.trim()
        .strip_prefix("tty")
        .and_then(|n| n.parse::<i32>().ok())
        .filter(|vt| *vt > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::tests::physical_for_test;
    use crate::input::{KeySource, NoopHook, SeatInputKind};
    use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

    const CTRL_L: u32 = 0xffe3;
    const ALT_L: u32 = 0xffe9;
    const SHIFT_L: u32 = 0xffe1;

    fn physical(keysym: u32, state: KeyState) -> SeatInput {
        physical_for_test(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym,
            state,
        })
    }

    fn emulated(keysym: u32, state: KeyState) -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym,
            state,
        })
    }

    /// A physical key carrying a real **evdev scancode**, which is how the DRM
    /// backend's libinput path mints them and the only source that pairs a
    /// release to its press across a modifier change.
    fn physical_dev(scancode: u32, keysym: u32, state: KeyState) -> SeatInput {
        physical_for_test(SeatInputKind::Key {
            source: KeySource::Device(scancode),
            keysym,
            state,
        })
    }

    /// **The chord as it actually arrives on hardware.**
    ///
    /// This is the test the first bring-up needed and did not have. With a
    /// `--keymap` loaded, `ctrl+alt+F2` does **not** arrive as `XK_F2` with
    /// modifiers — the standard xkb `"CTRL+ALT"` key type puts
    /// `XF86Switch_VT_2` on the level Ctrl+Alt selects, so that is the keysym
    /// the router sees. Verified against the very keymap the 2026-08-09 run
    /// used: `symbols[1]= [ F1, F1, F1, F1, XF86Switch_VT_1 ]`.
    ///
    /// A matcher keyed on the F-row alone compiles, passes every other test in
    /// this file, and leaves the human trapped — which is exactly what the
    /// first version of this module did.
    #[test]
    fn the_resolved_switch_keysym_is_what_fires_on_hardware() {
        for n in 1..=VT_COUNT {
            let (signal, mut hook) = bare_hook();
            let scancode = 100 + n as u32;
            let sym = XF86_SWITCH_VT_1 + (n as u32 - 1);

            assert_eq!(
                feed(&mut hook, &physical_dev(scancode, sym, KeyState::Pressed)),
                Gate::Consume,
                "XF86Switch_VT_{n} must be consumed, never handed to the app: it is the \
                 layout's own word for 'the human is leaving'"
            );
            let req = signal
                .borrow_mut()
                .take_pending()
                .unwrap_or_else(|| panic!("XF86Switch_VT_{n} must request a switch"));
            assert_eq!(req.target().get(), n);

            // The release arrives as PLAIN F-key, because the human let go of
            // Ctrl and Alt first. Pairing by keysym would miss it and leak a
            // release whose press the app never saw -- the P1.7.2 latch.
            assert_eq!(
                feed(
                    &mut hook,
                    &physical_dev(scancode, fkey(n), KeyState::Released)
                ),
                Gate::Consume,
                "the release must pair by SCANCODE: by the time it arrives the modifiers are \
                 up and the keysym has resolved back to F{n}"
            );
        }
    }

    /// An agent naming the switch keysym directly moves nobody.
    #[test]
    fn an_emulated_switch_keysym_moves_no_human() {
        let (signal, mut hook) = bare_hook();
        let emulated_switch = SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Device(101),
            keysym: XF86_SWITCH_VT_1 + 1,
            state: KeyState::Pressed,
        });
        assert_eq!(feed(&mut hook, &emulated_switch), Gate::Deliver);
        assert!(
            signal.borrow_mut().take_pending().is_none(),
            "an agent that names XF86Switch_VT_2 must not move the human's screen: this path \
             does not go through ChordMatcher's origin guard and needs its own"
        );
    }

    /// A hook that counts what reaches it, so the forwarding above it is
    /// observable rather than assumed.
    #[derive(Default)]
    struct CountingHook {
        observed: usize,
        gated: usize,
    }

    impl PreemptionHook for CountingHook {
        fn observe(&mut self, _input: &SeatInput) {
            self.observed += 1;
        }
        fn gate(&mut self, _input: &SeatInput) -> Gate {
            self.gated += 1;
            Gate::Deliver
        }
        fn attention(&self) -> Option<Rc<RefCell<crate::attention::AttentionSignal>>> {
            None
        }
        fn clipboard(&self) -> Option<Rc<RefCell<crate::clipboard::ClipboardSignal>>> {
            None
        }
        fn screenshot(&self) -> Option<Rc<RefCell<crate::screenshot::ScreenshotSignal>>> {
            None
        }

        fn backlight(&self) -> Option<Rc<RefCell<crate::backlight::BacklightSignal>>> {
            None
        }
    }

    /// **`VtHook` is now the OUTERMOST hook, so its `observe` is the sole route
    /// by which every inner hook sees physical input at all.**
    ///
    /// `DeadManHook` detects the off-switch in `observe`. `ClipboardHook`,
    /// `ScreenshotHook` and `AttentionHook` track their modifiers there. Delete
    /// one line — `self.inner.observe(input)` — and every one of them goes
    /// blind, including the human's emergency stop, while this module's own
    /// tests stay green because they all use `NoopHook`.
    ///
    /// Found by review, not by the suite: nothing pinned the forward.
    #[test]
    fn the_outermost_hook_forwards_every_event_inward() {
        let signal = Rc::new(RefCell::new(VtSignal::new().expect("twelve bindings")));
        let mut hook = VtHook::new(signal, CountingHook::default());

        // A plain key that matches nothing: it must reach the inner stack in
        // both halves.
        feed(&mut hook, &physical_dev(30, 0x0061, KeyState::Pressed));
        assert_eq!(
            (hook.inner.observed, hook.inner.gated),
            (1, 1),
            "an ordinary key must reach the hooks beneath: the dead-man switch detects in \
             observe, and it is now behind this hook"
        );

        // Modifiers, which the VT chord must never consume, also pass through.
        feed(&mut hook, &physical(CTRL_L, KeyState::Pressed));
        feed(&mut hook, &physical(ALT_L, KeyState::Pressed));
        assert_eq!(
            hook.inner.observed, 3,
            "every modifier transition must still reach DeadManHook and the clipboard and \
             screenshot matchers, or their chords silently stop matching"
        );

        // A CONSUMED chord still forwards `observe` -- rule 2: a release an
        // outer gate swallowed must still clear the inner matchers' bits.
        let before = hook.inner.observed;
        feed(
            &mut hook,
            &physical_dev(120, XF86_SWITCH_VT_1 + 1, KeyState::Pressed),
        );
        assert_eq!(
            hook.inner.observed,
            before + 1,
            "observe must forward even when gate consumes: an inner matcher that never sees \
             the press has no way to know its modifier bits are stale"
        );
    }

    /// A release we did not consume the press of is the app's.
    #[test]
    fn an_unmatched_release_is_never_swallowed() {
        let (_signal, mut hook) = bare_hook();
        assert_eq!(
            feed(&mut hook, &physical_dev(120, fkey(4), KeyState::Released)),
            Gate::Deliver,
            "swallowing a release whose press the app DID see is the latch this contract \
             exists to prevent"
        );
    }

    /// The keysym `f{n}` produces, straight off the layout-invariant table so
    /// this fixture cannot be the thing under test.
    fn fkey(n: i32) -> u32 {
        Trigger::parse(&format!("f{n}"))
            .expect("f1..f12 parse")
            .keysym()
    }

    /// Drive one event through a bare hook (observe then gate, the router's
    /// order) and report the verdict.
    fn feed<H: PreemptionHook>(hook: &mut H, input: &SeatInput) -> Gate {
        hook.observe(input);
        hook.gate(input)
    }

    fn bare_hook() -> (Rc<RefCell<VtSignal>>, VtHook<NoopHook>) {
        let signal = Rc::new(RefCell::new(VtSignal::new().expect("twelve bindings")));
        (Rc::clone(&signal), VtHook::new(signal, NoopHook))
    }

    /// **All twelve chords fire, each on its own VT** — press consumed, release
    /// consumed, payload equal to the number in the key's name.
    ///
    /// An off-by-one in the mapping, or a dropped binding, fails on a named
    /// `n` rather than somewhere downstream.
    #[test]
    fn ctrl_alt_f1_through_f12_each_request_its_own_vt() {
        for n in 1..=VT_COUNT {
            let (signal, mut hook) = bare_hook();
            assert_eq!(
                feed(&mut hook, &physical(CTRL_L, KeyState::Pressed)),
                Gate::Deliver
            );
            assert_eq!(
                feed(&mut hook, &physical(ALT_L, KeyState::Pressed)),
                Gate::Deliver
            );
            assert_eq!(
                feed(&mut hook, &physical(fkey(n), KeyState::Pressed)),
                Gate::Consume,
                "ctrl+alt+f{n}'s press must never reach the confined app"
            );
            let request = signal
                .borrow_mut()
                .take_pending()
                .unwrap_or_else(|| panic!("ctrl+alt+f{n} must request a switch"));
            assert_eq!(request.target(), TargetVt(n));
            assert_eq!(
                feed(&mut hook, &physical(fkey(n), KeyState::Released)),
                Gate::Consume,
                "the release of a consumed press must be consumed too, or the app is left \
                 holding a key it never saw pressed"
            );
            assert!(signal.borrow_mut().take_pending().is_none());
        }
    }

    /// **An F-key that matched nothing is delivered untouched**, press and
    /// release. Pins the exact-set-equality rule: a superset match would make
    /// `ctrl+shift+alt+f2` a VT switch and cost the app four bindings it owns.
    #[test]
    fn an_unmatched_f_key_is_delivered_untouched() {
        let f2 = fkey(2);
        for held in [
            Vec::new(),
            vec![CTRL_L],
            vec![ALT_L],
            vec![CTRL_L, SHIFT_L, ALT_L],
        ] {
            let (signal, mut hook) = bare_hook();
            for key in &held {
                feed(&mut hook, &physical(*key, KeyState::Pressed));
            }
            assert_eq!(
                feed(&mut hook, &physical(f2, KeyState::Pressed)),
                Gate::Deliver,
                "held: {held:x?}"
            );
            assert_eq!(
                feed(&mut hook, &physical(f2, KeyState::Released)),
                Gate::Deliver,
                "held: {held:x?}"
            );
            assert!(
                signal.borrow_mut().take_pending().is_none(),
                "held: {held:x?}"
            );
        }
    }

    /// **The VT chord never consumes a modifier.**
    ///
    /// A swallowed Ctrl release leaves the confined app holding Ctrl and
    /// silently rewrites everything typed afterwards — the exact bug P1.7.2
    /// shipped once. It is also what keeps `LockGate`'s "I see every modifier
    /// transition" sound now that a hook sits outside it.
    #[test]
    fn the_vt_chord_never_consumes_a_modifier() {
        let (_signal, mut hook) = bare_hook();
        for keysym in [CTRL_L, ALT_L, SHIFT_L] {
            assert_eq!(
                feed(&mut hook, &physical(keysym, KeyState::Pressed)),
                Gate::Deliver
            );
            assert_eq!(
                feed(&mut hook, &physical(keysym, KeyState::Released)),
                Gate::Deliver
            );
        }
    }

    /// **An agent's emulated F-key requests nothing, even when the human
    /// really is holding Ctrl and Alt.**
    ///
    /// The modifiers are pressed **physically** on purpose, because that is
    /// what makes this test about the `gate` guard rather than about the
    /// `observe` one. A human holding ctrl+alt is not a contrived state: it is
    /// the state they are in for the whole of every VT chord they make, and it
    /// is the window in which an agent's `actuate.key` would otherwise
    /// complete a gesture the human began.
    ///
    /// Non-vacuous under a one-line mutation: delete
    /// `if input.origin() != Origin::Physical { return (Gate::Deliver, None); }`
    /// from `ChordMatcher::gate` and this goes red while
    /// `an_agent_holding_a_synthetic_ctrl_alt_cannot_arm_the_humans_next_f_key`
    /// stays green — which is exactly why both exist.
    #[test]
    fn an_agents_emulated_f_key_never_requests_a_vt_switch() {
        let (signal, mut hook) = bare_hook();
        // The human's own hands, on the modifiers.
        feed(&mut hook, &physical(CTRL_L, KeyState::Pressed));
        feed(&mut hook, &physical(ALT_L, KeyState::Pressed));
        assert_eq!(
            feed(&mut hook, &emulated(fkey(2), KeyState::Pressed)),
            Gate::Deliver,
            "an admitted actuation is the chokepoint's business, not this gate's"
        );
        assert!(
            signal.borrow_mut().take_pending().is_none(),
            "a principal holding actuate_key must not be able to move the human's VT, not even \
             by completing a chord the human's hands had already half-made"
        );

        // ...and the wholly-emulated chord likewise, which is the case both
        // guards close together.
        let (all_emulated_signal, mut all_emulated_hook) = bare_hook();
        for keysym in [CTRL_L, ALT_L, fkey(2)] {
            assert_eq!(
                feed(&mut all_emulated_hook, &emulated(keysym, KeyState::Pressed)),
                Gate::Deliver
            );
        }
        assert!(all_emulated_signal.borrow_mut().take_pending().is_none());
    }

    /// **An agent holding a synthetic Ctrl+Alt cannot arm the human's next
    /// F-key** — the subtler half, and the one `crate::chord` names by hand.
    ///
    /// Without the `observe` guard an agent could hold emulated modifiers and
    /// wait for the human to press a bare F2 for any ordinary reason, and the
    /// human would be thrown off their own screen by a gesture they did not
    /// make.
    ///
    /// Non-vacuous under a different one-line mutation from the test above:
    /// delete the origin guard from `ChordMatcher::observe` and this goes red
    /// while that one stays green, which is exactly why both exist.
    #[test]
    fn an_agent_holding_a_synthetic_ctrl_alt_cannot_arm_the_humans_next_f_key() {
        let (signal, mut hook) = bare_hook();
        feed(&mut hook, &emulated(CTRL_L, KeyState::Pressed));
        feed(&mut hook, &emulated(ALT_L, KeyState::Pressed));
        assert_eq!(
            feed(&mut hook, &physical(fkey(2), KeyState::Pressed)),
            Gate::Deliver,
            "the human pressed a bare F2; an agent's synthetic modifiers must not make it a \
             chord"
        );
        assert!(signal.borrow_mut().take_pending().is_none());
    }

    /// **A second chord in one dispatch turn keeps the first.**
    #[test]
    fn a_second_request_in_one_turn_is_dropped_rather_than_queued() {
        let (signal, mut hook) = bare_hook();
        feed(&mut hook, &physical(CTRL_L, KeyState::Pressed));
        feed(&mut hook, &physical(ALT_L, KeyState::Pressed));
        feed(&mut hook, &physical(fkey(2), KeyState::Pressed));
        feed(&mut hook, &physical(fkey(2), KeyState::Released));
        feed(&mut hook, &physical(fkey(5), KeyState::Pressed));
        let mut signal = signal.borrow_mut();
        assert_eq!(
            signal.take_pending().expect("the first request").target(),
            TargetVt(2)
        );
        assert!(signal.take_pending().is_none());
    }

    /// **A seat pause forgets the modifier bits and the consumed set.**
    ///
    /// At the instant of a VT switch the human is holding Ctrl and Alt by
    /// construction, and libinput is suspended before either release arrives.
    /// Without this the human's next **bare F5** — refreshing something, back
    /// on this VT — matches `ctrl+alt+f5` and throws them onto VT 5.
    #[test]
    fn a_paused_seat_leaves_no_stale_modifier_or_consumed_key() {
        let (signal, mut hook) = bare_hook();
        feed(&mut hook, &physical(CTRL_L, KeyState::Pressed));
        feed(&mut hook, &physical(ALT_L, KeyState::Pressed));
        assert_eq!(
            feed(&mut hook, &physical(fkey(5), KeyState::Pressed)),
            Gate::Consume
        );
        assert!(signal.borrow_mut().take_pending().is_some());

        // The seat leaves. No release for Ctrl, Alt or F5 will ever arrive.
        signal.borrow_mut().forget_physical_state();

        assert_eq!(
            feed(&mut hook, &physical(fkey(5), KeyState::Pressed)),
            Gate::Deliver,
            "a stale ctrl+alt would turn the human's next bare F5 into a VT switch"
        );
        assert_eq!(
            feed(&mut hook, &physical(fkey(5), KeyState::Released)),
            Gate::Deliver,
            "a stale `consumed` entry eats a release whose press was delivered, which latches \
             the key down in the confined app"
        );
        assert!(signal.borrow_mut().take_pending().is_none());
    }

    /// **The twelve chords parse, and their triggers are the invariant table's
    /// F-keys.** A rename in `Trigger::VOCABULARY` becomes a red test rather
    /// than a silently missing escape key.
    #[test]
    fn every_vt_chord_parses_against_the_layout_invariant_table() {
        // The literal, not `VT_COUNT`: every other assertion here derives its
        // range from that constant, so shrinking it would silently shrink the
        // whole test with it and the escape would quietly lose keys.
        assert_eq!(
            VT_COUNT, 12,
            "the kernel binds twelve VT chords and the reflex being rescued is the kernel's"
        );
        let built = bindings().expect("the twelve bindings are not duplicates");
        assert_eq!(built.len(), VT_COUNT as usize);
        for (n, (chord, target)) in (1..=VT_COUNT).zip(built) {
            assert_eq!(target, TargetVt(n));
            assert_eq!(chord.spelling(), format!("ctrl+alt+f{n}"));
            // KEY_F1..F10 are contiguous at 59; F11/F12 are not.
            let expected = match n {
                1..=10 => crate::input::invariant_keysym(59 + (n as u32 - 1)),
                11 => Some(0xffc8),
                _ => Some(0xffc9),
            };
            assert_eq!(Some(chord.trigger_keysym()), expected, "f{n}");
        }
        assert_eq!(reserved_triggers().len(), VT_COUNT as usize);
    }

    /// **THE property this whole change turns on: the escape fires through a
    /// raised lock, and the lock is still up afterwards** (D-031(2)).
    ///
    /// Driven through the **real** production stack —
    /// `VtHook<LockGate<ConsentGate<DeadManHook<ClipboardHook<ScreenshotHook<
    /// AttentionHook<NoopHook>>>>>>>`, which is [`crate::backend::drm::DrmHook`]
    /// built — not a stub and not the hooks in isolation. It is the mirror of
    /// `lock::gate`'s `the_dead_man_chord_arms_and_fires_through_a_locked_gate`
    /// and it is written as adversarially: the lock is raised **before** the
    /// chord is touched, and a control asserts the lock really is consuming
    /// everything, so the positive below cannot pass because nothing is being
    /// consumed at all.
    ///
    /// Both halves matter and neither is worth much alone. A human whose
    /// passphrase will not go in, on a lock they cannot dismiss, on a VT they
    /// cannot leave, is the observed defect one state deeper with no exit at
    /// all — so the escape must work. And a VT switch must never be a way past
    /// a lock, so the last assertion is that it is not.
    ///
    /// Non-vacuous: moving `VtHook` inside `LockGate` turns this red — in fact
    /// it stops the crate compiling, via
    /// `backend::drm`'s `the_vt_chord_is_the_only_hook_outside_the_lock_gate`.
    /// Belt and braces on the property that matters most.
    #[test]
    fn the_vt_chord_fires_through_a_raised_lock() {
        use std::cell::Cell;
        use std::time::Instant;

        use crate::consent::grab::{ConsentGate, ConsentGrab};
        use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch};
        use crate::input::InputRouter;
        use crate::lock::{lock_gate, LockCause, LockScreen};

        let t0 = Instant::now();
        let now = Rc::new(Cell::new(t0));
        let lock_chord = ModChord::parse("ctrl+alt+delete").expect("the default lock chord");
        let screen = Rc::new(RefCell::new(LockScreen::new(
            lock_chord,
            None,
            None,
            Rc::new(RefCell::new(crate::backend::blank::SessionActivity::new(
                None, t0,
            ))),
        )));
        let signal = Rc::new(RefCell::new(VtSignal::new().expect("twelve bindings")));

        let mut router = InputRouter::detached(VtHook::new(
            Rc::clone(&signal),
            lock_gate(
                Rc::clone(&screen),
                Rc::clone(&now),
                ConsentGate::new(
                    Rc::new(RefCell::new(ConsentGrab::new())),
                    Rc::clone(&now),
                    DeadManHook::new(
                        Rc::new(RefCell::new(DeadManSwitch::new(DeadManConfig::default()))),
                        Rc::clone(&now),
                        crate::clipboard::ClipboardHook::new(
                            Rc::new(RefCell::new(crate::clipboard::ClipboardSignal::detached())),
                            crate::screenshot::ScreenshotHook::new(
                                Rc::new(RefCell::new(
                                    crate::screenshot::ScreenshotSignal::detached(),
                                )),
                                crate::attention::AttentionHook::new(
                                    Rc::new(RefCell::new(
                                        crate::attention::AttentionSignal::detached(),
                                    )),
                                    Rc::clone(&now),
                                    NoopHook,
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        ));
        let realm = crate::grants::RealmId::new("realm-0");
        assert!(router.bind_to(&realm).is_none());
        let view: crate::view::ViewGeometry = (640u32, 480u32).into();

        // The session locks. From here the lock consumes every physical event.
        screen.borrow_mut().raise(LockCause::Chord);
        assert!(screen.borrow().is_locked());

        // The control FIRST: an ordinary key reaches no app, so the
        // assertions below cannot pass vacuously.
        assert!(
            router
                .route_physical(physical(0x61, KeyState::Pressed), view, Some(view.usable()))
                .is_none(),
            "the lock must be consuming physical input for this test to mean anything"
        );

        // The human chords their way out.
        for keysym in [CTRL_L, ALT_L] {
            router.route_physical(
                physical(keysym, KeyState::Pressed),
                view,
                Some(view.usable()),
            );
        }
        assert!(
            router
                .route_physical(
                    physical(fkey(2), KeyState::Pressed),
                    view,
                    Some(view.usable())
                )
                .is_none(),
            "the chord's press must not reach the app"
        );
        let request = signal.borrow_mut().take_pending().expect(
            "THE property: a locked gate must not be able to swallow the way OFF this \
                     VT. A human whose passphrase will not go in, on a lock they cannot \
                     dismiss, on a VT they cannot leave, has no exit at all",
        );
        assert_eq!(request.target(), TargetVt(2));
        assert!(router
            .route_physical(
                physical(fkey(2), KeyState::Released),
                view,
                Some(view.usable())
            )
            .is_none());

        // ...and the lock is still up. A VT switch is never a way past it.
        assert!(
            screen.borrow().is_locked(),
            "leaving the VT must not lower the lock: the passphrase is still owed on return"
        );
    }

    /// **The session's own VT is parsed, not guessed.**
    #[test]
    fn the_active_tty_is_parsed_and_nonsense_is_refused() {
        assert_eq!(parse_active_tty("tty3\n"), Some(3));
        assert_eq!(parse_active_tty("tty12"), Some(12));
        assert_eq!(
            parse_active_tty("tty0"),
            None,
            "tty0 is the alias, not a VT"
        );
        assert_eq!(parse_active_tty("ttyS0"), None);
        assert_eq!(parse_active_tty(""), None);
        assert_eq!(parse_active_tty("3"), None);
    }
}
