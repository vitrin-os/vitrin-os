//! The grant table v0 (P1.4.2, issue #26): the in-memory heart of the
//! capability kernel (artifact A3).
//!
//! Rows follow PRD Doc 2 section 5.2 **exactly** -- `{grant_id,
//! principal_id, realm_id, resource_ref, verbs[], constraints{expiry,
//! max_event_rate, focus_condition, one_shot?},
//! persistence(once|while_running|until_revoked|always), provenance_ref?,
//! parent_grant_id?, issued_at, issuer}` -- including the fields the MVP
//! cannot fill yet, which are present-but-null so the schema never breaks
//! when later phases fill them (each names its filling phase on its doc
//! comment). Fields the PRD marks optional but the MVP cannot state are
//! *type-level* null where possible: [`FocusCondition`] and
//! [`ProvenanceRef`] are empty enums, so an MVP row cannot even represent a
//! value there -- present-but-null enforced by construction, not
//! convention. Rows are keyed by the verifier-canonical
//! [`PrincipalIdentity`] the identity layer binds (P1.4.1, issue #25) --
//! the grant table never sees free client text.
//!
//! Table bookkeeping that is *not* row schema (liveness state, the cached
//! expiry deadline) lives beside the row in the table's private entry, so
//! [`GrantRow`]'s `Debug` output is exactly the PRD row and nothing else.
//!
//! # Decisions this task settles
//!
//! **Clock injection: explicit `now` parameters, monotonic
//! [`Instant`], no clock reads in table logic.** Every time-sensitive
//! operation takes time as an argument (`issued_at` on [`GrantTable::
//! insert`], `now` on [`GrantTable::check_use`] and
//! [`GrantTable::get`]); the table never calls `Instant::now()` and has no
//! clock object at all. This is the strongest form of the injected-clock
//! requirement -- there is nothing to swap in tests, only values to pass --
//! and it matches the chokepoint contract, whose query is *defined* as
//! `(principal, resource, verb, now)`: P1.4.4 samples the clock once per
//! request at its single site, so one consistent `now` governs the whole
//! decision. Monotonic `Instant` rather than wall time because expiry is a
//! relative lifetime (`expiry_ms` on the wire): setting the system clock
//! back must never resurrect an expired grant, and forward jumps must never
//! mass-expire live ones.
//!
//! **Expiry boundary: fail-closed.** A grant with a time bound is live over
//! the half-open interval `[issued_at, issued_at + expiry)`; a use at
//! exactly the deadline is refused `expired`. When authority is in doubt at
//! an instant, the chokepoint refuses.
//!
//! **Revocation: state flip now, refusal on next use -- no push event.**
//! IDL-first finding: version 1 of `vitrin_grant` deliberately defines *no*
//! asynchronous `revoked` push event (it is a documented version-2+ growth
//! seam); `refused(revoked)` is the enforcement-bearing signal, "effective
//! on the very next request". The table therefore only flips the row's
//! liveness state -- [`GrantTable::revoke`] (panel/policy, one grant) and
//! [`GrantTable::revoke_principal`] (hold-Esc, P1.7.3, every grant of one
//! principal) -- and the very next [`GrantTable::check_use`] refuses
//! [`RefusalReason::Revoked`]. Notifying holders is the chokepoint's job at
//! use time; inventing a table-side notification would be a protocol change
//! this module has no authority to make.
//!
//! **A spent `once` grant refuses `expired`.** The `once` rung is
//! "single-use authority": the first allowed use consumes the grant. The
//! IDL defines `expiry_ms = 0` as "bounded by the rung" -- the rung itself
//! is a lifetime bound, and for `once` that bound is one use -- so
//! exhausting it is the grant's lifetime passing: wire code `expired`, SDK
//! `GrantExpired`, recovery = petition again (exactly as for time expiry).
//! `not_granted` would be wrong: its IDL causes are all
//! never-was-active cases (pending, ungranted facet, non-`granted`
//! resolution). A `once` grant consumed by an admitted use stays consumed
//! even if the operation later fails server-side (`internal`): fail-closed,
//! never authority-expanding.
//!
//! **Refusal precedence (documented determinism, not policy).** Per row:
//! `revoked` > `expired` (time or spent) > `not_granted` (verb outside the
//! effective set) -- a dead grant refuses with its death code for every
//! verb, matching the IDL's revocation and expiry flows. Across rows, when
//! no candidate allows, the aggregate refusal is the most severe seen
//! (`revoked` > `expired` > `not_granted`), and no covering row at all is
//! `not_granted`. Among *allowing* candidates the table prefers a
//! `while_running` row over consuming a `once` row (never burn single-use
//! authority a durable row already covers), tie-broken by lowest
//! `grant_id`; deterministic by construction.
//!
//! **What the table does *not* do.** No rate limiting: `max_event_rate` is
//! stored and handed to the chokepoint in [`Allowed`], but the token bucket
//! is P1.4.4's, in the input router (PRD Doc 2 sections 5.2/8) -- a
//! table-side bucket would be a second enforcement site. No consent, no
//! petition policy, no realm existence checks (an unknown realm resolves
//! `unavailable` in the petition flow, P1.4.3): the table answers exactly
//! what its rows state, nothing more. No persistence: grants die with the
//! process (restore tokens are a later phase), and durable rungs are
//! **absent from [`PersistenceRung`], not hidden** -- converting a wire
//! `until_revoked`/`always` fails typed, which the petition flow maps to
//! the `unsupported` outcome.
//!
//! **Connection death is removal, not revocation.** Version 1 grants all
//! die with their connection ("all of the principal's grants die with the
//! connection"), but the PRD row deliberately has no connection field --
//! the connection is transport scope, not authority schema. The connection
//! object (P1.4.3) owns the wire handles it minted, so it calls
//! [`GrantTable::remove`] for each of its grant ids at teardown. Removal
//! deletes the row outright: a later same-principal query finds no row and
//! refuses `not_granted`, which is the honest code -- the asker never held
//! a live handle -- whereas a tombstone would falsely report `revoked` to a
//! *different* connection of the same principal that never held this grant.
//! Skipping teardown would leak another connection's dead authority into
//! [`GrantTable::check_use`]'s principal-keyed lookup; the contract is
//! documented here and enforced by the P1.4.3 caller.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use vitrin_protocol::generated::vitrin_grant::{Persistence as WirePersistence, Refusal, Verb};

use crate::identity::PrincipalIdentity;

// ---------------------------------------------------------------------------
// Row field types
// ---------------------------------------------------------------------------

/// `grant_id` (PRD Doc 2 section 5.2): the table-assigned, process-unique,
/// never-reused identifier of one grant row. Distinct from any wire object
/// id: wire ids are connection-scoped and client-allocated; this id is the
/// server-global name the flight recorder (P1.4.5) and the attenuation tree
/// (`parent_grant_id`, later) refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct GrantId(u64);

impl GrantId {
    /// The raw id value (log/serialization use).
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for GrantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "grant-{}", self.0)
    }
}

/// `realm_id` (PRD Doc 2 section 5.2): the realm a grant attaches to,
/// by well-known name (`"realm-0"` is the single realm of version 1;
/// `vitrin_realm` carries names, max 64 bytes). A light newtype so realm
/// and principal strings cannot be swapped at the query boundary; the
/// realm manager (P1.5) may take ownership of this type when realms grow
/// lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RealmId(String);

impl RealmId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RealmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `resource_ref` (PRD Doc 2 section 5.2): what within the realm the grant
/// covers. Version 1 serves exactly one granularity -- the whole realm
/// (`request_grant`'s null-or-empty `resource` selector) -- so this enum
/// has exactly one variant; the finer type-prefixed selectors the wire
/// vocabulary reserves (`surface:...`, `node:...`) arrive as new variants
/// in Phase 2, refining [`ResourceRef::covers`] without changing the row
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceRef {
    /// Everything in the realm (wire selector: null or empty string).
    WholeRealm,
}

impl ResourceRef {
    /// Whether authority over `self` covers a use targeting `target`.
    /// Version 1 is trivially total (whole realm covers whole realm);
    /// Phase 2 variants add real containment arms here.
    pub fn covers(&self, target: &ResourceRef) -> bool {
        match (self, target) {
            (ResourceRef::WholeRealm, ResourceRef::WholeRealm) => true,
        }
    }
}

/// `persistence` (PRD Doc 2 section 5.2): the consent-ladder rung a row
/// carries. Exactly the two MVP rungs -- the durable rungs
/// (`until_revoked`, `always`) are **absent, not hidden**: they cannot be
/// represented in a row until provenance verification exists (Phase 3,
/// E3.7), and a wire petition naming one fails
/// [`PersistenceRung::try_from`] typed, which the petition flow (P1.4.3)
/// maps to the `unsupported` outcome. The wire enum
/// ([`WirePersistence`]) keeps all four rungs so the wire never changes
/// shape; this type is the table's honest subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistenceRung {
    /// Single-use authority: consumed by its first allowed use, then
    /// refuses `expired` (the rung-bounded lifetime has passed).
    Once,
    /// Lives while the requesting principal's connection lives; unlimited
    /// uses until expiry, revocation, or connection teardown
    /// ([`GrantTable::remove`]).
    WhileRunning,
}

/// A wire persistence rung the table cannot represent: the typed refusal
/// behind "durable rungs absent, not hidden".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DurableRungUnsupported(pub WirePersistence);

impl fmt::Display for DurableRungUnsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "persistence rung {:?} requires verified provenance (Phase 3, E3.7); \
             version 1 grants only once/while_running",
            self.0
        )
    }
}

impl TryFrom<WirePersistence> for PersistenceRung {
    type Error = DurableRungUnsupported;

    fn try_from(wire: WirePersistence) -> Result<Self, Self::Error> {
        match wire {
            WirePersistence::Once => Ok(PersistenceRung::Once),
            WirePersistence::WhileRunning => Ok(PersistenceRung::WhileRunning),
            WirePersistence::UntilRevoked | WirePersistence::Always => {
                Err(DurableRungUnsupported(wire))
            }
        }
    }
}

/// `constraints.focus_condition` (PRD Doc 2 section 5.2): a value-bearing
/// use condition ("only while the surface is focused"). **Present-but-null
/// until Phase 2**: value-bearing constraints arrive on the wire as a
/// since-gated builder request preceding `request_grant`, and the variants
/// land here with them. An empty enum, so an MVP row *cannot* carry a
/// focus condition -- the null is type-enforced, and enforcement can never
/// be silently skipped for a value that cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusCondition {}

/// `provenance_ref` (PRD Doc 2 section 5.2): the reference to a verified
/// binary identity that durable persistence rungs require ("the grant dies
/// the moment the presenting binary's identity no longer matches").
/// **Present-but-null until Phase 3 (E3.7, wallet + provenance)**, which
/// fills it with the Sigstore-style identity reference (decision D-009)
/// and thereby unblocks `until_revoked`/`always`. An empty enum: an MVP
/// row cannot fabricate provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvenanceRef {}

/// `issuer` (PRD Doc 2 section 5.2): which authority created the row.
/// Version 1 has exactly the two consent paths of Phase 1; both name a
/// *core-side* decision -- agents never issue grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Issuer {
    /// The human approved the core-rendered consent prompt (P1.7.1).
    HumanConsent,
    /// The loudly-logged `--consent=auto-approve` policy for headless CI
    /// and demos (P1.7.2, plan risk R6).
    AutoApprovePolicy,
}

/// `constraints{...}` (PRD Doc 2 section 5.2), stored exactly as the row
/// states them. The table *enforces* only `expiry` (its query refuses
/// `expired`); `max_event_rate` is enforced by the chokepoint's token
/// bucket (P1.4.4, the input router), which reads the ceiling from
/// [`Allowed`]. The two constraints the MVP cannot state are
/// present-but-null with type-level nulls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Constraints {
    /// Effective lifetime from `issued_at`; `None` is the wire's
    /// `expiry_ms = 0`, "bounded by the persistence rung" (no time bound).
    pub expiry: Option<Duration>,
    /// Effective event-rate ceiling, events per second, for observation
    /// and actuation alike. Non-zero by type: the wire's `0 = server
    /// default, never unlimited` is resolved to a concrete ceiling by the
    /// petition flow *before* the row is written -- a row never states
    /// "unlimited".
    pub max_event_rate: NonZeroU32,
    /// Present-but-null until Phase 2 (see [`FocusCondition`]).
    pub focus_condition: Option<FocusCondition>,
    /// Present-but-null: reserved as `request_grant` flags bit 0, which
    /// version 1 requires to be zero (a set bit resolves `unsupported`).
    /// A later version unreserves the bit; the PRD's wallet
    /// presentation flow (Doc 2 section 13, Phase 3) is its first named
    /// consumer.
    pub one_shot: Option<bool>,
}

/// One grant-table row, field-for-field the PRD Doc 2 section 5.2 shape --
/// nothing added, nothing omitted. Liveness (spent/revoked) and the cached
/// expiry deadline are table bookkeeping, deliberately *outside* this
/// struct, so `Debug` renders exactly the canonical row.
///
/// Construction is table-only ([`GrantTable::insert`]) and access is
/// read-only (`&GrantRow`), so every invariant the insert path checks
/// holds for every row ever observable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantRow {
    /// Table-assigned, never reused.
    pub grant_id: GrantId,
    /// The verifier-canonical principal the authority belongs to -- the
    /// identity layer's type (P1.4.1), never re-modeled, never free text.
    pub principal_id: PrincipalIdentity,
    /// The realm the grant attaches to.
    pub realm_id: RealmId,
    /// What within the realm (whole realm is version 1's only rung).
    pub resource_ref: ResourceRef,
    /// The effective verb set (the PRD's `verbs[]`, as the wire's
    /// bitfield projection). Non-empty: an empty petition is fatal
    /// `invalid_argument` on the wire, and [`GrantTable::insert`] refuses
    /// an empty set as defense in depth.
    pub verbs: Verb,
    /// The effective constraints, exactly as stated.
    pub constraints: Constraints,
    /// The consent-ladder rung (MVP subset; durable rungs absent).
    pub persistence: PersistenceRung,
    /// Present-but-null until Phase 3 (see [`ProvenanceRef`]).
    pub provenance_ref: Option<ProvenanceRef>,
    /// Present-but-null until attenuation lands (a documented
    /// `vitrin_grant` version-2+ growth seam): the parent in the PRD's
    /// attenuation/revocation tree, `None` for every root grant --
    /// and every MVP grant is a root.
    pub parent_grant_id: Option<GrantId>,
    /// When the row was created (the insert call's injected clock
    /// reading); the anchor `constraints.expiry` counts from.
    pub issued_at: Instant,
    /// Which core-side authority created the row.
    pub issuer: Issuer,
}

// ---------------------------------------------------------------------------
// Insert API
// ---------------------------------------------------------------------------

/// Everything the petition flow (P1.4.3) hands the table after consent
/// resolves `granted`: the **effective** values the human (or the
/// auto-approve policy) actually chose -- possibly narrower than the
/// petition -- with wire defaults already resolved (`expiry_ms = 0` to
/// `None`, `max_event_rate = 0` to the server's concrete default ceiling).
/// The spec deliberately has no fields for `provenance_ref`,
/// `parent_grant_id`, `focus_condition`, or `one_shot`: the MVP cannot
/// state them, so no caller can smuggle one in.
#[derive(Debug, Clone)]
pub(crate) struct GrantSpec {
    pub principal_id: PrincipalIdentity,
    pub realm_id: RealmId,
    pub resource_ref: ResourceRef,
    pub verbs: Verb,
    pub expiry: Option<Duration>,
    pub max_event_rate: NonZeroU32,
    pub persistence: PersistenceRung,
    pub issuer: Issuer,
}

/// Why [`GrantTable::insert`] refused to create a row. Both cases are
/// caller bugs upstream of the table (the wire layer already makes an
/// empty verb set fatal `invalid_argument`, and `expiry_ms` is `u32`
/// milliseconds, far inside `Instant` range), surfaced as typed errors
/// rather than panics: the TCB does not panic on reachable input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertError {
    /// The spec's verb set is empty; a grant conferring nothing is not a
    /// grant.
    EmptyVerbs,
    /// `issued_at + expiry` is not representable as an `Instant`
    /// (unreachable via the wire's `u32` milliseconds).
    ExpiryUnrepresentable,
}

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertError::EmptyVerbs => f.write_str("grant spec has an empty verb set"),
            InsertError::ExpiryUnrepresentable => {
                f.write_str("grant expiry deadline is not representable")
            }
        }
    }
}

impl std::error::Error for InsertError {}

// ---------------------------------------------------------------------------
// Query API
// ---------------------------------------------------------------------------

/// A use the table allowed: what the enforcement chokepoint (P1.4.4) needs
/// *after* admission -- the row's identity (to voice any later refusal on
/// the right `vitrin_grant`, and for the flight recorder) and the
/// row-stated rate ceiling its token bucket enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Allowed {
    /// The row that granted this use (after the documented selection
    /// preference when several rows cover it).
    pub grant_id: GrantId,
    /// The row's `constraints.max_event_rate`, for the chokepoint's
    /// bucket. The table itself never rate-limits (one enforcement site).
    pub max_event_rate: NonZeroU32,
}

/// Why the table refused a use: exactly the refusal codes a grant *row*
/// can decide. The chokepoint's other codes (`rate_limited`, `preempted`,
/// `consent_held`, `no_surface`, `internal`) are use-context, decided at
/// the chokepoint -- the table cannot honestly emit them, so its type
/// cannot express them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RefusalReason {
    /// No covering grant, or the verb is outside the covering grant's
    /// effective set (wire `not_granted`). Lowest severity.
    NotGranted,
    /// The grant's lifetime passed: its time bound (including exactly at
    /// the deadline -- fail-closed), or its `once` rung already consumed
    /// (wire `expired`).
    Expired,
    /// Revoked by hold-Esc, panel, or policy; effective on the very next
    /// request (wire `revoked`). Highest severity.
    Revoked,
}

impl From<RefusalReason> for Refusal {
    /// The wire projection, for the chokepoint's `vitrin_grant.refused`.
    fn from(reason: RefusalReason) -> Refusal {
        match reason {
            RefusalReason::NotGranted => Refusal::NotGranted,
            RefusalReason::Expired => Refusal::Expired,
            RefusalReason::Revoked => Refusal::Revoked,
        }
    }
}

/// A row's liveness as reported by [`GrantTable::get`] at a given `now` --
/// the stored state with time expiry folded in, same precedence as the
/// refusal path (revoked > expired > spent > active).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrantState {
    /// Live: uses under it are checked against verbs and constraints.
    Active,
    /// Explicitly revoked; every use refuses `revoked`.
    Revoked,
    /// Its time bound passed; every use refuses `expired`.
    Expired,
    /// Its single `once` use is consumed; every use refuses `expired`.
    Spent,
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// Stored liveness (what queries cannot derive from time alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Liveness {
    Active,
    /// The `once` rung's single use is consumed.
    Spent,
    /// Explicitly revoked (tombstoned so the next use refuses `revoked`).
    Revoked,
}

/// One table entry: the PRD row plus the bookkeeping that is deliberately
/// not row schema.
#[derive(Debug)]
struct Entry {
    row: GrantRow,
    /// `issued_at + expiry`, precomputed at insert (checked there, so the
    /// query path is total); `None` = no time bound.
    deadline: Option<Instant>,
    liveness: Liveness,
}

impl Entry {
    /// This row's answer for one use, death states first: a dead grant
    /// refuses with its death code for every verb (the IDL's revocation
    /// and expiry flows show both facets refusing the death code), then
    /// verb membership. `None` = this row allows the use.
    fn refusal_for(&self, verb: Verb, now: Instant) -> Option<RefusalReason> {
        if self.liveness == Liveness::Revoked {
            return Some(RefusalReason::Revoked);
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return Some(RefusalReason::Expired);
        }
        if self.liveness == Liveness::Spent {
            return Some(RefusalReason::Expired);
        }
        if !self.row.verbs.contains(verb) {
            return Some(RefusalReason::NotGranted);
        }
        None
    }
}

/// The in-memory grant store (PRD Doc 2 section 5.2): the single source of
/// authority the enforcement chokepoint queries. In-memory only -- grants
/// die with the process; restore tokens are a later phase.
///
/// The table is not itself the chokepoint: it is the *answer* behind it.
/// P1.4.4's one server-side check function calls [`GrantTable::check_use`]
/// and nothing else consults rows for authority.
#[derive(Debug, Default)]
pub(crate) struct GrantTable {
    /// Keyed by [`GrantId`]; `BTreeMap` for deterministic ascending-id
    /// iteration (the documented lowest-id tie-break falls out of it).
    entries: BTreeMap<GrantId, Entry>,
    /// Next id to assign; ids start at 1 and are never reused, so an id
    /// outlives its row unambiguously in logs.
    next_id: u64,
}

impl GrantTable {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// Create a row from effective, consent-resolved values, at
    /// `issued_at` (the caller's injected clock reading). Returns the
    /// assigned [`GrantId`].
    pub fn insert(&mut self, spec: GrantSpec, issued_at: Instant) -> Result<GrantId, InsertError> {
        if spec.verbs.bits() == 0 {
            return Err(InsertError::EmptyVerbs);
        }
        let deadline = match spec.expiry {
            Some(expiry) => Some(
                issued_at
                    .checked_add(expiry)
                    .ok_or(InsertError::ExpiryUnrepresentable)?,
            ),
            None => None,
        };
        let grant_id = GrantId(self.next_id);
        self.next_id += 1;
        let row = GrantRow {
            grant_id,
            principal_id: spec.principal_id,
            realm_id: spec.realm_id,
            resource_ref: spec.resource_ref,
            verbs: spec.verbs,
            constraints: Constraints {
                expiry: spec.expiry,
                max_event_rate: spec.max_event_rate,
                // Present-but-null (module docs): the MVP cannot state
                // these, so the insert path cannot accept them.
                focus_condition: None,
                one_shot: None,
            },
            persistence: spec.persistence,
            // Present-but-null: filled by Phase 3 provenance (E3.7) and
            // the version-2+ attenuation seam respectively.
            provenance_ref: None,
            parent_grant_id: None,
            issued_at,
            issuer: spec.issuer,
        };
        self.entries.insert(
            grant_id,
            Entry {
                row,
                deadline,
                liveness: Liveness::Active,
            },
        );
        Ok(grant_id)
    }

    /// **The chokepoint query** (P1.4.4): may this principal perform
    /// `verb` on `(realm, resource)` at `now`? One call per capture or
    /// actuation, from the single enforcement site.
    ///
    /// Allowing consumes a `once` row's single use (so a `once` grant
    /// admitted here is spent even if the operation later fails
    /// server-side -- fail-closed). Selection among several allowing rows
    /// and the refusal precedence when none allows are documented in the
    /// module docs; no policy beyond what rows state is applied.
    pub fn check_use(
        &mut self,
        principal: &PrincipalIdentity,
        realm: &RealmId,
        resource: &ResourceRef,
        verb: Verb,
        now: Instant,
    ) -> Result<Allowed, RefusalReason> {
        // Aggregate refusal severity across covering rows; RefusalReason's
        // derived Ord is the documented precedence (NotGranted < Expired <
        // Revoked).
        let mut refusal: Option<RefusalReason> = None;
        // The allowing candidate: prefer WhileRunning over consuming a
        // Once; ascending-id iteration makes "first seen" the lowest id.
        let mut chosen: Option<(GrantId, PersistenceRung)> = None;
        for (&id, entry) in &self.entries {
            let row = &entry.row;
            if row.principal_id != *principal
                || row.realm_id != *realm
                || !row.resource_ref.covers(resource)
            {
                continue;
            }
            match entry.refusal_for(verb, now) {
                Some(reason) => refusal = refusal.max(Some(reason)),
                None => {
                    let replaces = match chosen {
                        None => true,
                        Some((_, PersistenceRung::Once)) => {
                            entry.row.persistence == PersistenceRung::WhileRunning
                        }
                        Some((_, PersistenceRung::WhileRunning)) => false,
                    };
                    if replaces {
                        chosen = Some((id, row.persistence));
                    }
                }
            }
        }
        match chosen {
            Some((id, rung)) => {
                let entry = self
                    .entries
                    .get_mut(&id)
                    .expect("chosen id was just observed in the map");
                if rung == PersistenceRung::Once {
                    entry.liveness = Liveness::Spent;
                }
                Ok(Allowed {
                    grant_id: id,
                    max_event_rate: entry.row.constraints.max_event_rate,
                })
            }
            None => Err(refusal.unwrap_or(RefusalReason::NotGranted)),
        }
    }

    /// Revoke one grant (panel or policy). Returns whether the grant
    /// exists and was not already revoked. Effective on the very next
    /// query -- there is nothing else to do: no version-1 push event
    /// exists (module docs), and the next `check_use` refuses `revoked`.
    /// Revoking an expired or spent grant is permitted and wins the
    /// refusal precedence (the deliberate human act is the loudest fact).
    pub fn revoke(&mut self, grant: GrantId) -> bool {
        match self.entries.get_mut(&grant) {
            Some(entry) if entry.liveness != Liveness::Revoked => {
                entry.liveness = Liveness::Revoked;
                true
            }
            _ => false,
        }
    }

    /// Revoke every grant of one principal -- the hold-Esc dead-man
    /// switch's table half (P1.7.3): "a core-level revoke of the active
    /// principal's grants". Returns how many grants this call newly
    /// revoked.
    pub fn revoke_principal(&mut self, principal: &PrincipalIdentity) -> usize {
        let mut revoked = 0;
        for entry in self.entries.values_mut() {
            if entry.row.principal_id == *principal && entry.liveness != Liveness::Revoked {
                entry.liveness = Liveness::Revoked;
                revoked += 1;
            }
        }
        revoked
    }

    /// Delete a row outright -- connection teardown (module docs:
    /// version-1 grants die with their connection, and the owning
    /// connection calls this for each grant id it minted). Returns whether
    /// the row existed. After removal the id is never reassigned.
    pub fn remove(&mut self, grant: GrantId) -> bool {
        self.entries.remove(&grant).is_some()
    }

    /// Read one row and its liveness at `now` (panel/flight-recorder
    /// view; tests). Never consumes anything.
    pub fn get(&self, grant: GrantId, now: Instant) -> Option<(&GrantRow, GrantState)> {
        self.entries.get(&grant).map(|entry| {
            let state = match entry.liveness {
                Liveness::Revoked => GrantState::Revoked,
                _ if entry.deadline.is_some_and(|deadline| now >= deadline) => GrantState::Expired,
                Liveness::Spent => GrantState::Spent,
                Liveness::Active => GrantState::Active,
            };
            (&entry.row, state)
        })
    }

    /// All rows, ascending [`GrantId`] (the "connected apps"-style
    /// enumeration surface; the flight recorder's snapshot source).
    pub fn rows(&self) -> impl Iterator<Item = &GrantRow> {
        self.entries.values().map(|entry| &entry.row)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// One deterministic clock origin per test; all times are `T0 + offset`
    /// (module docs: time is injected as values, so tests never sleep and
    /// never read the clock beyond this anchor).
    fn t0() -> Instant {
        Instant::now()
    }

    fn principal(s: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(s).unwrap()
    }

    fn rate(events_per_second: u32) -> NonZeroU32 {
        NonZeroU32::new(events_per_second).unwrap()
    }

    /// A while_running observe-grant spec for `p`, expiring after `expiry`.
    fn spec(p: &str, verbs: Verb, expiry: Option<Duration>) -> GrantSpec {
        GrantSpec {
            principal_id: principal(p),
            realm_id: RealmId::new("realm-0"),
            resource_ref: ResourceRef::WholeRealm,
            verbs,
            expiry,
            max_event_rate: rate(20),
            persistence: PersistenceRung::WhileRunning,
            issuer: Issuer::AutoApprovePolicy,
        }
    }

    /// `check_use` with the standard target, at `now`.
    fn use_at(
        table: &mut GrantTable,
        p: &str,
        verb: Verb,
        now: Instant,
    ) -> Result<Allowed, RefusalReason> {
        table.check_use(
            &principal(p),
            &RealmId::new("realm-0"),
            &ResourceRef::WholeRealm,
            verb,
            now,
        )
    }

    const DEMO: &str = "vitrin://local/agent/demo";
    const OTHER: &str = "vitrin://local/agent/other";

    // -- expiry (injected clock) -------------------------------------------

    #[test]
    fn expiry_is_honored_including_the_exact_deadline() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let id = table
            .insert(spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(5))), t0)
            .unwrap();

        // Live strictly before the deadline.
        assert!(use_at(&mut table, DEMO, Verb::OBSERVE, t0).is_ok());
        let just_before = t0 + Duration::from_millis(4_999);
        assert!(use_at(&mut table, DEMO, Verb::OBSERVE, just_before).is_ok());

        // Boundary: a use at exactly `issued_at + expiry` is refused
        // (fail-closed half-open lifetime).
        let deadline = t0 + Duration::from_secs(5);
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, deadline),
            Err(RefusalReason::Expired)
        );
        assert_eq!(
            use_at(
                &mut table,
                DEMO,
                Verb::OBSERVE,
                deadline + Duration::from_secs(1)
            ),
            Err(RefusalReason::Expired)
        );
        assert_eq!(
            table.get(id, deadline).unwrap().1,
            GrantState::Expired,
            "get() folds time expiry into the reported state"
        );

        // Expiry never resurrects: still expired much later.
        assert_eq!(
            use_at(
                &mut table,
                DEMO,
                Verb::OBSERVE,
                t0 + Duration::from_secs(3600)
            ),
            Err(RefusalReason::Expired)
        );
    }

    #[test]
    fn no_time_bound_means_bounded_by_the_rung_only() {
        let t0 = t0();
        let mut table = GrantTable::new();
        table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        // Ten years on, a rung-bounded while_running grant still allows.
        let far = t0 + Duration::from_secs(10 * 365 * 24 * 3600);
        assert!(use_at(&mut table, DEMO, Verb::OBSERVE, far).is_ok());
    }

    // -- verb checks --------------------------------------------------------

    #[test]
    fn a_verb_outside_the_grant_is_refused_not_granted() {
        let t0 = t0();
        let mut table = GrantTable::new();
        table
            .insert(spec(DEMO, Verb::OBSERVE | Verb::ACTUATE_TEXT, None), t0)
            .unwrap();

        assert!(use_at(&mut table, DEMO, Verb::OBSERVE, t0).is_ok());
        assert!(use_at(&mut table, DEMO, Verb::ACTUATE_TEXT, t0).is_ok());
        assert_eq!(
            use_at(&mut table, DEMO, Verb::ACTUATE_POINTER, t0),
            Err(RefusalReason::NotGranted)
        );
        // A multi-bit use requires every bit (contains semantics): a set
        // including an ungranted verb is refused whole.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE | Verb::ACTUATE_POINTER, t0),
            Err(RefusalReason::NotGranted)
        );
    }

    #[test]
    fn no_covering_row_is_refused_not_granted() {
        let t0 = t0();
        let mut table = GrantTable::new();
        // Empty table.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Err(RefusalReason::NotGranted)
        );
        table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        // Wrong principal.
        assert_eq!(
            use_at(&mut table, OTHER, Verb::OBSERVE, t0),
            Err(RefusalReason::NotGranted)
        );
        // Wrong realm.
        assert_eq!(
            table.check_use(
                &principal(DEMO),
                &RealmId::new("realm-1"),
                &ResourceRef::WholeRealm,
                Verb::OBSERVE,
                t0,
            ),
            Err(RefusalReason::NotGranted)
        );
    }

    // -- revocation ---------------------------------------------------------

    #[test]
    fn revocation_is_effective_on_the_very_next_request() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();

        assert!(use_at(&mut table, DEMO, Verb::OBSERVE, t0).is_ok());
        assert!(table.revoke(id));
        // Same injected instant: no grace, no cache -- the very next query
        // refuses `revoked`.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Err(RefusalReason::Revoked)
        );
        assert_eq!(table.get(id, t0).unwrap().1, GrantState::Revoked);
        // Idempotent: a second revoke reports nothing newly revoked.
        assert!(!table.revoke(id));
        // Both verbs of a dead grant refuse the death code (IDL flow 4).
        assert_eq!(
            use_at(&mut table, DEMO, Verb::ACTUATE_POINTER, t0),
            Err(RefusalReason::Revoked)
        );
    }

    #[test]
    fn revoke_principal_kills_all_and_only_that_principals_grants() {
        let t0 = t0();
        let mut table = GrantTable::new();
        table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        table
            .insert(spec(DEMO, Verb::ACTUATE_POINTER, None), t0)
            .unwrap();
        table.insert(spec(OTHER, Verb::OBSERVE, None), t0).unwrap();

        assert_eq!(table.revoke_principal(&principal(DEMO)), 2);
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Err(RefusalReason::Revoked)
        );
        assert_eq!(
            use_at(&mut table, DEMO, Verb::ACTUATE_POINTER, t0),
            Err(RefusalReason::Revoked)
        );
        // The other principal is untouched.
        assert!(use_at(&mut table, OTHER, Verb::OBSERVE, t0).is_ok());
        // Nothing left to newly revoke.
        assert_eq!(table.revoke_principal(&principal(DEMO)), 0);
    }

    #[test]
    fn removal_deletes_the_row_rather_than_tombstoning_it() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert!(table.remove(id));
        // Gone means gone: not revoked, not expired -- no row, so the
        // refusal is `not_granted` (connection-teardown semantics).
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Err(RefusalReason::NotGranted)
        );
        assert!(table.get(id, t0).is_none());
        assert!(!table.remove(id));
        // Ids are never reused, even after removal.
        let next = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert!(next > id);
    }

    // -- persistence rungs --------------------------------------------------

    #[test]
    fn while_running_repeats_but_once_is_spent_by_its_first_use() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let mut once = spec(DEMO, Verb::OBSERVE, None);
        once.persistence = PersistenceRung::Once;
        let once_id = table.insert(once, t0).unwrap();

        // First use is allowed and consumes the single-use authority.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Ok(Allowed {
                grant_id: once_id,
                max_event_rate: rate(20),
            })
        );
        // Spent: the rung-bounded lifetime has passed -> `expired`
        // (module docs), on the very next request.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Err(RefusalReason::Expired)
        );
        assert_eq!(table.get(once_id, t0).unwrap().1, GrantState::Spent);

        // while_running: use after use after use.
        let wr_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        for _ in 0..10 {
            assert_eq!(
                use_at(&mut table, DEMO, Verb::OBSERVE, t0).map(|a| a.grant_id),
                Ok(wr_id)
            );
        }
        // A refused use consumes nothing: a wrong-verb refusal against a
        // fresh `once` grant leaves it unspent.
        let mut fresh = spec(OTHER, Verb::OBSERVE, None);
        fresh.persistence = PersistenceRung::Once;
        let fresh_id = table.insert(fresh, t0).unwrap();
        assert_eq!(
            use_at(&mut table, OTHER, Verb::ACTUATE_TEXT, t0),
            Err(RefusalReason::NotGranted)
        );
        assert_eq!(table.get(fresh_id, t0).unwrap().1, GrantState::Active);
    }

    #[test]
    fn selection_prefers_while_running_over_spending_a_once() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let mut once = spec(DEMO, Verb::OBSERVE, None);
        once.persistence = PersistenceRung::Once;
        let once_id = table.insert(once, t0).unwrap();
        let wr_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();

        // Both cover; repeated uses ride the while_running row and never
        // burn the single-use authority.
        for _ in 0..3 {
            assert_eq!(
                use_at(&mut table, DEMO, Verb::OBSERVE, t0).map(|a| a.grant_id),
                Ok(wr_id)
            );
        }
        assert_eq!(table.get(once_id, t0).unwrap().1, GrantState::Active);

        // With the durable row revoked, the once row serves -- exactly
        // once.
        assert!(table.revoke(wr_id));
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0).map(|a| a.grant_id),
            Ok(once_id)
        );
        // Now every covering row is dead; revoked outranks expired in the
        // aggregate refusal.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Err(RefusalReason::Revoked)
        );
    }

    #[test]
    fn refusal_precedence_is_revoked_over_expired_over_not_granted() {
        let t0 = t0();
        let later = t0 + Duration::from_secs(10);
        let mut table = GrantTable::new();

        // One expired row, one wrong-verb row: aggregate says `expired`.
        table
            .insert(spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(1))), t0)
            .unwrap();
        table
            .insert(spec(DEMO, Verb::ACTUATE_TEXT, None), t0)
            .unwrap();
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, later),
            Err(RefusalReason::Expired)
        );

        // Add a revoked row: `revoked` wins.
        let revoked_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert!(table.revoke(revoked_id));
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, later),
            Err(RefusalReason::Revoked)
        );

        // An active covering row still allows even beside dead rows.
        let live_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, later).map(|a| a.grant_id),
            Ok(live_id)
        );

        // Revoking an already-expired grant flips its report to revoked
        // (the deliberate act is the loudest fact).
        let expired_id = table
            .insert(spec(OTHER, Verb::OBSERVE, Some(Duration::from_secs(1))), t0)
            .unwrap();
        assert_eq!(table.get(expired_id, later).unwrap().1, GrantState::Expired);
        assert!(table.revoke(expired_id));
        assert_eq!(
            use_at(&mut table, OTHER, Verb::OBSERVE, later),
            Err(RefusalReason::Revoked)
        );
    }

    #[test]
    fn durable_rungs_are_absent_not_hidden() {
        // The wire ladder has four rungs; the table's type converts only
        // the two MVP rungs and refuses the durable ones typed.
        assert_eq!(
            PersistenceRung::try_from(WirePersistence::Once),
            Ok(PersistenceRung::Once)
        );
        assert_eq!(
            PersistenceRung::try_from(WirePersistence::WhileRunning),
            Ok(PersistenceRung::WhileRunning)
        );
        assert_eq!(
            PersistenceRung::try_from(WirePersistence::UntilRevoked),
            Err(DurableRungUnsupported(WirePersistence::UntilRevoked))
        );
        assert_eq!(
            PersistenceRung::try_from(WirePersistence::Always),
            Err(DurableRungUnsupported(WirePersistence::Always))
        );
    }

    // -- insert validation --------------------------------------------------

    #[test]
    fn insert_refuses_empty_verbs_and_unrepresentable_expiry() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let empty = spec(DEMO, Verb::default(), None);
        assert_eq!(table.insert(empty, t0), Err(InsertError::EmptyVerbs));

        let overflow = spec(DEMO, Verb::OBSERVE, Some(Duration::MAX));
        assert_eq!(
            table.insert(overflow, t0),
            Err(InsertError::ExpiryUnrepresentable)
        );
        // Nothing was inserted by the refused calls.
        assert_eq!(table.rows().count(), 0);
    }

    // -- row-shape fidelity -------------------------------------------------

    #[test]
    fn the_row_debug_shows_every_prd_field_and_nulls_that_do_not_lie() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let mut human = spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(300)));
        human.issuer = Issuer::HumanConsent;
        let id = table.insert(human, t0).unwrap();
        let (row, state) = table.get(id, t0).unwrap();
        assert_eq!(state, GrantState::Active);
        let debug = format!("{row:?}");

        // Every PRD Doc 2 section 5.2 field, by name, in the one Debug
        // rendering -- the row shape is the schema, verbatim.
        for field in [
            "grant_id",
            "principal_id",
            "realm_id",
            "resource_ref",
            "verbs",
            "constraints",
            "expiry",
            "max_event_rate",
            "focus_condition",
            "one_shot",
            "persistence",
            "provenance_ref",
            "parent_grant_id",
            "issued_at",
            "issuer",
        ] {
            assert!(
                debug.contains(field),
                "Debug output lacks `{field}`: {debug}"
            );
        }

        // The MVP-unfillable fields are present and visibly null -- never
        // omitted, never fabricated.
        for null_field in [
            "focus_condition: None",
            "one_shot: None",
            "provenance_ref: None",
            "parent_grant_id: None",
        ] {
            assert!(
                debug.contains(null_field),
                "Debug output lacks `{null_field}`: {debug}"
            );
        }

        // And the filled fields are the effective values, not defaults.
        assert_eq!(row.grant_id, id);
        assert_eq!(row.principal_id, principal(DEMO));
        assert_eq!(row.realm_id, RealmId::new("realm-0"));
        assert_eq!(row.resource_ref, ResourceRef::WholeRealm);
        assert!(row.verbs.contains(Verb::OBSERVE));
        assert_eq!(row.constraints.expiry, Some(Duration::from_secs(300)));
        assert_eq!(row.constraints.max_event_rate, rate(20));
        assert_eq!(row.persistence, PersistenceRung::WhileRunning);
        assert_eq!(row.issued_at, t0);
        assert_eq!(row.issuer, Issuer::HumanConsent);
    }

    #[test]
    fn ids_and_names_render_for_logs() {
        // The flight recorder's (P1.4.5) accessors: raw id, `grant-N`
        // rendering, realm name round-trip.
        let t0 = t0();
        let mut table = GrantTable::new();
        let id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert_eq!(id.as_u64(), 1);
        assert_eq!(id.to_string(), "grant-1");
        let realm = RealmId::new("realm-0");
        assert_eq!(realm.as_str(), "realm-0");
        assert_eq!(realm.to_string(), "realm-0");
    }

    #[test]
    fn refusal_reasons_project_onto_the_wire_codes() {
        assert_eq!(
            Refusal::from(RefusalReason::NotGranted),
            Refusal::NotGranted
        );
        assert_eq!(Refusal::from(RefusalReason::Expired), Refusal::Expired);
        assert_eq!(Refusal::from(RefusalReason::Revoked), Refusal::Revoked);
    }
}
