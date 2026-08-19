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
use super::{LockCause, SeatChangePolicy, UnlockMethod};

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
    /// **The session's one activity clock**, shared with the idle blank
    /// (WS-E.4.3, issue #223).
    ///
    /// This used to be a plain `last_activity: Instant` on this struct, and the
    /// blank needs the same fact: "when did the human last touch this session".
    /// Two fields would be two clocks, they would drift, and the drift would be
    /// invisible — so the field was lifted into
    /// [`crate::backend::blank::SessionActivity`] behind an `Rc<RefCell<..>>`,
    /// on [`crate::input::InputRouter`]'s presence-record pattern: minted once,
    /// handed to everyone who reads it, written at exactly one site.
    ///
    /// That one site is [`Self::judge`], below its `Origin::Physical` check —
    /// the very line that was already there. The blank therefore inherits, as a
    /// property of the code rather than as a second rule, the thing
    /// `an_agents_actuation_never_holds_the_idle_lock_open` pins for the lock:
    /// **an agent's actuations postpone neither timer.**
    ///
    /// It also carries `seat_absent` (D-030(7), Taha 2026-08-09): while the
    /// human is on another VT the idle clock does not run, because physical
    /// input is suspended for the whole switch so the stamp cannot advance, and
    /// a session that counted that time would lock itself at an instant no human
    /// could observe. The accepted cost is stated rather than hidden: a session
    /// switched away from for eight hours is unlocked when it is switched back
    /// to. Deliberately NOT a lowering — a lock already up stays up across a
    /// switch; this suppresses only the *raise*.
    ///
    /// That paragraph describes [`SeatChangePolicy::Never`], which is still the
    /// default; issue #246 made it one of three, and `on_seat_change` below
    /// says which this session took.
    activity: Rc<RefCell<crate::backend::blank::SessionActivity>>,
    /// The stored digest an attempt is checked against, or `None` for a
    /// privacy screen with no authentication ([`UnlockMethod`]).
    verifier: Option<PassphraseFile>,
    /// What losing the seat does (issue #246). See [`SeatChangePolicy`], and
    /// [`Self::set_seat_absent`] for the one place it is read.
    on_seat_change: SeatChangePolicy,
    /// Idle time an absence contributed that the shared clock's
    /// refresh-on-return would otherwise forgive — non-zero only under
    /// [`SeatChangePolicy::Idle`] (issue #246).
    ///
    /// **An offset on the one clock, deliberately not a second clock.** The
    /// activity record is shared with the blank and its `set_seat_absent(false,
    /// now)` restamps `last_activity` on return, which the blank needs and this
    /// policy must not inherit — under `idle` the countdown is supposed to have
    /// been running the whole time. Rather than fork the record (two clocks
    /// that drift, which is exactly what WS-E.4.3 merged them to prevent), the
    /// absence is charged here and [`Self::tick`] adds it to the elapsed time.
    ///
    /// Cleared by any physical event, at the same site that restamps the clock,
    /// so the two can never disagree about when the human was last here. It
    /// accumulates across repeated absences (`+=`, not `=`): each addition
    /// measures from the last restamp, so a second switch-away adds only what
    /// has passed since the first return.
    seat_absence_carry: Duration,
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
            .field("on_seat_change", &self.on_seat_change)
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
        activity: Rc<RefCell<crate::backend::blank::SessionActivity>>,
    ) -> Self {
        Self {
            locked: None,
            chord_spelling: chord.spelling(),
            // `ChordMatcher::new` only refuses duplicate bindings; one binding
            // cannot duplicate itself, so this is infallible by construction.
            matcher: ChordMatcher::new(vec![(chord, LockGesture::Lock)])
                .expect("a single binding cannot collide with itself"),
            idle_after,
            // Taken rather than minted: the clock this gate stamps must be the
            // clock the blank reads and the clock the backend composites off, or
            // the two idle timers are measuring different sessions.
            activity,
            verifier,
            // D-030(2)'s answer, which is still the default. The one backend
            // that can observe a seat change names its policy explicitly
            // through `with_seat_change_policy`; every other construction site
            // is one where `set_seat_absent` is never called at all.
            on_seat_change: SeatChangePolicy::default(),
            seat_absence_carry: Duration::ZERO,
            journal: Vec::new(),
        }
    }

    /// Name the seat-change policy this session was configured with (issue
    /// #246).
    ///
    /// **A builder call rather than a fifth constructor parameter**, because
    /// only a backend that *has* a seat has an answer: the nested backend
    /// deliberately grows no seat handling (a host compositor's focus loss is
    /// not a seat loss), and forcing it to name a policy it can never apply
    /// would put a claim in the code that the code does not implement. The
    /// bare-metal backend calls this; nothing else does, and
    /// `the_bare_metal_backend_hands_the_lock_its_configured_seat_policy` pins
    /// that it keeps doing so.
    pub(crate) fn with_seat_change_policy(mut self, policy: SeatChangePolicy) -> Self {
        self.on_seat_change = policy;
        self
    }

    /// Tell the gate whether this session still holds the seat (D-030(7),
    /// issue #246).
    ///
    /// Called from the bare-metal backend's `PauseSession` / `ActivateSession`
    /// handler and nowhere else — nested has no equivalent, because a host
    /// compositor's focus loss is not a seat loss and the human can still see
    /// the window.
    ///
    /// # The three policies, and where each one acts
    ///
    /// * [`SeatChangePolicy::Never`] — the default and D-030(2)'s answer: the
    ///   shared record freezes the countdown for the absence and restamps it on
    ///   return, so a human who comes back to their own screen has, by
    ///   returning, done the one thing the idle timer is asking about. Nothing
    ///   below the forward runs.
    /// * [`SeatChangePolicy::Immediate`] — the raise happens **here**, on the
    ///   way out, so the lock is already up before the panel belongs to anyone
    ///   else. [`LockCause::SeatChange`] carries the attributability argument.
    /// * [`SeatChangePolicy::Idle`] — the absence is charged to
    ///   [`Self::seat_absence_carry`] on the way **in**, read before the
    ///   forward restamps the clock. The raise then happens through
    ///   [`Self::tick`] like any other idle raise, on the first dispatch round
    ///   after the human is back, rather than during an absence nobody could
    ///   have watched.
    ///
    /// **No policy lowers a lock that is already up.** `raise` is a no-op on a
    /// raised lock and nothing here ever clears `locked`, so a VT switch cannot
    /// become a way past a lock screen under any of the three.
    pub(crate) fn set_seat_absent(&mut self, absent: bool, now: Instant) {
        match (self.on_seat_change, absent) {
            (SeatChangePolicy::Immediate, true) => {
                self.raise(LockCause::SeatChange);
            }
            (SeatChangePolicy::Idle, false) => {
                // BEFORE the forward below, which is the whole trick: that call
                // restamps `last_activity` to `now`, and this reads the stamp
                // the absence froze. The difference is every second since the
                // human was last here -- their idle time before leaving plus
                // the absence itself.
                let idle_across =
                    now.saturating_duration_since(self.activity.borrow().last_activity());
                self.seat_absence_carry = self.seat_absence_carry.saturating_add(idle_across);
            }
            _ => {}
        }
        // Forwarded to the one shared record (WS-E.4.3), which also forces the
        // blank's phase back to lit: a paused session must not hold a blank it
        // cannot undo, since `DrmSurface::clear` answers `DeviceInactive` while
        // the seat is somebody else's.
        self.activity.borrow_mut().set_seat_absent(absent, now);
    }

    /// Forget the lock chord's modifier bits and consumed set across a seat
    /// pause — [`ChordMatcher::forget_physical_state`], whose docs carry the
    /// reason.
    ///
    /// The lock's own instance is the sharpest of the four, because the
    /// default lock chord is `ctrl+alt+delete` and the VT escape is
    /// `ctrl+alt+F<n>`: a human who left this VT was holding **exactly**
    /// `ctrl+alt`, so without this their next bare Delete raises the lock
    /// screen.
    #[cfg_attr(
        not(feature = "drm-backend"),
        allow(
            dead_code,
            reason = "the only production caller is the bare-metal PauseSession arm -- a seat                       pause is the one event that ends physical input without releases, and                       only that backend has a seat. The behaviour is tested below in every                       build"
        )
    )]
    pub(crate) fn forget_physical_state(&mut self) {
        self.matcher.forget_physical_state();
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
        // Read through the shared record, never a second copy of the flag.
        //
        // **This early return holds under all three seat policies** (issue
        // #246), including `idle`. Under that one the absence IS counted, but
        // it is counted into `seat_absence_carry` and spent on the human's
        // return, so the raise still happens at an instant they can watch --
        // which is D-030(2)'s own objection to the pre-D-030 behaviour, kept
        // rather than reinstated. Under `immediate` the lock is already up by
        // the time this runs, so the `locked.is_some()` return above wins.
        //
        // **Deliberately NOT also suppressed while the screen is dark**
        // (WS-E.4.3). Blanking and locking are uncoupled by owner decision, and
        // the coupling that would be easiest to introduce by accident is this
        // one: a `tick` that returned early on a dark screen would mean
        // `--blank-idle 300 --lock-idle 600` never locks, because the shorter
        // timer would silently disable the longer. Blanking while unlocked and
        // then locking behind the dark screen is the correct behaviour --
        // the human touches a key, the wake is consumed, and the screen comes
        // back showing the lock card. `a_blank_does_not_disable_the_idle_lock`
        // pins it.
        let activity = self.activity.borrow();
        if activity.seat_absent() {
            return false;
        }
        let Some(after) = self.idle_after else {
            return false;
        };
        // The carry is zero except under `SeatChangePolicy::Idle`, and zero
        // under it too until an absence has actually been charged -- so this
        // addition is the identity on every session that does not ask for the
        // policy, which is what "the default is not weakened" means here.
        let idle_for = now
            .saturating_duration_since(activity.last_activity())
            .saturating_add(self.seat_absence_carry);
        if idle_for < after {
            return false;
        }
        drop(activity);
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

        // Modifier tracking, then the match. Sound here — rather than in an
        // `observe` — only because this gate is outermost; see the module docs
        // and `the_lock_gate_is_the_outermost_hook`.
        self.matcher.observe(input);

        // **The activity stamp and the wake verdict, in one call, and HERE**
        // (WS-E.4.3, issue #223). The position is the whole point and it is
        // wedged between two constraints:
        //
        // * it must be **after** `self.matcher.observe`, or a consumed wake
        //   press desyncs the chord's modifier bits. Concretely: screen dark,
        //   the human presses Ctrl to wake it (eaten), then Alt, then Delete —
        //   a matcher that never saw Ctrl go down does not raise the lock. That
        //   is the `forget_physical_state` bug with a new cause.
        // * it must be **before** `self.matcher.gate` and before `type_key`,
        //   or the press that woke the screen fires the lock chord, or is typed
        //   into a passphrase attempt as an invisible stray character.
        //
        // And it must be inside *this* gate rather than in a `BlankGate`
        // stacked outside it: an outer gate eats whatever the human happened to
        // press first, which is very often a bare modifier, and that is the
        // first bullet again with nothing tracking the modifier at all.
        // `crate::backend::blank`'s module docs carry the full argument.
        //
        // **A release is stamped and wakes, but is never consumed**, which is
        // this gate's own pairing contract applied one rule up. A human holding
        // a modifier while the idle timer fires -- unlikely, but a `--blank-idle
        // 300` and a key held five minutes is all it takes -- would otherwise
        // have that key's release eaten by the wake, and the confined app is
        // left holding it down for the rest of the session. That is the P1.7.2
        // regression exactly, and consuming a release buys nothing: the app
        // already saw the press, so there is nothing left to hide from it.
        let wake = self.activity.borrow_mut().note_physical(now);
        // The human is here, so no absence is owed to the idle countdown any
        // more (issue #246). Cleared at the same site that restamps the shared
        // clock, and unconditionally -- a wake press that this gate is about to
        // swallow is still the human touching their keyboard, and an offset
        // that survived it would lock the screen they just woke.
        self.seat_absence_carry = Duration::ZERO;
        if wake.consumes() && !is_pairing_release(input) {
            return Gate::Consume;
        }

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
            }
            // A gesture's end is a release for this purpose (WS-E.4.2,
            // issue #222): if the app was mid-pinch when the lock came up,
            // consuming its end leaves it accumulating that pinch forever,
            // and the router already drops any end whose begin it did not
            // deliver — so, like the two above, this can leak nothing.
            | SeatInputKind::GestureEnd { .. } => Gate::Deliver,
            SeatInputKind::Key {
                keysym,
                state: KeyState::Pressed,
                ..
            } => {
                self.type_key(*keysym);
                Gate::Consume
            }
            // Motion, scroll, button presses and text — plus relative motion
            // and a gesture's begin and updates, which are input to the app
            // exactly as the first four are. Exhaustive by intent: a new
            // input kind must be classified here rather than defaulting to
            // reaching an app behind a locked screen.
            SeatInputKind::Motion { .. }
            | SeatInputKind::Scroll { .. }
            | SeatInputKind::Button { .. }
            | SeatInputKind::Text { .. }
            | SeatInputKind::RelativeMotion { .. }
            | SeatInputKind::GestureBegin { .. }
            | SeatInputKind::GestureSwipeUpdate { .. }
            | SeatInputKind::GesturePinchUpdate { .. } => Gate::Consume,
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

/// Whether this event is the second half of a pair a confined app may already
/// be holding (WS-E.4.3).
///
/// [`LockScreen::judge`]'s own rule — "this gate never consumes a release" —
/// hoisted to a named predicate because the wake verdict now needs it too, and
/// because a rule stated twice is a rule that will one day be stated
/// differently. A gesture's end counts: consuming it leaves the app
/// accumulating a pinch forever, and the router already drops any end whose
/// begin it did not deliver.
fn is_pairing_release(input: &SeatInput) -> bool {
    matches!(
        input.kind(),
        SeatInputKind::Key {
            state: KeyState::Released,
            ..
        } | SeatInputKind::Button {
            state: ButtonState::Released,
            ..
        } | SeatInputKind::GestureEnd { .. }
    )
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
    use vitrin_protocol::generated::vitrin_shim_seat::{GestureKind, GestureState};

    fn chord() -> ModChord {
        ModChord::parse("ctrl+alt+delete").expect("the default lock chord parses")
    }

    /// A fresh activity record for a test that does not care about the blank.
    fn clock(now: Instant) -> Rc<RefCell<crate::backend::blank::SessionActivity>> {
        Rc::new(RefCell::new(crate::backend::blank::SessionActivity::new(
            None, now,
        )))
    }

    fn screen(idle: Option<Duration>, now: Instant) -> LockScreen {
        LockScreen::new(chord(), idle, None, clock(now))
    }

    /// The same fixture under one of the three seat-change policies
    /// (issue #246).
    fn screen_with_policy(
        idle: Option<Duration>,
        now: Instant,
        policy: SeatChangePolicy,
    ) -> LockScreen {
        screen(idle, now).with_seat_change_policy(policy)
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
    ///
    /// **Issue #246 made this one of three policies and did not touch this
    /// test.** The fixture takes no policy, so it runs on the default — which
    /// is the point: if the default ever stopped being D-030(2)'s answer, this
    /// test would fail, and that is the tripwire the configuration surface had
    /// to be built behind.
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
    ///
    /// **Held for all three seat policies** (issue #246). This invariant is not
    /// one of the options: whichever answer an operator picks for "what does
    /// leaving do", none of them may answer "it undoes a lock". The loop is the
    /// enforcement — a fourth policy added later cannot be merged without
    /// coming past this list.
    #[test]
    fn losing_the_seat_never_lowers_a_lock_that_is_already_up() {
        for policy in [
            SeatChangePolicy::Never,
            SeatChangePolicy::Idle,
            SeatChangePolicy::Immediate,
        ] {
            let t0 = Instant::now();
            let mut s = screen_with_policy(Some(Duration::from_secs(60)), t0, policy);
            assert!(s.tick(t0 + Duration::from_secs(60)));
            assert!(s.is_locked());
            // The cause the human's absence raised, before any seat event.
            assert_eq!(s.cause(), Some(LockCause::Idle));

            s.set_seat_absent(true, t0 + Duration::from_secs(61));
            assert!(
                s.is_locked(),
                "{}: a VT switch must not unlock a locked session",
                policy.as_str()
            );
            assert_eq!(
                s.cause(),
                Some(LockCause::Idle),
                "{}: and it must not rewrite why the session locked -- a journal reader has to \
                 be able to tell an idle lock from a seat lock",
                policy.as_str()
            );
            s.set_seat_absent(false, t0 + Duration::from_secs(999));
            assert!(
                s.is_locked(),
                "{}: and coming back must still meet the lock the human left up",
                policy.as_str()
            );
        }
    }

    /// **`immediate`: losing the seat raises the lock at once** (issue #246).
    ///
    /// It does not need `--lock-idle` — the two are independent, and a session
    /// that locks on every switch is precisely the operator who does not want
    /// to think about timeouts. The cause is its own, so the card and the
    /// journal both say what happened rather than blaming a timer that never
    /// fired.
    #[test]
    fn an_immediate_policy_locks_the_session_when_the_seat_goes_away() {
        let t0 = Instant::now();
        let mut s = screen_with_policy(None, t0, SeatChangePolicy::Immediate);
        assert!(!s.is_locked());

        s.set_seat_absent(true, t0 + Duration::from_secs(1));
        assert!(
            s.is_locked(),
            "under `immediate` the lock is up before the panel belongs to anyone else"
        );
        assert_eq!(s.cause(), Some(LockCause::SeatChange));
        assert_eq!(
            s.take_journal(),
            vec![LockJournal::Locked {
                cause: LockCause::SeatChange
            }],
            "the recorder is owed one entry naming the seat, not an idle raise"
        );

        // Coming back does not lower it: that is what "always costs a
        // passphrase" means, and it is the same invariant as the test above.
        s.set_seat_absent(false, t0 + Duration::from_secs(2));
        assert!(s.is_locked());
        assert!(
            s.take_journal().is_empty(),
            "returning is not a second lock event"
        );

        // A switch away from an ALREADY locked session queues no second entry:
        // three switches must not read as three locks in the journal.
        let mut again = screen_with_policy(None, t0, SeatChangePolicy::Immediate);
        again.set_seat_absent(true, t0);
        again.set_seat_absent(false, t0 + Duration::from_secs(1));
        again.set_seat_absent(true, t0 + Duration::from_secs(2));
        assert_eq!(
            again.take_journal(),
            vec![LockJournal::Locked {
                cause: LockCause::SeatChange
            }]
        );
    }

    /// **`idle`: a long absence returns to a locked screen, a short one does
    /// not** (issue #246).
    ///
    /// The clock is charged for the whole absence, but the raise lands on the
    /// human's return rather than during the switch-away — D-030(2)'s objection
    /// to the pre-D-030 behaviour was that it locked at an instant nobody could
    /// observe, and this policy answers the objection instead of reinstating
    /// it. What the human experiences is what the table in #246 promises.
    #[test]
    fn an_idle_policy_charges_the_absence_to_the_countdown() {
        let t0 = Instant::now();
        let idle = Duration::from_secs(60);

        // Long absence: locked by the time the human has their screen back.
        let mut long = screen_with_policy(Some(idle), t0, SeatChangePolicy::Idle);
        long.set_seat_absent(true, t0 + Duration::from_secs(1));
        assert!(
            !long.tick(t0 + Duration::from_secs(8 * 60 * 60)),
            "still not DURING the absence: the raise the human cannot watch is the one D-030(2) \
             refused, and this policy does not bring it back"
        );
        let back = t0 + Duration::from_secs(8 * 60 * 60);
        long.set_seat_absent(false, back);
        assert!(
            long.tick(back),
            "the first round after the return must find the countdown already spent"
        );
        assert_eq!(long.cause(), Some(LockCause::Idle));

        // Short absence: nothing happens, and the countdown keeps its
        // remainder rather than being forgiven -- 30s away plus 20s away plus
        // 10s at the keyboard is 60s of not being here.
        let mut short = screen_with_policy(Some(idle), t0, SeatChangePolicy::Idle);
        short.set_seat_absent(true, t0);
        short.set_seat_absent(false, t0 + Duration::from_secs(30));
        assert!(!short.tick(t0 + Duration::from_secs(30)));
        short.set_seat_absent(true, t0 + Duration::from_secs(30));
        short.set_seat_absent(false, t0 + Duration::from_secs(50));
        assert!(
            !short.tick(t0 + Duration::from_secs(59)),
            "50s of absence and 9s at the keyboard is 59s, and the timeout is 60"
        );
        assert!(
            short.tick(t0 + Duration::from_secs(60)),
            "two absences must ADD, not overwrite: a session that forgave the first one every \
             time it was switched away from again would never lock"
        );
        assert_eq!(short.cause(), Some(LockCause::Idle));
    }

    /// **The policies are driven through the primitive the backend actually
    /// calls** (issues #246 and #257) — the one test here that does not choose
    /// its own instant.
    ///
    /// Every other seat test above calls `set_seat_absent(absent, now)` and
    /// picks `now` itself. That is exactly the freedom the production caller
    /// does not have, and it is the freedom that hid a defect once:
    /// `handle_session_event`'s activate arm used to pass `DrmState::now`, the
    /// **input turn's** clock sample, and a paused session sees no input turn —
    /// the chord that switches the VT back is delivered to whichever session is
    /// currently active. So `now` was by construction the same instant as
    /// `last_activity`; `idle`'s charge was exactly **zero** on the only
    /// backend that can produce a seat event, and every test above stayed green
    /// because each supplied a fresh instant the backend never did.
    ///
    /// [`crate::session::note_seat_presence`] (issue #257) is the fix: it
    /// samples the instant itself and takes no `now` at all, so the stale stamp
    /// is not expressible. This test goes through **that** function, so the two
    /// policies are pinned apart on a clock the test never touches.
    ///
    /// No sleep and no timing tolerance. The activity record starts ten minutes
    /// in the past against a sixty-second timeout, so whatever `Instant::now()`
    /// answers, `idle` has minutes to charge and `never` has just restamped the
    /// clock to a moment with nothing on it.
    #[test]
    fn the_seat_primitive_charges_an_absence_under_idle_and_forgives_it_under_never() {
        let idle = Duration::from_secs(60);
        let long_ago = Instant::now() - Duration::from_secs(600);

        for (policy, expected) in [
            (SeatChangePolicy::Idle, true),
            (SeatChangePolicy::Never, false),
        ] {
            let screen = RefCell::new(screen_with_policy(Some(idle), long_ago, policy));
            crate::session::note_seat_presence(&screen, true);
            crate::session::note_seat_presence(&screen, false);
            assert_eq!(
                screen.borrow_mut().tick(Instant::now()),
                expected,
                "{}: the ten minutes this session spent on another VT must be charged to the \
                 countdown under `idle` and forgiven under `never`, and the instant that decides \
                 it has to come from the seat event rather than from a cell an input turn wrote",
                policy.as_str()
            );
            assert_eq!(
                screen.borrow().is_locked(),
                expected,
                "{}: and the raise is the ordinary idle one, landing on the return",
                policy.as_str()
            );
        }
    }

    /// Touching the keyboard clears what the absence charged (issue #246).
    ///
    /// The failure this closes is the one that would be reported as "it locks
    /// the moment I come back and type": a carry that survived physical input
    /// would spend the absence against a human who is demonstrably at the
    /// keyboard.
    #[test]
    fn coming_back_and_typing_clears_the_absence_the_idle_policy_charged() {
        let t0 = Instant::now();
        let mut s = screen_with_policy(Some(Duration::from_secs(60)), t0, SeatChangePolicy::Idle);
        s.set_seat_absent(true, t0);
        let back = t0 + Duration::from_secs(8 * 60 * 60);
        s.set_seat_absent(false, back);

        // One keystroke, before the round that would have locked the screen.
        s.judge(&press(KEYSYM_RETURN), back);
        assert!(
            !s.tick(back + Duration::from_secs(59)),
            "the human is here; the absence is spent and the countdown starts over"
        );
        assert!(
            s.tick(back + Duration::from_secs(60)),
            "and it is the ordinary countdown afterwards, not a disabled one"
        );
    }

    /// **Saying `never` out loud is the same session as saying nothing**
    /// (issue #246) — the "no silent weakening" assertion from the other side.
    ///
    /// A configuration surface can weaken a default in two ways: by changing
    /// what the absent flag means, or by making the explicitly-named default
    /// take a different path through the code than the unnamed one. The first
    /// is held by `a_seat_taken_away_stops_the_idle_clock_and_returning_restarts_it`,
    /// which never names a policy. This is the second.
    #[test]
    fn naming_the_default_seat_policy_changes_nothing_about_the_session() {
        let idle = Duration::from_secs(60);
        let t0 = Instant::now();
        for mut s in [
            screen(Some(idle), t0),
            screen_with_policy(Some(idle), t0, SeatChangePolicy::Never),
        ] {
            s.set_seat_absent(true, t0 + Duration::from_secs(1));
            assert!(!s.tick(t0 + Duration::from_secs(8 * 60 * 60)));
            let back = t0 + Duration::from_secs(8 * 60 * 60);
            s.set_seat_absent(false, back);
            assert!(
                !s.tick(back + Duration::from_secs(59)),
                "the countdown restarts from the return under the default, named or not"
            );
            assert!(s.tick(back + Duration::from_secs(60)));
            assert_eq!(s.cause(), Some(LockCause::Idle));
        }
    }

    /// **A seat pause leaves no stale `ctrl+alt` behind, so the human's next
    /// bare Delete does not lock their screen** (WS-E.3.5).
    ///
    /// This is the lock's instance of a defect that was live on every branch
    /// before the VT escape landed, and the escape is what makes it fire on
    /// the first use: the default lock chord is `ctrl+alt+delete` and the
    /// escape is `ctrl+alt+F<n>`, so a human leaving this VT is holding
    /// **exactly** the lock chord's modifiers. libinput is suspended before
    /// either release can arrive, and the presses the app is paid are handed
    /// straight to the delivery funnel rather than back through the hook
    /// stack, so nothing else clears the bits.
    ///
    /// The control comes first, or the assertion would pass against a matcher
    /// that never tracked a modifier at all.
    #[test]
    fn a_seat_pause_leaves_no_stale_modifier_in_the_lock_chord() {
        let t0 = Instant::now();

        // The control: with ctrl+alt genuinely held, Delete locks.
        let mut s = screen(None, t0);
        s.judge(&press(0xffe3), t0);
        s.judge(&press(0xffe9), t0);
        assert_eq!(s.judge(&press(0xffff), t0), Gate::Consume);
        assert!(s.is_locked(), "the fixture must reach the chord at all");

        // The real case: the human held ctrl+alt, left for another VT, came
        // back, and pressed a bare Delete.
        let mut s = screen(None, t0);
        s.judge(&press(0xffe3), t0);
        s.judge(&press(0xffe9), t0);
        s.forget_physical_state();
        assert_eq!(
            s.judge(&press(0xffff), t0),
            Gate::Deliver,
            "a stale ctrl+alt turns the human's next bare Delete into a lock chord -- and \
             takes the key away from the app on the way"
        );
        assert!(!s.is_locked());
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
        // The version-2 classes (WS-E.4.2, issue #222). A delta leaks the same
        // path a motion does, and a gesture's begin and updates are input --
        // but its END is a release for the contract's purpose: consuming it
        // would leave the app accumulating a pinch the human began before the
        // lock came up, forever.
        let phys = crate::input::tests::physical_for_test;
        assert_eq!(
            s.judge(
                &phys(SeatInputKind::RelativeMotion {
                    dx: 1.0,
                    dy: 2.0,
                    dx_unaccel: 1.0,
                    dy_unaccel: 2.0,
                }),
                t0
            ),
            Gate::Consume
        );
        assert_eq!(
            s.judge(
                &phys(SeatInputKind::GestureBegin {
                    kind: GestureKind::Swipe,
                    fingers: 3,
                }),
                t0
            ),
            Gate::Consume
        );
        assert_eq!(
            s.judge(
                &phys(SeatInputKind::GestureSwipeUpdate { dx: 1.0, dy: 0.0 }),
                t0
            ),
            Gate::Consume
        );
        assert_eq!(
            s.judge(
                &phys(SeatInputKind::GesturePinchUpdate {
                    dx: 0.0,
                    dy: 0.0,
                    scale: 1.5,
                    rotation: 10.0,
                }),
                t0
            ),
            Gate::Consume
        );
        assert_eq!(
            s.judge(
                &phys(SeatInputKind::GestureEnd {
                    kind: GestureKind::Swipe,
                    state: GestureState::Completed,
                }),
                t0
            ),
            Gate::Deliver,
            "the pairing contract, third shape: the router drops an end whose begin it \
             did not deliver, so delivering this can leak nothing -- and consuming it \
             latches a gesture no drain can afterwards repair"
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
            clock(t0),
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
        let mut s = LockScreen::new(chord(), None, Some(file), clock(t0));
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
        let mut s = LockScreen::new(chord(), None, Some(file), clock(t0));
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
        let mut s = LockScreen::new(chord(), None, Some(file), clock(t0));
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
        let mut s = LockScreen::new(chord(), None, Some(file), clock(t0));
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
        let mut s = LockScreen::new(chord(), None, Some(file), clock(t0));
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
        let screen = Rc::new(RefCell::new(LockScreen::new(
            chord(),
            None,
            None,
            clock(t0),
        )));
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

    /// **The human's off-switch works at a dark screen** (WS-E.4.3, issue #223).
    ///
    /// The wake consumes the press so it reaches no app — and the dead-man must
    /// arm on it anyway, because it detects in `PreemptionHook::observe`, which
    /// `GateOnlyHook` forwards unconditionally and no gate can suppress. The
    /// hold then completes on its own clock rather than on a vblank, which
    /// matters here specifically: a dark CRTC produces no vblanks at all, so a
    /// hold paced by presentation would never complete on the one screen state
    /// where the human most needs it to.
    ///
    /// `docs/book/src/limits.md` publishes this. Without this test that is a
    /// claim about architecture that nobody re-checks — and the blank is a
    /// brand-new consumer of the human's press, which is exactly when an
    /// architectural argument stops being self-evidently still true.
    #[test]
    fn the_dead_man_arms_on_the_press_that_wakes_a_dark_screen() {
        use crate::consent::grab::{ConsentGate, ConsentGrab};
        use crate::deadman::{DeadManConfig, DeadManHook, DeadManSwitch, DEFAULT_HOLD};

        let t0 = Instant::now();
        let now = Rc::new(Cell::new(t0));
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(60)),
            t0,
        )));
        let screen = Rc::new(RefCell::new(LockScreen::new(
            chord(),
            None,
            None,
            Rc::clone(&activity),
        )));
        let switch = Rc::new(RefCell::new(DeadManSwitch::new(DeadManConfig::default())));
        let grab = Rc::new(RefCell::new(ConsentGrab::new()));

        let mut router = InputRouter::detached(lock_gate(
            Rc::clone(&screen),
            Rc::clone(&now),
            ConsentGate::new(
                Rc::clone(&grab),
                Rc::clone(&now),
                DeadManHook::new(Rc::clone(&switch), Rc::clone(&now), NoopHook),
            ),
        ));
        let realm = crate::grants::RealmId::new("realm-0");
        assert!(router.bind_to(&realm).is_none());
        let view = (640, 480);

        // The screen goes dark on the idle timer, with the session UNLOCKED --
        // Decision 1's shape: idle blanks, it does not lock.
        let blanked_at = t0 + Duration::from_secs(60);
        {
            let mut a = activity.borrow_mut();
            assert!(a.tick(blanked_at, false));
            a.note_frame_queued();
            a.went_dark();
        }
        // The router's own clock moves with the activity clock: the hold below
        // is measured from when the human actually pressed, not from t0.
        now.set(blanked_at);
        assert!(
            !screen.borrow().is_locked(),
            "the fixture must leave the session unlocked, or this tests the lock path instead"
        );

        // A sanity control FIRST, so nothing below can pass because nothing is
        // being consumed: an ordinary key is eaten by the wake and reaches no app.
        assert!(
            router
                .route_physical(press(0x61), view, Some(view))
                .is_none(),
            "the wake must be consuming physical input for this test to mean anything"
        );

        // Re-dark, so the chord below is also a wake press rather than an
        // ordinary one on an already-lit screen.
        {
            let mut a = activity.borrow_mut();
            a.note_frame_queued();
            a.went_dark();
        }

        // THE property: the press that wakes the screen still arms the switch.
        assert!(
            router
                .route_physical(crate::input::tests::chord_press(), view, Some(view))
                .is_none(),
            "the chord's press is consumed as a wake and reaches no app"
        );
        assert_eq!(
            switch.borrow().deadline(),
            Some(blanked_at + DEFAULT_HOLD),
            "a dark screen must not be able to blind the human's off-switch. If this fails, a \
             human whose screen blanked on a timer -- unattended and routinely -- can no longer \
             revoke an agent's authority by the one gesture the whole design promises always \
             works"
        );

        // ...and it completes on its own clock, with no vblank to pace it.
        let due = blanked_at + DEFAULT_HOLD;
        now.set(due);
        router.observe_at(due);
        switch.borrow_mut().fire_if_due(due);
        let trigger = switch
            .borrow_mut()
            .take_trigger()
            .expect("a completed hold must fire while the panel is dark");
        assert_eq!(trigger.held, DEFAULT_HOLD);
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
        let screen = Rc::new(RefCell::new(LockScreen::new(
            chord(),
            None,
            None,
            clock(t0),
        )));
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

    // ------------------------------------------------------------------
    // WS-E.4.3, issue #223: the idle blank shares this gate's activity clock,
    // and takes its wake verdict inside this gate's `judge`.
    // ------------------------------------------------------------------

    use crate::backend::blank::{Phase, SessionActivity};

    /// A screen already dark, with the lock armed on the same clock.
    fn dark_screen(
        idle: Option<Duration>,
        now: Instant,
    ) -> (LockScreen, Rc<RefCell<SessionActivity>>) {
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(60)),
            now,
        )));
        let screen = LockScreen::new(chord(), idle, None, Rc::clone(&activity));
        {
            let mut a = activity.borrow_mut();
            assert!(a.tick(now + Duration::from_secs(60), false));
            a.note_frame_queued();
            a.went_dark();
            assert_eq!(a.phase(), Phase::Dark);
        }
        (screen, activity)
    }

    /// **THE position test** (WS-E.4.3): the wake verdict is taken *after*
    /// `ChordMatcher::observe` and *before* `ChordMatcher::gate`, so a press
    /// swallowed to wake the screen still leaves the chord's modifier bits
    /// correct.
    ///
    /// This is the failure the whole "no `BlankGate` outside `LockGate`"
    /// argument exists to prevent, and it is not hypothetical: a wake gate
    /// stacked above this one eats whatever the human happens to press first,
    /// which is very often a bare modifier, and the lock chord then stops
    /// working after every blank. Moving the `note_physical` call above
    /// `self.matcher.observe(input)` reproduces it exactly and turns this red.
    ///
    /// The control comes first, or the assertion would pass against a matcher
    /// that never tracked a modifier at all.
    #[test]
    fn a_wake_press_is_swallowed_but_the_lock_chords_modifiers_are_not() {
        let t0 = Instant::now();

        // The control: lit, ctrl+alt+delete raises the lock.
        let mut s = screen(None, t0);
        s.judge(&press(0xffe3), t0);
        s.judge(&press(0xffe9), t0);
        assert_eq!(s.judge(&press(0xffff), t0), Gate::Consume);
        assert!(s.is_locked(), "the fixture must reach the chord at all");

        // The real case: the human comes back to a dark screen holding down
        // ctrl, then alt, then delete.
        let (mut s, activity) = dark_screen(None, t0);
        let wake = t0 + Duration::from_secs(90);
        assert_eq!(
            s.judge(&press(0xffe3), wake),
            Gate::Consume,
            "the press that wakes the screen must not reach an app the human cannot see"
        );
        assert_eq!(activity.borrow().phase(), Phase::Waking);
        assert_eq!(
            s.judge(&press(0xffe9), wake),
            Gate::Consume,
            "and neither must the next one, until the panel is actually back"
        );
        assert!(
            !s.is_locked(),
            "a swallowed press must not fire a chord either -- the human was reaching for a \
             screen, not for a gesture"
        );

        // The panel comes back...
        activity.borrow_mut().note_flip_completed();
        assert_eq!(activity.borrow().phase(), Phase::Lit);

        // ...and the modifiers the human has been holding all along are still
        // recorded, so their Delete is the chord. THIS is what a wake gate
        // stacked outside this one would break.
        assert_eq!(
            s.judge(&press(0xffff), wake + Duration::from_millis(120)),
            Gate::Consume
        );
        assert!(
            s.is_locked(),
            "the wake swallowed ctrl and alt as EVENTS but must not have swallowed them as \
             MODIFIER STATE: `matcher.observe` runs before the wake verdict precisely so \
             ctrl+alt+delete still raises the lock after a blank"
        );
    }

    /// A wake press types nothing into a passphrase attempt.
    ///
    /// The other half of the position argument: the verdict is taken *before*
    /// `type_key`, so the key a human pressed at a dark screen does not become
    /// an invisible stray character in front of the passphrase they are about
    /// to type.
    #[test]
    fn a_wake_press_is_not_typed_into_the_passphrase() {
        let t0 = Instant::now();
        let file = super::super::tests::cheap_verifier(b"ok");
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(60)),
            t0,
        )));
        let mut s = LockScreen::new(chord(), None, Some(file), Rc::clone(&activity));
        s.raise(LockCause::Chord);
        let _ = s.take_journal();
        {
            let mut a = activity.borrow_mut();
            assert!(a.tick(t0 + Duration::from_secs(60), false));
            a.note_frame_queued();
            a.went_dark();
        }

        // The human bumps the keyboard to wake the screen: an `x`.
        let wake = t0 + Duration::from_secs(90);
        assert_eq!(s.judge(&press(0x78), wake), Gate::Consume);
        activity.borrow_mut().note_flip_completed();

        // ...and now types the passphrase. If the `x` had been typed, the
        // attempt would be "xok" and this would stay locked.
        for ch in "ok".chars() {
            s.judge(&press(ch as u32), wake + Duration::from_millis(200));
        }
        s.judge(&press(KEYSYM_RETURN), wake + Duration::from_millis(200));
        assert!(
            !s.is_locked(),
            "the key that woke the screen must not have been typed into the attempt: a stray \
             invisible character in front of a passphrase is a lock nobody can open"
        );
    }

    /// **An agent's actuation neither postpones the blank nor wakes the
    /// screen** (WS-E.4.3).
    ///
    /// The postpone half is inherited structurally from
    /// `an_agents_actuation_never_holds_the_idle_lock_open` — the origin check
    /// sits above the one site that stamps the clock, so sharing the clock made
    /// the blank inherit it rather than restate it. The **wake** half is the
    /// sharper property and has its own argument: there is no verb in the IDL
    /// for "power the human's display", so an agent that could wake a panel
    /// would be making an unrequested change to the human's physical
    /// environment, remotely triggerable, under no grant at all.
    #[test]
    fn an_agents_actuation_neither_postpones_the_blank_nor_wakes_the_screen() {
        let t0 = Instant::now();
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(60)),
            t0,
        )));
        let mut s = LockScreen::new(chord(), None, None, Rc::clone(&activity));
        let emulated = SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: 0x61,
            state: KeyState::Pressed,
        });

        // Working through the night: the blank still falls on schedule.
        for i in 0..60 {
            assert_eq!(
                s.judge(&emulated, t0 + Duration::from_secs(i)),
                Gate::Deliver
            );
        }
        assert!(
            activity
                .borrow_mut()
                .tick(t0 + Duration::from_secs(60), false),
            "an agent's actuations must not hold the screen awake for a human who went home: \
             it spends the machine's battery and lights an empty room under no authority"
        );
        activity.borrow_mut().note_frame_queued();
        activity.borrow_mut().went_dark();

        // ...and it cannot turn the panel back on, either.
        for i in 0..10 {
            assert_eq!(
                s.judge(&emulated, t0 + Duration::from_secs(100 + i)),
                Gate::Deliver,
                "an agent's admitted actuation stays the chokepoint's business"
            );
        }
        assert_eq!(
            activity.borrow().phase(),
            Phase::Dark,
            "an agent that could wake the human's display would hold a remotely-triggerable \
             primitive over their physical environment that no grant names"
        );
    }

    /// **Every physical event postpones the blank, and every physical event
    /// wakes it** — presses and releases, pointer and keyboard alike.
    ///
    /// Enumerated rather than sampled because the rule is "everything physical
    /// and nothing else", and a rule with one accidental exception is a screen
    /// that goes dark under a human's hand.
    #[test]
    fn every_physical_event_postpones_the_blank_and_wakes_a_dark_screen() {
        use vitrin_protocol::generated::vitrin_shim_seat::{GestureKind, GestureState};
        let phys = crate::input::tests::physical_for_test;
        let t0 = Instant::now();

        let kinds: Vec<SeatInput> = vec![
            press(0x61),
            release(0x61),
            phys(SeatInputKind::Motion { x: 4.0, y: 5.0 }),
            phys(SeatInputKind::Scroll {
                axis: vitrin_protocol::generated::vitrin_actuator_pointer::Axis::Vertical,
                value120: 120,
            }),
            phys(SeatInputKind::Button {
                button: 0x110,
                state: ButtonState::Pressed,
            }),
            phys(SeatInputKind::Button {
                button: 0x110,
                state: ButtonState::Released,
            }),
            phys(SeatInputKind::Text {
                text: "hi".to_string(),
            }),
            phys(SeatInputKind::RelativeMotion {
                dx: 1.0,
                dy: 2.0,
                dx_unaccel: 1.0,
                dy_unaccel: 2.0,
            }),
            phys(SeatInputKind::GestureBegin {
                kind: GestureKind::Swipe,
                fingers: 3,
            }),
            phys(SeatInputKind::GestureSwipeUpdate { dx: 1.0, dy: 0.0 }),
            phys(SeatInputKind::GesturePinchUpdate {
                dx: 0.0,
                dy: 0.0,
                scale: 1.5,
                rotation: 10.0,
            }),
            phys(SeatInputKind::GestureEnd {
                kind: GestureKind::Swipe,
                state: GestureState::Completed,
            }),
        ];

        for input in &kinds {
            // Postpone: the deadline moves out by exactly the event's instant.
            let activity = Rc::new(RefCell::new(SessionActivity::new(
                Some(Duration::from_secs(60)),
                t0,
            )));
            let mut s = LockScreen::new(chord(), None, None, Rc::clone(&activity));
            s.judge(input, t0 + Duration::from_secs(59));
            assert!(
                !activity
                    .borrow_mut()
                    .tick(t0 + Duration::from_secs(118), false),
                "{input:?} must postpone the blank"
            );
            assert!(activity
                .borrow_mut()
                .tick(t0 + Duration::from_secs(119), false));

            // Wake: from dark, this one event wakes the screen. **Presses are
            // swallowed and releases are not** -- this gate's own pairing
            // contract, applied to the wake for the same reason: a human
            // holding a modifier when the idle timer fires would otherwise have
            // its release eaten, and the confined app is left holding that key
            // down for the rest of the session (the P1.7.2 regression). The app
            // already saw the press, so consuming the release hides nothing.
            let (mut s, activity) = dark_screen(None, t0);
            let wake = t0 + Duration::from_secs(90);
            let expected = if is_pairing_release(input) {
                Gate::Deliver
            } else {
                Gate::Consume
            };
            assert_eq!(
                s.judge(input, wake),
                expected,
                "{input:?} must wake a dark screen, and be swallowed doing it unless it is \
                 the release half of a pair the app is already holding"
            );
            assert_eq!(
                activity.borrow().phase(),
                Phase::Waking,
                "{input:?} must wake the screen either way -- a human letting go of a key IS \
                 a human at the keyboard"
            );
        }
    }

    /// **A key held across a blank is released in the app** — the P1.7.2
    /// regression, reached from an idle *timer* rather than from a prompt.
    ///
    /// The wake consumes presses so a keystroke aimed at a dark screen reaches
    /// no app. If it consumed releases too, a human holding a modifier when the
    /// idle timeout expires would let go into a swallowed event, and the
    /// confined app would hold that modifier down for the rest of the session —
    /// silently rewriting everything typed afterwards. Driven through the REAL
    /// router, because what has to be true is that the delivery funnel sees it.
    #[test]
    fn a_modifier_held_when_the_screen_blanks_is_released_in_the_app() {
        let t0 = Instant::now();
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(60)),
            t0,
        )));
        let screen = Rc::new(RefCell::new(LockScreen::new(
            chord(),
            None,
            None,
            Rc::clone(&activity),
        )));
        let now = Rc::new(Cell::new(t0));
        let mut router =
            InputRouter::detached(lock_gate(Rc::clone(&screen), Rc::clone(&now), NoopHook));
        let realm = crate::grants::RealmId::new("realm-0");
        assert!(router.bind_to(&realm).is_none());
        let view = (100, 100);
        let surface = Some((100, 100));

        // Shift goes down while the screen is lit: the app is holding it.
        assert!(
            router
                .route_physical(press(0xffe1), view, surface)
                .is_some(),
            "a lit session delivers the press"
        );

        // ...and the human keeps holding it until the idle timer fires.
        {
            let mut a = activity.borrow_mut();
            assert!(a.tick(t0 + Duration::from_secs(60), false));
            a.note_frame_queued();
            a.went_dark();
        }

        let up = router.route_physical(release(0xffe1), view, surface);
        assert!(
            up.is_some(),
            "the release of a press the app already saw MUST reach the app even though it is \
             also the event that wakes the screen: consuming it latches the modifier in the \
             confined app for the rest of the session"
        );
        assert_eq!(
            activity.borrow().phase(),
            Phase::Waking,
            "...and it still woke the screen"
        );
    }

    /// **A blank must not disable the idle lock.**
    ///
    /// The two are uncoupled by owner decision (Taha, 2026-08-10) and share
    /// only the activity clock, so a session configured `--blank-idle 300
    /// --lock-idle 600` must still lock at 600 — behind a screen that has been
    /// dark since 300. A `LockScreen::tick` that returned early on a dark
    /// screen would mean the shorter timer silently switched the longer one
    /// off, which is exactly the class of unchosen behaviour D-030(2) was
    /// written to catch.
    #[test]
    fn a_blank_does_not_disable_the_idle_lock() {
        let t0 = Instant::now();
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(300)),
            t0,
        )));
        let mut s = LockScreen::new(
            chord(),
            Some(Duration::from_secs(600)),
            None,
            Rc::clone(&activity),
        );

        // The screen goes dark at 300 and the session is still UNLOCKED, which
        // is the published consequence of the decision rather than a bug.
        assert!(activity
            .borrow_mut()
            .tick(t0 + Duration::from_secs(300), false));
        activity.borrow_mut().note_frame_queued();
        activity.borrow_mut().went_dark();
        assert!(
            !s.is_locked(),
            "idle BLANKS, it does not lock: an unlocked session behind a dark screen is the \
             owner's decision and is published, not softened"
        );
        assert!(!s.tick(t0 + Duration::from_secs(599)));

        // ...and the lock still fires at 600, behind the dark screen.
        assert!(
            s.tick(t0 + Duration::from_secs(600)),
            "a dark screen must not freeze the idle lock, or `--blank-idle 300 --lock-idle \
             600` silently never locks"
        );
        assert_eq!(s.cause(), Some(LockCause::Idle));
        assert_eq!(
            activity.borrow().phase(),
            Phase::Dark,
            "and locking behind the cover does not itself wake anything"
        );
    }

    /// **An idle inhibit holds the blank and not the lock** (D-042, issue
    /// #306, D-042).
    ///
    /// The sibling of the test above, from the other direction and with a
    /// sharper stake: that one holds that a *short* blank must not switch off a
    /// *long* lock, and this one holds that a **confined app** must not be able
    /// to switch off the lock at all. An app playing a film asks the core not to
    /// blank; the core honours that and locks anyway, because `--lock-idle` is a
    /// security control and an app's comfort request is not authority over one.
    ///
    /// The property is structural rather than checked: the inhibit is a
    /// parameter of `SessionActivity::tick` and writes nothing, so
    /// `LockScreen::tick`'s only input — `last_activity()` — cannot see it. This
    /// test is what stops a later "simpler" implementation from moving the
    /// suppression into the clock, where it would silently postpone both.
    #[test]
    fn an_idle_inhibit_holds_the_blank_and_not_the_lock() {
        let t0 = Instant::now();
        let activity = Rc::new(RefCell::new(SessionActivity::new(
            Some(Duration::from_secs(300)),
            t0,
        )));
        let mut s = LockScreen::new(
            chord(),
            Some(Duration::from_secs(600)),
            None,
            Rc::clone(&activity),
        );

        // The app is holding an inhibit, so the blank does not fire at 300 --
        // and does not fire at 599 either.
        assert!(
            !activity
                .borrow_mut()
                .tick(t0 + Duration::from_secs(300), true),
            "an inhibit held by the realm the human is looking at holds the blank off"
        );
        assert!(!activity
            .borrow_mut()
            .tick(t0 + Duration::from_secs(599), true));
        assert_eq!(activity.borrow().phase(), Phase::Lit);

        // ...and the lock fires at 600 regardless. This is the assertion the
        // whole feature is bounded by.
        assert!(
            s.tick(t0 + Duration::from_secs(600)),
            "an idle inhibit must not hold the idle LOCK: a confined app that could suppress \
             `--lock-idle` would be a comfort feature disabling a security control, which \
             D-033(1) forbids"
        );
        assert_eq!(s.cause(), Some(LockCause::Idle));
        assert_eq!(
            activity.borrow().phase(),
            Phase::Lit,
            "and the lock raising did not blank anything either -- the two remain uncoupled in \
             both directions"
        );

        // Releasing it hands the countdown straight back: the deadline is
        // measured from the last physical event, not from the release, so a
        // session that has been idle throughout blanks on the very next round.
        assert!(
            activity
                .borrow_mut()
                .tick(t0 + Duration::from_secs(601), false),
            "releasing an inhibit restores the countdown rather than restarting it"
        );
        assert_eq!(activity.borrow().phase(), Phase::Covering);
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
