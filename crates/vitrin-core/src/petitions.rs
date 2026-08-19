// SPDX-License-Identifier: MPL-2.0
//! The grant request flow (P1.4.3, issue #27): the petition lifecycle state
//! machine of the capability kernel -- `request_grant` to pending entry, the
//! consent-policy seam, and resolution toward an active [`GrantTable`] row
//! (minted at delivery, on the petitioner's connection -- see below).
//!
//! [`PetitionRegistry`] owns every **pending** petition in the core, across
//! all principal connections, because the IDL's admission policy crosses
//! connections: "pending-petition admission is capped per verified identity,
//! across all of that identity's connections", plus a global ceiling on
//! concurrent-plus-queued prompts. The per-connection halves of the flow --
//! wire decode, the multi-`new_id` rule, the petition-rate ceiling, the
//! live-object cap, and event emission -- live in
//! [`crate::principal::PrincipalServer`], which is also the **single
//! emission path** for petition-lifecycle events: every resolution, whether
//! decided here at admission or later (scripted injector, timeout), is
//! delivered through `PrincipalServer::deliver_resolution`, whose
//! pending-to-resolved flip enforces the IDL's "resolved fires exactly once
//! ever" as a state transition, not a convention -- and which mints the
//! grant-table row of a granted verdict at that same instant (see the
//! delivery decision below). On the registry side the
//! same invariant holds by construction: resolving **removes** the pending
//! entry, so a petition cannot resolve twice any more than a moved value
//! can be moved twice.
//!
//! # Decisions this task settles
//!
//! **Consent timeout: 120 seconds, configurable
//! ([`PetitionConfig::consent_timeout`]).** Two minutes matches the
//! elevation-prompt convention (Windows UAC and sudo-style askpass prompts
//! time out on the order of one to two minutes): long enough for a human to
//! actually read a first-contact consent prompt -- the prompt is the
//! security surface, and a rushed prompt breeds reflexive clicking -- and
//! short enough that an abandoned prompt does not pin the identity's
//! admission slot (and, once E7 renders prompts, the physical-input grab)
//! indefinitely. The deadline is half-open and fail-closed exactly like
//! grant expiry in [`crate::grants`]: a petition still pending at
//! `admitted_at + timeout` (`now >= deadline`) resolves `timed_out`.
//!
//! **Duplicate concurrent petitions do not coalesce; the valve is the
//! admission cap resolving `busy`.** The IDL prescribes the mechanism
//! verbatim ("excess petitions resolve busy") and coalescing would break
//! two of its invariants: each petition mints its own five wire objects and
//! owes its own exactly-once `resolved` terminal, so folding two petitions
//! onto one decision would either double-fire one `resolved` or leave one
//! petition unresolved -- a hang, which denial explicitly must never be.
//! Nor are two look-alike petitions semantically one: the human may
//! legitimately approve the first and deny a second asked minutes later.
//! Caps: **4 pending per verified identity** (a compliant Phase-1 agent has
//! at most one in flight; 4 tolerates a re-petition racing a timeout
//! without letting one credential queue a prompt storm) and **16 pending
//! globally** (more prompts than a human can meaningfully answer is
//! consent fatigue by definition). Both configurable
//! ([`PetitionConfig`]); both refusals are `busy`, the IDL's one
//! consent-fatigue outcome.
//!
//! **Server default event-rate ceiling: 20 events/second.** The wire's
//! `max_event_rate = 0` means "the server's default ceiling -- never
//! unlimited"; 20/s is the PRD's own example grant constraint (Doc 2
//! section 5.2 usage example) and rows never state "unlimited"
//! ([`crate::grants::Constraints::max_event_rate`] is non-zero by type).
//! The default is resolved to a concrete ceiling **at admission**, so a
//! pending petition already carries the effective rate its row will state.
//!
//! **Auto-approve grants exactly what was petitioned.** Under
//! [`ConsentPolicy::AutoApprove`] (the walking skeleton and headless CI;
//! plan risk R6) the effective authority equals the requested authority
//! with wire defaults resolved (`expiry_ms = 0` to no time bound,
//! `max_event_rate = 0` to the default ceiling). Narrowing is what the
//! consent surface is *for*; a policy that has no human has nobody to
//! choose a narrower shape on the human's behalf. Every auto-approval is
//! loudly logged (`tracing::warn!`) per approval, on top of the startup
//! warning the `--consent=auto-approve` flag emits.
//!
//! **A granted decision becomes a grant-table row only at delivery.**
//! Every consent path (auto-approve, scripted, and E7's human approval
//! when it lands) produces an [`ApprovedGrant`] -- the effective authority
//! plus everything the row states -- and the row is inserted by
//! `PrincipalServer::deliver_resolution`, in the same single-threaded step
//! that flips the wire grant handle and emits `resolved(granted)`.
//! Decision-time insertion would open a window (decision made, delivery
//! not yet run) in which the table holds an active row the connection's
//! teardown cannot see: teardown removes rows by scanning *resolved*
//! grant handles, and a handle flips only at delivery, so a connection
//! dying inside that window would orphan live authority -- violating the
//! IDL's "all of the principal's grants die with the connection"
//! (`vitrin_principal`). Minting at delivery closes the window by
//! construction: authority exists if and only if its wire handle
//! resolved, the teardown scan is complete, and an approval whose
//! petitioner disconnected while the decision was in flight evaporates
//! without ever creating a row. The wire cannot tell the difference:
//! "granted" is announced by `resolved`, and any facet use before that
//! refuses `not_granted` at the chokepoint either way.
//!
//! **Interactive policy pends for a human decision.**
//! [`ConsentPolicy::Interactive`] admits the petition into the pending
//! table and emits `vitrin_consent.state(queued)` -- "waiting behind ... a
//! policy decision" -- and it leaves the table by exactly one of three
//! doors: [`PetitionRegistry::resolve_human`] (P1.7.2's consent prompt, the
//! production path), [`PetitionRegistry::expire_due`] (the consent
//! timeout), or [`PetitionRegistry::withdraw_connection`] (the petitioner
//! disconnected). Fail-closed by construction in every direction: a
//! petition nobody answers resolves `timed_out`, and nothing but an
//! affirmative human decision produces a grant.
//!
//! The embedder that raises those prompts is now wired (issue #90):
//! [`crate::session::service_consent_round`] runs once per dispatch round on
//! the nested backend, raising the front pending petition through
//! [`crate::consent::grab::ConsentGrab::raise`] and routing a human's click
//! into [`PetitionRegistry::resolve_human`]. Under `interactive` a petition a
//! human never answers still resolves `timed_out` -- fail-closed -- but that
//! is now the fallback rather than the only outcome. (`--headless` has no
//! display and no physical input device, so it is refused with
//! `--consent=interactive` at startup; auto-approve is the headless policy.)
//!
//! **The scripted-consent injector is a build-gated hook, not a policy and
//! not a wire message.** The IDL is explicit that consent decisions are not
//! protocol-expressible; tests approve/deny/narrow specific pending
//! petitions through [`PetitionRegistry::resolve_scripted`], compiled only
//! under `cfg(test)` or the `scripted-consent` cargo feature. A deployment
//! build cannot even name the symbol. Scripted approval may **narrow** the
//! requested authority (fewer verbs, a shorter rung, a tighter expiry) --
//! exactly what `resolved`'s effective arguments exist to carry -- and a
//! decision that would *widen* it is refused typed, fail-closed, with the
//! petition left pending.
//!
//! **Timeout driving: injected time, embedder-polled.** Like
//! [`crate::grants`], this module never reads a clock; every operation
//! takes `now`, and expiry is a poll rather than a self-driven alarm:
//! [`PetitionRegistry::expire_due`] returns the resolutions whose deadline
//! passed and the embedder -- the runtime's armed calloop timer,
//! `session::sweep` -- routes each to its connection's
//! `deliver_resolution`. The timer needs no
//! precision guarantee: a late poll only stretches a prompt's life, never
//! resurrects a resolved petition.
//!
//! **Withdrawal on disconnect.** A pending petition is withdrawn when its
//! connection closes ("consent is in-context, so the prompt disappears with
//! the petitioner"): connection teardown calls
//! [`PetitionRegistry::withdraw_connection`], dropping every pending entry
//! of that connection with no events (the connection that could carry them
//! is gone; `resolved` then never fires for those petitions, which is the
//! documented withdrawal semantics, not a violated exactly-once). The
//! same teardown removes the connection's granted rows from the
//! [`GrantTable`] -- see `PrincipalServer::teardown`, which owns the
//! whole contract. A resolution already produced but not yet delivered
//! when the connection dies is phase-refused at delivery and dropped
//! whole -- safe, because its row was never minted (previous paragraph).
//!
//! **The `consent_held` mapping (P1.4.4, issue #28), absent a renderer.**
//! The IDL keys the chokepoint's `consent_held` refusal on "the
//! principal's own pending petition **has a prompt up**" -- prompt states
//! are `queued`, `shown`, `closed`, and "up" is exactly **`shown`**
//! ("visible; physical input is grabbed by the prompt"): the hold exists
//! because the human is *actively deciding about this principal*, which a
//! merely `queued` petition -- waiting behind another prompt or a policy
//! decision, nothing on screen, no input grab -- does not entail. So the
//! registry tracks a per-petition `prompt_shown` flag: E7's renderer
//! (P1.7.2) calls [`PetitionRegistry::mark_prompt_shown`] when it
//! composites the prompt (the same moment it emits
//! `vitrin_consent.state(shown)` through the connection's emission path),
//! and the prompt is *down* when the petition leaves the pending table
//! (resolution or withdrawal -- the moments that emit/imply `closed`) **or
//! when the prompt is taken off screen with the petition still pending**,
//! which is [`PetitionRegistry::mark_prompt_hidden`].
//!
//! That second clearing path was missing when P1.7.2 first landed, and its
//! absence was a live defect rather than a tidiness one: the only way to
//! advance a prompt queue is `lower` then `raise`
//! ([`crate::consent::grab::ConsentGrab`]), and with no way to clear the
//! flag the *lowered* petition stayed marked `shown` while a different
//! petition's card was on screen. `prompt_up_for` then kept answering
//! `true` for the first principal, so the chokepoint refused its
//! actuations `consent_held` -- citing a prompt no human could see -- for
//! up to the full [`PetitionConfig::consent_timeout`]. Fail-closed, so
//! never an authority leak, but a silent two-minute denial of authority the
//! core had already granted. `shown` and `hidden` are therefore a matched
//! pair, and `lower` calls the second exactly as `raise` calls the first.
//!
//! The chokepoint asks [`PetitionRegistry::prompt_up_for`], keyed by
//! verified **identity** (the IDL says "the principal's own ... other
//! principals' grants are unaffected", and a principal spanning several
//! connections is one principal -- the same keying as the admission cap).
//! P1.7.2 inherited those semantics for free, exactly as predicted:
//! [`crate::consent::grab::ConsentGrab::raise`] calls `mark_prompt_shown`
//! in the same statement sequence that puts the pixels up and seizes the
//! input grab, and **nothing in the chokepoint changed**. The flag is
//! reachable at runtime now that [`crate::session::service_consent_round`]
//! raises a prompt each dispatch round (issue #90).
//!
//! **Prompt selection is FIFO by admission** ([`PetitionRegistry::front_pending`]).
//! Up to [`PetitionConfig::max_pending_global`] petitions can pend at once,
//! and something has to decide which one the human is looking at. The
//! pending table is a `BTreeMap` keyed by a monotonically increasing
//! [`PetitionId`], so "lowest id" is "admitted earliest" is a total,
//! deterministic order that needs no extra bookkeeping -- and the oldest
//! petition is also the one closest to its consent deadline, so serving it
//! first is what keeps a queue from timing out from the back. The embedder
//! raises `front_pending`, and advances by `lower` then `raise` again.
//! Deliberately **not** `pending_ids`: that one is build-gated to tests and
//! the `scripted-consent` feature, so a deployment build could not have
//! enumerated the table at all.
//!
//! **Realm knowledge is the realm registry's, not this module's** (P1.5.1,
//! issue #30). Admission asks [`RealmRegistry::resolve_for_petition`]
//! whether the name the handle was minted with denotes a petitionable
//! realm, and refuses `unavailable` when it does not -- the IDL's "unknown
//! **or vacant**" pair, deliberately indistinguishable to the client
//! because realm absence is a race, not a protocol error. This module
//! judges no realm facts of its own: which realms exist comes from
//! `realm.toml`, and whether a realm is vacant is
//! [`crate::realm::Realm::admits_petitions`], where P1.5.2/P1.5.3 add
//! lifecycle without touching admission. (Before P1.5.1 this was a
//! hardcoded comparison against the well-known name.) Surface vacancy
//! remains a use-time `no_surface` concern (P1.4.4), never a petition-time
//! lie: a configured realm whose app has not painted is addressable, and
//! authority over it is inert rather than absent.
//!
//! **Policy-check order at admission** (documented determinism):
//! `unavailable` (addressing: which realm) precedes `unsupported` (policy:
//! flags, unserved verb bits, durable rung, resource granularity)
//! precedes `busy` (admission).
//! Petitions refused by any of these never enter the pending table, never
//! consume an admission slot, and emit **no** consent transitions -- no
//! prompt lifecycle ever began (the IDL's busy/unavailable flows show
//! exactly this shape).

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use vitrin_protocol::generated::vitrin_grant::{Outcome, Persistence as WirePersistence, Verb};

use crate::consent::{Choice, PromptContent};
use crate::grants::{
    GrantId, GrantSpec, GrantTable, InsertError, Issuer, PersistenceRung, RealmId, ResourceRef,
};
use crate::identity::PrincipalIdentity;
use crate::realm::RealmRegistry;

/// A core-global, never-reused identifier for one accepted principal
/// connection, minted by [`PetitionRegistry::register_connection`]. The
/// embedder maps it back to the live connection when routing a deferred
/// [`Resolution`]; it is transport bookkeeping, never authority (grant rows
/// deliberately carry no connection field -- see [`crate::grants`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ConnectionId(u64);

impl ConnectionId {
    /// Test-only: a connection id without a registry, so unit tests of pure
    /// consumers (the flight recorder's entry shapes) need not stand one
    /// up. Outside `cfg(test)` ids are minted by
    /// [`PetitionRegistry::register_connection`] and nowhere else.
    #[cfg(test)]
    pub fn from_u64_for_test(raw: u64) -> Self {
        Self(raw)
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn-{}", self.0)
    }
}

/// A registry-assigned, never-reused identifier for one pending petition.
/// Distinct from every wire id (those are connection-scoped and
/// client-allocated) and from [`GrantId`] (a petition has no grant row
/// unless and until it resolves granted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PetitionId(u64);

impl fmt::Display for PetitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "petition-{}", self.0)
    }
}

/// The consent-policy seam (PRD Doc 2 section 5.3): who answers a petition
/// that survives admission. The E7 consent renderer (issues #37-#39) plugs
/// into [`ConsentPolicy::Interactive`] when it lands; the build-gated
/// scripted injector ([`PetitionRegistry::resolve_scripted`]) is a hook
/// over pending petitions, deliberately not a policy variant -- gating the
/// *hook* keeps deployment builds from representing it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsentPolicy {
    /// Petitions pend for a human decision. Until E7 renders prompts, a
    /// pending petition resolves only by the consent timeout -- fail-closed:
    /// no consent surface means nothing is granted.
    Interactive,
    /// Every admitted petition is granted as requested, immediately --
    /// the walking skeleton and headless CI (plan risk R6). Explicitly
    /// flagged (`--consent=auto-approve`) and loudly logged, at startup and
    /// per grant.
    AutoApprove,
}

/// Deployment-tunable petition policy. Defaults are the settled values in
/// the module docs.
///
/// `run_session` constructs the registry with [`Self::default`]: no CLI
/// surface overrides these yet, so `consent_timeout` is 120 s in every
/// shipped session. A `--consent-timeout` flag is the obvious next step and
/// is what an *integration* test of the timeout against the shipped binary
/// would need; the in-process tests inject a short one by building the
/// registry directly.
#[derive(Debug, Clone)]
pub(crate) struct PetitionConfig {
    /// How long a pending petition may wait for consent before resolving
    /// `timed_out` (half-open: expired at `admitted_at + consent_timeout`).
    pub consent_timeout: Duration,
    /// Pending-petition admission cap per verified identity, across all of
    /// that identity's connections; excess resolves `busy`.
    pub max_pending_per_identity: usize,
    /// Global ceiling on concurrent-plus-queued prompts; excess resolves
    /// `busy`.
    pub max_pending_global: usize,
    /// The concrete ceiling behind the wire's `max_event_rate = 0`
    /// ("server default, never unlimited"), in events per second.
    pub default_max_event_rate: NonZeroU32,
}

impl Default for PetitionConfig {
    fn default() -> Self {
        Self {
            consent_timeout: Duration::from_secs(120),
            max_pending_per_identity: 4,
            max_pending_global: 16,
            default_max_event_rate: NonZeroU32::new(20).expect("20 is non-zero"),
        }
    }
}

/// One decoded, wire-valid petition as the connection server hands it to
/// admission: grammar, the non-zero-verb rule, the multi-`new_id` rule, and
/// the per-connection resource bounds have already been enforced (those are
/// fatal, connection-scoped, and belong to `PrincipalServer`); everything
/// *policy* -- realm existence, flags, rungs, granularity, admission caps,
/// consent -- is decided here, so the petition-policy razor lives in
/// exactly one place.
#[derive(Debug, Clone)]
pub(crate) struct PetitionRequest {
    pub connection: ConnectionId,
    /// The verifier-canonical identity bound to the petitioning connection
    /// (the admission cap keys on it) -- never claimed client text.
    pub identity: PrincipalIdentity,
    /// The name the realm handle was minted with (`get_realm`), unchecked
    /// at mint time by design ("naming is not authority").
    pub realm_name: String,
    /// The co-minted grant handle's wire id on the petitioning connection.
    pub grant_wire_id: u32,
    /// The co-minted consent observer's wire id.
    pub consent_wire_id: u32,
    /// The wire `resource` selector (empty = whole realm; string bound
    /// already enforced by decode).
    pub resource: String,
    /// The requested verb set (non-zero; wire-enforced).
    pub verbs: Verb,
    /// Requested lifetime in milliseconds; 0 = bounded by the rung.
    pub expiry_ms: u32,
    /// Requested ceiling in events per second; 0 = server default.
    pub max_event_rate: u32,
    /// The requested persistence rung, as the wire states it (durable
    /// rungs are policy-refused here, not wire errors).
    pub persistence: WirePersistence,
    /// The reserved constraint bits (any set bit is policy-refused
    /// `unsupported`, deliberately never a protocol error).
    pub flags: u32,
}

/// The requested authority of a pending petition, wire defaults resolved
/// at admission (rate 0 -> default ceiling) so approval narrows from a
/// concrete shape.
#[derive(Debug, Clone, Copy)]
struct Requested {
    verbs: Verb,
    persistence: PersistenceRung,
    expiry_ms: u32,
    max_event_rate: NonZeroU32,
}

/// One pending petition: everything resolution needs, held until consent,
/// timeout, or withdrawal.
#[derive(Debug)]
struct PendingPetition {
    connection: ConnectionId,
    identity: PrincipalIdentity,
    realm: RealmId,
    grant_wire_id: u32,
    consent_wire_id: u32,
    requested: Requested,
    /// `admitted_at + consent_timeout`; expired (fail-closed, half-open)
    /// once `now >= deadline`.
    deadline: Instant,
    /// Whether this petition's consent prompt is currently on screen
    /// (module docs: the `consent_held` mapping). Set by
    /// [`PetitionRegistry::mark_prompt_shown`] -- E7's renderer when it
    /// lands, tests today -- and implicitly cleared when the petition
    /// leaves the pending table.
    prompt_shown: bool,
    /// The petitioned realm's program, kept **only** when the petition
    /// names `realm_launch` (WS-E.1.1, issue #207): what the consent card
    /// has to show, because approving that verb approves starting *this*
    /// program.
    ///
    /// Resolved here, at admission, from the same [`RealmRegistry`] lookup
    /// that decided the realm exists -- so [`Self::prompt_content`] stays a
    /// pure function of pending state and the renderer never gains a second
    /// route from configuration to the screen. `None` for every other
    /// petition, so no card names a program the human is not being asked
    /// about.
    launch_command: Option<crate::realm::AuditedCommand>,
}

/// The effective authority a granted resolution carries -- what the
/// consent decision actually chose, possibly narrower than requested, and
/// exactly what `vitrin_grant.resolved` echoes and the grant row states.
/// `expiry_ms` stays wire-shaped (`0` = bounded by the rung) so the
/// resolved event never needs a lossy conversion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EffectiveAuthority {
    pub verbs: Verb,
    pub persistence: PersistenceRung,
    pub expiry_ms: u32,
    /// Stated on the row for the chokepoint's token bucket (P1.4.4);
    /// deliberately not echoed in `resolved` (the IDL: throttling is
    /// discovered operationally).
    pub max_event_rate: NonZeroU32,
}

/// One approved-but-not-yet-active grant: the consent decision's full
/// output, carried by [`Verdict::Granted`] in place of a row id. The
/// grant-table row is deliberately **not** minted at decision time --
/// `PrincipalServer::deliver_resolution` inserts it in the same
/// single-threaded step that flips the wire handle (module docs: a
/// decision-time row would be invisible to the teardown scan until
/// delivery, and a connection dying in that window would orphan live
/// authority).
#[derive(Debug, Clone)]
pub(crate) struct ApprovedGrant {
    /// The petitioner's verifier-canonical identity: the row's
    /// `principal_id`.
    pub identity: PrincipalIdentity,
    /// The petitioned realm: the row's `realm_id`.
    pub realm: RealmId,
    /// What the consent decision actually chose (possibly narrower than
    /// requested): echoed by `resolved` and stated on the row.
    pub effective: EffectiveAuthority,
    /// Which consent path decided: the row's provenance column.
    pub issuer: Issuer,
}

impl ApprovedGrant {
    /// Write this approval's row at the delivery instant `now`: the single
    /// decision-to-row conversion, shared by every consent path
    /// (auto-approve, scripted; E7's human approval joins it) and called
    /// from the single delivery site, so effective-to-row conversion can
    /// never fork. Fallible only as defense in depth: every decision was
    /// validated before it became an approval.
    pub fn insert_row(
        &self,
        grants: &mut GrantTable,
        now: Instant,
    ) -> Result<GrantId, InsertError> {
        let expiry = match self.effective.expiry_ms {
            0 => None,
            ms => Some(Duration::from_millis(u64::from(ms))),
        };
        grants.insert(
            GrantSpec {
                principal_id: self.identity.clone(),
                realm_id: self.realm.clone(),
                resource_ref: ResourceRef::WholeRealm,
                verbs: self.effective.verbs,
                expiry,
                max_event_rate: self.effective.max_event_rate,
                persistence: self.effective.persistence,
                issuer: self.issuer,
            },
            now,
        )
    }
}

/// How one petition resolved.
#[derive(Debug, Clone)]
pub(crate) enum Verdict {
    /// Approved: `resolved(granted, ...)` carries `grant.effective`, and
    /// the delivery step mints the row -- authority is active as of
    /// delivery, never before.
    Granted { grant: ApprovedGrant },
    /// Any non-`granted` outcome; `resolved` carries zeroed trailing
    /// arguments. Constructed only by this module, never with
    /// [`Outcome::Granted`].
    Declined { outcome: Outcome },
}

/// One petition resolution, ready for delivery on the petitioner's
/// connection: the registry decides, `PrincipalServer::deliver_resolution`
/// speaks. `emit_closed` says whether a consent lifecycle existed for this
/// petition (admission-refused petitions never began one, so they emit no
/// consent transitions at all).
#[derive(Debug, Clone)]
pub(crate) struct Resolution {
    pub connection: ConnectionId,
    pub grant_wire_id: u32,
    pub consent_wire_id: u32,
    pub emit_closed: bool,
    pub verdict: Verdict,
}

/// What admission decided: resolved on the spot (policy refusal or
/// auto-approve) or pending for a consent decision.
#[derive(Debug)]
pub(crate) enum Admission {
    Resolved(Resolution),
    Pending { petition: PetitionId },
}

/// Where one pending petition's consent-prompt events must be sent: the
/// petitioner's connection and the `vitrin_consent` object it minted.
///
/// Returned by [`PetitionRegistry::pending_route`] so the consent surface
/// (P1.7.2) can emit `vitrin_consent.state(shown)` when it raises a prompt
/// without holding a reference to the petition table's private rows. The
/// registry decides; `PrincipalServer` speaks -- the same division every
/// other petition event already follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PromptRoute {
    pub connection: ConnectionId,
    pub consent_wire_id: u32,
}

/// Why [`PetitionRegistry::resolve_human`] refused a decision taken on the
/// consent prompt. Both cases leave the petition exactly as it was --
/// fail-closed, the same posture the scripted injector takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanDecisionError {
    /// No pending petition with this id: it timed out, was withdrawn with
    /// its connection, or was already decided. The exactly-once guard,
    /// typed -- and the reason a decision drained from the grab carries its
    /// petition id rather than being applied to whatever is pending now.
    NotPending,
    /// The chosen rung would outlive the petitioned one. Unreachable
    /// through the prompt (it only draws rungs that
    /// [`PersistenceRung::narrows`] admits, the same predicate checked
    /// here), so this is the fail-closed backstop against a UI bug rather
    /// than a reachable outcome.
    WouldWiden,
}

impl fmt::Display for HumanDecisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HumanDecisionError::NotPending => f.write_str("no such pending petition"),
            HumanDecisionError::WouldWiden => {
                f.write_str("the chosen rung outlives the petitioned one")
            }
        }
    }
}

/// A decision injected through the build-gated scripted-consent hook: what
/// the consent surface (or the auto-approve policy) would have decided,
/// driven programmatically by a test harness.
#[cfg(any(test, feature = "scripted-consent"))]
#[derive(Debug, Clone, Copy)]
pub(crate) enum ScriptedDecision {
    /// Approve with this effective authority, which MUST NOT widen the
    /// petition: `verbs` a non-empty subset of the requested set,
    /// `persistence` no longer-lived than the requested rung, `expiry_ms`
    /// no later than the requested bound (`0` = keep rung-bounded, legal
    /// only when the petition itself was rung-bounded).
    Approve {
        verbs: Verb,
        persistence: PersistenceRung,
        expiry_ms: u32,
    },
    /// The human said no.
    Deny,
}

/// Why [`PetitionRegistry::resolve_scripted`] refused a scripted decision.
/// Every case leaves the petition exactly as it was (pending petitions stay
/// pending) -- fail-closed against harness bugs.
#[cfg(any(test, feature = "scripted-consent"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptedError {
    /// No pending petition with this id (never admitted, already resolved,
    /// or withdrawn) -- the exactly-once guard, typed.
    NotPending,
    /// The decision would widen or void the petitioned authority; the
    /// detail names the offending dimension.
    InvalidDecision(&'static str),
}

#[cfg(any(test, feature = "scripted-consent"))]
impl fmt::Display for ScriptedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptedError::NotPending => f.write_str("no such pending petition"),
            ScriptedError::InvalidDecision(detail) => {
                write!(f, "scripted decision refused: {detail}")
            }
        }
    }
}

/// The core-global pending-petition table and consent-policy seam. One
/// instance per core process, beside the one [`GrantTable`]; the
/// per-connection `PrincipalServer`s feed it admissions and deliver its
/// resolutions.
#[derive(Debug)]
pub(crate) struct PetitionRegistry {
    policy: ConsentPolicy,
    config: PetitionConfig,
    /// Pending petitions, keyed by ascending admission order (ids are
    /// minted monotonically), which makes expiry and enumeration
    /// deterministic.
    pending: BTreeMap<PetitionId, PendingPetition>,
    next_petition_id: u64,
    next_connection_id: u64,
}

impl PetitionRegistry {
    pub fn new(policy: ConsentPolicy, config: PetitionConfig) -> Self {
        Self {
            policy,
            config,
            pending: BTreeMap::new(),
            next_petition_id: 1,
            next_connection_id: 1,
        }
    }

    /// Mint a never-reused id for one accepted principal connection (the
    /// embedder calls this at accept and hands the id to
    /// `PrincipalServer::new`).
    pub fn register_connection(&mut self) -> ConnectionId {
        let id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        id
    }

    /// Admit one wire-valid petition: run the policy checks in the
    /// documented order (`unavailable` -> `unsupported` -> `layout_held`
    /// -> `busy`), then
    /// either resolve on the spot (policy refusal, or auto-approve
    /// approval) or enter it pending for a consent decision. Infallible by
    /// construction: admission decides, it never writes the grant table --
    /// rows are minted at delivery (module docs).
    ///
    /// `realms` is the core's realm registry (P1.5.1), consulted for the
    /// addressing check only; this module holds no realm state of its own.
    ///
    /// `layout_arrange_held` is the grant table's answer to
    /// [`GrantTable::any_live_holder_of`], resolved by the caller for the
    /// same reason `enforcement::UseEnv::realm_view` and `grant_realm` are:
    /// this module holds no grant state, and taking the table as a parameter
    /// here would give petition admission a second view of authority beside
    /// the one the chokepoint queries.
    ///
    /// [`GrantTable::any_live_holder_of`]: crate::grants::GrantTable::any_live_holder_of
    pub fn admit(
        &mut self,
        req: PetitionRequest,
        now: Instant,
        realms: &RealmRegistry,
        layout_arrange_held: bool,
    ) -> Admission {
        let declined = |outcome: Outcome| {
            Admission::Resolved(Resolution {
                connection: req.connection,
                grant_wire_id: req.grant_wire_id,
                consent_wire_id: req.consent_wire_id,
                // Refused at admission: no consent lifecycle ever began.
                emit_closed: false,
                verdict: Verdict::Declined { outcome },
            })
        };

        // Addressing first: which realm is being asked about. Realm absence
        // is a race, not a protocol error (IDL), and unknown and vacant are
        // one answer here by design -- the registry collapses them so a
        // petitioner cannot probe which realms merely exist.
        let Some(realm) = realms.resolve_for_petition(&req.realm_name).cloned() else {
            return declined(Outcome::Unavailable);
        };

        // Policy refusals, each "honest refusal rather than
        // accepted-and-unenforced" (IDL): a set reserved flag bit, a
        // durable rung without provenance, a resource granularity finer
        // than this core serves, a defined verb this core does not serve.
        if req.flags != 0 {
            return declined(Outcome::Unsupported);
        }
        // A verb bit the IDL defines but this core does not enforce --
        // today exactly `observe_cursor` (D-017). `layout_arrange` and
        // `layout_focus` left that set at WS-E.1.4 and `realm_launch` at
        // WS-E.1.1, each when the core gained the mechanism its refusal
        // stood for. Refused **whole**, never narrowed to the served
        // remainder: narrowing is the human's move at consent time, and
        // silently dropping a requested verb would leave the agent
        // believing it holds authority nothing checks. The wire keeps
        // these bits in range precisely so this is a recoverable
        // `unsupported` rather than a fatal `invalid_argument`.
        //
        // This also subsumes, today, the one verb-composition rule the IDL
        // states: `observe_cursor` is meaningful only alongside `observe`,
        // and naming it alone resolves `unsupported`. This core refuses
        // `observe_cursor` in every combination, so no separate check can
        // be distinguished from this one -- whoever begins to serve the
        // verb (M2) must add the pairing check here, because deleting the
        // bit from `UNSERVED_VERB_BITS` deletes the only thing enforcing it.
        if req.verbs.bits() & crate::grants::UNSERVED_VERB_BITS != 0 {
            return declined(Outcome::Unsupported);
        }
        let Ok(rung) = PersistenceRung::try_from(req.persistence) else {
            return declined(Outcome::Unsupported);
        };
        if !req.resource.is_empty() {
            return declined(Outcome::Unsupported);
        }

        // D-018(4)'s single-holder rule: at most one live grant carries
        // `layout_arrange` per output, and there is exactly one output.
        //
        // **Ordered after `unsupported` and before `busy`**, and neither
        // neighbour is arbitrary. After `unsupported`, because a deployment
        // that does not serve the verb has no holders to contend with and
        // must answer that it does not serve it -- telling a client the
        // authority is *taken* when it is not even offered would send it
        // back to poll forever. Before `busy`, because `busy` is the
        // consent-fatigue valve and this petition is never going to reach a
        // prompt: it costs the human nothing, so it must not be refused for
        // the human's sake.
        //
        // **Admission-time, not use-time.** Contention here is about who
        // *holds* the authority, not about one use of it; a use-time answer
        // would let two principals both believe they hold arrangement and
        // discover otherwise one request at a time.
        //
        // Both halves are checked. A live row is the holder proper; a
        // *pending* petition naming the verb is a holder-in-waiting, and
        // omitting it would let two petitions race through a human's two
        // approvals and mint two holders -- which is exactly the state this
        // rule exists to make unreachable. The pending half is this
        // module's own state; the live half is the caller's answer from the
        // grant table.
        if req.verbs.contains(Verb::LAYOUT_ARRANGE)
            && (layout_arrange_held
                || self
                    .pending
                    .values()
                    .any(|p| p.requested.verbs.contains(Verb::LAYOUT_ARRANGE)))
        {
            return declined(Outcome::LayoutHeld);
        }

        // Admission caps: per verified identity across all its connections,
        // and the global prompt ceiling. Excess resolves busy -- the
        // consent-fatigue valve -- and consumes nothing.
        if self.pending_for_identity(&req.identity) >= self.config.max_pending_per_identity
            || self.pending.len() >= self.config.max_pending_global
        {
            return declined(Outcome::Busy);
        }

        let requested = Requested {
            verbs: req.verbs,
            persistence: rung,
            expiry_ms: req.expiry_ms,
            max_event_rate: self.resolve_rate(req.max_event_rate),
        };

        match self.policy {
            ConsentPolicy::AutoApprove => {
                // Effective = requested, verbatim (module docs): narrowing
                // is the human's move, and this policy has no human.
                let effective = EffectiveAuthority {
                    verbs: requested.verbs,
                    persistence: requested.persistence,
                    expiry_ms: requested.expiry_ms,
                    max_event_rate: requested.max_event_rate,
                };
                // The loud per-approval log the auto-approve policy owes
                // (plan risk R6), beside the startup warning. No row id
                // yet: the row is minted at delivery (module docs), so
                // this names the decision, not the row.
                tracing::warn!(
                    identity = %req.identity,
                    verbs = effective.verbs.bits(),
                    "consent AUTO-APPROVE: petition granted without a human decision"
                );
                Admission::Resolved(Resolution {
                    connection: req.connection,
                    grant_wire_id: req.grant_wire_id,
                    consent_wire_id: req.consent_wire_id,
                    // The policy stands in for the prompt; announce its
                    // close (the IDL allows only-closed or nothing).
                    emit_closed: true,
                    verdict: Verdict::Granted {
                        grant: ApprovedGrant {
                            identity: req.identity,
                            realm,
                            effective,
                            issuer: Issuer::AutoApprovePolicy,
                        },
                    },
                })
            }
            ConsentPolicy::Interactive => {
                let petition = PetitionId(self.next_petition_id);
                self.next_petition_id += 1;
                // The one configuration fact a card may name, resolved here
                // and only for the verb it belongs to (WS-E.1.1). Asked of
                // the same registry that already said this realm exists, so
                // there is one lookup and one authority for what the realm
                // runs. Unreachable `None` for a resolved realm; a card that
                // could not name the program simply does not claim to.
                let launch_command = if requested.verbs.contains(Verb::REALM_LAUNCH) {
                    realms
                        .get(realm.as_str())
                        .map(|r| r.spawn().audited_command())
                } else {
                    None
                };
                // Fail-closed on an unrepresentable deadline (absurd
                // configured timeout): collapse to immediate expiry, never
                // to a never-expiring prompt.
                let deadline = now.checked_add(self.config.consent_timeout).unwrap_or(now);
                self.pending.insert(
                    petition,
                    PendingPetition {
                        connection: req.connection,
                        identity: req.identity,
                        realm,
                        grant_wire_id: req.grant_wire_id,
                        consent_wire_id: req.consent_wire_id,
                        requested,
                        deadline,
                        // Born queued; the renderer (P1.7.2) flips this
                        // when the prompt actually reaches the screen.
                        prompt_shown: false,
                        launch_command,
                    },
                );
                Admission::Pending { petition }
            }
        }
    }

    /// Resolve `timed_out` every pending petition whose deadline has
    /// passed at `now` (half-open, fail-closed: expired at exactly the
    /// deadline). Returns the resolutions in admission order; the embedder
    /// routes each to its connection's `deliver_resolution`.
    pub fn expire_due(&mut self, now: Instant) -> Vec<Resolution> {
        let due: Vec<PetitionId> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(&id, _)| id)
            .collect();
        due.into_iter()
            .map(|id| {
                let p = self
                    .pending
                    .remove(&id)
                    .expect("due id was just observed in the map");
                Resolution {
                    connection: p.connection,
                    grant_wire_id: p.grant_wire_id,
                    consent_wire_id: p.consent_wire_id,
                    // A consent lifecycle existed (the petition pended);
                    // close it before the terminal.
                    emit_closed: true,
                    verdict: Verdict::Declined {
                        outcome: Outcome::TimedOut,
                    },
                }
            })
            .collect()
    }

    /// Withdraw every pending petition of one closing connection, with no
    /// events (module docs: the prompt disappears with the petitioner).
    /// Returns how many petitions were withdrawn, for the teardown log.
    pub fn withdraw_connection(&mut self, connection: ConnectionId) -> usize {
        let before = self.pending.len();
        self.pending.retain(|_, p| p.connection != connection);
        before - self.pending.len()
    }

    /// Concurrent-plus-queued prompt count (the global ceiling's measure;
    /// the E7 prompt queue reads the same number).
    pub fn pending_total(&self) -> usize {
        self.pending.len()
    }

    /// Record that `petition`'s consent prompt is now on screen (module
    /// docs: the `consent_held` mapping). [`ConsentGrab::raise`] calls this
    /// at the same moment it emits `vitrin_consent.state(shown)`, driven at
    /// runtime by [`crate::session::service_consent_round`] (issue #90).
    /// Returns whether the petition was pending (a resolved or withdrawn
    /// petition has no prompt to show -- `false`, nothing recorded).
    pub fn mark_prompt_shown(&mut self, petition: PetitionId) -> bool {
        match self.pending.get_mut(&petition) {
            Some(pending) => {
                pending.prompt_shown = true;
                true
            }
            None => false,
        }
    }

    /// Record that `petition`'s prompt is no longer on screen **while the
    /// petition is still pending** -- the matched counterpart of
    /// [`Self::mark_prompt_shown`], called by
    /// [`crate::consent::grab::ConsentGrab::lower`].
    ///
    /// Needed because `shown` is not only ended by resolution. A prompt
    /// queue advances by lowering one card and raising the next, and the
    /// lowered petition goes back to being merely `queued` -- nothing on
    /// screen, no input grab, so by the IDL's own definition no prompt is
    /// "up" for it and `consent_held` must stop holding. Without this the
    /// flag was write-once and the first petition of any queue kept
    /// refusing its principal's actuations until it timed out (module
    /// docs).
    ///
    /// Returns whether the petition was pending. `false` -- a petition that
    /// already left the table -- is the ordinary resolution path and not an
    /// error: the entry that carried the flag is gone, so there is nothing
    /// left to clear.
    pub fn mark_prompt_hidden(&mut self, petition: PetitionId) -> bool {
        match self.pending.get_mut(&petition) {
            Some(pending) => {
                pending.prompt_shown = false;
                true
            }
            None => false,
        }
    }

    /// When `petition` resolves `timed_out` if no human answers it
    /// (`admitted_at + consent_timeout`), or `None` once it is no longer
    /// pending.
    ///
    /// Read by [`crate::consent::grab::ConsentGrab::raise`] so the grab
    /// carries its own prompt's expiry: a grab that outlived its petition
    /// would hold the human's physical input hostage with nothing behind
    /// it, and the grab must be able to let go on its own rather than
    /// trusting every embedder to notice. The deadline travels *into* the
    /// grab rather than the grab asking the registry per event, because the
    /// router's hook point holds no registry borrow.
    pub fn pending_deadline(&self, petition: PetitionId) -> Option<Instant> {
        Some(self.pending.get(&petition)?.deadline)
    }

    /// The petition whose prompt should be shown next: the lowest pending
    /// id, i.e. the earliest admitted (module docs: prompt selection is
    /// FIFO by admission, and `PetitionId`s are handed out monotonically
    /// into a `BTreeMap`, so this is a total deterministic order).
    ///
    /// Ungated on purpose, unlike `pending_ids` (build-gated to tests and
    /// the `scripted-consent` feature): without this a deployment build
    /// could learn a `PetitionId` only from [`Admission::Pending`] at admit
    /// time and would have to keep a shadow queue with its own ordering --
    /// an invitation for the order the human sees to diverge from the order
    /// the core admitted. It exposes nothing [`Self::pending_total`] and
    /// [`Self::prompt_content`] do not.
    pub fn front_pending(&self) -> Option<PetitionId> {
        self.pending.keys().next().copied()
    }

    /// What the consent prompt for `petition` may say (P1.7.1, E7).
    ///
    /// **The only non-test constructor of [`PromptContent`]**, which is what
    /// makes "the prompt renders core-validated data, never free client text"
    /// (the IDL's `vitrin_consent` page) structural rather than a rule
    /// someone has to keep applying: the renderer has no way to obtain
    /// content that did not come out of a pending petition, and the type it
    /// receives has no free-text field to smuggle anything through (see
    /// [`crate::consent`]).
    ///
    /// Note which realm value travels: `self.realm` is the **registry's**
    /// [`RealmId`], resolved at admission from `realm.toml`, not the name the
    /// client minted its realm handle with. The requested name is never
    /// stored, so it can never reach a screen.
    ///
    /// `None` when the petition is not pending -- resolved, timed out, or
    /// withdrawn. A prompt for a petition that no longer exists must not be
    /// renderable at all, which is the same exactly-once property removal
    /// from this table already gives every other consumer.
    pub fn prompt_content(&self, petition: PetitionId) -> Option<PromptContent> {
        let pending = self.pending.get(&petition)?;
        Some(PromptContent {
            principal: pending.identity.clone(),
            realm: pending.realm.clone(),
            verbs: pending.requested.verbs,
            persistence: pending.requested.persistence,
            expiry_ms: pending.requested.expiry_ms,
            // Resolved at admission, from the registry, for `realm_launch`
            // petitions only -- see [`PendingPetition::launch_command`].
            command: pending.launch_command.clone(),
        })
    }

    /// Where `petition`'s consent-prompt events go: its petitioner's
    /// connection and consent object (P1.7.2). `None` once the petition is
    /// no longer pending -- a `state` event for a resolved petition would
    /// violate the IDL's "every `state` is delivered before `resolved`",
    /// and this returning `None` is what makes that unrepresentable rather
    /// than merely avoided.
    pub fn pending_route(&self, petition: PetitionId) -> Option<PromptRoute> {
        let pending = self.pending.get(&petition)?;
        Some(PromptRoute {
            connection: pending.connection,
            consent_wire_id: pending.consent_wire_id,
        })
    }

    /// Whether this verified identity currently has a consent prompt **on
    /// screen** for any of its pending petitions, across all of its
    /// connections -- the `consent_held` judgement the enforcement
    /// chokepoint (P1.4.4) consults for actuations (module docs: `shown`
    /// holds, `queued` does not, and resolution/withdrawal ends the hold
    /// by removing the entry).
    pub fn prompt_up_for(&self, identity: &PrincipalIdentity) -> bool {
        self.pending
            .values()
            .any(|p| p.prompt_shown && p.identity == *identity)
    }

    /// **The production consent path** (P1.7.2): resolve one pending
    /// petition with the decision a human took on the core-rendered prompt.
    ///
    /// This is the sibling of [`resolve_scripted`](Self::resolve_scripted),
    /// not a caller of it: that injector is compiled only under `cfg(test)`
    /// or the `scripted-consent` feature precisely so a deployment build
    /// cannot represent it, and routing real human decisions through it
    /// would defeat that -- as well as recording every human approval under
    /// [`Issuer::ScriptedConsent`], a lie in exactly the log a session is
    /// reconstructed from. Both paths share [`Self::finish`], so the
    /// pending-entry removal, the exactly-once property, and the
    /// [`Resolution`] shape cannot fork between them.
    ///
    /// # Why the human decision cannot widen anything
    ///
    /// A [`Choice`] carries a persistence rung and nothing else, and the
    /// effective authority is assembled here from the **petition's own**
    /// stored request: the verb set, the expiry, and the resolved rate
    /// ceiling are copied verbatim from what was petitioned. There is no
    /// argument through which a caller could ask for more, so the only
    /// dimension a decision can move is the rung -- and that is checked
    /// against [`PersistenceRung::narrows`], the same predicate
    /// [`crate::consent::PromptContent::choices`] filters the buttons with.
    /// The prompt therefore cannot draw a button this refuses, and this
    /// cannot accept a rung the prompt would not draw.
    ///
    /// Narrowing the *verbs* is not offered in the MVP: the card lists the
    /// requested verbs and the buttons choose a rung. Per-verb approval is
    /// a real want, and the machinery already carries it (the effective
    /// authority is a full [`EffectiveAuthority`], and `resolved` echoes
    /// it) -- it needs prompt affordances, not a state-machine change.
    ///
    /// `Deny` resolves `denied` with zeroed effective arguments: a clean
    /// protocol terminal on the petitioner's grant handle, which is what
    /// makes refusal a *decision the agent observes* rather than a hang.
    pub fn resolve_human(
        &mut self,
        petition: PetitionId,
        choice: Choice,
    ) -> Result<Resolution, HumanDecisionError> {
        let pending = self
            .pending
            .get(&petition)
            .ok_or(HumanDecisionError::NotPending)?;
        let approved = match choice {
            Choice::Deny => None,
            Choice::Allow(rung) => {
                let requested = pending.requested;
                if !rung.narrows(requested.persistence) {
                    return Err(HumanDecisionError::WouldWiden);
                }
                Some(EffectiveAuthority {
                    // Verbatim from the petition: the decision chooses a
                    // rung, never a wider authority (doc comment above).
                    verbs: requested.verbs,
                    persistence: rung,
                    expiry_ms: requested.expiry_ms,
                    max_event_rate: requested.max_event_rate,
                })
            }
        };
        // Loud by policy, like every other authority-changing decision: a
        // human's yes or no is the most consequential event in a session,
        // and the flight-recorder entry follows at delivery.
        tracing::info!(
            identity = %pending.identity,
            realm = %pending.realm,
            granted = approved.is_some(),
            "consent decision taken on the prompt"
        );
        Ok(self.finish(petition, approved, Issuer::HumanConsent))
    }

    /// Consume `petition` from the pending table and build its
    /// [`Resolution`] -- the one place a decided petition stops being
    /// pending, shared by every decision path that is not admission itself.
    ///
    /// Removal is what makes "resolved fires exactly once ever" structural
    /// on this side: a petition cannot resolve twice any more than a moved
    /// value can be moved twice. Callers validate *first* and call this
    /// last, so a refused decision leaves the petition pending.
    ///
    /// # Panics
    ///
    /// Panics if `petition` is not pending. Callers reach this only after
    /// a successful lookup in the same `&mut self` borrow, so the entry
    /// cannot have gone away in between.
    fn finish(
        &mut self,
        petition: PetitionId,
        approved: Option<EffectiveAuthority>,
        issuer: Issuer,
    ) -> Resolution {
        let p = self
            .pending
            .remove(&petition)
            .expect("caller validated the entry in this same borrow");
        let verdict = match approved {
            Some(effective) => Verdict::Granted {
                grant: ApprovedGrant {
                    identity: p.identity,
                    realm: p.realm,
                    effective,
                    issuer,
                },
            },
            None => Verdict::Declined {
                outcome: Outcome::Denied,
            },
        };
        Resolution {
            connection: p.connection,
            grant_wire_id: p.grant_wire_id,
            consent_wire_id: p.consent_wire_id,
            // A consent lifecycle existed (the petition pended), so its
            // close is announced before the terminal.
            emit_closed: true,
            verdict,
        }
    }

    /// This identity's pending-petition count across all its connections.
    fn pending_for_identity(&self, identity: &PrincipalIdentity) -> usize {
        self.pending
            .values()
            .filter(|p| p.identity == *identity)
            .count()
    }

    /// The wire's `0 = server default, never unlimited` resolved to a
    /// concrete ceiling.
    fn resolve_rate(&self, requested: u32) -> NonZeroU32 {
        NonZeroU32::new(requested).unwrap_or(self.config.default_max_event_rate)
    }

    /// Build-gated: pending petition ids in admission order, so a harness
    /// can name the petition it is about to decide. Deployment builds have
    /// [`Self::front_pending`] instead, which is all a prompt queue needs.
    #[cfg(any(test, feature = "scripted-consent"))]
    pub fn pending_ids(&self) -> Vec<PetitionId> {
        self.pending.keys().copied().collect()
    }

    /// The build-gated scripted-consent injector (module docs): decide one
    /// pending petition programmatically, exactly as the consent surface
    /// would. Approval validates that the decision only narrows the
    /// petitioned authority; any refusal leaves the petition pending. The
    /// returned resolution *carries* an approval rather than a row: the
    /// grant-table row is minted at delivery, not here (module docs), so a
    /// resolution the embedder can no longer deliver leaves no authority
    /// behind.
    #[cfg(any(test, feature = "scripted-consent"))]
    pub fn resolve_scripted(
        &mut self,
        petition: PetitionId,
        decision: ScriptedDecision,
    ) -> Result<Resolution, ScriptedError> {
        let pending = self
            .pending
            .get(&petition)
            .ok_or(ScriptedError::NotPending)?;
        let approved = match decision {
            ScriptedDecision::Deny => None,
            ScriptedDecision::Approve {
                verbs,
                persistence,
                expiry_ms,
            } => {
                let requested = pending.requested;
                if verbs.bits() == 0 {
                    return Err(ScriptedError::InvalidDecision(
                        "effective verb set is empty",
                    ));
                }
                if !requested.verbs.contains(verbs) {
                    return Err(ScriptedError::InvalidDecision(
                        "effective verbs outside the petitioned set",
                    ));
                }
                // The shared narrowing rule (`PersistenceRung::narrows`), not
                // a local comparison: the consent prompt decides which
                // allow-buttons to draw with the same predicate, so it can
                // never offer a rung this refuses.
                if !persistence.narrows(requested.persistence) {
                    return Err(ScriptedError::InvalidDecision(
                        "persistence rung longer-lived than petitioned",
                    ));
                }
                if requested.expiry_ms != 0 && (expiry_ms == 0 || expiry_ms > requested.expiry_ms) {
                    return Err(ScriptedError::InvalidDecision(
                        "expiry later than the petitioned bound",
                    ));
                }
                Some(EffectiveAuthority {
                    verbs,
                    persistence,
                    expiry_ms,
                    max_event_rate: requested.max_event_rate,
                })
            }
        };
        // Only now consume the entry: a refused decision above left the
        // petition pending, and removal there is what makes a second
        // resolution structurally impossible. Shared with the production
        // human path (`resolve_human`) so the two cannot fork -- only the
        // issuer differs, and it differs honestly.
        Ok(self.finish(petition, approved, Issuer::ScriptedConsent))
    }
}

// ---------------------------------------------------------------------------
// Tests (registry-level; the wire-level lifecycle tests live in
// `crate::principal`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn identity(s: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(s).unwrap()
    }

    const DEMO: &str = "vitrin://local/agent/demo";

    fn registry(policy: ConsentPolicy) -> PetitionRegistry {
        PetitionRegistry::new(policy, PetitionConfig::default())
    }

    /// The realm registry admission consults: one configured realm under
    /// the well-known default id, exactly what a minimal `realm.toml`
    /// produces.
    fn realms() -> RealmRegistry {
        crate::realm::tests::registry_with(&[crate::realm::WELL_KNOWN_REALM_ID])
    }

    /// A wire-valid whole-realm petition for `who`, with wire ids picked by
    /// the caller (arbitrary at this level -- object rules are the
    /// connection server's).
    fn request(who: &str, connection: ConnectionId, grant_wire_id: u32) -> PetitionRequest {
        PetitionRequest {
            connection,
            identity: identity(who),
            realm_name: crate::realm::WELL_KNOWN_REALM_ID.into(),
            grant_wire_id,
            consent_wire_id: grant_wire_id + 1,
            resource: String::new(),
            verbs: Verb::OBSERVE | Verb::ACTUATE_TEXT,
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: WirePersistence::WhileRunning,
            flags: 0,
        }
    }

    fn expect_declined(admission: Admission, want: Outcome) -> Resolution {
        match admission {
            Admission::Resolved(res) => {
                match res.verdict {
                    Verdict::Declined { outcome } => assert_eq!(outcome, want),
                    other => panic!("expected Declined({want:?}), got {other:?}"),
                }
                assert!(
                    !res.emit_closed,
                    "admission refusals never began a consent lifecycle"
                );
                res
            }
            Admission::Pending { .. } => panic!("expected an immediate {want:?} resolution"),
        }
    }

    #[test]
    fn policy_checks_run_in_the_documented_order() {
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();

        // Unknown realm wins over everything else on the same petition.
        let mut req = request(DEMO, conn, 10);
        req.realm_name = "realm-9".into();
        req.flags = 1;
        req.persistence = WirePersistence::Always;
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unavailable);

        // Reserved flags before rung: both set, flags is checked first --
        // but both are the same outcome, so assert each in isolation.
        let mut req = request(DEMO, conn, 20);
        req.flags = 0b1;
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);
        let mut req = request(DEMO, conn, 30);
        req.persistence = WirePersistence::UntilRevoked;
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);
        let mut req = request(DEMO, conn, 40);
        req.resource = "surface:main".into();
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);

        // None of the refusals entered the pending table (and admission
        // never writes the grant table at all -- rows are minted at
        // delivery).
        assert_eq!(reg.pending_total(), 0);
    }

    #[test]
    fn verbs_defined_but_unserved_resolve_unsupported_never_narrowed() {
        // D-017/D-018: a verb is on the wire so that petitioning for it is
        // an answer rather than a connection death, and this core refuses
        // the ones it does not implement. The failure this guards is the
        // silent one: accepting the bit and enforcing nothing.
        //
        // **Two bits now**, and every movement in or out was a decision
        // rather than a subtraction: WS-E.1.4 (issue #210) served the layout
        // pair, and WS-E.1.1 (issue #207) served `realm_launch` once the core
        // gained the spawn path, the realm cap and the consent copy its
        // absence used to stand for. `observe_cursor` remains, because
        // per-principal cursor delivery is still M2's. `egress` **joined**
        // at P2.7.2 (issue #196): the bit is on the wire so a petition for
        // it is answered rather than killed, and this core enforces nothing
        // by it — the mediating proxy is P2.7.3's — so admission refuses it,
        // whole. That is the fail-closed default working rather than a
        // separate rule: `UNSERVED_VERB_BITS` is derived from
        // `Verb::VALID_MASK`, so the new bit was unserved the moment the IDL
        // defined it, and `SERVED_VERB_BITS` was not touched.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::AutoApprove);
        let conn = reg.register_connection();

        let mut wire_id = 10;
        // **Derived from `UNSERVED_VERB_BITS`, not hand-listed.** This set has
        // shrunk three times (D-018's two verbs, then `realm_launch` at
        // WS-E.1.1), grown once (`egress` at P2.7.2) and will move again when
        // cursor delivery and the egress proxy land. A hand-written list would
        // keep passing while a newly appended, unclassified bit went
        // unexercised — the shape of failure this repo keeps finding — so the
        // loop reads the constant instead. The exact membership is pinned in
        // `consent::render`'s catalogue test, which is what forces a human to
        // classify a new bit; here the membership is only *used*.
        let unserved: Vec<Verb> = (0..32)
            .map(|shift| 1u32 << shift)
            .filter(|bit| crate::grants::UNSERVED_VERB_BITS & bit != 0)
            .map(|bit| Verb::from_bits(bit).expect("an unserved bit is a defined wire bit"))
            .collect();
        assert!(
            !unserved.is_empty(),
            "no unserved verb bits: this test would assert nothing"
        );
        for verb in unserved {
            // Alone.
            let mut req = request(DEMO, conn, wire_id);
            req.verbs = verb;
            expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);
            wire_id += 10;

            // Mixed with served verbs: refused WHOLE. Were it narrowed to
            // the served remainder, the agent would be granted `observe`
            // while believing it also holds `verb`.
            let mut req = request(DEMO, conn, wire_id);
            req.verbs = Verb::OBSERVE | verb;
            expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);
            wire_id += 10;
        }

        // The served set still admits under the same policy, so the check
        // above refuses the unserved bits and nothing else.
        let mut req = request(DEMO, conn, wire_id);
        req.verbs = Verb::OBSERVE
            | Verb::ACTUATE_POINTER
            | Verb::ACTUATE_TEXT
            | Verb::LAYOUT_ARRANGE
            | Verb::LAYOUT_FOCUS;
        let Admission::Resolved(res) = reg.admit(req, t0, &realms(), false) else {
            panic!("auto-approve must resolve a served petition");
        };
        assert!(matches!(res.verdict, Verdict::Granted { .. }));

        // No refusal entered the pending table.
        assert_eq!(reg.pending_total(), 0);
    }

    #[test]
    fn a_petition_naming_egress_resolves_unsupported_whole() {
        // P2.7.2 (issue #196), named separately from the derived sweep above
        // so the requirement is findable by the verb's name rather than only
        // by the constant it happens to be in today.
        //
        // The bit is on the wire (P2.7.2's IDL half) and this core serves
        // nothing by it: the mediating proxy that would ask the chokepoint per
        // connection is P2.7.3's, and the facet a connection would be asked
        // for through is P2.6.5's. Two halves, and both matter. In range, so
        // naming it is never a killed connection; unserved, so it is never
        // granted. A deployment MUST NOT grant a verb it does not enforce.
        assert_eq!(
            Verb::EGRESS.bits() & crate::grants::SERVED_VERB_BITS,
            0,
            "SERVED_VERB_BITS gained `egress` without a proxy to enforce it \
             or consent copy to name it (P2.7.3, P2.6.8's Q13 review)"
        );

        let t0 = t0();
        let mut reg = registry(ConsentPolicy::AutoApprove);
        let conn = reg.register_connection();

        // Alone.
        let mut req = request(DEMO, conn, 10);
        req.verbs = Verb::EGRESS;
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);

        // Mixed with the whole served set: refused WHOLE, never narrowed to
        // the served remainder. The IDL states this ("the server never quietly
        // drops an unserved bit and grants the remainder"), and a narrowing
        // core would hand the agent five verbs while it believed it also held
        // network egress.
        let mut req = request(DEMO, conn, 20);
        req.verbs = Verb::OBSERVE
            | Verb::ACTUATE_POINTER
            | Verb::ACTUATE_TEXT
            | Verb::LAYOUT_ARRANGE
            | Verb::LAYOUT_FOCUS
            | Verb::EGRESS;
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);

        // Positive control: the same petition without the bit is granted, so
        // the two refusals above are about `egress` and not about the shape of
        // the request.
        let mut req = request(DEMO, conn, 30);
        req.verbs = Verb::OBSERVE
            | Verb::ACTUATE_POINTER
            | Verb::ACTUATE_TEXT
            | Verb::LAYOUT_ARRANGE
            | Verb::LAYOUT_FOCUS;
        let Admission::Resolved(res) = reg.admit(req, t0, &realms(), false) else {
            panic!("auto-approve must resolve a served petition");
        };
        assert!(matches!(res.verdict, Verdict::Granted { .. }));

        // A `net:` selector does not rescue it either, and for a second,
        // independent reason: this core serves no resource granularity finer
        // than the whole realm, so any non-empty selector is `unsupported`
        // here. `ResourceRef::Net` exists and parses (`grants.rs`), and
        // **nothing in this admission path calls it yet** -- wiring it in is
        // P2.7.3/P2.7.4's. Asserted so that stays true by test rather than by
        // memory.
        let mut req = request(DEMO, conn, 40);
        req.verbs = Verb::EGRESS;
        req.resource = "net:example.com:443".into();
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Unsupported);

        assert_eq!(reg.pending_total(), 0);
    }

    #[test]
    fn admission_resolves_the_realm_name_against_the_registry() {
        // P1.5.1: the `unavailable` judgement is a registry lookup, not a
        // comparison against a well-known constant. A registry that serves
        // only "kiosk" admits "kiosk" and refuses "realm-0" -- the exact
        // inversion a hardcode could not produce.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let realms = crate::realm::tests::registry_with(&["kiosk"]);

        let mut req = request(DEMO, conn, 10);
        req.realm_name = "kiosk".into();
        let Admission::Pending { .. } = reg.admit(req, t0, &realms, false) else {
            panic!("a configured realm must admit petitions");
        };

        let mut req = request(DEMO, conn, 20);
        req.realm_name = crate::realm::WELL_KNOWN_REALM_ID.into();
        expect_declined(reg.admit(req, t0, &realms, false), Outcome::Unavailable);

        // An empty registry is the vacant case the IDL folds into the same
        // answer: every name resolves unavailable, none is distinguishable.
        let empty = crate::realm::tests::registry_with(&[]);
        let mut req = request(DEMO, conn, 30);
        req.realm_name = "kiosk".into();
        expect_declined(reg.admit(req, t0, &empty, false), Outcome::Unavailable);
        assert_eq!(reg.pending_total(), 1, "only the kiosk petition pended");
    }

    #[test]
    fn a_granted_petition_states_the_registrys_realm_id_on_its_row() {
        // The realm a grant attaches to is the registry's id, not a
        // constant: `realm:<id>` is the MVP's whole-realm grant scope.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::AutoApprove);
        let conn = reg.register_connection();
        let realms = crate::realm::tests::registry_with(&["kiosk"]);
        let mut req = request(DEMO, conn, 10);
        req.realm_name = "kiosk".into();

        let Admission::Resolved(res) = reg.admit(req, t0, &realms, false) else {
            panic!("auto-approve resolves immediately");
        };
        let Verdict::Granted { grant } = res.verdict else {
            panic!("auto-approve grants");
        };
        assert_eq!(grant.realm, RealmId::new("kiosk"));
        let mut grants = GrantTable::new();
        let row = grant.insert_row(&mut grants, t0).unwrap();
        assert_eq!(
            grants.get(row, t0).unwrap().0.realm_id,
            RealmId::new("kiosk")
        );
    }

    #[test]
    fn the_global_prompt_ceiling_refuses_busy_across_identities() {
        // The per-identity cap (4) never binds here; the global ceiling
        // (16) does: 4 identities x 4 pending fill it, and a fifth
        // identity's first petition -- which its own cap would admit --
        // resolves busy.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        for who in 0..4 {
            for i in 0..4 {
                let req = request(&format!("vitrin://local/agent/a{who}"), conn, 100 * who + i);
                assert!(matches!(
                    reg.admit(req, t0, &realms(), false),
                    Admission::Pending { .. }
                ));
            }
        }
        assert_eq!(reg.pending_total(), 16);
        let req = request("vitrin://local/agent/fifth", conn, 900);
        expect_declined(reg.admit(req, t0, &realms(), false), Outcome::Busy);
    }

    #[test]
    fn prompt_up_tracks_shown_not_queued_and_ends_with_the_petition() {
        // The consent_held mapping (module docs): only a SHOWN prompt
        // holds, keyed by identity across connections, and the hold ends
        // when the petition leaves the pending table.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn_a = reg.register_connection();
        let conn_b = reg.register_connection();

        let Admission::Pending { petition } =
            reg.admit(request(DEMO, conn_a, 10), t0, &realms(), false)
        else {
            panic!("interactive petition must pend");
        };
        // Queued is not up: nothing is on screen yet.
        assert!(!reg.prompt_up_for(&identity(DEMO)));

        assert!(reg.mark_prompt_shown(petition));
        assert!(reg.prompt_up_for(&identity(DEMO)));
        // Identity-keyed: another identity is unaffected, and the same
        // identity is held regardless of which connection asks.
        assert!(!reg.prompt_up_for(&identity("vitrin://local/agent/other")));

        // Resolution removes the entry: the prompt is down.
        let resolution = reg
            .resolve_scripted(petition, ScriptedDecision::Deny)
            .unwrap();
        assert_eq!(resolution.connection, conn_a);
        assert!(!reg.prompt_up_for(&identity(DEMO)));

        // Withdrawal ends the hold the same way (the prompt disappears
        // with the petitioner), and a gone petition cannot be marked.
        let Admission::Pending { petition } =
            reg.admit(request(DEMO, conn_b, 10), t0, &realms(), false)
        else {
            panic!("interactive petition must pend");
        };
        assert!(reg.mark_prompt_shown(petition));
        assert!(reg.prompt_up_for(&identity(DEMO)));
        assert_eq!(reg.withdraw_connection(conn_b), 1);
        assert!(!reg.prompt_up_for(&identity(DEMO)));
        assert!(!reg.mark_prompt_shown(petition));
    }

    #[test]
    fn hiding_a_prompt_ends_the_hold_without_resolving_the_petition() {
        // The matched counterpart of `mark_prompt_shown` (module docs).
        // Taking a card off screen while its petition is still pending
        // returns that petition to `queued`, so `consent_held` must stop
        // holding -- otherwise the queue-advance path (`lower` then
        // `raise`) refuses the first principal's actuations for a prompt
        // nobody can see, until the consent timeout.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let Admission::Pending { petition } =
            reg.admit(request(DEMO, conn, 10), t0, &realms(), false)
        else {
            panic!("interactive petition must pend");
        };

        assert!(reg.mark_prompt_shown(petition));
        assert!(reg.prompt_up_for(&identity(DEMO)));
        assert!(reg.mark_prompt_hidden(petition));
        assert!(
            !reg.prompt_up_for(&identity(DEMO)),
            "a hidden prompt is not up"
        );
        // The petition itself is untouched: still pending, still
        // answerable, still on its original deadline.
        assert_eq!(reg.pending_total(), 1);
        assert!(reg.prompt_content(petition).is_some());
        assert_eq!(
            reg.pending_deadline(petition),
            Some(t0 + PetitionConfig::default().consent_timeout)
        );

        // Re-raising the same petition holds again -- the flag is a state,
        // not a one-shot.
        assert!(reg.mark_prompt_shown(petition));
        assert!(reg.prompt_up_for(&identity(DEMO)));

        // And a petition that already left the table reports `false`
        // rather than pretending: the entry that carried the flag is gone.
        reg.resolve_scripted(petition, ScriptedDecision::Deny)
            .unwrap();
        assert!(!reg.mark_prompt_hidden(petition));
        assert_eq!(reg.pending_deadline(petition), None);
    }

    #[test]
    fn the_front_of_the_queue_is_the_earliest_admitted_petition() {
        // Prompt selection is FIFO by admission (module docs). Pinned
        // because the alternative -- an embedder-side shadow queue -- is
        // how the order the human sees drifts from the order the core
        // admitted, and because `front_pending` must stay ungated: it is
        // the ONLY way a deployment build can enumerate the table.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        assert_eq!(reg.front_pending(), None);

        let mut admitted = Vec::new();
        for i in 0..3 {
            let Admission::Pending { petition } =
                reg.admit(request(DEMO, conn, 10 + i), t0, &realms(), false)
            else {
                panic!("interactive petition must pend");
            };
            admitted.push(petition);
        }
        assert_eq!(reg.front_pending(), Some(admitted[0]));

        // Serving the front advances the queue in admission order, and the
        // order does not depend on which petition was shown.
        reg.resolve_scripted(admitted[0], ScriptedDecision::Deny)
            .unwrap();
        assert_eq!(reg.front_pending(), Some(admitted[1]));
        reg.resolve_scripted(admitted[1], ScriptedDecision::Deny)
            .unwrap();
        assert_eq!(reg.front_pending(), Some(admitted[2]));
        reg.resolve_scripted(admitted[2], ScriptedDecision::Deny)
            .unwrap();
        assert_eq!(reg.front_pending(), None);
    }

    /// **The card names the program a `realm_launch` approval would start,
    /// and only then** (WS-E.1.1, issue #207).
    ///
    /// Resolved here, at admission, from the same registry lookup that
    /// decided the realm exists — so the renderer never gains a second route
    /// from configuration to the screen, and a petition that does not ask to
    /// launch carries no program at all.
    #[test]
    fn only_a_launch_petition_carries_its_templates_command_to_the_prompt() {
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let realms =
            crate::realm::tests::registry_of(vec![crate::realm::tests::template_with_spawn(
                "kiosk",
                std::path::Path::new("/usr/bin/kiosk-browser"),
                &[],
            )]);

        let mut req = request(DEMO, conn, 10);
        req.realm_name = "kiosk".into();
        req.verbs = Verb::OBSERVE | Verb::REALM_LAUNCH;
        let Admission::Pending { petition } = reg.admit(req, t0, &realms, false) else {
            panic!("interactive petition must pend");
        };
        let prompt = reg.prompt_content(petition).expect("pending");
        assert_eq!(
            prompt
                .command
                .as_ref()
                .map(|c| c.as_path().display().to_string()),
            Some("/usr/bin/kiosk-browser".to_string()),
            "a launch prompt must name the program it would start"
        );

        // The same realm, without the verb: no program travels. A card that
        // named one anyway would be showing the human a fact they are not
        // being asked about.
        let mut req = request(DEMO, conn, 20);
        req.realm_name = "kiosk".into();
        req.verbs = Verb::OBSERVE;
        let Admission::Pending { petition } = reg.admit(req, t0, &realms, false) else {
            panic!("interactive petition must pend");
        };
        assert!(reg
            .prompt_content(petition)
            .expect("pending")
            .command
            .is_none());
    }

    #[test]
    fn prompt_content_comes_from_the_pending_petition_and_dies_with_it() {
        // P1.7.1: the consent renderer's only source of content. Three
        // things are pinned here because the prompt's integrity rests on
        // them.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let realms = crate::realm::tests::registry_with(&["kiosk"]);

        let mut req = request(DEMO, conn, 10);
        // The client names the realm "kiosk"; what the prompt shows must be
        // the REGISTRY's id, resolved from realm.toml -- the requested name
        // is never stored, so no client string can reach a screen.
        req.realm_name = "kiosk".into();
        req.verbs = Verb::OBSERVE;
        req.expiry_ms = 30_000;
        req.persistence = WirePersistence::Once;
        let Admission::Pending { petition } = reg.admit(req, t0, &realms, false) else {
            panic!("interactive petition must pend");
        };

        // 1. Every field is the petition's own, verbatim.
        let prompt = reg
            .prompt_content(petition)
            .expect("the petition is pending");
        assert_eq!(prompt.principal, identity(DEMO));
        assert_eq!(prompt.realm, RealmId::new("kiosk"));
        assert_eq!(prompt.verbs, Verb::OBSERVE);
        assert_eq!(prompt.persistence, PersistenceRung::Once);
        assert_eq!(prompt.expiry_ms, 30_000);

        // 2. The choices offered are the ones this petition can legally
        //    resolve to -- the registry's own narrowing rule decides, so the
        //    prompt cannot offer a button `resolve_scripted` would refuse.
        for choice in prompt.choices() {
            let crate::consent::Choice::Allow(rung) = choice else {
                continue;
            };
            assert!(
                reg.resolve_scripted(
                    petition,
                    ScriptedDecision::Approve {
                        verbs: prompt.verbs,
                        persistence: rung,
                        expiry_ms: prompt.expiry_ms,
                    },
                )
                .is_ok(),
                "the prompt offered {rung:?}, which this petition refuses"
            );
            // Re-admit for the next iteration: resolving consumed it.
            let mut again = request(DEMO, conn, 10);
            again.realm_name = "kiosk".into();
            again.persistence = WirePersistence::Once;
            let Admission::Pending { .. } = reg.admit(again, t0, &realms, false) else {
                panic!("re-admission must pend");
            };
        }

        // 3. A petition that is no longer pending has no prompt at all --
        //    the same exactly-once property removal gives every other
        //    consumer, so a decided petition cannot be re-rendered.
        let pending = reg.pending_ids();
        for id in &pending {
            reg.resolve_scripted(*id, ScriptedDecision::Deny).unwrap();
            assert!(reg.prompt_content(*id).is_none());
        }
        assert!(reg.prompt_content(PetitionId(u64::MAX)).is_none());
    }

    #[test]
    fn scripted_approval_must_not_widen_the_petition() {
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();

        // Petition: observe|text, while_running, bounded to 60s.
        let mut req = request(DEMO, conn, 10);
        req.expiry_ms = 60_000;
        let Admission::Pending { petition } = reg.admit(req, t0, &realms(), false) else {
            panic!("interactive petition must pend");
        };

        let widenings: [(ScriptedDecision, &str); 4] = [
            (
                ScriptedDecision::Approve {
                    verbs: Verb::ACTUATE_POINTER,
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 60_000,
                },
                "verbs outside the petitioned set",
            ),
            (
                ScriptedDecision::Approve {
                    verbs: Verb::default(),
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 60_000,
                },
                "empty effective verb set",
            ),
            (
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 90_000,
                },
                "expiry above the petitioned bound",
            ),
            (
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 0,
                },
                "unbounding a bounded petition",
            ),
        ];
        for (decision, label) in widenings {
            assert!(
                matches!(
                    reg.resolve_scripted(petition, decision),
                    Err(ScriptedError::InvalidDecision(_))
                ),
                "{label}: a widening decision must be refused"
            );
            // Fail-closed: the petition is still pending.
            assert_eq!(reg.pending_ids(), vec![petition], "{label}");
        }

        // A once-rung petition cannot be approved while_running.
        let mut once = request(DEMO, conn, 20);
        once.persistence = WirePersistence::Once;
        let Admission::Pending { petition: once_pet } = reg.admit(once, t0, &realms(), false)
        else {
            panic!("interactive petition must pend");
        };
        assert!(matches!(
            reg.resolve_scripted(
                once_pet,
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 0,
                },
            ),
            Err(ScriptedError::InvalidDecision(_))
        ));

        // And a valid narrowing (fewer verbs, shorter rung, tighter
        // expiry) resolves granted with exactly that effective shape --
        // and the delivery-time insert states the same shape on the row.
        let res = reg
            .resolve_scripted(
                petition,
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::Once,
                    expiry_ms: 30_000,
                },
            )
            .unwrap();
        let Verdict::Granted { grant } = res.verdict else {
            panic!("expected a granted verdict");
        };
        assert_eq!(grant.effective.verbs, Verb::OBSERVE);
        assert_eq!(grant.effective.persistence, PersistenceRung::Once);
        assert_eq!(grant.effective.expiry_ms, 30_000);
        assert_eq!(grant.issuer, Issuer::ScriptedConsent);
        let mut grants = GrantTable::new();
        let row = grant.insert_row(&mut grants, t0).unwrap();
        let (grant_row, _) = grants.get(row, t0).unwrap();
        assert_eq!(grant_row.verbs, Verb::OBSERVE);
        assert_eq!(grant_row.persistence, PersistenceRung::Once);
        assert_eq!(
            grant_row.constraints.expiry,
            Some(Duration::from_millis(30_000))
        );
    }

    #[test]
    fn a_human_approval_grants_exactly_the_petitioned_authority_at_the_chosen_rung() {
        // The production consent path (P1.7.2): a `Choice` carries only a
        // rung, and everything else on the resulting row is the petition's
        // own request, verbatim. Recorded as `human_consent`, never as the
        // scripted stand-in.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let mut req = request(DEMO, conn, 10);
        req.expiry_ms = 60_000;
        req.max_event_rate = 7;
        let Admission::Pending { petition } = reg.admit(req, t0, &realms(), false) else {
            panic!("interactive petition must pend");
        };

        let res = reg
            .resolve_human(petition, Choice::Allow(PersistenceRung::Once))
            .expect("the petition is pending");
        assert_eq!(res.connection, conn);
        assert!(res.emit_closed, "a prompt lifecycle existed");
        let Verdict::Granted { grant } = res.verdict else {
            panic!("Allow grants");
        };
        assert_eq!(grant.effective.verbs, Verb::OBSERVE | Verb::ACTUATE_TEXT);
        assert_eq!(grant.effective.persistence, PersistenceRung::Once);
        assert_eq!(grant.effective.expiry_ms, 60_000);
        assert_eq!(grant.effective.max_event_rate.get(), 7);
        assert_eq!(grant.issuer, Issuer::HumanConsent);

        // The row the delivery step mints states the same shape.
        let mut grants = GrantTable::new();
        let row = grant.insert_row(&mut grants, t0).unwrap();
        let (grant_row, _) = grants.get(row, t0).unwrap();
        assert_eq!(grant_row.persistence, PersistenceRung::Once);
        assert_eq!(grant_row.issuer, Issuer::HumanConsent);
    }

    #[test]
    fn a_human_denial_is_a_clean_declined_resolution() {
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let Admission::Pending { petition } =
            reg.admit(request(DEMO, conn, 10), t0, &realms(), false)
        else {
            panic!("interactive petition must pend");
        };
        let res = reg
            .resolve_human(petition, Choice::Deny)
            .expect("the petition is pending");
        assert!(matches!(
            res.verdict,
            Verdict::Declined {
                outcome: Outcome::Denied
            }
        ));
        assert!(res.emit_closed);
        assert_eq!(reg.pending_total(), 0, "the decision consumed the petition");
    }

    #[test]
    fn a_human_decision_resolves_exactly_once_and_cannot_widen() {
        // Two fail-closed guards on the production path. The widening one
        // is unreachable through the prompt (it only draws rungs that
        // narrow) and exists precisely so a UI bug cannot become an
        // authority bug.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let mut once = request(DEMO, conn, 10);
        once.persistence = WirePersistence::Once;
        let Admission::Pending { petition } = reg.admit(once, t0, &realms(), false) else {
            panic!("interactive petition must pend");
        };

        // A `once` petition cannot be approved `while_running`...
        assert_eq!(
            reg.resolve_human(petition, Choice::Allow(PersistenceRung::WhileRunning))
                .err(),
            Some(HumanDecisionError::WouldWiden)
        );
        // ...and the refusal left it pending, so the human can still answer.
        assert_eq!(reg.pending_ids(), vec![petition]);
        assert!(reg
            .resolve_human(petition, Choice::Allow(PersistenceRung::Once))
            .is_ok());

        // Exactly once: a second decision on the same petition -- a click
        // that raced a timeout, a decision drained twice -- finds nothing.
        assert_eq!(
            reg.resolve_human(petition, Choice::Deny).err(),
            Some(HumanDecisionError::NotPending)
        );
        assert_eq!(
            reg.resolve_human(PetitionId(u64::MAX), Choice::Deny).err(),
            Some(HumanDecisionError::NotPending)
        );
    }

    #[test]
    fn the_prompt_can_only_offer_rungs_the_human_path_accepts() {
        // The shared-predicate property, stated across the two modules that
        // must agree: every button `PromptContent::choices` draws is a
        // decision `resolve_human` accepts, for every representable rung.
        let t0 = t0();
        for requested in PersistenceRung::ALL {
            let mut reg = registry(ConsentPolicy::Interactive);
            let conn = reg.register_connection();
            for choice in {
                let mut req = request(DEMO, conn, 10);
                req.persistence = WirePersistence::from(requested);
                let Admission::Pending { petition } = reg.admit(req, t0, &realms(), false) else {
                    panic!("interactive petition must pend");
                };
                reg.prompt_content(petition).expect("pending").choices()
            } {
                let mut req = request(DEMO, conn, 20);
                req.persistence = WirePersistence::from(requested);
                let Admission::Pending { petition } = reg.admit(req, t0, &realms(), false) else {
                    panic!("interactive petition must pend");
                };
                assert!(
                    reg.resolve_human(petition, choice).is_ok(),
                    "the prompt offered {choice:?} on a {requested:?} petition, \
                     which the consent path refuses"
                );
            }
        }
    }

    #[test]
    fn pending_route_names_the_petitioner_and_dies_with_the_petition() {
        // Where `vitrin_consent.state(shown)` is sent -- and the reason the
        // IDL's "every state precedes resolved" holds structurally: once a
        // petition is resolved there is no route to send a state on.
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let conn = reg.register_connection();
        let Admission::Pending { petition } =
            reg.admit(request(DEMO, conn, 10), t0, &realms(), false)
        else {
            panic!("interactive petition must pend");
        };
        assert_eq!(
            reg.pending_route(petition),
            Some(PromptRoute {
                connection: conn,
                consent_wire_id: 11,
            })
        );
        reg.resolve_human(petition, Choice::Deny).unwrap();
        assert_eq!(reg.pending_route(petition), None);
    }

    #[test]
    fn withdrawal_drops_only_the_closing_connections_petitions() {
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::Interactive);
        let a = reg.register_connection();
        let b = reg.register_connection();
        assert_ne!(a, b, "connection ids are never reused");

        for wire in [10, 20] {
            assert!(matches!(
                reg.admit(request(DEMO, a, wire), t0, &realms(), false),
                Admission::Pending { .. }
            ));
        }
        assert!(matches!(
            reg.admit(request(DEMO, b, 10), t0, &realms(), false),
            Admission::Pending { .. }
        ));

        assert_eq!(reg.withdraw_connection(a), 2);
        assert_eq!(reg.pending_total(), 1);
        // Withdrawn petitions never time out (no ghost resolutions for a
        // dead connection); the survivor still does.
        let due = reg.expire_due(t0 + Duration::from_secs(3600));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].connection, b);
        // Idempotent.
        assert_eq!(reg.withdraw_connection(a), 0);
    }

    #[test]
    fn the_default_rate_ceiling_backs_a_zero_request() {
        let t0 = t0();
        let mut reg = registry(ConsentPolicy::AutoApprove);
        let conn = reg.register_connection();

        // rate 0 -> the documented 20/s default, on the approval and on
        // the row its delivery-time insert mints.
        let Admission::Resolved(res) = reg.admit(request(DEMO, conn, 10), t0, &realms(), false)
        else {
            panic!("auto-approve resolves immediately");
        };
        let Verdict::Granted { grant } = res.verdict else {
            panic!("auto-approve grants");
        };
        assert_eq!(grant.effective.max_event_rate.get(), 20);
        assert_eq!(grant.issuer, Issuer::AutoApprovePolicy);
        let mut grants = GrantTable::new();
        let row = grant.insert_row(&mut grants, t0).unwrap();
        let (grant_row, _) = grants.get(row, t0).unwrap();
        assert_eq!(grant_row.constraints.max_event_rate.get(), 20);

        // An explicit rate is honored verbatim.
        let mut req = request(DEMO, conn, 20);
        req.max_event_rate = 5;
        let Admission::Resolved(res) = reg.admit(req, t0, &realms(), false) else {
            panic!("auto-approve resolves immediately");
        };
        let Verdict::Granted { grant } = res.verdict else {
            panic!("auto-approve grants");
        };
        assert_eq!(grant.effective.max_event_rate.get(), 5);
    }
}
