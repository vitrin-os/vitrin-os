// SPDX-License-Identifier: MPL-2.0
//! The lock's input half (WS-E.2.2, issue #214): the state a raised lock
//! judges physical input against, and the hook that attaches it to the router.
//!
//! # This gate consumes ALL physical input, which is why its shape is unusual
//!
//! Every other policy in the stack takes something narrow away from the
//! confined app: one chord, one key, one prompt's worth of clicks. This one
//! takes the whole device, for as long as the lock is up, and that inverts the
//! usual risk calculus. A bug in [`crate::attention`] costs a focus change that
//! did not happen. A bug here can wedge a session — and the specific bug that
//! matters is not "the lock does not lock", it is **"the lock swallowed the
//! human's off-switch"**, which would leave a human who cannot revoke strictly
//! worse off locked than unlocked and would invert the entire argument for
//! having a lock screen at all.
//!
//! So the off-switch's survival is not left to this file getting it right.
//!
//! # [`LockPolicy`] cannot touch the observe tap, structurally
//!
//! [`crate::deadman`] detects the chord in [`PreemptionHook::observe`]
//! ([`crate::input::PreemptionHook`]'s split), and every hook above it must
//! forward that tap unconditionally. The failure is one line to write —
//! `if locked { return; }` at the top of an `observe` body — and it is silent:
//! every test stays green and the human's off-switch simply stops working.
//!
//! [`LockPolicy`] therefore **does not implement [`PreemptionHook`]**. It
//! implements [`crate::input::ConsumingGate`], a trait with no observation
//! method, and [`crate::input::GateOnlyHook`] supplies the hook impl. The code
//! that forwards `observe` lives in `crate::input`, which has no `use
//! crate::lock` and no notion that a lock exists. An edit in *this* module
//! cannot make observation conditional, because the observing code is not
//! reachable from here and the trait it calls through cannot express an
//! observation. That is the #210/#232 shape — make the mistake unconstructible
//! rather than merely tested — and `the_dead_man_chord_arms_and_fires_through_a_locked_gate`
//! holds the consequence against the **real** stack rather than a stub.
//!
//! # Where it is stacked, and why issue #214's own sentence is now stale
//!
//! Issue #214 says to stack it outside the dead-man hook, and writes the stack
//! as `InputRouter<LockGate<ConsentGate<DeadManHook<NoopHook>>>>`. That
//! spelling predates the clipboard (WS-E.2.1) and attention (WS-E.1.7) hooks,
//! which now sit in it. The *intent* — nothing may be able to preempt the lock,
//! and the lock may not be able to preempt the off-switch's detection — is
//! preserved by putting it **outermost**:
//!
//! ```text
//! InputRouter<LockGate<ConsentGate<DeadManHook<ClipboardHook<AttentionHook<NoopHook>>>>>>
//! ```
//!
//! Outermost is the position each of the four consequences argues for:
//!
//! - **The dead-man switch is unaffected**, because its detection is in
//!   `observe` and this gate cannot stop an `observe` (above). Being outermost
//!   does mean the switch's *gate* half is short-circuited while locked, and
//!   that is correct and inert: that half exists only to keep the chord key
//!   from reaching the confined app, and while locked nothing reaches the
//!   confined app anyway.
//! - **A consent prompt cannot be answered while locked.** [`ConsentGate`]
//!   sits inside, so its `judge` never runs, nothing arms, and no click can
//!   commit a grant. A petition raised while the human is away resolves
//!   `timed_out` — refusal — which is the fail-closed direction and the one a
//!   lock screen must take. Inside the consent gate, the opposite would hold:
//!   a prompt would keep grabbing input *through* the lock and a click on the
//!   scrim would be answering a security question nobody was at the keyboard
//!   for.
//! - **No clipboard gesture and no attention press fires while locked**, for
//!   free, by the same short-circuit.
//! - **The chord matcher's modifier bits stay correct.** [`crate::chord`]'s
//!   rule 2 tracks modifiers in `observe` precisely because an outer hook can
//!   eat a release. This gate has no `observe`, so it tracks them in its own
//!   `judge` — sound *only because it is outermost*, where `gate` sees exactly
//!   the events `observe` does. That is load-bearing, so it is asserted rather
//!   than assumed: [`crate::backend::winit::NestedHook`] is the one place the
//!   stack is named, and `the_lock_gate_is_the_outermost_hook` pins it.
//!
//! # The pairing contract, re-asserted for a third gate
//!
//! [`crate::input::PreemptionHook`]'s razor: **a gate must not consume a
//! release whose press the router delivered.** P1.7.2 learned it the hard way —
//! the consent grab once consumed key releases, which left a confined app
//! holding whatever modifier was down when a prompt appeared, silently
//! rewriting every keystroke the human typed afterwards.
//!
//! A lock raises at a moment the human did not choose (an idle timer), so it
//! raises mid-drag and mid-`Shift` at least as often as a prompt does. The rule
//! here is therefore the same one, with no exceptions worth remembering:
//! **this gate never consumes a release.** Presses are consumed, so nothing the
//! human types at the lock screen reaches an app; releases are delivered, and
//! the router's per-button-code and per-keysym pairing drops the ones whose
//! press this gate ate. Nothing leaks, and nothing latches.
//!
//! The one apparent exception is not one: the lock **chord** consumes both
//! halves of its own pair, through [`crate::chord::ChordMatcher`], which is the
//! sound exception the contract names — the app never saw that press begin.
//!
//! # Agent input passes through untouched
//!
//! [`crate::consent::grab`]'s argument, verbatim, and it is the mechanical half
//! of the owner's decision recorded in D-025: an emulated event reaching this
//! gate has already been admitted by the enforcement chokepoint against a live
//! grant, so dropping it here would silently overturn an authority decision the
//! core just made — no refusal, no terminal, a spent rate-limit token. And
//! [`crate::input`]'s module docs make it a rule that the router holds no
//! authority check and must not grow one. A lock that suspended agents would be
//! a *wire* semantic, which is the protocol track's to define, not a gate's to
//! invent. This is published in `docs/book/src/limits.md` rather than left to
//! be discovered.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use vitrin_protocol::generated::vitrin_actuator_pointer::ButtonState;
use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

use crate::chord::{ChordMatcher, ModChord};
use crate::input::{ConsumingGate, Gate, GateOnlyHook, PreemptionHook, SeatInput, SeatInputKind};

use super::passphrase::{wipe, PassphraseFile, MAX_ATTEMPT_BYTES};
use super::{LockCause, UnlockMethod};

/// X11 keysyms of the three editing keys the lock screen understands.
/// From `keysymdef.h`, and all three are in [`crate::input::invariant_keysym`],
/// so they reach the core on any backend that has a keyboard at all.
const KEYSYM_RETURN: u32 = 0xff0d;
const KEYSYM_BACKSPACE: u32 = 0xff08;
const KEYSYM_ESCAPE: u32 = 0xff1b;

/// The `0x0100_0000 | codepoint` "Unicode keysym" range
/// ([`crate::input::host_keysym`]'s convention).
///
/// Re-exported from `crate::input` rather than spelled again here since
/// WS-E.3.1 (D-028(1)): three places depend on this convention now —
/// `host_keysym` encodes nested input into it, `crate::input::keymap`
/// normalises a real keymap's legacy keysyms into it, and [`printable`]
/// below decodes the passphrase out of it. A private copy is how the third
/// one would quietly stop agreeing with the first two.
use crate::input::UNICODE_KEYSYM_BASE;

/// What the lock chord means. A unit-like payload rather than `()` so the
/// matcher's type says what a match is, and so a second lock-related chord
/// (issue #216's screenshot key is *not* one) would have to be named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockGesture {
    /// Raise the lock now.
    Lock,
}

/// One fact the lock produced that the flight recorder owes an entry for.
///
/// The gate cannot journal: it runs inside
/// [`crate::input::InputRouter::route_physical`], which holds no recorder and
/// must not grow one — the same division [`crate::clipboard::ClipboardSignal`]
/// and [`crate::attention::AttentionSignal`] make. So it queues facts and
/// [`crate::session::service_lock_round`] writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockJournal {
    Locked {
        cause: LockCause,
    },
    /// **One entry per attempt, never a summary.** A failed unlock is a
    /// security fact and the count is the signal: three failures in a minute
    /// and three failures a day apart are different sessions, and a summarised
    /// "3 failures" cannot tell them apart.
    Attempted {
        accepted: bool,
    },
    Unlocked,
}

/// The lock's state: what the gate judges against and what the embedder drives.
///
/// Shared by `Rc<RefCell<..>>`, the shape every piece of hook-side state the
/// embedder also touches uses ([`crate::consent::grab::ConsentGrab`], the
/// dead-man switch, the router's presence record).
///
/// **No `Debug` derive.** [`LockedState::attempt`] holds the passphrase a human
/// is mid-way through typing, and a derived `Debug` would put it in any tracing
/// span, panic message or `dbg!` that ever touched this type. See [`Self::fmt`].
pub(crate) struct LockScreen {
    /// `None` when unlocked. There is no third state: a lock is up or it is
    /// not, and every question anyone asks of this module is answered by which.
    locked: Option<LockedState>,
    /// The manual lock chord, and the modifier state it is matched against.
    matcher: ChordMatcher<LockGesture>,
    /// The configured chord's spelling, rebuilt from the parsed parts, for the
    /// startup banner. Never echoed from an operator's typing.
    chord_spelling: String,
    /// How long without physical input before the lock raises itself; `None`
    /// disables the idle raise entirely (`--lock-idle` omitted).
    idle_after: Option<Duration>,
    /// When physical input was last seen, or — before the first event — when
    /// the session armed.
    ///
    /// **Seeded at construction rather than left `None`**, because "no input
    /// has ever arrived" and "no input has arrived recently" are the same
    /// situation for an idle lock, and a `None` that meant "never lock" would
    /// leave a session that came up in front of an empty chair unlocked
    /// forever. The fail-closed reading is the one that is also the obvious
    /// one.
    last_activity: Instant,
    /// Whether the seat has been taken away from this session — on bare metal,
    /// the human switched to another VT (D-030(7)).
    ///
    /// While true the idle clock does not run. **This is an owner decision,
    /// not a derivation** (Taha, 2026-08-09): switching away must not lock the
    /// session. Without it the behaviour is one nobody chose — physical input
    /// is suspended for the whole switch, so `last_activity` cannot advance,
    /// so a session with `--lock-idle` locks *during* the absence, at a moment
    /// no human could see, and the human returns to a passphrase prompt they
    /// did not ask for.
    ///
    /// The accepted cost is stated rather than hidden: a session switched away
    /// from for eight hours is unlocked when it is switched back to, because
    /// vitrind saw no idle time pass. A configurable policy — lock on switch,
    /// lock on idle, never — is filed as its own issue rather than guessed at
    /// here.
    ///
    /// Deliberately NOT a lowering: a lock already up stays up across a
    /// switch. This suppresses only the *raise*.
    seat_absent: bool,
    /// The stored digest an attempt is checked against, or `None` for a
    /// privacy screen with no authentication ([`UnlockMethod`]).
    verifier: Option<PassphraseFile>,
    /// Facts owed to the flight recorder, drained once per dispatch round.
    journal: Vec<LockJournal>,
}

impl std::fmt::Debug for LockScreen {
    /// Deliberately hand-written and deliberately incomplete: it reports
    /// whether a lock is up and how it got there, and **nothing about the
    /// attempt** — not its bytes and not its length. Length is metadata rather
    /// than key material, and it is omitted anyway, because the way a
    /// passphrase ends up in a log is one field at a time.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockScreen")
            .field("locked", &self.locked.as_ref().map(|l| l.cause))
            .field("chord", &self.chord_spelling)
            .field("idle_after", &self.idle_after)
            .field("passphrase", &self.verifier.is_some())
            .field("journal_pending", &self.journal.len())
            .finish_non_exhaustive()
    }
}

/// A raised lock: why it went up, and what the human has typed at it so far.
struct LockedState {
    cause: LockCause,
    /// The passphrase being typed. Wiped on every path that ends an attempt
    /// **and** on drop, so an unlock does not leave the passphrase resident in
    /// the trusted core for the rest of the session.
    attempt: Vec<u8>,
}

impl Drop for LockedState {
    /// The last of the three wipe paths, and the one that catches the others'
    /// mistakes: submit wipes, Escape wipes, and this catches an unlock, a
    /// shutdown, or any future path that simply drops the state.
    fn drop(&mut self) {
        wipe(&mut self.attempt);
    }
}

impl LockScreen {
    /// Arm the lock: a chord, an optional idle timeout, an optional verifier.
    ///
    /// `now` seeds the idle clock (see [`Self::last_activity`]).
    pub fn new(
        chord: ModChord,
        idle_after: Option<Duration>,
        verifier: Option<PassphraseFile>,
        now: Instant,
    ) -> Self {
        Self {
            locked: None,
            chord_spelling: chord.spelling(),
            // `ChordMatcher::new` only refuses duplicate bindings; one binding
            // cannot duplicate itself, so this is infallible by construction.
            matcher: ChordMatcher::new(vec![(chord, LockGesture::Lock)])
                .expect("a single binding cannot collide with itself"),
            idle_after,
            last_activity: now,
            seat_absent: false,
            verifier,
            journal: Vec::new(),
        }
    }

    /// Tell the gate whether this session still holds the seat (D-030(7)).
    ///
    /// Called from the bare-metal backend's `PauseSession` / `ActivateSession`
    /// handler and nowhere else — nested has no equivalent, because a host
    /// compositor's focus loss is not a seat loss and the human can still see
    /// the window.
    ///
    /// Returning advances the activity clock, so the idle countdown restarts
    /// from the moment the human comes back rather than resuming a count that
    /// was frozen mid-way. A human who returns to their own screen has, by
    /// returning, done the one thing the idle timer is asking about.
    pub(crate) fn set_seat_absent(&mut self, absent: bool, now: Instant) {
        if !absent && self.seat_absent {
            self.last_activity = now;
        }
        self.seat_absent = absent;
    }

    /// Whether a lock is up, and why.
    pub fn cause(&self) -> Option<LockCause> {
        self.locked.as_ref().map(|l| l.cause)
    }

    pub fn is_locked(&self) -> bool {
        self.locked.is_some()
    }

    /// How this session's lock is answered — drawn on the card, so the human is
    /// told which of the two things they are looking at.
    pub fn unlock_method(&self) -> UnlockMethod {
        match self.verifier {
            Some(_) => UnlockMethod::Passphrase,
            None => UnlockMethod::AnyKey,
        }
    }

    /// The configured chord's spelling, for the startup banner.
    pub fn chord_spelling(&self) -> &str {
        &self.chord_spelling
    }

    /// Drain the facts the recorder owes entries for, oldest first.
    pub fn take_journal(&mut self) -> Vec<LockJournal> {
        std::mem::take(&mut self.journal)
    }

    /// **Test seam.** Queue a fact as if the gate had produced it.
    ///
    /// The gate's own path from a keystroke to a `LockJournal` is covered
    /// exhaustively by this module's tests. This seam exists for the
    /// *embedder*-level round instead, so `session::service_lock_round`'s job —
    /// mirroring the gate onto the surface and turning facts into recorder
    /// entries — can be driven without also re-testing the state machine that
    /// produced them.
    #[cfg(test)]
    pub(crate) fn journal_for_test(&mut self, entry: LockJournal) {
        self.journal.push(entry);
    }

    /// Raise the lock, if it is not already up. Returns whether it changed.
    ///
    /// The only ways in are this and the chord; both funnel here so "the lock
    /// went up" and "the journal says so" cannot drift apart.
    pub fn raise(&mut self, cause: LockCause) -> bool {
        if self.locked.is_some() {
            return false;
        }
        self.locked = Some(LockedState {
            cause,
            // Sized to the cap up front. Growing one keystroke at a time
            // reallocates ~8 times on the way to 512 bytes, and every
            // reallocation leaves a plaintext PREFIX of the passphrase in
            // freed heap that `wipe` can never reach -- it only ever sees the
            // live buffer. The cap is a compile-time constant, so this is
            // removable rather than inherent.
            attempt: Vec::with_capacity(MAX_ATTEMPT_BYTES),
        });
        self.journal.push(LockJournal::Locked { cause });
        true
    }

    /// Raise the lock if the session has been idle past its configured
    /// timeout. Returns whether it changed.
    ///
    /// Driven by the embedder once per dispatch round rather than by a timer
    /// source, for the reason [`crate::petitions::PetitionRegistry::expire_due`]
    /// is: the round already samples one instant, and a second clock would be a
    /// second thing to keep in step. The cost is that a completely idle session
    /// raises on the next event the loop wakes for; the session's own sweep
    /// timer bounds that.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.locked.is_some() {
            return false;
        }
        // The seat is somebody else's right now (D-030(7)). Time the human
        // spends on another VT is not idle time, and counting it would lock
        // the session at an instant nobody could observe.
        if self.seat_absent {
            return false;
        }
        let Some(after) = self.idle_after else {
            return false;
        };
        if now.saturating_duration_since(self.last_activity) < after {
            return false;
        }
        self.raise(LockCause::Idle)
    }

    /// Judge one intake event — the whole gate policy, in one place.
    ///
    /// See the module docs for the pairing contract, the origin check and why
    /// the chord matcher's tracking half runs here rather than in an `observe`
    /// this type deliberately does not have.
    fn judge(&mut self, input: &SeatInput, now: Instant) -> Gate {
        // Agent input is the chokepoint's business, not the router's (module
        // docs). Above everything, including the activity clock: an agent's
        // actuations must not hold the idle lock open for a human who left.
        if input.origin() != Origin::Physical {
            return Gate::Deliver;
        }
        self.last_activity = now;

        // Modifier tracking, then the match. Sound here — rather than in an
        // `observe` — only because this gate is outermost; see the module docs
        // and `the_lock_gate_is_the_outermost_hook`.
        self.matcher.observe(input);
        let (chord_gate, gesture) = self.matcher.gate(input);
        if let Some(LockGesture::Lock) = gesture {
            // Already locked: the chord is inert rather than a toggle. A chord
            // that unlocked would be an unlock with no authentication at all,
            // which is the one thing a lock screen may not have.
            self.raise(LockCause::Chord);
            return Gate::Consume;
        }
        if chord_gate == Gate::Consume {
            // The release of a press this matcher consumed — the contract's one
            // sound exception, taken by the matcher and not by this file.
            return Gate::Consume;
        }
        if self.locked.is_none() {
            return Gate::Deliver;
        }

        match input.kind() {
            // Releases are ALWAYS delivered (module docs: the pairing
            // contract). The router drops the ones whose press this gate ate,
            // so nothing leaks and nothing latches.
            SeatInputKind::Key {
                state: KeyState::Released,
                ..
            }
            | SeatInputKind::Button {
                state: ButtonState::Released,
                ..
            } => Gate::Deliver,
            SeatInputKind::Key {
                keysym,
                state: KeyState::Pressed,
                ..
            } => {
                self.type_key(*keysym);
                Gate::Consume
            }
            // Motion, scroll, button presses and text. Exhaustive by intent: a
            // new input kind must be classified here rather than defaulting to
            // reaching an app behind a locked screen.
            SeatInputKind::Motion { .. }
            | SeatInputKind::Scroll { .. }
            | SeatInputKind::Button { .. }
            | SeatInputKind::Text { .. } => Gate::Consume,
        }
    }

    /// Fold one consumed key press into the passphrase attempt.
    ///
    /// Three editing keys and the printable alphabet; everything else is
    /// ignored (still consumed — it does not reach the app either). Note what
    /// is deliberately absent: no key cancels the lock, and no key count
    /// unlocks it.
    fn type_key(&mut self, keysym: u32) {
        let submit = {
            let Some(locked) = self.locked.as_mut() else {
                return;
            };
            match keysym {
                KEYSYM_RETURN => true,
                KEYSYM_BACKSPACE => {
                    // Pop one whole UTF-8 character, not one byte: popping a
                    // byte off a multi-byte character would leave a partial
                    // sequence the human cannot see and cannot delete.
                    while locked.attempt.pop().is_some_and(|b| b & 0xc0 == 0x80) {}
                    false
                }
                KEYSYM_ESCAPE => {
                    // Clear, not cancel. Escape is also the dead-man chord's
                    // default key, and the two do not collide: the switch
                    // detects in `observe`, which this gate cannot stop, so a
                    // *held* Escape still revokes every grant while a *tapped*
                    // one clears the field.
                    wipe(&mut locked.attempt);
                    locked.attempt.clear();
                    false
                }
                _ => {
                    if let Some(ch) = printable(keysym) {
                        let mut buf = [0u8; 4];
                        let encoded = ch.encode_utf8(&mut buf);
                        if locked.attempt.len() + encoded.len() <= MAX_ATTEMPT_BYTES {
                            locked.attempt.extend_from_slice(encoded.as_bytes());
                        }
                        wipe(&mut buf);
                    }
                    false
                }
            }
        };
        if submit {
            self.submit();
        }
    }

    /// Check the typed attempt and, if it is right, take the lock down.
    ///
    /// **The verification runs here, inside the dispatch turn**, and that is a
    /// real cost stated rather than hidden: one Argon2id derivation at the
    /// configured parameters blocks the compositor for tens of milliseconds.
    /// It is accepted because the alternative — handing the attempt to another
    /// thread — would put the passphrase in a second place, and because a human
    /// who has just pressed Enter at a lock screen is not also mid-gesture in
    /// an app.
    ///
    /// With no verifier configured, any submission is accepted: the session is
    /// running a privacy screen, the card says so in as many words
    /// ([`crate::lock::render`]'s `UNLOCK_ANY`), and pretending otherwise would
    /// be the dishonest direction.
    fn submit(&mut self) {
        let Some(locked) = self.locked.as_mut() else {
            return;
        };
        let accepted = match &self.verifier {
            Some(verifier) => verifier.verify(&locked.attempt),
            None => true,
        };
        wipe(&mut locked.attempt);
        locked.attempt.clear();
        // Journaled before the state change, and journaled for **every**
        // attempt including the accepted one: "how many times did somebody try"
        // is the question this entry exists to answer, and an accepted attempt
        // with no entry would make a session with one wrong guess look
        // identical to one with none.
        self.journal.push(LockJournal::Attempted { accepted });
        if accepted {
            self.locked = None;
            self.journal.push(LockJournal::Unlocked);
        }
    }
}

/// Which character, if any, a keysym types.
///
/// The `keysymdef.h` convention ([`crate::input::host_keysym`]): codepoints
/// below `0x100` are their own keysym, everything else is
/// `0x0100_0000 | codepoint`. Control characters and the function-key range
/// (`0xff00..`) yield nothing.
fn printable(keysym: u32) -> Option<char> {
    let code = if keysym < 0x100 {
        keysym
    } else if keysym & 0xff00_0000 == UNICODE_KEYSYM_BASE {
        keysym & 0x00ff_ffff
    } else {
        return None;
    };
    let ch = char::from_u32(code)?;
    (!ch.is_control()).then_some(ch)
}

/// The lock's consuming half, attached at the router's preemption hook point
/// through [`GateOnlyHook`].
///
/// It holds no observe tap and cannot be given one — see the module docs for
/// what that buys and why it is the point of the type rather than an accident
/// of it.
pub(crate) struct LockPolicy {
    screen: Rc<RefCell<LockScreen>>,
    /// The dispatch turn's instant, shared with the embedder — the same cell
    /// the consent grab, the dead-man watcher and the router's presence record
    /// read, because the hook trait deliberately carries no clock.
    now: Rc<Cell<Instant>>,
}

impl LockPolicy {
    pub fn new(screen: Rc<RefCell<LockScreen>>, now: Rc<Cell<Instant>>) -> Self {
        Self { screen, now }
    }
}

impl ConsumingGate for LockPolicy {
    fn judge(&mut self, input: &SeatInput) -> Gate {
        self.screen.borrow_mut().judge(input, self.now.get())
    }
}

/// The lock at the router's hook point: [`LockPolicy`] wrapped so it gates and
/// nothing else.
pub(crate) type LockGate<H> = GateOnlyHook<LockPolicy, H>;

/// Build the hook, on the router's own clock cell.
pub(crate) fn lock_gate<H: PreemptionHook>(
    screen: Rc<RefCell<LockScreen>>,
    now: Rc<Cell<Instant>>,
    inner: H,
) -> LockGate<H> {
    GateOnlyHook::new(LockPolicy::new(screen, now), inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::Trigger;
    use crate::input::{InputRouter, KeySource, NoopHook, SeatInputKind};

    fn chord() -> ModChord {
        ModChord::parse("ctrl+alt+delete").expect("the default lock chord parses")
    }

    fn screen(idle: Option<Duration>, now: Instant) -> LockScreen {
        LockScreen::new(chord(), idle, None, now)
    }

    /// One physical event, built through the crate-visible emulated
    /// constructor and re-tagged by the router's own test seam. The lock's own
    /// tests judge through `judge_for_test`, which takes the whole `SeatInput`,
    /// so they need a physical one: `crate::input::tests::physical_for_test` is
    /// the shared way to get one without a physical-origin constructor leaking
    /// out of intake.
    fn key(keysym: u32, state: KeyState) -> SeatInput {
        crate::input::tests::physical_for_test(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym,
            state,
        })
    }

    fn press(keysym: u32) -> SeatInput {
        key(keysym, KeyState::Pressed)
    }

    fn release(keysym: u32) -> SeatInput {
        key(keysym, KeyState::Released)
    }

    #[test]
    fn an_idle_session_locks_itself_and_a_busy_one_does_not() {
        let t0 = Instant::now();
        let mut s = screen(Some(Duration::from_secs(60)), t0);
        assert!(!s.tick(t0 + Duration::from_secs(59)));
        assert!(!s.is_locked());
        assert!(s.tick(t0 + Duration::from_secs(60)));
        assert!(s.is_locked());
        assert_eq!(s.cause(), Some(LockCause::Idle));
        assert_eq!(
            s.take_journal(),
            vec![LockJournal::Locked {
                cause: LockCause::Idle
            }]
        );

        // Physical input pushes the deadline out.
        let mut s = screen(Some(Duration::from_secs(60)), t0);
        s.judge(&press(KEYSYM_RETURN), t0 + Duration::from_secs(59));
        assert!(!s.tick(t0 + Duration::from_secs(118)));
        assert!(s.tick(t0 + Duration::from_secs(119)));
    }

    #[test]
    fn an_agents_actuation_never_holds_the_idle_lock_open() {
        // The subtler half of the origin check: an agent working through the
        // night must not keep a session unlocked for a human who went home.
        let t0 = Instant::now();
        let mut s = screen(Some(Duration::from_secs(60)), t0);
        let emulated = SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: 0x61,
            state: KeyState::Pressed,
        });
        for i in 0..120 {
            assert_eq!(
                s.judge(&emulated, t0 + Duration::from_secs(i)),
                Gate::Deliver,
                "an agent's admitted actuation is the chokepoint's business"
            );
        }
        assert!(s.tick(t0 + Duration::from_secs(60)));
    }

    /// **Switching to another VT does not lock the session** (D-030(7)).
    ///
    /// An owner decision, not a derivation (Taha, 2026-08-09), and the
    /// behaviour it replaces is one nobody chose. Physical input is suspended
    /// for the whole absence, so `last_activity` cannot advance — a session
    /// with `--lock-idle` would therefore lock *during* the switch-away, at an
    /// instant no human could observe, and the human would return to a
    /// passphrase prompt they never asked for.
    ///
    /// The accepted cost is asserted here too, so it cannot be quietly
    /// changed: an eight-hour absence returns to an unlocked screen.
    #[test]
    fn a_seat_taken_away_stops_the_idle_clock_and_returning_restarts_it() {
        let t0 = Instant::now();
        let mut s = screen(Some(Duration::from_secs(60)), t0);

        s.set_seat_absent(true, t0 + Duration::from_secs(1));
        // Well past the deadline, and past any plausible absence.
        assert!(
            !s.tick(t0 + Duration::from_secs(8 * 60 * 60)),
            "a session must not lock itself while the human is on another VT: nobody could see \
             it happen, and they would come back to a prompt they did not ask for"
        );
        assert!(!s.is_locked());

        // Coming back restarts the countdown rather than resuming a frozen
        // one -- returning to your own screen is what an idle timer asks about.
        let back = t0 + Duration::from_secs(8 * 60 * 60);
        s.set_seat_absent(false, back);
        assert!(
            !s.tick(back + Duration::from_secs(59)),
            "the countdown must restart from the return, not resume mid-way"
        );
        assert!(s.tick(back + Duration::from_secs(60)));
        assert_eq!(s.cause(), Some(LockCause::Idle));
    }

    /// A lock already up is **not** lowered by losing the seat. The absence
    /// suppresses the raise; it is not an unlock, and a VT switch must never
    /// be a way past a lock screen.
    #[test]
    fn losing_the_seat_never_lowers_a_lock_that_is_already_up() {
        let t0 = Instant::now();
        let mut s = screen(Some(Duration::from_secs(60)), t0);
        assert!(s.tick(t0 + Duration::from_secs(60)));
        assert!(s.is_locked());

        s.set_seat_absent(true, t0 + Duration::from_secs(61));
        assert!(
            s.is_locked(),
            "a VT switch must not unlock a locked session"
        );
        s.set_seat_absent(false, t0 + Duration::from_secs(999));
        assert!(
            s.is_locked(),
            "and coming back must still meet the lock the human left up"
        );
    }

    #[test]
    fn no_idle_timeout_means_no_idle_raise_however_long_the_session_sits() {
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        assert!(!s.tick(t0 + Duration::from_secs(86_400)));
        assert!(!s.is_locked());
    }

    #[test]
    fn the_chord_raises_the_lock_and_is_inert_once_it_is_up() {
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        // ctrl+alt+delete: the two modifiers are delivered, the trigger is not.
        assert_eq!(s.judge(&press(0xffe3), t0), Gate::Deliver);
        assert_eq!(s.judge(&press(0xffe9), t0), Gate::Deliver);
        assert_eq!(s.judge(&press(0xffff), t0), Gate::Consume);
        assert!(s.is_locked());
        assert_eq!(s.cause(), Some(LockCause::Chord));
        // The trigger's release is consumed too -- the matcher's own sound
        // exception, because it consumed the press.
        assert_eq!(s.judge(&release(0xffff), t0), Gate::Consume);
        // Pressed again while locked: still consumed, still locked, and NOT a
        // toggle. An unlock with no authentication is the one thing this
        // surface may not offer.
        assert_eq!(s.judge(&press(0xffff), t0), Gate::Consume);
        assert!(s.is_locked());
        assert_eq!(
            s.take_journal(),
            vec![LockJournal::Locked {
                cause: LockCause::Chord
            }],
            "a second chord press must not journal a second lock"
        );
    }

    #[test]
    fn the_trigger_alone_is_not_the_chord() {
        // Exact modifier-set equality (`crate::chord` rule 3): a bare Delete is
        // an ordinary key an app keeps.
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        assert_eq!(s.judge(&press(0xffff), t0), Gate::Deliver);
        assert!(!s.is_locked());
        // ...and so is ctrl+delete without alt.
        s.judge(&press(0xffe3), t0);
        assert_eq!(s.judge(&press(0xffff), t0), Gate::Deliver);
        assert!(!s.is_locked());
    }

    #[test]
    fn an_agent_cannot_chord_the_lock_up() {
        // `ChordMatcher`'s rule 1, re-asserted at this gate: a principal
        // holding `actuate_text` must not be able to lock the human's session.
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        for keysym in [0xffe3, 0xffe9, 0xffff] {
            let emulated = SeatInput::emulated(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym,
                state: KeyState::Pressed,
            });
            assert_eq!(s.judge(&emulated, t0), Gate::Deliver);
        }
        assert!(!s.is_locked());
    }

    #[test]
    fn a_locked_screen_consumes_presses_and_delivers_every_release() {
        // THE pairing contract, for the third gate in this stack. A release
        // this gate consumed would strand its press in the confined app -- the
        // P1.7.2 regression, one gate over.
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        s.raise(LockCause::Chord);
        assert_eq!(s.judge(&press(0x61), t0), Gate::Consume);
        assert_eq!(s.judge(&release(0x61), t0), Gate::Deliver);
        // Modifiers especially: a latched Shift rewrites everything typed after.
        assert_eq!(s.judge(&press(0xffe1), t0), Gate::Consume);
        assert_eq!(s.judge(&release(0xffe1), t0), Gate::Deliver);
        // Pointer: press consumed, release delivered.
        let btn = |state| {
            crate::input::tests::physical_for_test(SeatInputKind::Button {
                button: 0x110,
                state,
            })
        };
        assert_eq!(s.judge(&btn(ButtonState::Pressed), t0), Gate::Consume);
        assert_eq!(s.judge(&btn(ButtonState::Released), t0), Gate::Deliver);
        // Motion and scroll are consumed: an app that tracks hover must not
        // learn the path a human traced across a lock screen.
        assert_eq!(
            s.judge(
                &crate::input::tests::physical_for_test(SeatInputKind::Motion { x: 1.0, y: 2.0 }),
                t0
            ),
            Gate::Consume
        );
    }

    #[test]
    fn a_modifier_held_when_the_lock_raises_is_released_in_the_app() {
        // The P1.7.2 regression, re-asserted for this gate through the REAL
        // router: Shift goes down while unlocked (delivered, so the app is
        // holding it), the lock raises, and the release must still reach the
        // app or the app is left with a latched modifier for the rest of the
        // session.
        let t0 = Instant::now();
        let screen = Rc::new(RefCell::new(super::LockScreen::new(
            chord(),
            None,
            None,
            t0,
        )));
        let now = Rc::new(Cell::new(t0));
        let mut router =
            InputRouter::detached(lock_gate(Rc::clone(&screen), Rc::clone(&now), NoopHook));
        let realm = crate::grants::RealmId::new("realm-0");
        assert!(
            router.bind_to(&realm).is_none(),
            "the first bind leaves no previous realm owed a release"
        );
        let view = (100, 100);
        let surface = Some((100, 100));

        let down = router.route_physical(press(0xffe1), view, surface);
        assert!(down.is_some(), "an unlocked session delivers the press");

        screen.borrow_mut().raise(LockCause::Idle);

        let up = router.route_physical(release(0xffe1), view, surface);
        assert!(
            up.is_some(),
            "the release of a press the app already saw MUST reach the app; consuming it \
             latches the modifier in the confined app for the rest of the session"
        );
    }

    #[test]
    fn typing_the_right_passphrase_unlocks_and_a_wrong_one_does_not() {
        let t0 = Instant::now();
        let file = super::super::tests::cheap_verifier(b"hunter2");
        let mut s = LockScreen::new(chord(), None, Some(file), t0);
        s.raise(LockCause::Idle);
        let _ = s.take_journal();

        // A wrong attempt: one entry, still locked.
        for ch in "hunter3".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_RETURN), t0);
        assert!(s.is_locked());
        assert_eq!(
            s.take_journal(),
            vec![LockJournal::Attempted { accepted: false }]
        );

        // N wrong attempts write N entries -- never a summary.
        for _ in 0..3 {
            s.judge(&press(0x78), t0);
            s.judge(&press(KEYSYM_RETURN), t0);
        }
        assert_eq!(
            s.take_journal(),
            vec![
                LockJournal::Attempted { accepted: false },
                LockJournal::Attempted { accepted: false },
                LockJournal::Attempted { accepted: false },
            ]
        );
        assert!(s.is_locked());

        // The right one unlocks, and journals both facts.
        for ch in "hunter2".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_RETURN), t0);
        assert!(!s.is_locked());
        assert_eq!(
            s.take_journal(),
            vec![
                LockJournal::Attempted { accepted: true },
                LockJournal::Unlocked
            ]
        );
    }

    #[test]
    fn a_failed_attempt_leaves_nothing_of_itself_behind() {
        // The next attempt must start empty: a wrong guess whose bytes stayed
        // in the buffer would make the following correct passphrase fail, and
        // would leave the wrong one resident in the core.
        let t0 = Instant::now();
        let file = super::super::tests::cheap_verifier(b"ok");
        let mut s = LockScreen::new(chord(), None, Some(file), t0);
        s.raise(LockCause::Idle);
        for ch in "no".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_RETURN), t0);
        for ch in "ok".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_RETURN), t0);
        assert!(!s.is_locked(), "the second, correct attempt must unlock");
    }

    #[test]
    fn backspace_and_escape_edit_the_attempt() {
        let t0 = Instant::now();
        let file = super::super::tests::cheap_verifier(b"ab");
        let mut s = LockScreen::new(chord(), None, Some(file), t0);
        s.raise(LockCause::Idle);
        // "abx", backspace, Enter -> "ab" -> unlocked.
        for ch in "abx".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_BACKSPACE), t0);
        s.judge(&press(KEYSYM_RETURN), t0);
        assert!(!s.is_locked());

        // Escape clears: "zz", Escape, "ab", Enter -> unlocked.
        s.raise(LockCause::Idle);
        for ch in "zz".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_ESCAPE), t0);
        for ch in "ab".chars() {
            s.judge(&press(ch as u32), t0);
        }
        s.judge(&press(KEYSYM_RETURN), t0);
        assert!(!s.is_locked());
    }

    #[test]
    fn an_agent_cannot_type_at_the_lock_screen() {
        // The gate forwards emulated events untouched, so an agent's
        // `actuate_text` never reaches the attempt buffer -- an agent that
        // could type here would be an agent that could brute-force the
        // human's passphrase at wire speed.
        let t0 = Instant::now();
        let file = super::super::tests::cheap_verifier(b"a");
        let mut s = LockScreen::new(chord(), None, Some(file), t0);
        s.raise(LockCause::Idle);
        let _ = s.take_journal();
        for keysym in [0x61u32, KEYSYM_RETURN] {
            let emulated = SeatInput::emulated(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym,
                state: KeyState::Pressed,
            });
            assert_eq!(s.judge(&emulated, t0), Gate::Deliver);
        }
        assert!(s.is_locked(), "an agent's keystrokes must not unlock");
        assert!(s.take_journal().is_empty(), "and must not even be attempts");
    }

    #[test]
    fn with_no_passphrase_configured_enter_dismisses_a_privacy_screen() {
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        assert_eq!(s.unlock_method(), UnlockMethod::AnyKey);
        s.raise(LockCause::Idle);
        let _ = s.take_journal();
        s.judge(&press(0x61), t0);
        assert!(s.is_locked(), "an ordinary key does not dismiss it");
        s.judge(&press(KEYSYM_RETURN), t0);
        assert!(!s.is_locked());
        assert_eq!(
            s.take_journal(),
            vec![
                LockJournal::Attempted { accepted: true },
                LockJournal::Unlocked
            ]
        );
    }

    #[test]
    fn the_attempt_is_bounded() {
        let t0 = Instant::now();
        let file = super::super::tests::cheap_verifier(b"x");
        let mut s = LockScreen::new(chord(), None, Some(file), t0);
        s.raise(LockCause::Idle);
        for _ in 0..(MAX_ATTEMPT_BYTES * 4) {
            s.judge(&press(0x61), t0);
        }
        assert!(
            s.locked.as_ref().unwrap().attempt.len() <= MAX_ATTEMPT_BYTES,
            "a held key must not grow an unbounded buffer inside the TCB"
        );
    }

    #[test]
    fn the_debug_impl_carries_no_passphrase_material() {
        let t0 = Instant::now();
        let mut s = screen(None, t0);
        s.raise(LockCause::Idle);
        for ch in "seekrit".chars() {
            s.judge(&press(ch as u32), t0);
        }
        let rendered = format!("{s:?}");
        assert!(!rendered.contains("seekrit"), "{rendered}");
        // ...and not its length either.
        assert!(!rendered.contains('7'), "{rendered}");
    }

    #[test]
    fn printable_decodes_the_two_keysym_conventions_and_refuses_the_rest() {
        assert_eq!(printable(0x61), Some('a'));
        assert_eq!(printable(0x20), Some(' '));
        assert_eq!(printable(0xe9), Some('é'));
        assert_eq!(printable(0x0100_0000 | 0x2603), Some('☃'));
        // Function/editing keys and control codes type nothing.
        assert_eq!(printable(KEYSYM_RETURN), None);
        assert_eq!(printable(0xff52), None);
        assert_eq!(printable(0x0d), None);
        assert_eq!(printable(0x0100_0000 | 0x11_0000), None);
    }

    /// **THE adversarial test this whole issue turns on** (issue #214
    /// acceptance criterion 3): the human's off-switch still arms and still
    /// fires while the lock gate is consuming every single physical event —
    /// driven through the **real** `InputRouter<LockGate<ConsentGate<DeadManHook<
    /// ClipboardHook<AttentionHook<NoopHook>>>>>>`, not a stub, not a spy, and
    /// not the two hooks in isolation.
    ///
    /// A lock that could swallow, delay or split the dead-man chord would leave
    /// a human who cannot revoke strictly *worse off* locked than unlocked,
    /// which inverts the entire argument for shipping a lock screen. So this is
    /// written adversarially: the lock is raised **before** the chord is
    /// touched, a consent prompt is up underneath it (so `ConsentGate` would be
    /// consuming too, if it were ever reached), and the assertions are that the
    /// switch *arms*, that it *completes*, and that the trigger `apply` needs is
    /// really there.
    ///
    /// Reverting `GateOnlyHook::observe`'s unconditional forward — the one line
    /// this test exists for — turns it red at `arm_state`.
    #[test]
    fn the_dead_man_chord_arms_and_fires_through_a_locked_gate() {
        use crate::consent::grab::{ConsentGate, ConsentGrab};
        use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch, DEFAULT_HOLD};

        let t0 = Instant::now();
        let now = Rc::new(Cell::new(t0));
        let screen = Rc::new(RefCell::new(LockScreen::new(chord(), None, None, t0)));
        let switch = Rc::new(RefCell::new(DeadManSwitch::new(DeadManConfig::default())));
        let grab = Rc::new(RefCell::new(ConsentGrab::new()));

        // The production stack, in the production order. `NestedHook`'s doc
        // comment is the statement of that order; this is it built.
        let mut router = InputRouter::detached(lock_gate(
            Rc::clone(&screen),
            Rc::clone(&now),
            ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                DeadManHook::new(
                    Rc::clone(&switch),
                    Rc::clone(&now),
                    crate::clipboard::ClipboardHook::new(
                        Rc::new(RefCell::new(crate::clipboard::ClipboardSignal::detached())),
                        crate::attention::AttentionHook::new(
                            Rc::new(RefCell::new(crate::attention::AttentionSignal::detached())),
                            Rc::clone(&now),
                            NoopHook,
                        ),
                    ),
                ),
            ),
        ));
        let realm = crate::grants::RealmId::new("realm-0");
        assert!(router.bind_to(&realm).is_none());
        let view = (640, 480);

        // The session locks. From here on the lock consumes everything.
        screen.borrow_mut().raise(LockCause::Chord);
        assert!(screen.borrow().is_locked());

        // A sanity control FIRST, so the assertions below cannot pass because
        // nothing is being consumed at all: an ordinary key reaches no app.
        assert!(
            router
                .route_physical(press(0x61), view, Some(view))
                .is_none(),
            "the lock must be consuming physical input for this test to mean anything"
        );

        // The human holds the dead-man chord. Its press is consumed by the
        // lock (the app sees nothing), and the switch arms ANYWAY, because
        // detection rides `observe` and the lock has no `observe` to make
        // conditional.
        assert!(
            router
                .route_physical(crate::input::tests::chord_press(), view, Some(view))
                .is_none(),
            "the chord's press does not reach the app while locked"
        );
        assert_eq!(
            switch.borrow().deadline(),
            Some(t0 + DEFAULT_HOLD),
            "THE property: a locked gate must not be able to blind the human's off-switch. \
             If this fails, a human who locked their screen can no longer revoke an agent's \
             authority, which is strictly worse than not locking at all."
        );
        assert!(
            switch
                .borrow()
                .hold_progress(t0 + DEFAULT_HOLD / 2)
                .is_some(),
            "...and the hold really is in progress, so the indicator can be painted"
        );

        // ...and it completes on the same clock the rest of the turn uses.
        let due = t0 + DEFAULT_HOLD;
        now.set(due);
        router.observe_at(due);
        switch.borrow_mut().fire_if_due(due);
        let trigger = switch
            .borrow_mut()
            .take_trigger()
            .expect("a completed hold must fire while the session is locked");
        assert_eq!(trigger.held, DEFAULT_HOLD);

        // The lock is still up afterwards, and that is correct: the off-switch
        // revokes AUTHORITY, it does not prove a human is present. A dead-man
        // trigger that unlocked the screen would be a way past the lock.
        assert!(
            screen.borrow().is_locked(),
            "revoking every grant must not unlock the session"
        );
    }

    /// The same stack, the other direction: while the lock is up, the hooks
    /// *below* it are short-circuited — no clipboard gesture, no attention
    /// window, no consent decision.
    ///
    /// Not a smoke test of the same thing: it pins the consequence of being
    /// **outermost**, which is what makes "nobody who is not there can answer a
    /// security question" true. Moving `LockGate` inside `ConsentGate` turns
    /// this red on the clipboard assertion.
    #[test]
    fn a_locked_gate_short_circuits_every_hook_below_it() {
        use crate::clipboard::{ClipboardHook, ClipboardSignal};

        let t0 = Instant::now();
        let now = Rc::new(Cell::new(t0));
        let screen = Rc::new(RefCell::new(LockScreen::new(chord(), None, None, t0)));
        let signal = Rc::new(RefCell::new(ClipboardSignal::detached()));
        let mut hook = lock_gate(
            Rc::clone(&screen),
            Rc::clone(&now),
            ClipboardHook::new(Rc::clone(&signal), NoopHook),
        );

        // Unlocked: the clipboard chord fires, which is what makes the locked
        // half below a real difference rather than a vacuous one.
        let insert = |state| {
            crate::input::tests::physical_for_test(SeatInputKind::Key {
                source: KeySource::Keysym,
                keysym: 0xff63,
                state,
            })
        };
        for keysym in [0xffe3u32, 0xffe1] {
            hook.observe(&press(keysym));
            hook.gate(&press(keysym));
        }
        hook.observe(&insert(KeyState::Pressed));
        assert_eq!(hook.gate(&insert(KeyState::Pressed)), Gate::Consume);
        assert_eq!(
            signal.borrow_mut().take_pending().len(),
            1,
            "the chord must work while unlocked, or the locked case proves nothing"
        );
        hook.observe(&insert(KeyState::Released));
        hook.gate(&insert(KeyState::Released));

        // Locked: the identical gesture produces nothing.
        screen.borrow_mut().raise(LockCause::Chord);
        hook.observe(&insert(KeyState::Pressed));
        assert_eq!(hook.gate(&insert(KeyState::Pressed)), Gate::Consume);
        assert!(
            signal.borrow_mut().take_pending().is_empty(),
            "a clipboard gesture must not fire while the session is locked"
        );
    }

    #[test]
    fn the_trigger_vocabulary_still_holds_the_default_lock_chords_key() {
        // `Trigger::parse` cross-checks the vocabulary against
        // `invariant_keysym`, so this fails if the default chord's key ever
        // stops being deliverable -- the fail-closed posture
        // `deadman::Chord::parse` sets.
        assert!(Trigger::parse("delete").is_ok());
    }
}
