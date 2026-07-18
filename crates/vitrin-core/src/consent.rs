//! The consent-decision seam and petition-admission policy of the
//! capability kernel (P1.4.3, issue #27): the [`ConsentDecider`] trait the
//! consent surface (E7, P1.7.x) will implement, the walking skeleton's
//! loudly-logged auto-approve decider, and the cross-connection
//! [`GrantKernel`] the petition flow in [`principal`](crate::principal)
//! consults and writes through.
//!
//! PRD Doc 2 section 5.3 puts the consent prompt *in the core* and the
//! decision *at the human*; this module is the boundary between the two.
//! The petition flow (the `request_grant` dispatch arm) asks the decider
//! for a verdict; the decider either answers immediately (a policy) or
//! holds the petition pending (a prompt), and a held petition completes
//! later through [`PrincipalServer::resolve_pending`] or expires through
//! [`PrincipalServer::expire_pending`] -- the *same* entry points the real
//! prompt renderer (P1.7.1) and its input routing (P1.7.2) will drive.
//! Consent decisions are deliberately not protocol-expressible (IDL,
//! `vitrin_consent`): nothing an agent sends can reach this seam.
//!
//! # Decisions this task settles
//!
//! **Pending-consent timeout: 120 seconds**
//! ([`PENDING_CONSENT_TIMEOUT`]). The IDL requires a timeout (`resolved`
//! outcome `timed_out`: "the consent prompt expired unanswered"); the value
//! is core policy. Two minutes is long enough that a human reads the
//! prompt and decides deliberately -- a consent prompt rushed by a short
//! fuse produces exactly the reflexive click-through the PRD's consent
//! model exists to avoid -- and short enough that an unattended prompt
//! releases what it holds: the identity's single admission slot (below),
//! and, once P1.7.2 lands, the prompt's exclusive input grab and the
//! `consent_held` actuation freeze. Interactive-auth precedent sits in the
//! same band (e.g. systemd's ask-password defaults to 90 s). Expiry is
//! fail-closed at the boundary, like every deadline in
//! [`grants`](crate::grants): a petition expires at *exactly*
//! `parked_at + PENDING_CONSENT_TIMEOUT`, and time is injected by the
//! caller (the event loop samples the clock and calls `expire_pending`;
//! no clock read hides in the flow).
//!
//! **Concurrent duplicate petitions do not coalesce -- they resolve
//! `busy`.** IDL-first: `vitrin_grant.outcome.busy` already defines the
//! behavior ("the pending-petition admission cap for this verified
//! identity, across all of its connections, was reached"), and the
//! `vitrin_realm` prose names it the consent-fatigue valve. Coalescing was
//! rejected outright: every petition co-mints its own grant handle whose
//! `resolved` fires exactly once, so "coalescing" could only mean answering
//! two grant handles from one human decision -- attributing consent the
//! human was never shown (the prompt renders *one* petition's verbs and
//! expiry). The caps: [`MAX_PENDING_PER_IDENTITY`] = 1 -- the consent
//! surface shows at most one prompt per identity, so a second concurrent
//! petition from the same verified identity (same *or different*
//! connection) resolves `busy` immediately -- and [`MAX_PENDING_GLOBAL`]
//! = 4, the deployment-level ceiling on concurrent-plus-queued prompts the
//! IDL requires. Petitioning again after a resolution (grant, denial,
//! timeout) is always legal and never coalesced with anything.
//!
//! **Default event-rate ceiling: 60 events/second**
//! ([`DEFAULT_MAX_EVENT_RATE`]). The wire defines `max_event_rate = 0` as
//! "the server's default ceiling -- never unlimited", and the grant table
//! stores only concrete ceilings ([`Constraints::max_event_rate`] is
//! `NonZeroU32`), so the petition flow resolves the default *before* the
//! row is written. 60/s is one event per frame at the 60 Hz compositor
//! cadence: enough for fluid pointer gestures and burst clicks
//! (move+press+release), low enough that a runaway agent cannot flood
//! faster than the screen can show. Enforcement is P1.4.4's token bucket;
//! the table only carries the number.
//!
//! **The decider chooses effective authority; the flow clamps it.** The
//! human may narrow a petition (fewer verbs, `Allow once` instead of
//! `Allow while running`, a shorter expiry) -- `resolved` carries the
//! effective values, IDL -- but consent can never *widen* one. The flow
//! intersects the decider's verbs with the petition's, downgrades a rung
//! the petition did not request, and caps the expiry at the requested
//! bound; a decision left empty by clamping resolves `denied`
//! (fail-closed: a buggy decider becomes a refusal, never authority the
//! petitioner did not ask for).
//!
//! # Scope seams (marked, not smuggled)
//!
//! - **P1.7.1 (prompt renderer)** implements [`ConsentDecider`] returning
//!   [`ConsentVerdict::Pending`], renders the core-drawn prompt from the
//!   [`PetitionSummary`] (all core-validated data: the verifier-canonical
//!   identity, parsed verbs, realm, expiry), and calls `resolve_pending`
//!   on the human's click. Emitting `vitrin_consent.state(shown)` when the
//!   prompt becomes visible -- and dismissing a live prompt whose
//!   petitioner's connection died -- are prompt-lifecycle mechanics that
//!   land with it.
//! - **P1.7.2 (consent input routing)** adds the exclusive input grab and
//!   the `consent_held` actuation refusal; the `--consent=auto-approve`
//!   runtime flag (plan risk R6: explicit flag, persistent warning,
//!   refuses a registry larger than the demo principal) wires
//!   [`AutoApproveDecider`] in.
//! - **Realm liveness** is a static set here ([`GrantKernel::new`]); the
//!   realm manager (P1.5.1) takes ownership of it -- including resolving
//!   `unavailable` for a realm that *closes while a petition is pending* --
//!   when realms grow lifecycle.
//!
//! [`PrincipalServer::resolve_pending`]: crate::principal::PrincipalServer::resolve_pending
//! [`PrincipalServer::expire_pending`]: crate::principal::PrincipalServer::expire_pending
//! [`Constraints::max_event_rate`]: crate::grants::Constraints::max_event_rate

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::time::Duration;

use vitrin_protocol::generated::vitrin_grant::Verb;

use crate::grants::{GrantTable, Issuer, PersistenceRung, RealmId};
use crate::identity::PrincipalIdentity;

/// How long a parked petition waits for the human before it resolves
/// `timed_out` (decision rationale in the module docs). Fail-closed
/// boundary: the petition expires at exactly `parked_at + this`.
pub(crate) const PENDING_CONSENT_TIMEOUT: Duration = Duration::from_secs(120);

/// The concrete ceiling substituted for a petition's `max_event_rate = 0`
/// ("server default, never unlimited" -- IDL). Rationale in the module
/// docs; enforced by P1.4.4's token bucket, stored by the grant table.
pub(crate) const DEFAULT_MAX_EVENT_RATE: NonZeroU32 = match NonZeroU32::new(60) {
    Some(rate) => rate,
    None => unreachable!(),
};

/// Pending-petition admission cap per verified identity, across all of
/// that identity's connections (IDL). 1 in version 1: one prompt per
/// identity, and the no-coalescing decision (module docs) -- an excess
/// petition resolves `busy`.
pub(crate) const MAX_PENDING_PER_IDENTITY: usize = 1;

/// The deployment-level ceiling on concurrent-plus-queued prompts (IDL:
/// "the deployment additionally enforces a global ceiling"). Bounds the
/// prompt queue a fleet of distinct identities could otherwise grow
/// without limit; excess petitions resolve `busy`.
pub(crate) const MAX_PENDING_GLOBAL: usize = 4;

// ---------------------------------------------------------------------------
// The consent seam
// ---------------------------------------------------------------------------

/// Everything the consent authority sees about one petition -- and
/// everything the core-rendered prompt may display: exclusively
/// core-validated data (IDL, `vitrin_consent`): the *verifier-canonical*
/// identity (never client text), the realm, and the parsed request. No
/// agent-controlled free text reaches the human through this struct.
#[derive(Debug)]
pub(crate) struct PetitionSummary<'a> {
    /// The verified identity petitioning (what the prompt names).
    pub principal: &'a PrincipalIdentity,
    /// The realm petitioned over.
    pub realm: &'a RealmId,
    /// The petition's grant handle id on its connection -- the key the
    /// consent surface hands back to `resolve_pending`. Connection-scoped:
    /// meaningful only to the `PrincipalServer` that produced this summary
    /// (the surface's queue keys prompts by connection *and* this id).
    pub grant_object: u32,
    /// Requested verb set (non-zero; the wire already made empty fatal).
    pub verbs: Verb,
    /// Requested rung, already reduced to the MVP subset -- durable rungs
    /// resolved `unsupported` before the decider is consulted.
    pub persistence: PersistenceRung,
    /// Requested lifetime; 0 = bounded by the rung.
    pub expiry_ms: u32,
    /// Requested event-rate ceiling; 0 = server default. Shown for
    /// completeness; the effective rate is server policy, not a prompt
    /// choice (the IDL deliberately omits it from `resolved`).
    pub max_event_rate: u32,
}

/// The human's (or policy's) answer to one petition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsentDecision {
    /// Approved, with the effective authority actually chosen -- possibly
    /// narrower than the petition (never wider: the flow clamps, module
    /// docs).
    Granted(EffectiveAuthority),
    /// The human said no. A clean `resolved(denied)` -- never a hang,
    /// never a connection death.
    Denied,
}

/// The authority a `Granted` decision confers: exactly the fields
/// `vitrin_grant.resolved` echoes, plus the issuer for the table row.
/// [`PersistenceRung`] (not the wire enum) by construction: a decision
/// *cannot* express a durable rung, so consent can never smuggle one past
/// the petition flow's `unsupported` gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectiveAuthority {
    /// Effective verb set; MUST be a non-empty subset of the petition's
    /// (the flow clamps and fail-closes to `denied` if emptied).
    pub verbs: Verb,
    /// Effective rung; MUST NOT exceed the requested rung.
    pub persistence: PersistenceRung,
    /// Effective lifetime in milliseconds; 0 = bounded by the rung. MUST
    /// NOT exceed a non-zero requested lifetime.
    pub expiry_ms: u32,
    /// Which core-side authority made this decision (grant-row `issuer`).
    pub issuer: Issuer,
}

/// A decider's verdict on one petition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsentVerdict {
    /// Decided synchronously (a policy decider, or a scripted test): the
    /// flow resolves the petition immediately.
    Decided(ConsentDecision),
    /// The decision arrives later (a prompt is up or queued): the flow
    /// parks the petition -- `vitrin_consent.state(queued)` on the wire --
    /// counts it against admission, and completes it through
    /// `resolve_pending`, or expires it at the pending deadline.
    Pending,
}

/// The consent authority: exactly one per core, serving every connection
/// (like [`Verifier`](crate::identity::Verifier)). P1.7.1's prompt
/// renderer and P1.7.2's auto-approve flag both live behind this trait;
/// the petition flow does not know which is wired.
///
/// Contract: `decide` is consulted once per admitted petition, *after*
/// the flow's own policy gates (unsupported/unavailable/busy) -- a decider
/// never sees a petition the core already refused. It MUST NOT block on
/// the human (return [`ConsentVerdict::Pending`] and answer through
/// `resolve_pending`); it MAY decide synchronously from policy.
pub(crate) trait ConsentDecider {
    /// Judge one admitted petition.
    fn decide(&mut self, petition: &PetitionSummary<'_>) -> ConsentVerdict;
}

/// The walking skeleton's consent policy (plan D4/R6, milestone M1.1):
/// approve every admitted petition exactly as requested, loudly, with
/// [`Issuer::AutoApprovePolicy`] on the row. Headless CI and demos only.
///
/// The R6 guardrails -- an explicit `--consent=auto-approve` flag, a
/// persistent startup warning, and refusing to run when the identity
/// registry holds more than the demo principal -- are runtime wiring
/// (P1.7.2); this type is the decider that flag selects, and it logs
/// every single approval so a quiet rot into "auto-approve by default"
/// stays impossible to miss.
pub(crate) struct AutoApproveDecider;

impl ConsentDecider for AutoApproveDecider {
    fn decide(&mut self, petition: &PetitionSummary<'_>) -> ConsentVerdict {
        tracing::warn!(
            principal = %petition.principal,
            realm = %petition.realm,
            verbs = petition.verbs.bits(),
            expiry_ms = petition.expiry_ms,
            max_event_rate = petition.max_event_rate,
            "consent AUTO-APPROVED by policy (no human in the loop)"
        );
        ConsentVerdict::Decided(ConsentDecision::Granted(EffectiveAuthority {
            verbs: petition.verbs,
            persistence: petition.persistence,
            expiry_ms: petition.expiry_ms,
            issuer: Issuer::AutoApprovePolicy,
        }))
    }
}

/// The build-gated scripted-consent injector (IDL, `vitrin_consent`: "a
/// build-gated core hook, not a wire message"): plays verdicts from a
/// script through the same seam P1.7.x implements, so tests can hold,
/// approve, deny, or let a petition time out. An exhausted script holds
/// (the pending path is the interesting one to leave a test in).
#[cfg(test)]
pub(crate) struct ScriptedDecider {
    /// Verdicts consumed front-to-back, one per `decide` call.
    pub script: std::collections::VecDeque<ConsentVerdict>,
    /// The `grant_object` of every petition that reached the seam, in
    /// order -- lets tests assert what the consent surface was shown.
    pub seen: Vec<u32>,
}

#[cfg(test)]
impl ScriptedDecider {
    /// A decider that holds every petition pending.
    pub fn holding() -> Self {
        Self {
            script: std::collections::VecDeque::new(),
            seen: Vec::new(),
        }
    }

    /// A decider that plays `verdicts` in order, then holds.
    pub fn scripted(verdicts: impl IntoIterator<Item = ConsentVerdict>) -> Self {
        Self {
            script: verdicts.into_iter().collect(),
            seen: Vec::new(),
        }
    }
}

#[cfg(test)]
impl ConsentDecider for ScriptedDecider {
    fn decide(&mut self, petition: &PetitionSummary<'_>) -> ConsentVerdict {
        self.seen.push(petition.grant_object);
        self.script.pop_front().unwrap_or(ConsentVerdict::Pending)
    }
}

// ---------------------------------------------------------------------------
// The shared kernel state
// ---------------------------------------------------------------------------

/// The cross-connection state of the capability kernel: the one
/// [`GrantTable`], the pending-petition admission counters (per identity
/// *across connections*, plus the global prompt ceiling -- IDL), and the
/// set of live realms. One instance per core, threaded into every
/// [`PrincipalServer`](crate::principal::PrincipalServer) call by the
/// embedder, exactly like the verifier.
///
/// The kernel holds *state*, not flow: admission accounting lives here
/// because the busy cap spans connections; every wire decision (what
/// resolves, what is emitted, on which object) stays in the per-connection
/// petition flow. Grant-row bookkeeping contracts (removal at connection
/// teardown, `grants.rs` module docs) are discharged by
/// [`PrincipalServer::teardown`](crate::principal::PrincipalServer::teardown)
/// through this struct.
pub(crate) struct GrantKernel {
    /// The single grant store the enforcement chokepoint (P1.4.4) queries.
    table: GrantTable,
    /// Realms petitions may currently attach to. Static in version 1
    /// (exactly `realm-0`, created at startup); owned by the realm manager
    /// (P1.5.1) when realms grow lifecycle.
    live_realms: BTreeSet<RealmId>,
    /// Live pending petitions per verified identity (the `busy` cap's
    /// per-identity leg). Entries are removed at zero so the map stays
    /// O(identities with prompts up).
    pending_by_identity: BTreeMap<PrincipalIdentity, usize>,
    /// Live pending petitions overall (the global prompt ceiling).
    pending_total: usize,
}

impl GrantKernel {
    /// A kernel serving the given live realms (version 1 passes exactly
    /// one, `realm-0`, from `realm.toml` -- P1.5.1).
    pub fn new(live_realms: impl IntoIterator<Item = RealmId>) -> Self {
        Self {
            table: GrantTable::new(),
            live_realms: live_realms.into_iter().collect(),
            pending_by_identity: BTreeMap::new(),
            pending_total: 0,
        }
    }

    /// The grant table, for row insertion (petition flow), the chokepoint
    /// query (P1.4.4), revocation surfaces (P1.7.3), and teardown removal.
    pub fn table_mut(&mut self) -> &mut GrantTable {
        &mut self.table
    }

    /// Read-only table access (panel/flight-recorder view; tests).
    pub fn table(&self) -> &GrantTable {
        &self.table
    }

    /// Whether petitions naming `realm` proceed (false resolves
    /// `unavailable`).
    pub fn realm_is_live(&self, realm: &RealmId) -> bool {
        self.live_realms.contains(realm)
    }

    /// Whether one more pending petition from `identity` fits under both
    /// admission caps (false resolves `busy`).
    pub fn admission_available(&self, identity: &PrincipalIdentity) -> bool {
        self.pending_total < MAX_PENDING_GLOBAL
            && self.pending_by_identity.get(identity).copied().unwrap_or(0)
                < MAX_PENDING_PER_IDENTITY
    }

    /// Count one newly parked petition. Callers check
    /// [`admission_available`](Self::admission_available) first; this
    /// method only counts.
    pub fn admit_pending(&mut self, identity: &PrincipalIdentity) {
        *self
            .pending_by_identity
            .entry(identity.clone())
            .or_insert(0) += 1;
        self.pending_total += 1;
    }

    /// Release one pending petition's admission slot (resolution, expiry,
    /// or withdrawal at connection teardown). Saturating: a release
    /// without a matching admit is a core bug surfacing as a debug
    /// assertion, never an underflow panic in release builds
    /// (fail-closed: counters stick at zero).
    pub fn release_pending(&mut self, identity: &PrincipalIdentity) {
        match self.pending_by_identity.get_mut(identity) {
            Some(count) if *count > 0 => {
                *count -= 1;
                if *count == 0 {
                    self.pending_by_identity.remove(identity);
                }
            }
            _ => {
                debug_assert!(false, "release_pending without a matching admit_pending");
            }
        }
        if self.pending_total > 0 {
            self.pending_total -= 1;
        } else {
            debug_assert!(false, "pending_total underflow");
        }
    }

    /// Live pending petitions overall (telemetry; tests).
    pub fn pending_total(&self) -> usize {
        self.pending_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(s: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(s).unwrap()
    }

    fn kernel() -> GrantKernel {
        GrantKernel::new([RealmId::new("realm-0")])
    }

    #[test]
    fn admission_caps_per_identity_and_globally() {
        let mut kernel = kernel();
        let a = principal("vitrin://local/agent/a");

        // Per-identity: the single slot admits once, then refuses.
        assert!(kernel.admission_available(&a));
        kernel.admit_pending(&a);
        assert!(!kernel.admission_available(&a));
        assert_eq!(kernel.pending_total(), 1);

        // Releasing restores the slot.
        kernel.release_pending(&a);
        assert!(kernel.admission_available(&a));
        assert_eq!(kernel.pending_total(), 0);

        // Global ceiling: distinct identities fill it, then even a fresh
        // identity is refused.
        let others: Vec<_> = (0..MAX_PENDING_GLOBAL)
            .map(|i| principal(&format!("vitrin://local/agent/queue-{i}")))
            .collect();
        for other in &others {
            assert!(kernel.admission_available(other));
            kernel.admit_pending(other);
        }
        assert_eq!(kernel.pending_total(), MAX_PENDING_GLOBAL);
        assert!(!kernel.admission_available(&a));
        // Draining one slot re-opens the ceiling.
        kernel.release_pending(&others[0]);
        assert!(kernel.admission_available(&a));
    }

    #[test]
    fn realm_liveness_is_exactly_the_configured_set() {
        let kernel = kernel();
        assert!(kernel.realm_is_live(&RealmId::new("realm-0")));
        assert!(!kernel.realm_is_live(&RealmId::new("realm-1")));
        assert!(!kernel.realm_is_live(&RealmId::new("")));
    }

    #[test]
    fn auto_approve_echoes_the_petition_with_the_policy_issuer() {
        let identity = principal("vitrin://local/agent/demo");
        let realm = RealmId::new("realm-0");
        let verdict = AutoApproveDecider.decide(&PetitionSummary {
            principal: &identity,
            realm: &realm,
            grant_object: 4,
            verbs: Verb::OBSERVE | Verb::ACTUATE_TEXT,
            persistence: PersistenceRung::WhileRunning,
            expiry_ms: 5_000,
            max_event_rate: 0,
        });
        assert_eq!(
            verdict,
            ConsentVerdict::Decided(ConsentDecision::Granted(EffectiveAuthority {
                verbs: Verb::OBSERVE | Verb::ACTUATE_TEXT,
                persistence: PersistenceRung::WhileRunning,
                expiry_ms: 5_000,
                issuer: Issuer::AutoApprovePolicy,
            }))
        );
    }

    #[test]
    fn the_scripted_decider_plays_its_script_then_holds() {
        let identity = principal("vitrin://local/agent/demo");
        let realm = RealmId::new("realm-0");
        let summary = |grant_object| PetitionSummary {
            principal: &identity,
            realm: &realm,
            grant_object,
            verbs: Verb::OBSERVE,
            persistence: PersistenceRung::Once,
            expiry_ms: 0,
            max_event_rate: 0,
        };
        let mut decider =
            ScriptedDecider::scripted([ConsentVerdict::Decided(ConsentDecision::Denied)]);
        assert_eq!(
            decider.decide(&summary(4)),
            ConsentVerdict::Decided(ConsentDecision::Denied)
        );
        assert_eq!(decider.decide(&summary(9)), ConsentVerdict::Pending);
        assert_eq!(decider.seen, vec![4, 9]);
    }
}
