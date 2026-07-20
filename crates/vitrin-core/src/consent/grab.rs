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
//! - **Scroll and keys are consumed**, for the same reason plus a blunter
//!   one: a human answering a prompt is not typing at the app, so any key
//!   the app received during a prompt would be a keystroke aimed somewhere
//!   else.
//! - **Button presses are consumed**, and a press inside a button rectangle
//!   arms the decision its release commits (below).
//!
//! Button *releases* are the one deliberate exception, and they are not a
//! hole — see the pairing contract next.
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
//! Blanket-delivering releases is safe rather than merely convenient,
//! because the router pairs releases **per button code** and drops any whose
//! press it did not deliver ([`crate::input`]). So a release reaches the app
//! if and only if its own press did, which — since presses during the grab
//! are consumed — means only presses from before the grab. Nothing the human
//! aimed at the prompt can leak through, and no bookkeeping in this module
//! decides that: the router's existing pairing does.
//!
//! It also composes with the physical device's own state machine. A button
//! cannot be pressed twice without an intervening release, so the release
//! that ends a pre-grab drag can never be mistaken for the release that
//! commits a prompt decision: the former has no armed button (its press was
//! not consumed by this gate), the latter always does.
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
//! Noted for the protocol track rather than papered over here:
//! `docs/protocol/05-vitrin_consent.md` says agent actuation is refused
//! "under any grant" while a prompt is shown, which is *wider* than the
//! IDL's `consent_held` summary quoted above. Where prose and IDL disagree
//! the IDL wins (repo CLAUDE.md), so the per-principal reading is
//! implemented and the prose sentence is flagged for a protocol-track
//! amendment — the same treatment the whole-pixel-motion sentence got in
//! [`crate::input`]. Multi-agent arbitration ("physical preempts agent",
//! PRD Doc 2 §8) is Phase 2 regardless, and v0 has one agent.
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
//!    not to offer the surface.
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
//! neither Esc nor Enter, plus a guard interval that ignores keys arriving
//! within a few hundred milliseconds of the prompt appearing (so type-through
//! stays impossible). That is a design task with an accessibility surface of
//! its own, deferred rather than half-built.
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
//! # Interaction with the unsolved replica problem (issue #85)
//!
//! A confined app can draw a byte-identical copy of the consent card; there
//! is no trusted indicator yet, and building one needs startup ceremony or a
//! reserved output region (tracked in #85, explicitly not this task). One
//! partial mitigation falls out of this module for free and is worth
//! stating: **a replica gets no input grab.** Clicking a real prompt's
//! buttons produces no app-visible input at all, while clicking a replica
//! behaves like clicking an app — the app sees the press, the release, and
//! every motion between. The behaviours differ; what is missing is any way
//! for the human to *notice* the difference before deciding. This is a
//! mitigation, not a fix, and it does not close #85.
//!
//! # What is mechanism-only
//!
//! Nothing raises a prompt at runtime, because nothing constructs a
//! [`PetitionRegistry`] at runtime: the M1.1 listener wiring (issue #77)
//! owns that. The nested backend really does carry this gate in its router
//! (so the grab is live the instant a prompt is raised) and really does feed
//! it the view size on every input event — but with no petition registry
//! there is no petition, so [`ConsentGrab::raise`] has no caller outside
//! tests. Stated plainly, as P1.7.1 stated its own gap: **a running
//! `vitrind` still shows no consent prompt, and therefore never grabs
//! input.** The mechanism below is exercised end to end by tests, including
//! through the real router and the real wire to a mock shim.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use vitrin_protocol::generated::vitrin_actuator_pointer::ButtonState;
use vitrin_protocol::generated::vitrin_shim_seat::Origin;

use crate::input::{Gate, PreemptionHook, SeatInput, SeatInputKind};
use crate::petitions::{PetitionId, PetitionRegistry, PromptRoute};

use super::render::ChoiceBox;
use super::{centered, Choice, ConsentSurface};

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
    /// registry, and seize physical input — **one call**, so "visible",
    /// "input grabbed", and "`consent_held` holds" cannot drift apart. That
    /// was the seam [`ConsentSurface::show`] was documented to leave for
    /// this task, and this is the only place in the core that takes it.
    ///
    /// Returns where the petitioner's `vitrin_consent.state(shown)` must be
    /// sent — the caller's remaining job, because only the connection's
    /// `PrincipalServer` may speak on the wire
    /// ([`crate::principal::PrincipalServer::emit_consent_shown`]).
    ///
    /// `None` — and nothing changed — in two cases:
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
    ///   which the human ever sees what they are answering. Re-raising the
    ///   *same* petition is idempotent and allowed.
    pub fn raise(
        &mut self,
        petition: PetitionId,
        petitions: &mut PetitionRegistry,
        surface: &mut ConsentSurface,
    ) -> Option<PromptRoute> {
        if let Some(current) = self.armed_petition() {
            if current != petition {
                tracing::warn!(
                    %current,
                    incoming = %petition,
                    "refusing to replace a raised consent prompt; lower it first"
                );
                return None;
            }
        }
        // Both lookups first: nothing is shown or grabbed until the
        // petition has proven it is still pending.
        let content = petitions.prompt_content(petition)?;
        let route = petitions.pending_route(petition)?;

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
        Some(route)
    }

    /// Take the prompt down and release the grab — every path that ends a
    /// prompt: a decision drained, the consent timeout, the petitioner
    /// disconnecting. Idempotent.
    ///
    /// The registry needs nothing here: a petition's `prompt_shown` flag
    /// dies with its pending entry, so `consent_held` stops holding at the
    /// same moment the petition leaves the table
    /// ([`crate::petitions`]).
    pub fn lower(&mut self, surface: &mut ConsentSurface) {
        surface.dismiss();
        self.prompt = None;
        self.armed = None;
    }

    /// The petition whose prompt currently holds the grab, if any.
    pub fn armed_petition(&self) -> Option<PetitionId> {
        self.prompt.as_ref().map(|p| p.petition)
    }

    /// Drain the next decision the human took, oldest first.
    pub fn take_decision(&mut self) -> Option<Decision> {
        self.decisions.pop_front()
    }

    /// Judge one intake event: the whole grab policy, in one place.
    ///
    /// Takes `(origin, kind)` rather than a [`SeatInput`] for the reason
    /// [`crate::input::PhysicalPresence::note`] does: tests outside the
    /// input module can then model a physical event without a
    /// physical-origin constructor leaking out of intake. The only runtime
    /// caller is [`ConsentGate::gate`], which passes the tag intake bound.
    pub fn judge(&mut self, origin: Origin, kind: &SeatInputKind) -> Gate {
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

        if self.prompt.is_none() {
            return Gate::Deliver;
        }
        // Agent input is the chokepoint's business, not the router's
        // (module docs: consuming it here would silently overturn an
        // authority decision the core just made).
        if origin != Origin::Physical {
            return Gate::Deliver;
        }

        match kind {
            SeatInputKind::Button {
                button,
                state: ButtonState::Pressed,
            } => {
                self.armed = self.hit_test().map(|choice| (*button, choice));
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
            // Motion, scroll, keys — and text, which physical intake never
            // produces but which would be human-aimed if it ever did.
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
    inner: H,
}

impl<H: PreemptionHook> ConsentGate<H> {
    pub fn new(grab: Rc<RefCell<ConsentGrab>>, inner: H) -> Self {
        Self { grab, inner }
    }
}

impl<H: PreemptionHook> PreemptionHook for ConsentGate<H> {
    fn observe(&mut self, input: &SeatInput) {
        // Never consuming, never conditional: the tap must see the raw
        // event stream even while this gate is swallowing all of it.
        self.inner.observe(input);
    }

    fn gate(&mut self, input: &SeatInput) -> Gate {
        match self.grab.borrow_mut().judge(input.origin(), input.kind()) {
            Gate::Consume => Gate::Consume,
            Gate::Deliver => self.inner.gate(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use vitrin_protocol::generated::vitrin_actuator_pointer::Axis;
    use vitrin_protocol::generated::vitrin_grant::{Persistence as WirePersistence, Verb};
    use vitrin_protocol::generated::vitrin_shim_seat::KeyState;

    use super::*;
    use crate::consent::tests::PROMPT_IDENTITY;
    use crate::grants::PersistenceRung;
    use crate::identity::PrincipalIdentity;
    use crate::petitions::{
        Admission, ConsentPolicy, PetitionConfig, PetitionRegistry, PetitionRequest,
    };

    const VIEW: (u32, u32) = (900, 700);
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;

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

    /// A grab with the fixture prompt up, shown at [`VIEW`], plus the
    /// registry the petition lives in and the surface it is drawn on.
    fn armed() -> (ConsentGrab, ConsentSurface, PetitionRegistry, PetitionId) {
        let (mut registry, petition) = pending_petition(WirePersistence::WhileRunning);
        let mut surface = ConsentSurface::new();
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        grab.raise(petition, &mut registry, &mut surface)
            .expect("the petition is pending");
        (grab, surface, registry, petition)
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

    /// Click (press then release) at `(x, y)` with BTN_LEFT.
    fn click(grab: &mut ConsentGrab, (x, y): (f64, f64)) {
        grab.judge(Origin::Physical, &motion(x, y));
        grab.judge(Origin::Physical, &press(BTN_LEFT));
        grab.judge(Origin::Physical, &release(BTN_LEFT));
    }

    #[test]
    fn an_idle_grab_consumes_nothing() {
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        for kind in [
            motion(10.0, 10.0),
            press(BTN_LEFT),
            release(BTN_LEFT),
            SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: -120,
            },
            SeatInputKind::Key {
                keysym: 0xff1b,
                state: KeyState::Pressed,
            },
        ] {
            assert_eq!(grab.judge(Origin::Physical, &kind), Gate::Deliver);
        }
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn a_raised_prompt_consumes_every_physical_event_except_releases() {
        // "All physical input now routes exclusively to it" -- read
        // literally: motion (a hover side channel), scroll, and keys are
        // all stopped, not just clicks. Releases are the one documented
        // exception, and the router's pairing is what makes that safe.
        let (mut grab, _surface, _registry, _petition) = armed();
        for kind in [
            motion(10.0, 10.0),
            press(BTN_LEFT),
            SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: -120,
            },
            SeatInputKind::Key {
                keysym: 0xff0d,
                state: KeyState::Pressed,
            },
            SeatInputKind::Key {
                keysym: 0xff1b, // Escape: consumed, but decides nothing
                state: KeyState::Pressed,
            },
            SeatInputKind::Text {
                text: "typed".into(),
            },
        ] {
            assert_eq!(
                grab.judge(Origin::Physical, &kind),
                Gate::Consume,
                "{kind:?} must not reach the app while a prompt is up"
            );
        }
        assert_eq!(
            grab.judge(Origin::Physical, &release(BTN_LEFT)),
            Gate::Deliver,
            "hold-until-release: the router pairs and drops it"
        );
        assert!(
            grab.take_decision().is_none(),
            "no key and no stray click may decide a petition"
        );
    }

    #[test]
    fn no_key_answers_the_prompt() {
        // The settled keyboard decision, pinned so a later "just bind
        // Enter to the first button" cannot land quietly. Escape is
        // singled out because P1.7.3 claims it as the revocation chord:
        // it must mean nothing here.
        let (mut grab, _surface, _registry, petition) = armed();
        for keysym in [
            0xff0d, // Return
            0xff1b, // Escape -- reserved for P1.7.3's hold-Esc revocation
            0x0020, // space
            0xff09, // Tab
        ] {
            for state in [KeyState::Pressed, KeyState::Released] {
                assert_eq!(
                    grab.judge(Origin::Physical, &SeatInputKind::Key { keysym, state }),
                    Gate::Consume
                );
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
            let (mut grab, _surface, _registry, petition) = armed();
            let target = center_of(&grab, choice);
            click(&mut grab, target);
            assert_eq!(
                grab.take_decision(),
                Some(Decision { petition, choice }),
                "a click on {choice:?} must decide {choice:?}"
            );
            assert!(grab.take_decision().is_none(), "exactly one decision");
        }
    }

    #[test]
    fn a_press_that_slides_off_its_button_decides_nothing() {
        // The recoverable-misclick affordance (module docs): press on
        // Allow, notice, slide onto Deny, release -- and nothing is
        // decided, because the release did not land on the button the
        // press armed. Neither choice fires: not the one armed, not the
        // one under the cursor.
        let (mut grab, _surface, _registry, _petition) = armed();
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));
        let deny = center_of(&grab, Choice::Deny);

        grab.judge(Origin::Physical, &motion(allow.0, allow.1));
        grab.judge(Origin::Physical, &press(BTN_LEFT));
        grab.judge(Origin::Physical, &motion(deny.0, deny.1));
        grab.judge(Origin::Physical, &release(BTN_LEFT));
        assert!(grab.take_decision().is_none());

        // Sliding off the card entirely is the same story.
        grab.judge(Origin::Physical, &motion(allow.0, allow.1));
        grab.judge(Origin::Physical, &press(BTN_LEFT));
        grab.judge(Origin::Physical, &motion(1.0, 1.0));
        grab.judge(Origin::Physical, &release(BTN_LEFT));
        assert!(grab.take_decision().is_none());

        // ...and the prompt is still answerable afterwards.
        click(&mut grab, deny);
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
        let (mut grab, _surface, _registry, _petition) = armed();
        let deny = center_of(&grab, Choice::Deny);
        grab.judge(Origin::Physical, &motion(deny.0, deny.1));
        grab.judge(Origin::Physical, &press(BTN_LEFT));
        grab.judge(Origin::Physical, &release(BTN_RIGHT));
        assert!(grab.take_decision().is_none());
        // And the mismatched release disarmed, so BTN_LEFT's own release
        // (the human lifting the finger they pressed with) decides nothing
        // either -- fail-closed, never a decision from a confused pair.
        grab.judge(Origin::Physical, &release(BTN_LEFT));
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn clicking_the_card_body_or_the_scrim_decides_nothing() {
        let (mut grab, _surface, _registry, _petition) = armed();
        // The scrim, far from the card.
        click(&mut grab, (2.0, 2.0));
        // The card's own title area: inside the card, outside every button.
        let (ox, oy) = {
            let prompt = grab.prompt.as_ref().unwrap();
            centered(prompt.card.0, prompt.card.1, VIEW.0, VIEW.1)
        };
        click(&mut grab, (f64::from(ox) + 20.0, f64::from(oy) + 20.0));
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn a_second_click_cannot_decide_the_same_petition_twice() {
        // The grab keeps consuming after a decision (the card is still on
        // screen until the embedder lowers it, and input must not start
        // reaching the app in that gap), but never queues a second
        // decision -- the exactly-once property, held on this side too.
        let (mut grab, mut surface, _registry, petition) = armed();
        let deny = center_of(&grab, Choice::Deny);
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));
        click(&mut grab, deny);
        click(&mut grab, allow);
        assert_eq!(
            grab.take_decision(),
            Some(Decision {
                petition,
                choice: Choice::Deny
            })
        );
        assert!(grab.take_decision().is_none());
        assert_eq!(
            grab.judge(Origin::Physical, &motion(5.0, 5.0)),
            Gate::Consume,
            "input stays grabbed until the prompt is lowered"
        );

        grab.lower(&mut surface);
        assert_eq!(
            grab.judge(Origin::Physical, &motion(5.0, 5.0)),
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
        // actuation would be worse than letting it through.
        let (mut grab, _surface, _registry, _petition) = armed();
        for kind in [
            motion(10.0, 10.0),
            press(BTN_LEFT),
            release(BTN_LEFT),
            SeatInputKind::Text {
                text: "agent text".into(),
            },
        ] {
            assert_eq!(grab.judge(Origin::Emulated, &kind), Gate::Deliver);
        }
        assert!(grab.take_decision().is_none());
    }

    #[test]
    fn an_agent_cannot_move_the_hit_test_under_the_humans_click() {
        // v0 shares one cursor between origins, so an agent with a pointer
        // grant on a *different* petition can move it. That must not
        // relocate what the human's next click hits: the grab follows the
        // human's device only.
        let (mut grab, _surface, _registry, petition) = armed();
        let deny = center_of(&grab, Choice::Deny);
        let allow = center_of(&grab, Choice::Allow(PersistenceRung::WhileRunning));

        grab.judge(Origin::Physical, &motion(deny.0, deny.1));
        // The agent slides the shared cursor onto Allow...
        grab.judge(Origin::Emulated, &motion(allow.0, allow.1));
        // ...and the human clicks where they were looking.
        grab.judge(Origin::Physical, &press(BTN_LEFT));
        grab.judge(Origin::Physical, &release(BTN_LEFT));
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
        let mut surface = ConsentSurface::new();
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        let expired =
            registry.expire_due(std::time::Instant::now() + std::time::Duration::from_secs(3600));
        assert_eq!(expired.len(), 1);

        assert!(grab.raise(petition, &mut registry, &mut surface).is_none());
        assert!(grab.armed_petition().is_none());
        assert!(surface.prompt().is_none());
        assert_eq!(
            grab.judge(Origin::Physical, &motion(1.0, 1.0)),
            Gate::Deliver
        );
    }

    #[test]
    fn a_raised_prompt_is_not_replaced_by_another_petitions() {
        // One prompt at a time. Replacing would leave the *first* petition
        // marked `prompt_shown` with nothing on screen for it, so
        // `consent_held` would keep refusing that principal until the
        // timeout for a prompt no human can see. A queue advances by
        // lowering first.
        let (mut registry, first) = pending_petition(WirePersistence::WhileRunning);
        let realms = crate::realm::tests::registry_with(&["realm-0"]);
        let connection = registry.register_connection();
        let Admission::Pending { petition: second } = registry.admit(
            PetitionRequest {
                connection,
                identity: PrincipalIdentity::parse("vitrin://local/agent/other").unwrap(),
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

        let mut surface = ConsentSurface::new();
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        grab.raise(first, &mut registry, &mut surface)
            .expect("first");
        assert!(
            grab.raise(second, &mut registry, &mut surface).is_none(),
            "a second petition must not displace a raised prompt"
        );
        assert_eq!(grab.armed_petition(), Some(first));
        assert!(
            !registry
                .prompt_up_for(&PrincipalIdentity::parse("vitrin://local/agent/other").unwrap()),
            "the refused petition must not be marked shown"
        );
        // Re-raising the SAME petition is idempotent, not a refusal.
        assert!(grab.raise(first, &mut registry, &mut surface).is_some());

        // Lower, then raise: the queue advances.
        grab.lower(&mut surface);
        assert!(grab.raise(second, &mut registry, &mut surface).is_some());
        assert_eq!(grab.armed_petition(), Some(second));
    }

    #[test]
    fn raising_marks_the_prompt_shown_so_consent_held_becomes_true() {
        // The one-moment property: the pixels, the grab, and the
        // chokepoint's `consent_held` state all begin together, because
        // one call does all three.
        let (mut registry, petition) = pending_petition(WirePersistence::WhileRunning);
        let identity = PrincipalIdentity::parse(PROMPT_IDENTITY).unwrap();
        let mut surface = ConsentSurface::new();
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        assert!(
            !registry.prompt_up_for(&identity),
            "queued is not shown (petitions.rs: the consent_held mapping)"
        );

        let route = grab
            .raise(petition, &mut registry, &mut surface)
            .expect("pending");
        assert_eq!(route.consent_wire_id, 11);
        assert!(registry.prompt_up_for(&identity));
        assert!(surface.prompt().is_some());
        assert_eq!(grab.armed_petition(), Some(petition));
    }

    #[test]
    fn the_hit_test_follows_the_card_when_the_view_changes() {
        // The card is centered, so a different view puts the buttons
        // somewhere else. Hit-testing derives the origin from the current
        // view through the compositor's own `centered`, so the two cannot
        // disagree -- the property that keeps a click landing on the button
        // the human sees.
        let (mut grab, _surface, _registry, petition) = armed();
        let deny_at_900x700 = center_of(&grab, Choice::Deny);

        const BIGGER: (u32, u32) = (1400, 1000);
        grab.set_view(BIGGER);
        // The old coordinates now point at the scrim.
        click(&mut grab, deny_at_900x700);
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
        let mut surface = ConsentSurface::new();
        let mut grab = ConsentGrab::new();
        grab.set_view(VIEW);
        grab.raise(petition, &mut registry, &mut surface)
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

        let (mut grab, _surface, _registry, _petition) = armed();
        click(&mut grab, (f64::NAN, f64::NAN));
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

        let (grab, _surface, _registry, _petition) = armed();
        let grab = Rc::new(RefCell::new(grab));
        let observed = Rc::new(Cell::new(0));
        let gated = Rc::new(Cell::new(0));
        let mut gate = ConsentGate::new(
            Rc::clone(&grab),
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
