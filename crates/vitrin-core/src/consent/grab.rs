// SPDX-License-Identifier: MPL-2.0
//! The consent surface's input grab and decision routing (P1.7.2, issue
//! #38): the *trust* half of the consent surface.
//!
//! P1.7.1 drew a prompt. A prompt nobody can answer is not a consent
//! surface; a prompt an agent can answer — or can act around while the human
//! reads it — is worse than none, because it manufactures a record of
//! consent that was never given. This module closes both halves: while a
//! prompt is up, physical input belongs to the prompt and to nothing else,
//! and a click on one of its buttons becomes a petition resolution through
//! the same state machine every other consent path uses.
//!
//! # What "exclusively" has to mean
//!
//! `docs/protocol/05-vitrin_consent.md` on `shown`: "the prompt is visible.
//! All physical input now routes exclusively to it (the input grab)."
//! Read literally, which covers far more than clicks:
//!
//! - **Motion is consumed.** Not because a moving cursor authorizes
//!   anything, but because an app that tracks hover would otherwise learn
//!   the pointer path the human traced across a security decision — which
//!   button they hovered, in what order, for how long. That is a side
//!   channel out of the consent surface into the confined app, and it is
//!   free to close.
//! - **Scroll and key presses are consumed**, for the same reason plus a
//!   blunter one: a human answering a prompt is not typing at the app, so
//!   any key the app received during a prompt would be a keystroke aimed
//!   somewhere else.
//! - **Button presses are consumed**, and a press inside a button rectangle
//!   arms the decision its release commits (below).
//!
//! *Releases* — button and key alike — are the one deliberate exception,
//! and they are not a hole; see the pairing contract next.
//!
//! # Hold-until-release: why releases are always delivered
//!
//! [`crate::input::PreemptionHook`]'s pairing contract states it: a gate
//! that begins consuming while router-delivered presses are outstanding
//! should keep answering [`Gate::Deliver`] for releases. A prompt can appear
//! in the middle of a human's drag, and the press that began that drag is
//! already in the app. Consuming its release would leave the app holding a
//! button forever — a stuck mouse in a confined app is exactly the kind of
//! damage a security prompt must not cause.
//!
//! **The keyboard is the same argument and was initially the same bug.** A
//! prompt can equally appear while a human holds `Shift` or `Ctrl`. This
//! module once consumed key releases as well as key presses, on the reading
//! that "no key answers the prompt" made the whole keyboard inert — but
//! inert at the gate is not inert in the app, and the app was left holding
//! whatever was down when the prompt appeared. A latched modifier is worse
//! than a stuck mouse button: it silently rewrites the meaning of every
//! keystroke the human types after the prompt closes, and a latched letter
//! key autorepeats. So the rule here is uniform and has no exceptions worth
//! remembering: **this gate never consumes a release.**
//!
//! Blanket-delivering releases is safe rather than merely convenient,
//! because the router pairs releases **per button code and per keysym** and
//! drops any whose press it did not deliver ([`crate::input`]). So a
//! release reaches the app if and only if its own press did, which — since
//! presses during the grab are consumed — means only presses from before
//! the grab. Nothing the human aimed at the prompt can leak through, and no
//! bookkeeping in this module decides that: the router's pairing does. (The
//! per-keysym half of that pairing did not exist when this module was
//! written; it was added with this fix, because a rule enforced by the
//! router's own accounting is worth more than a shadow table here that
//! could drift from it across a shim generation.)
//!
//! It also composes with the physical device's own state machine. A button
//! cannot be pressed twice without an intervening release, so the release
//! that ends a pre-grab drag can never be mistaken for the release that
//! commits a prompt decision: the former has no armed button (its press was
//! not consumed by this gate), the latter always does.
//!
//! # The guard interval: a click already in flight decides nothing
//!
//! A prompt appears at a moment the human did not choose. The agent chose
//! it: the prompt goes up because `vitrin_realm.petition` arrived, the card
//! is centred deterministically, and so *where* the affirmative button will
//! land is computable in advance by whoever picked the timing. A human
//! reaching for something in the app, with the pointer already resting
//! where Allow is about to be drawn, would otherwise press-and-release into
//! a grant they never read — the click-through / tapjacking class that
//! Firefox (`security.notification_enable_delay`, 500 ms before a
//! permission prompt accepts activation), Android
//! (`FILTER_TOUCHES_WHEN_OBSCURED`) and Windows UAC's secure desktop all
//! defend against.
//!
//! So a press arriving within [`GUARD_INTERVAL`] of the prompt appearing
//! **arms nothing** — it is still consumed, so it never reaches the app
//! either; it simply cannot become a decision. The human's second click,
//! after they have had time to see what they are answering, works
//! normally. This is the same guard the keyboard section below proposes for
//! a future focus-ring design, applied now to the one surface that can
//! actually grant.
//!
//! The rejected cheaper alternative is worth recording: clearing the
//! remembered pointer position at raise, so the first hit test misses until
//! a fresh motion event proves the human moved after the card appeared.
//! That is not equivalent. It admits a press that follows motion by
//! microseconds (a human mid-sweep when the prompt appears is still a human
//! who has not read it), and it punishes the honest case where the button
//! genuinely is under the cursor. A clock is the honest measure of "has a
//! human had time to look at this", so the clock is what is used.
//!
//! # The grab cannot outlive its petition
//!
//! [`ConsentGrab::raise`] refuses a petition that is not pending — but a
//! petition can die *while its prompt is up*: the consent timeout fires, or
//! the petitioner disconnects. Nothing about those paths runs inside this
//! module, and a grab that kept consuming would hold the human's physical
//! input hostage forever, with no card that answers and no way for the
//! embedder to be told. That is a permanent input lockout produced by a
//! missed call, which is not a contract this module is entitled to write.
//!
//! Two independent backstops, deliberately not one:
//!
//! - [`ConsentGrab::judge`] carries the petition's own consent deadline and
//!   **stops consuming the instant it passes**. This one does not depend on
//!   the embedder at all: the worst case is a card left painted with no
//!   grab behind it (inert pixels), never a human who cannot use their
//!   machine.
//! - [`ConsentGrab::retire_stale`] lowers a prompt whose petition has left
//!   the pending table, and [`ConsentGrab::raise`] performs the same
//!   self-heal before refusing a new petition — so an embedder that drives
//!   the queue at all recovers the pixels too, and a dead prompt can never
//!   block the next one.
//!
//! # Press arms, release commits
//!
//! A decision is taken on the *release*, and only if the pointer is still
//! inside the same button its press armed. That is not decoration:
//!
//! - It is the affordance every desktop toolkit has trained into every
//!   human — press, notice the mistake, slide off, release, nothing
//!   happens. A consent prompt is precisely the dialog where "I clicked the
//!   wrong thing" must stay recoverable, because one of the buttons hands
//!   an agent authority over the human's screen.
//! - Press-to-decide would make a stray click during the ~0 ms between the
//!   prompt appearing and the human noticing it into an irrevocable grant.
//!   The window is small; the loss is total.
//!
//! A press that lands outside every button arms nothing, and its release
//! decides nothing: clicking the card's body or the scrim is inert, which is
//! also the conventional behaviour of a modal dialog.
//!
//! # Why agent input is *not* consumed here
//!
//! This gate consumes `physical` input only. Emulated events pass through
//! untouched, which looks wrong for about ten seconds and is in fact the
//! only defensible answer:
//!
//! - **For the principal being asked about, nothing arrives anyway.** The
//!   enforcement chokepoint refuses its actuations `consent_held` while its
//!   own prompt is up ([`crate::enforcement`], step 5b), *before* the
//!   actuation is ever wrapped as an input event. The gate would consume a
//!   stream that is already empty.
//! - **For any other principal, consuming would silently overturn an
//!   authority decision.** The IDL is explicit on `consent_held`: "the
//!   principal's own pending petition has a prompt up; that principal's
//!   actuation is refused ... **other principals' grants are unaffected**."
//!   An event that reaches this gate with an emulated tag has already been
//!   admitted by the chokepoint against a live grant. Dropping it here would
//!   deny an authority the core just granted, invisibly — no refusal, no
//!   terminal, a spent rate-limit token and possibly a consumed `once` use
//!   for nothing. Silent denial is the failure mode this codebase refuses
//!   everywhere else.
//! - **The router must not grow an authority check.** [`crate::input`]'s
//!   module docs make that a rule: "by the time an event reaches it, the
//!   authority question is settled". A gate that judged emulated events by
//!   which principal was mid-prompt would be a second enforcement path in
//!   the one module documented to have none.
//!
//! What the origin check does *not* excuse is being untested. "An agent
//! cannot answer its own prompt" is the single most load-bearing sentence
//! in this module and it rests on one `if` above the match in
//! [`ConsentGrab::judge`]; a refactor that let emulated events fall through
//! to the match while still returning [`Gate::Deliver`] would keep every
//! routing property true and hand an agent the ability to click Allow with
//! the human's own cursor position. So it is pinned directly, by aiming an
//! emulated click at a real button rectangle — see
//! `an_agent_cannot_answer_the_prompt_it_petitioned_for`.
//!
//! **Known divergence, flagged for the protocol track (not fixable here).**
//! `docs/protocol/05-vitrin_consent.md` line 84 says agent actuation is
//! refused "under any grant" while a prompt is shown, which is *wider* than
//! the IDL's `consent_held` summary quoted above
//! (`protocol/vitrin-v0.xml`: "other principals' grants are unaffected").
//! Where prose and IDL disagree the IDL wins (repo CLAUDE.md), so the
//! per-principal reading is what is implemented, and the prose page needs a
//! `track:protocol` amendment to match — the same treatment the
//! whole-pixel-motion sentence got in [`crate::input`]. This is stated here
//! rather than left in a review comment because a reader who consults page
//! 05 will otherwise believe the wider guarantee holds. Reachable only with
//! two principals holding live grants; multi-agent arbitration ("physical
//! preempts agent", PRD Doc 2 §8) is Phase 2 regardless, and v0 has one
//! agent.
//!
//! # The keyboard question (decided: the prompt is pointer-only)
//!
//! Keys are consumed while a prompt is up, and **no key answers it**. Three
//! reasons, in order of weight:
//!
//! 1. **Type-through.** A prompt can appear while a human is typing at the
//!    app. If Enter (or Space, or any key) committed the affirmative choice,
//!    an already-in-flight keystroke aimed at a text field would grant an
//!    agent authority over the screen. Every keystroke arriving in the first
//!    moments of a prompt is, by construction, a keystroke aimed at
//!    something else. Real elevation prompts fight the same problem — Windows
//!    UAC moves to a secure desktop, GNOME's polkit dialog refuses to
//!    default-focus the affirmative action — and the MVP's answer is simply
//!    not to offer the surface. (The pointer faces the identical problem
//!    and cannot decline to offer the surface, which is what
//!    [`GUARD_INTERVAL`] is for.)
//! 2. **Escape is spoken for.** P1.7.3 makes hold-Esc the revocation chord.
//!    Binding Esc to Deny here would give one key two different
//!    security meanings depending on state the human cannot see, and would
//!    force P1.7.3 to carve out an exception. **Esc has no meaning in the
//!    consent grab** — deliberately, so P1.7.3 inherits it unencumbered.
//!    (It is still *consumed*, like every other key, so the app never sees
//!    it; the revocation watcher rides the non-consuming `observe` tap and
//!    is unaffected — that is exactly why that tap exists.)
//! 3. **Refusal is already reachable without a key.** A human who will not
//!    or cannot use the pointer simply does not answer, and the petition
//!    resolves `timed_out` at [`crate::petitions::PetitionConfig::consent_timeout`].
//!    The keyboard's absence therefore fails closed, never open.
//!
//! The cost is real and is not hidden: a keyboard-only human cannot approve
//! a petition. The honest fix is a focus ring with an activation key that is
//! neither Esc nor Enter, reusing [`GUARD_INTERVAL`] so type-through stays
//! impossible. That is a design task with an accessibility surface of its
//! own, deferred rather than half-built.
//!
//! # Geometry: one origin function, one view size
//!
//! Hit-testing uses [`super::render::Card::buttons`] — card-local rectangles
//! produced by the same pass that painted them — translated by the same
//! [`super::centered`] the compositor positions the card with. Renderer and
//! grab cannot disagree about where a button is, because neither computes a
//! layout the other does not.
//!
//! That leaves exactly one way for a hit test to go wrong: a stale view
//! size, which would shift the whole button row and could turn a click aimed
//! at Deny into an Allow. So the view is not cached at arm time; the embedder
//! feeds it with [`ConsentGrab::set_view`] **from the same value it passes to
//! [`crate::input::InputRouter::route`], in the same step** — see the nested
//! backend's `handle_input`, where the two sit on adjacent lines and read one
//! local. A view that could drift is a view that came from somewhere else.
//!
//! # Interaction with the replica problem (issue #85)
//!
//! A confined app can still draw a byte-identical copy of the consent card,
//! but issue #85 has since landed the trusted indicator that lets a human tell
//! the two apart — a per-session secret colour, minted at startup and painted
//! as a reserved band plus the genuine card's frame on a display path the app
//! cannot observe (see [`crate::consent::indicator`]). This module contributes
//! a *second*, independent line of defence, worth stating because it holds
//! even if a human misses the frame: **a replica gets no input grab.** Clicking
//! a real prompt's buttons produces no app-visible input at all, while clicking
//! a replica behaves like clicking an app — the app sees the press, the
//! release, and every motion between. So the app cannot both forge the pixels
//! *and* silently harvest the human's clicks the way a real prompt's grab
//! prevents; the indicator is what lets the human refuse before clicking at
//! all.
//!
//! # Wired at runtime (issue #90)
//!
//! This gate is now driven end to end. The nested backend carries it in its
//! router (so the grab is live the instant a prompt is raised) and feeds it the
//! view size on every input event, and once per dispatch round
//! [`crate::session::service_consent_round`] calls [`ConsentGrab::raise`] for
//! the front pending petition, drains the decisions this gate produced into
//! [`PetitionRegistry::resolve_human`], and lowers a card whose petition has
//! left the table. So under `--consent=interactive` a running `vitrind` really
//! does show the prompt and grab physical input while it is up. (`--headless`
//! has no display and no physical input device, so it is refused with
//! `--consent=interactive` at startup.) The mechanism below is additionally
//! exercised in isolation by this module's tests, through the real router and
//! the real wire to a mock shim.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vitrin_protocol::generated::vitrin_actuator_pointer::ButtonState;
use vitrin_protocol::generated::vitrin_consent::ConsentState;
use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

use crate::input::{Gate, PreemptionHook, SeatInput, SeatInputKind};
use crate::petitions::{PetitionId, PetitionRegistry, PromptRoute};
use crate::recorder::{Event, Recorder};

use super::render::ChoiceBox;
use super::{centered, Choice, ConsentSurface};

/// How long after a prompt appears its buttons refuse to arm (module docs:
/// the guard interval).
///
/// 500 ms is chosen to match the best-known precedent rather than invented:
/// Firefox holds its permission prompts inert for
/// `security.notification_enable_delay` = 500 ms for exactly this attack.
/// The lower bound is human: reaction time to an unexpected visual change
/// is ~250 ms *before* any reading happens, so anything shorter would
/// certify a click the human could not have evaluated. The upper bound is
/// that this delay is paid by every honest human on every prompt, and a
/// prompt that ignores a deliberate click feels broken; a full second reads
/// as a bug, half a second reads as the dialog appearing.
///
/// It is not a security boundary on its own — a human who waits and then
/// misreads the card is not helped by any timer — but it removes the entire
/// class of grants that come from a click aimed at something else.
pub(crate) const GUARD_INTERVAL: Duration = Duration::from_millis(500);

/// One decision the human took on a prompt, waiting for the embedder to
/// route it into the petition state machine
/// ([`PetitionRegistry::resolve_human`]).
///
/// Carries the petition id rather than just the choice so a decision can
/// never be applied to whatever petition happens to be pending when it is
/// drained: the registry refuses an id that is no longer pending, so a
/// decision that raced a timeout or a withdrawal fails closed instead of
/// landing on the next petition in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub petition: PetitionId,
    pub choice: Choice,
}

/// The prompt currently grabbing input: which petition it asks about, and
/// the geometry a hit test needs.
///
/// The button rectangles are **card-local**, exactly as
/// [`super::render::rasterize`] produced them; the card's origin in the view
/// is derived at hit-test time from the current view size (module docs), so
/// nothing here can hold a stale position.
#[derive(Debug)]
struct ArmedPrompt {
    petition: PetitionId,
    /// The rasterized card's size, for the centering computation.
    card: (u32, u32),
    /// Every choice and where it was drawn, card-local, in render order.
    buttons: Vec<ChoiceBox>,
    /// When this prompt went up. A press before `raised_at + `
    /// [`GUARD_INTERVAL`] arms nothing (module docs: the guard interval).
    raised_at: Instant,
    /// The petition's own consent deadline, copied from the registry at
    /// raise. Past it the grab stops consuming even if nobody lowered the
    /// prompt — the backstop against a permanent human input lockout
    /// (module docs).
    deadline: Instant,
    /// Set once this prompt's decision has been taken. The grab keeps
    /// consuming afterwards — the card is still on screen until the
    /// embedder lowers it, and input must not start reaching the app in the
    /// gap — but no second decision is ever queued for the same petition.
    decided: bool,
}

/// The consent input grab: the state [`ConsentGate`] judges against, shared
/// between the router (which owns the gate) and the embedder (which raises
/// and lowers prompts and drains decisions).
///
/// Shared by `Rc<RefCell<..>>`, the shape [`crate::input::PresenceHook`]
/// already established for hook-side state the embedder also touches.
#[derive(Debug)]
pub(crate) struct ConsentGrab {
    prompt: Option<ArmedPrompt>,
    /// The composed realm view's size — the space pointer events arrive in.
    /// Fed by the embedder from the same value it routes with (module docs).
    view: (u32, u32),
    /// Last known **physical** pointer position, in view coordinates.
    ///
    /// Deliberately not updated by emulated motion, even though v0 shares
    /// one cursor between origins: if an agent's pointer moves could
    /// relocate the position this grab hit-tests, an agent holding a
    /// pointer grant could slide the hit target under the human's finger
    /// and turn a click aimed at Deny into an Allow. The human's hit test
    /// follows the human's device and nothing else.
    pointer: Option<(f64, f64)>,
    /// The button code currently held down inside a choice, and which
    /// choice — the press that a matching release commits.
    armed: Option<(u32, Choice)>,
    /// Decisions taken and not yet drained (FIFO). Bounded in practice by
    /// one per raised prompt: `ArmedPrompt::decided` stops the second.
    decisions: VecDeque<Decision>,
}

impl ConsentGrab {
    /// An idle grab: no prompt, nothing consumed, a zero view until the
    /// embedder feeds one.
    ///
    /// The zero view is fail-closed rather than merely unset: with it, the
    /// card's derived origin is far off-screen and every hit test misses, so
    /// an embedder that raised a prompt without ever calling
    /// [`Self::set_view`] would produce an unanswerable prompt that resolves
    /// `timed_out` — refusal — never a prompt whose buttons landed
    /// somewhere unintended.
    pub fn new() -> Self {
        Self {
            prompt: None,
            view: (0, 0),
            pointer: None,
            armed: None,
            decisions: VecDeque::new(),
        }
    }

    /// Tell the grab the size of the view pointer events arrive in.
    ///
    /// MUST be called with the same `view` the embedder passes to
    /// [`crate::input::InputRouter::route`], in the same step — module docs
    /// explain why a view sourced anywhere else is a correctness hazard
    /// rather than a stale cache.
    pub fn set_view(&mut self, view: (u32, u32)) {
        self.view = view;
    }

    /// Raise `petition`'s prompt: put it on screen, mark it shown in the
    /// registry, seize physical input, and record that a human was asked —
    /// **one call**, so "visible", "input grabbed", "`consent_held` holds"
    /// and "the log says so" cannot drift apart. That was the seam
    /// [`ConsentSurface::show`] was documented to leave for this task, and
    /// this is the only place in the core that takes it.
    ///
    /// Returns where the petitioner's `vitrin_consent.state(shown)` must be
    /// sent — the caller's remaining job, because only the connection's
    /// `PrincipalServer` may speak on the wire
    /// ([`crate::principal::PrincipalServer::emit_consent_shown`]). That
    /// call is now *only* the wire: the flight-recorder entry is written
    /// here, at the moment core state actually changed, because
    /// `emit_consent_shown` can refuse for routing and liveness reasons
    /// (including the expected race where the petitioner dies as its prompt
    /// goes up) and a session where the human was asked must not be
    /// reconstructible as one where they were not.
    ///
    /// `None` — and nothing newly shown — in three cases:
    ///
    /// - **`petition` is not pending** (resolved, timed out, or withdrawn
    ///   between queueing and raising). A prompt for a petition that no
    ///   longer exists must not be renderable, and must certainly not grab a
    ///   human's input.
    /// - **A different petition's prompt is already up.** One prompt at a
    ///   time, enforced here rather than trusted to the caller: replacing an
    ///   armed prompt would leave the *previous* petition marked
    ///   `prompt_shown` in the registry with nothing on screen for it, so
    ///   `consent_held` would keep refusing that principal's actuations
    ///   for a prompt no human can see. A queue advances by
    ///   [`Self::lower`] then `raise`, which is also the only order in
    ///   which the human ever sees what they are answering.
    /// - **The renderer produced no card** — defence in depth; nothing is
    ///   left shown or grabbed.
    ///
    /// # Re-raising the same petition is a true no-op
    ///
    /// The natural embedder shape is "ensure the front-of-queue prompt is
    /// up", called every loop iteration, so this must cost nothing and
    /// change nothing. It once did neither: it re-ran `surface.show`
    /// (bumping the generation, which invalidates P1.7.1's card raster and
    /// the nested backend's window texture — a full re-upload at frame
    /// cadence) and rebuilt [`ArmedPrompt`] with `decided: false`, silently
    /// re-opening a prompt the human had already answered so a second click
    /// could queue a second decision. Now the same-petition path returns the
    /// route without touching the surface or any grab state.
    ///
    /// # Self-healing an armed prompt whose petition died
    ///
    /// If the armed petition has left the pending table (timeout,
    /// withdrawal, a resolution the embedder has not routed yet) this
    /// lowers it before doing anything else. Without that, `raise` returned
    /// `None` while leaving the dead petition's card composited and the
    /// human's input grabbed — indistinguishable, to the caller, from
    /// "someone else's prompt is up, retry later", which is exactly the
    /// reading that turns a missed `lower` into a permanent input lockout.
    pub fn raise(
        &mut self,
        petition: PetitionId,
        now: Instant,
        petitions: &mut PetitionRegistry,
        surface: &mut ConsentSurface,
        recorder: &mut Recorder,
    ) -> Option<PromptRoute> {
        if let Some(current) = self.armed_petition() {
            if petitions.pending_route(current).is_none() {
                // The armed petition died under us: take its card down and
                // release the grab before considering the new one.
                tracing::debug!(
                    %current,
                    "lowering a raised prompt whose petition is no longer pending"
                );
                self.lower(petitions, surface);
            } else if current != petition {
                tracing::warn!(
                    %current,
                    incoming = %petition,
                    "refusing to replace a raised consent prompt; lower it first"
                );
                return None;
            } else {
                // Already up, still pending: nothing to do, and nothing may
                // be touched (doc comment above).
                return petitions.pending_route(petition);
            }
        }
        // Every lookup first: nothing is shown or grabbed until the
        // petition has proven it is still pending.
        let content = petitions.prompt_content(petition)?;
        let route = petitions.pending_route(petition)?;
        let deadline = petitions.pending_deadline(petition)?;

        // Presentation before registry state, deliberately: everything up
        // to here is undoable with a `dismiss`, while the registry's
        // `prompt_shown` flag is what the enforcement chokepoint reads for
        // `consent_held`. Setting it last means no failure path can leave a
        // petition marked "a human is looking at this" with nothing on
        // screen -- a state that would refuse that principal's actuations
        // for a prompt nobody can answer, until the timeout.
        surface.show(content);
        // `show` just set the prompt, so the card rasterizes. The `else`
        // arm is defense in depth against a renderer returning nothing.
        let Some(card) = surface.card() else {
            surface.dismiss();
            return None;
        };
        let armed = ArmedPrompt {
            petition,
            card: (card.width, card.height),
            buttons: card.buttons.clone(),
            raised_at: now,
            deadline,
            decided: false,
        };
        // Checked rather than `debug_assert`ed: assertions vanish in the
        // release build CI also runs, and a grab held with no `consent_held`
        // behind it is exactly the failure that must not happen quietly.
        // Unreachable -- the petition was pending three statements ago, in
        // this same borrow.
        if !petitions.mark_prompt_shown(petition) {
            tracing::error!(%petition, "petition vanished mid-raise; no prompt shown");
            surface.dismiss();
            return None;
        }
        self.prompt = Some(armed);
        // A press held from before the prompt cannot arm one of its
        // buttons: arming requires a press this gate consumed.
        self.armed = None;
        // Last, once the state it describes is all true: the human is now
        // being asked, and the log says so whatever happens on the wire.
        recorder.record(Event::ConsentTransition {
            connection: route.connection,
            consent_wire_id: route.consent_wire_id,
            state: ConsentState::Shown,
            petition: Some(petition),
        });
        Some(route)
    }

    /// Take the prompt down and release the grab — every path that ends a
    /// prompt: a decision drained, the consent timeout, the petitioner
    /// disconnecting, or a queue advancing to the next petition.
    /// Idempotent.
    ///
    /// Takes the registry because taking a card off screen is a registry
    /// fact, not only a presentation one. The original code did not, on the
    /// argument that "a petition's `prompt_shown` flag dies with its
    /// pending entry, so `consent_held` stops holding at the same moment
    /// the petition leaves the table" — true when `lower` follows a
    /// resolution, and false on the other path this module itself
    /// prescribes. A queue advances by `lower` then `raise`, and the
    /// lowered petition is *still pending*: it went back to `queued`,
    /// nothing on screen, no input grab. Leaving it marked `shown` made
    /// `prompt_up_for` keep answering `true`, so the enforcement
    /// chokepoint refused that principal's every actuation `consent_held`
    /// — citing a prompt no human could see — until the consent timeout.
    /// [`PetitionRegistry::mark_prompt_hidden`] is a no-op for a petition
    /// that already left the table, so the resolution path is unaffected.
    ///
    /// Queued decisions for the prompt being lowered are dropped, for the
    /// same reason the in-flight press is: both are answers to a card that
    /// is no longer on screen. Keeping them was an unexplained asymmetry
    /// with a real edge — on the queue-advance path the petition is still
    /// pending, so a decision drained long afterwards would resolve it with
    /// no prompt visible anywhere. Losing an undrained decision instead
    /// costs at worst a timeout, which is refusal: the fail-closed
    /// direction. Embedders should still drain before lowering; this is the
    /// backstop, not the contract.
    pub fn lower(&mut self, petitions: &mut PetitionRegistry, surface: &mut ConsentSurface) {
        surface.dismiss();
        if let Some(prompt) = self.prompt.take() {
            petitions.mark_prompt_hidden(prompt.petition);
            self.decisions.retain(|d| d.petition != prompt.petition);
        }
        self.armed = None;
    }

    /// Lower the prompt if its petition has left the pending table, and
    /// report whether it did.
    ///
    /// The embedder's per-iteration hygiene call, next to
    /// [`PetitionRegistry::expire_due`]: it turns "remember to lower on
    /// every one of the three removal paths" into "ask once per turn". Note
    /// that [`Self::judge`] already refuses to hold the grab past the
    /// petition's deadline without any help — this is what takes the
    /// *pixels* down, and what frees the queue for the next petition.
    pub fn retire_stale(
        &mut self,
        petitions: &mut PetitionRegistry,
        surface: &mut ConsentSurface,
    ) -> bool {
        let Some(current) = self.armed_petition() else {
            return false;
        };
        if petitions.pending_route(current).is_some() {
            return false;
        }
        self.lower(petitions, surface);
        true
    }

    /// The petition whose prompt currently holds the grab, if any.
    pub fn armed_petition(&self) -> Option<PetitionId> {
        self.prompt.as_ref().map(|p| p.petition)
    }

    /// Drain the next decision the human took, oldest first.
    pub fn take_decision(&mut self) -> Option<Decision> {
        self.decisions.pop_front()
    }

    /// **Test seam.** Push a decision into the queue as if a physical click
    /// had produced it.
    ///
    /// The click-to-decision path — hit test, guard interval, press-arms /
    /// release-commits, the origin check that stops an agent answering its own
    /// prompt — is exhaustively covered by this module's own tests, which
    /// drive [`Self::judge_parts`] with real events. This seam exists for the
    /// *session*-level orchestration instead: it lets
    /// [`crate::session::service_consent_round`]'s drive
    /// (drain → `resolve_human` → `deliver` → lower → raise-next) be exercised
    /// end to end over the wire without a display or an input device, which no
    /// runner in CI has.
    ///
    /// # Why the `consent-injector` build shares it (issue #138)
    ///
    /// The out-of-process gate (`tests/integration/test_real_consent.py`)
    /// needs the same thing one process boundary further out: a real
    /// `vitrind` raising a real prompt over a real app, answered by something
    /// that is not a mouse. Its socket source
    /// ([`crate::backend::headless`]) lands here and nowhere else, so the
    /// injected answer is drained, validated and applied by the *production*
    /// path — [`crate::session::service_consent_round`] →
    /// [`PetitionRegistry::resolve_human`] — rather than by a second
    /// decision path of its own. **Enqueuing is deliberately all this does:**
    /// it confers nothing, decides nothing, and a decision naming a petition
    /// that has since timed out or been withdrawn is refused `NotPending` at
    /// the registry exactly as a slow human's click is (see [`Decision`]).
    #[cfg(any(test, feature = "consent-injector"))]
    pub(crate) fn queue_decision(&mut self, decision: Decision) {
        self.decisions.push_back(decision);
    }

    /// Judge one intake event: the whole grab policy, in one place.
    ///
    /// Takes the whole [`SeatInput`] rather than `(origin, kind)`, and that
    /// is the point: [`crate::input::SeatInput::physical`] is private to
    /// `crate::input` so that intake is the only place a physical origin
    /// tag can be minted, and a `pub fn judge(origin, kind)` on a type the
    /// backend holds an `Rc` to handed that tag back to anyone — letting
    /// any module in the core drive a consent decision without going
    /// through intake at all. The trust boundary this module exists to
    /// enforce should be held by visibility, not by a comment saying who
    /// the only caller is. This module's own tests still model a physical
    /// event by calling the *private* [`Self::judge_parts`] directly, which
    /// is the accommodation [`crate::input::PhysicalPresence::note`] makes
    /// too — the difference is that it is no longer part of the surface any
    /// other module can reach.
    ///
    /// `now` is the dispatch turn's injected instant ([`crate::grants`]'
    /// clock discipline), carried in by [`ConsentGate`]'s shared clock cell
    /// because the hook trait deliberately holds no clock. Two policies
    /// need it: the guard interval, and the deadline backstop.
    pub fn judge(&mut self, input: &SeatInput, now: Instant) -> Gate {
        self.judge_parts(input.origin(), input.kind(), now)
    }

    /// [`Self::judge`]'s body, split out so this module's tests can supply
    /// an origin tag directly. Private: outside `crate::consent::grab` the
    /// only way in is [`Self::judge`], which demands a [`SeatInput`] that
    /// only intake can have tagged physical.
    fn judge_parts(&mut self, origin: Origin, kind: &SeatInputKind, now: Instant) -> Gate {
        // Where the human's pointer is, recorded before any grab decision
        // and whether or not a prompt is up: it is a physical fact, and a
        // prompt raised long after the last motion must still know where
        // the cursor sits. (The router keeps its own copy for its own
        // hit-testing, for the same reason and with the same wording.)
        if origin == Origin::Physical {
            if let SeatInputKind::Motion { x, y } = kind {
                self.pointer = Some((*x, *y));
            }
        }

        // Copied out rather than held by reference: everything below may
        // need `&mut self` (arming, committing), and the three facts a
        // judgement needs from the prompt are three `Copy` scalars.
        let Some((petition, raised_at, deadline)) = self
            .prompt
            .as_ref()
            .map(|p| (p.petition, p.raised_at, p.deadline))
        else {
            return Gate::Deliver;
        };
        // The deadline backstop (module docs). Past its own consent
        // timeout the petition is dead or about to be, and no missed
        // `lower` may cost the human the use of their machine: the grab
        // stops consuming and stops deciding. `self.prompt` is deliberately
        // NOT cleared -- `lower` still owes the registry a
        // `mark_prompt_hidden` for it, and `retire_stale` still owes the
        // screen a `dismiss`; forgetting which petition is up here would
        // strand both.
        if now >= deadline {
            self.armed = None;
            return Gate::Deliver;
        }

        // Agent input is the chokepoint's business, not the router's
        // (module docs: consuming it here would silently overturn an
        // authority decision the core just made). THIS CHECK IS THE WHOLE
        // "an agent cannot answer its own prompt" property -- it must stay
        // above the match, and a refactor that lets emulated events fall
        // through while still returning `Deliver` is a grant an agent
        // writes for itself.
        if origin != Origin::Physical {
            return Gate::Deliver;
        }

        match kind {
            SeatInputKind::Button {
                button,
                state: ButtonState::Pressed,
            } => {
                // The guard interval (module docs): a press that arrives
                // before the human could have read the card arms nothing.
                // Still consumed -- it does not reach the app either.
                self.armed = if now.saturating_duration_since(raised_at) < GUARD_INTERVAL {
                    tracing::debug!(
                        %petition,
                        "press ignored inside the consent prompt's guard interval"
                    );
                    None
                } else {
                    self.hit_test().map(|choice| (*button, choice))
                };
                Gate::Consume
            }
            SeatInputKind::Button {
                button,
                state: ButtonState::Released,
            } => {
                self.commit(*button);
                // Hold-until-release (module docs): always delivered, and
                // safe because the router drops any release whose press it
                // did not deliver.
                Gate::Deliver
            }
            // Key releases join button releases in the hold-until-release
            // exception: the app may be holding this key from before the
            // prompt, and the router's per-keysym pairing drops the release
            // if it is not. Consuming it would latch a modifier in a
            // confined app for as long as the human keeps using it.
            SeatInputKind::Key {
                state: KeyState::Released,
                ..
            } => Gate::Deliver,
            // Motion, scroll, key presses — and text, which physical intake
            // never produces but which would be human-aimed if it ever did.
            // Exhaustive by intent: a new input kind must be classified
            // here rather than defaulting to reaching the app mid-prompt.
            SeatInputKind::Motion { .. }
            | SeatInputKind::Scroll { .. }
            | SeatInputKind::Key { .. }
            | SeatInputKind::Text { .. } => Gate::Consume,
        }
    }

    /// Commit the armed decision if `button`'s release still lands on the
    /// choice its press armed. Any mismatch — a different button code, the
    /// pointer slid off, nothing armed at all — disarms without deciding,
    /// which is the "slide off to cancel" affordance the module docs
    /// justify.
    fn commit(&mut self, button: u32) {
        let Some((armed_button, armed_choice)) = self.armed.take() else {
            return;
        };
        if armed_button != button || self.hit_test() != Some(armed_choice) {
            return;
        }
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        if prompt.decided {
            return;
        }
        prompt.decided = true;
        self.decisions.push_back(Decision {
            petition: prompt.petition,
            choice: armed_choice,
        });
    }

    /// Which choice the human's pointer is over, if any: view coordinates →
    /// card-local through the same centering the compositor draws with,
    /// then the renderer's own rectangles.
    fn hit_test(&self) -> Option<Choice> {
        let prompt = self.prompt.as_ref()?;
        let (px, py) = self.pointer?;
        let (ox, oy) = centered(prompt.card.0, prompt.card.1, self.view.0, self.view.1);
        let cx = card_local(px, ox)?;
        let cy = card_local(py, oy)?;
        prompt
            .buttons
            .iter()
            .find(|b| b.rect.contains(cx, cy))
            .map(|b| b.choice)
    }
}

/// One axis of view → card-local: floor the (possibly fractional, HiDPI)
/// pointer coordinate and subtract the card's origin.
///
/// `None` for a non-finite coordinate. `f64 as i32` saturates in Rust, but
/// maps NaN to `0` — which would place a garbage pointer at the card's
/// top-left corner and could *hit a button*. A consent decision must never
/// come out of arithmetic nobody meant, so it is rejected explicitly.
/// Saturation at the i32 bounds is fine and deliberate: a coordinate that
/// far out hits nothing.
fn card_local(view_coord: f64, origin: i32) -> Option<i32> {
    if !view_coord.is_finite() {
        return None;
    }
    Some((view_coord.floor() as i32).saturating_sub(origin))
}

/// The consent grab attached at the router's preemption hook point: the
/// consuming half ([`PreemptionHook::gate`]), wrapping an inner hook so the
/// other P1.7.x consumer stacks beside it.
///
/// Precedence is deliberate and documented: when the grab consumes, the
/// inner gate is **not** consulted. A prompt on screen is the highest-
/// priority preemption in the core — there is no policy that could
/// legitimately overrule "the human is deciding right now" and hand the
/// event to the app after all. The non-consuming `observe` tap is passed
/// through unconditionally in every case, which is what keeps P1.7.3's
/// revocation watcher working *while* a prompt grabs input.
pub(crate) struct ConsentGate<H: PreemptionHook> {
    grab: Rc<RefCell<ConsentGrab>>,
    /// The dispatch turn's instant, shared with the embedder that drives
    /// [`crate::input::InputRouter::route`] — the shape
    /// [`crate::input::PresenceHook`] already established for exactly this
    /// problem: the hook trait carries no clock (the router never reads
    /// one), and the grab needs one for its guard interval and its deadline
    /// backstop. The embedder advances this cell to the same single-sample
    /// `now` the rest of the turn uses.
    now: Rc<Cell<Instant>>,
    inner: H,
}

impl<H: PreemptionHook> ConsentGate<H> {
    pub fn new(grab: Rc<RefCell<ConsentGrab>>, now: Rc<Cell<Instant>>, inner: H) -> Self {
        Self { grab, now, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for ConsentGate<H> {
    fn observe(&mut self, input: &SeatInput) {
        // Never consuming, never conditional: the tap must see the raw
        // event stream even while this gate is swallowing all of it.
        // P1.7.3's revocation watcher rides this, and a prompt on screen is
        // precisely when revocation matters most.
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        match self.grab.borrow_mut().judge(input, self.now.get()) {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use vitrin_protocol::generated::vitrin_actuator_pointer::Axis;
    use vitrin_protocol::generated::vitrin_grant::{Persistence as WirePersistence, Verb};

    use super::*;
    use crate::consent::tests::PROMPT_IDENTITY;
    use crate::grants::PersistenceRung;
    use crate::identity::PrincipalIdentity;
    use crate::petitions::{
        Admission, ConsentPolicy, PetitionConfig, PetitionRegistry, PetitionRequest,
    };
    use crate::recorder::tests::scratch_recorder;

    const VIEW: (u32, u32) = (900, 700);
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;

    /// A recorder whose log this test does not read, for the many cases
    /// where `raise`'s own bookkeeping is not what is under test. The file
    /// and its directory are removed when the guard drops, so a suite that
    /// raises hundreds of prompts leaves nothing behind.
    struct Scratch {
        recorder: Recorder,
        path: std::path::PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let (recorder, path) = scratch_recorder("consent-grab");
            Self { recorder, path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            crate::recorder::tests::cleanup(&self.path);
        }
    }

    /// The instant every fixture prompt is raised at. Tests name times
    /// relative to it rather than sampling a clock, so the guard interval
    /// and the consent deadline are exercised deterministically instead of
    /// depending on how fast CI happens to be.
    fn t0() -> Instant {
        Instant::now()
    }

    /// A moment past the guard interval: when a click first counts.
    fn awake(t0: Instant) -> Instant {
        t0 + GUARD_INTERVAL
    }

    fn motion(x: f64, y: f64) -> SeatInputKind {
        SeatInputKind::Motion { x, y }
    }

    fn press(button: u32) -> SeatInputKind {
        SeatInputKind::Button {
            button,
            state: ButtonState::Pressed,
        }
    }

    fn release(button: u32) -> SeatInputKind {
        SeatInputKind::Button {
            button,
            state: ButtonState::Released,
        }
    }

    fn key(keysym: u32, state: KeyState) -> SeatInputKind {
        SeatInputKind::Key { keysym, state }
    }

    /// A grab with the fixture prompt up, shown at [`VIEW`], plus the
    /// registry the petition lives in, the surface it is drawn on, and the
    /// instant it was raised at.
    fn armed() -> (
        ConsentGrab,
        ConsentSurface,
        PetitionRegistry,
        PetitionId,
        Instant,
    ) {
        let (mut registry, petition) = pending_petition(WirePersistence::WhileRunning);
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        let mut grab = ConsentGrab::new();
        let t0 = t0();
        grab.set_view(VIEW);
        grab.raise(
            petition,
            t0,
            &mut registry,
            &mut surface,
            &mut Scratch::new().recorder,
        )
        .expect("the petition is pending");
        (grab, surface, registry, petition, t0)
    }

    /// One pending petition in a fresh interactive registry.
    fn pending_petition(persistence: WirePersistence) -> (PetitionRegistry, PetitionId) {
        let mut registry =
            PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let connection = registry.register_connection();
        let realms = crate::realm::tests::registry_with(&["realm-0"]);
        let request = PetitionRequest {
            connection,
            identity: PrincipalIdentity::parse(PROMPT_IDENTITY).expect("fixture identity"),
            realm_name: "realm-0".into(),
            grant_wire_id: 10,
            consent_wire_id: 11,
            resource: String::new(),
            verbs: Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT,
            expiry_ms: 60_000,
            max_event_rate: 0,
            persistence,
            flags: 0,
        };
        let Admission::Pending { petition } =
            registry.admit(request, std::time::Instant::now(), &realms)
        else {
            panic!("an interactive petition must pend");
        };
        (registry, petition)
    }

    /// The center of the button offering `choice`, in view coordinates.
    fn center_of(grab: &ConsentGrab, choice: Choice) -> (f64, f64) {
        let prompt = grab.prompt.as_ref().expect("a prompt is up");
        let (ox, oy) = centered(prompt.card.0, prompt.card.1, VIEW.0, VIEW.1);
        let button = prompt
            .buttons
            .iter()
            .find(|b| b.choice == choice)
            .unwrap_or_else(|| panic!("the prompt offers {choice:?}"));
        (
            f64::from(ox + button.rect.x) + f64::from(button.rect.w) / 2.0,
            f64::from(oy + button.rect.y) + f64::from(button.rect.h) / 2.0,
        )
    }

    /// Click (press then release) at `(x, y)` with BTN_LEFT, at `now`.
    fn click(grab: &mut ConsentGrab, (x, y): (f64, f64), now: Instant) {
        grab.judge_parts(Origin::Physical, &motion(x, y), now);
        grab.judge_parts(Origin::Physical, &press(BTN_LEFT), now);
        grab.judge_parts(Origin::Physical, &release(BTN_LEFT), now);
    }

    #[test]
    fn an_idle_grab_consumes_nothing() {
        let mut grab = ConsentGrab::new();
        let now = t0();
        grab.set_view(VIEW);
        for kind in [
            motion(10.0, 10.0),
            press(BTN_LEFT),
            release(BTN_LEFT),
            SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: -120,
            },
            key(0xff1b, KeyState::Pressed),
        ] {
            assert_eq!(
                grab.judge_parts(Origin::Physical, &kind, now),
                Gate::Deliver
            );
        }
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn a_raised_prompt_consumes_every_physical_event_except_releases() {
        // "All physical input now routes exclusively to it" -- read
        // literally: motion (a hover side channel), scroll, and key
        // presses are all stopped, not just clicks. Releases -- button and
        // key alike -- are the one documented exception, and the router's
        // pairing is what makes that safe.
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let now = awake(t0);
        for kind in [
            motion(10.0, 10.0),
            press(BTN_LEFT),
            SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: -120,
            },
            key(0xff0d, KeyState::Pressed),
            key(0xff1b, KeyState::Pressed), // Escape: consumed, decides nothing
            SeatInputKind::Text {
                text: "typed".into(),
            },
        ] {
            assert_eq!(
                grab.judge_parts(Origin::Physical, &kind, now),
                Gate::Consume,
                "{kind:?} must not reach the app while a prompt is up"
            );
        }
        for kind in [release(BTN_LEFT), key(0xffe1, KeyState::Released)] {
            assert_eq!(
                grab.judge_parts(Origin::Physical, &kind, now),
                Gate::Deliver,
                "hold-until-release: {kind:?} is paired and dropped by the router"
            );
        }
        assert!(
            grab.take_decision().is_none(),
            "no key and no stray click may decide a petition"
        );
    }

    #[test]
    fn a_key_release_is_never_consumed_so_no_modifier_latches_in_the_app() {
        // The keyboard half of hold-until-release. A prompt can appear
        // while the human holds Shift or Ctrl; consuming that key's release
        // would leave the confined app believing the modifier is still
        // down, silently rewriting every keystroke the human types
        // afterwards. The router pairs keys per keysym, so delivering the
        // release is safe: one whose press the grab consumed is dropped
        // there (proved through the real router in `crate::input`).
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let now = awake(t0);
        for keysym in [
            0xffe1, // Shift_L
            0xffe3, // Control_L
            0xffe9, // Alt_L
            0xff1b, // Escape -- P1.7.3's chord, and still not consumed on release
            0x0020, // space
        ] {
            assert_eq!(
                grab.judge_parts(Origin::Physical, &key(keysym, KeyState::Pressed), now),
                Gate::Consume,
                "a key PRESS during a prompt is aimed at something else"
            );
            assert_eq!(
                grab.judge_parts(Origin::Physical, &key(keysym, KeyState::Released), now),
                Gate::Deliver,
                "a key RELEASE must never be consumed: keysym {keysym:#x} would latch"
            );
        }
        assert!(grab.take_decision().is_none(), "no key answers the prompt");
    }

    #[test]
    fn no_key_answers_the_prompt() {
        // The settled keyboard decision, pinned so a later "just bind
        // Enter to the first button" cannot land quietly. Escape is
        // singled out because P1.7.3 claims it as the revocation chord:
        // it must mean nothing here.
        let (mut grab, _surface, _registry, petition, t0) = armed();
        let now = awake(t0);
        for keysym in [
            0xff0d, // Return
            0xff1b, // Escape -- reserved for P1.7.3's hold-Esc revocation
            0x0020, // space
            0xff09, // Tab
        ] {
            for state in [KeyState::Pressed, KeyState::Released] {
                grab.judge_parts(Origin::Physical, &key(keysym, state), now);
            }
        }
        assert!(grab.take_decision().is_none());
        assert_eq!(grab.armed_petition(), Some(petition));
    }

    #[test]
    fn clicking_allow_and_deny_yields_exactly_that_choice() {
        for choice in [
            Choice::Allow(PersistenceRung::WhileRunning),
            Choice::Allow(PersistenceRung::Once),
            Choice::Deny,
        ] {
            let (mut grab, _surface, _registry, petition, t0) = armed();
            let target = center_of(&grab, choice);
            click(&mut grab, target, awake(t0));
            assert_eq!(
                grab.take_decision(),
                Some(Decision { petition, choice }),
                "a click on {choice:?} must decide {choice:?}"
            );
            assert!(grab.take_decision().is_none(), "exactly one decision");
        }
    }

    #[test]
    fn a_click_inside_the_guard_interval_decides_nothing() {
        // The tapjacking / click-through defence (module docs). The agent
        // chooses when the prompt appears and the card is centred
        // deterministically, so a human whose pointer already rests where
        // Allow will be drawn must not grant by completing a click they
        // aimed at the application.
        for choice in [Choice::Allow(PersistenceRung::WhileRunning), Choice::Deny] {
            let (mut grab, _surface, _registry, petition, t0) = armed();
            let target = center_of(&grab, choice);

            // The instant the prompt appears: inert.
            click(&mut grab, target, t0);
            assert!(
                grab.take_decision().is_none(),
                "a click landing with the prompt must not decide {choice:?}"
            );
            // Still inside the interval: still inert.
            click(
                &mut grab,
                target,
                t0 + GUARD_INTERVAL - Duration::from_millis(1),
            );
            assert!(grab.take_decision().is_none(), "the guard is half-open");

            // ...and the prompt is fully answerable once the human has had
            // time to read it. The guard delays, it never disables.
            click(&mut grab, target, awake(t0));
            assert_eq!(grab.take_decision(), Some(Decision { petition, choice }));
        }
    }

    #[test]
    fn the_guard_interval_is_measured_from_the_press_not_the_release() {
        // A press inside the guard arms nothing, so its release -- however
        // late -- commits nothing. Otherwise an in-flight click could be
        // "completed" by simply holding the button past the interval,
        // which is the same grant with an extra step.
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));

        grab.judge_parts(Origin::Physical, &motion(allow.0, allow.1), t0);
        grab.judge_parts(Origin::Physical, &press(BTN_LEFT), t0);
        grab.judge_parts(
            Origin::Physical,
            &release(BTN_LEFT),
            t0 + Duration::from_secs(5),
        );
        assert!(
            grab.take_decision().is_none(),
            "a press the guard rejected cannot be committed by a late release"
        );
    }

    #[test]
    fn an_agent_cannot_answer_the_prompt_it_petitioned_for() {
        // THE trust property of this module, stated directly: an emulated
        // click aimed at a real button rectangle, with the human's own
        // pointer resting on that button (v0 shares one cursor), decides
        // nothing. Distinct from `agent_input_passes_the_grab_untouched`,
        // which asserts routing at coordinates that are scrim and would
        // pass even with the origin check deleted.
        for choice in [Choice::Allow(PersistenceRung::WhileRunning), Choice::Deny] {
            let (mut grab, _surface, _registry, _petition, t0) = armed();
            let now = awake(t0);
            let target = center_of(&grab, choice);

            // The human is looking at the button, cursor on it -- the most
            // favourable position an agent could borrow.
            grab.judge_parts(Origin::Physical, &motion(target.0, target.1), now);
            assert_eq!(
                grab.hit_test(),
                Some(choice),
                "fixture check: the pointer really is on {choice:?}"
            );

            // The agent clicks it.
            assert_eq!(
                grab.judge_parts(Origin::Emulated, &press(BTN_LEFT), now),
                Gate::Deliver
            );
            assert_eq!(
                grab.judge_parts(Origin::Emulated, &release(BTN_LEFT), now),
                Gate::Deliver
            );
            assert!(
                grab.take_decision().is_none(),
                "an agent must never be able to answer a consent prompt ({choice:?})"
            );

            // And the human's own click on the same pixel still works, so
            // this is a rejection of the *origin*, not of the geometry.
            click(&mut grab, target, now);
            assert_eq!(grab.take_decision().map(|d| d.choice), Some(choice));
        }
    }

    #[test]
    fn a_press_that_slides_off_its_button_decides_nothing() {
        // The recoverable-misclick affordance (module docs): press on
        // Allow, notice, slide onto Deny, release -- and nothing is
        // decided, because the release did not land on the button the
        // press armed. Neither choice fires: not the one armed, not the
        // one under the cursor.
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let now = awake(t0);
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));
        let deny = center_of(&grab, Choice::Deny);

        grab.judge_parts(Origin::Physical, &motion(allow.0, allow.1), now);
        grab.judge_parts(Origin::Physical, &press(BTN_LEFT), now);
        grab.judge_parts(Origin::Physical, &motion(deny.0, deny.1), now);
        grab.judge_parts(Origin::Physical, &release(BTN_LEFT), now);
        assert!(grab.take_decision().is_none());

        // Sliding off the card entirely is the same story.
        grab.judge_parts(Origin::Physical, &motion(allow.0, allow.1), now);
        grab.judge_parts(Origin::Physical, &press(BTN_LEFT), now);
        grab.judge_parts(Origin::Physical, &motion(1.0, 1.0), now);
        grab.judge_parts(Origin::Physical, &release(BTN_LEFT), now);
        assert!(grab.take_decision().is_none());

        // ...and the prompt is still answerable afterwards.
        click(&mut grab, deny, now);
        assert_eq!(
            grab.take_decision().map(|d| d.choice),
            Some(Choice::Deny),
            "a cancelled click must not disable the prompt"
        );
    }

    #[test]
    fn a_release_only_commits_the_button_code_that_armed_it() {
        // Physical devices interleave codes; a right-button release must
        // not commit what the left button armed.
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let now = awake(t0);
        let deny = center_of(&grab, Choice::Deny);
        grab.judge_parts(Origin::Physical, &motion(deny.0, deny.1), now);
        grab.judge_parts(Origin::Physical, &press(BTN_LEFT), now);
        grab.judge_parts(Origin::Physical, &release(BTN_RIGHT), now);
        assert!(grab.take_decision().is_none());
        // And the mismatched release disarmed, so BTN_LEFT's own release
        // (the human lifting the finger they pressed with) decides nothing
        // either -- fail-closed, never a decision from a confused pair.
        grab.judge_parts(Origin::Physical, &release(BTN_LEFT), now);
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn clicking_the_card_body_or_the_scrim_decides_nothing() {
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let now = awake(t0);
        // The scrim, far from the card.
        click(&mut grab, (2.0, 2.0), now);
        // The card's own title area: inside the card, outside every button.
        let (ox, oy) = {
            let prompt = grab.prompt.as_ref().unwrap();
            centered(prompt.card.0, prompt.card.1, VIEW.0, VIEW.1)
        };
        click(&mut grab, (f64::from(ox) + 20.0, f64::from(oy) + 20.0), now);
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn a_second_click_cannot_decide_the_same_petition_twice() {
        // The grab keeps consuming after a decision (the card is still on
        // screen until the embedder lowers it, and input must not start
        // reaching the app in that gap), but never queues a second
        // decision -- the exactly-once property, held on this side too.
        let (mut grab, mut surface, mut registry, petition, t0) = armed();
        let now = awake(t0);
        let deny = center_of(&grab, Choice::Deny);
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));
        click(&mut grab, deny, now);
        click(&mut grab, allow, now);
        assert_eq!(
            grab.take_decision(),
            Some(Decision {
                petition,
                choice: Choice::Deny
            })
        );
        assert!(grab.take_decision().is_none());
        assert_eq!(
            grab.judge_parts(Origin::Physical, &motion(5.0, 5.0), now),
            Gate::Consume,
            "input stays grabbed until the prompt is lowered"
        );

        grab.lower(&mut registry, &mut surface);
        assert_eq!(
            grab.judge_parts(Origin::Physical, &motion(5.0, 5.0), now),
            Gate::Deliver
        );
        assert!(grab.armed_petition().is_none());
        assert!(surface.prompt().is_none());
    }

    #[test]
    fn agent_input_passes_the_grab_untouched() {
        // The chokepoint owns agent actuation (`consent_held`); the router
        // must not grow a second authority check. Module docs carry the
        // full argument, including why silently dropping an admitted
        // actuation would be worse than letting it through. That an agent
        // cannot *decide* with those events is
        // `an_agent_cannot_answer_the_prompt_it_petitioned_for`; this is
        // only the routing half.
        let (mut grab, _surface, _registry, _petition, t0) = armed();
        let now = awake(t0);
        for kind in [
            motion(10.0, 10.0),
            press(BTN_LEFT),
            release(BTN_LEFT),
            SeatInputKind::Text {
                text: "agent text".into(),
            },
        ] {
            assert_eq!(
                grab.judge_parts(Origin::Emulated, &kind, now),
                Gate::Deliver
            );
        }
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn an_agent_cannot_move_the_hit_test_under_the_humans_click() {
        // v0 shares one cursor between origins, so an agent with a pointer
        // grant on a *different* petition can move it. That must not
        // relocate what the human's next click hits: the grab follows the
        // human's device only.
        let (mut grab, _surface, _registry, petition, t0) = armed();
        let now = awake(t0);
        let deny = center_of(&grab, Choice::Deny);
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));

        grab.judge_parts(Origin::Physical, &motion(deny.0, deny.1), now);
        // The agent slides the shared cursor onto Allow...
        grab.judge_parts(Origin::Emulated, &motion(allow.0, allow.1), now);
        // ...and the human clicks where they were looking.
        grab.judge_parts(Origin::Physical, &press(BTN_LEFT), now);
        grab.judge_parts(Origin::Physical, &release(BTN_LEFT), now);
        assert_eq!(
            grab.take_decision(),
            Some(Decision {
                petition,
                choice: Choice::Deny
            }),
            "the human's pointer decides, never the agent's"
        );
    }

    #[test]
    fn a_petition_that_stopped_pending_cannot_be_raised() {
        // Timed out, withdrawn, or already resolved between queueing and
        // raising: nothing is shown and nothing is grabbed. A prompt for a
        // dead petition must not hold a human's input hostage.
        let (mut registry, petition) = pending_petition(WirePersistence::WhileRunning);
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        let mut grab = ConsentGrab::new();
        let t0 = t0();
        grab.set_view(VIEW);
        let expired =
            registry.expire_due(std::time::Instant::now() + std::time::Duration::from_secs(3600));
        assert_eq!(expired.len(), 1);

        assert!(grab
            .raise(
                petition,
                t0,
                &mut registry,
                &mut surface,
                &mut Scratch::new().recorder
            )
            .is_none());
        assert!(grab.armed_petition().is_none());
        assert!(surface.prompt().is_none());
        assert_eq!(
            grab.judge_parts(Origin::Physical, &motion(1.0, 1.0), t0),
            Gate::Deliver
        );
    }

    #[test]
    fn a_raised_prompt_is_not_replaced_by_another_petitions() {
        // One prompt at a time. Replacing would leave the *first* petition
        // marked `prompt_shown` with nothing on screen for it, so
        // `consent_held` would keep refusing that principal until the
        // timeout for a prompt no human can see. A queue advances by
        // lowering first -- and lowering is what clears the flag.
        //
        // Both petitions carry the SAME identity, deliberately: with two
        // identities the `prompt_up_for` assertions below are vacuous, and
        // vacuous is how the missing `mark_prompt_hidden` survived review.
        let (mut registry, first) = pending_petition(WirePersistence::WhileRunning);
        let identity = PrincipalIdentity::parse(PROMPT_IDENTITY).unwrap();
        let realms = crate::realm::tests::registry_with(&["realm-0"]);
        let connection = registry.register_connection();
        let Admission::Pending { petition: second } = registry.admit(
            PetitionRequest {
                connection,
                identity: identity.clone(),
                realm_name: "realm-0".into(),
                grant_wire_id: 20,
                consent_wire_id: 21,
                resource: String::new(),
                verbs: Verb::OBSERVE,
                expiry_ms: 0,
                max_event_rate: 0,
                persistence: WirePersistence::WhileRunning,
                flags: 0,
            },
            std::time::Instant::now(),
            &realms,
        ) else {
            panic!("an interactive petition must pend");
        };

        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        let mut grab = ConsentGrab::new();
        let t0 = t0();
        grab.set_view(VIEW);
        grab.raise(
            first,
            t0,
            &mut registry,
            &mut surface,
            &mut Scratch::new().recorder,
        )
        .expect("first");
        assert!(
            grab.raise(
                second,
                t0,
                &mut registry,
                &mut surface,
                &mut Scratch::new().recorder
            )
            .is_none(),
            "a second petition must not displace a raised prompt"
        );
        assert_eq!(grab.armed_petition(), Some(first));
        assert!(registry.prompt_up_for(&identity));

        // Re-raising the SAME petition is idempotent, not a refusal.
        assert!(grab
            .raise(
                first,
                t0,
                &mut registry,
                &mut surface,
                &mut Scratch::new().recorder
            )
            .is_some());

        // Lower, then raise: the queue advances -- and the lowered
        // petition stops holding `consent_held` the moment its card leaves
        // the screen, even though it is still pending.
        grab.lower(&mut registry, &mut surface);
        assert!(
            !registry.prompt_up_for(&identity),
            "a lowered prompt must not keep refusing its principal `consent_held`"
        );
        assert!(
            registry.prompt_content(first).is_some(),
            "lowering must not resolve the petition -- it goes back to queued"
        );
        assert!(grab
            .raise(
                second,
                t0,
                &mut registry,
                &mut surface,
                &mut Scratch::new().recorder
            )
            .is_some());
        assert_eq!(grab.armed_petition(), Some(second));
        assert!(registry.prompt_up_for(&identity), "the new prompt holds");
    }

    #[test]
    fn re_raising_the_same_petition_changes_nothing_at_all() {
        // "Ensure the front-of-queue prompt is up", called every loop
        // iteration, is the shape the doc blesses -- so it must be free
        // and inert. It was neither: it re-rasterized the card (bumping
        // the surface generation, which invalidates the nested backend's
        // whole window texture at frame cadence) and reset the `decided`
        // latch, so a human who clicked a second time on a card that had
        // not visibly changed queued a SECOND decision for one petition.
        let (mut grab, mut surface, mut registry, petition, t0) = armed();
        let now = awake(t0);
        let deny = center_of(&grab, Choice::Deny);
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));
        let generation = surface.generation();

        click(&mut grab, deny, now);

        // The embedder's idempotent "keep it up" call, a hundred times.
        for _ in 0..100 {
            assert!(grab
                .raise(
                    petition,
                    now,
                    &mut registry,
                    &mut surface,
                    &mut Scratch::new().recorder
                )
                .is_some());
        }
        assert_eq!(
            surface.generation(),
            generation,
            "a no-op raise must not invalidate the card raster"
        );

        // The human, seeing nothing happen, clicks the other button.
        click(&mut grab, allow, now);
        assert_eq!(
            grab.take_decision(),
            Some(Decision {
                petition,
                choice: Choice::Deny
            })
        );
        assert!(
            grab.take_decision().is_none(),
            "exactly one decision per petition, across any number of raises"
        );
    }

    #[test]
    fn a_grab_never_outlives_its_petitions_deadline() {
        // The backstop against a permanent human input lockout (module
        // docs). A petition can die while its prompt is up -- the consent
        // timeout is the ordinary case -- and every path that removes it
        // lives outside this module. Past the deadline the grab lets go on
        // its own, without being told and without a registry borrow.
        let (mut grab, mut surface, mut registry, petition, _t0) = armed();
        let deadline = registry
            .pending_deadline(petition)
            .expect("the petition is pending");

        // Just inside: still grabbed, still answerable.
        assert_eq!(
            grab.judge_parts(
                Origin::Physical,
                &motion(5.0, 5.0),
                deadline - Duration::from_millis(1)
            ),
            Gate::Consume
        );

        // At the deadline (half-open, matching the registry's own rule)
        // the human has their machine back, whatever the embedder did.
        for kind in [
            motion(5.0, 5.0),
            press(BTN_LEFT),
            release(BTN_LEFT),
            key(0xff1b, KeyState::Pressed),
        ] {
            assert_eq!(
                grab.judge_parts(Origin::Physical, &kind, deadline),
                Gate::Deliver,
                "{kind:?} must reach the app once the prompt's petition is due"
            );
        }
        // And no click on the dead card can decide anything.
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));
        click(&mut grab, allow, deadline);
        assert!(grab.take_decision().is_none());

        // The registry state is still the grab's to settle -- the deadline
        // releases input, `retire_stale` takes the pixels down and clears
        // the flag once the petition actually leaves the table.
        registry.expire_due(deadline);
        assert!(grab.retire_stale(&mut registry, &mut surface));
        assert!(surface.prompt().is_none());
        assert!(grab.armed_petition().is_none());
        assert!(
            !grab.retire_stale(&mut registry, &mut surface),
            "idempotent"
        );
    }

    #[test]
    fn raising_over_a_dead_prompt_heals_instead_of_wedging() {
        // `raise` returning `None` while leaving a dead petition's card up
        // and the human's input grabbed is indistinguishable, to the
        // caller, from "someone else's prompt is up, retry later" -- the
        // reading that turns one missed `lower` into a permanent lockout.
        let (mut grab, mut surface, mut registry, first, t0) = armed();
        let realms = crate::realm::tests::registry_with(&["realm-0"]);
        let connection = registry.register_connection();
        let Admission::Pending { petition: second } = registry.admit(
            PetitionRequest {
                connection,
                identity: PrincipalIdentity::parse(PROMPT_IDENTITY).unwrap(),
                realm_name: "realm-0".into(),
                grant_wire_id: 20,
                consent_wire_id: 21,
                resource: String::new(),
                verbs: Verb::OBSERVE,
                expiry_ms: 0,
                max_event_rate: 0,
                persistence: WirePersistence::WhileRunning,
                flags: 0,
            },
            std::time::Instant::now(),
            &realms,
        ) else {
            panic!("an interactive petition must pend");
        };

        // The armed petition times out and the embedder has not lowered.
        registry.expire_due(std::time::Instant::now() + Duration::from_secs(3600));
        assert_eq!(grab.armed_petition(), Some(first));

        // Raising the next one heals rather than refusing: the dead card
        // comes down, the grab is released, and the queue advances.
        assert!(grab
            .raise(
                second,
                t0,
                &mut registry,
                &mut surface,
                &mut Scratch::new().recorder
            )
            .is_none());
        assert!(grab.armed_petition().is_none(), "the dead prompt is gone");
        assert!(surface.prompt().is_none());
    }

    #[test]
    fn lowering_discards_a_decision_nobody_drained() {
        // `lower` clears the in-flight press; a committed-but-undrained
        // decision is the same thing one step later, and leaving it queued
        // meant a click on a card since taken off screen could resolve a
        // still-pending petition at an arbitrary later time. Losing it
        // costs a timeout, which is refusal.
        let (mut grab, mut surface, mut registry, petition, t0) = armed();
        let deny = center_of(&grab, Choice::Deny);
        click(&mut grab, deny, awake(t0));

        grab.lower(&mut registry, &mut surface);
        assert!(
            grab.take_decision().is_none(),
            "a decision must not outlive the card it was taken on"
        );
        assert!(
            registry.prompt_content(petition).is_some(),
            "and the petition is untouched: still pending, still answerable"
        );
    }

    #[test]
    fn raising_marks_the_prompt_shown_so_consent_held_becomes_true() {
        // The one-moment property: the pixels, the grab, the chokepoint's
        // `consent_held` state and the flight-recorder entry all begin
        // together, because one call does all four.
        use crate::recorder::tests::{cleanup, of_kind, read_log};

        let (mut registry, petition) = pending_petition(WirePersistence::WhileRunning);
        let identity = PrincipalIdentity::parse(PROMPT_IDENTITY).unwrap();
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        let mut grab = ConsentGrab::new();
        let (mut recorder, log_path) = scratch_recorder("consent-grab-shown");
        grab.set_view(VIEW);
        assert!(
            !registry.prompt_up_for(&identity),
            "queued is not shown (petitions.rs: the consent_held mapping)"
        );

        let route = grab
            .raise(petition, t0(), &mut registry, &mut surface, &mut recorder)
            .expect("pending");
        assert_eq!(route.consent_wire_id, 11);
        assert!(registry.prompt_up_for(&identity));
        assert!(surface.prompt().is_some());
        assert_eq!(grab.armed_petition(), Some(petition));

        // The log states that a human was asked, written where the state
        // changed rather than by the separate wire call that can refuse.
        recorder.finish();
        let log = read_log(&log_path);
        let shown: Vec<_> = of_kind(&log, "consent_transition")
            .into_iter()
            .filter(|e| e.str("state") == "shown")
            .collect();
        assert_eq!(shown.len(), 1, "exactly one `shown` entry");
        assert_eq!(shown[0].str("petition"), petition.to_string());
        cleanup(&log_path);
    }

    #[test]
    fn the_hit_test_follows_the_card_when_the_view_changes() {
        // The card is centered, so a different view puts the buttons
        // somewhere else. Hit-testing derives the origin from the current
        // view through the compositor's own `centered`, so the two cannot
        // disagree -- the property that keeps a click landing on the button
        // the human sees.
        let (mut grab, _surface, _registry, petition, t0) = armed();
        let now = awake(t0);
        let deny_at_900x700 = center_of(&grab, Choice::Deny);

        const BIGGER: (u32, u32) = (1400, 1000);
        grab.set_view(BIGGER);
        // The old coordinates now point at the scrim.
        click(&mut grab, deny_at_900x700, now);
        assert!(grab.take_decision().is_none());

        // The button's new position decides.
        let prompt_card = {
            let prompt = grab.prompt.as_ref().unwrap();
            (prompt.card, prompt.buttons.clone())
        };
        let (ox, oy) = centered(prompt_card.0 .0, prompt_card.0 .1, BIGGER.0, BIGGER.1);
        let deny = prompt_card
            .1
            .iter()
            .find(|b| b.choice == Choice::Deny)
            .expect("Deny is always offered");
        click(
            &mut grab,
            (
                f64::from(ox + deny.rect.x) + f64::from(deny.rect.w) / 2.0,
                f64::from(oy + deny.rect.y) + f64::from(deny.rect.h) / 2.0,
            ),
            now,
        );
        assert_eq!(
            grab.take_decision(),
            Some(Decision {
                petition,
                choice: Choice::Deny
            })
        );
    }

    #[test]
    fn a_once_petition_offers_no_button_the_registry_would_refuse() {
        // The prompt only draws rungs that narrow, so the grab can only
        // ever produce a decision `resolve_human` accepts. Checked through
        // the geometry rather than the choice list, because the grab's
        // output is what the state machine sees.
        let (mut registry, petition) = pending_petition(WirePersistence::Once);
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        grab.raise(
            petition,
            t0(),
            &mut registry,
            &mut surface,
            &mut Scratch::new().recorder,
        )
        .expect("pending");
        let offered: Vec<Choice> = grab
            .prompt
            .as_ref()
            .unwrap()
            .buttons
            .iter()
            .map(|b| b.choice)
            .collect();
        assert_eq!(
            offered,
            vec![Choice::Allow(PersistenceRung::Once), Choice::Deny],
            "a `once` petition must not be offered a longer rung"
        );
    }

    #[test]
    fn non_finite_pointer_coordinates_never_hit_a_button() {
        // `f64 as i32` maps NaN to 0, which would land at the card's
        // top-left corner. A consent decision must not come out of
        // arithmetic nobody meant (see `card_local`).
        assert_eq!(card_local(f64::NAN, 0), None);
        assert_eq!(card_local(f64::INFINITY, 0), None);
        assert_eq!(card_local(f64::NEG_INFINITY, 0), None);
        assert_eq!(card_local(10.75, 4), Some(6));
        assert_eq!(card_local(-0.5, 0), Some(-1), "floor, not truncate");

        let (mut grab, _surface, _registry, _petition, t0) = armed();
        click(&mut grab, (f64::NAN, f64::NAN), awake(t0));
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn the_gate_delegates_events_the_grab_does_not_take() {
        // The wrapper's own contract: `observe` is unconditional
        // pass-through, and an event the grab declines to consume is
        // handed to the inner gate rather than swallowed. The
        // *short-circuit* half -- a consumed physical event never reaching
        // the inner gate, while the tap still sees it -- is proved through
        // the real router in `crate::input`'s tests, where a
        // physical-origin event can be minted (intake keeps that
        // constructor private, by design).
        use std::cell::Cell;

        struct Spy {
            observed: Rc<Cell<usize>>,
            gated: Rc<Cell<usize>>,
        }
        impl PreemptionHook for Spy {
            fn observe(&mut self, _input: &SeatInput) {
                self.observed.set(self.observed.get() + 1);
            }
            fn gate(&mut self, _input: &SeatInput) -> Gate {
                self.gated.set(self.gated.get() + 1);
                Gate::Deliver
            }
        }

        let (grab, _surface, _registry, _petition, t0) = armed();
        let grab = Rc::new(RefCell::new(grab));
        let observed = Rc::new(Cell::new(0));
        let gated = Rc::new(Cell::new(0));
        let mut gate = ConsentGate::new(
            Rc::clone(&grab),
            Rc::new(Cell::new(awake(t0))),
            Spy {
                observed: Rc::clone(&observed),
                gated: Rc::clone(&gated),
            },
        );

        let input = SeatInput::emulated(motion(1.0, 1.0));
        gate.observe(&input);
        assert_eq!(gate.gate(&input), Gate::Deliver);
        assert_eq!((observed.get(), gated.get()), (1, 1));
    }
}
