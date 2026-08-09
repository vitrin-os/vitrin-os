// SPDX-License-Identifier: MPL-2.0
//! The **cross-realm clipboard**: one core-held slot, filled and offered by two
//! distinct physical human gestures (WS-E.2.1, issue #213,
//! [D-024](../../../docs/plan/20-decision-log.md)).
//!
//! # What this is, and the cost it is
//!
//! [PRD](../../../docs/PRD.md) §11 fixes the model — a core-held buffer, two
//! explicit human actions, the Qubes Ctrl-Shift-C/V shape — and PRD §15's first
//! threat row used to say a malicious app cannot *"reach the session's real
//! seat/clipboard/a11y bus"*. This module is a **deliberate, bounded hole in a
//! published claim**, and the bound is the artifact: one MIME type, 60 KiB, one
//! direction per gesture, cleared three ways.
//!
//! It is also the first place the trusted core retains bytes an **application**
//! authored. The core holds client *pixels* it never interprets and typed values
//! it validated itself; a clipboard slot is neither. A password copied from a
//! manager transits `vitrin-core` and rests here with a lifetime. That cost was
//! put to the maintainer in those words on 2026-08-08 and accepted; nothing
//! below removes it, and no later edit should describe the mitigations as if one
//! did.
//!
//! # The core pulls, and that is a type rather than a rule
//!
//! A shim never pushes. There is no `selection_changed` event, because a shim
//! that forwarded every app-side copy upstream would make every ordinary Ctrl-C
//! a cross-realm event — an ambient channel wearing a broker's clothes.
//!
//! What makes it structural: [`ClipboardSlot::fill`] takes a
//! [`PendingPromotion`] **by value**. That type has no public constructor, is
//! neither `Copy` nor `Clone`, and the only way to obtain one is
//! [`ClipboardSlot::claim_answer`], which matches the serial of an outstanding
//! promotion this core itself opened on a human's gesture and removes it. An
//! unsolicited `selection` request from a compromised shim therefore has no
//! ticket to consume and **fills nothing** — a compile-time shape, in the mould
//! of [`crate::attention::Exemption`] and `spawn::SpawnRecord`, rather than an
//! `if` a later refactor can route around.
//!
//! # One gesture transfers nothing
//!
//! Promoting writes the slot and reaches no other realm. Offering reads the slot
//! and never writes it — [`ClipboardSlot::peek`] takes `&self`. The two gestures
//! are distinct chords whose modifier sets are compared for *equality*
//! ([`crate::chord`]), so no single press can produce both, and no press can
//! produce the wrong one.
//!
//! # Secrecy: length and digest, never content
//!
//! The precedent is `recorder.rs`'s `credential_bytes: usize` — *"Length only —
//! never the bytes (secrecy contract)"* — and [`ActuationDetail::Text`], which
//! *"holds a length and a digest, and has no field a string could be put in"*.
//! Both journal entries this module produces are built the same way, and so is
//! [`ClipboardContent`]: its `Debug` prints a length and a digest, because a
//! `#[derive(Debug)]` on anything holding the slot would otherwise print a
//! password into a panic message.
//!
//! The MIME type recorded is this module's own `&'static str` constant, never
//! the shim's bytes, so even the type field cannot carry attacker-chosen text.
//!
//! [`ActuationDetail::Text`]: crate::recorder::ActuationDetail

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::chord::{ChordMatcher, ChordSpecError, ModChord, Modifier, Trigger};
use crate::grants::RealmId;
use crate::input::{Gate, PreemptionHook, SeatInput};

/// The only MIME type that crosses a realm boundary.
///
/// Every other type is a decoder question, and *"no image codec in the core, in
/// any dependency class"* is a rule `crates/vitrin-core/Cargo.toml` states
/// twice. `text/uri-list` is refused on a separate ground: a path is a
/// **designation**, designation belongs to the powerbox (E2.6), and a clipboard
/// that moved paths would be a file-transfer channel wearing a clipboard's
/// clothes.
pub(crate) const CLIPBOARD_MIME: &str = "text/plain;charset=utf-8";

/// The hard byte cap, **measured** rather than asserted (D-024(5); the table is
/// in `docs/plan/14-workstream-session-mode.md` §4.1).
///
/// Two bounds meet here and only one is a choice. The wire's `u16` frame size
/// caps a whole frame at 65 535 bytes, so the largest string a `selection` can
/// carry is 65 476 and the 1 MiB the issue proposed is **inexpressible** without
/// an fd — which would put a shim-controlled mapping inside the TCB to carry a
/// clipboard. Within that ceiling, against this repository's own 6 550 text
/// files as the proxy for "a human selects all and copies", 60 KiB copies 96.58%
/// of them whole where the frame maximum copies 96.70%: the last 4 036 bytes buy
/// 0.12 points and spend every byte of framing headroom. 61 440 leaves 4 039
/// bytes spare, which is room for `selection` to gain an argument later without
/// the cap moving — and the cap **cannot** move, because it lives in the IDL's
/// immutable `(max N bytes)` token.
///
/// Kept in step with the generated decoder's own bound by
/// `the_cap_is_the_wires_cap`, so the two can never drift into a state where the
/// core believes it is enforcing something the wire already refused (or, worse,
/// the reverse).
pub(crate) const MAX_CLIPBOARD_BYTES: usize = 61440;

/// How long a promoted selection stays offerable.
///
/// **Bracketed, not measured, and said so.** There is no experiment here that
/// yields a number: the lower bound is the human's slowest realistic path
/// (promote, move the output — which today means typing a line into a shell —
/// find the paste point), which is seconds to tens of seconds; the upper bound
/// is "a copied password must not be resident for the session". Two minutes sits
/// inside that bracket with room at both ends. Anyone with a better instrument
/// should replace this comment with its output rather than re-deriving the same
/// guess.
///
/// "Cleared on paste" was rejected outright: humans paste twice, and a mechanism
/// that surprises them once is one they stop trusting.
pub(crate) const CLIPBOARD_LIFETIME: Duration = Duration::from_secs(120);

/// The default trigger key for both chords: `ctrl+shift+insert` promotes,
/// `shift+insert` offers.
///
/// `KEY_INSERT` is in [`crate::input::invariant_keysym`] and letters are not
/// (asserted, not assumed), so this is the only shape of the Qubes gesture that
/// survives WS-E Stage 3's keymap-less backend — and Shift-Insert is the
/// historical X11 clipboard chord, so the pair is familiar rather than invented.
pub(crate) const DEFAULT_TRIGGER: &str = "insert";

/// The most gestures one dispatch turn will queue before dropping them.
///
/// A human cannot chord four times in one compositor turn; the bound exists so
/// that a stuck embedder cannot grow this queue without limit, which is the
/// same posture `deadman::MAX_REPLAY_BACKLOG` takes for the same reason.
const MAX_PENDING_GESTURES: usize = 4;

/// Which of the human's two clipboard gestures fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardGesture {
    /// `ctrl+shift+insert`: ask the realm the output is bound to for its
    /// selection, and put it in the slot.
    Promote,
    /// `shift+insert`: offer the slot to the realm the output is bound to.
    Offer,
}

impl ClipboardGesture {
    /// A fixed label for the journal and the operator's log — never free-form
    /// text.
    pub fn label(self) -> &'static str {
        match self {
            ClipboardGesture::Promote => "promote",
            ClipboardGesture::Offer => "offer",
        }
    }
}

/// The clipboard's payload: UTF-8 text the core will not print.
///
/// **The `Debug` impl is the secrecy contract, not a courtesy.** Every struct
/// that transitively holds one — the slot, the kernel, a `Runtime` — derives
/// `Debug`, and a derived one here would print a copied password into any panic
/// message, `tracing` field or test failure that touched an enclosing value.
/// There is no `Display`, no `AsRef<str>` and no `Deref`: the one reader is
/// [`Self::expose`], whose name is what a reviewer greps for.
pub(crate) struct ClipboardContent {
    text: String,
    digest: blake3::Hash,
}

impl ClipboardContent {
    fn new(text: String) -> Self {
        let digest = blake3::hash(text.as_bytes());
        Self { text, digest }
    }

    /// Byte length — safe to log, and the only size anything outside sees.
    pub fn bytes(&self) -> usize {
        self.text.len()
    }

    /// BLAKE3 over the UTF-8, the same algorithm and tagging every other digest
    /// in the flight recorder uses. Enough to see that the same string crossed
    /// twice, or that it matches a known corpus, without the log ever holding
    /// it.
    pub fn digest(&self) -> blake3::Hash {
        self.digest
    }

    /// **The one reader.** Named so that a review of "what can put clipboard
    /// bytes anywhere" is a grep for this identifier and nothing else.
    pub fn expose(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Debug for ClipboardContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardContent")
            .field("bytes", &self.text.len())
            .field("digest", &self.digest.to_hex().as_str())
            .finish()
    }
}

/// **Permission to fill the slot once**, minted only when a human's promote
/// gesture actually fired, and consumed by [`ClipboardSlot::fill`].
///
/// This type *is* Axis 2's "the core pulls" — not a test of it. Its fields are
/// private to this module, so nothing outside can mint one; it is neither `Copy`
/// nor `Clone`, so no caller can keep a spare; and `fill` takes it **by value**,
/// so filling the slot twice from one gesture does not compile. A `selection`
/// request that no `request_selection` asked for finds no ticket in
/// [`ClipboardSlot::claim_answer`] and can do nothing at all.
///
/// `#[must_use]` catches the opposite mistake — claiming a ticket and dropping
/// it — as a warning rather than as a promotion that silently never happened.
#[derive(Debug)]
#[must_use = "a claimed promotion ticket must be spent on `fill` or the human's \
              gesture silently did nothing"]
pub(crate) struct PendingPromotion {
    /// The realm the core asked. Carried so the journal names the *source* of
    /// the bytes rather than only their size, and so a realm's death can cancel
    /// an answer that is still in flight.
    realm: RealmId,
}

/// One outstanding `request_selection`, waiting for its answer.
#[derive(Debug)]
struct Outstanding {
    serial: u32,
    realm: RealmId,
}

/// What the slot holds, once filled.
#[derive(Debug)]
struct Held {
    /// The realm the bytes came from — cleared when that realm dies.
    source: RealmId,
    content: ClipboardContent,
    /// When the promotion completed; [`CLIPBOARD_LIFETIME`] runs from here.
    at: Instant,
}

/// A borrowed view of the slot, for the code that puts it on the wire.
///
/// `Debug` is hand-written for the same reason [`ClipboardContent`]'s is: this
/// type carries the plaintext, so a derived one prints a password into any
/// panic message, `dbg!` or `{:?}` that touches it. `ClipboardContent` was
/// written that way from the start and this borrowed view — the one type that
/// actually *escapes* the slot — got the derive anyway, which is the shape a
/// secrecy rule fails in: applied to the obvious holder and missed on the one
/// that leaves.
pub(crate) struct Offered<'a> {
    /// This module's own constant, never the shim's bytes.
    pub mime: &'static str,
    pub data: &'a str,
    pub bytes: usize,
    pub digest: blake3::Hash,
    pub source: &'a RealmId,
}

impl std::fmt::Debug for Offered<'_> {
    /// Length, digest and source — never the bytes. Same contract as
    /// [`ClipboardContent`]'s, and the digest is what makes two offers
    /// comparable in a log without either being readable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offered")
            .field("mime", &self.mime)
            .field("bytes", &self.bytes)
            .field("digest", &self.digest.to_hex().as_str())
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// What a completed promotion is worth telling the journal.
#[derive(Debug)]
pub(crate) struct Promoted {
    pub mime: &'static str,
    pub bytes: usize,
    pub digest: blake3::Hash,
}

/// Why a `selection` answer did not reach the slot.
///
/// Every variant leaves the slot **exactly as it was** — there is no partial
/// fill, structurally, because [`ClipboardSlot::fill`] validates before it
/// touches `held` at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillRefused {
    /// The shim said it had nothing, or said `ok` and sent nothing.
    Empty,
    /// Not `text/plain;charset=utf-8`, whatever the shim's `status` claimed.
    /// The core re-judges the type because a shim is untrusted and `ok` is a
    /// claim rather than a credential.
    WrongType,
    /// Past [`MAX_CLIPBOARD_BYTES`]. Reachable from a well-behaved shim only as
    /// its own `too_large` status; a shim that sent oversized *data* would have
    /// been killed by the generated decoder's bound before reaching here, which
    /// is why the IDL asks the shim to judge it first.
    TooLarge,
}

impl FillRefused {
    /// A fixed label for the journal.
    pub fn label(self) -> &'static str {
        match self {
            FillRefused::Empty => "empty",
            FillRefused::WrongType => "wrong_type",
            FillRefused::TooLarge => "too_large",
        }
    }
}

/// **The session's one clipboard slot.**
///
/// Single, not per-realm: a per-realm broker multiplies state by `MAX_REALMS`
/// without changing the trust question (D-024(1)).
#[derive(Debug, Default)]
pub(crate) struct ClipboardSlot {
    held: Option<Held>,
    outstanding: Option<Outstanding>,
    /// Strictly increasing, never reused within a session, so a stale answer to
    /// a superseded gesture cannot fill the slot a newer gesture is waiting on.
    next_serial: u32,
}

impl ClipboardSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a promotion: mint the serial the core will send in
    /// `request_selection` and remember which realm was asked.
    ///
    /// A second gesture **supersedes** the first rather than queueing behind it:
    /// the human pressed again, so the earlier answer — if it ever comes — is
    /// stale by their own account. That is also why serials never repeat.
    pub fn open_promotion(&mut self, realm: RealmId) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        self.outstanding = Some(Outstanding { serial, realm });
        serial
    }

    /// Match a shim's answer against the outstanding promotion, consuming it.
    ///
    /// `None` for an unsolicited answer, for a stale serial, or for an answer
    /// that arrived on a *different* realm's connection than the one asked — the
    /// last is what stops a compromised shim answering on its neighbour's
    /// behalf. Without a ticket, nothing can call [`Self::fill`].
    pub fn claim_answer(&mut self, realm: &RealmId, serial: u32) -> Option<PendingPromotion> {
        let outstanding = self.outstanding.as_ref()?;
        if outstanding.serial != serial || &outstanding.realm != realm {
            return None;
        }
        let Outstanding { realm, .. } = self.outstanding.take()?;
        Some(PendingPromotion { realm })
    }

    /// Fill the slot from an answered promotion.
    ///
    /// **Fail-closed, with no partial fill**, and that is structural rather than
    /// careful: every check runs before `self.held` is touched, so a refusal
    /// leaves whatever was there before exactly as it was. A human whose oversized
    /// copy was refused still has their previous clipboard.
    pub fn fill(
        &mut self,
        ticket: PendingPromotion,
        mime: &str,
        data: &str,
        now: Instant,
    ) -> Result<Promoted, FillRefused> {
        if mime != CLIPBOARD_MIME {
            return Err(FillRefused::WrongType);
        }
        if data.len() > MAX_CLIPBOARD_BYTES {
            return Err(FillRefused::TooLarge);
        }
        if data.is_empty() {
            return Err(FillRefused::Empty);
        }
        let content = ClipboardContent::new(data.to_owned());
        let promoted = Promoted {
            mime: CLIPBOARD_MIME,
            bytes: content.bytes(),
            digest: content.digest(),
        };
        self.held = Some(Held {
            source: ticket.realm,
            content,
            at: now,
        });
        Ok(promoted)
    }

    /// Give up on an outstanding promotion (its realm died, or the shim
    /// answered a refusing status). The slot is untouched.
    pub fn abandon_promotion(&mut self) {
        self.outstanding = None;
    }

    /// **Read the slot.** `&self`, deliberately: offering never writes, which is
    /// half of "one gesture transfers nothing" made structural.
    pub fn peek(&self, now: Instant) -> Option<Offered<'_>> {
        let held = self.held.as_ref()?;
        if now.saturating_duration_since(held.at) >= CLIPBOARD_LIFETIME {
            return None;
        }
        Some(Offered {
            mime: CLIPBOARD_MIME,
            data: held.content.expose(),
            bytes: held.content.bytes(),
            digest: held.content.digest(),
            source: &held.source,
        })
    }

    /// Drop a slot past [`CLIPBOARD_LIFETIME`]. Returns whether it dropped one.
    ///
    /// Separate from [`Self::peek`] so that reading stays `&self`: expiry is a
    /// mutation and is called from the dispatch loop, where the mutable borrow
    /// is already available.
    pub fn expire(&mut self, now: Instant) -> bool {
        let stale = self
            .held
            .as_ref()
            .is_some_and(|h| now.saturating_duration_since(h.at) >= CLIPBOARD_LIFETIME);
        if stale {
            self.held = None;
        }
        stale
    }

    /// Clear everything this realm owns: the slot if it was promoted from there,
    /// and an outstanding promotion addressed to it.
    ///
    /// Called when a realm dies. Keeping bytes from a realm that no longer exists
    /// would mean the slot outliving the only thing that could account for it.
    pub fn forget_realm(&mut self, realm: &RealmId) -> bool {
        let mut cleared = false;
        if self.held.as_ref().is_some_and(|h| &h.source == realm) {
            self.held = None;
            cleared = true;
        }
        if self.outstanding.as_ref().is_some_and(|o| &o.realm == realm) {
            self.outstanding = None;
        }
        cleared
    }

    /// Empty the slot and forget any outstanding promotion.
    ///
    /// Called by [`crate::deadman::apply`]: a human who has just hit the
    /// off-switch is not, in the same second, leaving a copied password in the
    /// core. Unlike the attention window's clear this is **not** belt and
    /// braces — nothing else would have removed it, and the slot is the one
    /// piece of session state the dead-man switch's grant sweep cannot reach.
    pub fn clear(&mut self) -> bool {
        self.outstanding = None;
        self.held.take().is_some()
    }
}

/// The human's two clipboard chords, and the gestures they have produced that
/// the embedder has not yet acted on.
///
/// **The hook cannot act on a gesture and must not learn how**, which is the
/// same division [`crate::attention::AttentionSignal`] and
/// [`crate::deadman::Trigger`] make: the hook runs inside
/// [`crate::input::InputRouter::route_physical`], which holds no authority state
/// and no realm map and must never grow either. So the gate records the gesture
/// here and the embedder — which knows which realm the output is bound to, owns
/// the shim connections and owns the slot — drains it.
///
/// This is also why the hook is never told a realm and does not need to be: the
/// trait's session-wide guarantee is untouched.
#[derive(Debug)]
pub(crate) struct ClipboardSignal {
    matcher: ChordMatcher<ClipboardGesture>,
    pending: Vec<ClipboardGesture>,
    /// The configured trigger key's name, for the startup log and the journal.
    trigger: &'static str,
}

impl ClipboardSignal {
    /// Build the pair of chords from one trigger key: `ctrl+shift+KEY` promotes,
    /// `shift+KEY` offers.
    ///
    /// One trigger rather than two independent chords, because the two gestures
    /// are one mechanism and an operator who moved only half of it would have a
    /// clipboard that copies and does not paste.
    pub fn new(trigger: Trigger) -> Result<Self, ChordSpecError> {
        let promote = ModChord::new(&[Modifier::Ctrl, Modifier::Shift], trigger)?;
        let offer = ModChord::new(&[Modifier::Shift], trigger)?;
        Ok(Self {
            matcher: ChordMatcher::new(vec![
                (promote, ClipboardGesture::Promote),
                (offer, ClipboardGesture::Offer),
            ])?,
            pending: Vec::new(),
            trigger: trigger.name(),
        })
    }

    /// A signal for a build whose hook stack carries no [`ClipboardHook`] — a
    /// plain headless run, which has no physical input device to chord on.
    ///
    /// The kernel holds one unconditionally, exactly as it holds an attention
    /// signal and a presence map unconditionally, so "this backend has no
    /// clipboard chord" is a fact about the hook stack rather than about the
    /// session's state.
    pub fn detached() -> Self {
        Self::new(
            Trigger::parse(DEFAULT_TRIGGER).expect("the default trigger is in the vocabulary"),
        )
        .expect("the default chord pair is well-formed")
    }

    /// The configured trigger key's name.
    pub fn trigger(&self) -> &'static str {
        self.trigger
    }

    /// Every chord this signal owns, for `main.rs`'s startup collision check
    /// against the dead-man and attention vocabularies.
    pub fn chords(&self) -> impl Iterator<Item = &ModChord> {
        self.matcher.chords()
    }

    /// Drain the gestures this turn produced, oldest first.
    pub fn take_pending(&mut self) -> Vec<ClipboardGesture> {
        std::mem::take(&mut self.pending)
    }

    /// Forget the modifier bits and the consumed set across a seat pause —
    /// [`ChordMatcher::forget_physical_state`], whose docs carry the reason.
    ///
    /// The clipboard's own instance of it: a human holding Shift when they
    /// switch VT comes back to a matcher that believes Shift is still down, so
    /// their next bare Insert offers the core's slot to the focused realm.
    pub fn forget_physical_state(&mut self) {
        self.matcher.forget_physical_state();
    }
}

/// The clipboard chords attached at the router's preemption hook point.
///
/// Stacked `ConsentGate<DeadManHook<ClipboardHook<AttentionHook<NoopHook>>>>`,
/// and every position in that order is a decision:
///
/// - a **consent prompt** consumes the chord before this hook's `gate` sees it,
///   so no clipboard gesture can fire while the human is answering a security
///   question — while `observe` still runs (the grab forwards it
///   unconditionally), so a modifier release the prompt swallowed still clears
///   its bit and the matcher cannot be left believing Shift is held;
/// - a **dead-man chord press** short-circuits above this for the same reason:
///   the human's off-switch is never delayed by clipboard bookkeeping;
/// - it sits **outside** [`crate::attention::AttentionHook`], which consumes
///   both Supers. Inside it, this matcher would never see a Super press and its
///   `super` modifier bit would be permanently wrong — harmless for issue #213's
///   two chords and a trap laid for whichever of #214/#216 wants one.
pub(crate) struct ClipboardHook<H: PreemptionHook> {
    signal: Rc<RefCell<ClipboardSignal>>,
    inner: H,
}

impl<H: PreemptionHook> ClipboardHook<H> {
    pub fn new(signal: Rc<RefCell<ClipboardSignal>>, inner: H) -> Self {
        Self { signal, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for ClipboardHook<H> {
    /// Modifier tracking only — unconditional, so a release the consent grab
    /// swallowed still clears its bit ([`crate::chord`]'s rule 2). It decides
    /// nothing and can stop nothing.
    fn observe(&mut self, input: &SeatInput) {
        self.signal.borrow_mut().matcher.observe(input);
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        let verdict = {
            let mut signal = self.signal.borrow_mut();
            let (gate, gesture) = signal.matcher.gate(input);
            if let Some(gesture) = gesture {
                if signal.pending.len() < MAX_PENDING_GESTURES {
                    signal.pending.push(gesture);
                } else {
                    // A human cannot chord four times in one compositor turn, so
                    // this means the embedder is not draining. Dropping is the
                    // benign direction: a gesture that does not happen costs a
                    // repeat, where an unbounded queue costs the session.
                    tracing::warn!(
                        gesture = gesture.label(),
                        "clipboard gesture dropped: the pending queue is not being drained"
                    );
                }
            }
            gate
        };
        match verdict {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }

    fn attention(&self) -> Option<Rc<RefCell<crate::attention::AttentionSignal>>> {
        self.inner.attention()
    }

    fn clipboard(&self) -> Option<Rc<RefCell<ClipboardSignal>>> {
        Some(Rc::clone(&self.signal))
    }

    fn screenshot(&self) -> Option<Rc<RefCell<crate::screenshot::ScreenshotSignal>>> {
        self.inner.screenshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputRouter, NoopHook};
    use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

    fn realm(name: &str) -> RealmId {
        RealmId::new(name)
    }

    fn slot_with(text: &str, now: Instant) -> ClipboardSlot {
        let mut slot = ClipboardSlot::new();
        let source = realm("realm-0");
        let serial = slot.open_promotion(source.clone());
        let ticket = slot
            .claim_answer(&source, serial)
            .expect("the answer matches the promotion we opened");
        slot.fill(ticket, CLIPBOARD_MIME, text, now)
            .expect("a well-formed selection fills the slot");
        slot
    }

    // -- the cap and the allow-list -----------------------------------------

    /// The core's cap must be exactly the wire's, in both directions. A core cap
    /// *above* the wire's would let the core believe it accepted something the
    /// decoder had already killed the connection over; a core cap *below* it
    /// would refuse silently what the IDL advertises as legal.
    #[test]
    fn the_cap_is_the_wires_cap() {
        let bound = vitrin_protocol::generated::vitrin_shim_session::requests::Selection {
            serial: 0,
            status: vitrin_protocol::generated::vitrin_shim_session::SelectionStatus::Ok,
            mime: String::from(CLIPBOARD_MIME),
            data: "x".repeat(MAX_CLIPBOARD_BYTES),
        }
        .encode(1);
        assert!(
            bound.len() <= u16::MAX as usize,
            "a `selection` at the cap must fit one frame: {} bytes",
            bound.len()
        );
        // ...and the whole message stays clear of the ceiling with room for the
        // signature to grow, which is the reason the cap is 61440 rather than
        // the frame maximum (D-024(5)).
        assert!(u16::MAX as usize - bound.len() >= 4000);
        assert_eq!(CLIPBOARD_MIME.len(), 24);
    }

    #[test]
    fn an_oversized_selection_is_refused_and_the_slot_is_left_untouched() {
        // Fail-closed with no partial fill: the human's previous clipboard
        // survives an oversized copy.
        let now = Instant::now();
        let mut slot = slot_with("keep me", now);
        let source = realm("realm-1");
        let serial = slot.open_promotion(source.clone());
        let ticket = slot.claim_answer(&source, serial).unwrap();
        let huge = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert_eq!(
            slot.fill(ticket, CLIPBOARD_MIME, &huge, now).err(),
            Some(FillRefused::TooLarge)
        );
        let offered = slot.peek(now).expect("the old slot survives");
        assert_eq!(offered.data, "keep me");
        assert_eq!(offered.source, &realm("realm-0"));

        // Exactly at the cap is legal -- the boundary is inclusive, and a test
        // that only checked `cap + 1` would pass on an off-by-one that refused
        // every copy of exactly 60 KiB.
        let serial = slot.open_promotion(source.clone());
        let ticket = slot.claim_answer(&source, serial).unwrap();
        let exact = "y".repeat(MAX_CLIPBOARD_BYTES);
        assert!(slot.fill(ticket, CLIPBOARD_MIME, &exact, now).is_ok());
    }

    #[test]
    fn a_mime_outside_the_allow_list_is_refused_and_the_slot_is_left_untouched() {
        let now = Instant::now();
        let mut slot = slot_with("keep me", now);
        let source = realm("realm-1");
        for mime in [
            "image/png",
            "text/uri-list",
            "text/plain",
            "text/plain;charset=UTF-8",
            "",
        ] {
            let serial = slot.open_promotion(source.clone());
            let ticket = slot.claim_answer(&source, serial).unwrap();
            assert_eq!(
                slot.fill(ticket, mime, "hostile", now).err(),
                Some(FillRefused::WrongType),
                "`{mime}` must not cross the realm boundary"
            );
        }
        assert_eq!(slot.peek(now).unwrap().data, "keep me");
    }

    // -- the core pulls: the ticket ------------------------------------------

    /// **A shim cannot push.** There is no path from an unsolicited `selection`
    /// to the slot, because `fill` cannot be called without a
    /// [`PendingPromotion`] and `claim_answer` is the only source of one.
    #[test]
    fn an_unsolicited_or_stale_or_misaddressed_answer_claims_nothing() {
        let mut slot = ClipboardSlot::new();
        let a = realm("realm-0");
        let b = realm("realm-1");

        // Nothing outstanding at all.
        assert!(slot.claim_answer(&a, 0).is_none());

        let serial = slot.open_promotion(a.clone());
        // Right realm, wrong serial: a stale answer to a superseded gesture.
        assert!(slot.claim_answer(&a, serial.wrapping_add(1)).is_none());
        // Right serial, wrong realm: a compromised shim answering on its
        // neighbour's behalf.
        assert!(slot.claim_answer(&b, serial).is_none());
        // The real answer still works, and works exactly once.
        assert!(slot.claim_answer(&a, serial).is_some());
        assert!(slot.claim_answer(&a, serial).is_none());
    }

    #[test]
    fn a_second_gesture_supersedes_the_first_and_serials_are_never_reused() {
        let mut slot = ClipboardSlot::new();
        let a = realm("realm-0");
        let first = slot.open_promotion(a.clone());
        let second = slot.open_promotion(a.clone());
        assert_ne!(first, second);
        assert!(
            slot.claim_answer(&a, first).is_none(),
            "the superseded gesture's answer must not fill the slot the newer one awaits"
        );
        assert!(slot.claim_answer(&a, second).is_some());
    }

    // -- lifetime -------------------------------------------------------------

    #[test]
    fn the_slot_expires_and_a_dead_source_realm_takes_it_with_it() {
        let now = Instant::now();
        let mut slot = slot_with("secret", now);
        assert!(slot.peek(now).is_some());
        assert!(slot.peek(now + CLIPBOARD_LIFETIME).is_none());
        assert!(slot.expire(now + CLIPBOARD_LIFETIME));
        assert!(!slot.expire(now + CLIPBOARD_LIFETIME));

        let mut slot = slot_with("secret", now);
        assert!(!slot.forget_realm(&realm("realm-9")));
        assert!(slot.peek(now).is_some());
        assert!(slot.forget_realm(&realm("realm-0")));
        assert!(slot.peek(now).is_none());

        // An outstanding promotion addressed to a dead realm is abandoned too,
        // so its answer can never arrive from a realm that no longer exists.
        let mut slot = ClipboardSlot::new();
        let serial = slot.open_promotion(realm("realm-0"));
        slot.forget_realm(&realm("realm-0"));
        assert!(slot.claim_answer(&realm("realm-0"), serial).is_none());
    }

    #[test]
    fn clearing_empties_the_slot_and_reports_whether_it_held_anything() {
        let now = Instant::now();
        let mut slot = slot_with("secret", now);
        assert!(slot.clear());
        assert!(!slot.clear());
        assert!(slot.peek(now).is_none());
    }

    // -- secrecy --------------------------------------------------------------

    /// **Content never reaches a log**, and the check is on the `Debug` output
    /// of the whole enclosing structure rather than on the leaf, because that is
    /// what a panic message or a `tracing` field actually prints.
    #[test]
    fn no_debug_rendering_anywhere_contains_the_content() {
        const SECRET: &str = "correct-horse-battery-staple";
        let now = Instant::now();
        let slot = slot_with(SECRET, now);
        let rendered = format!("{slot:?}");
        assert!(
            !rendered.contains(SECRET),
            "a Debug impl leaked clipboard content"
        );
        assert!(rendered.contains("bytes"), "{rendered}");
        assert!(
            rendered.contains(blake3::hash(SECRET.as_bytes()).to_hex().as_str()),
            "the digest is what stands in for the content: {rendered}"
        );
        // The leaf too, since it is what a future struct will hold.
        let content = ClipboardContent::new(String::from(SECRET));
        assert!(!format!("{content:?}").contains(SECRET));
        assert_eq!(content.expose(), SECRET);

        // ...and the BORROWED VIEW, which is the one that leaves this module
        // and therefore the one most likely to be formatted somewhere else.
        // It carried a derived `Debug` while the two types above were
        // hand-written, and this test checked the two that never travel. That
        // is how a secrecy rule fails: applied to the obvious holder, missed on
        // the one that escapes.
        let offered = slot.peek(now).expect("the fixture slot is full");
        let rendered = format!("{offered:?}");
        assert!(
            !rendered.contains(SECRET),
            "Offered's Debug leaked clipboard content: {rendered}"
        );
        assert!(
            rendered.contains(blake3::hash(SECRET.as_bytes()).to_hex().as_str()),
            "the digest must still stand in for the content: {rendered}"
        );
    }

    // -- the hook -------------------------------------------------------------

    fn signal() -> Rc<RefCell<ClipboardSignal>> {
        Rc::new(RefCell::new(
            ClipboardSignal::new(Trigger::parse(DEFAULT_TRIGGER).unwrap()).unwrap(),
        ))
    }

    /// The two chords, driven through the **production router** so consumption
    /// is the router's own answer rather than the hook's, and so the physical
    /// tag comes from the real intake table (`SeatInput::physical` is private).
    #[test]
    fn both_chords_are_consumed_by_the_real_router_and_queue_one_gesture_each() {
        const KEY_LEFTCTRL: u32 = 29;
        const KEY_LEFTSHIFT: u32 = 42;
        const KEY_INSERT: u32 = 110;

        for (mods, want) in [
            (vec![KEY_LEFTCTRL, KEY_LEFTSHIFT], ClipboardGesture::Promote),
            (vec![KEY_LEFTSHIFT], ClipboardGesture::Offer),
        ] {
            let sig = signal();
            let mut router = InputRouter::detached(ClipboardHook::new(Rc::clone(&sig), NoopHook));
            let bound = realm("realm-0");
            let _owed = router.bind_to(&bound);

            let mut delivered = Vec::new();
            let route = |router: &mut InputRouter<ClipboardHook<NoopHook>>,
                         evdev: u32,
                         state: KeyState,
                         delivered: &mut Vec<_>| {
                for input in crate::input::physical_key(evdev, None, state) {
                    if let Some(d) = router.route_physical(input, (640, 480), Some((640, 480))) {
                        delivered.push(d);
                    }
                }
            };
            for m in &mods {
                route(&mut router, *m, KeyState::Pressed, &mut delivered);
            }
            let before = delivered.len();
            route(&mut router, KEY_INSERT, KeyState::Pressed, &mut delivered);
            route(&mut router, KEY_INSERT, KeyState::Released, &mut delivered);
            assert_eq!(
                delivered.len(),
                before,
                "both halves of the chord are consumed: the app never learns it happened"
            );
            assert_eq!(
                delivered.len(),
                mods.len(),
                "the modifiers themselves are still delivered -- the app needs its Shift"
            );
            assert_eq!(sig.borrow_mut().take_pending(), vec![want]);
        }
    }

    #[test]
    fn a_detached_signal_carries_the_default_pair() {
        let sig = ClipboardSignal::detached();
        assert_eq!(sig.trigger(), DEFAULT_TRIGGER);
        let spellings: Vec<String> = sig.chords().map(|c| c.spelling()).collect();
        assert_eq!(spellings, vec!["ctrl+shift+insert", "shift+insert"]);
    }
}
