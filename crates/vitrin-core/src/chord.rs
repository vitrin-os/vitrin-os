// SPDX-License-Identifier: MPL-2.0
//! A **general modifier-aware chord matcher** over the keysyms
//! [`crate::input`] already delivers (WS-E.2.1, issue #213).
//!
//! # Why this exists as its own module, and why it is not clipboard code
//!
//! The core owned two chord vocabularies before this and neither can express a
//! modifier chord:
//!
//! - [`crate::deadman::Chord`] **excludes every modifier**, and that is a
//!   correctness rule rather than an omission — a human holds Shift for seconds
//!   at a time to select text, so a modifier off-switch would fire during
//!   ordinary use.
//! - [`crate::attention::AttentionChord`] is nothing *but* modifiers — the two
//!   Supers — and it matches a bare key with no notion of what else is held.
//!
//! Issue #213 needs `ctrl+shift+insert` and `shift+insert`, which is neither
//! shape. Issue #214's lock chord and issue #216's screenshot key both want the
//! same mechanism, and [D-024](../../../docs/plan/20-decision-log.md) makes
//! this module the one they consume: a second modifier matcher in the same hook
//! stack is a second place to get the gate wrong, in the stack the human's
//! off-switch lives in.
//!
//! So nothing here mentions the clipboard. This module knows about keysyms,
//! modifier state and bindings; what a match *means* is the caller's.
//!
//! # The three rules that make it safe to consume
//!
//! **1. Physical origin only, at both halves.** An emulated key carrying a
//! chord's keysym is an actuation the enforcement chokepoint already admitted
//! against a live grant. Matching it would let any principal holding
//! `actuate_text` manufacture the human's gesture; *tracking modifiers from it*
//! would be subtler and worse — an agent could hold a synthetic Ctrl and turn
//! the human's next ordinary Insert into a chord. Both are closed by the same
//! `if`, and both are pinned adversarially below.
//!
//! **2. Modifier state is tracked in [`PreemptionHook::observe`], the match is
//! made in [`PreemptionHook::gate`].** `observe` is unconditional — the consent
//! grab forwards it even for events it swallows — so a Shift release that a
//! prompt ate still clears the bit, and the matcher cannot be left believing a
//! modifier is held forever. Putting the *match* in `gate` is what makes a
//! chord suppressible by a consent prompt and by the dead-man switch, which is
//! the correct posture for anything whose failure mode is "a convenience did
//! not happen". Same split, and the same reason, as [`crate::deadman`].
//!
//! **3. Modifier sets match EXACTLY, so at most one binding can fire.** A
//! matcher holding both `shift+insert` and `ctrl+shift+insert` answers *one* of
//! them for any press, never both and never the wrong one, because
//! `ctrl+shift+insert` is not a superset match of `shift+insert` — it is a
//! different set. That is what makes "one gesture does one thing" a property of
//! the type rather than of the order two `if`s were written in, and
//! [`ChordMatcher::new`] refuses two bindings on the same chord outright so the
//! ambiguity is unconstructible as well as unreachable.
//!
//! # Consumption and the pairing contract
//!
//! A matched press is consumed, and its **release is consumed too** — recorded
//! per keysym, so the release is consumed whatever the modifiers are doing by
//! then (a human releases Shift before Insert about as often as not). That is
//! exactly the one sound exception [`PreemptionHook`]'s pairing contract names:
//! the app never saw the press begin, so it is not left holding anything. A
//! trigger press that matched *nothing* is delivered untouched, and so is its
//! release — an app that binds Insert keeps it.
//!
//! Modifier keys themselves are **never** consumed. The app needs its Shift.

use std::collections::BTreeSet;

use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

use crate::input::{Gate, SeatInput, SeatInputKind};

/// One modifier, as a bit in [`Modifiers`].
///
/// Four, not five: `Caps_Lock` is in [`crate::input::invariant_keysym`] and is
/// deliberately absent here, because it is a *lock* rather than a held
/// modifier. A matcher that tracked it would need the lock's state, which the
/// core does not have and refuses to grow a keymap to learn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

impl Modifier {
    /// name → (bit, left keysym, right keysym, left evdev scancode).
    ///
    /// Both sides of each pair map to one bit: a chord written `ctrl+…` means
    /// *a* Control, never a particular one, because a human who reaches for the
    /// right-hand Control has not asked for a different gesture.
    ///
    /// The scancode is the **left** key's, and it has exactly one consumer —
    /// the `physical-input-injector` channel, which has to press a real key
    /// through the production intake rather than mint a keysym. Which side a
    /// harness presses is arbitrary precisely because the two are one modifier.
    const TABLE: &'static [(&'static str, Modifier, u32, u32, u32)] = &[
        ("ctrl", Modifier::Ctrl, 0xffe3, 0xffe4, 29),
        ("shift", Modifier::Shift, 0xffe1, 0xffe2, 42),
        ("alt", Modifier::Alt, 0xffe9, 0xffea, 56),
        ("super", Modifier::Super, 0xffeb, 0xffec, 125),
    ];

    fn bit(self) -> u8 {
        match self {
            Modifier::Ctrl => 1,
            Modifier::Shift => 2,
            Modifier::Alt => 4,
            Modifier::Super => 8,
        }
    }

    /// Which modifier, if any, this keysym is.
    fn of_keysym(keysym: u32) -> Option<Modifier> {
        Self::TABLE
            .iter()
            .find(|(_, _, left, right, _)| *left == keysym || *right == keysym)
            .map(|(_, modifier, _, _, _)| *modifier)
    }

    fn parse(name: &str) -> Option<Modifier> {
        Self::TABLE
            .iter()
            .find(|(candidate, _, _, _, _)| *candidate == name)
            .map(|(_, modifier, _, _, _)| *modifier)
    }

    /// The left key's evdev scancode — the `physical-input-injector`
    /// channel's only need (see [`Self::TABLE`]).
    #[cfg_attr(not(any(test, feature = "physical-input-injector")), allow(dead_code))]
    pub fn evdev(self) -> u32 {
        Self::TABLE
            .iter()
            .find(|(_, candidate, _, _, _)| *candidate == self)
            .map(|(_, _, _, _, evdev)| *evdev)
            .expect("every modifier is in the table")
    }
}

/// The set of modifiers held right now, or required by a chord.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Modifiers(u8);

impl Modifiers {
    fn with(self, modifier: Modifier) -> Self {
        Modifiers(self.0 | modifier.bit())
    }

    fn without(self, modifier: Modifier) -> Self {
        Modifiers(self.0 & !modifier.bit())
    }

    fn contains(self, modifier: Modifier) -> bool {
        self.0 & modifier.bit() != 0
    }

    /// Whether this set is empty — a chord with no modifiers at all, which
    /// [`ModChord::parse`] refuses so that this module never becomes a second
    /// way to spell a bare-key chord.
    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A trigger key: the non-modifier half of a chord.
///
/// The vocabulary is every layout-invariant, non-modifier key
/// [`crate::input::invariant_keysym`] can produce from a scancode alone. It is
/// deliberately **wider than issue #213 needs**, because #214 and #216 consume
/// this module and a vocabulary grown one key per issue is a vocabulary nobody
/// audits. Letters and digits are absent for the reason the whole core has no
/// keymap: `invariant_keysym` returns `None` for them, so on a backend with no
/// host compositor to interpret them (WS-E Stage 3) they do not exist.
///
/// `print` is the one row added after the fact (WS-E.2.4, issue #216), and it
/// was added to [`crate::input::invariant_keysym`] **first**: this table is a
/// selection from that one, never a source of new keys, so a name here that
/// intake cannot deliver is refused by [`Self::parse`] rather than shipped as a
/// dead gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Trigger {
    name: &'static str,
    evdev: u32,
    keysym: u32,
}

impl Trigger {
    /// name → (evdev scancode, keysym).
    ///
    /// The scancode is carried for the same single consumer the modifiers' is:
    /// the `physical-input-injector` channel presses the *configured* gesture
    /// through the production intake rather than a number a harness guessed.
    /// That the two columns agree is checked against
    /// [`crate::input::invariant_keysym`] in [`Self::parse`] rather than
    /// trusted, so this table can never drift into naming a scancode that
    /// produces a different key.
    /// `(name, evdev scancode, keysym)` for every trigger a core-owned chord
    /// may use.
    ///
    /// `pub(crate)` since WS-E.3.1 so [`crate::input::keymap::CoreKeymap`] can
    /// refuse a keymap that does not agree with it. A layout that moves any of
    /// these keys silently disarms the gesture bound to it — and one of those
    /// gestures is the human's off-switch.
    pub(crate) const VOCABULARY: &'static [(&'static str, u32, u32)] = &[
        ("esc", 1, 0xff1b),
        ("tab", 15, 0xff09),
        ("backspace", 14, 0xff08),
        ("enter", 28, 0xff0d),
        ("space", 57, 0x0020),
        ("f1", 59, 0xffbe),
        ("f2", 60, 0xffbf),
        ("f3", 61, 0xffc0),
        ("f4", 62, 0xffc1),
        ("f5", 63, 0xffc2),
        ("f6", 64, 0xffc3),
        ("f7", 65, 0xffc4),
        ("f8", 66, 0xffc5),
        ("f9", 67, 0xffc6),
        ("f10", 68, 0xffc7),
        ("f11", 87, 0xffc8),
        ("f12", 88, 0xffc9),
        ("insert", 110, 0xff63),
        ("delete", 111, 0xffff),
        ("print", 99, 0xff61),
        ("home", 102, 0xff50),
        ("end", 107, 0xff57),
        ("pageup", 104, 0xff55),
        ("pagedown", 109, 0xff56),
        ("up", 103, 0xff52),
        ("down", 108, 0xff54),
        ("left", 105, 0xff51),
        ("right", 106, 0xff53),
    ];

    /// Resolve a trigger-key name.
    ///
    /// Refuses anything the vocabulary does not name and — defence in depth
    /// against a vocabulary entry drifting from the intake table — anything
    /// intake could not deliver ([`crate::input::keysym_is_intakeable`]).
    /// `deadman::Chord::parse`'s discipline, for its reason: a session that
    /// comes up with a gesture that can never fire is a fail-open trap, however
    /// mild the gesture.
    pub fn parse(name: &str) -> Result<Self, ChordSpecError> {
        let (name, evdev, keysym) = Self::VOCABULARY
            .iter()
            .find(|(candidate, _, _)| *candidate == name)
            .copied()
            .ok_or(ChordSpecError::UnknownKey)?;
        if !crate::input::keysym_is_intakeable(keysym) {
            return Err(ChordSpecError::NotIntakeable);
        }
        if crate::input::invariant_keysym(evdev) != Some(keysym) {
            return Err(ChordSpecError::NotIntakeable);
        }
        Ok(Self {
            name,
            evdev,
            keysym,
        })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn keysym(&self) -> u32 {
        self.keysym
    }

    /// The evdev scancode a device produces for this key — the
    /// `physical-input-injector` channel's only need.
    #[cfg_attr(not(any(test, feature = "physical-input-injector")), allow(dead_code))]
    pub fn evdev(&self) -> u32 {
        self.evdev
    }

    /// Every trigger name this build accepts, for a CLI's error message.
    pub fn vocabulary() -> impl Iterator<Item = &'static str> {
        Self::VOCABULARY.iter().map(|(name, _, _)| *name)
    }
}

/// One modifier chord: a required modifier set plus a trigger key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModChord {
    mods: Modifiers,
    trigger: Trigger,
}

impl ModChord {
    /// Build a chord from a modifier list and a trigger.
    ///
    /// `mods` may not be empty: a bare-key "chord" belongs to
    /// [`crate::deadman::Chord`] or [`crate::attention::AttentionChord`], each
    /// of which has its own reasons for its own vocabulary, and a third spelling
    /// of the same thing is how two mechanisms end up disagreeing about which
    /// keys the core owns.
    pub fn new(mods: &[Modifier], trigger: Trigger) -> Result<Self, ChordSpecError> {
        let mut set = Modifiers::default();
        for modifier in mods {
            if set.contains(*modifier) {
                return Err(ChordSpecError::RepeatedModifier);
            }
            set = set.with(*modifier);
        }
        if set.is_empty() {
            return Err(ChordSpecError::NoModifier);
        }
        Ok(Self { mods: set, trigger })
    }

    /// Parse `"ctrl+shift+insert"`: one or more modifier names, then exactly one
    /// trigger, `+`-separated, lowercase, no spaces.
    ///
    /// Deliberately strict. A tolerant parser here would accept two spellings of
    /// one gesture and an operator's config file would stop being a record of
    /// what the core owns.
    pub fn parse(spec: &str) -> Result<Self, ChordSpecError> {
        let mut parts = spec.split('+');
        let mut mods = Vec::new();
        let last = loop {
            let part = parts.next().ok_or(ChordSpecError::UnknownKey)?;
            match Modifier::parse(part) {
                Some(modifier) => mods.push(modifier),
                None => break part,
            }
        };
        if parts.next().is_some() {
            // A modifier after the trigger: `insert+ctrl` is not a spelling of
            // `ctrl+insert`, it is a typo, and accepting it would mean the
            // vocabulary has an order-insensitive dialect nobody documented.
            return Err(ChordSpecError::UnknownKey);
        }
        Self::new(&mods, Trigger::parse(last)?)
    }

    /// The trigger key's keysym, for `main`'s startup collision check against
    /// the core's other chord owners.
    ///
    /// Deliberately the *trigger* rather than the whole chord: two gestures
    /// that differ only in their modifier set are told apart perfectly well by
    /// [`ChordMatcher`], but an operator reading two flags should not have to
    /// reason about modifier-set equality to know whether their keys collide,
    /// and the dead-man watcher's `observe` tap does not care about modifiers
    /// at all -- it sees the trigger's press whatever else is held.
    pub fn trigger_keysym(&self) -> u32 {
        self.trigger.keysym
    }

    /// The evdev scancodes one press of this chord is spelled with: the
    /// modifiers in [`Modifier::TABLE`] order, then the trigger.
    ///
    /// One consumer, the same one [`Trigger::evdev`] and [`Modifier::evdev`]
    /// have: the `physical-input-injector` channel, which presses the
    /// *configured* gesture through the production intake rather than a
    /// scancode a harness guessed. Assembled here rather than in the channel so
    /// a chord's spelling and the keys pressed for it cannot come from two
    /// different places — that is exactly how a harness ends up proving
    /// something about a gesture the operator did not configure.
    #[cfg_attr(not(any(test, feature = "physical-input-injector")), allow(dead_code))]
    pub fn scancodes(&self) -> (Vec<u32>, u32) {
        let mods = Modifier::TABLE
            .iter()
            .filter(|(_, modifier, _, _, _)| self.mods.contains(*modifier))
            .map(|(_, modifier, _, _, _)| modifier.evdev())
            .collect();
        (mods, self.trigger.evdev)
    }

    /// Human-facing spelling, rebuilt from the parsed parts rather than echoed
    /// from input, so a log line can never carry an operator's typo.
    pub fn spelling(&self) -> String {
        let mut out = String::new();
        for (name, modifier, _, _, _) in Modifier::TABLE {
            if self.mods.contains(*modifier) {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(self.trigger.name);
        out
    }
}

/// Why a chord specification was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChordSpecError {
    /// Not a trigger name this build offers.
    UnknownKey,
    /// Named, but this build's input intake cannot produce its keysym — so the
    /// chord could never fire. Unreachable while [`Trigger::VOCABULARY`] and
    /// [`crate::input::invariant_keysym`] agree; it exists so that if they ever
    /// stop agreeing, the failure is a startup error rather than a silently
    /// dead gesture.
    NotIntakeable,
    /// A modifier named twice.
    RepeatedModifier,
    /// No modifier at all.
    NoModifier,
    /// Two bindings on the same chord.
    Duplicate,
}

impl std::fmt::Display for ChordSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChordSpecError::UnknownKey => write!(f, "unknown chord key"),
            ChordSpecError::NotIntakeable => {
                write!(f, "this build's input intake cannot deliver that key")
            }
            ChordSpecError::RepeatedModifier => write!(f, "a modifier is named twice"),
            ChordSpecError::NoModifier => write!(f, "a chord needs at least one modifier"),
            ChordSpecError::Duplicate => write!(f, "two bindings name the same chord"),
        }
    }
}

/// A set of chord bindings and the modifier state they are judged against.
///
/// `K` is whatever the caller wants a match to mean; this module never inspects
/// it. It must be `Copy` so a match can be reported without borrowing the
/// matcher across the gate's `&mut self`.
#[derive(Debug)]
pub(crate) struct ChordMatcher<K: Copy> {
    bindings: Vec<(ModChord, K)>,
    /// Modifiers the **human** is holding, updated in `observe` so a prompt
    /// that swallows a release cannot leave a bit stuck (module docs, rule 2).
    down: Modifiers,
    /// Trigger keysyms whose press this matcher consumed, so their release is
    /// consumed too however the modifiers have moved since.
    consumed: BTreeSet<u32>,
}

impl<K: Copy> ChordMatcher<K> {
    /// Build a matcher, refusing two bindings on the same chord.
    ///
    /// The refusal is what makes "at most one binding fires" unconstructible
    /// rather than merely true of the current list: with duplicates excluded and
    /// modifier sets compared for equality, the search below can find one entry
    /// or none, never two.
    pub fn new(bindings: Vec<(ModChord, K)>) -> Result<Self, ChordSpecError> {
        for (index, (chord, _)) in bindings.iter().enumerate() {
            if bindings[..index].iter().any(|(other, _)| other == chord) {
                return Err(ChordSpecError::Duplicate);
            }
        }
        Ok(Self {
            bindings,
            down: Modifiers::default(),
            consumed: BTreeSet::new(),
        })
    }

    /// Every chord this matcher holds, for a startup collision check against the
    /// core's other chord owners.
    pub fn chords(&self) -> impl Iterator<Item = &ModChord> {
        self.bindings.iter().map(|(chord, _)| chord)
    }

    /// **The tracking half**: fold one event into the modifier state.
    ///
    /// Physical origin only. An agent's emulated Shift must not arm the human's
    /// next keypress, which is the subtler half of the unforgeability claim and
    /// the one a reader is likelier to miss.
    pub fn observe(&mut self, input: &SeatInput) {
        if input.origin() != Origin::Physical {
            return;
        }
        let SeatInputKind::Key { keysym, state, .. } = input.kind() else {
            return;
        };
        let Some(modifier) = Modifier::of_keysym(*keysym) else {
            return;
        };
        self.down = match state {
            KeyState::Pressed => self.down.with(modifier),
            KeyState::Released => self.down.without(modifier),
        };
    }

    /// **The matching half**: decide whether this event is a chord.
    ///
    /// Returns the gate verdict and, on a matched press, the binding's value.
    /// A modifier key is never consumed; an unmatched trigger is never
    /// consumed; a matched trigger's press *and* its release both are.
    pub fn gate(&mut self, input: &SeatInput) -> (Gate, Option<K>) {
        if input.origin() != Origin::Physical {
            return (Gate::Deliver, None);
        }
        let SeatInputKind::Key { keysym, state, .. } = input.kind() else {
            return (Gate::Deliver, None);
        };
        if Modifier::of_keysym(*keysym).is_some() {
            return (Gate::Deliver, None);
        }
        match state {
            KeyState::Pressed => {
                // Exact set equality, never a superset test: `ctrl+shift+insert`
                // and `shift+insert` are different gestures, and a superset test
                // would make the first fire both (module docs, rule 3).
                let hit = self
                    .bindings
                    .iter()
                    .find(|(chord, _)| chord.trigger.keysym == *keysym && chord.mods == self.down)
                    .map(|(_, value)| *value);
                match hit {
                    Some(value) => {
                        self.consumed.insert(*keysym);
                        (Gate::Consume, Some(value))
                    }
                    None => (Gate::Deliver, None),
                }
            }
            KeyState::Released => {
                if self.consumed.remove(keysym) {
                    (Gate::Consume, None)
                } else {
                    (Gate::Deliver, None)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{KeySource, SeatInputKind};

    const SHIFT_L: u32 = 0xffe1;
    const SHIFT_R: u32 = 0xffe2;
    const CTRL_L: u32 = 0xffe3;
    const INSERT: u32 = 0xff63;

    /// WS-E.3.1 (issue #217, D-028): the core can now interpret a keymap on a
    /// bare-metal backend, so "which keysym does this scancode produce" stops
    /// being a constant for most of the keyboard. Every key **this** module's
    /// two vocabularies name must stay in the part that is still constant —
    /// [`crate::input::invariant_keysym`]'s fixed table — because a chord
    /// whose keysym moved with the layout is a human gesture that stops
    /// working on somebody else's keyboard.
    ///
    /// The counts are asserted so the sweep cannot silently shrink to
    /// nothing, and they are the numbers the tables actually hold: 28
    /// triggers, 4 modifiers with a left and a right keysym each.
    #[test]
    fn every_chord_vocabulary_keysym_survives_a_core_that_owns_a_keymap() {
        assert_eq!(Trigger::VOCABULARY.len(), 28);
        for (name, evdev, keysym) in Trigger::VOCABULARY {
            assert_eq!(
                crate::input::invariant_keysym(*evdev),
                Some(*keysym),
                "trigger `{name}` must still come off the layout-invariant table"
            );
            assert!(
                crate::input::keysym_is_intakeable(*keysym),
                "trigger `{name}` must still be deliverable"
            );
        }

        assert_eq!(Modifier::TABLE.len(), 4);
        for (name, _, left, right, evdev) in Modifier::TABLE {
            assert_eq!(
                crate::input::invariant_keysym(*evdev),
                Some(*left),
                "modifier `{name}`'s scancode must still resolve to its left keysym"
            );
            for (side, keysym) in [("left", left), ("right", right)] {
                assert!(
                    crate::input::keysym_is_intakeable(*keysym),
                    "modifier `{name}` ({side}) must still be deliverable"
                );
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Act {
        Promote,
        Offer,
    }

    fn key(keysym: u32, pressed: bool) -> SeatInputKind {
        SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym,
            state: if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
        }
    }

    fn physical(keysym: u32, pressed: bool) -> SeatInput {
        // The production intake is the only physical-tagging path in the crate
        // (`SeatInput::physical` is private to `crate::input`), so a test drives
        // the real scancode table rather than minting a tag.
        let evdev = match keysym {
            SHIFT_L => 42,
            SHIFT_R => 54,
            CTRL_L => 29,
            INSERT => 110,
            other => panic!("no scancode wired for {other:#x}"),
        };
        let mut events = crate::input::physical_key(
            evdev,
            None,
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
        );
        assert_eq!(events.len(), 1, "the intake table must produce this key");
        events.remove(0)
    }

    fn emulated(keysym: u32, pressed: bool) -> SeatInput {
        SeatInput::emulated(key(keysym, pressed))
    }

    fn matcher() -> ChordMatcher<Act> {
        ChordMatcher::new(vec![
            (ModChord::parse("ctrl+shift+insert").unwrap(), Act::Promote),
            (ModChord::parse("shift+insert").unwrap(), Act::Offer),
        ])
        .unwrap()
    }

    /// Drive one event through both halves in the order the hook stack does.
    fn step(m: &mut ChordMatcher<Act>, input: &SeatInput) -> (Gate, Option<Act>) {
        m.observe(input);
        m.gate(input)
    }

    // -- the vocabulary and its startup discipline ---------------------------

    #[test]
    fn every_offered_trigger_survives_the_real_intake_table() {
        for name in Trigger::vocabulary() {
            let trigger = Trigger::parse(name).expect("every offered name must parse");
            assert!(
                crate::input::keysym_is_intakeable(trigger.keysym()),
                "{name}: a trigger intake cannot deliver could never fire"
            );
            assert!(
                Modifier::of_keysym(trigger.keysym()).is_none(),
                "{name}: a modifier must never be a trigger -- it would match itself"
            );
            assert_eq!(
                crate::input::invariant_keysym(trigger.evdev()),
                Some(trigger.keysym()),
                "{name}: the scancode beside the keysym must be the one that produces it"
            );
        }
    }

    #[test]
    fn a_malformed_specification_is_refused_rather_than_defaulted() {
        assert_eq!(ModChord::parse("insert"), Err(ChordSpecError::NoModifier));
        assert_eq!(ModChord::parse("ctrl+a"), Err(ChordSpecError::UnknownKey));
        assert_eq!(ModChord::parse("ctrl+1"), Err(ChordSpecError::UnknownKey));
        assert_eq!(
            ModChord::parse("CTRL+INSERT"),
            Err(ChordSpecError::UnknownKey)
        );
        assert_eq!(
            ModChord::parse("ctrl+ctrl+insert"),
            Err(ChordSpecError::RepeatedModifier)
        );
        assert_eq!(
            ModChord::parse("insert+ctrl"),
            Err(ChordSpecError::UnknownKey),
            "a modifier after the trigger is a typo, not an order-insensitive dialect"
        );
        assert_eq!(ModChord::parse(""), Err(ChordSpecError::UnknownKey));
        assert_eq!(
            ModChord::parse("ctrl+shift+insert").unwrap().spelling(),
            "ctrl+shift+insert"
        );
        assert_eq!(
            ModChord::parse("shift+ctrl+insert").unwrap().spelling(),
            "ctrl+shift+insert",
            "spelling is rebuilt from the parsed set, so it is canonical"
        );
    }

    #[test]
    fn two_bindings_on_one_chord_are_unconstructible() {
        let err = ChordMatcher::new(vec![
            (ModChord::parse("shift+insert").unwrap(), Act::Promote),
            (ModChord::parse("shift+insert").unwrap(), Act::Offer),
        ])
        .unwrap_err();
        assert_eq!(err, ChordSpecError::Duplicate);
    }

    // -- unforgeability: the two `if`s ---------------------------------------

    #[test]
    fn an_emulated_chord_matches_nothing_and_is_delivered_untouched() {
        // An agent holding `actuate_text` over any realm can put these keysyms
        // on the wire. If the matcher judged by keysym alone it could
        // manufacture the human's clipboard gesture.
        let mut m = matcher();
        for event in [
            emulated(SHIFT_L, true),
            emulated(INSERT, true),
            emulated(INSERT, false),
            emulated(SHIFT_L, false),
        ] {
            assert_eq!(step(&mut m, &event), (Gate::Deliver, None));
        }
    }

    #[test]
    fn an_emulated_modifier_cannot_arm_a_humans_ordinary_keypress() {
        // The subtler half, and the one a reader is likelier to miss: an agent
        // holds a synthetic Shift, the human then presses Insert for their own
        // reasons, and -- if `observe` tracked emulated modifiers -- the core
        // would read a cross-realm clipboard gesture out of one ordinary
        // keystroke that nobody chorded.
        let mut m = matcher();
        assert_eq!(
            step(&mut m, &emulated(SHIFT_L, true)),
            (Gate::Deliver, None)
        );
        assert_eq!(
            step(&mut m, &physical(INSERT, true)),
            (Gate::Deliver, None),
            "a physical Insert with only an AGENT's Shift held is not a chord"
        );
    }

    // -- exact matching -------------------------------------------------------

    /// **`ctrl+shift+insert` must not also satisfy `shift+insert`.** With a
    /// superset test it would, and the promote gesture would silently offer as
    /// well -- precisely "one gesture transfers something", the property #213
    /// exists to forbid.
    ///
    /// **Driven under BOTH binding orders, and that is the whole test.** The
    /// first draft registered the chords in one order only, and a superset
    /// match survived it untouched: `find` returns the first entry, and with
    /// `ctrl+shift+insert` listed first the wrong answer was never reachable.
    /// The property is not "this list is in a lucky order", it is "the sets
    /// are compared for equality", so the check has to be order-free -- which
    /// means constructing the matcher both ways and demanding the same answer.
    #[test]
    fn the_two_chords_are_mutually_exclusive_by_exact_modifier_equality() {
        for reversed in [false, true] {
            let bindings = if reversed {
                vec![
                    (ModChord::parse("shift+insert").unwrap(), Act::Offer),
                    (ModChord::parse("ctrl+shift+insert").unwrap(), Act::Promote),
                ]
            } else {
                vec![
                    (ModChord::parse("ctrl+shift+insert").unwrap(), Act::Promote),
                    (ModChord::parse("shift+insert").unwrap(), Act::Offer),
                ]
            };
            let mut m = ChordMatcher::new(bindings).unwrap();
            step(&mut m, &physical(CTRL_L, true));
            step(&mut m, &physical(SHIFT_L, true));
            assert_eq!(
                step(&mut m, &physical(INSERT, true)),
                (Gate::Consume, Some(Act::Promote)),
                "reversed={reversed}: ctrl+shift+insert must be the PROMOTE chord under \
                 either registration order. A superset match makes it satisfy \
                 `shift+insert` too, and then the answer is whichever was listed first"
            );

            let mut m = matcher();
            step(&mut m, &physical(SHIFT_L, true));
            assert_eq!(
                step(&mut m, &physical(INSERT, true)),
                (Gate::Consume, Some(Act::Offer))
            );
        }
    }

    #[test]
    fn either_side_of_a_modifier_pair_is_the_same_modifier() {
        let mut m = matcher();
        step(&mut m, &physical(SHIFT_R, true));
        assert_eq!(
            step(&mut m, &physical(INSERT, true)),
            (Gate::Consume, Some(Act::Offer)),
            "a human reaching for the right Shift has not asked for a different gesture"
        );
    }

    #[test]
    fn a_bare_trigger_and_a_wrongly_modified_one_are_delivered() {
        let mut m = matcher();
        assert_eq!(step(&mut m, &physical(INSERT, true)), (Gate::Deliver, None));
        assert_eq!(
            step(&mut m, &physical(INSERT, false)),
            (Gate::Deliver, None),
            "an app that binds Insert keeps it"
        );

        step(&mut m, &physical(CTRL_L, true));
        assert_eq!(
            step(&mut m, &physical(INSERT, true)),
            (Gate::Deliver, None),
            "ctrl+insert is nobody's chord here"
        );
    }

    #[test]
    fn modifiers_are_never_consumed() {
        let mut m = matcher();
        for event in [
            physical(CTRL_L, true),
            physical(SHIFT_L, true),
            physical(SHIFT_L, false),
            physical(CTRL_L, false),
        ] {
            assert_eq!(
                step(&mut m, &event),
                (Gate::Deliver, None),
                "the app needs its modifiers"
            );
        }
    }

    // -- pairing --------------------------------------------------------------

    #[test]
    fn a_consumed_press_has_its_release_consumed_however_the_modifiers_moved() {
        // A human releases Shift before Insert about as often as not. If the
        // release were judged against the modifiers again it would be
        // *delivered*, and the app would see a release whose press it never
        // saw -- the exact bookkeeping the pairing contract exists to protect.
        let mut m = matcher();
        step(&mut m, &physical(SHIFT_L, true));
        assert_eq!(
            step(&mut m, &physical(INSERT, true)),
            (Gate::Consume, Some(Act::Offer))
        );
        step(&mut m, &physical(SHIFT_L, false));
        assert_eq!(
            step(&mut m, &physical(INSERT, false)),
            (Gate::Consume, None),
            "the release of a consumed press is consumed too"
        );
        // ...and only once: a second, unpaired release is delivered.
        assert_eq!(
            step(&mut m, &physical(INSERT, false)),
            (Gate::Deliver, None)
        );
    }

    #[test]
    fn a_modifier_release_the_gate_never_saw_still_clears_the_bit() {
        // `observe` is unconditional, so this is the state a consent prompt
        // leaves behind when it swallows a release. If tracking lived in `gate`
        // the bit would stick and every later Insert would be a chord.
        let mut m = matcher();
        m.observe(&physical(SHIFT_L, true));
        m.observe(&physical(SHIFT_L, false));
        assert_eq!(m.gate(&physical(INSERT, true)), (Gate::Deliver, None));
    }
}
