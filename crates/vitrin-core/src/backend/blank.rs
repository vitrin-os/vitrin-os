// SPDX-License-Identifier: MPL-2.0
//! The idle blank (WS-E.4.3, issue #223): the session's activity clock, the
//! state machine that turns idleness into a dark panel, and the opaque cover
//! that goes on the human-visible output while it happens.
//!
//! # The blank does not lock, and that is an owner decision
//!
//! Taha, 2026-08-10: **on idle the screen goes dark and the session stays
//! unlocked.** Locking remains the human's manual chord ([`crate::lock`]),
//! which already exists and already has its own idle timer. There is no
//! idle-lock coupling here, in either direction, and building one would be
//! taking a decision that was taken the other way.
//!
//! The consequence is real and is published rather than softened: **an unlocked
//! session can sit behind a dark screen.** Anyone who walks up and presses a key
//! is inside it. Two sharper forms of the same fact are worth stating with it:
//! a dark screen is no longer evidence that nothing is being *observed* (D-025
//! keeps every grant live across anything the human's own screen does), and —
//! because a disabled CRTC produces no vblank — it is not evidence that anything
//! is being observed *live* either (see [`SessionActivity::is_dark`]).
//!
//! What the two mechanisms **do** share is the activity clock, and exactly that:
//! [`SessionActivity::last_activity`] is read by
//! [`crate::lock::LockScreen::tick`] and by [`SessionActivity::tick`], and
//! written at one site. A blank that also froze the lock's clock would mean
//! `--blank-idle 300 --lock-idle 600` silently never locks, which is the class
//! of unchosen behaviour D-030(2) was written to catch;
//! `a_blank_does_not_disable_the_idle_lock` pins that it does not.
//!
//! # One type, not two, and why
//!
//! The design this implements described a `SessionActivity { last, blank:
//! Screen }` pair. It is one type here because every site that reads the phase
//! also reads or writes the clock — the wake verdict, the idle tick, the seat
//! pause — so two `RefCell`s would be two borrows taken together at every call
//! and one more thing that can be held out of step. `crate::input`'s presence
//! record is the precedent for the sharing shape (`Rc<RefCell<_>>`, minted
//! once, handed to everyone who needs it); the argument for *one* cell is
//! [`crate::lock::gate`]'s own — "a second clock would be a second thing to
//! keep in step".
//!
//! # The clock has exactly one writer
//!
//! [`SessionActivity::note_physical`] is called from exactly one place:
//! `LockScreen::judge`, below its `Origin::Physical` check. So:
//!
//! - **A human's press, release, motion, scroll, delta, gesture or text
//!   postpones the blank and wakes it.** Everything physical, both halves of
//!   every pair.
//! - **An agent's actuation does neither.** The origin check is above the stamp
//!   and was already there, with `an_agents_actuation_never_holds_the_idle_lock_open`
//!   pinning it for the lock; the blank inherits the property structurally
//!   rather than restating it. The independent argument for the *wake* half is
//!   that there is no verb in the IDL for "power the human's display": an agent
//!   that could wake a panel would be making an unrequested change to the
//!   human's physical environment, remotely triggerable, under no grant.
//! - **The core's own drawing does neither.** A consent card is refused a raise
//!   entirely while dark ([`crate::session::PromptVisibility::ScreenIsDark`]),
//!   the status strip's minute rollover marks the round dirty and queues
//!   nothing, and a realm's commit is an app painting rather than a human
//!   arriving. A core that woke the screen because an agent petitioned would
//!   hand every principal a remote "wake the human's display" primitive, and
//!   petitioning needs no grant at all.
//!
//! # The wake event is consumed, and its POSITION is the trap
//!
//! A press the human aimed at a dark screen must neither commit a consent card
//! nor act inside a confined app, so the first physical event after the screen
//! went dark is consumed. **It cannot be consumed by a gate stacked outside
//! [`crate::lock::LockGate`]**, and that is not hypothetical: the lock tracks
//! its chord's modifiers in `judge`, sound only because nothing above it eats an
//! event. A wake gate above it would eat whatever the human happened to press
//! first — very often a bare modifier — and `ctrl+alt+delete` would stop raising
//! the lock after every blank. So the verdict is taken *inside*
//! `LockScreen::judge`, after `ChordMatcher::observe` (or the modifier bits
//! desync) and before `ChordMatcher::gate` (or the wake press fires a chord, or
//! is typed into a passphrase attempt as an invisible stray character).
//!
//! What survives a consumed wake, and is checked: the dead-man switch, because
//! it detects in `PreemptionHook::observe`, which [`crate::input::GateOnlyHook`]
//! forwards unconditionally and no gate can suppress. So does the VT escape,
//! twice over — `VtHook` is stacked *outside* the lock gate, so its `gate` runs
//! first, and it tracks modifiers in `observe` besides.
//!
//! What does not survive, and is accepted: a bare-trigger chord whose trigger
//! *is* the wake event — in practice only the attention chord's bare Super tap,
//! which is self-correcting (tap again).
//!
//! # Fail open on input, fail closed on authority
//!
//! The consume lasts from the moment the cover goes up until the first flip
//! after the wake lands — not one event — because a modeset on an internal panel
//! is long enough for a human to type several characters into an app they cannot
//! see. It is **bounded** by [`WAKE_DEADLINE`], after which input is delivered
//! regardless: a dark screen that also swallows input is indistinguishable from
//! a wedged session, and manufacturing those is what the recovery runbook exists
//! to avoid. Losing the consume costs nothing security-critical, because the
//! property that stops a clickjack is the consent guard restart
//! ([`crate::session::screen_became_visible`]), not the consume.

use std::time::{Duration, Instant, SystemTime};

use crate::paint::canvas::{Canvas, Rect};

/// The cover's colour: opaque black.
///
/// Not the lock cover's near-black ([`crate::lock::render`]'s `COVER_RGBA`) and
/// not a scrim. A blank is the absence of a picture, and on the failure path —
/// a `clear()` the display controller refuses — this is what the human is left
/// looking at, so it should be as close to "the panel is off" as pixels get.
const COVER_RGBA: [u8; 4] = [0x00, 0x00, 0x00, 0xff];

/// How long physical input may be swallowed by a wake before it is delivered
/// anyway.
///
/// Two seconds is chosen against the cost it bounds rather than against a
/// display's timing: unblanking is a full modeset (connectors are detached by
/// `DrmSurface::clear`), which on an internal panel is tens to low hundreds of
/// milliseconds, so a correct wake never comes near this. What the bound is for
/// is the *incorrect* one — a modeset that fails leaves a session that is dark
/// **and** deaf, which is exactly the wedge a human has no way to distinguish
/// from a crash.
pub(crate) const WAKE_DEADLINE: Duration = Duration::from_secs(2);

/// How much further the wall clock must have advanced than the monotonic clock
/// for [`ResumeWatch`] to call it a suspend.
///
/// `Instant` is `CLOCK_MONOTONIC`, which does not count time the machine spent
/// suspended; `SystemTime` is `CLOCK_REALTIME`, which does. A round in which the
/// two disagree by more than this is a resume that just happened.
///
/// Ten seconds rather than one, because an NTP **step** moves the wall clock
/// with no suspend behind it. The residual is stated rather than tuned away: a
/// step larger than this reads as a resume, and what that costs is one consent
/// guard restart and one repaint — both idempotent, neither able to resolve a
/// petition. Erring in that direction is deliberate.
pub(crate) const SUSPEND_GAP: Duration = Duration::from_secs(10);

/// The startup refusal for `--blank-idle` on a backend that does not own a
/// display controller.
///
/// A `const` in one place, on [`crate::lock::PASSPHRASE_NEEDS_A_KEYMAP`]'s
/// precedent, so the message the operator reads and the message the test asserts
/// are one string.
///
/// **Refused rather than accepted as a no-op**, which is `--agent-cursor`'s and
/// `--size`'s posture, and here it is the stronger kind of refusal: on headless
/// the flag would not merely do nothing, it would wedge the session. Deferred,
/// not refused forever — the evidence that would reopen it is a backend that
/// both owns a display and stacks a lock gate to write the activity clock.
pub(crate) const BLANK_NEEDS_THE_OUTPUT: &str =
    "`--blank-idle` needs a backend that owns the display controller, and only `--drm` does. \
     `--nested` runs as a client of another compositor: painting its own window black would \
     assert something about a screen the host owns, and it cannot power a panel down at all. \
     `--headless` has no output to blank AND no lock gate in its hook stack, so nothing would \
     ever write the activity clock the blank is postponed and woken by -- the session would go \
     dark after the timeout and never come back. Run `--drm`, or omit the flag.";

/// Where the screen is in the blank cycle.
///
/// Four states rather than two, because "the cover is composited" and "the
/// display controller has been told to power the panel down" are different
/// facts that are true at different instants, and the order between them is
/// load-bearing (see [`SessionActivity::dpms_owed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The ordinary state: the human's picture is on the panel.
    Lit,
    /// The idle deadline passed. The cover is composited and a flip carrying it
    /// is owed or in flight; the panel is **still lit**.
    Covering,
    /// The cover's flip landed and the display power state has been set off (or
    /// the attempt failed and the human is looking at the cover instead). No
    /// flip may be queued.
    Dark,
    /// A physical event arrived while covering or dark. The cover comes down and
    /// the next composite re-enables the CRTC through the ordinary commit.
    Waking,
}

/// What [`SessionActivity::note_physical`] decided about one physical event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wake {
    /// The screen was already lit. Deliver the event normally.
    Awake,
    /// The screen was dark, covering, or mid-wake: consume this event. It types
    /// nothing, fires no chord and reaches no app.
    Consumed,
    /// The wake has been running longer than [`WAKE_DEADLINE`]. Deliver the
    /// event anyway — see the module docs' fail-open rule.
    Overdue,
}

impl Wake {
    /// Whether the caller must return `Gate::Consume` for this event.
    pub(crate) fn consumes(self) -> bool {
        matches!(self, Wake::Consumed)
    }
}

/// The session's one answer to "when did the human last touch this session",
/// and the blank state machine that reads it.
///
/// Shared by `Rc<RefCell<..>>` between the lock's gate (which writes the clock)
/// and the backend that owns the output (which ticks the phase, composites the
/// cover and sets the display power state).
#[derive(Debug)]
pub(crate) struct SessionActivity {
    /// When physical input was last seen, or — before the first event — when the
    /// session armed.
    ///
    /// **Seeded at construction rather than left `None`**, inheriting the
    /// argument the lock's own field carried: "no input has ever arrived" and
    /// "no input has arrived recently" are the same situation for an idle timer,
    /// and a `None` meaning "never blank" would leave a session that came up in
    /// front of an empty chair lit forever.
    last: Instant,
    phase: Phase,
    /// `None` = no idle blank at all (`--blank-idle` omitted). Off by default,
    /// on `--lock-idle`'s precedent: a core that guessed a timeout would be a
    /// core that blanked somebody's demo.
    after: Option<Duration>,
    /// Whether the seat has been taken away from this session — on bare metal,
    /// the human switched to another VT (D-030(7)).
    ///
    /// One copy of this fact for the whole session: the lock's idle raise reads
    /// it through [`Self::seat_absent`] rather than keeping its own, because two
    /// booleans meaning "somebody else has the panel" is two things to keep in
    /// step.
    seat_absent: bool,
    /// A flip carrying the cover has been queued.
    ///
    /// The whole reason [`Phase::Covering`] exists. Without it the display power
    /// state would be set off at the first vblank after the deadline — which is
    /// very likely the completion of a flip queued *before* the cover was
    /// raised, so a failed `clear()` would leave the human's last screenful
    /// frozen on a panel nobody is at. Cover first, power second.
    cover_queued: bool,
    /// When the current wake began, for [`WAKE_DEADLINE`].
    waking_since: Option<Instant>,
    /// Latched so a wake that blows its deadline is logged once rather than once
    /// per event.
    overdue_logged: bool,
}

impl SessionActivity {
    /// Arm the clock. `now` seeds it; `after` is `--blank-idle`.
    pub(crate) fn new(after: Option<Duration>, now: Instant) -> Self {
        Self {
            last: now,
            phase: Phase::Lit,
            after,
            seat_absent: false,
            cover_queued: false,
            waking_since: None,
            overdue_logged: false,
        }
    }

    /// When the human last touched this session.
    pub(crate) fn last_activity(&self) -> Instant {
        self.last
    }

    /// Whether the seat is somebody else's right now.
    pub(crate) fn seat_absent(&self) -> bool {
        self.seat_absent
    }

    /// Which state the screen is in.
    ///
    /// Every production decision is taken through the named predicates below
    /// ([`Self::is_covering`], [`Self::is_dark`], [`Self::dpms_owed`]), so this
    /// exists for the tests that assert the *transitions* rather than their
    /// consequences -- which is the whole of what CI can hold about a state
    /// machine whose one driver needs a display controller.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "read only by this module's own transition tests; production reads the \
                      named predicates instead"
        )
    )]
    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether the cover belongs on this frame.
    ///
    /// True while covering **and** while dark: a dark session queues no flip, so
    /// the cover being in a composite nobody presents costs nothing — and if a
    /// composite does happen (a status tick, a realm commit) it must not be the
    /// human's last screenful sitting in the scanout buffer waiting for the next
    /// unblank to reveal it.
    pub(crate) fn is_covering(&self) -> bool {
        matches!(self.phase, Phase::Covering | Phase::Dark)
    }

    /// Whether the display controller has been told to power the panel down.
    ///
    /// Read by `should_queue_flip` — a flip against a disabled CRTC is not a
    /// frame — and by the consent round, because a card composited now reaches
    /// nobody. **Note what it also means**, which is the first-order consequence
    /// this whole change has and #223 does not mention: a disabled CRTC produces
    /// no vblank, `DrmState::on_vblank` is the only bare-metal caller of
    /// `session::emit_presented`, so **no realm's `frame_done` is discharged
    /// while the screen is dark** and every `frame_done`-paced client stops
    /// painting. D-030(6) already published that for a VT switch and called it
    /// "worse than a stall"; a timer makes it routine.
    pub(crate) fn is_dark(&self) -> bool {
        self.phase == Phase::Dark
    }

    /// Tell the clock whether this session still holds the seat (D-030(7)).
    ///
    /// Two effects, and neither is optional. While absent the idle countdown
    /// does not run, because time the human spends on another VT is not idle
    /// time and counting it would blank (and, through the lock's own tick, lock)
    /// the session at an instant nobody could observe. And a pause **forces the
    /// phase back to [`Phase::Lit`]**: a paused session must not hold a blank it
    /// cannot undo — the CRTC is not ours to re-enable, `clear()` answers
    /// `DeviceInactive`, and the activate arm's ordinary repaint is what puts a
    /// picture back.
    ///
    /// Returning advances the clock, so the countdown restarts from the moment
    /// the human comes back rather than resuming one frozen mid-way.
    pub(crate) fn set_seat_absent(&mut self, absent: bool, now: Instant) {
        if absent {
            self.force_lit();
        } else if self.seat_absent {
            self.last = now;
        }
        self.seat_absent = absent;
    }

    /// Record one physical event and decide whether it is swallowed as a wake.
    ///
    /// **The one writer of the clock** (module docs). The caller has already
    /// established that the event's origin is `Origin::Physical`.
    pub(crate) fn note_physical(&mut self, now: Instant) -> Wake {
        self.last = now;
        match self.phase {
            Phase::Lit => Wake::Awake,
            Phase::Covering | Phase::Dark => {
                self.phase = Phase::Waking;
                self.cover_queued = false;
                self.waking_since = Some(now);
                self.overdue_logged = false;
                Wake::Consumed
            }
            Phase::Waking => {
                let overdue = self
                    .waking_since
                    .is_some_and(|since| now.saturating_duration_since(since) >= WAKE_DEADLINE);
                if !overdue {
                    return Wake::Consumed;
                }
                if !self.overdue_logged {
                    self.overdue_logged = true;
                    tracing::warn!(
                        deadline_ms = WAKE_DEADLINE.as_millis(),
                        "the screen has not come back within the wake deadline; delivering \
                         physical input anyway. A dark session that also swallows input is \
                         indistinguishable from a wedged one"
                    );
                }
                Wake::Overdue
            }
        }
    }

    /// Advance the idle countdown for one dispatch round. Returns whether the
    /// phase changed.
    ///
    /// Driven by the embedder once per round rather than by a timer source, for
    /// the reason [`crate::lock::LockScreen::tick`] gives and inherits it from:
    /// the round already samples one instant, the loop is woken at least once a
    /// second by the session's own sweep, and a second clock would be a second
    /// thing to keep in step. Worst-case lateness is one second against a
    /// multi-minute timeout.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        if self.phase != Phase::Lit || self.seat_absent {
            return false;
        }
        let Some(after) = self.after else {
            return false;
        };
        if now.saturating_duration_since(self.last) < after {
            return false;
        }
        self.phase = Phase::Covering;
        self.cover_queued = false;
        true
    }

    /// A flip carrying whatever is composited right now has been queued.
    ///
    /// Called from the one place a flip is actually accepted by the swapchain,
    /// so "the cover is on its way to the panel" cannot be believed of a frame
    /// the hardware refused.
    pub(crate) fn note_frame_queued(&mut self) {
        if self.phase == Phase::Covering {
            self.cover_queued = true;
        }
    }

    /// Whether the display power state should be set off **now**: the cover's
    /// own flip has completed.
    pub(crate) fn dpms_owed(&self) -> bool {
        self.phase == Phase::Covering && self.cover_queued
    }

    /// The panel is off (or the attempt failed and the cover is what the human
    /// has instead).
    pub(crate) fn went_dark(&mut self) {
        if self.phase == Phase::Covering {
            self.phase = Phase::Dark;
        }
    }

    /// A flip completed. Returns whether this is the flip that ended a wake —
    /// i.e. whether the human can see this session again as of now.
    pub(crate) fn note_flip_completed(&mut self) -> bool {
        if self.phase != Phase::Waking {
            return false;
        }
        self.force_lit();
        true
    }

    /// Whether the wake has outrun [`WAKE_DEADLINE`] with no flip behind it.
    ///
    /// Read once per round by the embedder so a wake that never completes is
    /// abandoned rather than left holding the input stream. Separate from
    /// [`Self::note_physical`]'s own deadline check because a human who gives up
    /// pressing keys would otherwise leave the session stuck in [`Phase::Waking`]
    /// forever, and the next press would be swallowed again.
    pub(crate) fn wake_expired(&self, now: Instant) -> bool {
        self.phase == Phase::Waking
            && self
                .waking_since
                .is_some_and(|since| now.saturating_duration_since(since) >= WAKE_DEADLINE)
    }

    /// Force the screen back to [`Phase::Lit`], dropping the cover and any wake
    /// in progress. The one way out of every non-lit state.
    pub(crate) fn force_lit(&mut self) {
        self.phase = Phase::Lit;
        self.cover_queued = false;
        self.waking_since = None;
        self.overdue_logged = false;
    }
}

/// The blank's surface at the backend output stage: whether the cover is
/// composited, and the generation counter presentation caches key on.
///
/// [`crate::lock::LockSurface`]'s shape deliberately, down to the generation
/// discipline — two opaque covers on one output stage that behaved differently
/// under a resize or a cache would be two bugs waiting.
///
/// # Why the constructor is `pub(in crate::backend)`
///
/// A cover that could be raised from anywhere is a cover that can be raised over
/// the trusted band by code that never read [`super`]'s ordering argument. No
/// module outside `crate::backend` can mint one of these, so no module outside
/// `crate::backend` can hold one, so none can draw one — the compiler enforces
/// the half of the chokepoint that it can, and
/// `the_blank_cover_has_exactly_one_chokepoint` holds the half it cannot (that
/// the *one* call inside `crate::backend` is the shared output stage's).
pub(crate) struct BlankSurface {
    covering: bool,
    generation: u64,
}

impl BlankSurface {
    /// An empty surface: no cover, nothing composited.
    pub(in crate::backend) fn new() -> Self {
        Self {
            covering: false,
            generation: 0,
        }
    }

    /// **Test seam.** A surface for the many tests outside `crate::backend`
    /// that call the shared output stage and have no blank of their own —
    /// [`crate::consent::TrustedIndicator::for_test`]'s precedent, and gated the
    /// same way.
    ///
    /// It widens nothing that matters: [`Self::composite_over`] is still
    /// `pub(in crate::backend)`, so a caller outside this module can hold a
    /// cover and still cannot draw one.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::new()
    }

    /// Whether the cover is composited right now.
    ///
    /// Read by `overlay_needs_the_window`, which is what keeps the zero-copy
    /// dmabuf branch off while a blank is falling: that branch presents the
    /// confined client's own texture with no core composite at all, so a
    /// covering session that took it would put the app on screen full-window
    /// with no cover on it — the exact hole WS-E.2.2 had to close for the lock
    /// and #85 for the band.
    pub(crate) fn is_covering(&self) -> bool {
        self.covering
    }

    /// The content generation: changes exactly when composited output may have
    /// changed. Presentation caches key on it.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Raise or lower the cover. Returns whether it changed.
    pub(crate) fn set_covering(&mut self, covering: bool) -> bool {
        if self.covering == covering {
            return false;
        }
        self.covering = covering;
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// Composite the cover over an already-composed human-visible view, in
    /// place.
    ///
    /// **Human-visible output only**, and there is exactly one call site: the
    /// shared output stage in [`super::human_visible_from_view`], after the lock
    /// cover and **before** the trusted band. Before the band deliberately —
    /// nothing may sit on the band, core-drawn or not, and a blank that overdrew
    /// it would be the first exception ever granted. The consequence is a
    /// feature: a blanked frame is black *with this session's secret colour
    /// still lit along the top*, which in the failure case (a display controller
    /// that refuses to power the panel down) is precisely what distinguishes
    /// "vitrind blanked" from "a confined app painted itself black".
    ///
    /// A no-op when no cover is up. A mis-sized view is refused rather than
    /// panicked on, as everywhere else on this path.
    pub(in crate::backend) fn composite_over(&self, view: &mut [u8], width: u32, height: u32) {
        if !self.covering {
            return;
        }
        let Some(mut canvas) = Canvas::new(view, width, height) else {
            return;
        };
        canvas.fill_rect(
            Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            },
            COVER_RGBA,
        );
    }
}

/// Post-hoc suspend detection, from the two clocks this core already samples.
///
/// # Why there is no `PrepareForSleep` here
///
/// #223's key decision is that the core speaks no D-Bus: taking logind's
/// `PrepareForSleep` means a message bus and its dependency tree inside the TCB,
/// while D-020 and #160 exist to *remove* ambient bus access from realms. The
/// obvious fallback — libseat — turns out not to be one either, and that is a
/// finding rather than an assumption: libseat's logind backend registers D-Bus
/// matches for `PauseDevice` and `PropertiesChanged` only and has no
/// `PrepareForSleep` handler at all, and smithay collapses its listener into
/// exactly `SessionEvent::{PauseSession, ActivateSession}`. **A system suspend
/// therefore delivers this core no session event whatsoever**, so #223's "route
/// resume through the same reactivation path as VT switch" has no producer to
/// route from.
///
/// What is left is what the round already reads: `post_dispatch` samples
/// `SystemTime::now()` and `Instant::now()` in one place. `Instant` is
/// `CLOCK_MONOTONIC` and does not advance across a suspend; `SystemTime` does. A
/// round in which the two disagree by more than [`SUSPEND_GAP`] is a resume that
/// just ended — no bus client, no new syscall, no third clock.
///
/// # What it cannot do, stated plainly
///
/// It detects a resume strictly **after** the fact. A frame may still be
/// submitted into a suspending device, and the first post-resume frame may be
/// late. #223's own key-decisions block says so; this is that residual, made
/// concrete.
#[derive(Debug, Default)]
pub(crate) struct ResumeWatch {
    last: Option<(SystemTime, Instant)>,
}

impl ResumeWatch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fold one round's clock pair in. `Some(gap)` means a suspend of about
    /// `gap` just ended.
    pub(crate) fn sample(&mut self, wall: SystemTime, mono: Instant) -> Option<Duration> {
        let previous = self.last.replace((wall, mono))?;
        let (previous_wall, previous_mono) = previous;
        // A wall clock that went *backwards* is an NTP step, never a suspend.
        let wall_delta = wall.duration_since(previous_wall).ok()?;
        let mono_delta = mono.saturating_duration_since(previous_mono);
        let skew = wall_delta.checked_sub(mono_delta)?;
        (skew >= SUSPEND_GAP).then_some(skew)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::BYTES_PER_PIXEL;

    fn flat(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
        for px in v.chunks_exact_mut(BYTES_PER_PIXEL) {
            px[..3].copy_from_slice(&rgb);
            px[3] = 0xff;
        }
        v
    }

    fn armed(secs: u64, now: Instant) -> SessionActivity {
        SessionActivity::new(Some(Duration::from_secs(secs)), now)
    }

    #[test]
    fn an_idle_session_covers_and_a_busy_one_does_not() {
        let t0 = Instant::now();
        let mut a = armed(60, t0);
        assert_eq!(a.phase(), Phase::Lit);
        assert!(!a.tick(t0 + Duration::from_secs(59)));
        assert!(a.tick(t0 + Duration::from_secs(60)));
        assert_eq!(a.phase(), Phase::Covering);
        assert!(a.is_covering());
        assert!(!a.is_dark(), "the cover is up but the panel is still lit");

        // Physical input pushes the deadline out.
        let mut a = armed(60, t0);
        assert_eq!(
            a.note_physical(t0 + Duration::from_secs(59)),
            Wake::Awake,
            "a lit screen delivers the event"
        );
        assert!(!a.tick(t0 + Duration::from_secs(118)));
        assert!(a.tick(t0 + Duration::from_secs(119)));
    }

    #[test]
    fn no_blank_timeout_means_no_blank_however_long_the_session_sits() {
        let t0 = Instant::now();
        let mut a = SessionActivity::new(None, t0);
        assert!(!a.tick(t0 + Duration::from_secs(86_400)));
        assert_eq!(a.phase(), Phase::Lit);
    }

    /// **Cover, then power — in that order, and never the other way.**
    ///
    /// If the display power state were set off at the first vblank after the
    /// deadline, that vblank would very likely be the completion of a flip
    /// queued *before* the cover went up. A `clear()` that then failed would
    /// leave the human's last screenful frozen on a panel nobody is at, which is
    /// exactly the disclosure the cover exists to prevent.
    #[test]
    fn the_panel_is_powered_down_only_after_the_covers_own_flip_lands() {
        let t0 = Instant::now();
        let mut a = armed(60, t0);
        assert!(a.tick(t0 + Duration::from_secs(60)));
        assert!(
            !a.dpms_owed(),
            "nothing carrying the cover has been queued yet"
        );
        a.note_frame_queued();
        assert!(a.dpms_owed(), "the cover's flip is now on its way");
        a.went_dark();
        assert_eq!(a.phase(), Phase::Dark);
        assert!(a.is_dark() && a.is_covering());
        assert!(!a.dpms_owed(), "and the power state is not set twice");
    }

    #[test]
    fn a_frame_queued_while_lit_never_arms_the_power_down() {
        let t0 = Instant::now();
        let mut a = armed(60, t0);
        a.note_frame_queued();
        assert!(!a.dpms_owed());
        // ...and the flag does not survive into the cover, either.
        assert!(a.tick(t0 + Duration::from_secs(60)));
        assert!(
            !a.dpms_owed(),
            "a flip queued before the cover went up must not count as the cover's"
        );
    }

    #[test]
    fn any_physical_event_wakes_a_dark_screen_and_is_consumed() {
        let t0 = Instant::now();
        let mut a = armed(60, t0);
        assert!(a.tick(t0 + Duration::from_secs(60)));
        a.note_frame_queued();
        a.went_dark();

        let wake = t0 + Duration::from_secs(90);
        assert_eq!(a.note_physical(wake), Wake::Consumed);
        assert!(a.note_physical(wake).consumes());
        assert_eq!(a.phase(), Phase::Waking);
        assert!(
            !a.is_covering(),
            "the cover comes down on the wake, or the frame that re-enables the CRTC is black"
        );
        assert!(!a.is_dark(), "and a flip may be queued again");

        // The wake ends at the first completed flip, and only then.
        assert!(a.note_flip_completed());
        assert_eq!(a.phase(), Phase::Lit);
        assert!(
            !a.note_flip_completed(),
            "a second flip is not a second wake"
        );
        assert_eq!(a.note_physical(wake + Duration::from_secs(1)), Wake::Awake);
    }

    /// **A dark screen must never also swallow input forever.** The consume is
    /// defence in depth; the consent guard restart is what holds the security
    /// property, so this may fail open and does.
    #[test]
    fn a_wake_that_never_completes_stops_eating_input() {
        let t0 = Instant::now();
        let mut a = armed(60, t0);
        assert!(a.tick(t0 + Duration::from_secs(60)));
        a.note_frame_queued();
        a.went_dark();

        let wake = t0 + Duration::from_secs(90);
        assert_eq!(a.note_physical(wake), Wake::Consumed);
        assert!(!a.wake_expired(wake + WAKE_DEADLINE - Duration::from_millis(1)));
        assert_eq!(
            a.note_physical(wake + WAKE_DEADLINE - Duration::from_millis(1)),
            Wake::Consumed
        );
        assert!(a.wake_expired(wake + WAKE_DEADLINE));
        assert_eq!(a.note_physical(wake + WAKE_DEADLINE), Wake::Overdue);
        assert!(
            !a.note_physical(wake + WAKE_DEADLINE).consumes(),
            "past the deadline the human's keystrokes reach their apps again"
        );
    }

    /// **A paused seat holds no blank** (D-030(7)), and returning restarts the
    /// countdown rather than resuming one frozen mid-way.
    #[test]
    fn a_seat_taken_away_forces_the_screen_lit_and_stops_the_clock() {
        let t0 = Instant::now();
        let mut a = armed(60, t0);
        assert!(a.tick(t0 + Duration::from_secs(60)));
        a.note_frame_queued();
        a.went_dark();

        a.set_seat_absent(true, t0 + Duration::from_secs(61));
        assert_eq!(
            a.phase(),
            Phase::Lit,
            "a paused session must not hold a blank it cannot undo: the CRTC is not ours to \
             re-enable and clear() answers DeviceInactive"
        );
        assert!(a.seat_absent());
        assert!(
            !a.tick(t0 + Duration::from_secs(8 * 60 * 60)),
            "time on another VT is not idle time"
        );

        let back = t0 + Duration::from_secs(8 * 60 * 60);
        a.set_seat_absent(false, back);
        assert!(
            !a.tick(back + Duration::from_secs(59)),
            "the countdown restarts from the return"
        );
        assert!(a.tick(back + Duration::from_secs(60)));
    }

    #[test]
    fn the_cover_replaces_the_view_and_lowering_leaves_no_pixel_behind() {
        const W: u32 = 96;
        const H: u32 = 64;
        let bg = [0x40, 0x50, 0x60];
        let mut surface = BlankSurface::new();
        let clean = flat(W, H, bg);

        let mut view = clean.clone();
        surface.composite_over(&mut view, W, H);
        assert_eq!(view, clean, "no cover up means no pixel changes");

        assert!(surface.set_covering(true));
        assert!(
            !surface.set_covering(true),
            "an idempotent raise is no change"
        );
        let g = surface.generation();
        let mut view = clean.clone();
        surface.composite_over(&mut view, W, H);
        assert!(
            view.chunks_exact(BYTES_PER_PIXEL).all(|p| p == COVER_RGBA),
            "not one pixel of the realm view may survive the cover"
        );

        assert!(surface.set_covering(false));
        assert_ne!(surface.generation(), g);
        let mut view = clean.clone();
        surface.composite_over(&mut view, W, H);
        assert_eq!(view, clean);
    }

    #[test]
    fn a_mis_sized_or_degenerate_view_is_refused_not_panicked_on() {
        let mut surface = BlankSurface::new();
        surface.set_covering(true);
        let mut wrong = vec![0u8; 10];
        surface.composite_over(&mut wrong, 64, 48);
        assert_eq!(wrong, vec![0u8; 10], "a mis-sized view must be left alone");
        let mut empty: Vec<u8> = Vec::new();
        surface.composite_over(&mut empty, 0, 48);
        assert!(empty.is_empty());
    }

    /// The clock-pair detector, over the three cases that matter: the ordinary
    /// round, the suspend, and the NTP step backwards.
    #[test]
    fn a_resume_is_the_two_clocks_disagreeing_and_nothing_else() {
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mono = Instant::now();
        let mut watch = ResumeWatch::new();

        assert!(
            watch.sample(wall, mono).is_none(),
            "the first round has nothing to compare against"
        );
        // An ordinary round: both clocks advanced together.
        assert!(watch
            .sample(
                wall + Duration::from_millis(16),
                mono + Duration::from_millis(16)
            )
            .is_none());
        // A long but awake gap: the loop simply had nothing to do.
        let base_wall = wall + Duration::from_millis(16);
        let base_mono = mono + Duration::from_millis(16);
        assert!(watch
            .sample(
                base_wall + Duration::from_secs(3600),
                base_mono + Duration::from_secs(3600)
            )
            .is_none());
        // The suspend: wall advanced two hours, monotonic advanced a round.
        let base_wall = base_wall + Duration::from_secs(3600);
        let base_mono = base_mono + Duration::from_secs(3600);
        let gap = watch
            .sample(
                base_wall + Duration::from_secs(7200),
                base_mono + Duration::from_millis(20),
            )
            .expect("a two-hour wall jump with no monotonic advance is a resume");
        assert!(gap >= Duration::from_secs(7199) && gap <= Duration::from_secs(7200));
        // A wall clock stepped backwards is never a resume.
        let base_wall = base_wall + Duration::from_secs(7200);
        let base_mono = base_mono + Duration::from_millis(20);
        assert!(watch
            .sample(
                base_wall - Duration::from_secs(60),
                base_mono + Duration::from_millis(16)
            )
            .is_none());
    }

    /// Just under the threshold is not a resume, and just over is.
    #[test]
    fn the_suspend_threshold_is_a_real_edge() {
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mono = Instant::now();

        let mut watch = ResumeWatch::new();
        watch.sample(wall, mono);
        assert!(watch
            .sample(
                wall + SUSPEND_GAP - Duration::from_millis(1),
                mono + Duration::from_millis(0)
            )
            .is_none());

        let mut watch = ResumeWatch::new();
        watch.sample(wall, mono);
        assert_eq!(
            watch.sample(wall + SUSPEND_GAP, mono + Duration::from_millis(0)),
            Some(SUSPEND_GAP)
        );
    }
}
