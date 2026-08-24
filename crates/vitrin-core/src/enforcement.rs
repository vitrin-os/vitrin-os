// SPDX-License-Identifier: MPL-2.0
//! The enforcement chokepoint (P1.4.4, issue #28): the **single function**
//! through which every capture, every actuation and every launch passes --
//! [`Chokepoint::enforce_use`] -- checking `connection -> principal ->
//! grant -> verbs -> constraints` in exactly that order (PRD Doc 2 §5) and
//! voicing every refusal as `vitrin_grant.refused` from one emission site.
//!
//! `vitrin_launcher.launch` (since version 2) passes through the same
//! funnel, and since WS-E.1.1 (issue #207) it can actually *succeed*:
//! `realm_launch` is in
//! [`SERVED_VERB_BITS`](crate::grants::SERVED_VERB_BITS), so a grant row
//! may carry the bit and an admitted launch forks a realm through
//! [`UseEnv::launch`]. **This is the point at which a request on a socket
//! can make the trusted core create a process**, and routing it here is
//! what buys that a consent prompt, an expiry, revocation, the token
//! bucket, a realm cap and a journal entry naming who asked -- rather than
//! a second path beside the chain, which is what the one-path property
//! below exists to forbid. The mint that produces the facet
//! ([`vitrin_grant.get_launcher`](crate::principal)) stays structural and
//! always legal; the *use* is what is judged.
//!
//! # The one-path property (grep-provable)
//!
//! "Exactly one code path to the check" is this module's acceptance
//! criterion, and it is pinned mechanically, not by convention:
//!
//! - every facet request arm in [`crate::principal`] converges on
//!   [`Chokepoint::enforce_use`] (grep `enforce_use`);
//! - the grant table's chokepoint query `check_use_grant` is called from
//!   exactly one non-test site in the crate -- here;
//! - `vitrin_grant.refused` is constructed at exactly one site in the
//!   crate -- [`Chokepoint::voice_refusal`], one refusal voice for every
//!   use class this core serves (IDL `vitrin_grant`; that function's own
//!   docs name the one class it does not, and why);
//! - the capture mechanics entry [`crate::capture::render_frame`] and the
//!   emulated-input constructor `SeatInput::emulated` each have exactly
//!   one caller outside their home modules -- here, *after* admission.
//!   Since WS-E.2.4 (issue #216) `capture.rs` also holds
//!   [`crate::capture::render_screenshot`], the human screenshot key's
//!   encoder, and the clause above stays literally true because that is a
//!   **sibling** of `render_frame` rather than a caller of it. The scan says
//!   so in both directions: `render_screenshot` has zero occurrences in this
//!   file and exactly one outside its home module (`crate::screenshot`), and
//!   `crate::screenshot` contains none of the chokepoint's own identifiers.
//!   A human holds no grant and is no principal, so there was never a check
//!   for that path to pass; what keeps it from being a bypass is that it
//!   cannot produce a `frame_ready`, cannot reach a connection, and cannot be
//!   called at all without a `HumanGesture` only a physical chord mints;
//! - and [`GrantTable::holds_verb`] -- the attention event's **delivery
//!   filter**, added by WS-E.1.7 -- is named by the scan **as an exclusion**:
//!   zero occurrences in this file, exactly one outside the grant table. It
//!   is a query about who to *tell*, never about who may *act*, and pinning
//!   it by name is what keeps that distinction from being re-established by
//!   accident every time someone reads the list;
//!
//! all four are asserted by this module's `single_enforcement_path` test,
//! which scans the crate's sources -- the reviewer's grep, run by CI.
//! The scan matches whole identifiers, not call shapes, so an import
//! (plain, renamed, or glob), a UFCS call, or a fn-pointer take of a
//! guarded item trips it exactly like a direct call; and a bare
//! `#[cfg(test)]` item mid-file does not end a file's scan -- only the
//! trailing `mod tests` module does.
//!
//! # The decision chain (and where each refusal code sits)
//!
//! 1. **connection** -- structural, upstream of this function: the facet
//!    id resolved in *this* connection's object table (sender-constrained
//!    handles; a foreign id died fatal `invalid_object` in dispatch) and
//!    the connection is in its bound steady state.
//! 2. **principal** -- the verifier-canonical identity bound at `hello`,
//!    passed in [`UseRequest::principal`]; the grant-row cross-check below
//!    re-verifies ownership (defense in depth).
//! 3. **grant** -- the facet's co-minted grant handle must have resolved
//!    `granted` (a pending handle, or one that resolved denied/timed_out/
//!    unavailable/unsupported/busy, is `not_granted` -- the IDL's "use
//!    while pending, through an ungranted facet, or after any non-granted
//!    resolution"), and its row must be alive: `revoked` > `expired` (time
//!    or spent) per the table's documented precedence.
//! 4. **verbs** -- the facet's verb must be in the row's effective set
//!    (`not_granted` otherwise, unconditionally -- judged inside the same
//!    table query, before the row's death code, per the table's docs).
//! 5. **constraints & use-context** -- only a use that holds live
//!    authority reaches these, so a transient hold can never mask the
//!    honest authority answer (`consent_held` on an ungranted facet would
//!    tell the agent to wait for a prompt that changes nothing):
//!    a. `no_surface` (capture and actuation alike, but **never a
//!       launch**): the realm has no live view. A capture must serve
//!       "never a stale frame", and an actuation into a dead realm must
//!       be refused audibly, never swallowed -- the IDL's refusal entry
//!       is verb-neutral, prose pages 07/08 list `no_surface` in both
//!       actuators' applicable sets, and the sync-barrier discovery idiom
//!       (IDL `sync`) relies on every enforcement failure being voiced as
//!       an event. Judged first among the use-context gates and before
//!       the bucket, so a vacant realm never drains quota and a transient
//!       hold never masks the realm's death. A launch is exempt because a
//!       vacant realm is the state `realm_launch` exists to leave (IDL
//!       `refusal`: "a launch is never refused no_surface").
//!    b. `consent_held` (attention-contending -- actuation *and* layout, see
//!       [`UseKind::contends_for_attention`]): the principal's own prompt is
//!       up ([`PetitionRegistry::prompt_up_for`] -- the mapping is
//!       documented in [`crate::petitions`]). Layout joined at WS-E.1.4
//!       because a principal that could fullscreen over its own pending
//!       card would be arranging the decision it is waiting on.
//!    c. `preempted` (attention-contending, same set): physical human input
//!       owns **the realm this use acts on**
//!       ([`PhysicalPresenceMap::owns_target`], fed per realm at the
//!       input router's hook point). Moving the output out from under a
//!       hand already on the keyboard is the hazard a synthetic click
//!       poses, one step larger. Which realm that is differs by kind and
//!       the choice is made in one place, at the gate: a seat-delivered
//!       use is judged against the realm its **grant** names, a layout
//!       request against the realm **physical input currently follows**
//!       (it moves what the human is looking at rather than being
//!       delivered into anything). Per realm since WS-E.1.6 (issue #212):
//!       one session-wide answer refused an agent working in realm B
//!       because a human was typing in realm A, which is the
//!       concurrent-operation claim denied for no reason a human could
//!       see.
//!
//!       **`preempted` is conditional here since WS-E.1.7 (issue #232), and
//!       only for the two layout verbs.** Nested inside 5c, strictly after
//!       5b, is the human's own attention window ([`crate::attention`]): a
//!       core-owned Super tap opens a one-second, single-use window in which
//!       a `layout_focus`/`layout_arrange` use by a principal the `attention`
//!       event reached is **not** refused `preempted`. It exists because a
//!       human at an in-realm shell otherwise cannot ask it to switch realms
//!       — the Enter that sends the request is the physical input that
//!       forbids it. Seat-delivered uses are untouched: a hand still mutes an
//!       agent actuating into the realm the hand is in, and no human gesture
//!       can lift that. **The press delegates nothing** — it withdraws a
//!       transient courtesy the core extends to the human's typing, and every
//!       authority exercised afterwards came from a grant approved on a
//!       consent card.
//!    d. `rate_limited`: the per-grant token bucket -- deliberately the
//!       **last** gate, so a token is consumed if and only if the use is
//!       otherwise admitted: quota meters what would actually happen, and
//!       an agent blocked by a prompt, a human hand, or a dead realm is
//!       told that, not `rate_limited` noise (and never billed for it).
//!    `preempted` and `consent_held` never refuse a capture (IDL
//!    `vitrin_view`: observation is concurrent by design). `no_surface`
//!    is about the *surface*, not the seat: a live realm whose shim has
//!    not yet minted its seat still admits actuations, and dropping
//!    those events is the delivery edge's business (IDL
//!    `vitrin_shim_session.get_seat` -- a delivery matter, never an
//!    authority question).
//! 6. **admission** -- [`GrantTable::commit_use`] spends single-use
//!    authority (two-phase admission, the grant table's documented
//!    decision: a *refused* use never burns a `once`; a use that fails
//!    server-side *after* this point -- `internal` -- stays spent,
//!    fail-closed), then the operation runs: a capture renders and sends
//!    `frame_ready`, an actuation is wrapped `SeatInput::emulated` (B2:
//!    the origin tag says who really caused it) and handed to the
//!    embedder's delivery sink **naming the grant's own realm** (WS-E.1.6:
//!    the sink addresses per realm, so an actuation reaches the app its
//!    grant is over whether or not a human is looking at it). **The
//!    attention window is claimed here too** (WS-E.1.7), and only when 5c's
//!    exemption really did suppress a refusal: a refused use never burns
//!    it, exactly as a refused use never burns a `once` rung, and a use that
//!    needed no exemption leaves it open for the one that does.
//!
//! # The gate this chain no longer has (WS-E.1.6, issue #212)
//!
//! Between 5c and the rate gate there used to be a **cross-realm delivery
//! guard**: the session had one input router and one delivery target, so a
//! grant naming any other realm was refused `internal` rather than having
//! its keystroke delivered into a different app. It was a stopgap, said so,
//! and is now **deleted** -- not relaxed. The realm travels with the
//! admitted event ([`UseEnv::grant_realm`] -> `session::route_seat`), so
//! there is no comparison left to make and nothing an agent can do to reach
//! a realm its grant does not name. The cross-principal denial-of-service
//! surface a `layout_focus` holder had over *other* principals' actuations
//! goes with it.
//!
//! # Decisions this task settles
//!
//! **Token-bucket state lives here, per connection, keyed by the wire
//! grant id -- not in the grant-table row.** Three reasons. (1) One
//! enforcement site: the table answers what rows *state*; a table-side
//! bucket would be a second place authority is enforced (the table's own
//! docs forbid it). (2) Lifetime: version-1 grants die with their
//! connection, and the [`Chokepoint`] is owned by the per-connection
//! server, so bucket state evaporates exactly when the authority does --
//! a core-global map would need a parallel cleanup contract and leak by
//! default. (3) The wire grant id is the key the *refusal-coalescing*
//! state needs anyway (a never-granted facet has no row id but its
//! refusals still coalesce "per grant"), and one wire grant maps to at
//! most one row forever (`resolved` fires exactly once), so the finer key
//! costs nothing. Bucket shape: capacity = `max_event_rate` (one second
//! of burst -- 100 captures against a 5/s grant admit exactly 5), refill
//! interval = `ceil(1s / rate)` so the sustained admitted rate can never
//! exceed the ceiling (fail-closed rounding), `retry_after_ms` =
//! time-to-next-whole-token, rounded up, never zero.
//!
//! **Rate-refusal granularity: per-capture refusals are uncoalesced
//! (IDL-mandated: one terminal per `capture_frame`, in request order);
//! fire-and-forget actuation refusals are coalesced at exactly the
//! delivery classification's two MAY-bounds** -- at most one
//! `refused(rate_limited)` per grant per bucket-refill window, and at
//! most one `refused` per grant per `(verb, code)` pair until a
//! subsequent request on that grant succeeds. Why coalesce at all: a
//! runaway 100/s actuator against a 5/s grant would otherwise be answered
//! with a ~95/s refusal storm -- wire noise the rate ceiling exists to
//! prevent, in the opposite direction. Why it is safe: the sync-barrier
//! idiom (conventions §6.4) needs only *at least one* refusal queued
//! before `done`, and each window/pair keeps its first as the
//! representative; the settled "excess events are dropped with an error
//! event, never silently" holds as the *condition* always being voiced --
//! per event where the IDL demands (captures), per window/pair where it
//! permits. The suppression state rides the same per-grant entry as the
//! bucket and is cleared by the next admitted use on that grant, exactly
//! as the classification specifies.
//!
//! **`consent_held` mapping**: decided and documented in
//! [`crate::petitions`] (prompt *shown* holds, `queued` does not; keyed by
//! verified identity across its connections; the hold ends when the
//! petition leaves the pending table). This module only consults
//! [`PetitionRegistry::prompt_up_for`], so P1.7.2's renderer inherits the
//! enforcement semantics by calling `mark_prompt_shown` -- no chokepoint
//! change.
//!
//! # One natural emission site (the P1.4.5 seam, now consumed)
//!
//! Every decision -- allow or refuse, voiced or coalesced -- funnels
//! through the tail of [`Chokepoint::enforce_use`] and is summarized in
//! its returned [`UseOutcome`]; every refusal frame is built in
//! [`Chokepoint::voice_refusal`], whose result (`voiced`) rides back in
//! that same summary. The flight recorder (P1.4.5, [`crate::recorder`])
//! consumes exactly that return value at `enforce_use`'s single caller and
//! **does not appear in this module at all**: authority code stays
//! authority code, no third code path appears, and the grep-provable
//! single-path property is unaffected by the existence of a log.
//!
//! Two facts the recorder needs are therefore carried out through
//! [`UseOutcome`] rather than fetched by a second observer: the delivered
//! frame's B1 observation digest (produced on the capture copy path, see
//! [`crate::capture::render_frame`]) and whether this admission spent a
//! `once` rung ([`GrantTable::commit_use`]'s return).
//!
//! # Clock discipline
//!
//! Like the grant table and the petition registry, this module never
//! reads a clock: `enforce_use` takes the dispatch turn's injected `now`
//! and uses that single instant for the table judgement, the prompt and
//! preemption holds, the bucket, and the coalescing windows.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use vitrin_ipc::TransportError;
use vitrin_protocol::generated::vitrin_grant::{self as grant, Refusal, Verb};
use vitrin_protocol::generated::vitrin_launcher as launcher;

use crate::capture::{self, RealmViewFrame};
use crate::grants::{GrantId, GrantTable};
use crate::identity::PrincipalIdentity;
use crate::input::{PhysicalPresenceMap, SeatInput, SeatInputKind};
use crate::petitions::PetitionRegistry;
use crate::realm::MintedRealmId;
use crate::recorder::ObservedFrame;

/// Nanoseconds per second, for exact integer bucket arithmetic.
const NANOS_PER_SEC: u64 = 1_000_000_000;

/// One facet-borne use, as the connection server resolved it from its
/// object table before calling the chokepoint (steps 1-2 of the chain:
/// the facet id was found in *this* connection's table, and `principal`
/// is the connection's verifier-canonical bound identity).
pub(crate) struct UseRequest<'a> {
    /// The facet object the request arrived on (`frame_ready` addresses
    /// it on success).
    pub facet_id: u32,
    /// The co-minted grant handle's wire id (`refused` addresses it, and
    /// the per-grant bucket/coalescing state keys on it).
    pub grant_wire_id: u32,
    /// The grant-table row behind the handle, if it resolved `granted`
    /// (`None` = pending or any non-granted resolution: `not_granted`).
    pub grant_row: Option<GrantId>,
    /// The connection's bound identity -- never client-claimed text.
    pub principal: &'a PrincipalIdentity,
    /// What the use does; its variant names the verb being exercised.
    pub kind: UseKind,
}

/// The operation a facet use performs. The variant *is* the verb: a
/// capture exercises `observe`, pointer events `actuate_pointer`, text
/// `actuate_text`, a launch `realm_launch` -- one facet, one verb bit
/// (IDL `request_grant`).
pub(crate) enum UseKind {
    /// `vitrin_view.capture_frame` (reply-bearing).
    Capture,
    /// `vitrin_actuator_pointer.move`/`button`/`scroll` (fire-and-forget).
    Pointer(SeatInputKind),
    /// `vitrin_actuator_text.type` (fire-and-forget).
    Text(SeatInputKind),
    /// `vitrin_launcher.launch` (reply-bearing, since version 2): fork a
    /// new realm instance from the template **the grant names**. Carries
    /// no payload, and that emptiness is the security property -- the
    /// request has no arguments on the wire and this variant is where
    /// that stays true in the core.
    Launch,
    /// `vitrin_layout_focus.focus` (fire-and-forget, since version 2):
    /// bind the output to this grant's realm and send the human's own
    /// physical input there. One act, never two -- see the IDL interface.
    LayoutFocus,
    /// `vitrin_layout_arrange.set_fullscreen` (fire-and-forget, since
    /// version 2): whether this grant's realm view tracks the output's
    /// size or keeps its own and is letterboxed.
    LayoutArrange(LayoutMode),
}

/// The arrangement a `set_fullscreen` asks for, re-stated in core terms so
/// the chokepoint and the session do not both import the wire enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutMode {
    /// The realm's view size tracks the output's: `configure` is re-sent
    /// with the output size now and on every later output resize.
    Fullscreen,
    /// The core imposes no size: nothing is sent, the realm keeps the size
    /// it has, and `Scene::compose` letterboxes it.
    Windowed,
}

/// A layout act the chokepoint **admitted**, handed to the embedder's
/// layout sink exactly as an admitted actuation is handed to
/// [`UseEnv::actuations`].
///
/// The realm is resolved by the caller from the grant row (never from an
/// argument -- neither layout request carries one), so a layout act can
/// only ever name the realm the human saw on the consent prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LayoutAct {
    /// Bind the output to this realm, and move the human's physical input
    /// with it.
    Focus { realm: crate::grants::RealmId },
    /// Put this realm in the named arrangement.
    Arrange {
        realm: crate::grants::RealmId,
        mode: LayoutMode,
    },
}

impl UseKind {
    /// The verb this use exercises (also the `verb` argument of any
    /// refusal, which identifies the facet to the client, and the `verb`
    /// field of the flight recorder's `use_decision` entry).
    pub fn verb(&self) -> Verb {
        match self {
            UseKind::Capture => Verb::OBSERVE,
            UseKind::Pointer(_) => Verb::ACTUATE_POINTER,
            UseKind::Text(_) => Verb::ACTUATE_TEXT,
            UseKind::Launch => Verb::REALM_LAUNCH,
            UseKind::LayoutFocus => Verb::LAYOUT_FOCUS,
            UseKind::LayoutArrange(_) => Verb::LAYOUT_ARRANGE,
        }
    }

    /// Whether refusals of this use MAY be coalesced (fire-and-forget
    /// actuations and layout requests) or are per-request terminals
    /// (reply-bearing captures and launches) -- the delivery
    /// classification, conventions §6.
    fn coalescible(&self) -> bool {
        !matches!(self, UseKind::Capture | UseKind::Launch)
    }

    /// Whether this use **contends for the human's attention**: it moves, or
    /// competes for, the thing the human is looking at or touching. The two
    /// actuations do; so do both layout requests, which is why they meet
    /// `consent_held` and `preempted` on the same terms (IDL
    /// `vitrin_grant.refusal`). A capture does not (observation is
    /// concurrent by design) and neither does a launch (it creates a realm
    /// rather than competing for one).
    ///
    /// **Named for what it means, not for the noun it used to share.** It was
    /// called `attention-shaped` until WS-E.1.7 (issue #232), which put a wire
    /// event *named* `attention` a few lines below meaning something entirely
    /// different -- the human's own signal, which *lifts* a refusal this
    /// predicate selects for. A free rename then; a permanent reading hazard
    /// in the one function where misreading it is most expensive.
    pub(crate) fn contends_for_attention(&self) -> bool {
        matches!(
            self,
            UseKind::Pointer(_)
                | UseKind::Text(_)
                | UseKind::LayoutFocus
                | UseKind::LayoutArrange(_)
        )
    }

    /// Whether the chokepoint needs [`UseEnv::grant_realm`] resolved for
    /// this use: the two actuations and the two layout requests, which name
    /// a realm to the embedder or are judged against one's physical
    /// presence, **and a launch**, whose template is the grant's realm.
    ///
    /// Named and public so `PrincipalServer::serve_facet_use` asks the kind
    /// rather than re-deriving the set with its own `matches!` — the
    /// duplicate that existed until WS-E.1.1 and would have silently
    /// handed the launch arm `None` (and therefore `internal`) the moment
    /// the verb became servable. A **capture** is still excluded, which is
    /// what keeps the high-rate path free of the realm-name clone.
    pub(crate) fn names_a_realm(&self) -> bool {
        self.contends_for_attention() || matches!(self, UseKind::Launch)
    }

    /// The two **layout** uses, and only those — the exact set the human's
    /// attention key may lift `preempted` for
    /// ([`crate::attention::AttentionSignal::exempt`], WS-E.1.7).
    ///
    /// A strict subset of [`Self::contends_for_attention`], and the difference
    /// between the two is the whole security claim: an actuation delivered
    /// through the seat is **never** exempted, so a human's hand still mutes an
    /// agent acting in the realm the hand is in, and no gesture of theirs can
    /// lift that. Deliberately a named predicate beside its superset rather
    /// than a `match` arm at the chokepoint, so the two sets sit next to each
    /// other and a future verb has to choose between them on purpose.
    pub(crate) fn is_layout(&self) -> bool {
        matches!(self, UseKind::LayoutFocus | UseKind::LayoutArrange(_))
    }

    /// Whether this use is **delivered into a realm through that realm's
    /// seat**, which is what decides *whose* physical presence preempts it
    /// (step 5c).
    ///
    /// Only the two actuations are. A layout request is emphatically not:
    /// `focus` exists precisely to *move* which realm the human's input
    /// follows, and `set_fullscreen` addresses a shim's `configure`, which is
    /// not the seat at all. That difference is exactly why the two are judged
    /// against different realms' presence.
    ///
    /// This predicate used to gate the cross-realm delivery guard, which
    /// WS-E.1.6 deleted along with the one-target placeholder it defended;
    /// what survives is the narrower question above.
    fn delivered_through_the_seat(&self) -> bool {
        matches!(self, UseKind::Pointer(_) | UseKind::Text(_))
    }
}

/// The per-turn environment the embedder provides alongside a dispatched
/// message: everything the chokepoint's use-context judgements and the
/// admitted operation need, borrowed for the call.
pub(crate) struct UseEnv<'a> {
    /// The realm's latest completed view, `None` while the realm has no
    /// surface (shim never attached, crashed, or exited): the
    /// `no_surface` judgement (capture and actuation alike) and the
    /// capture/clamp source.
    pub realm_view: Option<&'a RealmViewFrame<'a>>,
    /// Physical-input presence **per realm**, fed at the input router's hook
    /// point: the `preempted` judgement. Which realm's entry that judgement
    /// reads is chosen at the gate, from [`Self::grant_realm`] or
    /// [`Self::physical_realm`] — see step 5c.
    pub presence: &'a PhysicalPresenceMap,
    /// **The human's own attention signal** (WS-E.1.7, issue #232): the short,
    /// single-use window a core-owned Super tap opens, read by exactly one
    /// judgement — the exemption nested inside step 5c, for the two layout
    /// verbs only.
    ///
    /// A `&RefCell` rather than a borrow of the contents, because this is the
    /// one environment fact the chokepoint **writes**: the window is claimed at
    /// step-6 admission, and only when the exemption actually suppressed a
    /// refusal. It is the same `Rc<RefCell<_>>` the router's hook opens
    /// ([`crate::session::Kernel::attention`]), reached through the router, so
    /// it cannot be a second signal nothing writes.
    ///
    /// **It delegates nothing.** The press is the human saying their hand is
    /// off this app, not authorising anything: every authority exercised after
    /// it came from a grant approved on a consent card. See
    /// [`crate::attention`].
    pub attention: &'a std::cell::RefCell<crate::attention::AttentionSignal>,
    /// **The realm the human's physical input currently follows**, or `None`
    /// when no realm is bound (`session::physical_seat_target`, which follows
    /// the output binding a `layout_focus` holder moves).
    ///
    /// Consulted by exactly one judgement — `preempted` for a **layout**
    /// request — because a layout act is not delivered into a realm at all: it
    /// moves what the human is looking at, so the human it can steal from is
    /// the one in the realm they are already in, not the one the grant names.
    ///
    /// This field replaces `seat_reaches_grant_realm`, which asked whether the
    /// session's single seat happened to serve the grant's realm and refused
    /// `internal` when it did not (the chokepoint's old step 5d). WS-E.1.6
    /// deleted that question: seat delivery is per realm now, so an actuation
    /// always reaches the realm its grant names and there is nothing to
    /// compare. A value rather than a callback because there is exactly one
    /// answer per dispatch turn.
    pub physical_realm: Option<&'a crate::grants::RealmId>,
    /// Where admitted actuations go, already origin-tagged `emulated` and
    /// **addressed to the realm the grant names** (at runtime: that realm's
    /// seat state in the session's input router, reached through
    /// `session::route_seat`; tests: a capture buffer). Delivery beyond this
    /// sink -- including dropping events for a seatless or dead realm -- is
    /// the delivery edge's business, never an authority question.
    ///
    /// The realm is a parameter rather than something the sink derives,
    /// because deriving it is exactly the bug WS-E.1.6 closed: a sink that
    /// picked the target itself would deliver an agent's keystroke into
    /// whichever realm the session happened to be showing.
    pub actuations: &'a mut dyn FnMut(&crate::grants::RealmId, SeatInput),
    /// **The realm this grant names**, resolved by the caller from the
    /// grant row -- the same resolution that produced
    /// [`Self::realm_view`], from the same row, on the same line.
    ///
    /// Needed by the layout arms, which must name a realm to the embedder and
    /// must never take one from the wire (neither layout request carries a
    /// realm argument, precisely so that a holder can only ever move the realm
    /// the human saw on its consent prompt), and — since WS-E.1.6 — by the
    /// actuation arms, which must name the realm the event is **delivered
    /// into**, and by step 5c, which must name the realm whose physical
    /// presence preempts a seat-delivered use.
    ///
    /// `None` when the row is gone, which is unreachable past step 3; every
    /// arm that needs it surfaces that impossible case as the IDL's
    /// `internal` rather than guessing a realm.
    pub grant_realm: Option<&'a crate::grants::RealmId>,
    /// Where admitted **layout** acts go (at runtime:
    /// `session::apply_layout`, which binds the output and re-sends
    /// `configure`; tests: a log).
    ///
    /// A second sink beside [`Self::actuations`] rather than a widened
    /// one, because the two are different kinds of thing: an actuation is
    /// an input event with an origin tag that the delivery edge may drop,
    /// and a layout act is a change to the session's presentation that
    /// nothing downstream may reinterpret.
    pub layout: &'a mut dyn FnMut(LayoutAct),
    /// **Where an admitted launch forks** (WS-E.1.1, issue #207): given the
    /// realm the *grant* names, mint an instance id and create the process,
    /// or say why not.
    ///
    /// Three shapes here are load-bearing and none is an economy:
    ///
    /// - **It takes a realm and nothing else.** No command, no argument, no
    ///   id. `launch` carries no arguments on the wire and this signature
    ///   is where that stays true inside the core: the template comes from
    ///   [`Self::grant_realm`], resolved from the grant row exactly as the
    ///   layout arms' realm is, so a holder can only ever launch the
    ///   template the human saw on its consent card.
    /// - **It returns a [`MintedRealmId`]**, a type only
    ///   `RealmRegistry::mint_instance` constructs. The chokepoint
    ///   therefore *cannot* answer `launched` with anything a client
    ///   supplied -- there is no value of that type reachable from a wire
    ///   decode.
    /// - **It is synchronous, and the fork is inside it.** `launched` is a
    ///   terminal, so a deferred fork would mean replying success and
    ///   discovering failure afterwards, with no way to voice the IDL's
    ///   `internal`. What the runtime *does* defer is only the attach
    ///   sequence after the child exists (`configure`, the loop
    ///   registration, the registry insert), none of which can turn a
    ///   forked realm back into a refusal.
    pub launch: &'a mut dyn FnMut(LaunchAsk<'_>) -> Result<MintedRealmId, LaunchRefusal>,
}

/// Everything the embedder is told about an admitted launch — and the
/// enumeration is the security claim, so read it as an exhaustive list.
///
/// Three fields. **None of them came off the wire**: the template is the
/// realm the *grant row* names, the principal is the verifier-canonical
/// identity bound at `hello`, and the grant is the row the chain judged.
/// `vitrin_launcher.launch` carries no arguments at all, so there is
/// nothing else it *could* carry — and adding a field here later would be
/// a visible change at a site whose whole point is that it has none.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LaunchAsk<'a> {
    /// The realm whose configuration names the program to fork: the realm
    /// the human saw on this grant's consent card, never a client's choice.
    pub template: &'a crate::grants::RealmId,
    /// Who asked, for the flight recorder's `realm_spawned` entry.
    pub principal: &'a PrincipalIdentity,
    /// Which authority they exercised, likewise.
    pub grant: GrantId,
}

/// Why an admitted launch could not create a realm -- the two refusal codes
/// reachable *after* the authority chain has said yes.
///
/// Deliberately not [`Refusal`] itself: the sink answers about creating a
/// realm, and letting it name any refusal code would let the embedder
/// invent an authority answer (`not_granted` from a spawn path) that no
/// authority check produced. Two variants, mapped at one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchRefusal {
    /// The deployment is at [`crate::realm::MAX_REALMS`]. A **policy**
    /// answer, not a failure -- which is why the IDL gave it its own code
    /// rather than folding it into `internal`.
    Capacity,
    /// The fork, the exec, or the spawn-time program audit failed. The
    /// IDL's `internal`: "a spawn failure the core did not choose".
    Internal,
}

/// How one use was decided -- the chokepoint's summary of its own
/// decision, and the shape the flight recorder (P1.4.5) consumes. Every
/// fact the recorder's `use_decision` entry states is here, so the recorder
/// observes the chokepoint through its return value and never appears
/// inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseOutcome {
    /// Admitted: the operation ran (frame sent, or actuation handed to
    /// the sink) under this grant row.
    Admitted {
        grant: GrantId,
        /// The delivered observation's identity (B1) -- `Some` for an
        /// admitted capture, `None` for an actuation, which delivers no
        /// frame to identify.
        frame: Option<ObservedFrame>,
        /// Whether this admission consumed a `once` rung's single use --
        /// the active-to-spent grant lifecycle transition, reported at the
        /// instant it happens.
        spent_once: bool,
        /// Whether this admission **spent the human's attention window**
        /// (WS-E.1.7): `true` exactly when step 5c would have refused
        /// `preempted` and the exemption suppressed it. Carried out through
        /// the return value for the same reason `spent_once` is -- the
        /// recorder observes the chokepoint through its outcome and never
        /// appears inside it, so a journal exists without a third code path.
        attention_claimed: bool,
    },
    /// Refused with this code; `voiced` says whether a `refused` event
    /// was actually emitted (`false` = coalesced away under the delivery
    /// classification's MAY-bounds -- possible only for actuations).
    Refused { code: Refusal, voiced: bool },
}

/// A refusal decided by the chain, before emission. `retry_after_ms` is
/// nonzero only for [`Refusal::RateLimited`] by construction (the only
/// constructor that sets it is [`Refuse::rate_limited`]); the emission
/// site re-enforces the invariant defensively.
#[derive(Debug, Clone, Copy)]
struct Refuse {
    code: Refusal,
    retry_after_ms: u32,
}

impl Refuse {
    fn code(code: Refusal) -> Self {
        Self {
            code,
            retry_after_ms: 0,
        }
    }

    fn rate_limited(retry_after_ms: u32) -> Self {
        Self {
            code: Refusal::RateLimited,
            retry_after_ms,
        }
    }
}

/// The per-grant token bucket enforcing `constraints.max_event_rate`
/// over observation and actuation alike (PRD Doc 2 §5.2/§8). Integer
/// arithmetic on injected instants; capacity = rate (one second of
/// burst); refill interval rounded **up** so the sustained admitted rate
/// never exceeds the stated ceiling.
#[derive(Debug)]
struct TokenBucket {
    /// Whole tokens currently available.
    tokens: u32,
    /// The credit horizon: refill has been granted for time up to here;
    /// the sub-interval remainder beyond it stays banked.
    anchor: Instant,
}

/// Nanoseconds per token at `rate` events/second, rounded up (fail-closed:
/// a truncated interval would admit fractionally more than the ceiling).
/// Never zero: `div_ceil` of a positive numerator is at least 1.
fn interval_ns(rate: NonZeroU32) -> u64 {
    NANOS_PER_SEC.div_ceil(u64::from(rate.get()))
}

impl TokenBucket {
    /// A fresh bucket, born full at `now`: a grant's first uses may burst
    /// up to one second's worth before the sustained rate binds.
    fn full(rate: NonZeroU32, now: Instant) -> Self {
        Self {
            tokens: rate.get(),
            anchor: now,
        }
    }

    /// Refill-then-take at `now`. `Ok(())` consumed one token; `Err(ms)`
    /// is the rate-limit refusal with the IDL's refill hint: milliseconds
    /// until the next whole token accrues, rounded up, never zero.
    fn take(&mut self, rate: NonZeroU32, now: Instant) -> Result<(), u32> {
        let interval = u128::from(interval_ns(rate));
        let capacity = rate.get();
        let elapsed = now.saturating_duration_since(self.anchor).as_nanos();
        let credit = elapsed / interval;
        if credit > 0 {
            let total = u128::from(self.tokens) + credit;
            if total >= u128::from(capacity) {
                // Full bucket: excess credit is dropped, the horizon
                // restarts here (the petition bucket's precedent).
                self.tokens = capacity;
                self.anchor = now;
            } else {
                self.tokens = total as u32;
                // Advance by exactly the credited time. Bounded: credit <
                // capacity <= u32::MAX and credit * interval <= elapsed,
                // so the product fits u64 and the anchor never passes now.
                self.anchor += Duration::from_nanos((credit * interval) as u64);
            }
        }
        if self.tokens > 0 {
            self.tokens -= 1;
            Ok(())
        } else {
            // Empty implies credit == 0 above, so anchor <= now < anchor
            // + interval: the wait is positive and the ceiling keeps the
            // hint nonzero (IDL: nonzero exactly for rate_limited).
            let next_token_at = self.anchor + Duration::from_nanos(interval as u64);
            let wait = next_token_at.saturating_duration_since(now);
            let ms = wait
                .as_nanos()
                .div_ceil(1_000_000)
                .clamp(1, u128::from(u32::MAX)) as u32;
            Err(ms)
        }
    }
}

/// Per-grant enforcement state (keyed by wire grant id -- module docs):
/// the token bucket, lazily created from the row's effective ceiling on
/// the first admitted-path check, and the actuation-refusal coalescing
/// marks, cleared by the next admitted use.
#[derive(Debug, Default)]
struct GrantUseState {
    bucket: Option<TokenBucket>,
    /// `(verb bits, refusal wire code)` pairs already voiced since the
    /// last admitted use on this grant (coalescing rule: at most one per
    /// pair until a subsequent request succeeds).
    muted_pairs: BTreeSet<(u32, u32)>,
    /// Suppress further `rate_limited` refusals until this instant (at
    /// most one per bucket-refill window).
    rate_muted_until: Option<Instant>,
}

impl GrantUseState {
    /// An admitted use ends every coalescing window on this grant: the
    /// next refusal of any kind is voiced afresh.
    fn clear_mutes(&mut self) {
        self.muted_pairs.clear();
        self.rate_muted_until = None;
    }
}

/// The enforcement chokepoint of one principal connection: owns the
/// per-grant buckets and coalescing state, and exposes the **single**
/// enforcement function, [`Chokepoint::enforce_use`]. One instance per
/// connection (held by the connection's `PrincipalServer`), so its state
/// dies with the connection exactly as version-1 grants do.
#[derive(Debug, Default)]
pub(crate) struct Chokepoint {
    states: BTreeMap<u32, GrantUseState>,
}

impl Chokepoint {
    pub fn new() -> Self {
        Self::default()
    }

    /// **THE enforcement chokepoint** (PRD Doc 2 §5; issue #28): every
    /// `capture_frame`, `move`, `button`, `scroll`, and `type` passes
    /// through this function and no other authority decision exists on
    /// those paths. Checks `connection -> principal -> grant -> verbs ->
    /// constraints` (the first two are the caller's structural
    /// preconditions -- module docs), emits at most one terminal per use
    /// (`frame_ready` or `refused` for a capture, in request order and
    /// never coalesced; possibly-coalesced `refused` for actuations), and
    /// performs the admitted operation. Protocol-level infallible: every
    /// failure after decode is a typed recoverable refusal, and only
    /// transport death surfaces as `Err`.
    pub fn enforce_use<F>(
        &mut self,
        req: UseRequest<'_>,
        grants: &mut GrantTable,
        petitions: &PetitionRegistry,
        env: UseEnv<'_>,
        now: Instant,
        send: &mut F,
    ) -> Result<UseOutcome, TransportError>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        let verb = req.kind.verb();
        let coalescible = req.kind.coalescible();
        // The human's attention window, iff step 5c below actually used it to
        // suppress a `preempted` refusal. Declared out here because it is
        // *spent* at step 6 and nowhere else: a use refused by a later gate
        // (the bucket is the only one) drops it unclaimed, exactly as a refused
        // use never burns a `once` rung.
        let mut exemption: Option<crate::attention::Exemption> = None;

        // The decision chain (module docs). Every early exit is a typed
        // refusal -- fail closed: no path falls through to the operation
        // without an explicit admission.
        let decision: Result<crate::grants::Allowed, Refuse> = 'decide: {
            // Step 3, grant: a handle that never resolved `granted`
            // confers nothing (use while pending, or after any
            // non-granted resolution).
            let Some(row) = req.grant_row else {
                break 'decide Err(Refuse::code(Refusal::NotGranted));
            };
            // Steps 3-4, grant liveness + verbs (+ the expiry constraint,
            // checked on use): the grant table's single judgement.
            let allowed = match grants.check_use_grant(row, req.principal, verb, now) {
                Ok(allowed) => allowed,
                Err(reason) => break 'decide Err(Refuse::code(reason.into())),
            };
            // Step 5, use-context on live authority.
            // 5a, no_surface -- capture and actuation alike: never a
            // stale frame, and never input swallowed by a dead realm. A
            // launch is exempt by the IDL's own reachability note: a
            // vacant realm is the state `realm_launch` exists to leave,
            // so refusing a launch `no_surface` would refuse it for
            // being asked to do its job.
            if !matches!(req.kind, UseKind::Launch) && live_view(env.realm_view).is_none() {
                break 'decide Err(Refuse::code(Refusal::NoSurface));
            }
            if req.kind.contends_for_attention() {
                // **Whose realm the human's hand has to be in** for this use
                // to be preempted (5c below), and the one place the two
                // answers are chosen between (WS-E.1.6, issue #212).
                //
                // - A use **delivered through the seat** (pointer, text) acts
                //   on the realm its grant names, so that is the realm whose
                //   presence decides. This is the narrowing decision 4 buys:
                //   a human typing in realm A no longer mutes an agent
                //   working in realm B.
                // - A **layout** request is not delivered into a realm at all;
                //   it moves what the human is looking at. `focus` in
                //   particular takes the output *away from* the realm the
                //   human's hand is in, which is precisely the theft 5c
                //   exists to stop, and the grant's realm is the realm being
                //   moved *to*. So layout is judged against the realm
                //   physical input currently follows — which is exactly the
                //   session-wide behaviour layout had before this change,
                //   because physical input only ever accumulates presence in
                //   the realm it is addressed to.
                let preempted_by = if req.kind.delivered_through_the_seat() {
                    env.grant_realm
                } else {
                    env.physical_realm
                };
                // 5b, consent_held (attention-contending uses only): the
                // principal's own prompt is up. A layout request meets
                // this on the same terms an actuation does, and for a
                // sharper reason: the prompt IS the human's attention, and
                // a principal that could fullscreen a realm over its own
                // pending prompt's card -- or move the output away from
                // it -- would be arranging the very decision it is waiting
                // on. (Invariant 3 makes the card itself untouchable
                // whatever this gate does; this gate is the layer above,
                // and both exist.)
                if petitions.prompt_up_for(req.principal) {
                    break 'decide Err(Refuse::code(Refusal::ConsentHeld));
                }
                // 5c, preempted (attention-contending uses only): physical
                // human input owns the target right now. A focus request
                // yields to a hand on the keyboard for the same reason a
                // synthetic click does -- moving the output out from under
                // a human mid-keystroke is the theft this verb is
                // separately attenuable in order to bound.
                if env.presence.owns_target(preempted_by, now) {
                    // ...**unless the human just said their hand is off it**
                    // (WS-E.1.7, issue #232). The exemption nests inside 5c
                    // and strictly after 5b, and it is narrow in three
                    // independent ways:
                    //
                    // - **Layout only.** A seat-delivered use is untouched: a
                    //   human's hand still mutes an agent actuating into the
                    //   realm the hand is in, and no human gesture can lift
                    //   that. What the attention key answers is the loop a
                    //   human is *in* -- the Enter that tells a shell to
                    //   switch realms is the physical input that forbids the
                    //   switch.
                    // - **This principal only**, and only if the `attention`
                    //   event actually reached it at press time
                    //   (`AttentionSignal::exempt`). A grant resolving inside
                    //   the window cannot race in.
                    // - **Once.** `Exemption` is not `Copy`, cannot be minted
                    //   outside `crate::attention`, and is consumed by `claim`
                    //   at step 6 -- so one press admits at most one layout
                    //   use and a second claim is a compile error.
                    //
                    // It **delegates nothing**: the press is the human making
                    // a statement about their own input state, and every
                    // authority exercised after it came from a grant a human
                    // approved on a consent card naming this principal and
                    // this realm. A client that provokes the press gains
                    // timing, never authority.
                    //
                    // 5b is never exempted, and the order is what says so: a
                    // prompt up means the human is answering a security
                    // question, and a principal that could focus or fullscreen
                    // over its own pending card would be arranging the
                    // decision it is waiting on. (`ConsentGate` also consumes
                    // the chord before the attention hook sees it, so a window
                    // cannot open while a prompt is up -- but a window opened
                    // *before* the prompt went up must still meet
                    // `consent_held`, and only 5b-before-5c gives that answer.)
                    // The layout-only restriction is inside `exempt` (which
                    // takes the kind) rather than a `match` arm here: it is the
                    // sharpest claim this mechanism makes and it needs one home
                    // with one test reading it, not an arm whose widening the
                    // whole suite tolerated.
                    exemption = env.attention.borrow().exempt(req.principal, &req.kind, now);
                    if exemption.is_none() {
                        break 'decide Err(Refuse::code(Refusal::Preempted));
                    }
                }
            }
            // Step 5d, the rate constraint -- the final gate, so a token
            // is spent iff the use is otherwise admitted.
            let state = self.states.entry(req.grant_wire_id).or_default();
            let bucket = state
                .bucket
                .get_or_insert_with(|| TokenBucket::full(allowed.max_event_rate, now));
            match bucket.take(allowed.max_event_rate, now) {
                Ok(()) => Ok(allowed),
                Err(retry_after_ms) => break 'decide Err(Refuse::rate_limited(retry_after_ms)),
            }
        };

        // The single decision funnel: everything below either voices the
        // refusal (one emission site) or performs the admitted operation.
        // The flight recorder (P1.4.5) hooks exactly here.
        let allowed = match decision {
            Err(refuse) => {
                let voiced =
                    self.voice_refusal(req.grant_wire_id, verb, refuse, coalescible, now, send)?;
                return Ok(UseOutcome::Refused {
                    code: refuse.code,
                    voiced,
                });
            }
            Ok(allowed) => allowed,
        };

        // Step 6, admission. Commit before the operation: single-use
        // authority is consumed by the admitted use even if the server
        // fails it below (`internal`) -- fail-closed, never
        // authority-expanding (the grant table's documented decision).
        let spent_once = grants.commit_use(allowed.grant_id);
        // **The window is claimed here, at admission, and nowhere else.** Not
        // at the gate, because a use the bucket then refused would have burnt
        // the human's press for nothing; and not unconditionally, because
        // `exemption` is `Some` only when 5c really did find the human's hand
        // on the target and really did suppress the refusal. A use that needed
        // no exemption (`owns_target` false) leaves the window open for the one
        // that does. Same rule, and the same reason, as "a refused use never
        // burns a `once`".
        let attention_claimed = match exemption.take() {
            Some(exemption) => {
                env.attention.borrow_mut().claim(exemption);
                true
            }
            None => false,
        };
        if let Some(state) = self.states.get_mut(&req.grant_wire_id) {
            state.clear_mutes();
        }

        match req.kind {
            UseKind::Capture => {
                let rendered = match live_view(env.realm_view) {
                    Some(view) => capture::render_frame(view),
                    // Unreachable: the chain refused `no_surface` above at
                    // the same `now`. Surface typed and fail-closed --
                    // never a panic, never a fabricated frame.
                    None => Err(capture::CaptureError::DegenerateView {
                        width: 0,
                        height: 0,
                    }),
                };
                match rendered {
                    Ok((frame, digest)) => {
                        // Exactly one terminal per capture, in request
                        // order: the frame, with its fresh sealed memfd
                        // riding SCM_RIGHTS. The server's copy of the fd
                        // drops with `frame` after the send.
                        send(&frame.encode(req.facet_id), Some(frame.fd.as_fd()))?;
                        Ok(UseOutcome::Admitted {
                            grant: allowed.grant_id,
                            // B1: the delivered observation, identified by
                            // the digest the copy path computed over the
                            // very bytes just sent.
                            frame: Some(ObservedFrame {
                                width: frame.width,
                                height: frame.height,
                                stride: frame.stride,
                                bytes: u64::from(frame.stride) * u64::from(frame.height),
                                digest,
                            }),
                            spent_once,
                            attention_claimed,
                        })
                    }
                    Err(err) => {
                        // Post-admission server-side failure: the IDL's
                        // `internal`, recoverable, uncoalesced (a capture
                        // terminal is never coalesced).
                        tracing::warn!(%err, "capture failed after admission; refusing internal");
                        let voiced = self.voice_refusal(
                            req.grant_wire_id,
                            verb,
                            Refuse::code(Refusal::Internal),
                            false,
                            now,
                            send,
                        )?;
                        Ok(UseOutcome::Refused {
                            code: Refusal::Internal,
                            voiced,
                        })
                    }
                }
            }
            UseKind::Pointer(kind) | UseKind::Text(kind) => {
                // **The realm comes from the grant row**, exactly as it does
                // for the layout arms below and never from the wire (no
                // actuator request carries a realm argument). `None` is
                // unreachable past step 3 and is surfaced as the IDL's
                // `internal` rather than defaulting to some realm the grant
                // does not name -- the fail-closed direction, and the same
                // shape the layout arm and the unreachable capture readback
                // failure take.
                let Some(realm) = env.grant_realm else {
                    tracing::warn!("actuation admitted with no grant realm; refusing internal");
                    let voiced = self.voice_refusal(
                        req.grant_wire_id,
                        verb,
                        Refuse::code(Refusal::Internal),
                        coalescible,
                        now,
                        send,
                    )?;
                    return Ok(UseOutcome::Refused {
                        code: Refusal::Internal,
                        voiced,
                    });
                };
                // Realm-view coordinates outside the view are clamped,
                // not refused (IDL vitrin_actuator_pointer; conventions
                // §6.3 lists it legal-but-noteworthy). **To the granted
                // realm's own view**: `env.realm_view` is resolved by realm
                // id (WS-E.1.3), so the bound the coordinates are clamped
                // into is the one belonging to the realm this event is about,
                // not whatever the output happens to be showing.
                let kind = clamp_to_view(kind, live_view(env.realm_view));
                // B2: the origin tag is bound here, at the single
                // admitted-actuation intake -- the delivery sink (and
                // through it the router, shim seat, and app) sees exactly
                // who caused this event, and which realm it is for.
                (env.actuations)(realm, SeatInput::emulated(kind));
                Ok(UseOutcome::Admitted {
                    grant: allowed.grant_id,
                    // An actuation delivers no observation to identify.
                    frame: None,
                    spent_once,
                    attention_claimed,
                })
            }
            UseKind::LayoutFocus | UseKind::LayoutArrange(_) => {
                // The realm comes from the **grant row**, resolved by the
                // caller, never from the wire: neither layout request
                // carries a realm argument, so a holder can only move the
                // realm the human saw named on its consent prompt.
                //
                // `None` is unreachable past step 3 (a missing row was
                // refused `not_granted`) and is surfaced as the IDL's
                // `internal` rather than a panic or a silent drop, exactly
                // as the capture path's unreachable readback failure is.
                let Some(realm) = env.grant_realm else {
                    tracing::warn!("layout use admitted with no grant realm; refusing internal");
                    let voiced = self.voice_refusal(
                        req.grant_wire_id,
                        verb,
                        Refuse::code(Refusal::Internal),
                        coalescible,
                        now,
                        send,
                    )?;
                    return Ok(UseOutcome::Refused {
                        code: Refusal::Internal,
                        voiced,
                    });
                };
                let act = match req.kind {
                    UseKind::LayoutFocus => LayoutAct::Focus {
                        realm: realm.clone(),
                    },
                    UseKind::LayoutArrange(mode) => LayoutAct::Arrange {
                        realm: realm.clone(),
                        mode,
                    },
                    // The outer match already narrowed to these two.
                    _ => unreachable!("layout arm reached with a non-layout kind"),
                };
                (env.layout)(act);
                Ok(UseOutcome::Admitted {
                    grant: allowed.grant_id,
                    // A layout act delivers no observation to identify.
                    frame: None,
                    spent_once,
                    attention_claimed,
                })
            }
            UseKind::Launch => {
                // **The template comes from the grant row**, exactly as the
                // realm of an actuation and of a layout act does, and never
                // from the wire: `launch` carries no arguments at all, so a
                // holder can only ever start the template the human saw
                // named on its consent card. `None` is unreachable past
                // step 3 and is surfaced as `internal` rather than guessing
                // a realm to fork.
                let Some(realm) = env.grant_realm else {
                    tracing::warn!("launch admitted with no grant realm; refusing internal");
                    let voiced = self.voice_refusal(
                        req.grant_wire_id,
                        verb,
                        Refuse::code(Refusal::Internal),
                        false,
                        now,
                        send,
                    )?;
                    return Ok(UseOutcome::Refused {
                        code: Refusal::Internal,
                        voiced,
                    });
                };
                match (env.launch)(LaunchAsk {
                    template: realm,
                    principal: req.principal,
                    grant: allowed.grant_id,
                }) {
                    Ok(minted) => {
                        // Exactly one terminal per launch, in request order,
                        // never coalesced -- the reply-bearing pairing a
                        // capture's `frame_ready` obeys. The id is a
                        // `MintedRealmId`, so the *only* string that can
                        // appear here came out of the realm registry.
                        let event = launcher::events::Launched {
                            realm: minted.as_realm_id().to_string(),
                        };
                        send(&event.encode(req.facet_id), None)?;
                        Ok(UseOutcome::Admitted {
                            grant: allowed.grant_id,
                            // A launch delivers no observation to identify.
                            // Deliberately not the new realm's first frame:
                            // launching confers nothing over what was
                            // launched (IDL `launched`).
                            frame: None,
                            spent_once,
                            attention_claimed,
                        })
                    }
                    // Post-admission, and both are the IDL's own codes for
                    // this operation: `capacity` is a policy answer about
                    // the deployment, `internal` a spawn failure the core
                    // did not choose. Never coalesced -- a terminal that
                    // coalesced away would leave the client waiting forever.
                    Err(refusal) => {
                        let code = match refusal {
                            LaunchRefusal::Capacity => Refusal::Capacity,
                            LaunchRefusal::Internal => Refusal::Internal,
                        };
                        tracing::warn!(?code, "launch admitted but no realm was created");
                        let voiced = self.voice_refusal(
                            req.grant_wire_id,
                            verb,
                            Refuse::code(code),
                            false,
                            now,
                            send,
                        )?;
                        Ok(UseOutcome::Refused { code, voiced })
                    }
                }
            }
        }
    }

    /// The single site where every `vitrin_grant.refused` is built and
    /// sent -- one refusal voice (IDL). For coalescible refusals (the two
    /// actuations and the two layout requests -- see
    /// [`UseKind::coalescible`]) it applies the delivery classification's
    /// two MAY-bounds and returns whether the event was actually voiced;
    /// refusals of the reply-bearing requests (capture and launch) are
    /// always voiced, because a terminal that coalesced away would leave
    /// the client waiting forever. Defensively re-enforces the IDL
    /// invariant that `retry_after_ms` is nonzero only for
    /// `rate_limited`, in release builds too.
    ///
    /// **This function is one voice for four of the IDL's six use
    /// classes, not six**, and the missing two are named here rather than
    /// left to be inferred from a `match` with no arm.
    /// `vitrin_grant.refusal` enumerates capture, actuation, launch, the
    /// layout verbs, **designation** and **egress**; [`UseKind`] has a
    /// variant for the first four and for neither of the last two,
    /// because this core dispatches none of `vitrin_grant.get_powerbox`,
    /// `vitrin_powerbox`'s two asks, `vitrin_grant.get_egress` or
    /// `vitrin_egress.request_connect` -- every one of them is answered
    /// `invalid_opcode` today, which the IDL's own `vitrin_powerbox` and
    /// `vitrin_egress` descriptions record as a gap between the document
    /// and this binary. So no designation and no egress refusal is ever
    /// built here, and in both cases the reason is an absent dispatch arm
    /// rather than an exemption at the chokepoint. P2.6.6 (the core-drawn
    /// picker) and the task that lands the out-of-core proxy own closing
    /// the two halves.
    ///
    /// That count read **five** until this commit, and why is worth
    /// keeping: it was written here on the egress branch (P2.7.2) while
    /// `designate_file` (P2.6.5) was landing on a parallel one, so the
    /// IDL enumeration this comment restates had itself omitted
    /// designation. Restating a normative list is only ever as sound as
    /// the list -- which is why the number is spelled out here rather
    /// than left as "every class `UseKind` does not cover".
    fn voice_refusal<F>(
        &mut self,
        grant_wire_id: u32,
        verb: Verb,
        refuse: Refuse,
        coalescible: bool,
        now: Instant,
        send: &mut F,
    ) -> Result<bool, TransportError>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        let retry_after_ms = if refuse.code == Refusal::RateLimited {
            // The bucket's hint is already >= 1; keep the floor explicit
            // so the invariant survives refactors of the hint math.
            refuse.retry_after_ms.max(1)
        } else {
            if refuse.retry_after_ms != 0 {
                tracing::warn!(
                    code = ?refuse.code,
                    retry_after_ms = refuse.retry_after_ms,
                    "nonzero retry_after_ms on a non-rate_limited refusal; \
                     zeroing it (IDL vitrin_grant.refused invariant)"
                );
            }
            0
        };
        if coalescible {
            let state = self.states.entry(grant_wire_id).or_default();
            let suppressed = if refuse.code == Refusal::RateLimited {
                // At most one refused(rate_limited) per grant per
                // bucket-refill window: the window ends when the next
                // whole token accrues -- exactly the retry hint.
                let muted = state.rate_muted_until.is_some_and(|until| now < until);
                if !muted {
                    state.rate_muted_until =
                        Some(now + Duration::from_millis(u64::from(retry_after_ms)));
                }
                muted
            } else {
                // At most one refused per grant per (verb, code) pair
                // until a subsequent request on that grant succeeds.
                !state
                    .muted_pairs
                    .insert((verb.bits(), refuse.code.to_wire()))
            };
            if suppressed {
                return Ok(false);
            }
        }
        let event = grant::events::Refused {
            verb,
            code: refuse.code,
            retry_after_ms,
        };
        send(&event.encode(grant_wire_id), None)?;
        Ok(true)
    }
}

/// The realm view, iff it is live (present with nonzero dimensions) --
/// the single predicate behind both the `no_surface` judgement and the
/// capture/clamp source, so the two can never drift.
fn live_view<'v, 'p>(view: Option<&'v RealmViewFrame<'p>>) -> Option<&'v RealmViewFrame<'p>> {
    view.filter(|v| v.width > 0 && v.height > 0)
}

/// Clamp an actuation's realm-view coordinates into the live view (IDL:
/// "coordinates outside the view are clamped"). Only motion carries
/// coordinates in version 1. The no-view arm passes through untouched:
/// unreachable for admitted actuations (the chain refused `no_surface`
/// at the same instant), kept total so the helper stays a pure function
/// of its inputs.
///
/// **The view is the granted realm's**, not the output's, and has been since
/// WS-E.1.3 made [`UseEnv::realm_view`] a function of the realm id: a grant
/// over a hidden realm clamps into that realm's own frame, and a grant over a
/// realm with no frame at all is refused `no_surface` before reaching here.
/// The two are the same *numbers* today, because there is one output and every
/// realm's view is composed at its size (`scene::realms`), but they are no
/// longer the same *source* — and only the source is this function's to get
/// right. Per-realm view **sizes** would be window-management geometry, which
/// PRD §5.1 keeps out of the core permanently.
fn clamp_to_view(kind: SeatInputKind, view: Option<&RealmViewFrame<'_>>) -> SeatInputKind {
    match (kind, view) {
        (SeatInputKind::Motion { x, y }, Some(view)) => SeatInputKind::Motion {
            x: x.clamp(0.0, f64::from(view.width - 1)),
            y: y.clamp(0.0, f64::from(view.height - 1)),
        },
        (kind, _) => kind,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn rate(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    // -- the token bucket ---------------------------------------------------

    #[test]
    fn bucket_bursts_to_capacity_then_holds_the_sustained_rate() {
        let t0 = Instant::now();
        let r = rate(5);
        let mut bucket = TokenBucket::full(r, t0);

        // The acceptance shape: at one instant a 5/s grant admits exactly
        // 5, and every refusal hints the same next-token time.
        for _ in 0..5 {
            assert_eq!(bucket.take(r, t0), Ok(()));
        }
        for _ in 0..95 {
            assert_eq!(bucket.take(r, t0), Err(200), "5/s => 200 ms per token");
        }

        // One interval later exactly one token accrued.
        let later = t0 + Duration::from_millis(200);
        assert_eq!(bucket.take(r, later), Ok(()));
        assert!(bucket.take(r, later).is_err());

        // Sub-interval progress is banked, never lost: 100 ms after the
        // last credit the hint is the 100 ms remainder.
        let mid = later + Duration::from_millis(100);
        assert_eq!(bucket.take(r, mid), Err(100));
    }

    #[test]
    fn bucket_never_exceeds_capacity_and_hint_is_never_zero() {
        let t0 = Instant::now();
        let r = rate(2);
        let mut bucket = TokenBucket::full(r, t0);
        assert_eq!(bucket.take(r, t0), Ok(()));
        assert_eq!(bucket.take(r, t0), Ok(()));

        // An hour idle refills to capacity, not beyond.
        let after_idle = t0 + Duration::from_secs(3600);
        assert_eq!(bucket.take(r, after_idle), Ok(()));
        assert_eq!(bucket.take(r, after_idle), Ok(()));
        let refused = bucket.take(r, after_idle);
        assert!(
            matches!(refused, Err(ms) if ms >= 1),
            "hint must be nonzero: {refused:?}"
        );
    }

    #[test]
    fn bucket_interval_rounds_up_so_the_ceiling_binds() {
        // 3/s: a truncated interval (333_333_333 ns) would admit slightly
        // more than 3/s over time; div_ceil keeps the admitted rate at or
        // below the ceiling.
        assert_eq!(interval_ns(rate(3)), 333_333_334);
        assert_eq!(interval_ns(rate(1)), 1_000_000_000);
        // Rates above 1e9/s still cost at least a nanosecond per event.
        assert_eq!(interval_ns(rate(u32::MAX)), 1);
    }

    // -- coordinate clamping ------------------------------------------------

    #[test]
    fn motion_clamps_into_the_live_view_and_passes_through_without_one() {
        let rgba = vec![0u8; 64 * 48 * 4];
        let view = RealmViewFrame {
            rgba: &rgba,
            width: 64,
            height: 48,
        };
        let clamped = clamp_to_view(
            SeatInputKind::Motion { x: 1e6, y: -3.0 },
            live_view(Some(&view)),
        );
        assert_eq!(clamped, SeatInputKind::Motion { x: 63.0, y: 0.0 });

        let untouched = clamp_to_view(SeatInputKind::Motion { x: 1e6, y: -3.0 }, live_view(None));
        assert_eq!(untouched, SeatInputKind::Motion { x: 1e6, y: -3.0 });

        // Non-motion kinds are untouched.
        let scroll = SeatInputKind::Scroll {
            axis: vitrin_protocol::generated::vitrin_actuator_pointer::Axis::Vertical,
            value120: -120,
        };
        assert_eq!(clamp_to_view(scroll.clone(), Some(&view)), scroll);
    }

    /// **The clamp reads the frame it is handed, and nothing else**
    /// (WS-E.1.6, issue #212).
    ///
    /// Read what this does and does not establish, because the difference is
    /// where a vacuous test would sit. It pins that `clamp_to_view` is a
    /// function of its argument: hand it two different frames and it produces
    /// two different bounds. That is a real guard — it fails if someone later
    /// reaches for an output-sized or otherwise session-wide bound *inside*
    /// the clamp — and it is all a test at this level can reach.
    ///
    /// It does **not** establish #212's actual routing property, that the
    /// frame handed over is the *granted* realm's. That resolution happens in
    /// the caller ([`UseEnv::realm_view`], populated by realm id in
    /// `session`), so no argument this test constructs can exercise it: this
    /// test supplies the very binding the property is about, which is exactly
    /// the shape that made three earlier tests in this workstream vacuous.
    /// What pins the caller side is the mock-free gate
    /// `tests/integration/test_input_switch.py`, whose first and third cases
    /// drive a real agent's actuation into a realm the output is not showing
    /// and assert the flight recorder names that realm.
    ///
    /// **Per-realm view *sizes* do not exist today**, deliberately. There is
    /// one output and every realm's view composes at its size
    /// (`scene::realms`), so the two numbers agree in every running session;
    /// inventing a per-realm size would be window-management geometry, which
    /// PRD §5.1 keeps out of the core permanently. What is checkable — and is
    /// what a regression would break — is that the *source* is the realm's own
    /// frame rather than a session-wide one, which this test states by handing
    /// it two.
    #[test]
    fn the_clamp_bound_comes_from_the_frame_it_is_given_not_from_a_session_wide_one() {
        let small = vec![0u8; 8 * 6 * 4];
        let large = vec![0u8; 64 * 48 * 4];
        let realm_b = RealmViewFrame {
            rgba: &small,
            width: 8,
            height: 6,
        };
        let realm_a = RealmViewFrame {
            rgba: &large,
            width: 64,
            height: 48,
        };
        let far = || SeatInputKind::Motion { x: 1e6, y: 1e6 };
        assert_eq!(
            clamp_to_view(far(), live_view(Some(&realm_b))),
            SeatInputKind::Motion { x: 7.0, y: 5.0 },
            "a grant over realm B must clamp into B's own view"
        );
        assert_eq!(
            clamp_to_view(far(), live_view(Some(&realm_a))),
            SeatInputKind::Motion { x: 63.0, y: 47.0 },
            "...and one over realm A into A's, from the same function in the same build"
        );
    }

    // -- the one-path property (the reviewer's grep, run by CI) -------------

    fn rust_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("crate src dir is readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("source file is readable");
                out.push((path, text));
            }
        }
    }

    /// The non-test portion of one source file: everything before the
    /// crate-conventional trailing unit-test module -- a `#[cfg(test)]`
    /// attribute immediately followed by a `mod tests` item (any
    /// visibility). The one-path property is about production code
    /// paths; test code legitimately decodes refusals and drives
    /// mechanics. A bare `#[cfg(test)]` on a single item (e.g. a
    /// test-only helper inside a production impl block) deliberately
    /// does NOT end the scan: production code after it stays scanned,
    /// and the gated item itself is scanned too -- erring closed, since
    /// a test-only helper that names an enforcement identifier deserves
    /// to trip the tripwire. (`#[cfg(any(test, ..))]` hooks like
    /// scripted consent likewise stay scanned.)
    fn non_test(text: &str) -> &str {
        let attr = format!("#[cfg({})]\n", "test");
        let mut from = 0;
        while let Some(found) = text[from..].find(&attr) {
            let at = from + found;
            let item = text[at + attr.len()..]
                .lines()
                .next()
                .unwrap_or("")
                .trim_start();
            let item = item
                .strip_prefix("pub(crate) ")
                .or_else(|| item.strip_prefix("pub "))
                .unwrap_or(item);
            if item.starts_with("mod tests") {
                return &text[..at];
            }
            from = at + attr.len();
        }
        text
    }

    /// Whether `c` can continue a Rust identifier -- the boundary test
    /// for [`count`]'s whole-identifier matching.
    fn is_ident(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    /// Count whole-identifier occurrences of `needle` in the non-test
    /// portion of `haystack`, per line, skipping comment lines. Matching
    /// the bare identifier -- never a call shape like `.name(` -- is
    /// what makes the tripwire evasion-resistant: every Rust
    /// construction that reaches a guarded item (method call, UFCS,
    /// `use` import plain or renamed, glob-imported bare name, fn
    /// pointer) must utter its identifier somewhere in the file; only a
    /// proc-macro could paste one invisibly, and this crate uses none.
    fn count(haystack: &str, needle: &str) -> usize {
        non_test(haystack)
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| {
                let mut hits = 0;
                let mut from = 0;
                while let Some(found) = line[from..].find(needle) {
                    let at = from + found;
                    let end = at + needle.len();
                    let before = line[..at].chars().next_back().is_none_or(|c| !is_ident(c));
                    let after = line[end..].chars().next().is_none_or(|c| !is_ident(c));
                    if before && after {
                        hits += 1;
                    }
                    from = at + 1;
                }
                hits
            })
            .sum()
    }

    /// One tripwire of [`single_enforcement_path_is_grep_provable`]: an
    /// identifier that may appear only in its home module and (exactly
    /// `in_enforcement` times) in enforcement.rs.
    struct Rule<'r> {
        what: &'r str,
        needle: &'r str,
        /// Longer phrases whose occurrences are excused everywhere by
        /// subtraction: each contains the needle without naming the
        /// guarded item (the chokepoint's own outcome variant shares
        /// the wire event's `Refused` name, and so does an unrelated
        /// wire enum on another interface).
        ///
        /// **Every excusal must be a QUALIFIED path**, never a bare
        /// alias, which is what keeps the subtraction honest: a bare
        /// `Refused` anywhere outside the home module still trips the
        /// rule, so an excused enum can only spend its excusal by
        /// spelling out which enum it is.
        excused: &'r [&'r str],
        home: &'r str,
        /// Exact non-test occurrence count required in enforcement.rs.
        in_enforcement: usize,
    }

    #[test]
    fn single_enforcement_path_is_grep_provable() {
        // The acceptance criterion "exactly one code path to the check",
        // asserted against the crate's own sources. Needles are
        // assembled at runtime so this test's source never matches them
        // even if the truncation of its own test module ever regresses.
        let mut sources = Vec::new();
        rust_sources(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut sources,
        );
        assert!(
            sources.iter().any(|(p, _)| p.ends_with("principal.rs")),
            "source scan must cover the crate"
        );

        // (1) The grant table's chokepoint query has exactly one caller
        // outside the table's own module: the chokepoint.
        let table_query = format!("check_use{}", "_grant");
        // (2) The principal-keyed admission query backs no path outside
        // the table module (it is the documented Phase-2 seam).
        // Whole-identifier matching keeps `check_use_grant` from
        // matching this shorter needle.
        let admission_query = format!("check_{}", "use");
        // (3) The `vitrin_grant.refused` event type is named at exactly
        // two non-test sites in the crate, both in this module: the
        // `UseOutcome::Refused` variant *declaration* and the single
        // emission-site construction in `voice_refusal`. Any second
        // construction -- canonical path, `use` import, rename, or glob
        // -- must utter the type name and trips the rule; *uses* of the
        // outcome variant are excused by subtraction so consumers (the
        // P1.4.5 flight recorder) can match on it anywhere.
        let refusal_name = format!("{}used", "Ref");
        let outcome_variant = format!("UseOutcome::{}used", "Ref");
        // ...and a THIRD name that is not this vocabulary at all: the wire's
        // `vitrin_shim_session.pointer_constraint_status` has a `refused`
        // entry (WS-E.4.2, issue #222), and an app being told its pointer lock
        // was declined has nothing to do with a principal's grant being
        // refused. It is excused by its own qualified path, exactly as the
        // chokepoint's outcome variant is, so a BARE `Refused` in
        // `input/constraint.rs` still trips this rule.
        let constraint_status = format!("PointerConstraintStatus::{}used", "Ref");
        // (4) The capture mechanics entry has exactly one caller outside
        // its home module: the chokepoint, after admission.
        let capture_entry = format!("render_{}", "frame");
        // (5) Emulated input is minted at exactly one site outside the
        // input module: the chokepoint, after admission.
        let emulated_ctor = format!("{}mulated", "e");

        // Each identifier is allowed in its home module (definition +
        // that module's own unit tests) and in enforcement.rs -- nowhere
        // else; and enforcement.rs contains an exact occurrence census,
        // so the chokepoint itself cannot quietly grow a second site.
        let rules = [
            Rule {
                what: "chokepoint table query",
                needle: &table_query,
                excused: &[],
                home: "grants.rs",
                in_enforcement: 1,
            },
            Rule {
                what: "refusal event name",
                needle: &refusal_name,
                excused: &[&outcome_variant, &constraint_status],
                home: "enforcement.rs",
                in_enforcement: 2,
            },
            Rule {
                what: "capture mechanics entry",
                needle: &capture_entry,
                excused: &[],
                home: "capture.rs",
                in_enforcement: 1,
            },
            Rule {
                what: "emulated-input mint",
                needle: &emulated_ctor,
                excused: &[],
                home: "input/mod.rs",
                in_enforcement: 1,
            },
        ];
        for rule in &rules {
            let net = |text: &str| {
                count(text, rule.needle)
                    - rule
                        .excused
                        .iter()
                        .map(|excused| count(text, excused))
                        .sum::<usize>()
            };
            let mut in_enforcement = 0;
            for (path, text) in &sources {
                if path.ends_with("enforcement.rs") {
                    in_enforcement += net(text);
                } else {
                    assert!(
                        path.ends_with(rule.home) || net(text) == 0,
                        "{}: {} (`{}`) outside its home module and the \
                         chokepoint -- a second enforcement path",
                        path.display(),
                        rule.what,
                        rule.needle
                    );
                }
            }
            assert_eq!(
                in_enforcement, rule.in_enforcement,
                "exact {} census in the chokepoint",
                rule.what
            );
        }
        // The principal-keyed admission query backs no path outside the
        // table's own module (it is the documented Phase-2 seam).
        for (path, text) in &sources {
            assert!(
                path.ends_with("grants.rs") || count(text, &admission_query) == 0,
                "{}: the principal-keyed admission query must not back any path \
                 outside grants.rs",
                path.display()
            );
        }

        // (7) The human screenshot key's encoder (WS-E.2.4, issue #216),
        // censused in **both** directions so the one-path property's wording
        // above stays literally true.
        //
        // Forward: it is a sibling of `render_frame`, not a caller, so it must
        // have exactly one non-test caller outside `capture.rs` -- the drain in
        // `crate::screenshot` -- and zero occurrences here. A chokepoint that
        // grew a call to it would be an enforcement path producing a file
        // nobody judged; a second caller elsewhere would be a second way pixels
        // leave this process.
        //
        // Backward: `screenshot.rs` must not name the chokepoint's entry point.
        // "A human's screenshot never touches a grant" is the whole of issue
        // #216's title, and the honest way to keep a negative claim true is to
        // count the identifier rather than to re-read the file.
        let screenshot_encoder = format!("render_{}", "screenshot");
        let use_entry = format!("enforce_{}", "use");
        let mut encoder_callers = 0;
        for (path, text) in &sources {
            let hits = count(text, &screenshot_encoder);
            if path.ends_with("capture.rs") {
                continue;
            }
            assert!(
                !path.ends_with("enforcement.rs") || hits == 0,
                "the screenshot encoder must never be reachable from the enforcement \
                 chokepoint: a human holds no grant, and a picture is not a use"
            );
            encoder_callers += hits;
        }
        assert_eq!(
            encoder_callers, 1,
            "the screenshot encoder has exactly one non-test caller outside \
             capture.rs, and it is the human screenshot drain"
        );
        // **Positive controls first, because this half of the census errs OPEN.**
        // The loop below proves a negative over whichever files match a name,
        // so a rename makes it prove the negative over nothing and pass. Two
        // ways that happens and both are anticipated by the code it guards:
        // `screenshot.rs` growing into `screenshot/mod.rs` (its own docs plan
        // for a region and a window variant, and `Path::ends_with` matches
        // whole components, so the directory form would NOT match), and the
        // chokepoint's entry point being renamed out from under `use_entry`.
        // The forward half of this test already establishes the pattern 140
        // lines up; this half did not have it.
        let screenshot_sources: Vec<_> = sources
            .iter()
            .filter(|(path, _)| {
                path.ends_with("screenshot.rs")
                    || path.ends_with("screenshot/mod.rs")
                    || path
                        .parent()
                        .is_some_and(|parent| parent.ends_with("screenshot"))
            })
            .collect();
        assert!(
            !screenshot_sources.is_empty(),
            "the screenshot census matched no file: it proves a negative over whatever it \
             scans, so scanning nothing is a silent pass. Widen the match if the module was \
             renamed or split."
        );
        assert!(
            sources.iter().any(|(_, text)| count(text, &use_entry) > 0),
            "the `{use_entry}` needle matched nowhere in the crate, so asserting it is ABSENT \
             from the screenshot path proves nothing. The chokepoint's entry point was \
             probably renamed."
        );
        for (path, text) in &screenshot_sources {
            assert_eq!(
                count(text, &use_entry),
                0,
                "the human screenshot path must not name the enforcement entry \
                 point ({}): it holds no grant and has no principal to judge",
                path.display()
            );
            assert_eq!(
                count(text, &table_query),
                0,
                "the human screenshot path must not query the grant table ({})",
                path.display()
            );
        }

        // (6) The attention event's **delivery filter** (WS-E.1.7), excluded
        // from the enforcement path **by name rather than by accident**: it is
        // a query about who to *tell*, not about who may *act*, and the one
        // thing that would quietly turn it into a second authority site is a
        // call from inside the chokepoint. So the census is exact in both
        // directions -- zero here, exactly one non-test caller outside the
        // grant table, which is `session::open_attention_window`.
        let delivery_filter = format!("holds_{}", "verb");
        let mut filter_callers = 0;
        for (path, text) in &sources {
            let hits = count(text, &delivery_filter);
            if path.ends_with("grants.rs") {
                continue;
            }
            assert!(
                !path.ends_with("enforcement.rs") || hits == 0,
                "the attention delivery filter must never be called from the \
                 enforcement chokepoint: it says who to TELL, never who may ACT"
            );
            filter_callers += hits;
        }
        assert_eq!(
            filter_callers, 1,
            "the attention delivery filter has exactly one non-test caller \
             outside the grant table"
        );
    }

    #[test]
    fn single_path_scan_catches_the_documented_evasion_shapes() {
        // The tripwire's own regression tests: the constructions a
        // call-shaped pattern (`.name(`, `events::Name {`) would miss
        // must all count as occurrences, and the truncation must stop
        // only at the trailing tests module, not at a mid-file
        // `#[cfg(test)]` item.
        let needle = format!("check_use{}", "_grant");
        // Method call, UFCS, fn-pointer take, plain import, renamed
        // import: one occurrence each.
        for line in [
            "let v = table.check_use_grant(row, p, verb, now);",
            "let v = GrantTable::check_use_grant(&table, row, p, verb, now);",
            "let f = GrantTable::check_use_grant;",
            "use crate::grants::GrantTable::check_use_grant;",
            "use crate::grants::check_use_grant as q;",
        ] {
            assert_eq!(count(line, &needle), 1, "must catch: {line}");
        }
        // Whole-identifier discipline: a longer identifier is not the
        // guarded one, and comment lines stay prose.
        assert_eq!(count("fn check_use_granted() {}", &needle), 0);
        assert_eq!(count("// mentions check_use_grant only", &needle), 0);
        let shorter = format!("check_{}", "use");
        assert_eq!(
            count("table.check_use_grant(row, p, verb, now);", &shorter),
            0,
            "`check_use_grant` must not match the `check_use` needle"
        );
        assert_eq!(count("table.check_use(p, realm, verb, now);", &shorter), 1);

        // Truncation: a bare #[cfg(test)] item mid-file does not end the
        // scan (production code after it stays visible); the trailing
        // tests module does, with or without a visibility prefix.
        let attr = format!("#[cfg({})]", "test");
        let mid_file = format!(
            "impl S {{\n{attr}\n    fn helper() {{}}\n    fn prod(t: &T) {{ t.check_use_grant(); }}\n}}\n"
        );
        assert_eq!(
            count(&mid_file, &needle),
            1,
            "mid-file cfg(test) must not blind the scan"
        );
        for module in ["mod tests {", "pub(crate) mod tests {"] {
            let tail = format!("fn prod() {{}}\n{attr}\n{module}\n    fn t(x: &X) {{ x.check_use_grant(); }}\n}}\n");
            assert_eq!(
                count(&tail, &needle),
                0,
                "the trailing tests module is excluded"
            );
        }
    }
}
