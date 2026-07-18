//! The principal-connection protocol server (P1.4.1, issue #25): the server
//! side of the P1.1.3 handshake state machine, where the [`identity`] layer
//! binds -- `vitrin_handshake` (bootstrap object 1) plus the
//! `vitrin_principal` it mints.
//!
//! [`PrincipalServer`] is **one instance per accepted principal
//! connection**, exactly like [`ShimServer`](crate::shim::ShimServer) is per
//! shim connection: it owns the connection's handshake phase, its object
//! table (watermark plus what each id is), and the fatal-error funnel. The
//! conventions' state machine (section 7.1) maps as:
//!
//! - **CONNECTED**: only `hello` is legal; anything else is fatal
//!   `pre_handshake`. Unparseable garbage never reaches this module -- the
//!   transport funnel (P1.2.3) closes it silently.
//! - **VERIFYING** is *transient inside the `hello` dispatch* in this
//!   build: verification is a synchronous [`Verifier::verify`] call, so the
//!   connection is never at rest in VERIFYING between loop turns. Pipelined
//!   requests wait in the socket itself -- which is exactly the IDL's
//!   "bounded by ordinary transport backpressure, never an unbounded
//!   server-side buffer" -- and are dispatched after `bound` goes out, or
//!   never (the connection dies first) when verification fails. A future
//!   deferred verifier (remote SVID/OIDC, see [`identity`]'s trait docs)
//!   reifies VERIFYING as a resting state by parking the connection; the
//!   [`VerifyOutcome`] type already fits that shape.
//! - **BOUND**: the steady state; `sync` and the principal's requests
//!   dispatch, and every fatal condition maps onto the conventions' ten
//!   codes.
//! - **DEAD** ([`Phase::Dead`]): entered on any fatal return, as defense in
//!   depth -- the embedder contract is to close the connection and dispatch
//!   nothing further after an `Err`, and this phase makes a violation of
//!   that contract inert (later frames refuse `internal` without
//!   processing), so requests pipelined behind a failed `hello` are
//!   provably never processed even against an embedder bug.
//!
//! # Identity binding (the P1.4.1 chokepoint for *who is asking*)
//!
//! `hello` runs the IDL's fixed check order -- frame grammar, then the
//! version integer, then the `principal` new_id's allocation rules, then --
//! only for a version-accepted, well-formed `hello` -- credential
//! verification through the pluggable [`Verifier`]. The verifier receives
//! the claimed identity, scheme, credential bytes, and the `SO_PEERCRED`
//! the transport recorded at accept ([`PrincipalServer::new`] captures it
//! from the [`Connection`](vitrin_ipc::Connection) at registration time);
//! its canonical identity -- never the claimed string -- is what
//! `vitrin_principal.bound` carries and what
//! [`bound_identity`](PrincipalServer::bound_identity) exposes for the
//! grant table (P1.4.2) and enforcement chokepoint (P1.4.4) to key on.
//!
//! # Refusal uniformity (identity-probing resistance)
//!
//! Every credential-rejection cause -- unknown identity, bad token,
//! verifier failure or unavailability, `SO_PEERCRED` mismatch -- collapses
//! on the wire to fatal `auth_failed` with the **fixed phrase**
//! [`AUTH_REFUSED_PHRASE`], byte-identical across causes (a test asserts
//! frame equality). The cause, the claimed identity, and at most the
//! credential *scheme and byte length* go to the server log -- never
//! credential bytes. The generated `Hello` struct derives `Debug` including
//! the credential field, so this module never formats a decoded `Hello`;
//! see the summary flag to `track:protocol` about redacting that derive.
//!
//! # Sender-constrained handles
//!
//! The object table lives inside this per-connection instance and nowhere
//! else -- there is deliberately **no cross-connection handle namespace**
//! (ids are per-connection, conventions section 3; the identity layer adds
//! no registry keyed by object id). A handle minted on connection A and
//! presented on connection B lands in B's table lookup, finds nothing, and
//! dies fatal `invalid_object` -- the two-connection test proves it. The
//! sender-constraint triple (connection, verified credential,
//! `SO_PEERCRED`) is thereby structural: the table *is* the connection, and
//! the identity was verified against that connection's peercred.
//!
//! # The petition flow (P1.4.3, issue #27)
//!
//! `vitrin_realm.request_grant` runs the grant lifecycle state machine:
//! decode and validate (five co-minted ids under the multi-`new_id` rule,
//! conventions 3.2; non-zero verbs), register the quintet (the objects
//! exist whatever the outcome -- "the busy petition's co-minted objects
//! remain inert"), then resolve. Policy refusals resolve first, in
//! documented precedence -- `unsupported` (durable rung, reserved flag,
//! finer-than-whole-realm resource: the petition *as stated* can never be
//! served) before `unavailable` (unknown realm: could succeed later)
//! before `busy` (admission full: the most transient) -- none of which
//! consult consent or consume an admission slot. An admitted petition goes
//! to the [`ConsentDecider`] seam: an immediate decision resolves now
//! (`consent.state(closed)` then `resolved`, the IDL's transitions-before-
//! terminal order); a held one parks pending (`state(queued)`), counts
//! against the cross-connection admission caps, and completes through
//! [`resolve_pending`](PrincipalServer::resolve_pending) or expires
//! fail-closed at its deadline through
//! [`expire_pending`](PrincipalServer::expire_pending) -- time injected by
//! the caller in the same explicit style as [`grants`](crate::grants).
//! Denial, timeout, and every policy refusal are clean recoverable events
//! on the co-minted grant handle: never a hang, never a connection death.
//! Policy decisions (timeout value, no-coalescing/busy, default rate,
//! clamping) are documented on [`consent`](crate::consent).
//!
//! # Scope seams (marked, not smuggled)
//!
//! - **Facet use is P1.4.4**: requests on the co-minted view/pointer/text
//!   facets refuse fatal `internal` with a logged "not implemented" --
//!   honest server limitation -- until the enforcement chokepoint lands.
//!   The routing data it needs is already here
//!   ([`facet_binding`](PrincipalServer::facet_binding) -> owning grant
//!   handle + verb, [`granted_table_id`](PrincipalServer::granted_table_id)
//!   -> [`GrantTable::check_use_grant`]'s key); no authority decision is
//!   made outside that one future chokepoint. The IDL's server-side
//!   petition-rate ceiling on `request_grant` itself likewise lands with
//!   P1.4.4's token-bucket infrastructure rather than as a second ad-hoc
//!   rate limiter here.
//! - **Prompt lifecycle is P1.7.x**: `consent.state(shown)`, the exclusive
//!   input grab, `consent_held`, and dismissing a live prompt whose
//!   petitioner died are the consent surface's (see
//!   [`consent`](crate::consent)'s seam notes).
//! - The **unauthenticated deadline** (conventions 7.1 SHOULD) is a wall
//!   clock owned by the runtime wiring: nothing at runtime accepts
//!   principal connections yet (the listener wiring lands with M1.1
//!   integration), and the deadline is a calloop timer armed at accept and
//!   disarmed on [`is_bound`](PrincipalServer::is_bound) -- flagged in the
//!   task summary rather than half-built here. The pending-consent timer
//!   is the same wiring: calloop arms
//!   [`next_pending_deadline`](PrincipalServer::next_pending_deadline) and
//!   calls `expire_pending` when it fires.
//! - The flight recorder (P1.4.5) will observe handshakes and grant
//!   lifecycle through the same embedder that logs [`PrincipalFault`]s
//!   today.
//!
//! [`ConsentDecider`]: crate::consent::ConsentDecider
//! [`GrantTable::check_use_grant`]: crate::grants::GrantTable::check_use_grant
//!
//! [`identity`]: crate::identity
//! [`Verifier`]: crate::identity::Verifier
//! [`Verifier::verify`]: crate::identity::Verifier::verify
//! [`VerifyOutcome`]: crate::identity::VerifyOutcome

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use vitrin_ipc::{Message, PeerCred, TransportError};
use vitrin_protocol::error::DecodeError;
use vitrin_protocol::generated::vitrin_actuator_pointer as pointer;
use vitrin_protocol::generated::vitrin_actuator_text as text;
use vitrin_protocol::generated::vitrin_consent as wire_consent;
use vitrin_protocol::generated::vitrin_grant as wire_grant;
use vitrin_protocol::generated::vitrin_grant::Verb;
use vitrin_protocol::generated::vitrin_handshake as handshake;
use vitrin_protocol::generated::vitrin_handshake::Error as WireError;
use vitrin_protocol::generated::vitrin_principal as principal;
use vitrin_protocol::generated::vitrin_realm as realm;
use vitrin_protocol::generated::vitrin_view as view;
use vitrin_protocol::generated::PROTOCOL_VERSION;

use crate::consent::{
    ConsentDecider, ConsentDecision, ConsentVerdict, GrantKernel, PetitionSummary,
    DEFAULT_MAX_EVENT_RATE, PENDING_CONSENT_TIMEOUT,
};
use crate::grants::{GrantId, GrantSpec, PersistenceRung, RealmId, ResourceRef};
use crate::identity::{PresentedCredential, PrincipalIdentity, Verifier, VerifyOutcome};

/// The handshake bootstrap object: implicit object 1 on a principal
/// connection, never created by a message (conventions section 3).
pub(crate) const HANDSHAKE_ID: u32 = 1;

/// Top of the client-allocated object-id range; ids above it are reserved
/// to the server (conventions section 3, unused in version 0).
const CLIENT_ID_MAX: u32 = 0xfeff_ffff;

/// The fixed `error.message` phrase for every `auth_failed` (the prose
/// page's example wording). MUST stay cause-free and identity-free: a
/// refused handshake is deliberately uniform on the wire.
pub(crate) const AUTH_REFUSED_PHRASE: &str = "authentication refused";

/// Cap on live realm handles per connection -- the conventions' documented
/// per-connection live-object cap (fatal `resource_exhausted`), applied to
/// the one object class a bound principal can mint without bound in this
/// build. Version 1 defines no destructors, so every `get_realm` is a
/// permanent per-connection allocation; a compliant client needs exactly
/// one (`realm-0`), and 16 mirrors the shim server's surface cap.
pub(crate) const MAX_LIVE_REALMS: usize = 16;

/// Cap on petitions per connection -- the IDL's per-connection
/// live-object cap applied to `request_grant`, which permanently
/// allocates *five* object ids per call (version 1 has no destructors).
/// Breaching it is fatal `resource_exhausted`, "confining the
/// denial-of-service to the offending connection rather than the whole
/// core" (IDL). A compliant Phase-1 client needs exactly one petition;
/// 16 mirrors [`MAX_LIVE_REALMS`] and leaves ample re-petition headroom
/// after denials and timeouts (each re-petition is a fresh quintet).
pub(crate) const MAX_LIVE_PETITIONS: usize = 16;

/// A protocol violation by the principal client: the fatal conditions of
/// conventions section 5.2 as they arise on this connection class. Always
/// connection-fatal; [`PrincipalServer::handle_message`] delivers the
/// best-effort `vitrin_handshake.error` goodbye itself and the embedder
/// logs the violation (`Display` carries the log detail, including the
/// `auth_failed` cause the wire deliberately omits) and closes.
#[derive(Debug)]
pub(crate) enum PrincipalViolation {
    /// `pre_handshake`: parsed traffic before a first `hello`.
    PreHandshake { object_id: u32, opcode: u8 },
    /// `version_unsupported`: `hello` offered a version above this
    /// server's maximum (exactly [`PROTOCOL_VERSION`] in version 0).
    VersionUnsupported { offered: u32 },
    /// `auth_failed`: the verifier did not bind the credential. Uniform on
    /// the wire; the fields exist for the server log only and never carry
    /// credential bytes (scheme and byte length at most).
    AuthFailed {
        claimed_identity: String,
        credential_type: String,
        credential_len: usize,
        cause: String,
    },
    /// `invalid_opcode`: a second `hello` (its opcode is defined only in
    /// the CONNECTED state).
    SecondHello,
    /// `invalid_opcode`: an opcode the target object's interface does not
    /// define at the negotiated version.
    UnknownOpcode { object_id: u32, opcode: u8 },
    /// `invalid_object`: unknown or foreign object id (the
    /// sender-constraint kill site), or a `new_id` violating allocation
    /// rules.
    InvalidObject {
        object_id: u32,
        detail: &'static str,
    },
    /// `invalid_argument`: a decoded-but-semantically-invalid argument the
    /// frame grammar cannot catch (a petition's empty verb set -- the
    /// IDL's "MUST be non-zero").
    InvalidArgument {
        object_id: u32,
        detail: &'static str,
    },
    /// The frame did not decode as the selected message; maps onto the
    /// conventions' fatal code via [`DecodeError::to_wire_error`]. Carries
    /// the id the frame targeted so the goodbye cites the object where the
    /// error occurred (the IDL's `error.object_id` "may be 1" -- it is not
    /// always 1: a malformed `get_realm` cites the principal, not the
    /// handshake object).
    Malformed { object_id: u32, source: DecodeError },
    /// `resource_exhausted`: a documented per-connection bound was
    /// breached ([`MAX_LIVE_REALMS`], object-id exhaustion).
    ResourceExhausted(&'static str),
    /// `internal`: this build cannot serve the request -- the P1.4.3 seam
    /// (`request_grant` before the grant flow lands).
    Unimplemented { object_id: u32, what: &'static str },
    /// `internal`: a frame was dispatched after a fatal condition already
    /// killed this connection -- an embedder-contract violation made inert
    /// (defense in depth; see the module docs on DEAD).
    ConnectionDead,
}

impl PrincipalViolation {
    /// The fatal wire code (conventions 5.2) this violation maps to.
    fn wire_code(&self) -> WireError {
        match self {
            PrincipalViolation::PreHandshake { .. } => WireError::PreHandshake,
            PrincipalViolation::VersionUnsupported { .. } => WireError::VersionUnsupported,
            PrincipalViolation::AuthFailed { .. } => WireError::AuthFailed,
            PrincipalViolation::SecondHello | PrincipalViolation::UnknownOpcode { .. } => {
                WireError::InvalidOpcode
            }
            PrincipalViolation::InvalidObject { .. } => WireError::InvalidObject,
            PrincipalViolation::InvalidArgument { .. } => WireError::InvalidArgument,
            PrincipalViolation::Malformed { source, .. } => source.to_wire_error(),
            PrincipalViolation::ResourceExhausted(_) => WireError::ResourceExhausted,
            PrincipalViolation::Unimplemented { .. } | PrincipalViolation::ConnectionDead => {
                WireError::Internal
            }
        }
    }

    /// The id cited in the `error` event's `object_id` argument: where the
    /// error occurred (which may be 1, the handshake object itself).
    fn cited_object(&self) -> u32 {
        match self {
            PrincipalViolation::PreHandshake { object_id, .. }
            | PrincipalViolation::UnknownOpcode { object_id, .. }
            | PrincipalViolation::InvalidObject { object_id, .. }
            | PrincipalViolation::InvalidArgument { object_id, .. }
            | PrincipalViolation::Malformed { object_id, .. }
            | PrincipalViolation::Unimplemented { object_id, .. } => *object_id,
            _ => HANDSHAKE_ID,
        }
    }

    /// The `error.message` text: free-form debug wording, except for
    /// `auth_failed`, whose phrase is fixed and cause-free
    /// ([`AUTH_REFUSED_PHRASE`]) -- the wire must not learn what the log
    /// knows. `version_unsupported` likewise carries no supported-version
    /// hint (downgrade is refusal, not negotiation).
    fn wire_message(&self) -> String {
        match self {
            PrincipalViolation::PreHandshake { .. } => "traffic before hello".into(),
            PrincipalViolation::VersionUnsupported { .. } => {
                "protocol version not implemented".into()
            }
            PrincipalViolation::AuthFailed { .. } => AUTH_REFUSED_PHRASE.into(),
            PrincipalViolation::SecondHello => "hello is legal exactly once".into(),
            PrincipalViolation::UnknownOpcode { opcode, .. } => {
                format!("opcode {opcode} is not defined for this object")
            }
            PrincipalViolation::InvalidObject { detail, .. }
            | PrincipalViolation::InvalidArgument { detail, .. } => (*detail).into(),
            PrincipalViolation::Malformed { source, .. } => source.to_string(),
            PrincipalViolation::ResourceExhausted(detail) => (*detail).into(),
            PrincipalViolation::Unimplemented { what, .. } => {
                format!("{what} is not implemented in this build")
            }
            PrincipalViolation::ConnectionDead => "connection already dead".into(),
        }
    }
}

impl fmt::Display for PrincipalViolation {
    /// The server-log rendering: unlike [`wire_message`], this *does* name
    /// the `auth_failed` cause and the claimed identity -- the "logged
    /// reason" the state machine promises -- while still never containing
    /// credential bytes.
    ///
    /// [`wire_message`]: PrincipalViolation::wire_message
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrincipalViolation::PreHandshake { object_id, opcode } => write!(
                f,
                "pre_handshake: opcode {opcode} on object {object_id} before hello"
            ),
            PrincipalViolation::VersionUnsupported { offered } => {
                write!(
                    f,
                    "version_unsupported: offered {offered}, maximum {PROTOCOL_VERSION}"
                )
            }
            PrincipalViolation::AuthFailed {
                claimed_identity,
                credential_type,
                credential_len,
                cause,
            } => write!(
                f,
                "auth_failed: {cause} (claimed identity {claimed_identity:?}, \
                 credential scheme {credential_type:?}, {credential_len} bytes)"
            ),
            PrincipalViolation::SecondHello => {
                write!(f, "invalid_opcode: second hello (legal exactly once)")
            }
            PrincipalViolation::UnknownOpcode { object_id, opcode } => {
                write!(f, "invalid_opcode: opcode {opcode} on object {object_id}")
            }
            PrincipalViolation::InvalidObject { object_id, detail } => {
                write!(f, "invalid_object: id {object_id}: {detail}")
            }
            PrincipalViolation::InvalidArgument { object_id, detail } => {
                write!(f, "invalid_argument: object {object_id}: {detail}")
            }
            PrincipalViolation::Malformed { object_id, source } => {
                write!(f, "malformed message on object {object_id}: {source}")
            }
            PrincipalViolation::ResourceExhausted(detail) => {
                write!(f, "resource_exhausted: {detail}")
            }
            PrincipalViolation::Unimplemented { object_id, what } => {
                write!(f, "internal: {what} on object {object_id} not implemented")
            }
            PrincipalViolation::ConnectionDead => {
                write!(f, "internal: message dispatched to a dead connection")
            }
        }
    }
}

/// Why [`PrincipalServer::handle_message`] gave up on the connection:
/// either the client violated the protocol (goodbye already sent
/// best-effort; the embedder logs and closes) or sending an event hit a
/// terminal transport condition (the P1.2.3 funnel owns it; no goodbye is
/// attempted on a full or poisoned queue).
#[derive(Debug)]
pub(crate) enum PrincipalFault {
    Violation(PrincipalViolation),
    Transport(TransportError),
}

impl fmt::Display for PrincipalFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrincipalFault::Violation(v) => write!(f, "principal protocol violation: {v}"),
            PrincipalFault::Transport(e) => write!(f, "transport: {e}"),
        }
    }
}

impl From<PrincipalViolation> for PrincipalFault {
    fn from(v: PrincipalViolation) -> Self {
        PrincipalFault::Violation(v)
    }
}

impl From<TransportError> for PrincipalFault {
    fn from(e: TransportError) -> Self {
        PrincipalFault::Transport(e)
    }
}

/// The handshake phase of one principal connection (conventions 7.1).
/// VERIFYING is transient inside the `hello` dispatch in this build (module
/// docs), so it has no resting representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Pre-handshake: only `hello` is legal.
    Connected,
    /// Steady state: the principal is bound and requests dispatch.
    Bound,
    /// A fatal condition ended this connection; nothing dispatches again.
    Dead,
}

/// The role of one petition-minted companion object (everything in the
/// quintet except the grant handle itself), with the grant handle that
/// owns it -- the routing datum the enforcement chokepoint (P1.4.4) keys
/// facet-borne uses by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FacetRole {
    /// `vitrin_consent`: events only, no requests in version 1.
    Consent,
    /// `vitrin_view`: the observe facet.
    View,
    /// `vitrin_actuator_pointer`: the pointer facet.
    Pointer,
    /// `vitrin_actuator_text`: the text facet.
    Text,
}

impl FacetRole {
    /// The verb a use through this facet exercises (`None` for the
    /// consent observer, which carries no authority at all).
    pub fn verb(self) -> Option<Verb> {
        match self {
            FacetRole::Consent => None,
            FacetRole::View => Some(Verb::OBSERVE),
            FacetRole::Pointer => Some(Verb::ACTUATE_POINTER),
            FacetRole::Text => Some(Verb::ACTUATE_TEXT),
        }
    }

    /// Whether `opcode` names a request the role's interface defines at
    /// version 1 (an undefined opcode is fatal `invalid_opcode`; a defined
    /// one routes to the P1.4.4 seam).
    fn defines_opcode(self, opcode: u8) -> bool {
        match self {
            FacetRole::Consent => false,
            FacetRole::View => opcode == view::requests::CaptureFrame::OPCODE,
            FacetRole::Pointer => {
                opcode == pointer::requests::Move::OPCODE
                    || opcode == pointer::requests::Button::OPCODE
                    || opcode == pointer::requests::Scroll::OPCODE
            }
            FacetRole::Text => opcode == text::requests::Type::OPCODE,
        }
    }
}

/// One petition-minted companion object in the per-connection table.
#[derive(Debug, Clone, Copy)]
struct PetitionObject {
    role: FacetRole,
    /// The grant handle this object was co-minted with.
    grant_object: u32,
}

/// The petition's requested authority, kept while pending so the eventual
/// decision can be clamped against it (consent narrows, never widens).
#[derive(Debug, Clone)]
struct RequestedAuthority {
    verbs: Verb,
    persistence: PersistenceRung,
    expiry_ms: u32,
    max_event_rate: u32,
}

/// A petition parked on the consent decision.
#[derive(Debug, Clone)]
struct PendingPetition {
    /// `parked_at + PENDING_CONSENT_TIMEOUT`; expired fail-closed at
    /// exactly this instant by [`PrincipalServer::expire_pending`].
    deadline: Instant,
    realm: RealmId,
    requested: RequestedAuthority,
}

/// One grant handle's lifecycle state.
#[derive(Debug, Clone)]
enum PetitionState {
    /// Waiting on the consent decision (admission slot held).
    Pending(PendingPetition),
    /// `resolved` has been sent -- exactly once, ever (IDL). `table_id`
    /// is the grant-table row this petition minted, `Some` iff the
    /// outcome was `granted`; removed at connection teardown.
    Resolved { table_id: Option<GrantId> },
}

/// The per-connection record of one petition (keyed by grant handle id).
#[derive(Debug, Clone)]
struct GrantEntry {
    /// The co-minted consent observer, for `state` events at resolution.
    consent_id: u32,
    state: PetitionState,
}

/// How one petition resolves: the effective authority, or a refusal
/// outcome. Internal to the flow; the wire projection is
/// `vitrin_grant.resolved`.
#[derive(Debug, Clone, Copy)]
enum Resolution {
    Granted {
        verbs: Verb,
        persistence: PersistenceRung,
        expiry_ms: u32,
        table_id: GrantId,
    },
    /// One of the non-`granted` outcomes (`denied`, `timed_out`,
    /// `unavailable`, `unsupported`, `busy`); the event's trailing
    /// arguments are zero (IDL).
    Refused(wire_grant::Outcome),
}

impl Resolution {
    fn table_id(&self) -> Option<GrantId> {
        match self {
            Resolution::Granted { table_id, .. } => Some(*table_id),
            Resolution::Refused(_) => None,
        }
    }

    /// The `vitrin_grant.resolved` event this resolution projects to.
    fn to_event(self) -> wire_grant::events::Resolved {
        match self {
            Resolution::Granted {
                verbs,
                persistence,
                expiry_ms,
                ..
            } => wire_grant::events::Resolved {
                outcome: wire_grant::Outcome::Granted,
                verbs,
                persistence: persistence.into(),
                expiry_ms,
            },
            Resolution::Refused(outcome) => wire_grant::events::Resolved {
                outcome,
                verbs: Verb::default(),
                persistence: wire_grant::Persistence::Once,
                expiry_ms: 0,
            },
        }
    }
}

/// The per-connection principal protocol server. One instance per accepted
/// principal connection; single-threaded, driven by decoded [`Message`]s
/// from the connection's event source. The embedder passes the same
/// [`Verifier`], [`ConsentDecider`], and [`GrantKernel`] for the
/// connection's whole lifetime (one of each serves every connection) and
/// samples the clock once per dispatched frame (the injected-time style of
/// [`grants`](crate::grants)). On `Err` it logs the fault, closes the
/// connection without dispatching further frames, and calls
/// [`teardown`](PrincipalServer::teardown) -- as it also does on a clean
/// close -- so the connection's grants and pending petitions die with it.
pub(crate) struct PrincipalServer {
    phase: Phase,
    /// `SO_PEERCRED` recorded by the transport at accept -- the third leg
    /// of the sender-constraint triple, captured at construction and handed
    /// to the verifier on `hello`.
    peer: PeerCred,
    /// Highest object id allocated on this connection (starts at the
    /// bootstrap id); every `new_id` must exceed it -- strictly increasing,
    /// never reused (conventions 3.1).
    watermark: u32,
    /// The principal object minted by `hello`, live after `bound`.
    principal_id: Option<u32>,
    /// The verifier-canonical bound identity; what P1.4.2 keys grants by.
    identity: Option<PrincipalIdentity>,
    /// Realm handle id -> requested realm name. `BTreeMap` so iteration
    /// (and hence log output) is deterministic.
    realms: BTreeMap<u32, String>,
    /// Grant handle id -> petition record (state machine + table row).
    grants: BTreeMap<u32, GrantEntry>,
    /// Companion object id (consent/view/pointer/text) -> role + owning
    /// grant handle: the P1.4.4 facet-routing table.
    petition_objects: BTreeMap<u32, PetitionObject>,
}

impl PrincipalServer {
    /// A fresh server for one accepted connection, with the `SO_PEERCRED`
    /// the transport captured at accept ([`Connection::peer_cred`]).
    ///
    /// [`Connection::peer_cred`]: vitrin_ipc::Connection::peer_cred
    pub fn new(peer: PeerCred) -> Self {
        Self {
            phase: Phase::Connected,
            peer,
            watermark: HANDSHAKE_ID,
            principal_id: None,
            identity: None,
            realms: BTreeMap::new(),
            grants: BTreeMap::new(),
            petition_objects: BTreeMap::new(),
        }
    }

    /// Whether the handshake has succeeded (the embedder's cue to disarm
    /// its unauthenticated-phase deadline timer).
    pub fn is_bound(&self) -> bool {
        self.phase == Phase::Bound
    }

    /// The verifier-canonical bound identity, once bound: the value every
    /// grant row (P1.4.2) and enforcement decision (P1.4.4) keys on.
    pub fn bound_identity(&self) -> Option<&PrincipalIdentity> {
        self.identity.as_ref()
    }

    /// Dispatch one decoded frame from the principal connection. `now` is
    /// the caller's clock sample for this frame (injected time: petition
    /// deadlines and grant `issued_at` derive from it; the server itself
    /// never reads a clock).
    ///
    /// `Err` means the connection must die: the wire goodbye
    /// (`vitrin_handshake.error`) has already been sent best-effort for
    /// protocol violations (never for transport faults -- the queue that
    /// would carry it is the thing that failed), the violation has been
    /// logged, and the embedder closes the connection, calls
    /// [`teardown`](Self::teardown), and dispatches nothing further.
    pub fn handle_message<F>(
        &mut self,
        msg: Message,
        now: Instant,
        verifier: &dyn Verifier,
        decider: &mut dyn ConsentDecider,
        kernel: &mut GrantKernel,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let result = self.dispatch(msg, now, verifier, decider, kernel, send);
        if let Err(fault) = &result {
            let was_dead = self.phase == Phase::Dead;
            self.phase = Phase::Dead;
            match fault {
                // The one place every violation gets its reason logged
                // (the state machine's "logged reason" promise), then the
                // best-effort goodbye -- unless this *is* the goodbye
                // failing on an already-dead connection.
                PrincipalFault::Violation(v) => {
                    tracing::warn!(peer_uid = self.peer.uid, %v, "principal connection fatal");
                    if !was_dead {
                        let goodbye = handshake::events::Error {
                            object_id: v.cited_object(),
                            code: v.wire_code(),
                            message: v.wire_message(),
                        };
                        // Best-effort per the IDL: backpressure deaths and
                        // poisoned sends close without a goodbye.
                        let _ = send(&goodbye.encode(HANDSHAKE_ID));
                    }
                }
                PrincipalFault::Transport(e) => {
                    tracing::warn!(peer_uid = self.peer.uid, %e, "principal connection transport fault");
                }
            }
        }
        result
    }

    /// The undecorated dispatch: routing per the connection's phase and
    /// object table. Split from [`handle_message`](Self::handle_message) so
    /// the fatal funnel (log + goodbye + DEAD) lives in exactly one place.
    fn dispatch<F>(
        &mut self,
        msg: Message,
        now: Instant,
        verifier: &dyn Verifier,
        decider: &mut dyn ConsentDecider,
        kernel: &mut GrantKernel,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let object_id = msg.header.object_id;
        let opcode = msg.header.opcode;
        match self.phase {
            Phase::Dead => Err(PrincipalViolation::ConnectionDead.into()),
            Phase::Connected => {
                if object_id == HANDSHAKE_ID && opcode == handshake::requests::Hello::OPCODE {
                    self.handle_hello(msg, verifier, send)
                } else {
                    // Any parsed traffic before a first hello, whatever the
                    // object or opcode, is pre_handshake.
                    Err(PrincipalViolation::PreHandshake { object_id, opcode }.into())
                }
            }
            Phase::Bound => {
                if object_id == HANDSHAKE_ID {
                    match opcode {
                        // hello's opcode is defined only in CONNECTED, so a
                        // second hello is invalid_opcode, not pre_handshake.
                        handshake::requests::Hello::OPCODE => {
                            Err(PrincipalViolation::SecondHello.into())
                        }
                        handshake::requests::Sync::OPCODE => {
                            let (_, sync) =
                                handshake::requests::Sync::decode(&msg.bytes, msg.fd).map_err(
                                    |source| PrincipalViolation::Malformed { object_id, source },
                                )?;
                            // Single-threaded in-order dispatch: everything
                            // received before this sync has been processed
                            // and its events queued, so done goes out now.
                            let done = handshake::events::Done {
                                cookie: sync.cookie,
                            };
                            send(&done.encode(HANDSHAKE_ID))?;
                            Ok(())
                        }
                        _ => Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into()),
                    }
                } else if Some(object_id) == self.principal_id {
                    match opcode {
                        principal::requests::GetRealm::OPCODE => self.handle_get_realm(msg),
                        _ => Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into()),
                    }
                } else if self.realms.contains_key(&object_id) {
                    match opcode {
                        realm::requests::RequestGrant::OPCODE => {
                            self.handle_request_grant(msg, now, decider, kernel, send)
                        }
                        _ => Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into()),
                    }
                } else if self.grants.contains_key(&object_id) {
                    // `vitrin_grant` defines no requests in version 1 (the
                    // documented growth seams -- release, attenuate -- are
                    // later versions), so every opcode on a grant handle is
                    // invalid_opcode.
                    Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                } else if let Some(obj) = self.petition_objects.get(&object_id) {
                    if obj.role.defines_opcode(opcode) {
                        // The P1.4.4 seam: facet-borne use (capture and
                        // actuation) is the enforcement chokepoint's --
                        // admitting it here would be a second
                        // authority-checking path. Until it lands the honest
                        // answer is a server-side limitation, fatal
                        // `internal` -- never a fake refusal on the grant.
                        Err(PrincipalViolation::Unimplemented {
                            object_id,
                            what: "facet use under a grant (P1.4.4)",
                        }
                        .into())
                    } else {
                        Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                    }
                } else {
                    // The sender-constraint kill site: ids not in *this*
                    // connection's table -- including any handle minted on
                    // another connection -- are unknown or foreign.
                    Err(PrincipalViolation::InvalidObject {
                        object_id,
                        detail: "unknown or foreign object id",
                    }
                    .into())
                }
            }
        }
    }

    /// `hello`: the IDL's fixed check order -- grammar (decode), version,
    /// `principal` new_id allocation, then credential verification -- so
    /// `version_unsupported` and `invalid_object` reveal nothing about the
    /// credential, and refused verification reveals nothing beyond the
    /// uniform `auth_failed`.
    fn handle_hello<F>(
        &mut self,
        msg: Message,
        verifier: &dyn Verifier,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let object_id = msg.header.object_id;
        let (_, hello) = handshake::requests::Hello::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        if hello.version != PROTOCOL_VERSION {
            // Additive growth: this server implements exactly wire version
            // 1, so any other offer (a later version, or the never-issued
            // integer 0) is a version it does not implement.
            return Err(PrincipalViolation::VersionUnsupported {
                offered: hello.version,
            }
            .into());
        }
        self.allocate_id(hello.principal)?;

        let presented = PresentedCredential {
            claimed_identity: &hello.identity,
            credential_type: &hello.credential_type,
            credential: hello.credential.as_bytes(),
            peer: self.peer,
        };
        let cause = match verifier.verify(&presented) {
            VerifyOutcome::Bound(bound) => {
                let event = principal::events::Bound {
                    identity: bound.identity.as_str().to_owned(),
                };
                send(&event.encode(hello.principal))?;
                self.principal_id = Some(hello.principal);
                self.identity = Some(bound.identity);
                self.phase = Phase::Bound;
                return Ok(());
            }
            // Both non-Bound outcomes are wire-uniform auth_failed; they
            // differ only in the logged cause (rejected client vs. broken
            // verifier infrastructure).
            VerifyOutcome::Rejected(cause) => cause.to_string(),
            VerifyOutcome::Unavailable(detail) => format!("verifier unavailable: {detail}"),
        };
        Err(PrincipalViolation::AuthFailed {
            claimed_identity: hello.identity,
            credential_type: hello.credential_type,
            credential_len: hello.credential.len(),
            cause,
        }
        .into())
    }

    /// `get_realm`: a structural mint (no reply, no refusal) -- naming is
    /// not authority, and an unknown name still yields a handle whose
    /// petitions resolve `unavailable` later (IDL). Subject to the
    /// live-object cap and the watermark rule.
    fn handle_get_realm(&mut self, msg: Message) -> Result<(), PrincipalFault> {
        let object_id = msg.header.object_id;
        let (_, req) = principal::requests::GetRealm::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        // The cap precedes the mint (the shim server's precedent): version
        // 1 has no destructors, so every realm handle is a permanent
        // allocation, and unbounded minting is the conventions' fatal
        // resource_exhausted.
        if self.realms.len() >= MAX_LIVE_REALMS {
            return Err(PrincipalViolation::ResourceExhausted("live-realm cap exceeded").into());
        }
        self.allocate_id(req.realm)?;
        self.realms.insert(req.realm, req.name);
        Ok(())
    }

    /// `request_grant` (P1.4.3): the petition flow. State machine and
    /// refusal precedence in the module docs; policy decisions (timeout,
    /// no-coalescing/busy, default rate, clamping) on
    /// [`consent`](crate::consent).
    fn handle_request_grant<F>(
        &mut self,
        msg: Message,
        now: Instant,
        decider: &mut dyn ConsentDecider,
        kernel: &mut GrantKernel,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let object_id = msg.header.object_id;
        let (_, req) = realm::requests::RequestGrant::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        // The live-object cap precedes the mint (the `get_realm`
        // precedent): each petition permanently allocates five ids.
        if self.grants.len() >= MAX_LIVE_PETITIONS {
            return Err(PrincipalViolation::ResourceExhausted("live-petition cap exceeded").into());
        }
        // Multi-new_id rule (conventions 3.2): sequential allocation under
        // the watermark rule enforces exactly "distinct, strictly
        // increasing in argument order, all above the watermark" -- each
        // id must exceed the advancing watermark, which the previous id
        // just became. Any violation is fatal invalid_object, citing the
        // offending id.
        for id in [req.grant, req.consent, req.view, req.pointer, req.text] {
            self.allocate_id(id)?;
        }
        // The one argument invariant the frame grammar cannot check: an
        // empty petition is fatal invalid_argument (IDL: "MUST be
        // non-zero"; an out-of-range verb *bit* already died in decode).
        if req.verbs.bits() == 0 {
            return Err(PrincipalViolation::InvalidArgument {
                object_id,
                detail: "a petition's verb set MUST be non-zero",
            }
            .into());
        }

        // The quintet exists whatever the outcome ("the busy petition's
        // co-minted objects remain inert" -- prose flow 4): register the
        // companions before resolving anything.
        for (id, role) in [
            (req.consent, FacetRole::Consent),
            (req.view, FacetRole::View),
            (req.pointer, FacetRole::Pointer),
            (req.text, FacetRole::Text),
        ] {
            self.petition_objects.insert(
                id,
                PetitionObject {
                    role,
                    grant_object: req.grant,
                },
            );
        }

        let realm = RealmId::new(
            self.realms
                .get(&object_id)
                .expect("dispatched on a realm handle")
                .clone(),
        );
        let identity = self
            .identity
            .clone()
            .expect("requests dispatch only while bound");
        let requested_rung = PersistenceRung::try_from(req.persistence);

        // Policy refusals, in documented precedence (module docs):
        // unsupported (never serveable as stated) > unavailable (realm
        // state, could succeed later) > busy (admission, the most
        // transient). None consults consent, consumes an admission slot,
        // or opens a prompt -- so none emits a consent transition (prose
        // flows 4/5).
        let refusal = if req.flags != 0 {
            // A set reserved bit is honest refusal, not a protocol error
            // (IDL: flags is deliberately not wire-validated).
            Some((wire_grant::Outcome::Unsupported, "reserved flags bit set"))
        } else if !req.resource.is_empty() {
            // MVP resource granularity is the whole realm (the null-or-
            // empty selector; null encodes as empty on the wire). The
            // type-prefixed vocabulary (`surface:...`, `node:...`) is the
            // documented Phase-2 refinement -- refused honestly today,
            // never partially served.
            Some((
                wire_grant::Outcome::Unsupported,
                "resource selectors finer than the whole realm are not served in version 1",
            ))
        } else if requested_rung.is_err() {
            // Durable rungs are absent-not-hidden (grants.rs): the typed
            // conversion refusal is the wire's `unsupported` outcome
            // (SDK: GrantUnsupported).
            Some((
                wire_grant::Outcome::Unsupported,
                "durable persistence rungs require verified provenance (Phase 3)",
            ))
        } else if !kernel.realm_is_live(&realm) {
            // Naming is not authority and realm absence is a race, not a
            // protocol error (prose flow 5).
            Some((wire_grant::Outcome::Unavailable, "realm is not live"))
        } else if !kernel.admission_available(&identity) {
            // The consent-fatigue valve: concurrent duplicates do not
            // coalesce, they resolve busy (decision on crate::consent).
            Some((
                wire_grant::Outcome::Busy,
                "pending-petition admission cap reached",
            ))
        } else {
            None
        };
        if let Some((outcome, why)) = refusal {
            tracing::info!(
                principal = %identity,
                realm = %realm,
                grant_object = req.grant,
                outcome = ?outcome,
                why,
                "petition refused by policy"
            );
            self.grants.insert(
                req.grant,
                GrantEntry {
                    consent_id: req.consent,
                    state: PetitionState::Resolved { table_id: None },
                },
            );
            return Self::send_resolution(
                req.grant,
                req.consent,
                Resolution::Refused(outcome),
                false,
                send,
            )
            .map_err(Into::into);
        }

        let requested = RequestedAuthority {
            verbs: req.verbs,
            persistence: requested_rung.expect("durable rungs refused above"),
            expiry_ms: req.expiry_ms,
            max_event_rate: req.max_event_rate,
        };
        let verdict = decider.decide(&PetitionSummary {
            principal: &identity,
            realm: &realm,
            grant_object: req.grant,
            verbs: requested.verbs,
            persistence: requested.persistence,
            expiry_ms: requested.expiry_ms,
            max_event_rate: requested.max_event_rate,
        });
        match verdict {
            ConsentVerdict::Decided(decision) => {
                let resolution = self.apply_decision(decision, &requested, &realm, now, kernel);
                self.grants.insert(
                    req.grant,
                    GrantEntry {
                        consent_id: req.consent,
                        state: PetitionState::Resolved {
                            table_id: resolution.table_id(),
                        },
                    },
                );
                Self::send_resolution(req.grant, req.consent, resolution, true, send)
                    .map_err(Into::into)
            }
            ConsentVerdict::Pending => {
                kernel.admit_pending(&identity);
                self.grants.insert(
                    req.grant,
                    GrantEntry {
                        consent_id: req.consent,
                        state: PetitionState::Pending(PendingPetition {
                            deadline: now + PENDING_CONSENT_TIMEOUT,
                            realm,
                            requested,
                        }),
                    },
                );
                // Parked on a policy/human decision -- exactly the IDL's
                // `queued` ("waiting behind another prompt or a policy
                // decision"). `shown` is the prompt renderer's transition
                // when it lands (P1.7.1 seam).
                let queued = wire_consent::events::State {
                    state: wire_consent::ConsentState::Queued,
                };
                send(&queued.encode(req.consent)).map_err(Into::into)
            }
        }
    }

    /// Turn one consent decision into a petition's resolution, clamping
    /// the effective authority to the petition -- consent narrows, never
    /// widens (`crate::consent` module docs) -- and minting the
    /// grant-table row on approval. Fail-closed: a decision clamping empty
    /// resolves `denied`, never a fabricated grant.
    fn apply_decision(
        &self,
        decision: ConsentDecision,
        requested: &RequestedAuthority,
        realm: &RealmId,
        now: Instant,
        kernel: &mut GrantKernel,
    ) -> Resolution {
        let identity = self
            .identity
            .as_ref()
            .expect("petitions exist only while bound");
        let ConsentDecision::Granted(effective) = decision else {
            return Resolution::Refused(wire_grant::Outcome::Denied);
        };
        // Verbs: intersect with the petition. Empty after clamping means
        // the decision granted nothing the petitioner asked for.
        let verbs = Verb::from_bits(effective.verbs.bits() & requested.verbs.bits())
            .expect("an intersection of valid verb sets is a valid verb set");
        if verbs.bits() == 0 {
            tracing::warn!(
                principal = %identity,
                "consent decision granted no requested verb; resolving denied (fail-closed)"
            );
            return Resolution::Refused(wire_grant::Outcome::Denied);
        }
        if verbs != effective.verbs {
            tracing::warn!(
                principal = %identity,
                "consent decision named verbs outside the petition; clamped to the intersection"
            );
        }
        // Rung: `Allow once` may narrow a while_running petition; the
        // reverse would widen, and is clamped.
        let persistence = match (requested.persistence, effective.persistence) {
            (PersistenceRung::Once, PersistenceRung::WhileRunning) => {
                tracing::warn!(
                    principal = %identity,
                    "consent decision widened the persistence rung; clamped to once"
                );
                PersistenceRung::Once
            }
            (_, chosen) => chosen,
        };
        // Expiry: a non-zero request is an upper bound; 0 requested is
        // rung-bounded, which any concrete expiry only narrows.
        let expiry_ms = if requested.expiry_ms != 0
            && (effective.expiry_ms == 0 || effective.expiry_ms > requested.expiry_ms)
        {
            tracing::warn!(
                principal = %identity,
                "consent decision widened the expiry; clamped to the requested bound"
            );
            requested.expiry_ms
        } else {
            effective.expiry_ms
        };
        // The wire's `0 = server default, never unlimited` is resolved to
        // a concrete ceiling before the row is written (the table stores
        // no "unlimited"; grants.rs). The effective rate is server policy,
        // not a consent choice, and is deliberately not echoed in
        // `resolved` (IDL).
        let max_event_rate =
            NonZeroU32::new(requested.max_event_rate).unwrap_or(DEFAULT_MAX_EVENT_RATE);
        let table_id = kernel
            .table_mut()
            .insert(
                GrantSpec {
                    principal_id: identity.clone(),
                    realm_id: realm.clone(),
                    resource_ref: ResourceRef::WholeRealm,
                    verbs,
                    expiry: (expiry_ms != 0).then(|| Duration::from_millis(u64::from(expiry_ms))),
                    max_event_rate,
                    persistence,
                    issuer: effective.issuer,
                },
                now,
            )
            .expect("insert cannot fail: verbs checked non-empty, expiry bounded by u32 ms");
        Resolution::Granted {
            verbs,
            persistence,
            expiry_ms,
            table_id,
        }
    }

    /// Emit one petition's terminal onto the wire: the consent `closed`
    /// transition (iff a consent decision happened -- policy refusals
    /// never opened one) and then `resolved` -- the IDL's
    /// transitions-before-terminal order, each on its co-minted object.
    fn send_resolution<F>(
        grant_object: u32,
        consent_id: u32,
        resolution: Resolution,
        close_consent: bool,
        send: &mut F,
    ) -> Result<(), TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        if close_consent {
            let closed = wire_consent::events::State {
                state: wire_consent::ConsentState::Closed,
            };
            send(&closed.encode(consent_id))?;
        }
        send(&resolution.to_event().encode(grant_object))
    }

    /// Complete a pending petition with the consent decision -- the entry
    /// point the consent surface (P1.7.1) drives on the human's click, and
    /// the scripted-consent path in tests. Returns `Ok(true)` iff a
    /// pending petition existed and was resolved; `Ok(false)` is the
    /// benign no-op (the prompt raced connection death, a double click, or
    /// an already-expired petition -- a delivered expiry is final).
    ///
    /// Deliberately does *not* re-check the deadline: expiry happens only
    /// through [`expire_pending`](Self::expire_pending)'s sweep, so a
    /// human answer arriving while the prompt is still up (timer not yet
    /// fired) is honored -- the timeout reclaims *unanswered* prompts, it
    /// does not race the human's click.
    pub fn resolve_pending<F>(
        &mut self,
        grant_object: u32,
        decision: ConsentDecision,
        now: Instant,
        kernel: &mut GrantKernel,
        send: &mut F,
    ) -> Result<bool, TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        if self.phase == Phase::Dead {
            return Ok(false);
        }
        let Some(entry) = self.grants.get(&grant_object) else {
            return Ok(false);
        };
        let PetitionState::Pending(pending) = entry.state.clone() else {
            return Ok(false);
        };
        let consent_id = entry.consent_id;
        let identity = self
            .identity
            .clone()
            .expect("pending petitions exist only while bound");
        // Release the slot and flip the state *before* sending: a
        // transport fault mid-delivery must not leave the petition
        // re-resolvable or its admission slot occupied (the embedder
        // closes and tears down on any send failure).
        kernel.release_pending(&identity);
        let resolution =
            self.apply_decision(decision, &pending.requested, &pending.realm, now, kernel);
        self.grants
            .get_mut(&grant_object)
            .expect("entry observed above")
            .state = PetitionState::Resolved {
            table_id: resolution.table_id(),
        };
        Self::send_resolution(grant_object, consent_id, resolution, true, send)?;
        Ok(true)
    }

    /// Expire every pending petition whose deadline has passed (`now >=
    /// deadline` -- fail-closed at the exact boundary, the `grants.rs`
    /// convention): the consent prompt expired unanswered, delivered as a
    /// clean `resolved(timed_out)`; petitioning again later is legal
    /// (IDL). Driven by the embedder's timer, armed from
    /// [`next_pending_deadline`](Self::next_pending_deadline); time is
    /// injected, never read here. Returns how many petitions expired.
    pub fn expire_pending<F>(
        &mut self,
        now: Instant,
        kernel: &mut GrantKernel,
        send: &mut F,
    ) -> Result<usize, TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        if self.phase == Phase::Dead {
            return Ok(0);
        }
        let due: Vec<u32> = self
            .grants
            .iter()
            .filter_map(|(&id, entry)| match &entry.state {
                PetitionState::Pending(pending) if now >= pending.deadline => Some(id),
                _ => None,
            })
            .collect();
        let mut expired = 0;
        for grant_object in due {
            let entry = self
                .grants
                .get_mut(&grant_object)
                .expect("due id observed above");
            let consent_id = entry.consent_id;
            entry.state = PetitionState::Resolved { table_id: None };
            let identity = self
                .identity
                .as_ref()
                .expect("pending petitions exist only while bound");
            kernel.release_pending(identity);
            Self::send_resolution(
                grant_object,
                consent_id,
                Resolution::Refused(wire_grant::Outcome::TimedOut),
                true,
                send,
            )?;
            expired += 1;
        }
        Ok(expired)
    }

    /// The earliest pending-petition deadline on this connection, for the
    /// embedder's expiry timer (M1.1 wiring: a calloop timer re-armed from
    /// this after every dispatch, like the unauthenticated-phase
    /// deadline). `None` = nothing pending.
    pub fn next_pending_deadline(&self) -> Option<Instant> {
        self.grants
            .values()
            .filter_map(|entry| match &entry.state {
                PetitionState::Pending(pending) => Some(pending.deadline),
                _ => None,
            })
            .min()
    }

    /// Connection teardown: discharge the cross-connection contracts this
    /// connection holds, then go DEAD. The embedder MUST call this when it
    /// closes the connection (clean close or after a fatal):
    ///
    /// - every grant row this connection minted is **removed** from the
    ///   table (version-1 grants die with their connection; removal, not
    ///   revocation -- `grants.rs` module docs carry the contract and name
    ///   this caller);
    /// - every pending petition is **withdrawn**, releasing its admission
    ///   slot ("consent is in-context, so the prompt disappears with the
    ///   petitioner" -- IDL). No events are emitted: the connection that
    ///   would carry them is gone. Dismissing a *rendered* prompt is the
    ///   consent surface's half (P1.7.1 seam).
    ///
    /// Idempotent: a second call finds nothing left to discharge.
    pub fn teardown(&mut self, kernel: &mut GrantKernel) {
        for entry in self.grants.values() {
            match &entry.state {
                PetitionState::Pending(_) => {
                    let identity = self
                        .identity
                        .as_ref()
                        .expect("pending petitions exist only while bound");
                    kernel.release_pending(identity);
                }
                PetitionState::Resolved {
                    table_id: Some(table_id),
                } => {
                    kernel.table_mut().remove(*table_id);
                }
                PetitionState::Resolved { table_id: None } => {}
            }
        }
        self.grants.clear();
        self.petition_objects.clear();
        self.phase = Phase::Dead;
    }

    /// The grant-table row a granted petition minted, by grant handle id
    /// -- the key the enforcement chokepoint (P1.4.4) passes to
    /// [`check_use_grant`](crate::grants::GrantTable::check_use_grant) for
    /// facet-borne uses. `None` while pending or after any non-granted
    /// resolution: the facets are inert there and refuse `not_granted` --
    /// the chokepoint's judgement, not this lookup's.
    pub fn granted_table_id(&self, grant_object: u32) -> Option<GrantId> {
        match self.grants.get(&grant_object)?.state {
            PetitionState::Resolved { table_id } => table_id,
            PetitionState::Pending(_) => None,
        }
    }

    /// A petition companion object's role and owning grant handle -- the
    /// facet-routing lookup the enforcement chokepoint (P1.4.4) starts
    /// from (facet id -> grant handle -> table row -> `check_use_grant`).
    pub fn facet_binding(&self, object_id: u32) -> Option<(FacetRole, u32)> {
        self.petition_objects
            .get(&object_id)
            .map(|obj| (obj.role, obj.grant_object))
    }

    /// Enforce the watermark rule (conventions 3.1) for one `new_id`:
    /// strictly increasing, never reused, inside the client range -- and
    /// the id-space-exhaustion terminal (conventions 3.4) once no legal id
    /// remains.
    fn allocate_id(&mut self, id: u32) -> Result<(), PrincipalViolation> {
        if self.watermark >= CLIENT_ID_MAX {
            return Err(PrincipalViolation::ResourceExhausted(
                "object-id space exhausted",
            ));
        }
        if id <= self.watermark || id > CLIENT_ID_MAX {
            return Err(PrincipalViolation::InvalidObject {
                object_id: id,
                detail: "new_id at/below the watermark or outside the client range",
            });
        }
        self.watermark = id;
        Ok(())
    }

    /// Test-only: place the watermark at the top of the client id range so
    /// the (practically unreachable) exhaustion terminal is testable.
    #[cfg(test)]
    fn exhaust_id_space_for_test(&mut self) {
        self.watermark = CLIENT_ID_MAX;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;

    use vitrin_ipc::Connection;

    use crate::consent::{AutoApproveDecider, EffectiveAuthority, ScriptedDecider};
    use crate::grants::{GrantState, Issuer, RefusalReason};
    use crate::identity::{
        BoundPrincipal, RejectionCause, StaticPrincipal, StaticVerifier, STATIC_TOKEN_SCHEME,
    };

    use super::*;

    const TOKEN: &str = "9b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a";
    const DEMO_IDENTITY: &str = "vitrin://local/agent/demo";

    fn my_uid() -> u32 {
        rustix::process::geteuid().as_raw()
    }

    fn demo_verifier() -> StaticVerifier {
        StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: PrincipalIdentity::parse(DEMO_IDENTITY).unwrap(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            }],
            my_uid(),
        )
        .unwrap()
    }

    /// A fresh per-connection server + socketpair: core end, client end.
    fn setup() -> (PrincipalServer, Connection, Connection) {
        let (core, client) = Connection::pair().expect("socketpair");
        let server = PrincipalServer::new(core.peer_cred());
        (server, core, client)
    }

    /// A kernel with the version-1 realm topology: exactly `realm-0` live.
    fn test_kernel() -> GrantKernel {
        GrantKernel::new([RealmId::new("realm-0")])
    }

    /// Receive and dispatch exactly `n` client messages on the core side,
    /// with explicit petition context (decider + kernel + one clock
    /// sample) -- the full embedder shape. Tests exercising the petition
    /// flow's state MUST use this so admission accounting and time are
    /// real across calls.
    #[allow(clippy::too_many_arguments)]
    fn process_at(
        server: &mut PrincipalServer,
        core: &mut Connection,
        verifier: &dyn Verifier,
        decider: &mut dyn ConsentDecider,
        kernel: &mut GrantKernel,
        now: Instant,
        n: usize,
    ) -> Result<(), PrincipalFault> {
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            server.handle_message(msg, now, verifier, decider, kernel, &mut |frame| {
                core.send_message(frame, None)
            })?;
        }
        Ok(())
    }

    /// Convenience for traffic that never touches petition state (hello,
    /// sync, get_realm, violation probes): a throwaway kernel/decider and
    /// an arbitrary clock sample. Petition tests use `process_at`.
    fn process_n(
        server: &mut PrincipalServer,
        core: &mut Connection,
        verifier: &dyn Verifier,
        n: usize,
    ) -> Result<(), PrincipalFault> {
        let mut kernel = test_kernel();
        process_at(
            server,
            core,
            verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            Instant::now(),
            n,
        )
    }

    fn send_hello(client: &mut Connection, principal_id: u32, identity: &str, credential: &str) {
        send_hello_versioned(client, PROTOCOL_VERSION, principal_id, identity, credential);
    }

    fn send_hello_versioned(
        client: &mut Connection,
        version: u32,
        principal_id: u32,
        identity: &str,
        credential: &str,
    ) {
        let hello = handshake::requests::Hello {
            version,
            principal: principal_id,
            identity: identity.into(),
            credential_type: STATIC_TOKEN_SCHEME.into(),
            credential: credential.into(),
        };
        client
            .send_message(&hello.encode(HANDSHAKE_ID), None)
            .expect("send hello");
    }

    fn send_get_realm(client: &mut Connection, principal_id: u32, realm_id: u32, name: &str) {
        let req = principal::requests::GetRealm {
            realm: realm_id,
            name: name.into(),
        };
        client
            .send_message(&req.encode(principal_id), None)
            .expect("send get_realm");
    }

    fn send_sync(client: &mut Connection, cookie: u32) {
        client
            .send_message(
                &handshake::requests::Sync { cookie }.encode(HANDSHAKE_ID),
                None,
            )
            .expect("send sync");
    }

    /// A well-formed whole-realm petition whose five new_ids are
    /// `first_id..=first_id+4` (grant, consent, view, pointer, text --
    /// contiguous, the SDK allocator's shape), asking for the Phase-1 verb
    /// set, while_running, no time bound, server-default rate.
    fn petition(first_id: u32) -> realm::requests::RequestGrant {
        realm::requests::RequestGrant {
            grant: first_id,
            consent: first_id + 1,
            view: first_id + 2,
            pointer: first_id + 3,
            text: first_id + 4,
            resource: String::new(),
            verbs: Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT,
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: wire_grant::Persistence::WhileRunning,
            flags: 0,
        }
    }

    fn send_petition(
        client: &mut Connection,
        realm_object: u32,
        req: &realm::requests::RequestGrant,
    ) {
        client
            .send_message(&req.encode(realm_object), None)
            .expect("send request_grant");
    }

    /// Assert the next client-visible event is `vitrin_consent.state` on
    /// `consent_id` carrying `state`.
    fn expect_consent_state(
        client: &mut Connection,
        consent_id: u32,
        state: wire_consent::ConsentState,
    ) {
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, event) = wire_consent::events::State::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, consent_id, "state on the co-minted observer");
        assert_eq!(event.state, state);
    }

    /// Assert the next client-visible event is `vitrin_grant.resolved` on
    /// `grant_id`, returning it for outcome/effective assertions.
    fn expect_resolved(client: &mut Connection, grant_id: u32) -> wire_grant::events::Resolved {
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, event) = wire_grant::events::Resolved::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, grant_id, "resolved on the co-minted grant");
        event
    }

    /// Prove the connection is alive and nothing is queued ahead: sync's
    /// done must be the very next client-visible message.
    fn sync_probe(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
        cookie: u32,
    ) {
        send_sync(client, cookie);
        process_n(server, core, verifier, 1).expect("sync must dispatch");
        let msg = client.recv_message().unwrap().unwrap();
        let (_, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(done.cookie, cookie, "done must be the very next event");
    }

    /// Bind and mint `realm-0` as object 3 -- the standard petition
    /// setup; petition ids start at 4.
    fn bind_with_realm0(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
    ) {
        bind(server, core, client, verifier);
        send_get_realm(client, 2, 3, "realm-0");
        process_n(server, core, verifier, 1).expect("get_realm");
    }

    /// Complete a successful handshake with principal id 2 and return the
    /// decoded bound identity.
    fn bind(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
    ) -> String {
        send_hello(client, 2, DEMO_IDENTITY, TOKEN);
        process_n(server, core, verifier, 1).expect("handshake");
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, bound) = principal::events::Bound::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, 2, "bound arrives on the pre-allocated principal");
        bound.identity
    }

    /// Assert the next client-visible event is the fatal goodbye with the
    /// given code, returning the full decoded event for deeper assertions.
    fn expect_error(client: &mut Connection, code: WireError) -> handshake::events::Error {
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, err) = handshake::events::Error::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, HANDSHAKE_ID, "error is an event on object 1");
        assert_eq!(err.code, code, "unexpected wire code: {err:?}");
        err
    }

    fn expect_violation(result: Result<(), PrincipalFault>, want: &str) {
        match result {
            Err(PrincipalFault::Violation(v)) => {
                let text = v.to_string();
                assert!(
                    text.contains(want),
                    "expected a `{want}` violation, got: {text}"
                );
            }
            other => panic!("expected a `{want}` violation, got: {other:?}"),
        }
    }

    // -- acceptance: bind + refusals ---------------------------------------

    #[test]
    fn successful_handshake_sends_bound_with_the_canonical_identity() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        assert!(!server.is_bound());
        let identity = bind(&mut server, &mut core, &mut client, &verifier);
        assert_eq!(identity, DEMO_IDENTITY);
        assert!(server.is_bound());
        assert_eq!(server.bound_identity().unwrap().as_str(), DEMO_IDENTITY);
    }

    #[test]
    fn refused_handshake_is_wire_uniform_across_causes() {
        // Wrong token, missing (empty) token, unknown identity, peercred
        // mismatch, and verifier outage must produce byte-identical error
        // frames: auth_failed with the fixed phrase, no bound, nothing else.
        struct Outage;
        impl Verifier for Outage {
            fn verify(&self, _p: &PresentedCredential<'_>) -> VerifyOutcome {
                VerifyOutcome::Unavailable("SPIRE agent unreachable (simulated)".into())
            }
        }
        // The global anchor requires a uid the peer does not have (per-row
        // pins cannot express this: a pin differing from the anchor is
        // refused at load as unsatisfiable).
        let uid_mismatch = StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: PrincipalIdentity::parse(DEMO_IDENTITY).unwrap(),
                token: TOKEN.as_bytes().to_vec(),
                uid: None,
            }],
            my_uid().wrapping_add(1),
        )
        .unwrap();
        let registry = demo_verifier();
        let cases: [(&dyn Verifier, &str, &str, &str); 5] = [
            (&registry, DEMO_IDENTITY, "wrong-token", "wrong token"),
            (&registry, DEMO_IDENTITY, "", "missing token"),
            (
                &registry,
                "vitrin://local/agent/ghost",
                TOKEN,
                "unknown identity",
            ),
            (&uid_mismatch, DEMO_IDENTITY, TOKEN, "SO_PEERCRED mismatch"),
            (&Outage, DEMO_IDENTITY, TOKEN, "verifier unavailable"),
        ];
        let mut frames: Vec<(Vec<u8>, &str)> = Vec::new();
        for (verifier, identity, credential, label) in cases {
            let (mut server, mut core, mut client) = setup();
            send_hello(&mut client, 2, identity, credential);
            expect_violation(
                process_n(&mut server, &mut core, verifier, 1),
                "auth_failed",
            );
            let mut msg = client.recv_message().unwrap().unwrap();
            let (_, err) = handshake::events::Error::decode(&msg.bytes, msg.fd.take()).unwrap();
            assert_eq!(err.code, WireError::AuthFailed, "{label}");
            assert_eq!(err.message, AUTH_REFUSED_PHRASE, "{label}");
            assert!(!server.is_bound(), "{label}");
            frames.push((msg.bytes, label));
        }
        let (reference, _) = &frames[0];
        for (frame, label) in &frames[1..] {
            assert_eq!(
                frame, reference,
                "{label}: refusal frames must be byte-identical across causes"
            );
        }
    }

    #[test]
    fn refusal_log_detail_names_the_cause_but_never_the_credential() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        send_hello(&mut client, 2, DEMO_IDENTITY, "super-secret-wrong-token");
        let fault = process_n(&mut server, &mut core, &verifier, 1).unwrap_err();
        let log_line = fault.to_string();
        assert!(log_line.contains("does not match the registered token"));
        assert!(log_line.contains(DEMO_IDENTITY));
        assert!(!log_line.contains("super-secret-wrong-token"));
    }

    // -- state machine edges -----------------------------------------------

    #[test]
    fn version_check_precedes_verification() {
        struct Counting<'a> {
            inner: &'a StaticVerifier,
            calls: &'a Cell<u32>,
        }
        impl Verifier for Counting<'_> {
            fn verify(&self, p: &PresentedCredential<'_>) -> VerifyOutcome {
                self.calls.set(self.calls.get() + 1);
                self.inner.verify(p)
            }
        }
        let registry = demo_verifier();
        let calls = Cell::new(0);
        let counting = Counting {
            inner: &registry,
            calls: &calls,
        };

        let (mut server, mut core, mut client) = setup();
        send_hello_versioned(&mut client, PROTOCOL_VERSION + 1, 2, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &counting, 1),
            "version_unsupported",
        );
        let err = expect_error(&mut client, WireError::VersionUnsupported);
        assert_eq!(calls.get(), 0, "the verifier must never see the credential");
        // No supported-version hint: downgrade is refusal, not negotiation.
        assert!(!err.message.contains('1'));
    }

    #[test]
    fn traffic_before_hello_is_pre_handshake() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        send_sync(&mut client, 7);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "pre_handshake",
        );
        expect_error(&mut client, WireError::PreHandshake);
    }

    #[test]
    fn malformed_hello_dies_by_grammar_before_the_verifier_runs() {
        struct Panicking;
        impl Verifier for Panicking {
            fn verify(&self, _p: &PresentedCredential<'_>) -> VerifyOutcome {
                panic!("grammar must fail before verification");
            }
        }
        // A hello frame whose identity string is invalid UTF-8: header and
        // strings hand-assembled around the generated encoders.
        let mut frame = Vec::new();
        vitrin_protocol::wire::FrameHeader {
            object_id: HANDSHAKE_ID,
            size: 0,
            opcode: handshake::requests::Hello::OPCODE,
            fd_count: 0,
        }
        .encode_with_placeholder_size(&mut frame);
        vitrin_protocol::wire::write_uint(&mut frame, PROTOCOL_VERSION);
        vitrin_protocol::wire::write_uint(&mut frame, 2); // principal new_id
        vitrin_protocol::wire::write_uint(&mut frame, 2); // identity length...
        frame.extend_from_slice(&[0xff, 0xfe, 0, 0]); // ...invalid UTF-8 + pad
        vitrin_protocol::wire::write_string(&mut frame, STATIC_TOKEN_SCHEME, 32);
        vitrin_protocol::wire::write_string(&mut frame, TOKEN, 32768);
        vitrin_protocol::wire::patch_size(&mut frame);

        let (mut server, mut core, mut client) = setup();
        client.send_message(&frame, None).unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &Panicking, 1),
            "malformed",
        );
        let err = expect_error(&mut client, WireError::InvalidArgument);
        assert_eq!(
            err.object_id, HANDSHAKE_ID,
            "a malformed hello cites object 1"
        );
    }

    #[test]
    fn malformed_frames_cite_the_object_they_targeted() {
        // The IDL defines error.object_id as the id of the object where the
        // error occurred, "which may be 1" -- not always 1. A malformed
        // get_realm on the bound principal (object 2) must cite object 2,
        // so client-side debugging is not misdirected to the handshake
        // object. Here the name argument is invalid UTF-8.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        let mut frame = Vec::new();
        vitrin_protocol::wire::FrameHeader {
            object_id: 2,
            size: 0,
            opcode: principal::requests::GetRealm::OPCODE,
            fd_count: 0,
        }
        .encode_with_placeholder_size(&mut frame);
        vitrin_protocol::wire::write_uint(&mut frame, 3); // realm new_id
        vitrin_protocol::wire::write_uint(&mut frame, 2); // name length...
        frame.extend_from_slice(&[0xff, 0xfe, 0, 0]); // ...invalid UTF-8 + pad
        vitrin_protocol::wire::patch_size(&mut frame);

        client.send_message(&frame, None).unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "malformed message on object 2",
        );
        let err = expect_error(&mut client, WireError::InvalidArgument);
        assert_eq!(
            err.object_id, 2,
            "the citation names the object the frame targeted"
        );
    }

    #[test]
    fn second_hello_is_invalid_opcode() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        send_hello(&mut client, 3, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "second hello",
        );
        expect_error(&mut client, WireError::InvalidOpcode);
    }

    #[test]
    fn sync_answers_done_after_bound() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        send_sync(&mut client, 0xdead_beef);
        process_n(&mut server, &mut core, &verifier, 1).unwrap();
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, HANDSHAKE_ID);
        assert_eq!(done.cookie, 0xdead_beef);
    }

    #[test]
    fn unknown_opcodes_are_invalid_opcode() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        // Opcode 9 exists on no version-1 interface; try it on the
        // handshake object and on the principal.
        for object_id in [HANDSHAKE_ID, 2] {
            let mut frame = Vec::new();
            vitrin_protocol::wire::FrameHeader {
                object_id,
                size: 0,
                opcode: 9,
                fd_count: 0,
            }
            .encode_with_placeholder_size(&mut frame);
            vitrin_protocol::wire::patch_size(&mut frame);
            client.send_message(&frame, None).unwrap();
            let result = process_n(&mut server, &mut core, &verifier, 1);
            expect_violation(result, "invalid_opcode");
            expect_error(&mut client, WireError::InvalidOpcode);
            // Each fatal kills the connection; re-handshake on a fresh one.
            (server, core, client) = setup();
            bind(&mut server, &mut core, &mut client, &verifier);
        }
    }

    // -- object graph ------------------------------------------------------

    #[test]
    fn get_realm_mints_under_the_watermark_rule() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        send_get_realm(&mut client, 2, 3, "realm-0");
        process_n(&mut server, &mut core, &verifier, 1).unwrap();
        assert_eq!(server.realms.get(&3).map(String::as_str), Some("realm-0"));

        // Reusing an id at/below the watermark is fatal invalid_object.
        send_get_realm(&mut client, 2, 3, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "invalid_object",
        );
        expect_error(&mut client, WireError::InvalidObject);
    }

    #[test]
    fn hello_new_id_must_respect_the_watermark_rule() {
        let verifier = demo_verifier();
        // Id 1 is the bootstrap object: claiming it as the principal new_id
        // is at/below the watermark, fatal invalid_object -- before any
        // verification happens.
        let (mut server, mut core, mut client) = setup();
        send_hello(&mut client, HANDSHAKE_ID, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "invalid_object",
        );
        expect_error(&mut client, WireError::InvalidObject);

        // The reserved server range is equally out.
        let (mut server, mut core, mut client) = setup();
        send_hello(&mut client, 0xff00_0000, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "invalid_object",
        );
        expect_error(&mut client, WireError::InvalidObject);
    }

    #[test]
    fn realm_cap_is_resource_exhausted() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        for i in 0..MAX_LIVE_REALMS as u32 {
            send_get_realm(&mut client, 2, 3 + i, "realm-0");
        }
        process_n(&mut server, &mut core, &verifier, MAX_LIVE_REALMS).unwrap();
        send_get_realm(&mut client, 2, 100, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "resource_exhausted",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn id_space_exhaustion_is_resource_exhausted() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        server.exhaust_id_space_for_test();
        send_get_realm(&mut client, 2, CLIENT_ID_MAX, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "object-id space exhausted",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    // -- P1.4.3: the petition flow (pending -> consent -> resolution) ------

    #[test]
    fn auto_approve_grants_the_walking_skeleton_petition() {
        // The `--consent=auto-approve` path (M1.1, prose flow 1): petition
        // -> state(closed) -> resolved(granted, echo of the request), and
        // the minted handle is usable at the enforcement chokepoint's
        // exact seam.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let t0 = Instant::now();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);

        send_petition(&mut client, 3, &petition(4));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();

        // Transitions before the terminal, each on its co-minted object.
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, wire_grant::Outcome::Granted);
        assert_eq!(
            resolved.verbs,
            Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT
        );
        assert_eq!(resolved.persistence, wire_grant::Persistence::WhileRunning);
        assert_eq!(resolved.expiry_ms, 0);

        // Handle usable: `check_use_grant` -- the exact query P1.4.4's
        // chokepoint will issue for facet-borne uses -- admits every
        // granted verb under the minted row.
        let table_id = server.granted_table_id(4).expect("granted");
        let identity = server.bound_identity().unwrap().clone();
        for verb in [Verb::OBSERVE, Verb::ACTUATE_POINTER, Verb::ACTUATE_TEXT] {
            let allowed = kernel
                .table_mut()
                .check_use_grant(table_id, &identity, verb, t0)
                .expect("granted verb must be admitted");
            assert_eq!(allowed.grant_id, table_id);
            // The wire's rate 0 resolved to the concrete default ceiling
            // before the row was written -- never "unlimited".
            assert_eq!(allowed.max_event_rate, DEFAULT_MAX_EVENT_RATE);
        }
        let (row, state) = kernel.table().get(table_id, t0).unwrap();
        assert_eq!(row.issuer, Issuer::AutoApprovePolicy);
        assert_eq!(state, GrantState::Active);
        assert_eq!(kernel.pending_total(), 0, "auto-approve never pends");

        // The facet-routing table P1.4.4 starts from is in place, with
        // the verb each facet exercises (consent carries no authority).
        assert_eq!(server.facet_binding(5), Some((FacetRole::Consent, 4)));
        assert_eq!(server.facet_binding(6), Some((FacetRole::View, 4)));
        assert_eq!(server.facet_binding(7), Some((FacetRole::Pointer, 4)));
        assert_eq!(server.facet_binding(8), Some((FacetRole::Text, 4)));
        assert_eq!(FacetRole::Consent.verb(), None);
        assert_eq!(FacetRole::View.verb(), Some(Verb::OBSERVE));
        assert_eq!(FacetRole::Pointer.verb(), Some(Verb::ACTUATE_POINTER));
        assert_eq!(FacetRole::Text.verb(), Some(Verb::ACTUATE_TEXT));
        // And the connection is fully alive afterwards.
        sync_probe(&mut server, &mut core, &mut client, &verifier, 11);
    }

    #[test]
    fn pending_petition_parks_queued_then_approval_mints_the_grant() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let mut decider = ScriptedDecider::holding();
        let t0 = Instant::now();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);

        let mut req = petition(4);
        req.expiry_ms = 600_000;
        req.max_event_rate = 25;
        send_petition(&mut client, 3, &req);
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();

        // Parked: state(queued), the admission slot held, the petition
        // shown to the consent seam, the deadline armed.
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Queued);
        assert_eq!(decider.seen, vec![4]);
        assert_eq!(kernel.pending_total(), 1);
        assert_eq!(
            server.next_pending_deadline(),
            Some(t0 + PENDING_CONSENT_TIMEOUT)
        );
        assert_eq!(server.granted_table_id(4), None, "nothing minted yet");
        // resolved is exempt from the sync barrier (it waits on the
        // human): done answers while the petition is pending, and nothing
        // else is queued ahead of it.
        sync_probe(&mut server, &mut core, &mut client, &verifier, 21);

        // The human approves, narrower than requested: observe only,
        // Allow-once instead of while-running, half the expiry.
        let resolved_at = t0 + Duration::from_secs(30);
        let did = server
            .resolve_pending(
                4,
                ConsentDecision::Granted(EffectiveAuthority {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::Once,
                    expiry_ms: 300_000,
                    issuer: Issuer::HumanConsent,
                }),
                resolved_at,
                &mut kernel,
                &mut |frame| core.send_message(frame, None),
            )
            .unwrap();
        assert!(did);
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, wire_grant::Outcome::Granted);
        assert_eq!(resolved.verbs, Verb::OBSERVE);
        assert_eq!(resolved.persistence, wire_grant::Persistence::Once);
        assert_eq!(resolved.expiry_ms, 300_000);

        // The EFFECTIVE authority landed in the row, and the chokepoint
        // seam admits exactly it: the granted verb (which, on this `once`
        // rung, its first use spends) but never the narrowed-away one.
        let table_id = server.granted_table_id(4).unwrap();
        let identity = server.bound_identity().unwrap().clone();
        let (row, _) = kernel.table().get(table_id, resolved_at).unwrap();
        assert_eq!(row.issuer, Issuer::HumanConsent);
        assert_eq!(row.constraints.expiry, Some(Duration::from_millis(300_000)));
        assert_eq!(row.constraints.max_event_rate.get(), 25);
        assert_eq!(
            kernel.table_mut().check_use_grant(
                table_id,
                &identity,
                Verb::ACTUATE_POINTER,
                resolved_at
            ),
            Err(RefusalReason::NotGranted),
            "the narrowed-away verb was never conferred"
        );
        assert!(kernel
            .table_mut()
            .check_use_grant(table_id, &identity, Verb::OBSERVE, resolved_at)
            .is_ok());
        // The admission slot is free again; the deadline disarmed.
        assert_eq!(kernel.pending_total(), 0);
        assert_eq!(server.next_pending_deadline(), None);
    }

    #[test]
    fn pending_petition_denied_is_a_clean_event_and_the_connection_lives() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let mut decider = ScriptedDecider::holding();
        let t0 = Instant::now();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
        send_petition(&mut client, 3, &petition(4));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Queued);

        let did = server
            .resolve_pending(4, ConsentDecision::Denied, t0, &mut kernel, &mut |frame| {
                core.send_message(frame, None)
            })
            .unwrap();
        assert!(did);
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, wire_grant::Outcome::Denied);
        // Zeros on a non-granted resolution (IDL).
        assert_eq!(resolved.verbs.bits(), 0);
        assert_eq!(resolved.persistence, wire_grant::Persistence::Once);
        assert_eq!(resolved.expiry_ms, 0);

        // Denial is an answer, not a violation: no row, no residue, and a
        // second resolution attempt is a benign no-op (double click).
        assert_eq!(server.granted_table_id(4), None);
        assert_eq!(kernel.pending_total(), 0);
        assert_eq!(kernel.table().rows(t0).count(), 0);
        assert!(!server
            .resolve_pending(4, ConsentDecision::Denied, t0, &mut kernel, &mut |frame| {
                core.send_message(frame, None)
            })
            .unwrap());
        assert!(!server
            .resolve_pending(
                999,
                ConsentDecision::Denied,
                t0,
                &mut kernel,
                &mut |frame| { core.send_message(frame, None) }
            )
            .unwrap());
        sync_probe(&mut server, &mut core, &mut client, &verifier, 31);

        // The same connection may petition again -- approved this time.
        send_petition(&mut client, 3, &petition(9));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 10, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client, 9).outcome,
            wire_grant::Outcome::Granted
        );
        assert!(server.granted_table_id(9).is_some());
    }

    #[test]
    fn pending_petition_times_out_with_a_clean_event_at_the_exact_deadline() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let mut decider = ScriptedDecider::holding();
        let t0 = Instant::now();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
        send_petition(&mut client, 3, &petition(4));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Queued);

        // Strictly before the deadline nothing expires and nothing is
        // delivered (the sync probe would surface any stray event).
        let just_before = t0 + PENDING_CONSENT_TIMEOUT - Duration::from_millis(1);
        assert_eq!(
            server
                .expire_pending(just_before, &mut kernel, &mut |frame| {
                    core.send_message(frame, None)
                })
                .unwrap(),
            0
        );
        sync_probe(&mut server, &mut core, &mut client, &verifier, 41);
        assert_eq!(kernel.pending_total(), 1);

        // At exactly the deadline: fail-closed expiry, clean events.
        let deadline = t0 + PENDING_CONSENT_TIMEOUT;
        assert_eq!(
            server
                .expire_pending(deadline, &mut kernel, &mut |frame| {
                    core.send_message(frame, None)
                })
                .unwrap(),
            1
        );
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, wire_grant::Outcome::TimedOut);
        assert_eq!(resolved.verbs.bits(), 0);

        // Nothing leaks: no row, no admission residue, deadline disarmed,
        // and a late human answer to the expired petition is a no-op.
        assert_eq!(kernel.table().rows(deadline).count(), 0);
        assert_eq!(kernel.pending_total(), 0);
        assert_eq!(server.next_pending_deadline(), None);
        assert!(!server
            .resolve_pending(
                4,
                ConsentDecision::Denied,
                deadline,
                &mut kernel,
                &mut |frame| core.send_message(frame, None)
            )
            .unwrap());
        // The connection lives and the watermark is uncorrupted: the next
        // petition mints the next five ids and resolves ("petitioning
        // again later is legal" -- IDL).
        sync_probe(&mut server, &mut core, &mut client, &verifier, 42);
        send_petition(&mut client, 3, &petition(9));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            deadline,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 10, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client, 9).outcome,
            wire_grant::Outcome::Granted
        );
    }

    #[test]
    fn concurrent_duplicate_petitions_resolve_busy_across_connections() {
        // The no-coalescing decision (crate::consent): a second concurrent
        // petition from one verified identity resolves busy -- on the same
        // connection AND on a different connection of the same identity
        // (the IDL's across-connections cap) -- while the petition in
        // flight is untouched.
        let verifier = demo_verifier();
        let mut kernel = test_kernel();
        let mut decider = ScriptedDecider::holding();
        let t0 = Instant::now();

        let (mut server_a, mut core_a, mut client_a) = setup();
        bind_with_realm0(&mut server_a, &mut core_a, &mut client_a, &verifier);
        send_petition(&mut client_a, 3, &petition(4));
        process_at(
            &mut server_a,
            &mut core_a,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client_a, 5, wire_consent::ConsentState::Queued);

        // Same connection: busy, immediately, with no consent transition
        // (no prompt opened -- resolved is the very next event).
        send_petition(&mut client_a, 3, &petition(9));
        process_at(
            &mut server_a,
            &mut core_a,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        assert_eq!(
            expect_resolved(&mut client_a, 9).outcome,
            wire_grant::Outcome::Busy
        );
        assert_eq!(
            decider.seen,
            vec![4],
            "the excess petition never reached consent"
        );

        // Different connection, same verified identity: busy too.
        let (mut server_b, mut core_b, mut client_b) = setup();
        bind_with_realm0(&mut server_b, &mut core_b, &mut client_b, &verifier);
        send_petition(&mut client_b, 3, &petition(4));
        process_at(
            &mut server_b,
            &mut core_b,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        assert_eq!(
            expect_resolved(&mut client_b, 4).outcome,
            wire_grant::Outcome::Busy
        );

        // The in-flight petition still resolves; its slot then admits B.
        assert!(server_a
            .resolve_pending(4, ConsentDecision::Denied, t0, &mut kernel, &mut |frame| {
                core_a.send_message(frame, None)
            })
            .unwrap());
        expect_consent_state(&mut client_a, 5, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client_a, 4).outcome,
            wire_grant::Outcome::Denied
        );
        send_petition(&mut client_b, 3, &petition(9));
        process_at(
            &mut server_b,
            &mut core_b,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client_b, 10, wire_consent::ConsentState::Queued);
        assert_eq!(kernel.pending_total(), 1);
    }

    #[test]
    fn unserved_petitions_resolve_unsupported_without_consuming_anything() {
        // Durable rungs (absent-not-hidden, via grants.rs's typed
        // conversion -> the wire's unsupported -> SDK GrantUnsupported), a
        // set reserved flags bit, and finer-than-whole-realm resource
        // selectors (the documented Phase-2 seam) all resolve unsupported:
        // recoverable, no consent transition, no admission slot, no row --
        // and the decider is never consulted.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let mut decider = ScriptedDecider::holding();
        let t0 = Instant::now();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);

        let mut cases: Vec<realm::requests::RequestGrant> = Vec::new();
        let mut req = petition(4);
        req.persistence = wire_grant::Persistence::UntilRevoked;
        cases.push(req);
        let mut req = petition(9);
        req.persistence = wire_grant::Persistence::Always;
        cases.push(req);
        let mut req = petition(14);
        req.flags = 1; // reserved one_shot bit
        cases.push(req);
        let mut req = petition(19);
        req.resource = "surface:main".into();
        cases.push(req);
        let mut req = petition(24);
        req.resource = "x".into(); // any non-empty selector, not just known prefixes
        cases.push(req);

        for req in &cases {
            send_petition(&mut client, 3, req);
            process_at(
                &mut server,
                &mut core,
                &verifier,
                &mut decider,
                &mut kernel,
                t0,
                1,
            )
            .unwrap();
            let resolved = expect_resolved(&mut client, req.grant);
            assert_eq!(resolved.outcome, wire_grant::Outcome::Unsupported);
            assert_eq!(resolved.verbs.bits(), 0);
        }
        assert_eq!(kernel.table().rows(t0).count(), 0);
        assert_eq!(kernel.pending_total(), 0);
        assert!(
            decider.seen.is_empty(),
            "policy refusals never reach consent"
        );

        // A clean petition on the same connection still succeeds.
        send_petition(&mut client, 3, &petition(29));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 30, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client, 29).outcome,
            wire_grant::Outcome::Granted
        );
    }

    #[test]
    fn a_petition_on_an_unknown_realm_resolves_unavailable() {
        // Naming is not authority: the handle mints structurally and the
        // petition resolves unavailable (prose flow 5) -- recoverable, no
        // consent involvement.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let mut decider = ScriptedDecider::holding();
        let t0 = Instant::now();
        bind(&mut server, &mut core, &mut client, &verifier);
        send_get_realm(&mut client, 2, 3, "realm-9");
        process_n(&mut server, &mut core, &verifier, 1).unwrap();
        send_petition(&mut client, 3, &petition(4));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        assert_eq!(
            expect_resolved(&mut client, 4).outcome,
            wire_grant::Outcome::Unavailable
        );
        assert!(decider.seen.is_empty());
        assert_eq!(kernel.pending_total(), 0);

        // The live realm remains petitionable from the same connection.
        send_get_realm(&mut client, 2, 9, "realm-0");
        send_petition(&mut client, 9, &petition(10));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            t0,
            2,
        )
        .unwrap();
        expect_consent_state(&mut client, 11, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client, 10).outcome,
            wire_grant::Outcome::Granted
        );
    }

    #[test]
    fn consent_widening_is_clamped_fail_closed() {
        // Consent narrows, never widens: a decision wider than the
        // petition on every axis is clamped to the petition; a decision
        // disjoint from it confers nothing and resolves denied.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let t0 = Instant::now();
        let mut decider = ScriptedDecider::scripted([
            ConsentVerdict::Decided(ConsentDecision::Granted(EffectiveAuthority {
                verbs: Verb::OBSERVE | Verb::ACTUATE_TEXT, // wider than requested
                persistence: PersistenceRung::WhileRunning, // wider rung
                expiry_ms: 0,                              // unbounded: wider
                issuer: Issuer::HumanConsent,
            })),
            ConsentVerdict::Decided(ConsentDecision::Granted(EffectiveAuthority {
                verbs: Verb::ACTUATE_TEXT, // disjoint from the petition
                persistence: PersistenceRung::Once,
                expiry_ms: 5_000,
                issuer: Issuer::HumanConsent,
            })),
        ]);
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);

        let mut req = petition(4);
        req.verbs = Verb::OBSERVE;
        req.persistence = wire_grant::Persistence::Once;
        req.expiry_ms = 5_000;
        send_petition(&mut client, 3, &req);
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, wire_grant::Outcome::Granted);
        assert_eq!(resolved.verbs, Verb::OBSERVE);
        assert_eq!(resolved.persistence, wire_grant::Persistence::Once);
        assert_eq!(resolved.expiry_ms, 5_000);

        let mut req = petition(9);
        req.verbs = Verb::OBSERVE;
        send_petition(&mut client, 3, &req);
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client, 10, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client, 9).outcome,
            wire_grant::Outcome::Denied
        );
        assert_eq!(server.granted_table_id(9), None);
        assert_eq!(
            kernel.table().rows(t0).count(),
            1,
            "only the clamped grant's row exists"
        );
    }

    #[test]
    fn an_empty_verb_petition_is_fatal_invalid_argument() {
        // IDL: verbs MUST be non-zero -- an empty petition is a client
        // bug, fatal invalid_argument, citing the realm handle.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
        let mut req = petition(4);
        req.verbs = Verb::default();
        send_petition(&mut client, 3, &req);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "invalid_argument",
        );
        let err = expect_error(&mut client, WireError::InvalidArgument);
        assert_eq!(err.object_id, 3, "cites the realm handle petitioned");
    }

    #[test]
    fn multi_new_id_violations_are_fatal_invalid_object() {
        // Conventions 3.2: the five ids MUST be distinct, strictly
        // increasing in argument order, and all above the watermark.
        let verifier = demo_verifier();
        let cases: [([u32; 5], &str); 4] = [
            ([4, 4, 5, 6, 7], "duplicate id"),
            ([5, 4, 6, 7, 8], "non-increasing order"),
            ([2, 5, 6, 7, 8], "id at/below the watermark"),
            ([4, 5, 6, 7, 0xff00_0000], "server-reserved range"),
        ];
        for (ids, label) in cases {
            let (mut server, mut core, mut client) = setup();
            bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
            let mut req = petition(0);
            [req.grant, req.consent, req.view, req.pointer, req.text] =
                [ids[0], ids[1], ids[2], ids[3], ids[4]];
            send_petition(&mut client, 3, &req);
            let result = process_n(&mut server, &mut core, &verifier, 1);
            expect_violation(result, "invalid_object");
            expect_error(&mut client, WireError::InvalidObject);
            let _ = label;
        }
    }

    #[test]
    fn the_petition_cap_is_resource_exhausted() {
        // Every petition permanently allocates five ids; the documented
        // per-connection cap confines a petition-spinning client to its
        // own connection (fatal resource_exhausted -- IDL).
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        let mut kernel = test_kernel();
        let t0 = Instant::now();
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
        let mut first_id = 4;
        for _ in 0..MAX_LIVE_PETITIONS {
            send_petition(&mut client, 3, &petition(first_id));
            first_id += 5;
        }
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut AutoApproveDecider,
            &mut kernel,
            t0,
            MAX_LIVE_PETITIONS,
        )
        .unwrap();
        // Drain the per-petition closed + resolved pairs.
        for _ in 0..MAX_LIVE_PETITIONS * 2 {
            client.recv_message().unwrap().unwrap();
        }
        send_petition(&mut client, 3, &petition(first_id));
        expect_violation(
            process_at(
                &mut server,
                &mut core,
                &verifier,
                &mut AutoApproveDecider,
                &mut kernel,
                t0,
                1,
            ),
            "resource_exhausted",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn teardown_removes_grants_and_withdraws_pending_petitions() {
        // grants.rs's documented teardown contract, discharged by this
        // caller: rows are REMOVED (not revoked) and pending petitions
        // withdrawn, releasing their admission slots for a successor
        // connection of the same identity.
        let verifier = demo_verifier();
        let mut kernel = test_kernel();
        let t0 = Instant::now();
        let (mut server, mut core, mut client) = setup();
        let mut decider = ScriptedDecider::scripted([ConsentVerdict::Decided(
            ConsentDecision::Granted(EffectiveAuthority {
                verbs: Verb::OBSERVE,
                persistence: PersistenceRung::WhileRunning,
                expiry_ms: 0,
                issuer: Issuer::HumanConsent,
            }),
        )]); // first petition granted; the second holds pending
        bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
        send_petition(&mut client, 3, &petition(4));
        send_petition(&mut client, 3, &petition(9));
        process_at(
            &mut server,
            &mut core,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            2,
        )
        .unwrap();
        expect_consent_state(&mut client, 5, wire_consent::ConsentState::Closed);
        assert_eq!(
            expect_resolved(&mut client, 4).outcome,
            wire_grant::Outcome::Granted
        );
        expect_consent_state(&mut client, 10, wire_consent::ConsentState::Queued);
        let table_id = server.granted_table_id(4).unwrap();
        assert!(kernel.table().get(table_id, t0).is_some());
        assert_eq!(kernel.pending_total(), 1);

        server.teardown(&mut kernel);
        assert!(
            kernel.table().get(table_id, t0).is_none(),
            "removal, not revocation: the row is gone outright"
        );
        assert_eq!(kernel.pending_total(), 0);
        // Idempotent; and the server is DEAD afterwards (defense in depth
        // against post-teardown dispatch).
        server.teardown(&mut kernel);
        send_sync(&mut client, 7);
        expect_violation(
            process_at(
                &mut server,
                &mut core,
                &verifier,
                &mut decider,
                &mut kernel,
                t0,
                1,
            ),
            "dead connection",
        );

        // A successor connection of the same identity parks pending again
        // -- nothing leaked into the admission caps.
        let (mut server_b, mut core_b, mut client_b) = setup();
        bind_with_realm0(&mut server_b, &mut core_b, &mut client_b, &verifier);
        send_petition(&mut client_b, 3, &petition(4));
        process_at(
            &mut server_b,
            &mut core_b,
            &verifier,
            &mut decider,
            &mut kernel,
            t0,
            1,
        )
        .unwrap();
        expect_consent_state(&mut client_b, 5, wire_consent::ConsentState::Queued);
        assert_eq!(kernel.pending_total(), 1);
    }

    #[test]
    fn petition_objects_route_to_the_p144_seam_never_invalid_object() {
        // The quintet's objects EXIST on the connection whatever P1.4.4's
        // status: defined facet requests die an honest fatal internal (the
        // marked enforcement seam -- a missing object would instead be
        // invalid_object), and undefined opcodes on any of the five stay
        // invalid_opcode.
        let verifier = demo_verifier();
        let cases: [(u32, u8, &str, WireError); 8] = [
            (
                6,
                view::requests::CaptureFrame::OPCODE,
                "not implemented",
                WireError::Internal,
            ),
            (
                7,
                pointer::requests::Move::OPCODE,
                "not implemented",
                WireError::Internal,
            ),
            (
                7,
                pointer::requests::Button::OPCODE,
                "not implemented",
                WireError::Internal,
            ),
            (
                7,
                pointer::requests::Scroll::OPCODE,
                "not implemented",
                WireError::Internal,
            ),
            (
                8,
                text::requests::Type::OPCODE,
                "not implemented",
                WireError::Internal,
            ),
            // vitrin_consent and vitrin_grant define no requests at all.
            (5, 0, "invalid_opcode", WireError::InvalidOpcode),
            (4, 0, "invalid_opcode", WireError::InvalidOpcode),
            // An opcode outside a facet's interface.
            (6, 9, "invalid_opcode", WireError::InvalidOpcode),
        ];
        for (object_id, opcode, want, wire_code) in cases {
            let (mut server, mut core, mut client) = setup();
            let mut kernel = test_kernel();
            bind_with_realm0(&mut server, &mut core, &mut client, &verifier);
            send_petition(&mut client, 3, &petition(4));
            process_at(
                &mut server,
                &mut core,
                &verifier,
                &mut AutoApproveDecider,
                &mut kernel,
                Instant::now(),
                1,
            )
            .unwrap();
            // Drain closed + resolved.
            client.recv_message().unwrap().unwrap();
            client.recv_message().unwrap().unwrap();

            let mut frame = Vec::new();
            vitrin_protocol::wire::FrameHeader {
                object_id,
                size: 0,
                opcode,
                fd_count: 0,
            }
            .encode_with_placeholder_size(&mut frame);
            vitrin_protocol::wire::patch_size(&mut frame);
            client.send_message(&frame, None).unwrap();
            let result = process_at(
                &mut server,
                &mut core,
                &verifier,
                &mut AutoApproveDecider,
                &mut kernel,
                Instant::now(),
                1,
            );
            expect_violation(result, want);
            expect_error(&mut client, wire_code);
        }
    }

    // -- acceptance: sender constraint -------------------------------------

    #[test]
    fn handles_are_sender_constrained_across_connections() {
        // Connection A binds and mints realm handle 3. Connection B binds
        // with the same verifier and presents A's handle: B's per-connection
        // table does not know it, so B dies fatal invalid_object -- while A
        // and its handle stay fully live. This is the D2 per-connection id
        // model enforced end to end: the identity layer introduces no
        // cross-connection handle namespace.
        let verifier = demo_verifier();
        let (mut server_a, mut core_a, mut client_a) = setup();
        bind(&mut server_a, &mut core_a, &mut client_a, &verifier);
        send_get_realm(&mut client_a, 2, 3, "realm-0");
        process_n(&mut server_a, &mut core_a, &verifier, 1).unwrap();

        let (mut server_b, mut core_b, mut client_b) = setup();
        bind(&mut server_b, &mut core_b, &mut client_b, &verifier);
        // B presents A's realm handle (id 3). B's own table has ids 1 and 2
        // only. B could even have minted nothing: the number is meaningless
        // outside the connection that allocated it.
        let foreign = petition(4);
        client_b.send_message(&foreign.encode(3), None).unwrap();
        expect_violation(
            process_n(&mut server_b, &mut core_b, &verifier, 1),
            "unknown or foreign object id",
        );
        expect_error(&mut client_b, WireError::InvalidObject);
        assert_eq!(server_b.phase, Phase::Dead, "B's connection must be dead");

        // A is untouched: its handle still exists and its connection still
        // answers the sync barrier.
        assert!(server_a.is_bound());
        assert_eq!(server_a.realms.get(&3).map(String::as_str), Some("realm-0"));
        send_sync(&mut client_a, 99);
        process_n(&mut server_a, &mut core_a, &verifier, 1).unwrap();
        let msg = client_a.recv_message().unwrap().unwrap();
        let (_, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(done.cookie, 99);
    }

    // -- acceptance: failed handshake drops pipelined traffic --------------

    #[test]
    fn requests_pipelined_behind_a_failed_hello_are_never_processed() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        // Pipeline: bad hello + get_realm in one burst.
        send_hello(&mut client, 2, DEMO_IDENTITY, "wrong-token");
        send_get_realm(&mut client, 2, 3, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "auth_failed",
        );
        // The embedder contract closes here. Even if a buggy embedder kept
        // dispatching, the DEAD phase refuses to process the queued mint.
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "dead connection",
        );
        assert!(server.realms.is_empty(), "the queued mint must not execute");
    }

    #[test]
    fn requests_pipelined_behind_a_successful_hello_are_served_after_bound() {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        // hello + get_realm + sync pipelined in one burst (Flow 2).
        send_hello(&mut client, 2, DEMO_IDENTITY, TOKEN);
        send_get_realm(&mut client, 2, 3, "realm-0");
        send_sync(&mut client, 5);
        process_n(&mut server, &mut core, &verifier, 3).unwrap();
        // Client-visible order: bound first, then done; the mint executed.
        let msg = client.recv_message().unwrap().unwrap();
        let (_, bound) = principal::events::Bound::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(bound.identity, DEMO_IDENTITY);
        let msg = client.recv_message().unwrap().unwrap();
        let (_, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(done.cookie, 5);
        assert_eq!(server.realms.get(&3).map(String::as_str), Some("realm-0"));
    }

    // -- acceptance: the trait fits a non-static verifier ------------------

    #[test]
    fn a_mock_nonstatic_verifier_fits_the_trait_shape() {
        // The future-proofing check made concrete: a directory-backed
        // verifier standing in for an SVID/OIDC verifier -- it consults
        // out-of-registry state, *canonicalizes* the claimed identity
        // (lower-cases the trust domain, as SPIFFE normalization would), and
        // can be unavailable. It runs through the exact same server path as
        // StaticVerifier, as a trait object, with no signature accommodation.
        struct DirectoryVerifier {
            directory: HashMap<String, String>, // canonical -> expected credential
            outage: Cell<bool>,
        }
        impl Verifier for DirectoryVerifier {
            fn verify(&self, p: &PresentedCredential<'_>) -> VerifyOutcome {
                if self.outage.get() {
                    return VerifyOutcome::Unavailable("directory unreachable".into());
                }
                let (scheme, rest) = p.claimed_identity.split_once("://").unwrap_or(("", ""));
                let (authority, path) = rest.split_once('/').unwrap_or(("", ""));
                let canonical = format!("{scheme}://{}/{path}", authority.to_ascii_lowercase());
                match self.directory.get(&canonical) {
                    Some(expected) if expected.as_bytes() == p.credential => {
                        VerifyOutcome::Bound(BoundPrincipal {
                            identity: PrincipalIdentity::parse(&canonical).unwrap(),
                        })
                    }
                    Some(_) => VerifyOutcome::Rejected(RejectionCause::BadToken),
                    None => VerifyOutcome::Rejected(RejectionCause::UnknownIdentity),
                }
            }
        }
        let verifier = DirectoryVerifier {
            directory: HashMap::from([(
                "spiffe://prod.example/workload/scraper".to_owned(),
                "directory-credential-0123".to_owned(),
            )]),
            outage: Cell::new(false),
        };

        // Bound identity is the verifier-canonical form, not the claimed
        // echo: the claimed trust domain is upper-cased on the wire.
        let (mut server, mut core, mut client) = setup();
        send_hello(
            &mut client,
            2,
            "spiffe://PROD.example/workload/scraper",
            "directory-credential-0123",
        );
        process_n(&mut server, &mut core, &verifier as &dyn Verifier, 1).unwrap();
        let msg = client.recv_message().unwrap().unwrap();
        let (_, bound) = principal::events::Bound::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(bound.identity, "spiffe://prod.example/workload/scraper");
        assert_ne!(bound.identity, "spiffe://PROD.example/workload/scraper");

        // An outage is wire-uniform auth_failed, like every refusal.
        verifier.outage.set(true);
        let (mut server, mut core, mut client) = setup();
        send_hello(
            &mut client,
            2,
            "spiffe://prod.example/workload/scraper",
            "directory-credential-0123",
        );
        expect_violation(
            process_n(&mut server, &mut core, &verifier as &dyn Verifier, 1),
            "verifier unavailable",
        );
        let err = expect_error(&mut client, WireError::AuthFailed);
        assert_eq!(err.message, AUTH_REFUSED_PHRASE);
    }
}
