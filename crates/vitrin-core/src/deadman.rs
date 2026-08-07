// SPDX-License-Identifier: MPL-2.0
//! The dead-man switch (P1.7.3, issue #39): the human's unconditional
//! off-switch, reduced from PRD P10 to its MVP core — **holding one key for
//! a configured duration revokes every grant the core has issued, at once.**
//!
//! Everything else in this codebase is a rule about who may do what. This is
//! the rule that says a human can end all of it without asking anyone. Its
//! entire value is that nothing can defeat it, so this module is written
//! against an adversary rather than against a use case: an agent flooding
//! input, a wedged shim, a consent prompt on screen, a dead realm, a full
//! event queue, a timer that never fires, a partially-consumed chord. Each
//! of those is named below at the code that answers it.
//!
//! # Revocation flows through the ordinary grant path
//!
//! There is no kill switch, no side channel, and no second enforcement
//! surface. [`apply`] calls [`GrantTable::revoke_principal`] — which has
//! named this task in its doc comment since P1.4.2 — and the very next
//! [`Chokepoint::enforce_use`](crate::enforcement::Chokepoint::enforce_use)
//! refuses [`Refusal::Revoked`]. That is *why* the latency criterion is met
//! rather than approximated: the switch does not race the enforcement path,
//! it mutates the state the enforcement path reads, in the same
//! single-threaded turn. Both verbs die together because both verbs ask the
//! same table — capture and actuation reach `check_use_grant` through one
//! chokepoint, so there is no ordering in which one survives the other.
//!
//! # Design tension 1: `observe` cannot consume, but the shim must not see
//! the held chord
//!
//! [`crate::input::PreemptionHook`] documents `observe` as this task's
//! non-consuming tap, while issue #39 settles that the chord is evaluated
//! before any focus-based dispatch and "the shim never sees the held chord"
//! — which requires consumption. Both are right, because they are about
//! **two different jobs**, and [`DeadManHook`] implements both halves of the
//! trait to do them separately:
//!
//! - **`observe` is detection.** It is unconditional and cannot be stopped
//!   by any other policy: [`crate::consent::grab::ConsentGate`] forwards
//!   `observe` even for events it consumes entirely, so the switch keeps
//!   working while a consent prompt owns the screen — the moment revocation
//!   matters most. *All* switch state (`held_since`, `fired`, the trigger)
//!   is owned here, and `gate` may not touch it. That separation is the
//!   module's central safety invariant: **no bug on the suppression side can
//!   stop the switch from firing**, because the firing side never reads
//!   suppression state.
//! - **`gate` is suppression.** It decides only what the confined app sees.
//!   It is best-effort by construction, and its worst failure mode (below)
//!   is an Esc keystroke the app should not have received — never a
//!   revocation that did not happen.
//!
//! ## Swallowing Esc unconditionally is not acceptable, and a tap is not a
//! chord
//!
//! Esc is one of the most-used keys in a browser — it closes dialogs, stops
//! a load, leaves fullscreen — and P1.6.4 just brought Firefox up as the
//! demo app. A switch that costs the human the Esc key would be paid for on
//! every page, forever, to defend against a gesture used once a session.
//! So the rule is **tap-through, hold-swallow**, and the two are told apart
//! by the only thing that distinguishes them: whether the key came back up
//! before the chord completed.
//!
//! That cannot be decided at press time, which is the whole difficulty. The
//! press must therefore be **withheld** until its fate is known:
//!
//! | event | what the app sees |
//! |---|---|
//! | chord press | nothing yet — the press is buffered here |
//! | release **before** the hold elapses (a tap) | the buffered press *and* its release, replayed in order, late by however long the human held it (~80 ms for a real tap) |
//! | the hold elapses (a chord) | nothing, ever — the buffered press is dropped at the moment the switch fires |
//! | release **after** firing | nothing; the app never saw the press, so there is nothing to pair |
//!
//! **Why the press cannot simply be delivered and the release swallowed.**
//! That was the cheaper design and it is broken: [`Gate::Consume`] on a
//! release whose press *was* delivered reconciles the router's bookkeeping
//! but leaves the app holding the key down — a latched Esc, which is the
//! exact harm P1.7.2 fixed for modifiers ([`crate::consent::grab`]'s
//! hold-until-release section). Withholding the press is what makes
//! consuming the release safe here, and this gate consumes a release **only
//! when it also consumed that release's press**, so the pair is atomic and
//! the app's press/release accounting is never split. The router's own
//! per-keysym pairing confirms it: the reconciliation for the consumed
//! release finds no delivered press and does nothing.
//!
//! ## The replay is re-routed, not injected
//!
//! A tap's buffered press is handed back to the embedder by
//! [`DeadManSwitch::take_replay`] and routed through
//! [`InputRouter::route`](crate::input::InputRouter::route) again, exactly
//! like a fresh intake event. Two consequences worth stating:
//!
//! - **B2 is intact.** The replayed event is the *original* [`SeatInput`]
//!   that `intake_physical` minted, cloned — no origin tag is manufactured
//!   here, and `SeatInput::physical` remains private to [`crate::input`]
//!   with exactly one caller. This module buffers; it does not produce.
//! - **Every downstream policy still applies.** If a consent prompt went up
//!   between the press and the release, the replayed press meets
//!   `ConsentGate` on the way back and is consumed by it — the app gets
//!   nothing, which is correct, and this module needed no carve-out to
//!   achieve it.
//!
//! ## What a dropped replay costs (the failure mode, named)
//!
//! `take_replay` hands the pair to the embedder and records **those exact
//! events** in `replay_pending`, which `gate` matches against by value so the
//! replayed events pass its own withholding. An embedder that drains the
//! queue and then fails to route it — or a downstream policy that swallows
//! part of the drain, which `ConsentGate` really does — leaves entries
//! outstanding. The next chord event that does not match the front of that
//! queue clears it whole, so the *worst* case is one swallowed Esc tap.
//! **The switch still arms and still fires** either way, because arming lives
//! in `observe`, which never reads suppression state at all.
//!
//! Matching by value rather than by a count is load-bearing and was a real
//! bug: a bare credit granted in pairs but spent one per event desynchronises
//! the moment one half of a replayed pair is filtered outside this hook, and
//! the next genuine chord is then split — press delivered, release consumed,
//! Escape latched down in the confined app. See `replay_pending`'s own
//! comment for the full path, and
//! `a_prompt_between_a_taps_press_and_release_never_splits_a_later_chord`.
//!
//! ## What the core cannot see: a release that never arrives
//!
//! An armed hold is disarmed by exactly one thing — the chord key coming back
//! up — and that event can be lost. Verified in the pinned dependencies, not
//! assumed: Smithay 0.7.0's winit backend admits key events only under
//! `!is_synthetic && !event.repeat` (the same guard that filters autorepeat,
//! tension 2 below), which drops the synthetic releases winit emits on X11
//! focus loss; and winit 0.30.13's Wayland `wl_keyboard.leave` handler emits
//! `ModifiersChanged` and `Focused(false)` and no key events whatsoever.
//! An ordinary alt-tab with the key down therefore leaves the core believing
//! it is still held.
//!
//! Two answers, deliberately independent, because the second must hold even
//! where the first does not exist (a future backend with no focus concept):
//!
//! 1. [`DeadManSwitch::forget_hold`], called on host-window focus loss:
//!    without focus the hold is *unverifiable*, so it is cancelled. That doc
//!    comment carries the full argument for cancelling rather than firing,
//!    and for why it is not a defeat vector.
//! 2. `observe_event`'s press arm re-arms unconditionally, so no lost release
//!    can strand the switch in the `held_since = Some(_), fired = true` state
//!    — which is a state only a release can leave, and in which the switch is
//!    silently dead: no timer, no indicator, no firing.
//!
//! # Design tension 2: a hold needs a clock, and there is no autorepeat
//!
//! If the human presses the chord and holds it, **no further input event
//! arrives**. This was verified rather than assumed, and the answer is worse
//! than the usual "do not rely on autorepeat": Smithay 0.7.0's winit backend
//! filters key repeats out entirely before the core can see them —
//! `backend/winit/mod.rs`, the `WindowEvent::KeyboardInput { .. } if
//! !is_synthetic && !event.repeat` guard. A held key produces exactly **one**
//! press event and then silence until release.
//!
//! Evaluating the hold only on the next event would therefore make
//! revocation latency depend on whatever the human or the agent does next —
//! unbounded, and precisely the failure a dead-man switch may not have. So
//! the elapse check is driven by time, from three independent places:
//!
//! 1. **A `calloop` timer** armed at [`DeadManSwitch::deadline`], the
//!    primary path, giving one-shot firing at the hold boundary.
//! 2. **Every input dispatch turn**, because the switch is already being
//!    asked about that event.
//! 3. **Every frame**, at the host's redraw cadence (~16.7 ms).
//!
//! All three call the same idempotent [`DeadManSwitch::fire_if_due`], which
//! is **level-triggered**: it asks "is the chord still down and has the hold
//! elapsed", never "did an edge happen". A lost, early, or coalesced timer
//! therefore cannot defeat the switch — it only costs the difference between
//! the timer and the next frame. That is the answer to "a timer that never
//! fires": the timer is a latency optimisation over a path that is already
//! correct without it.
//!
//! # It fires exactly once per hold
//!
//! `fired` latches for the duration of one hold and clears on release, so a
//! human leaning on the key does not re-revoke every frame. Revocation is
//! idempotent anyway ([`GrantTable::revoke_principal`] returns only the ids
//! it *newly* revoked), but firing once keeps the flight recorder honest:
//! one human gesture is one entry.
//!
//! # Every principal, not just one
//!
//! The issue says "the agent principal's grants"; version 0 has one agent,
//! so the two readings coincide today. [`apply`] takes the wider one — every
//! principal holding a row in the table — for an adversarial reason: a
//! switch that revoked only one identity would be defeated by an attacker
//! who holds two. "Stop everything" is what the human's gesture means, and
//! it is the only reading that stays true when Phase 2 admits several
//! agents.
//!
//! # A pending consent prompt is denied, not left standing
//!
//! A human who has just hit the panic button is not, in that same second,
//! agreeing to grant new authority. Leaving a prompt up after a trigger
//! would be actively dangerous: its anti-tapjacking guard interval
//! ([`crate::consent::grab::GUARD_INTERVAL`]) has long since elapsed, so the
//! very next click would land on a live Allow button, handing back authority
//! the human just destroyed.
//!
//! So [`apply`] resolves every pending petition `Deny` through
//! [`PetitionRegistry::resolve_human`] — the ordinary human-decision path,
//! not a special case — which gives each petitioner a clean `resolved`
//! terminal instead of a hang. Taking the *card* off the screen needs no new
//! coupling: the petitions leave the pending table, so the embedder's
//! existing per-iteration
//! [`ConsentGrab::retire_stale`](crate::consent::grab::ConsentGrab::retire_stale)
//! lowers the prompt and releases the input grab on its next turn.
//!
//! # The switch does not just sweep, it seals
//!
//! Revoking the rows that exist at the instant the chord completes is not the
//! same as ending the session's authority, and the difference is a full
//! round-trip wide:
//!
//! - A consent decision made *before* the chord and delivered *after* it mints
//!   its row at delivery ([`crate::petitions`]' module docs: authority exists
//!   if and only if its wire handle resolved). At the moment [`apply`] runs it
//!   is neither a pending petition nor an existing row, so **both** halves of
//!   the sweep miss it.
//! - Under `ConsentPolicy::AutoApprove` the agent's next petition is granted
//!   by policy with no human involved — so the panic button would hold for
//!   exactly one round-trip.
//!
//! So [`apply`] also calls [`GrantTable::seal_dead_man`], after which every
//! row the table mints is born revoked and the chokepoint refuses it `revoked`
//! on first use, by the same path and with the same code as a swept row. The
//! wire cannot distinguish them, and there is still no side channel: the seal
//! is a property of the grant table, which is the one place authority is
//! created.
//!
//! This is what makes the gesture mean "stop, and stay stopped" rather than
//! "stop until the next petition". It is deliberately **not** undoable from
//! inside the session: a human who wants authority back after hitting the
//! off-switch restarts, which is the honest cost of a switch whose whole value
//! is that nothing inside can defeat it.
//!
//! # Configuration lives on the command line
//!
//! `--dead-man-chord` and `--dead-man-hold`, alongside `--consent`. That is
//! the precedent this follows exactly: a session-wide security policy, chosen
//! once at startup, that no client can influence. The alternatives were
//! considered and rejected — `realm.toml` describes *realms* (which app each
//! one launches) and this switch is not realm-scoped, it revokes across every
//! realm at once; [`PetitionConfig`](crate::petitions::PetitionConfig) is
//! petition-admission policy and the switch is not about petitions.
//!
//! **A chord the intake cannot deliver is refused at startup**, structurally:
//! [`Chord::parse`] validates every candidate against
//! [`crate::input::keysym_is_intakeable`], which asks the real
//! scancode→keysym table. Without that check, `--dead-man-chord f13` would
//! start a session whose off-switch silently never fires — a fail-open
//! configuration trap, which is the class of bug [`crate::realm`]'s loader
//! refuses for the same reason.
//!
//! # The switch is whole in nested mode
//!
//! It was mechanism-only until the runtime wiring: there was no
//! [`GrantTable`] and no [`PetitionRegistry`] in the process, so a completed
//! chord logged loudly and revoked nothing. Both now exist for the session's
//! whole life, and a completed chord reaches [`apply`] through
//! `session::Runtime::apply_dead_man` — revoking every grant, denying every
//! pending petition, sealing the table against a decision already in flight,
//! and delivering each denial to its petitioner over the wire.
//!
//! The ownership argument the previous gap rested on is what made that
//! possible rather than something worked around: the recorder was never
//! threaded into the backend, precisely because it belongs to the session
//! layer that also owns the grant table. That layer is `crate::session`, and
//! the backend reaches all three through it, borrowing nothing.
//!
//! **Headless has no chord**, structurally: no physical input device exists
//! there, so nothing can hold a key. The *detection* half — a human pressing
//! and holding a real key — is therefore a nested-mode guarantee only, and
//! stays that way; it is documented, workstation-only, in
//! `shim/docs/firefox.md`.
//!
//! # The test injector proves the *consequence* half on a GPU-less CI runner
//! (issue #109)
//!
//! What CI *can* prove against a real app, headless, is everything
//! downstream of a completed chord: that [`apply`] really does revoke every
//! grant, seal the table, deny every pending petition, and that the real
//! app then receives no further seat events — the actual security property,
//! as opposed to the input-device mechanics that produce the trigger. The
//! `dead-man-injector` cargo feature (never enabled in a deployment build —
//! same posture as [`crate::petitions`]'s `scripted-consent`) compiles a
//! `SIGUSR1` handler into the **headless** backend that synthesizes a
//! [`Trigger`] using the session's real configured chord/hold and calls the
//! exact same [`crate::session::Runtime::apply_dead_man`] entry point a
//! completed physical hold would. It is not a second, weaker revocation
//! path: it is the *same* embedder call, fed by a signal instead of a timer,
//! so what it proves about `apply`'s effect is exactly as strong as the
//! nested path's. What it does **not** prove — and cannot, because headless
//! has no input device to test it with — is that a real hold of a real key
//! reaches [`DeadManSwitch::observe_event`]/[`DeadManSwitch::fire_if_due`] in
//! the first place. That half stays nested-only.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

use crate::consent::Choice;
use crate::grants::{GrantId, GrantTable};
use crate::identity::PrincipalIdentity;
use crate::input::{Gate, PreemptionHook, SeatInput, SeatInputKind};
use crate::petitions::{PetitionRegistry, Resolution};
use crate::recorder::{Event, Recorder, REVOKE_CAUSE_DEAD_MAN, REVOKE_SCOPE_PRINCIPAL};

/// The default chord: Escape.
///
/// Chosen because it is the one key every human already understands as
/// "stop", it is layout-invariant on every keyboard (so
/// [`crate::input::invariant_keysym`] can always translate it — the reason
/// that table's docs call Escape out by name), and P1.7.2 deliberately left
/// it unclaimed in the consent grab so this task inherits it with no
/// carve-out.
pub(crate) const DEFAULT_CHORD: &str = "esc";

/// The default hold: one second, as the plan specifies.
///
/// Long enough that no ordinary Esc tap approaches it (a deliberate keypress
/// is 60–150 ms), short enough that a human reaching for the off-switch does
/// not have to wonder whether it is working.
pub(crate) const DEFAULT_HOLD: Duration = Duration::from_millis(1000);

/// Shortest configurable hold.
///
/// Below roughly this, the chord stops being a *hold* and becomes a
/// hair-trigger on an ordinary keypress: sources on deliberate keystroke
/// duration put a normal tap under ~150 ms, and a switch that fires on a tap
/// would revoke authority every time the human closed a dialog. It is also
/// what keeps the tap-vs-chord classification in `gate` meaningful — at a
/// zero hold every press is instantly a chord and the app would never see
/// Esc again.
pub(crate) const MIN_HOLD: Duration = Duration::from_millis(250);

/// Longest configurable hold. A switch the human must hold for a minute is
/// not an off-switch; the cap keeps a typo (`--dead-man-hold 100000`) from
/// quietly producing a session with no usable off-switch at all.
pub(crate) const MAX_HOLD: Duration = Duration::from_secs(10);

/// Fraction of the hold that must elapse before the on-screen indicator
/// appears (design note: user feedback while holding).
///
/// Not zero, and that is the whole point of the constant. Esc is pressed
/// constantly in a browser; an indicator that flashed on every tap would be
/// visual noise the human learns to ignore, which is how a warning stops
/// working. Revealing it partway through means it appears only once a tap has
/// been ruled out — and the remaining time is still ample warning that
/// something is about to happen, which is the accidental-trigger case the
/// indicator exists for.
const REVEAL_FRACTION: f64 = 0.45;

/// Ceiling on undrained replay events before the queue is dropped. Two taps'
/// worth: an embedder that has fallen this far behind is not draining at all,
/// and losing an app-visible Esc tap is the benign direction to fail in
/// (module docs: what a dropped replay costs).
const MAX_REPLAY_BACKLOG: usize = 4;

/// A configured chord key: the human-facing name and the keysym intake will
/// actually deliver for it.
///
/// Both halves are carried because both are needed and neither can be
/// derived from the other at the point of use — the keysym is what `observe`
/// matches, the name is what the flight recorder and the operator's log see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chord {
    name: &'static str,
    keysym: u32,
}

impl Chord {
    /// The chord vocabulary: name → keysym.
    ///
    /// Deliberately **excludes every modifier**, and that is a correctness
    /// rule rather than taste. Humans hold Shift for seconds at a time to
    /// select text and Ctrl through any chorded shortcut, so a modifier chord
    /// would fire during ordinary use — an off-switch with false positives is
    /// one the human learns to work around, and there is no way to work
    /// around this one.
    ///
    /// Every entry is layout-invariant, so [`crate::input::invariant_keysym`]
    /// can translate it under any keyboard layout; that this holds for all of
    /// them is asserted against the real table rather than trusted, in
    /// `every_offered_chord_survives_intake`.
    const VOCABULARY: &'static [(&'static str, u32)] = &[
        ("esc", 0xff1b),
        ("f1", 0xffbe),
        ("f2", 0xffbf),
        ("f3", 0xffc0),
        ("f4", 0xffc1),
        ("f5", 0xffc2),
        ("f6", 0xffc3),
        ("f7", 0xffc4),
        ("f8", 0xffc5),
        ("f9", 0xffc6),
        ("f10", 0xffc7),
        ("f11", 0xffc8),
        ("f12", 0xffc9),
        ("insert", 0xff63),
        ("delete", 0xffff),
        ("home", 0xff50),
        ("end", 0xff57),
        ("pageup", 0xff55),
        ("pagedown", 0xff56),
    ];

    /// Resolve a configured chord name.
    ///
    /// Refuses anything the vocabulary does not name, and — defence in depth
    /// against a vocabulary entry drifting from the intake table — anything
    /// intake could not deliver ([`crate::input::keysym_is_intakeable`]). A
    /// session must never come up with an off-switch that cannot fire.
    pub fn parse(name: &str) -> Result<Self, ChordError> {
        let (name, keysym) = Self::VOCABULARY
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .copied()
            .ok_or(ChordError::Unknown)?;
        if !crate::input::keysym_is_intakeable(keysym) {
            return Err(ChordError::NotIntakeable);
        }
        Ok(Self { name, keysym })
    }

    /// The chord's configured name (`"esc"`), for logs and the journal.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Every chord name this build accepts, for the CLI's error message.
    pub fn vocabulary() -> impl Iterator<Item = &'static str> {
        Self::VOCABULARY.iter().map(|(name, _)| *name)
    }
}

/// Why a configured chord was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChordError {
    /// Not a name this build offers.
    Unknown,
    /// Named, but nested intake cannot produce its keysym — so the switch
    /// could never fire. Unreachable while the vocabulary and the intake
    /// table agree; it exists so that if they ever stop agreeing, the failure
    /// is a startup error rather than a silently dead off-switch.
    NotIntakeable,
}

impl std::fmt::Display for ChordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChordError::Unknown => write!(f, "unknown chord key"),
            ChordError::NotIntakeable => {
                write!(f, "this build's input intake cannot deliver that key")
            }
        }
    }
}

/// The session's dead-man switch policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeadManConfig {
    pub chord: Chord,
    pub hold: Duration,
}

impl Default for DeadManConfig {
    fn default() -> Self {
        Self {
            chord: Chord::parse(DEFAULT_CHORD).expect("the default chord is in the vocabulary"),
            hold: DEFAULT_HOLD,
        }
    }
}

impl DeadManConfig {
    /// Set the hold from a configured millisecond count, refusing anything
    /// outside `[MIN_HOLD, MAX_HOLD]` (see those constants for why each bound
    /// exists). Inclusive at both ends: the bounds are the documented
    /// defensible values, not values just outside the defensible range.
    pub fn with_hold_ms(self, ms: u64) -> Result<Self, HoldError> {
        let hold = Duration::from_millis(ms);
        if hold < MIN_HOLD || hold > MAX_HOLD {
            return Err(HoldError { got: hold });
        }
        Ok(Self { hold, ..self })
    }
}

/// A configured hold outside the accepted range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoldError {
    pub got: Duration,
}

impl std::fmt::Display for HoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "hold {} ms is outside the accepted range {}..={} ms",
            self.got.as_millis(),
            MIN_HOLD.as_millis(),
            MAX_HOLD.as_millis()
        )
    }
}

/// One completed chord: the human held the key long enough, and authority
/// must now end.
///
/// A value rather than a direct call into the grant table, for the same
/// reason [`crate::consent::grab::Decision`] is one: the hook runs inside
/// [`InputRouter::route`](crate::input::InputRouter::route) and the router
/// holds no authority state and must never grow any
/// ([`crate::input`]'s module docs). The embedder drains this and applies it
/// where the table actually lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Trigger {
    /// The configured chord's name, carried so the journal can say *how* the
    /// human ended the session's authority.
    pub chord: &'static str,
    /// How long the key was actually held when the switch fired — at least
    /// the configured hold, and more when the elapse check ran late (a
    /// coalesced timer, a slow frame). Recorded as measured rather than as
    /// configured, because a replay reading "held 1000 ms" when the process
    /// was stalled for 300 ms of it would be a synthetic number.
    pub held: Duration,
}

/// What one trigger actually did, returned by [`apply`] so the embedder can
/// deliver the protocol terminals it owes.
///
/// Dead-code-allowed outside tests only because its two fields are read by
/// tests and by the nested backend's trigger disposal, which headless never
/// compiles a caller for.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Default)]
pub(crate) struct DeadManEffect {
    /// Rows this trigger newly revoked, ascending. Already journaled by
    /// [`apply`]; carried so the caller can log or surface a count.
    pub revoked: Vec<GrantId>,
    /// Petitions this trigger denied, as resolutions the caller must hand to
    /// `PrincipalServer::deliver_resolution` — the one path that flips the
    /// wire handle, emits `resolved`, and writes the `petition_resolved`
    /// journal entry. Deliberately **not** recorded here: that would
    /// double-record every denial, and the delivery funnel is where this
    /// codebase already records resolutions.
    pub denied: Vec<Resolution>,
}

/// The switch itself: the chord's hold state, and the app-visibility state
/// that keeps the chord from reaching the confined app.
///
/// Shared between the router's hook and the embedder by `Rc<RefCell<..>>` —
/// the shape [`crate::input::PresenceHook`] established and
/// [`crate::consent::grab::ConsentGrab`] followed.
///
/// **Field ownership is the safety invariant** (module docs, tension 1), and
/// it is a *read* invariant, which is the half that matters: the hold state
/// (`held_since`, `fired`, `trigger`) is written only by the detection path
/// ([`Self::observe_event`], the time-driven [`Self::fire_if_due`], and
/// [`Self::forget_hold`]), and **detection never reads suppression state at
/// all** — so no bug in `gate`, in the replay accounting, or in an embedder's
/// drain can stop the switch from firing.
///
/// The converse is deliberately *not* symmetric: detection clears the
/// suppression fields (`withheld`, `replay_pending`) when it decides a press
/// became a chord, or when a hold is forgotten. That direction is safe
/// because it can only ever result in the app being shown *less*, which is
/// the failure this side is allowed to have.
#[derive(Debug)]
pub(crate) struct DeadManSwitch {
    config: DeadManConfig,
    /// When the chord key went down; `None` while it is up.
    held_since: Option<Instant>,
    /// Whether the current hold already fired. Latches until release, so one
    /// human gesture is one revocation and one journal entry.
    fired: bool,
    /// A completed chord waiting for the embedder to apply it.
    trigger: Option<Trigger>,
    /// The chord press this gate held back from the app, kept so a tap can
    /// replay it (module docs). Cleared when the chord fires: a press that
    /// became a chord must never reach the app.
    withheld: Option<SeatInput>,
    /// Events handed to the embedder to re-route: a classified tap's press
    /// and its release, in that order.
    replay: Vec<SeatInput>,
    /// The exact events `gate` must let past untouched, in the order they
    /// were handed out, so a replayed pair is not withheld a second time.
    ///
    /// **This is a queue of identities, not a count, and that is a fix rather
    /// than a preference.** It was a `usize` credit, granted two at a time
    /// (the queue always holds a press/release pair) and spent one per event.
    /// That is only sound if *both* halves of every replayed pair reach this
    /// gate — and they do not. [`crate::consent::grab::ConsentGate`] sits
    /// **outside** this hook and its hold-until-release exception *delivers*
    /// physical key releases while *consuming* physical key presses. So a
    /// replay drained while a consent prompt is up loses its press to the
    /// grab (this gate never sees it, so no credit is spent) while its
    /// release falls through and spends one. The credit lands on an **odd**
    /// number, and the next genuine chord is split down the middle: its press
    /// delivered on the stale credit, its release consumed — a latched Escape
    /// in the confined app, which is precisely the harm P1.7.2 fixed for
    /// modifiers and precisely what [`crate::input::PreemptionHook`]'s
    /// pairing exception forbids ("a gate that consumed a press may consume
    /// that press's release, *because then nothing is stranded*").
    ///
    /// Matching on the event value cannot drift that way: an event that never
    /// reached this gate cannot spend another event's pass, because the pass
    /// names the event it belongs to. Any mismatch clears the whole queue
    /// rather than trying to resynchronise — a replay that did not arrive in
    /// the order it was handed out is evidence the drain was filtered, and
    /// the fail-safe reading of a filtered drain is "the app is owed
    /// nothing", which costs at most a swallowed Esc tap and never a split
    /// pair.
    replay_pending: VecDeque<SeatInput>,
}

impl DeadManSwitch {
    pub fn new(config: DeadManConfig) -> Self {
        Self {
            config,
            held_since: None,
            fired: false,
            trigger: None,
            withheld: None,
            replay: Vec::new(),
            replay_pending: VecDeque::new(),
        }
    }

    /// **Detection.** Update the hold from one observed intake event, then
    /// complete the chord if this event happens to arrive after the hold
    /// elapsed.
    ///
    /// Only **physical** input touches the hold, and both directions of that
    /// matter. An agent must not be able to *arm* the human's switch (it
    /// would be a way to spend the human's gesture on the agent's timing),
    /// and — the sharper one — an agent flooding emulated input must not be
    /// able to *disarm* a hold in progress. Neither is reachable, because
    /// emulated events return before touching anything.
    ///
    /// Non-chord input is deliberately ignored rather than treated as a
    /// cancellation. A hold that other keystrokes cancelled would be
    /// defeatable by anything that can produce input, and would break the
    /// honest case of a human who keeps using the machine with one finger on
    /// the off-switch.
    pub fn observe_event(&mut self, input: &SeatInput, now: Instant) {
        if input.origin() != Origin::Physical {
            return;
        }
        let SeatInputKind::Key { keysym, state } = input.kind() else {
            return;
        };
        if *keysym != self.config.chord.keysym {
            return;
        }
        // **Before this event is interpreted at all.** If the hold already
        // elapsed and nothing observed it -- the timer lost *and* the frame
        // backstop starved -- then the chord has completed, and the release
        // arriving now is the end of a chord rather than the end of a tap.
        //
        // This is the one place a missed tick could have *defeated* the
        // switch instead of merely delaying it: classifying afterwards, an
        // overdue release would take the `was_held && !fired` branch, replay
        // the withheld press into the app, and quietly downgrade a completed
        // chord into an ordinary Escape keystroke with no revocation at all.
        // Firing first makes the elapse check level-triggered on this edge
        // too, so the outcome depends on the clock and never on whether
        // anyone happened to look.
        self.fire_if_due(now);
        match state {
            KeyState::Pressed => {
                // A second press with no intervening release is not
                // physically possible at the keyboard, so reaching here with
                // `held_since` already set means **the release was lost** --
                // and a lost release is not hypothetical. Smithay 0.7.0's
                // winit backend admits key events only under `!is_synthetic
                // && !event.repeat`, which drops the synthetic releases winit
                // emits on X11 focus loss; on Wayland winit's
                // `wl_keyboard.leave` handler emits `ModifiersChanged` and
                // `Focused(false)` and no key events at all. Either way the
                // core can be left believing a key is still down.
                //
                // Two readings, and the choice between them is not a matter
                // of taste:
                //
                // - *Keep the earlier arm time* (the previous behaviour) is
                //   fail-safe only while `fired` is false -- it fires sooner,
                //   never later. Once a lost release has let a hold fire,
                //   `held_since` stays `Some` and `fired` stays `true`, and
                //   that pair is a state **only a release can leave**:
                //   `deadline` returns `None` (no timer is ever armed),
                //   `hold_progress` returns `None` (the indicator never
                //   appears), and `fire_if_due` returns early forever. The
                //   human's off-switch is then silently dead until they
                //   happen to complete one full press+release -- which this
                //   gate also swallows, so it is invisible to them and to the
                //   app.
                // - *Re-arm from this press* costs nothing real: within one
                //   genuine hold there is exactly one press event (repeats
                //   are filtered upstream, module docs, tension 2), so a
                //   press arriving while armed is evidence of a lost release
                //   and never of a hold in progress.
                //
                // So the arming path is made unable to reach a state only a
                // release can exit. This is deliberately independent of the
                // focus-loss disarm in `crate::backend::winit` -- that
                // handles the known cause, this makes *any* lost release cost
                // at most the gesture it happened during.
                if self.held_since.is_none() || self.fired {
                    self.held_since = Some(now);
                    self.fired = false;
                }
            }
            KeyState::Released => {
                let was_held = self.held_since.take().is_some();
                let fired = std::mem::replace(&mut self.fired, false);
                // A tap: the key came back up before the chord completed, so
                // the app is owed the press this gate withheld, followed by
                // this release. `withheld` is `None` when the press was never
                // withheld in the first place (a replayed press, or one
                // delivered on a stale credit), which is exactly what stops a
                // replay from replaying itself.
                if was_held && !fired {
                    if let Some(press) = self.withheld.take() {
                        self.queue_replay(press, input.clone());
                    }
                } else {
                    // Fired, or a release with no press we saw: nothing is
                    // owed to the app, and a buffered press from a chord must
                    // not survive into a later tap.
                    self.withheld = None;
                }
            }
        }
    }

    /// Queue a classified tap for the embedder to re-route, dropping the
    /// whole queue if the embedder is evidently not draining it
    /// ([`MAX_REPLAY_BACKLOG`]).
    fn queue_replay(&mut self, press: SeatInput, release: SeatInput) {
        if self.replay.len() >= MAX_REPLAY_BACKLOG {
            tracing::warn!(
                queued = self.replay.len(),
                "dead-man replay queue is not being drained; dropping buffered chord taps"
            );
            self.replay.clear();
        }
        self.replay.push(press);
        self.replay.push(release);
    }

    /// **Detection, time-driven.** Complete the chord if the key is still
    /// down and the hold has elapsed. Idempotent and cheap: safe to call from
    /// the timer, from every input turn, and from every frame (module docs:
    /// the level-triggered elapse check).
    pub fn fire_if_due(&mut self, now: Instant) {
        let Some(since) = self.held_since else {
            return;
        };
        if self.fired {
            return;
        }
        let held = now.saturating_duration_since(since);
        if held < self.config.hold {
            return;
        }
        self.fired = true;
        // The press became a chord: the app must never receive it.
        self.withheld = None;
        self.trigger = Some(Trigger {
            chord: self.config.chord.name,
            held,
        });
        // Loud, unconditionally, before anything else can go wrong: this is
        // the most consequential thing a human can do to a session, and it
        // must be visible in the operator's log even if the embedder that
        // drains the trigger is broken.
        tracing::warn!(
            chord = self.config.chord.name,
            held_ms = held.as_millis(),
            "dead-man chord completed: revoking all delegated authority"
        );
    }

    /// **Detection.** Forget any hold in progress: the core can no longer
    /// verify that the human is holding the key.
    ///
    /// Called when the host window loses keyboard focus
    /// ([`crate::backend::winit`]). Focus loss is the one event that makes an
    /// armed hold *unverifiable* rather than merely unobserved: the release
    /// will be delivered to whatever window took focus, and — verified in the
    /// pinned dependencies rather than assumed — neither backend tells us
    /// about it. Smithay 0.7.0 filters `is_synthetic` key events, which is
    /// how X11's focus-loss releases are lost; winit 0.30.13's Wayland
    /// `wl_keyboard.leave` handler emits no key events at all.
    ///
    /// **Cancelling is the fail-safe direction here, and the reasoning is
    /// asymmetric rather than obvious.** Firing looks like the cautious
    /// choice — this is a safety switch, and safety switches fail on. It is
    /// not, for three reasons:
    ///
    /// 1. The switch's entire premise is *a human is physically holding this
    ///    key right now*. Without focus there is no evidence of that, and
    ///    firing on absent evidence makes the most destructive operation the
    ///    system offers reachable by a plain alt-tab mid-tap.
    /// 2. The consequence would be journaled as a completed human gesture —
    ///    `dead_man_triggered` with a measured `held_ms` indistinguishable
    ///    from a real hold — which corrupts the one surface the acceptance
    ///    criteria name for auditing exactly this.
    /// 3. The failure is *recoverable in the direction that matters*: a
    ///    cancelled hold costs the human one repeated gesture, which they can
    ///    perform immediately and which the indicator shows them. A spurious
    ///    revocation costs the session's authority and cannot be undone from
    ///    the keyboard at all.
    ///
    /// **It is not a defeat vector.** A confined agent cannot move host-window
    /// focus: in nested mode the app renders *into* this window through the
    /// shim and has no host surface of its own, and emulated input is refused
    /// by [`Self::observe_event`]'s origin check before it can touch any state
    /// here. The only principal who can unfocus this window is the human at
    /// the host compositor — the same human whose gesture this is.
    ///
    /// Clears the suppression state too, not only the hold. The withheld
    /// press belongs to a gesture the core can no longer classify, and
    /// delivering it later — on a keyboard focus the app no longer has —
    /// would be a stray Escape arriving out of nowhere.
    pub fn forget_hold(&mut self) {
        self.held_since = None;
        self.fired = false;
        self.withheld = None;
        self.replay_pending.clear();
    }

    /// When the armed hold will elapse, for the embedder's timer. `None` when
    /// the key is up or the chord already fired — which is what lets the
    /// timer callback decide to stop rescheduling itself.
    pub fn deadline(&self) -> Option<Instant> {
        match (self.held_since, self.fired) {
            (Some(since), false) => Some(since + self.config.hold),
            _ => None,
        }
    }

    /// Drain a completed chord. The embedder applies it with [`apply`].
    pub fn take_trigger(&mut self) -> Option<Trigger> {
        self.trigger.take()
    }

    /// Drain the events the embedder must re-route (module docs: the replay
    /// is re-routed, not injected). Empty in the overwhelmingly common case.
    pub fn take_replay(&mut self) -> Vec<SeatInput> {
        if self.replay.is_empty() {
            return Vec::new();
        }
        let out = std::mem::take(&mut self.replay);
        // Replaces whatever was outstanding rather than appending to it: if a
        // previous drain never came back through the gate, its passes are
        // stale, and carrying them forward is exactly how a stale pass ends up
        // spent by a *later* gesture's press.
        self.replay_pending = out.iter().cloned().collect();
        out
    }

    /// How far through the hold the human is, `0.0..=1.0`, or `None` when
    /// there is nothing a human should be shown — the key is up, the chord
    /// already fired, or the hold has not yet passed [`REVEAL_FRACTION`].
    pub fn hold_progress(&self, now: Instant) -> Option<f64> {
        let since = self.held_since?;
        if self.fired {
            return None;
        }
        let hold = self.config.hold.as_secs_f64();
        if hold <= 0.0 {
            return None;
        }
        let progress = now.saturating_duration_since(since).as_secs_f64() / hold;
        (progress >= REVEAL_FRACTION).then(|| progress.clamp(0.0, 1.0))
    }

    /// **Suppression.** Decide whether one event reaches the confined app.
    ///
    /// Touches no detection state — the module's safety invariant. Everything
    /// that is not a physical chord-key event is passed straight through:
    /// emulated input is the chokepoint's business, not the router's (the
    /// same razor [`crate::consent::grab`] applies, for the same reason —
    /// consuming it here would silently overturn an authority decision the
    /// core just made).
    pub fn gate_event(&mut self, input: &SeatInput) -> Gate {
        if input.origin() != Origin::Physical {
            return Gate::Deliver;
        }
        let SeatInputKind::Key { keysym, state } = input.kind() else {
            return Gate::Deliver;
        };
        if *keysym != self.config.chord.keysym {
            return Gate::Deliver;
        }
        // A replayed event: this gate already judged its original and is
        // handing the pair back to the app on purpose. Matched by value and
        // in order (see `replay_pending`): a pass names the event it belongs
        // to, so an event the outer consent gate swallowed cannot leave a
        // pass behind for some later event to spend.
        if let Some(front) = self.replay_pending.front() {
            if front == input {
                self.replay_pending.pop_front();
                return Gate::Deliver;
            }
            // Out of order, or an event that was never replayed at all: the
            // drain was filtered somewhere outside this hook. Drop the whole
            // queue rather than resynchronising -- see `replay_pending`.
            self.replay_pending.clear();
        }
        match state {
            // Withheld until the tap/chord question is answered (module docs).
            KeyState::Pressed => {
                // Newly withholding: nothing outstanding can still be owed to
                // the app, and a leftover pass here is the one thing that
                // could split *this* gesture's pair.
                self.replay_pending.clear();
                self.withheld = Some(input.clone());
                Gate::Consume
            }
            // Consumed in both outcomes, and safe in both: a tap's pair is
            // replayed by the embedder, and a chord's press was never
            // delivered — so the router's per-keysym pairing finds nothing to
            // reconcile and the app is left holding nothing. This is the one
            // release this codebase consumes, and it is sound only because
            // the same gate consumed its press.
            KeyState::Released => Gate::Consume,
        }
    }
}

/// Apply one completed chord: revoke every grant, deny every pending
/// petition, and journal both with their cause.
///
/// The order is deliberate and is what a replay reader needs: the cause
/// (`dead_man_triggered`) is written **first**, then the individual
/// `grant_revoked` lines it explains. The trigger entry is written even when
/// nothing was revoked and nothing was pending — without it, a session where
/// the human hit the off-switch and it had nothing to do would be
/// indistinguishable from one where they never hit it, and "the switch
/// works" would be unverifiable from the log.
///
/// Called at runtime through `session::Runtime::apply_dead_man`, which is
/// what owns a [`GrantTable`], a [`PetitionRegistry`] and the run's
/// [`Recorder`] together; the nested backend's trigger disposal reaches it
/// from there. Exercised end to end here against all three.
///
/// No `allow(dead_code)`: `backend::winit` is compiled unconditionally, so
/// the production caller exists in every build — including the
/// `--headless`-only one, which simply never reaches it for want of a
/// physical input device to hold a chord on. An attribute here would silence
/// the warning that a future refactor dropping that caller is meant to
/// produce.
pub(crate) fn apply(
    trigger: &Trigger,
    grants: &mut GrantTable,
    petitions: &mut PetitionRegistry,
    recorder: &mut Recorder,
    now: Instant,
) -> DeadManEffect {
    // Every principal holding a row, not one (module docs: an off-switch that
    // revoked a single identity is defeated by an attacker holding two). The
    // ids are collected before mutating, because `rows` borrows the table.
    let mut principals: Vec<PrincipalIdentity> = grants
        .rows(now)
        .map(|(row, _state)| row.principal_id.clone())
        .collect();
    principals.sort();
    principals.dedup();

    let mut revoked = Vec::new();
    for principal in &principals {
        revoked.extend(grants.revoke_principal(principal));
    }
    revoked.sort();

    // Sealing is what makes this a *revocation of the session's authority*
    // rather than a sweep of one instant. The sweep above cannot reach a
    // consent decision that was made before the chord and is delivered after
    // it -- the row is minted at delivery, so at this moment it is neither
    // pending nor present -- nor can it stop `ConsentPolicy::AutoApprove` from
    // handing the identical authority back on the agent's next petition, one
    // round-trip later. `seal_dead_man` closes both by making every row minted
    // from here on born revoked; see its doc comment for why that lives on the
    // table rather than at each caller.
    grants.seal_dead_man();

    // Every pending petition is denied through the ordinary human-decision
    // path. `front_pending` rather than the build-gated `pending_ids`, so this
    // works in a deployment build; each `resolve_human` consumes the petition
    // it names, so the loop strictly shrinks the table. The `Err` arm is
    // unreachable (the id came from the pending table in the same borrow) and
    // breaks rather than retrying, so a future change to either function can
    // never turn this into a spin.
    let mut denied = Vec::new();
    while let Some(petition) = petitions.front_pending() {
        match petitions.resolve_human(petition, Choice::Deny) {
            Ok(resolution) => denied.push(resolution),
            Err(err) => {
                tracing::error!(
                    %petition,
                    ?err,
                    "dead-man switch could not deny a pending petition; stopping the sweep"
                );
                break;
            }
        }
    }

    recorder.record(Event::DeadManTriggered {
        chord: trigger.chord,
        held_ms: trigger.held.as_millis().min(u64::MAX as u128) as u64,
        revoked_grants: revoked.len(),
        denied_petitions: denied.len(),
    });
    recorder.record_revocations(&revoked, REVOKE_SCOPE_PRINCIPAL, REVOKE_CAUSE_DEAD_MAN);

    tracing::warn!(
        chord = trigger.chord,
        held_ms = trigger.held.as_millis(),
        principals = principals.len(),
        revoked = revoked.len(),
        denied = denied.len(),
        "dead-man switch applied"
    );

    DeadManEffect { revoked, denied }
}

/// The switch attached at the router's preemption hook point: the innermost
/// hook, under P1.7.2's consent grab, exactly as [`crate::input`]'s module
/// docs promised (`ConsentGate<NoopHook>` becomes `ConsentGate<DeadManHook>`).
///
/// Being innermost is what makes the switch undefeatable rather than merely
/// last: `ConsentGate` forwards `observe` unconditionally — it is documented
/// and tested to do so — so detection runs even for events the grab swallows
/// whole. The `gate` half *is* short-circuited by a raised prompt, and that
/// is correct: while a prompt is up the grab already stops the chord from
/// reaching the app, which is all this hook's gate half wanted.
pub(crate) struct DeadManHook<H: PreemptionHook> {
    switch: Rc<RefCell<DeadManSwitch>>,
    /// The dispatch turn's instant, shared with the embedder. The hook trait
    /// carries no clock by design (the router never reads one), so the
    /// embedder advances this cell — the arrangement
    /// [`crate::input::PresenceHook`] and
    /// [`crate::consent::grab::ConsentGate`] both use.
    now: Rc<Cell<Instant>>,
    inner: H,
}

impl<H: PreemptionHook> DeadManHook<H> {
    pub fn new(switch: Rc<RefCell<DeadManSwitch>>, now: Rc<Cell<Instant>>, inner: H) -> Self {
        Self { switch, now, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for DeadManHook<H> {
    fn observe(&mut self, input: &SeatInput) {
        self.switch
            .borrow_mut()
            .observe_event(input, self.now.get());
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        match self.switch.borrow_mut().gate_event(input) {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }
}

/// Paint the hold indicator onto **human-visible** output.
///
/// A thin bar across the top edge of the screen, filling left to right as the
/// hold completes. Three things about it are deliberate:
///
/// - **It is the last thing composited**, above the consent card, so a prompt
///   can never hide the fact that the human is mid-gesture on the off-switch.
/// - **The top edge, not the centre**, because that is where system-level
///   indicators live (screen-recording bars on every mobile platform), and
///   because it must not read as app content or as a dialog the human should
///   answer.
/// - **It inherits P1.7.1's fork.** This is called from the nested backend's
///   window composition, downstream of the point where capture takes the bare
///   realm view ([`crate::backend`]), so — like the consent overlay, and for
///   the same structural reason rather than a checked flag — it can never
///   reach `vitrin_view.frame_ready`.
///
/// `progress` is clamped; `view` must be `width * height * 4` RGBA8888.
pub(crate) fn composite_hold_indicator(view: &mut [u8], width: u32, height: u32, progress: f64) {
    use crate::consent::canvas::{Canvas, Rect};

    /// Bar height in pixels: readable at a glance, small enough that it never
    /// occludes anything the human is working with.
    const BAR_H: u32 = 6;
    /// The unfilled track, and the filled portion. Amber rather than red: the
    /// gesture has not completed yet, and this is a warning that something is
    /// about to happen, not a report that it did.
    const TRACK: [u8; 4] = [0x2a, 0x1e, 0x00, 0xff];
    const FILL: [u8; 4] = [0xff, 0xb0, 0x20, 0xff];

    if width == 0 || height == 0 {
        return;
    }
    let Some(mut canvas) = Canvas::new(view, width, height) else {
        return;
    };
    let progress = progress.clamp(0.0, 1.0);
    let h = BAR_H.min(height);
    canvas.fill_rect(
        Rect {
            x: 0,
            y: 0,
            w: width,
            h,
        },
        TRACK,
    );
    // Truncating, so the bar reaches the full width only at a genuinely
    // complete hold rather than one pixel-rounding short of it.
    let filled = (f64::from(width) * progress) as u32;
    if filled > 0 {
        canvas.fill_rect(
            Rect {
                x: 0,
                y: 0,
                w: filled.min(width),
                h,
            },
            FILL,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use vitrin_protocol::generated::vitrin_grant::{Persistence as WirePersistence, Refusal, Verb};

    use super::*;
    use crate::consent::tests::PROMPT_IDENTITY;
    use crate::grants::{GrantSpec, Issuer, PersistenceRung, RealmId, RefusalReason, ResourceRef};
    use crate::petitions::{
        Admission, ConsentPolicy, PetitionConfig, PetitionRegistry, PetitionRequest,
    };
    use crate::recorder::tests::{cleanup, scratch_recorder};

    const OTHER_IDENTITY: &str = "vitrin://local/agent/other";

    fn identity(text: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(text).expect("fixture identity")
    }

    /// A table holding one `while_running` grant for `who`, with every verb
    /// this task's acceptance criteria exercise.
    fn grant_for(table: &mut GrantTable, who: &PrincipalIdentity, at: Instant) -> GrantId {
        table
            .insert(
                GrantSpec {
                    principal_id: who.clone(),
                    realm_id: RealmId::new("realm-0"),
                    resource_ref: ResourceRef::WholeRealm,
                    verbs: Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT,
                    expiry: None,
                    max_event_rate: NonZeroU32::new(20).unwrap(),
                    persistence: PersistenceRung::WhileRunning,
                    issuer: Issuer::HumanConsent,
                },
                at,
            )
            .expect("a valid row")
    }

    fn switch() -> DeadManSwitch {
        DeadManSwitch::new(DeadManConfig::default())
    }

    /// A recorder whose file is removed when the guard drops.
    struct Scratch {
        recorder: Recorder,
        path: std::path::PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let (recorder, path) = scratch_recorder("deadman");
            Self { recorder, path }
        }

        /// Every entry written so far, parsed with the module's own reader
        /// (which also asserts the envelope and gap-free `seq`).
        fn entries(&self) -> Vec<crate::recorder::tests::Json> {
            crate::recorder::tests::read_log(&self.path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            cleanup(&self.path);
        }
    }

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    #[test]
    fn every_offered_chord_survives_intake() {
        // The fail-open trap this guards: a chord whose keysym nested intake
        // never produces would start a session with an off-switch that can
        // never fire. Asserted against the real scancode->keysym table, so a
        // vocabulary entry that drifts from it fails here rather than in the
        // field.
        for name in Chord::vocabulary() {
            let chord = Chord::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                crate::input::keysym_is_intakeable(chord.keysym),
                "chord `{name}` (keysym {:#x}) cannot be produced by intake",
                chord.keysym
            );
        }
        // And the default is one of them.
        assert_eq!(DeadManConfig::default().chord.name(), DEFAULT_CHORD);
        assert_eq!(DeadManConfig::default().chord.keysym, 0xff1b);
    }

    #[test]
    fn the_chord_vocabulary_excludes_modifiers_and_unknown_names() {
        // Modifiers are held for seconds during ordinary use (text selection,
        // any chorded shortcut), so a modifier chord would fire by accident.
        for modifier in ["shift", "ctrl", "alt", "super", "capslock"] {
            assert_eq!(Chord::parse(modifier), Err(ChordError::Unknown));
        }
        assert_eq!(Chord::parse("f13"), Err(ChordError::Unknown));
        assert_eq!(Chord::parse(""), Err(ChordError::Unknown));
        assert_eq!(Chord::parse("ESC"), Err(ChordError::Unknown));
    }

    #[test]
    fn the_hold_is_bounded_at_both_ends() {
        let base = DeadManConfig::default();
        assert_eq!(base.hold, DEFAULT_HOLD);
        // Inclusive bounds.
        assert_eq!(
            base.with_hold_ms(MIN_HOLD.as_millis() as u64).unwrap().hold,
            MIN_HOLD
        );
        assert_eq!(
            base.with_hold_ms(MAX_HOLD.as_millis() as u64).unwrap().hold,
            MAX_HOLD
        );
        // A hair-trigger hold would fire on an ordinary Esc tap.
        assert!(base.with_hold_ms(0).is_err());
        assert!(base.with_hold_ms(MIN_HOLD.as_millis() as u64 - 1).is_err());
        // A hold nobody can complete is not an off-switch.
        assert!(base.with_hold_ms(MAX_HOLD.as_millis() as u64 + 1).is_err());
        assert!(base.with_hold_ms(u64::MAX).is_err());
    }

    // ------------------------------------------------------------------
    // The hold: timing, and the fact that nothing else has to happen
    // ------------------------------------------------------------------

    #[test]
    fn the_chord_fires_on_time_with_no_further_input() {
        // Design tension 2, asserted directly: Smithay's winit backend drops
        // key repeats, so a held key produces exactly one press event. The
        // switch must complete from the clock alone -- `fire_if_due` is called
        // here with no intervening event, exactly as the timer calls it.
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);

        // Nothing has happened yet, and the deadline is exactly the hold.
        assert!(s.take_trigger().is_none());
        assert_eq!(s.deadline(), Some(t0 + DEFAULT_HOLD));

        // One microsecond short: still nothing. Fail-closed at the boundary
        // would be firing early; this is the honest half-open reading.
        s.fire_if_due(t0 + DEFAULT_HOLD - Duration::from_micros(1));
        assert!(s.take_trigger().is_none());

        // At the boundary: fired, with the hold it measured.
        s.fire_if_due(t0 + DEFAULT_HOLD);
        let trigger = s.take_trigger().expect("the chord completed");
        assert_eq!(trigger.chord, "esc");
        assert_eq!(trigger.held, DEFAULT_HOLD);
        // Armed no longer: the timer stops rescheduling.
        assert_eq!(s.deadline(), None);
    }

    #[test]
    fn a_late_elapse_check_reports_the_hold_it_measured() {
        // The timer may fire late (a coalesced wakeup, a stalled process) and
        // the frame backstop later still. The journal must say what actually
        // happened, not what was configured.
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);
        s.fire_if_due(t0 + Duration::from_millis(1731));
        assert_eq!(
            s.take_trigger().expect("fired").held,
            Duration::from_millis(1731)
        );
    }

    #[test]
    fn a_release_before_the_hold_disarms_and_a_second_press_starts_over() {
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);
        s.observe_event(
            &crate::input::tests::chord_release(),
            t0 + Duration::from_millis(80),
        );
        assert_eq!(s.deadline(), None);
        // Long past where the first press would have fired: nothing.
        s.fire_if_due(t0 + DEFAULT_HOLD * 4);
        assert!(s.take_trigger().is_none());

        // A fresh press arms from its own instant, not from the first one.
        let t1 = t0 + DEFAULT_HOLD * 4;
        s.observe_event(&crate::input::tests::chord_press(), t1);
        assert_eq!(s.deadline(), Some(t1 + DEFAULT_HOLD));
        s.fire_if_due(t1 + DEFAULT_HOLD);
        assert!(s.take_trigger().is_some());
    }

    #[test]
    fn one_hold_fires_exactly_once_however_long_it_is_held() {
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);
        s.fire_if_due(t0 + DEFAULT_HOLD);
        assert!(s.take_trigger().is_some());
        // Leaning on the key must not re-revoke every frame for a minute.
        for step in 1..60 {
            s.fire_if_due(t0 + DEFAULT_HOLD + Duration::from_millis(step * 17));
            assert!(s.take_trigger().is_none(), "fired twice on one hold");
        }
        // Only a release-then-press re-arms it.
        let t1 = t0 + Duration::from_secs(2);
        s.observe_event(&crate::input::tests::chord_release(), t1);
        s.observe_event(&crate::input::tests::chord_press(), t1);
        s.fire_if_due(t1 + DEFAULT_HOLD);
        assert!(s.take_trigger().is_some());
    }

    #[test]
    fn an_overdue_release_completes_the_chord_instead_of_replaying_a_tap() {
        // The one way a missed tick could *defeat* the switch rather than
        // delay it, pinned as a regression test.
        //
        // Simulates the worst case for the timing path: the timer never
        // fired AND no frame ran, so nothing called `fire_if_due` for the
        // whole hold, and the next thing the switch sees is the release --
        // long past the deadline. Classified naively this reads as "the key
        // came back up and never fired", i.e. a tap: the withheld press
        // would be replayed into the app and no revocation would happen at
        // all. It must instead complete the chord.
        let t0 = Instant::now();
        let mut s = switch();
        let press = crate::input::tests::chord_press();
        let release = crate::input::tests::chord_release();

        s.observe_event(&press, t0);
        assert_eq!(s.gate_event(&press), Gate::Consume);

        // No `fire_if_due` in between: the release is the first thing that
        // looks at the clock again, and it is well past the deadline.
        let late = t0 + DEFAULT_HOLD * 3;
        s.observe_event(&release, late);
        assert_eq!(s.gate_event(&release), Gate::Consume);

        let trigger = s
            .take_trigger()
            .expect("an overdue release must complete the chord, not cancel it");
        assert_eq!(trigger.held, DEFAULT_HOLD * 3);
        assert!(
            s.take_replay().is_empty(),
            "a completed chord must never be replayed into the app as a tap"
        );
    }

    #[test]
    fn only_physical_chord_input_moves_the_switch() {
        // An agent must not be able to arm the human's switch, and -- the
        // sharper direction -- must not be able to disarm a hold in progress
        // by flooding emulated input.
        let t0 = Instant::now();
        let mut s = switch();

        // Emulated chord presses arm nothing.
        s.observe_event(&crate::input::tests::emulated_chord_press(), t0);
        assert_eq!(s.deadline(), None);
        s.fire_if_due(t0 + DEFAULT_HOLD * 2);
        assert!(s.take_trigger().is_none());

        // A real hold, flooded with emulated input of every kind including a
        // chord release: the hold survives and fires on time.
        s.observe_event(&crate::input::tests::chord_press(), t0);
        for step in 0..50 {
            let at = t0 + Duration::from_millis(step * 10);
            s.observe_event(&crate::input::tests::emulated_chord_release(), at);
            s.observe_event(&crate::input::tests::emulated_motion(), at);
        }
        s.fire_if_due(t0 + DEFAULT_HOLD);
        assert!(
            s.take_trigger().is_some(),
            "an agent flooding input defeated the human's off-switch"
        );
    }

    #[test]
    fn unrelated_physical_input_does_not_cancel_a_hold() {
        // A hold that other keystrokes cancelled would be defeatable by
        // anything that can produce input, and would punish a human who keeps
        // working with one finger on the off-switch.
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);
        for step in 0..20 {
            s.observe_event(
                &crate::input::tests::other_key_press(),
                t0 + Duration::from_millis(step * 10),
            );
        }
        s.fire_if_due(t0 + DEFAULT_HOLD);
        assert!(s.take_trigger().is_some());
    }

    // ------------------------------------------------------------------
    // A release that never arrives
    // ------------------------------------------------------------------

    #[test]
    fn a_lost_release_cannot_leave_the_switch_unable_to_fire_again() {
        // The critical failure this guards, in the order it happens in the
        // field: the human presses the chord, host-window focus moves away
        // before they let go, and the release is delivered to another window.
        // Neither backend reports it (Smithay filters `is_synthetic`; winit's
        // Wayland `leave` emits no key events), so the core still believes
        // the key is down. The hold then completes on the timer.
        //
        // Before the fix that left `held_since = Some(_)` with `fired = true`
        // -- a state ONLY a release can leave. Every subsequent hold was
        // silently inert: no deadline, so no timer was ever armed; no
        // progress, so the indicator never appeared; and `fire_if_due`
        // returned early forever. The human's unconditional last resort was
        // dead with nothing on screen to say so.
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);
        s.fire_if_due(t0 + DEFAULT_HOLD);
        assert!(
            s.take_trigger().is_some(),
            "fixture check: the first hold fired"
        );

        // ... and the release never arrives. A minute later the human comes
        // back and deliberately holds the off-switch again.
        let t1 = t0 + Duration::from_secs(60);
        s.observe_event(&crate::input::tests::chord_press(), t1);

        // Everything a held chord owes the human is present again.
        assert_eq!(
            s.deadline(),
            Some(t1 + DEFAULT_HOLD),
            "no timer would be armed: the switch is inert"
        );
        assert!(
            s.hold_progress(t1 + DEFAULT_HOLD.mul_f64(0.9)).is_some(),
            "the human gets no indicator that the off-switch is working"
        );
        s.fire_if_due(t1 + DEFAULT_HOLD);
        let trigger = s
            .take_trigger()
            .expect("a lost release permanently disabled the human's off-switch");
        // Measured from THIS press, not the stale one: a re-arm that kept the
        // old instant would report a 61-second hold in the journal.
        assert_eq!(trigger.held, DEFAULT_HOLD);
    }

    #[test]
    fn forgetting_a_hold_cancels_it_rather_than_completing_it() {
        // The mirror image, and the reason `forget_hold` cancels instead of
        // firing: a human who taps Esc and immediately clicks another host
        // window must not have every grant in the session revoked one second
        // later, with the journal recording it as a completed human gesture
        // that never happened. See `forget_hold`'s doc comment for the full
        // asymmetry argument.
        let t0 = Instant::now();
        let mut s = switch();
        s.observe_event(&crate::input::tests::chord_press(), t0);
        assert!(s.deadline().is_some(), "fixture check: the hold armed");

        // Focus leaves 50 ms in; the release will go to the other window.
        s.forget_hold();

        assert_eq!(s.deadline(), None, "a forgotten hold still has a deadline");
        assert_eq!(s.hold_progress(t0 + DEFAULT_HOLD.mul_f64(0.9)), None);
        // Well past the deadline, from every driver of the elapse check.
        s.fire_if_due(t0 + DEFAULT_HOLD);
        s.fire_if_due(t0 + Duration::from_secs(30));
        assert!(
            s.take_trigger().is_none(),
            "an alt-tab mid-tap revoked the whole session"
        );

        // And the switch is immediately usable again -- cancelling costs the
        // human one repeated gesture and nothing more.
        let t1 = t0 + Duration::from_secs(30);
        s.observe_event(&crate::input::tests::chord_press(), t1);
        s.fire_if_due(t1 + DEFAULT_HOLD);
        assert!(
            s.take_trigger().is_some(),
            "the switch did not recover after a forgotten hold"
        );
    }

    #[test]
    fn forgetting_a_hold_drops_the_withheld_press_instead_of_leaking_it() {
        // `forget_hold` clears suppression state too. The withheld press
        // belongs to a gesture the core can no longer classify, and the app
        // has just lost keyboard focus -- replaying it later would be a stray
        // Escape arriving out of nowhere.
        let t0 = Instant::now();
        let mut s = switch();
        let press = crate::input::tests::chord_press();
        s.observe_event(&press, t0);
        assert_eq!(s.gate_event(&press), Gate::Consume);

        s.forget_hold();
        assert!(s.take_replay().is_empty());

        // The release arrives late (focus came back, or the app re-focused):
        // there is no press to pair it with, so nothing is replayed and
        // nothing is stranded.
        let release = crate::input::tests::chord_release();
        s.observe_event(&release, t0 + Duration::from_millis(200));
        assert_eq!(s.gate_event(&release), Gate::Consume);
        assert!(
            s.take_replay().is_empty(),
            "a forgotten press was replayed into the app after focus loss"
        );
    }

    // ------------------------------------------------------------------
    // Suppression: tap-through, hold-swallow
    // ------------------------------------------------------------------

    #[test]
    fn a_tap_is_replayed_to_the_app_and_a_chord_is_not() {
        let t0 = Instant::now();

        // A tap: the press is withheld, the release consumed, and the pair
        // handed back for re-routing in press-then-release order.
        let mut s = switch();
        let press = crate::input::tests::chord_press();
        let release = crate::input::tests::chord_release();
        s.observe_event(&press, t0);
        assert_eq!(s.gate_event(&press), Gate::Consume);
        assert!(s.take_replay().is_empty(), "nothing is owed mid-hold");
        s.observe_event(&release, t0 + Duration::from_millis(80));
        assert_eq!(s.gate_event(&release), Gate::Consume);
        let replay = s.take_replay();
        assert_eq!(replay.len(), 2);
        assert_eq!(&replay[0], &press);
        assert_eq!(&replay[1], &release);
        // The replayed pair passes the gate untouched, on the credit.
        assert_eq!(s.gate_event(&replay[0]), Gate::Deliver);
        assert_eq!(s.gate_event(&replay[1]), Gate::Deliver);

        // A chord: the press is withheld and then dropped at firing, the
        // release consumed, and nothing is ever owed to the app.
        let mut s = switch();
        s.observe_event(&press, t0);
        assert_eq!(s.gate_event(&press), Gate::Consume);
        s.fire_if_due(t0 + DEFAULT_HOLD);
        assert!(s.take_trigger().is_some());
        s.observe_event(&release, t0 + DEFAULT_HOLD + Duration::from_millis(400));
        assert_eq!(s.gate_event(&release), Gate::Consume);
        assert!(
            s.take_replay().is_empty(),
            "the app must never see the held chord"
        );
    }

    #[test]
    fn a_replayed_tap_does_not_replay_itself() {
        // The loop this could have been: the replayed press re-arms the hold
        // in `observe`, and its release would queue another replay. It does
        // not, because only a press this gate actually withheld can be owed
        // back -- and a replayed press is delivered, not withheld.
        let t0 = Instant::now();
        let mut s = switch();
        let press = crate::input::tests::chord_press();
        let release = crate::input::tests::chord_release();

        s.observe_event(&press, t0);
        s.gate_event(&press);
        s.observe_event(&release, t0 + Duration::from_millis(80));
        s.gate_event(&release);
        let replay = s.take_replay();
        assert_eq!(replay.len(), 2);

        // Re-route the pair exactly as the embedder does.
        for event in &replay {
            s.observe_event(event, t0 + Duration::from_millis(80));
            s.gate_event(event);
        }
        assert!(
            s.take_replay().is_empty(),
            "the replay replayed itself: an unbounded loop"
        );
    }

    #[test]
    fn a_dropped_replay_never_stops_the_switch_from_firing() {
        // The named failure mode (module docs): an embedder that drains the
        // queue and fails to route it leaves a stale credit. The cost must be
        // an app-visible Esc, never a revocation that did not happen.
        let t0 = Instant::now();
        let mut s = switch();
        let press = crate::input::tests::chord_press();
        let release = crate::input::tests::chord_release();

        s.observe_event(&press, t0);
        s.gate_event(&press);
        s.observe_event(&release, t0 + Duration::from_millis(50));
        s.gate_event(&release);
        let dropped = s.take_replay(); // drained ...
        assert_eq!(dropped.len(), 2);
        drop(dropped); // ... and never routed: the credit is now stale.

        // The next genuine hold still arms and still fires.
        let t1 = t0 + Duration::from_secs(1);
        s.observe_event(&press, t1);
        assert_eq!(s.deadline(), Some(t1 + DEFAULT_HOLD));
        // The stale credit only leaks the press to the app.
        assert_eq!(s.gate_event(&press), Gate::Deliver);
        s.fire_if_due(t1 + DEFAULT_HOLD);
        assert!(
            s.take_trigger().is_some(),
            "a suppression-side bug stopped the revocation"
        );

        // And it self-heals: the credit is spent, so the next hold is
        // withheld normally again.
        let t2 = t1 + Duration::from_secs(5);
        s.observe_event(&release, t2);
        s.gate_event(&release); // spends the last credit
        s.observe_event(&press, t2);
        assert_eq!(s.gate_event(&press), Gate::Consume);
    }

    #[test]
    fn an_undrained_replay_queue_is_bounded() {
        let t0 = Instant::now();
        let mut s = switch();
        let press = crate::input::tests::chord_press();
        let release = crate::input::tests::chord_release();
        for step in 0..20 {
            let at = t0 + Duration::from_millis(step * 200);
            s.observe_event(&press, at);
            s.gate_event(&press);
            s.observe_event(&release, at + Duration::from_millis(50));
            s.gate_event(&release);
        }
        assert!(s.replay.len() <= MAX_REPLAY_BACKLOG + 2);
    }

    #[test]
    fn nothing_but_the_chord_key_is_ever_suppressed() {
        let mut s = switch();
        for event in [
            crate::input::tests::other_key_press(),
            crate::input::tests::other_key_release(),
            crate::input::tests::emulated_motion(),
            crate::input::tests::emulated_chord_press(),
            crate::input::tests::emulated_chord_release(),
        ] {
            assert_eq!(
                s.gate_event(&event),
                Gate::Deliver,
                "{event:?} must not be suppressed by the dead-man hook"
            );
        }
    }

    // ------------------------------------------------------------------
    // Feedback while holding
    // ------------------------------------------------------------------

    #[test]
    fn hold_progress_appears_partway_through_and_stops_at_the_trigger() {
        let t0 = Instant::now();
        let mut s = switch();
        assert_eq!(s.hold_progress(t0), None, "nothing to show when idle");

        s.observe_event(&crate::input::tests::chord_press(), t0);
        // An ordinary tap must draw nothing at all: Esc is pressed constantly
        // in a browser, and a bar that flashed every time is noise.
        assert_eq!(s.hold_progress(t0), None);
        assert_eq!(s.hold_progress(t0 + Duration::from_millis(100)), None);
        // Past the reveal point it appears and grows.
        let early = s
            .hold_progress(t0 + DEFAULT_HOLD.mul_f64(0.5))
            .expect("revealed");
        let late = s
            .hold_progress(t0 + DEFAULT_HOLD.mul_f64(0.9))
            .expect("revealed");
        assert!((0.45..=1.0).contains(&early));
        assert!(late > early);
        // Clamped, never past full.
        assert_eq!(s.hold_progress(t0 + DEFAULT_HOLD * 3), Some(1.0));

        // Once it fires there is no hold left to indicate.
        s.fire_if_due(t0 + DEFAULT_HOLD);
        assert_eq!(s.hold_progress(t0 + DEFAULT_HOLD), None);
    }

    #[test]
    fn the_indicator_paints_a_bar_that_grows_with_progress() {
        const W: u32 = 64;
        const H: u32 = 32;
        let blank = vec![0u8; (W * H * 4) as usize];

        let filled_px = |progress: f64| {
            let mut px = blank.clone();
            composite_hold_indicator(&mut px, W, H, progress);
            // Count pixels on the top row that are neither untouched nor the
            // unfilled track.
            (0..W)
                .filter(|x| {
                    let i = (*x as usize) * 4;
                    px[i..i + 4] == [0xff, 0xb0, 0x20, 0xff]
                })
                .count()
        };
        assert_eq!(filled_px(0.0), 0);
        assert_eq!(filled_px(0.5), (W / 2) as usize);
        assert_eq!(filled_px(1.0), W as usize);
        // Out-of-range input is clamped, never panics or overdraws.
        assert_eq!(filled_px(-5.0), 0);
        assert_eq!(filled_px(17.0), W as usize);

        // Rows below the bar are untouched: the indicator never covers the
        // realm view or a consent card's body.
        let mut px = blank.clone();
        composite_hold_indicator(&mut px, W, H, 1.0);
        let below = (8 * W * 4) as usize;
        assert!(px[below..].iter().all(|&b| b == 0));

        // Degenerate geometry is survivable.
        composite_hold_indicator(&mut [], 0, 0, 0.5);
        let mut tiny = vec![0u8; 4];
        composite_hold_indicator(&mut tiny, 1, 1, 1.0);
    }

    // ------------------------------------------------------------------
    // Applying a trigger: revocation, denial, and the journal
    // ------------------------------------------------------------------

    /// One pending petition in a fresh interactive registry.
    fn pending_petition(
        registry: &mut PetitionRegistry,
        who: &str,
    ) -> crate::petitions::PetitionId {
        let connection = registry.register_connection();
        let realms = crate::realm::tests::registry_with(&["realm-0"]);
        let Admission::Pending { petition } = registry.admit(
            PetitionRequest {
                connection,
                identity: identity(who),
                realm_name: "realm-0".into(),
                grant_wire_id: 10,
                consent_wire_id: 11,
                resource: String::new(),
                verbs: Verb::OBSERVE | Verb::ACTUATE_POINTER,
                expiry_ms: 60_000,
                max_event_rate: 0,
                persistence: WirePersistence::WhileRunning,
                flags: 0,
            },
            Instant::now(),
            &realms,
            false,
        ) else {
            panic!("an interactive petition must pend");
        };
        petition
    }

    #[test]
    fn the_trigger_revokes_every_principal_not_just_one() {
        // The adversarial reading (module docs): an off-switch that killed a
        // single identity is defeated by an attacker who holds two.
        let now = Instant::now();
        let mut table = GrantTable::new();
        let demo = identity(PROMPT_IDENTITY);
        let other = identity(OTHER_IDENTITY);
        let a = grant_for(&mut table, &demo, now);
        let b = grant_for(&mut table, &other, now);
        let c = grant_for(&mut table, &demo, now);

        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();
        let effect = apply(
            &Trigger {
                chord: "esc",
                held: DEFAULT_HOLD,
            },
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now,
        );

        assert_eq!(effect.revoked, vec![a, b, c]);
        // Each row is checked against its *own* principal: the chokepoint
        // query cross-checks ownership, so asking with the wrong identity
        // would answer `not_granted` and prove nothing about revocation.
        for (id, owner) in [(a, &demo), (b, &other), (c, &demo)] {
            assert_eq!(
                table.check_use_grant(id, owner, Verb::OBSERVE, now).err(),
                Some(RefusalReason::Revoked),
                "grant {id} survived the dead-man switch"
            );
        }
    }

    #[test]
    fn both_verbs_die_together_on_the_very_next_check() {
        // The acceptance criteria's two halves, and the reason they cannot
        // come apart: capture and actuation ask the same table through the
        // same chokepoint query, so there is no ordering in which one
        // survives the other.
        let now = Instant::now();
        let mut table = GrantTable::new();
        let who = identity(PROMPT_IDENTITY);
        let row = grant_for(&mut table, &who, now);

        // Before: both verbs are live.
        for verb in [Verb::OBSERVE, Verb::ACTUATE_POINTER, Verb::ACTUATE_TEXT] {
            assert!(table.check_use_grant(row, &who, verb, now).is_ok());
        }

        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();
        apply(
            &Trigger {
                chord: "esc",
                held: DEFAULT_HOLD,
            },
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now,
        );

        // After, at the *same instant*: every verb refuses `revoked`. No
        // clock has to advance for the switch to bite -- which is what makes
        // "effective on the very next enforcement check" literal.
        for verb in [Verb::OBSERVE, Verb::ACTUATE_POINTER, Verb::ACTUATE_TEXT] {
            assert_eq!(
                table.check_use_grant(row, &who, verb, now).err(),
                Some(RefusalReason::Revoked),
                "{verb:?} outlived the dead-man switch"
            );
        }
        assert_eq!(Refusal::from(RefusalReason::Revoked), Refusal::Revoked);
    }

    #[test]
    fn the_trigger_denies_every_pending_petition() {
        // A human who just hit the panic button is not, in that same second,
        // granting new authority -- and the prompt's anti-tapjacking guard
        // has long since elapsed, so leaving it up would put a live Allow
        // button under the next click.
        let now = Instant::now();
        let mut table = GrantTable::new();
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let first = pending_petition(&mut registry, PROMPT_IDENTITY);
        let second = pending_petition(&mut registry, OTHER_IDENTITY);
        assert_eq!(registry.pending_ids(), vec![first, second]);

        let mut scratch = Scratch::new();
        let effect = apply(
            &Trigger {
                chord: "esc",
                held: DEFAULT_HOLD,
            },
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now,
        );

        assert_eq!(effect.denied.len(), 2);
        for resolution in &effect.denied {
            assert!(
                matches!(
                    resolution.verdict,
                    crate::petitions::Verdict::Declined {
                        outcome: vitrin_protocol::generated::vitrin_grant::Outcome::Denied
                    }
                ),
                "a denied petition must get a clean protocol terminal, not a hang"
            );
        }
        // The table is empty, which is also what lets the embedder's existing
        // `retire_stale` take the card down with no new coupling.
        assert!(registry.pending_ids().is_empty());
        assert_eq!(registry.front_pending(), None);
    }

    #[test]
    fn the_journal_names_the_revocation_and_its_human_cause() {
        let now = Instant::now();
        let mut table = GrantTable::new();
        let who = identity(PROMPT_IDENTITY);
        let row = grant_for(&mut table, &who, now);
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        pending_petition(&mut registry, PROMPT_IDENTITY);

        let mut scratch = Scratch::new();
        apply(
            &Trigger {
                chord: "esc",
                held: Duration::from_millis(1042),
            },
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now,
        );

        let entries = scratch.entries();
        let triggers = crate::recorder::tests::of_kind(&entries, "dead_man_triggered");
        assert_eq!(triggers.len(), 1, "one gesture, one entry");
        let trigger = triggers[0];
        assert_eq!(trigger.str("chord"), "esc");
        assert_eq!(trigger.u64("held_ms"), 1042);
        assert_eq!(trigger.u64("revoked_grants"), 1);
        assert_eq!(trigger.u64("denied_petitions"), 1);

        // And the revocation line itself names its cause, so a reader never
        // has to infer it from adjacency to another entry.
        let revocations = crate::recorder::tests::of_kind(&entries, "grant_revoked");
        assert_eq!(revocations.len(), 1);
        assert_eq!(revocations[0].str("grant_id"), row.to_string());
        assert_eq!(revocations[0].str("scope"), "principal");
        assert_eq!(revocations[0].str("cause"), "dead_man_chord");
        assert_eq!(revocations[0].str("transition"), "active_to_revoked");

        // Cause before effect: a replay reads the trigger, then the rows it
        // explains.
        assert!(trigger.u64("seq") < revocations[0].u64("seq"));
    }

    #[test]
    fn a_trigger_that_revokes_nothing_is_still_recorded() {
        // Without this entry, a session where the human hit the off-switch
        // and it had nothing to revoke is indistinguishable from one where
        // they never hit it -- and "the switch works" becomes unverifiable
        // from the log alone.
        let now = Instant::now();
        let mut table = GrantTable::new();
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();

        let effect = apply(
            &Trigger {
                chord: "f12",
                held: DEFAULT_HOLD,
            },
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now,
        );
        assert!(effect.revoked.is_empty() && effect.denied.is_empty());

        let entries = scratch.entries();
        let triggers = crate::recorder::tests::of_kind(&entries, "dead_man_triggered");
        assert_eq!(triggers.len(), 1, "an empty trigger is still a gesture");
        assert_eq!(triggers[0].str("chord"), "f12");
        assert_eq!(triggers[0].u64("revoked_grants"), 0);
        assert_eq!(triggers[0].u64("denied_petitions"), 0);
        assert!(
            crate::recorder::tests::of_kind(&entries, "grant_revoked").is_empty(),
            "nothing was revoked, so nothing may claim to have been"
        );
    }

    #[test]
    fn applying_twice_revokes_nothing_new() {
        // Revocation is idempotent, so a re-trigger (or a double-drain) is
        // harmless -- and reports honestly that it changed nothing.
        let now = Instant::now();
        let mut table = GrantTable::new();
        let who = identity(PROMPT_IDENTITY);
        grant_for(&mut table, &who, now);
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();
        let trigger = Trigger {
            chord: "esc",
            held: DEFAULT_HOLD,
        };

        assert_eq!(
            apply(
                &trigger,
                &mut table,
                &mut registry,
                &mut scratch.recorder,
                now
            )
            .revoked
            .len(),
            1
        );
        assert!(apply(
            &trigger,
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now
        )
        .revoked
        .is_empty());
    }

    #[test]
    fn the_next_use_through_the_real_chokepoint_is_refused_revoked() {
        // The latency criterion, bounded rather than timed. "Revocation
        // latency <= one protocol round-trip" is not a stopwatch claim here:
        // the switch mutates the very state the chokepoint reads, in the
        // same single-threaded turn, so the number of protocol operations
        // between the trigger and the refusal is **zero**. This test is what
        // makes that literal -- the chokepoint call immediately after
        // `apply` is the first one, it uses the *same* `now` (no clock
        // advances, so nothing can be attributed to expiry), and both verbs
        // are exercised through the real `enforce_use` rather than through
        // the table query underneath it.
        use crate::capture::RealmViewFrame;
        use crate::enforcement::{Chokepoint, UseEnv, UseKind, UseOutcome, UseRequest};
        use crate::input::PhysicalPresence;

        const W: u32 = 32;
        const H: u32 = 24;
        let _fd = crate::capture::tests::fd_lock();

        let now = Instant::now();
        let who = identity(PROMPT_IDENTITY);
        let mut grants = GrantTable::new();
        let row = grant_for(&mut grants, &who, now);
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();

        let rgba = vec![0u8; (W * H * 4) as usize];
        let frame = RealmViewFrame {
            rgba: &rgba,
            width: W,
            height: H,
        };
        let presence = PhysicalPresence::new();
        let mut chokepoint = Chokepoint::new();

        let attempt = |chokepoint: &mut Chokepoint,
                       grants: &mut GrantTable,
                       registry: &PetitionRegistry,
                       kind: UseKind| {
            chokepoint
                .enforce_use(
                    UseRequest {
                        facet_id: 20,
                        grant_wire_id: 10,
                        grant_row: Some(row),
                        principal: &who,
                        kind,
                    },
                    grants,
                    registry,
                    UseEnv {
                        realm_view: Some(&frame),
                        presence: &presence,
                        // The seat serves this grant's realm: the
                        // cross-realm guard is not this test's subject and
                        // must not silently supply its refusals.
                        seat_reaches_grant_realm: true,
                        actuations: &mut |_input| {},
                        // No layout use here; the sink must still exist,
                        // and a layout act reaching it would be the bug.
                        grant_realm: None,
                        layout: &mut |act| panic!("no layout act expected: {act:?}"),
                    },
                    now,
                    &mut |_frame, _fd| Ok(()),
                )
                .expect("transport is healthy")
        };

        // Before the chord: the agent is working normally. Both verbs are
        // admitted, so the refusals below cannot be an artefact of a grant
        // that was never usable.
        assert!(matches!(
            attempt(&mut chokepoint, &mut grants, &registry, UseKind::Capture),
            UseOutcome::Admitted { .. }
        ));
        assert!(matches!(
            attempt(
                &mut chokepoint,
                &mut grants,
                &registry,
                UseKind::Pointer(crate::input::SeatInputKind::Motion { x: 3.0, y: 4.0 }),
            ),
            UseOutcome::Admitted { .. }
        ));

        // The human holds the chord.
        apply(
            &Trigger {
                chord: "esc",
                held: DEFAULT_HOLD,
            },
            &mut grants,
            &mut registry,
            &mut scratch.recorder,
            now,
        );

        // The very next actuation, at the same instant, is refused
        // `revoked` -- and so is the very next capture. Both verbs die
        // together because both asked the same table through the same
        // chokepoint; there is no ordering in which one outlives the other.
        for kind in [
            UseKind::Pointer(crate::input::SeatInputKind::Motion { x: 5.0, y: 6.0 }),
            UseKind::Capture,
            UseKind::Text(crate::input::SeatInputKind::Text {
                text: "still there?".into(),
            }),
        ] {
            let verb = kind.verb();
            let outcome = attempt(&mut chokepoint, &mut grants, &registry, kind);
            assert!(
                matches!(
                    outcome,
                    UseOutcome::Refused {
                        code: Refusal::Revoked,
                        ..
                    }
                ),
                "{verb:?} was not refused `revoked` on the first use after the chord: {outcome:?}"
            );
        }
    }

    #[test]
    fn a_grant_minted_after_the_chord_is_born_revoked() {
        // The window a point-in-time sweep cannot close. A consent decision
        // becomes a grant-table row only at `deliver_resolution`, so a
        // `Verdict::Granted` decided BEFORE the chord and delivered AFTER it
        // is neither a pending petition nor an existing row when `apply`
        // runs -- both halves of the sweep miss it, and delivery then mints
        // live authority that outlived the human's off-switch.
        //
        // The same hole hands `ConsentPolicy::AutoApprove` the agent's whole
        // authority back on its next petition, one round-trip after the panic
        // button, with no human involved at all.
        //
        // `apply` seals the table, so a row minted afterwards is born revoked
        // and refused through the ordinary path with the ordinary code.
        // `RefusalReason` is already imported by this test module.

        let now = Instant::now();
        let who = identity(PROMPT_IDENTITY);
        let mut grants = GrantTable::new();
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();

        // The human hits the off-switch on an empty table -- nothing to
        // sweep, so ONLY the seal can be doing the work below.
        apply(
            &Trigger {
                chord: "esc",
                held: DEFAULT_HOLD,
            },
            &mut grants,
            &mut registry,
            &mut scratch.recorder,
            now,
        );

        // Now the in-flight resolution is delivered, minting its row.
        let late = grant_for(&mut grants, &who, now);
        assert_eq!(
            grants.get(late, now).map(|(_row, state)| state),
            Some(crate::grants::GrantState::Revoked),
            "a grant minted after the chord went live"
        );
        // And it is refused through the same query the chokepoint uses, with
        // the same code a swept row produces -- the wire cannot tell them
        // apart, and should not.
        assert_eq!(
            grants.check_use_grant(late, &who, Verb::ACTUATE_POINTER, now),
            Err(RefusalReason::Revoked),
            "authority decided before the chord survived it"
        );
        assert_eq!(
            grants.check_use_grant(late, &who, Verb::OBSERVE, now),
            Err(RefusalReason::Revoked),
            "capture survived the chord"
        );
    }

    #[test]
    fn the_seal_outlives_the_sweep_and_is_idempotent() {
        // The property that makes this a revocation of the session's
        // authority rather than of one instant: the seal is not a one-shot
        // that the next insert clears. Every row minted from here on is dead,
        // however many arrive and however much later.
        let now = Instant::now();
        let who = identity(PROMPT_IDENTITY);
        let other = identity(OTHER_IDENTITY);
        let mut grants = GrantTable::new();
        let live = grant_for(&mut grants, &who, now);
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::AutoApprove, PetitionConfig::default());
        let mut scratch = Scratch::new();

        let trigger = Trigger {
            chord: "esc",
            held: DEFAULT_HOLD,
        };
        let effect = apply(
            &trigger,
            &mut grants,
            &mut registry,
            &mut scratch.recorder,
            now,
        );
        assert_eq!(effect.revoked, vec![live], "the sweep missed the live row");

        // Three petitions later, for two different principals, an hour on.
        let much_later = now + Duration::from_secs(3600);
        for who in [&who, &other, &who] {
            let row = grant_for(&mut grants, who, much_later);
            assert_eq!(
                grants.get(row, much_later).map(|(_r, state)| state),
                Some(crate::grants::GrantState::Revoked),
                "the seal wore off"
            );
        }

        // A second chord is harmless: nothing new to revoke, and the seal
        // stays sealed.
        let effect = apply(
            &trigger,
            &mut grants,
            &mut registry,
            &mut scratch.recorder,
            much_later,
        );
        assert!(
            effect.revoked.is_empty(),
            "born-revoked rows were revoked a second time: {:?}",
            effect.revoked
        );
        let after = grant_for(&mut grants, &who, much_later);
        assert_eq!(
            grants.get(after, much_later).map(|(_r, state)| state),
            Some(crate::grants::GrantState::Revoked)
        );
    }

    #[test]
    fn a_real_calloop_timer_completes_the_chord_with_no_further_input() {
        // Design tension 2 through the machinery the backend actually uses.
        // Smithay's winit backend filters key repeats, so a held key
        // produces one press and then silence -- if the timing path were
        // wrong, this loop would sit idle forever rather than firing. The
        // This pins the SWITCH's half of the contract -- that `fire_if_due`
        // driven only by a timer completes a hold -- independently of any
        // embedder. The nested backend's own wiring (the source insert, the
        // early-wakeup reschedule, the `deadman_timer_armed` bookkeeping) is
        // a separate matter and is driven directly, through the code
        // production runs, by `backend::winit`'s
        // `the_backends_timer_completes_a_held_chord_with_no_further_input`
        // and `an_early_timer_wakeup_reschedules_instead_of_dropping`. This
        // test deliberately does NOT stand in for those: a hand-copy of the
        // backend's pattern would keep passing while the backend regressed,
        // which is exactly how the wiring came to be untested.
        use calloop::timer::{TimeoutAction, Timer};
        use calloop::EventLoop;

        // `EventLoop::try_new` opens epoll/eventfd descriptors, and
        // `capture`'s `fd_count_returns_to_baseline` asserts an exact
        // process-wide fd count. Take the same quiesce lock so the two never
        // run concurrently -- otherwise this test intermittently fails that
        // one, from a different module, for no reason a reader could find.
        let _fd = crate::capture::tests::fd_lock();

        // A short hold so the test is quick; the mechanism is duration-blind.
        let config = DeadManConfig::default()
            .with_hold_ms(250)
            .expect("within range");
        let switch = Rc::new(RefCell::new(DeadManSwitch::new(config)));

        let started = Instant::now();
        switch
            .borrow_mut()
            .observe_event(&crate::input::tests::chord_press(), started);
        let deadline = switch.borrow().deadline().expect("armed by the press");

        let mut event_loop: EventLoop<Option<Trigger>> = EventLoop::try_new().expect("event loop");
        let timer_switch = Rc::clone(&switch);
        event_loop
            .handle()
            .insert_source(Timer::from_deadline(deadline), move |_, _, fired| {
                let mut s = timer_switch.borrow_mut();
                s.fire_if_due(Instant::now());
                if let Some(trigger) = s.take_trigger() {
                    *fired = Some(trigger);
                }
                match s.deadline() {
                    Some(next) => TimeoutAction::ToInstant(next),
                    None => TimeoutAction::Drop,
                }
            })
            .expect("timer source");

        // Nothing else is inserted: no input source, no frame chain. The
        // only thing that can wake this loop is the dead-man timer.
        let mut fired: Option<Trigger> = None;
        let budget = Duration::from_secs(5);
        while fired.is_none() && started.elapsed() < budget {
            event_loop
                .dispatch(Some(Duration::from_millis(50)), &mut fired)
                .expect("dispatch");
        }

        let trigger = fired.expect("the chord must complete from the clock alone");
        assert_eq!(trigger.chord, "esc");
        assert!(
            trigger.held >= Duration::from_millis(250),
            "fired early: {:?}",
            trigger.held
        );
        // Latency, as actually observed here: the wakeup is a timer at the
        // deadline, so the trigger lands within a scheduling quantum of it,
        // not within "whenever the human next types". The generous bound
        // keeps this from being a CI flake detector while still failing
        // loudly if the timing path regressed to event-driven.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the chord took {:?} to complete; the timing path is not time-driven",
            started.elapsed()
        );
    }

    #[test]
    fn a_dead_realm_does_not_stop_the_switch() {
        // "Think adversarially": revocation is a table operation and touches
        // no realm, no shim, and no surface, so a wedged or dead realm cannot
        // hold authority open. Modelled by revoking with a table whose rows
        // name a realm nothing is serving.
        let now = Instant::now();
        let mut table = GrantTable::new();
        let who = identity(PROMPT_IDENTITY);
        let row = grant_for(&mut table, &who, now);
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let mut scratch = Scratch::new();
        apply(
            &Trigger {
                chord: "esc",
                held: DEFAULT_HOLD,
            },
            &mut table,
            &mut registry,
            &mut scratch.recorder,
            now,
        );
        assert_eq!(
            table.check_use_grant(row, &who, Verb::OBSERVE, now).err(),
            Some(RefusalReason::Revoked)
        );
    }
}
