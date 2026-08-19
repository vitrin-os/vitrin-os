// SPDX-License-Identifier: MPL-2.0
//! The human's **attention key** (WS-E.1.7, issue #232): the second, and only
//! other, core-owned physical chord — a tapped, consumed Super that opens a
//! short, single-use window in which the two layout verbs are not refused
//! `preempted`, and that delivers one argument-free `vitrin_principal.attention`
//! event to every principal holding layout authority.
//!
//! # What it is for, and the loop it closes
//!
//! A human at an in-realm shell types `focus editor` and presses Enter. That
//! Enter is physical input, so it marks [`crate::input::PhysicalPresence`] in
//! the realm the human is in; `layout_focus` contends for attention, so the
//! chokepoint's step 5c refuses it `preempted` — judged against the realm
//! physical input currently follows. **The input that tells the client to act
//! is the input that forbids it**, and pressing Enter again re-arms the window,
//! so it is a deterministic loop rather than a race. `fullscreen on<Enter>`
//! hits the identical loop through `layout_arrange`.
//!
//! The wire could not distinguish *"the human asked for this"* from *"an agent
//! tried it while the human happened to be typing"*. This is the smallest
//! mechanism that expresses the distinction.
//!
//! # The framing, which nothing here may drift from: **it delegates nothing**
//!
//! The press is not a confirmation and it is not a consent decision. It is the
//! human making a statement about *their own* input state — *"my hand is off
//! this app; do not defend it on my behalf for the next moment"* — which
//! withdraws a transient courtesy the core extends to the human's typing.
//! Every authority a client exercises afterwards came from a grant the human
//! approved on a consent card that named the principal and the realm. A client
//! that provokes the press gains **timing**, never authority. That is the
//! answer to the confused-deputy question, and it is why **no verb bit is
//! allocated**: a grantable "receive the human's attention key" verb would put
//! the delegation framing on the wire for a signal that delegates nothing
//! (this module added nothing to
//! [`Verb::VALID_MASK`](vitrin_protocol::generated::vitrin_grant::Verb::VALID_MASK),
//! which read 575 when that decision was taken and reads 639 since P2.6.5
//! allocated `designate_file` — a bit that has nothing to do with the
//! attention key and is not exempted by it).
//!
//! Why a bare bit is acceptable at all: **step 5c is not a focus-theft
//! defence.** It is a 500 ms courtesy
//! ([`crate::input::PHYSICAL_HOLD_WINDOW`]). A principal holding
//! `layout_focus` can already take the output at any moment the human pauses
//! for half a second, which is most moments. The grant — and its revocability,
//! and the dead-man switch — is what bounds focus theft. Lifting 5c for one
//! second on the human's own deliberate gesture widens an already-wide door by
//! a hair, and closes an interaction that is otherwise permanently broken.
//!
//! # Not a consent prompt, and the reason is scarcity
//!
//! The honest alternative was *no chord at all*: when step 5c would refuse a
//! layout use `preempted`, raise a prompt naming the principal and the realm
//! instead. That is strictly stronger on the properties this module is weakest
//! at — it cannot be hoarded, it names its subject in core-drawn pixels, and
//! confused-deputy provocation dies against it. It loses on one thing and it is
//! decisive: in an in-realm shell 5c fires on *every* switch, so the prompt
//! would appear on the session's single most-used action, answered by a mouse
//! click through the anti-tapjacking guard ([`crate::consent::grab`]) several
//! times a minute. That trains reflexive clicking on the one surface in this
//! system whose entire worth is that a human reads it. A modal card is the
//! right instrument for a first-contact delegation of authority and the wrong
//! one for a courtesy the human is withdrawing from themselves.
//!
//! # This is a neighbour of the dead-man switch, never a sibling
//!
//! [`crate::deadman`]'s module docs used to say the core owns exactly *one*
//! physical chord. This module falsifies that sentence, and the two are kept
//! structurally apart at every level rather than merged into one "core chord"
//! notion:
//!
//! | | dead-man ([`crate::deadman`]) | attention (here) |
//! |---|---|---|
//! | gesture | **hold**, for the irreversible and severe | **tap**, for the cheap and reversible |
//! | vocabulary | no modifiers, ever ([`Chord::VOCABULARY`](crate::deadman::Chord)) | modifiers only — the two Supers |
//! | where it acts | [`PreemptionHook::observe`] — unconditional, unstoppable, owns all its own state | [`PreemptionHook::gate`] only — suppressible by anything above it |
//! | stacking | **outside** this hook, so a chord press wins | innermost |
//! | failure mode | a revocation that does not happen | a focus change that does not happen |
//!
//! The general rule the split fixes for any future core-owned chord: **a hold
//! is for the irreversible and severe; a tap is for the cheap and reversible.**
//! An accidental attention press costs at most one focus change, undone by
//! pressing again; the dead-man may not fire by accident at all.
//!
//! The two vocabularies are **disjoint by construction** — `Chord::VOCABULARY`
//! excludes every modifier for its own good reason and this one contains
//! nothing else — so they cannot collide even before the startup equality
//! check, which `main.rs` keeps anyway as defence in depth against a later
//! vocabulary edit.
//!
//! # Consumed, not delivered, and that is a security property
//!
//! `deadman.rs` replays a tap because Escape is precious to a browser; Super is
//! not. Consuming means the confined app **never learns the human pressed
//! it**, so the only path by which a client can observe the signal is the
//! authority-gated wire event — which closes what would otherwise be a free,
//! silent keystroke-timing oracle available to every app in every realm. Both
//! halves of the press/release pair are consumed, which keeps the router's
//! per-keysym accounting whole: a release whose press was consumed is dropped
//! by the router's own razor regardless, and consuming a release whose press
//! this same gate consumed is the one sound exception
//! [`PreemptionHook`] names.
//!
//! # The chord still marks physical presence
//!
//! [`crate::input::InputRouter::route_into`] notes presence **above** the hook
//! stack, so a consumed key still records the human's hand — and it should.
//! The core must not manufacture a false fact about where the human is; a
//! seat-delivered actuation into the bound realm is still correctly refused
//! `preempted` in the same second. Suppressing the note would have been the
//! smaller diff and would have made the mechanism invisible at the one gate it
//! changes. The exemption is evaluated *over* presence rather than hidden
//! inside it.
//!
//! # The window: one second, fixed, single-use, claimed only when it worked
//!
//! [`ATTENTION_WINDOW`] is not a flag, unlike `--dead-man-hold`: that hold is
//! an ergonomics parameter with a legitimate range, this is a security
//! parameter with no deployment-specific right answer, and an operator who
//! lengthened it would be weakening step 5c with nothing on screen to say so.
//! One second is what a slow client's round trip needs — core sees press, event
//! on the wire, a Python client's event loop wakes, `focus()` arrives,
//! chokepoint — and `PHYSICAL_HOLD_WINDOW`'s 500 ms is too tight for it.
//!
//! One press admits **at most one** layout use, which is what makes "one bit"
//! literally true: it cannot authorise a burst, and it cannot be used to focus,
//! then fullscreen, then focus again. It is claimed at the chokepoint's step-6
//! admission and only when [`crate::input::PhysicalPresenceMap::owns_target`]
//! actually returned true — if the human's hand had already lapsed, the use
//! needed no exemption and the window stays open. A refused use never claims
//! it, for the same reason a refused use never burns a `once` rung.
//!
//! That "exactly once, and only when it did work" property is not a
//! convention here: [`AttentionSignal::exempt`] is the only way to obtain an
//! [`Exemption`], `Exemption` has no public constructor, is neither `Copy` nor
//! `Clone`, and [`AttentionSignal::claim`] consumes it. A second claim from one
//! press is a compile error rather than a red test.
//!
//! # The delivered-to set is the only narrowing there is, and it is the
//! design's weakest joint
//!
//! Any principal that was sent the event may claim the window; the core cannot
//! tell which of two focus holders the human meant. It cannot be fixed here:
//! choosing a recipient means choosing a shell, which is the window-management
//! policy PRD §5.1 exiles from the core, and the mapping that would make it
//! structural — "the principal that draws in the bound realm" — does not exist,
//! because a realm and a principal connection have no binding. The narrowing
//! that *is* cheap is taken: only principals in the set the event was delivered
//! to **at press time** may claim, so a grant resolving inside the window
//! cannot race in. What bounds the rest is that a thief gains one second of
//! something it could do at any lull, that the claim is journaled with its
//! principal and grant, and that the human's own switch visibly fails when it
//! is stolen.
//!
//! # What the human sees
//!
//! "Nothing" was not acceptable — the consent surface exists precisely because
//! this project refuses unattributed authority. While the window is open the
//! core draws [`composite_attention_marker`] on the human-visible output only
//! ([`crate::backend::human_visible_from_view`]), never in
//! [`Scene::compose`](crate::scene::Scene::compose), so an agent cannot
//! observe the human's attention presses through `frame_ready` — the same
//! structural argument the consent overlay and the trust band already rest on.
//! It is drawn **beside** the reserved band, never inside it: the band's whole
//! value is that its secret colour has exactly one correct appearance, and a
//! band that sometimes carries a marker is a band whose correct appearance is
//! fuzzier. What the marker buys the human: a focus change that happened with
//! no marker up was not theirs.
//!
//! Since WS-E.2.3 (issue #215) the marker is **not alone** in those rows: the
//! status strip ([`crate::status`]) occupies the same row range whenever
//! `--status` is on. The two do not fight. The strip is composited before the
//! band and the marker after it, so the marker is on top wherever they meet —
//! and, so the ordering never has to be the thing that saves it, the strip
//! leaves the leftmost [`MARKER_W`] columns blank. It reads that width from
//! here rather than restating it, and a `const` assertion in
//! `status::render` turns a marker that outgrew the lane into a compile
//! error. What is unchanged is the rule this module set: neither surface is
//! ever drawn in the band.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

use crate::identity::PrincipalIdentity;
use crate::input::{Gate, PreemptionHook, SeatInput, SeatInputKind};

/// The default attention chord: the left Super key.
pub(crate) const DEFAULT_CHORD: &str = "super";

/// How long one press keeps the window open.
///
/// **One second, fixed, and deliberately not configurable** — module docs. It
/// has to cover a slow client's whole round trip, and it is a security
/// parameter rather than an ergonomics one.
pub(crate) const ATTENTION_WINDOW: Duration = Duration::from_secs(1);

/// The two verbs the window exempts, and the two whose holders receive the
/// event. Nothing else is attention-keyed, and the pair is written once so a
/// third reader cannot disagree with the first two.
pub(crate) const EXEMPT_VERBS: [vitrin_protocol::generated::vitrin_grant::Verb; 2] = [
    vitrin_protocol::generated::vitrin_grant::Verb::LAYOUT_FOCUS,
    vitrin_protocol::generated::vitrin_grant::Verb::LAYOUT_ARRANGE,
];

/// A configured attention chord: the human-facing name, the evdev scancode a
/// device produces for it, and the keysym the core's own layout-invariant table
/// resolves that scancode to.
///
/// All three are carried because all three are needed and none can be derived
/// from another at the point of use: the keysym is what [`AttentionSignal`]
/// matches, the scancode is what the `physical-input-injector` channel's
/// `attention` line feeds to the production intake, and the name is what the
/// flight recorder and the operator's log see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttentionChord {
    name: &'static str,
    evdev: u32,
    keysym: u32,
}

impl AttentionChord {
    /// The chord vocabulary: name → (evdev scancode, keysym).
    ///
    /// **Exactly the two Super keys, and nothing else.** They are the only keys
    /// in [`crate::input::invariant_keysym`] that every desktop already
    /// reserves for the compositor, so consuming them costs the human nothing
    /// they expected to keep. That is the whole of the argument for eating a
    /// key unconditionally, and it is also the argument's limit: an app that
    /// legitimately wants Super — a nested compositor, a VM viewer, a
    /// remote-desktop client in a realm — loses it with no pass-through, which
    /// is a published cost rather than an oversight.
    ///
    /// **Disjoint from [`crate::deadman::Chord`]'s vocabulary by
    /// construction**: that one excludes every modifier (a human holds Shift
    /// for seconds at a time, so a modifier off-switch would fire during
    /// ordinary use) and this one contains nothing else. `main.rs` still
    /// refuses an equal pair at startup, as defence in depth against a later
    /// vocabulary edit that broke the disjointness.
    const VOCABULARY: &'static [(&'static str, u32, u32)] = &[
        // KEY_LEFTMETA -> XK_Super_L
        ("super", 125, 0xffeb),
        // KEY_RIGHTMETA -> XK_Super_R
        ("rsuper", 126, 0xffec),
    ];

    /// Resolve a configured chord name.
    ///
    /// Refuses anything the vocabulary does not name and — defence in depth
    /// against a vocabulary entry drifting from the intake table — anything
    /// intake could not deliver ([`crate::input::keysym_is_intakeable`]), or
    /// whose scancode does not actually resolve to the keysym written beside
    /// it. `Chord::parse`'s discipline, for the same reason: a session that
    /// came up with an attention key that cannot fire is the same fail-open
    /// trap, one gesture milder.
    pub fn parse(name: &str) -> Result<Self, AttentionChordError> {
        let (name, evdev, keysym) = Self::VOCABULARY
            .iter()
            .find(|(candidate, _, _)| *candidate == name)
            .copied()
            .ok_or(AttentionChordError::Unknown)?;
        if !crate::input::keysym_is_intakeable(keysym) {
            return Err(AttentionChordError::NotIntakeable);
        }
        if crate::input::invariant_keysym(evdev) != Some(keysym) {
            return Err(AttentionChordError::NotIntakeable);
        }
        Ok(Self {
            name,
            evdev,
            keysym,
        })
    }

    /// The chord's configured name (`"super"`), for logs and the journal.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The keysym intake delivers for this chord — what the gate matches.
    pub fn keysym(&self) -> u32 {
        self.keysym
    }

    /// The evdev scancode a device produces for this chord.
    ///
    /// One consumer: the `physical-input-injector` channel's `attention` line,
    /// which hands it to the production `physical_key` intake so a mock-free
    /// gate presses the *configured* chord rather than a number a harness
    /// guessed.
    #[cfg_attr(not(any(test, feature = "physical-input-injector")), allow(dead_code))]
    pub fn evdev(&self) -> u32 {
        self.evdev
    }

    /// Every chord name this build accepts, for the CLI's error message.
    pub fn vocabulary() -> impl Iterator<Item = &'static str> {
        Self::VOCABULARY.iter().map(|(name, _, _)| *name)
    }
}

/// Why a configured attention chord was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttentionChordError {
    /// Not a name this build offers.
    Unknown,
    /// Named, but intake cannot produce its keysym — so the window could never
    /// open. Unreachable while the vocabulary and the intake table agree; it
    /// exists so that if they ever stop agreeing, the failure is a startup
    /// error rather than a silently dead attention key.
    NotIntakeable,
}

impl std::fmt::Display for AttentionChordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttentionChordError::Unknown => write!(f, "unknown attention key"),
            AttentionChordError::NotIntakeable => {
                write!(f, "this build's input intake cannot deliver that key")
            }
        }
    }
}

/// Permission to skip one `preempted` refusal, handed out by
/// [`AttentionSignal::exempt`] and spent by [`AttentionSignal::claim`].
///
/// **This type is the "claimed exactly once" property**, not a test of it. Its
/// field is private to this module, so nothing outside can mint one; it is
/// neither `Copy` nor `Clone`, so the chokepoint cannot keep a spare; and
/// `claim` takes it **by value**, so a second claim from one press does not
/// compile. `#[must_use]` catches the opposite mistake — obtaining an
/// exemption and forgetting to claim it — as a warning rather than a silent
/// window that never closes.
///
/// A use that is refused *after* the exemption was obtained (the rate gate is
/// the only such gate) simply drops it: the window stays open, which is the
/// same rule `commit_use` follows for a `once` rung.
#[derive(Debug)]
#[must_use = "an exemption that suppressed a `preempted` refusal must be \
              claimed at admission, or the window never closes"]
pub(crate) struct Exemption {
    /// The principal the exemption was issued to — carried so `claim` can
    /// journal *who* spent the human's press, and so a mis-plumbed claim is
    /// visible in the log rather than anonymous.
    principal: PrincipalIdentity,
}

/// One open window: when the human pressed, and who was told.
#[derive(Debug)]
struct OpenWindow {
    /// The instant the press was gated, not the instant the event was sent —
    /// the human's gesture is what the window is measured from.
    at: Instant,
    /// The principals the `attention` event actually reached, resolved once at
    /// press time. Only these may claim, so a grant that resolves *inside* the
    /// window cannot race in behind the human's back.
    delivered_to: BTreeSet<PrincipalIdentity>,
}

/// The session's attention signal: the chord, one press
/// waiting for the embedder, and at most one open window.
///
/// Shared between the router's hook and the embedder by `Rc<RefCell<..>>` — the
/// shape [`crate::deadman::DeadManSwitch`] and
/// [`crate::consent::grab::ConsentGrab`] both follow.
///
/// **The hook does not deliver anything, and it must not.** The hook runs
/// inside [`crate::input::InputRouter::route_physical`], which holds no
/// authority state and must never grow any ([`crate::input`]'s module docs).
/// So the gate records a *pending press* here and the embedder drains it
/// ([`Self::take_pending`]), resolves who holds layout authority, sends the
/// events, and only then opens the window ([`Self::open`]). Exactly the
/// division [`crate::deadman::Trigger`] makes for the same reason.
#[derive(Debug)]
pub(crate) struct AttentionSignal {
    chord: AttentionChord,
    /// Whether the chord key is currently down. Latches the press so one hold
    /// is one signal; autorepeat is filtered upstream
    /// ([`crate::deadman`]'s tension 2), so a press arriving while this is set
    /// is evidence of a lost release and re-arms rather than being ignored.
    /// A gated press the embedder has not yet turned into a window.
    pending: Option<Instant>,
    /// The window, once the embedder said who was told.
    open: Option<OpenWindow>,
}

impl AttentionSignal {
    pub fn new(chord: AttentionChord) -> Self {
        Self {
            chord,
            pending: None,
            open: None,
        }
    }

    /// A signal nothing ever writes, for a build whose hook stack carries no
    /// [`AttentionHook`] — a plain headless run, which has no physical input
    /// device to press a chord on at all.
    ///
    /// The kernel holds one unconditionally, exactly as it holds a presence map
    /// unconditionally, so the chokepoint's exemption arm is the same code in
    /// every build and "this backend has no attention key" is a fact about the
    /// hook stack rather than about the enforcement path.
    pub fn detached() -> Self {
        Self::new(
            AttentionChord::parse(DEFAULT_CHORD).expect("the default chord is in the vocabulary"),
        )
    }

    /// The configured chord, for the CLI's startup log and the journal.
    pub fn chord(&self) -> AttentionChord {
        self.chord
    }

    /// **The gate.** Decide whether one event reaches the confined app, and
    /// record a press when one is the human's own chord.
    ///
    /// **The origin check is the whole of the unforgeability claim.** Only
    /// [`Origin::Physical`] — the tag bound at intake (B2), which
    /// [`crate::input::SeatInput::physical`]'s privacy makes unminttable
    /// outside `crate::input` — can open a window or be consumed here. An
    /// emulated key carrying this keysym is an actuation the chokepoint already
    /// admitted against a live grant: consuming it would silently overturn an
    /// authority decision the core just made (the razor
    /// [`crate::consent::grab`] applies for the same reason), and opening a
    /// window on it would let an agent manufacture the human's gesture. It is
    /// one `if`, so it is pinned adversarially by
    /// `an_emulated_super_opens_no_window_and_is_delivered_untouched`.
    pub fn gate_event(&mut self, input: &SeatInput, now: Instant) -> Gate {
        if input.origin() != Origin::Physical {
            return Gate::Deliver;
        }
        let SeatInputKind::Key { keysym, state, .. } = input.kind() else {
            return Gate::Deliver;
        };
        if *keysym != self.chord.keysym {
            return Gate::Deliver;
        }
        match state {
            KeyState::Pressed => {
                // Every press re-arms, and there is deliberately no held
                // latch. #232's decision 1 said "latched until release", and a
                // latch was written -- but nothing ever read it, so no latching
                // existed and the field was fiction. Re-arming is also the
                // CORRECT behaviour, which is why the field went rather than
                // the behaviour: autorepeat never reaches the core, so a second
                // press with no release between can only mean the release was
                // lost, and swallowing it would leave the human's key dead
                // until they pressed it twice. Same reading
                // `DeadManSwitch::observe_event` takes, on the same evidence.
                self.pending = Some(now);
            }
            KeyState::Released => {}
        }
        // Consumed in both directions, and safe in both: this gate consumed
        // the press, so the router's per-keysym pairing finds nothing to
        // reconcile and the app is left holding nothing. That is exactly the
        // one sound exception `PreemptionHook`'s pairing contract names.
        Gate::Consume
    }

    /// Drain a gated press for the embedder to turn into a window.
    ///
    /// `None` in the overwhelmingly common case. The embedder must follow a
    /// `Some` with either [`Self::open`] or nothing at all — a press that
    /// reached no layout holder opens no window, because a window nobody may
    /// claim is not open.
    pub fn take_pending(&mut self) -> Option<Instant> {
        self.pending.take()
    }

    /// Open the window on a drained press, naming exactly the principals the
    /// `attention` event reached.
    ///
    /// Replaces any window still open: a second press supersedes the first
    /// rather than extending it, so a human leaning on the key cannot bank a
    /// queue of exemptions.
    pub fn open(&mut self, at: Instant, delivered_to: BTreeSet<PrincipalIdentity>) {
        self.open = Some(OpenWindow { at, delivered_to });
    }

    /// **Is this principal exempt from `preempted` right now?**
    ///
    /// `Some` only when a window is open, unexpired at `now`, and this
    /// principal is in the set the event was delivered to at press time. The
    /// returned [`Exemption`] is the only thing [`Self::claim`] accepts, so the
    /// gate and the admission cannot disagree about whether an exemption
    /// applied.
    ///
    /// `&self`: asking costs nothing and burns nothing. Spending is
    /// [`Self::claim`]'s, at step-6 admission, and only for a use that was
    /// actually admitted.
    /// **The layout-only restriction lives here, not at the call site.** It is
    /// the sharpest security claim this whole mechanism makes -- a human's hand
    /// still mutes an agent actuating into the realm the hand is in, and no
    /// gesture of theirs can lift that -- and it had a writer and no reader:
    /// widening the chokepoint's `match` to every attention-contending use
    /// passed the entire suite *and* the mock-free gate. Pushing the test into
    /// this function gives the claim exactly one home and one unit test that
    /// reads it (`an_actuation_is_never_exempted_whatever_the_window_says`),
    /// instead of a `match` arm whose correctness nothing checked.
    ///
    /// `Pointer`/`Text` are refused an exemption even with a live window the
    /// principal was told about, because the attention key withdraws a
    /// courtesy the human extends to their OWN typing -- it says nothing about
    /// whether an agent may type into the app under their hand.
    pub fn exempt(
        &self,
        principal: &PrincipalIdentity,
        kind: &crate::enforcement::UseKind,
        now: Instant,
    ) -> Option<Exemption> {
        if !kind.is_layout() {
            return None;
        }
        let window = self.open.as_ref()?;
        if now.saturating_duration_since(window.at) >= ATTENTION_WINDOW {
            return None;
        }
        window.delivered_to.contains(principal).then(|| Exemption {
            principal: principal.clone(),
        })
    }

    /// Spend one exemption: the window closes, whatever time is left on it.
    ///
    /// Single-use is what makes "one bit" literally true — one press admits at
    /// most one layout use, so it can never authorise a burst. Consuming the
    /// [`Exemption`] by value is what makes a second claim from one press a
    /// compile error.
    pub fn claim(&mut self, exemption: Exemption) -> PrincipalIdentity {
        self.open = None;
        exemption.principal
    }

    /// Forget any open window and any pending press.
    ///
    /// Called by [`crate::deadman::apply`], belt-and-braces alongside
    /// `seal_dead_man`: a human who has just hit the off-switch is not, in the
    /// same second, lifting a defence for anybody. Sealing the grant table
    /// already refuses every layout use `revoked` before step 5c is reached, so
    /// this changes no outcome — which is precisely why it is cheap to do and
    /// worth doing.
    pub fn clear(&mut self) {
        self.pending = None;
        self.open = None;
    }

    /// Whether a window is open at `now` — the marker's only input.
    pub fn is_open(&self, now: Instant) -> bool {
        self.open
            .as_ref()
            .is_some_and(|w| now.saturating_duration_since(w.at) < ATTENTION_WINDOW)
    }
}

/// The attention key attached at the router's preemption hook point: the
/// **innermost** hook, under everything.
///
/// [`crate::backend::winit::NestedHook`] is where the whole order is written
/// down; here it matters that this is the bottom of it, and that every position
/// above is a decision rather than an arrangement:
///
/// - a **raised lock screen** (WS-E.2.2) is outermost and consumes every
///   physical event, so no attention window can open while nobody is at the
///   keyboard;
/// - a **consent prompt** consumes the chord before this hook sees it, so a
///   window cannot even open while the human is answering a security question;
/// - a **dead-man chord press** short-circuits here for the same reason, so the
///   human's off-switch can never be delayed by this hook's bookkeeping;
/// - a **clipboard chord** sits between them and this hook, outside it because
///   [`AttentionHook`] consumes both Supers and a modifier matcher below would
///   have a permanently wrong `super` bit (WS-E.2.1);
/// - and this hook implements [`PreemptionHook::gate`] **only**. Detection for
///   the dead-man lives in `observe`, which is unconditional and unstoppable;
///   putting attention in `gate` is what makes it suppressible by everything
///   above it, which is the correct posture for a mechanism whose failure mode
///   is a focus change that does not happen.
pub(crate) struct AttentionHook<H: PreemptionHook> {
    signal: Rc<RefCell<AttentionSignal>>,
    /// The dispatch turn's instant, shared with the embedder — the arrangement
    /// [`crate::consent::grab::ConsentGate`], [`crate::deadman::DeadManHook`]
    /// and the router's own presence record all use.
    now: Rc<Cell<Instant>>,
    inner: H,
}

impl<H: PreemptionHook> AttentionHook<H> {
    pub fn new(signal: Rc<RefCell<AttentionSignal>>, now: Rc<Cell<Instant>>, inner: H) -> Self {
        Self { signal, now, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for AttentionHook<H> {
    /// Nothing. The attention key has no detection half: unlike the dead-man
    /// switch it has no hold to track, and a signal that armed from an
    /// unconditional tap would be a signal a consent prompt could not suppress
    /// — which is the property decision 6 of issue #232 rests on.
    fn observe(&mut self, input: &SeatInput) {
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        match self.signal.borrow_mut().gate_event(input, self.now.get()) {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }

    fn attention(&self) -> Option<Rc<RefCell<AttentionSignal>>> {
        Some(Rc::clone(&self.signal))
    }

    fn clipboard(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::clipboard::ClipboardSignal>>> {
        self.inner.clipboard()
    }

    fn screenshot(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::screenshot::ScreenshotSignal>>> {
        self.inner.screenshot()
    }

    fn backlight(
        &self,
    ) -> Option<std::rc::Rc<std::cell::RefCell<crate::backlight::BacklightSignal>>> {
        self.inner.backlight()
    }
}

/// The marker's height and width in pixels. Small, fixed, and at the origin:
/// it says "a window is open" and nothing else, so it needs no size that
/// scales with anything.
///
/// **`pub(crate)` because a second core-drawn surface now shares this row
/// range.** [`crate::status`]'s strip (WS-E.2.3, issue #215) occupies the same
/// rows immediately below the band, and it keeps the leftmost [`MARKER_W`]
/// columns clear so the marker never lands on a glyph. It reads that width
/// from *here* — never restates it — and a `const` assertion in that module
/// turns a marker that outgrew the strip into a compile error rather than into
/// a half-covered clock. The marker itself is unchanged and still drawn after
/// the band, so it is on top of the strip wherever the two meet.
pub(crate) const MARKER_H: u32 = 6;
pub(crate) const MARKER_W: u32 = 96;

/// Paint the open-window marker onto **human-visible** output.
///
/// A short fixed-geometry tab at the top-left, immediately **below** the
/// reserved trusted band and never inside it. Three things are deliberate:
///
/// - **Beside the band, never in it.** The band's whole value is that its
///   secret colour has exactly one correct appearance
///   ([`crate::consent::indicator`]); a band that sometimes carries a marker is
///   a band whose correct appearance is fuzzier, and the human's check against
///   a forged prompt gets weaker. Rows `[0, TRUST_BAND_HEIGHT)` are untouched.
/// - **Human-visible output only.** Applied in
///   [`crate::backend::human_visible_from_view`], downstream of the point where
///   capture takes the bare realm view, so an agent cannot observe the human's
///   attention presses through `frame_ready` — the same structural argument the
///   consent overlay and the trust band rest on, not a checked flag.
/// - **Below the dead-man indicator in draw order.** That bar is composited
///   last of all, so nothing here can hide a hold in progress.
/// - **Above the status strip in draw order** (WS-E.2.3). The strip shares
///   these rows and is composited before the band; the marker is composited
///   after it, in a lane the strip leaves blank ([`MARKER_W`]), so the two
///   core-drawn surfaces neither overlap nor depend on the order to look
///   right.
///
/// `view` must be `width * height * 4` RGBA8888. A dimension mismatch is
/// refused rather than panicking, as elsewhere in the presentation path.
pub(crate) fn composite_attention_marker(view: &mut [u8], width: u32, height: u32) {
    use crate::paint::canvas::{Canvas, Rect};

    /// A cool, unmistakably non-secret colour: it must not be confusable with
    /// the session's trusted band (a random colour in `[64, 255]` per channel)
    /// nor with the dead-man's amber. Fixed and public by design — this marker
    /// asserts nothing about authenticity, so it carries no secret to leak.
    const MARKER: [u8; 4] = [0x30, 0xc0, 0xff, 0xff];

    let top = crate::consent::TRUST_BAND_HEIGHT;
    if width == 0 || height <= top {
        return;
    }
    let Some(mut canvas) = Canvas::new(view, width, height) else {
        return;
    };
    canvas.fill_rect(
        Rect {
            x: 0,
            y: top as i32,
            w: MARKER_W.min(width),
            h: MARKER_H.min(height - top),
        },
        MARKER,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputRouter, KeySource, NoopHook, SeatInputKind};
    use vitrin_protocol::generated::vitrin_grant::Verb;

    fn principal(name: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(name).expect("a well-formed test identity")
    }

    fn signal() -> AttentionSignal {
        AttentionSignal::new(AttentionChord::parse(DEFAULT_CHORD).unwrap())
    }

    fn emulated_super(pressed: bool) -> SeatInput {
        SeatInput::emulated(SeatInputKind::Key {
            source: KeySource::Keysym,
            keysym: 0xffeb,
            state: if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
        })
    }

    // -- the vocabulary and its startup discipline --------------------------

    #[test]
    fn the_vocabulary_is_exactly_the_two_supers_and_every_entry_survives_intake() {
        let names: Vec<&str> = AttentionChord::vocabulary().collect();
        assert_eq!(names, vec!["super", "rsuper"]);
        for name in names {
            let chord = AttentionChord::parse(name).expect("every offered name must parse");
            assert!(
                crate::input::keysym_is_intakeable(chord.keysym()),
                "{name}: a chord intake cannot deliver could never open a window"
            );
            assert_eq!(
                crate::input::invariant_keysym(chord.evdev()),
                Some(chord.keysym()),
                "{name}: the scancode beside the keysym must be the one that produces it"
            );
        }
        assert_eq!(AttentionChord::parse("super").unwrap().keysym(), 0xffeb);
        assert_eq!(AttentionChord::parse("rsuper").unwrap().keysym(), 0xffec);
    }

    #[test]
    fn the_attention_vocabulary_is_disjoint_from_the_dead_mans() {
        // Decision 1: disjoint **by construction**, because `Chord::VOCABULARY`
        // excludes every modifier and this one contains nothing else. The
        // startup equality check `main.rs` keeps is defence in depth against a
        // later edit to either list; this test is what would notice that edit.
        let dead: Vec<&str> = crate::deadman::Chord::vocabulary().collect();
        for name in AttentionChord::vocabulary() {
            assert!(
                !dead.contains(&name),
                "`{name}` is in both chord vocabularies: the two core-owned chords \
                 must not be able to collide"
            );
        }
    }

    #[test]
    fn an_unknown_or_undeliverable_key_is_refused_rather_than_defaulted() {
        assert_eq!(
            AttentionChord::parse("f13"),
            Err(AttentionChordError::Unknown)
        );
        assert_eq!(
            AttentionChord::parse("esc"),
            Err(AttentionChordError::Unknown),
            "the dead-man's own key is not in this vocabulary at all"
        );
        assert_eq!(
            AttentionChord::parse("SUPER"),
            Err(AttentionChordError::Unknown)
        );
        assert_eq!(AttentionChord::parse(""), Err(AttentionChordError::Unknown));
    }

    // -- unforgeability: the one `if` ---------------------------------------

    #[test]
    fn an_emulated_super_opens_no_window_and_is_delivered_untouched() {
        // **The whole of the unforgeability claim, pinned adversarially.**
        // In the spirit of `an_agent_cannot_answer_the_prompt_it_petitioned_for`:
        // an agent that holds `actuate_text` over any realm can already put a
        // keysym on the wire, and `invariant_keysym` resolves Super for it. If
        // the gate judged by keysym alone -- keeping every routing property in
        // this crate true -- that agent would be able to manufacture the human's
        // own gesture and lift step 5c for itself.
        let mut sig = signal();
        let t0 = Instant::now();
        assert_eq!(sig.gate_event(&emulated_super(true), t0), Gate::Deliver);
        assert_eq!(sig.gate_event(&emulated_super(false), t0), Gate::Deliver);
        assert!(
            sig.take_pending().is_none(),
            "an emulated key carrying the chord's keysym must open no window"
        );
        assert!(!sig.is_open(t0));
    }

    #[test]
    fn an_emulated_super_cannot_reach_the_window_through_the_real_hook_stack() {
        // The same claim one layer out, through the hook the backends really
        // stack, so a refactor that moved the origin check out of
        // `gate_event` and into the hook is still caught.
        let sig = Rc::new(RefCell::new(signal()));
        let t0 = Instant::now();
        let now = Rc::new(Cell::new(t0));
        let mut hook = AttentionHook::new(Rc::clone(&sig), Rc::clone(&now), NoopHook);
        assert_eq!(hook.gate(&emulated_super(true)), Gate::Deliver);
        assert!(sig.borrow_mut().take_pending().is_none());
    }

    #[test]
    fn a_physical_chord_press_is_consumed_and_records_one_pending_press() {
        // The positive half, driven through the production router so the
        // consumption is the router's own answer rather than the hook's.
        let sig = Rc::new(RefCell::new(signal()));
        let t0 = Instant::now();
        let now = Rc::new(Cell::new(t0));
        let mut router = InputRouter::detached(AttentionHook::new(
            Rc::clone(&sig),
            Rc::clone(&now),
            NoopHook,
        ));
        let realm = crate::grants::RealmId::new("realm-0");
        let _owed = router.bind_to(&realm);
        let pressed = crate::input::physical_key(
            AttentionChord::parse(DEFAULT_CHORD).unwrap().evdev(),
            None,
            KeyState::Pressed,
        );
        assert_eq!(pressed.len(), 1, "the chord must survive the intake table");
        let mut delivered = Vec::new();
        for input in pressed {
            if let Some(d) = router.route_physical(input, (640, 480), Some((640, 480))) {
                delivered.push(d);
            }
        }
        assert!(
            delivered.is_empty(),
            "the chord is consumed: the confined app must never learn the human pressed it"
        );
        assert!(
            sig.borrow_mut().take_pending().is_some(),
            "a physical chord press must record a pending press"
        );
    }

    // -- the window: single-use, membership, expiry --------------------------

    #[test]
    fn the_window_is_claimed_exactly_once() {
        let mut sig = signal();
        let t0 = Instant::now();
        let holder = principal("vitrin://local/agent/shell");
        sig.open(t0, BTreeSet::from([holder.clone()]));

        let first = sig
            .exempt(&holder, &crate::enforcement::UseKind::LayoutFocus, t0)
            .expect("the first use is exempt");
        assert_eq!(sig.claim(first), holder);
        assert!(
            sig.exempt(&holder, &crate::enforcement::UseKind::LayoutFocus, t0)
                .is_none(),
            "one press admits at most one layout use"
        );
    }

    /// **An actuation is never exempted, whatever the window says** — the
    /// sharpest claim this mechanism makes, and until this test existed it had
    /// a writer and no reader: widening the chokepoint to exempt every
    /// attention-contending use passed the whole suite *and* the mock-free
    /// gate.
    ///
    /// The human's attention key withdraws a courtesy they extend to their own
    /// typing. It says nothing whatever about whether an agent may type or
    /// click into the app under their hand, and if it ever did, a human could
    /// be socially engineered into unmuting an agent in the realm they are
    /// working in — with one keypress and nothing on screen naming what they
    /// gave up. Every fixture below is maximally favourable: a live window, the
    /// principal in the delivered-to set, well inside expiry. Only the kind
    /// differs.
    #[test]
    fn an_actuation_is_never_exempted_whatever_the_window_says() {
        use crate::enforcement::UseKind;

        let mut sig = signal();
        let t0 = Instant::now();
        let holder = principal("vitrin://local/agent/shell");
        sig.open(t0, BTreeSet::from([holder.clone()]));

        for (name, kind) in [
            (
                "pointer",
                UseKind::Pointer(crate::input::SeatInputKind::Motion { x: 1.0, y: 1.0 }),
            ),
            (
                "text",
                UseKind::Text(crate::input::SeatInputKind::Text {
                    text: String::from("x"),
                }),
            ),
        ] {
            assert!(
                sig.exempt(&holder, &kind, t0).is_none(),
                "a seat-delivered actuation ({name}) must never be exempted: the human's \
                 hand still mutes an agent acting in the realm the hand is in, and no \
                 gesture of theirs may lift that"
            );
        }

        // ...and the window is still there, unspent, for the use it IS for.
        assert!(
            sig.exempt(&holder, &UseKind::LayoutFocus, t0).is_some(),
            "refusing the actuations must not have consumed the window: a layout use is \
             what the press was for"
        );
    }

    #[test]
    fn only_a_principal_the_event_reached_may_claim() {
        let mut sig = signal();
        let t0 = Instant::now();
        let told = principal("vitrin://local/agent/shell");
        let untold = principal("vitrin://local/agent/other");
        sig.open(t0, BTreeSet::from([told.clone()]));
        assert!(sig
            .exempt(&untold, &crate::enforcement::UseKind::LayoutFocus, t0)
            .is_none());
        assert!(sig
            .exempt(&told, &crate::enforcement::UseKind::LayoutFocus, t0)
            .is_some());
    }

    #[test]
    fn the_window_expires_and_a_press_nobody_was_told_of_opens_nothing() {
        let mut sig = signal();
        let t0 = Instant::now();
        let holder = principal("vitrin://local/agent/shell");
        sig.open(t0, BTreeSet::from([holder.clone()]));
        assert!(sig
            .exempt(
                &holder,
                &crate::enforcement::UseKind::LayoutFocus,
                t0 + ATTENTION_WINDOW
            )
            .is_none());
        assert!(!sig.is_open(t0 + ATTENTION_WINDOW));
        assert!(sig.is_open(t0));

        // An empty delivered-to set is a window nobody may claim.
        sig.open(t0, BTreeSet::new());
        assert!(sig
            .exempt(&holder, &crate::enforcement::UseKind::LayoutFocus, t0)
            .is_none());
    }

    #[test]
    fn clearing_the_signal_shuts_an_open_window_and_drops_a_pending_press() {
        let mut sig = signal();
        let t0 = Instant::now();
        let holder = principal("vitrin://local/agent/shell");
        sig.open(t0, BTreeSet::from([holder.clone()]));
        sig.pending = Some(t0);
        sig.clear();
        assert!(sig
            .exempt(&holder, &crate::enforcement::UseKind::LayoutFocus, t0)
            .is_none());
        assert!(sig.take_pending().is_none());
    }

    #[test]
    fn the_two_exempt_verbs_are_the_two_layout_verbs_and_no_bit_is_allocated() {
        assert_eq!(EXEMPT_VERBS, [Verb::LAYOUT_FOCUS, Verb::LAYOUT_ARRANGE]);
        // The tripwire decision 7 asks for: this issue allocates no verb bit,
        // positively, because a grantable "receive the human's attention key"
        // verb would put the delegation framing on the wire for a signal that
        // delegates nothing.
        //
        // **Re-pinned 575 -> 639 by P2.6.5 (issue #189)**, which allocated
        // `designate_file` (64). What this pin says has NOT changed: the
        // attention signal still allocates nothing, and `EXEMPT_VERBS` above
        // is still exactly the two layout verbs. Pinning the whole mask is a
        // proxy for "no bit was added *here*", so it moves whenever any epic
        // adds one -- and moving it is the point, because it forces the next
        // author to check that the new bit is not an attention verb before
        // re-pinning. `designate_file` is not: nothing about the human's
        // attention key is exempted for it, and the equality on the line
        // above -- an equality against a two-element array, not a membership
        // test -- is already what forbids adding it.
        //
        // TWO FURTHER ASSERTIONS STOOD HERE AND WERE DELETED RATHER THAN
        // KEPT. `VALID_MASK & DESIGNATE_FILE == DESIGNATE_FILE` cannot fail
        // while the pin below holds, and `!EXEMPT_VERBS.contains(..)` cannot
        // fail while the equality above holds. Neither could ever go red, and
        // a check that cannot go red is the exact thing the pin below is for.
        assert_eq!(Verb::VALID_MASK, 639);
    }

    // -- the marker ----------------------------------------------------------

    #[test]
    fn the_marker_never_paints_a_row_of_the_trusted_band() {
        const W: u32 = 64;
        const H: u32 = 40;
        let mut view = vec![0u8; (W * H * 4) as usize];
        composite_attention_marker(&mut view, W, H);
        let band = crate::consent::TRUST_BAND_HEIGHT as usize;
        assert!(
            view[..band * W as usize * 4].iter().all(|&b| b == 0),
            "the marker must sit BESIDE the reserved band, never inside it"
        );
        let below = band * W as usize * 4;
        assert!(
            view[below..].iter().any(|&b| b != 0),
            "the marker must actually paint something below the band"
        );
    }

    #[test]
    fn the_marker_refuses_a_mismatched_buffer_rather_than_panicking() {
        let mut too_small = vec![0u8; 4];
        composite_attention_marker(&mut too_small, 64, 40);
        assert_eq!(too_small, vec![0u8; 4]);
        // A view with no room below the band paints nothing at all.
        let mut short = vec![0u8; (64 * crate::consent::TRUST_BAND_HEIGHT * 4) as usize];
        composite_attention_marker(&mut short, 64, crate::consent::TRUST_BAND_HEIGHT);
        assert!(short.iter().all(|&b| b == 0));
    }
}
