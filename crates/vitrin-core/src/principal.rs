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
//! `vitrin_realm.request_grant` is served here: the wire-facing half of the
//! grant request flow, split from the policy half in [`petitions`] along
//! the connection boundary. This module owns everything connection-scoped
//! -- decode, the non-zero-verb rule (fatal `invalid_argument`), the
//! petition-rate ceiling and live-petition cap (fatal
//! `resource_exhausted`, denial-of-service confinement per the
//! conventions), the multi-`new_id` rule (five ids, distinct, strictly
//! increasing, above the watermark -- enforced by allocating them in
//! argument order through the same [`allocate_id`] every mint uses), the
//! per-connection object table, and event emission. [`petitions`] owns
//! everything policy-scoped (realm existence, reserved flags, durable
//! rungs, resource granularity, admission caps, the consent decision) --
//! so the petition-policy razor has one home and the object-graph razor
//! has another.
//!
//! **One emission path, exactly-once `resolved`.** Every resolution --
//! immediate (admission refusals, auto-approve) or deferred (scripted
//! consent, timeout) -- is delivered by
//! [`deliver_resolution`](PrincipalServer::deliver_resolution): consent
//! `state` transitions first, then the `resolved` terminal, and the grant
//! handle's pending-to-resolved flip is checked there, making a double
//! `resolved` structurally impossible on top of the registry's
//! consume-on-resolve. Delivery is **phase-gated**: after the fatal
//! goodbye (the connection's terminal event -- conventions 5.2, error
//! then close) or after [`teardown`](PrincipalServer::teardown), a routed
//! resolution is refused typed ([`DeliveryError::ConnectionDead`]) and
//! dropped whole -- nothing is ever sent on a dead connection. And a
//! granted verdict **mints its grant-table row here, at delivery** -- not
//! when the consent decision was made -- in the same single-threaded step
//! that flips the handle, so authority exists if and only if its wire
//! handle resolved: the teardown scan is complete by construction, and an
//! approval whose petitioner disconnected while the decision was in
//! flight evaporates without ever creating a row ("all of the principal's
//! grants die with the connection" has no window; see
//! [`petitions`]' module docs for the full decision). The
//! petition-lifecycle events are exempt from the sync barrier by
//! construction: a pending petition emits only `queued` during dispatch,
//! so a later `done` never waits on consent.
//!
//! **Facets are minted inert; their use goes through THE chokepoint
//! (P1.4.4, issue #28).** The five co-minted objects enter the object
//! table with their roles (the facets carrying their co-minted grant's
//! wire id -- the chokepoint's key). Every facet request -- `capture_frame`,
//! `move`, `button`, `scroll`, `type` -- is decoded here (grammar and
//! argument validation stay connection-scoped: `type`'s forbidden-control-
//! character rule is fatal `invalid_argument`, like the zero-verb rule),
//! then handed as one [`UseKind`] to the **single enforcement function**,
//! [`Chokepoint::enforce_use`], which owns the whole `connection ->
//! principal -> grant -> verbs -> constraints` decision, every
//! `vitrin_grant.refused`, and the admitted operation (frame delivery /
//! origin-tagged actuation intake). This module never answers an authority
//! question itself -- no second enforcement voice exists, and
//! [`enforcement`]'s single-path test greps this file to prove it.
//! Requests on the grant and consent objects are fatal `invalid_opcode`
//! (their interfaces define no requests in version 1 -- grammar, not
//! authority).
//!
//! **Teardown contract.** The embedder MUST call
//! [`teardown`](PrincipalServer::teardown) when the connection closes: it
//! withdraws the connection's pending petitions (consent is in-context --
//! the prompt disappears with the petitioner, no events) and removes the
//! connection's granted rows from the [`GrantTable`] (version-1 grants die
//! with their connection; removal, not revocation -- see the grant table's
//! module docs, whose contract this fulfills). Its scan over resolved
//! grant handles is complete because rows are minted only at delivery
//! (above): no row of this connection can hide behind a still-pending
//! handle, and a consent decision still in flight at teardown mints
//! nothing.
//!
//! # Scope seams (marked, not smuggled)
//!
//! - The **unauthenticated deadline** (conventions 7.1 SHOULD) is a wall
//!   clock owned by the runtime wiring: nothing at runtime accepts
//!   principal connections yet (the listener wiring lands with M1.1
//!   integration), and the deadline is a calloop timer armed at accept and
//!   disarmed on [`is_bound`](PrincipalServer::is_bound) -- flagged in the
//!   task summary rather than half-built here.
//! - The **consent-timeout timer** is likewise M1.1 wiring: the embedder
//!   polls [`PetitionRegistry::expire_due`] (petitions' module docs) and
//!   routes each returned resolution to its connection's
//!   [`deliver_resolution`](PrincipalServer::deliver_resolution).
//! - The flight recorder (P1.4.5) will observe handshakes and petitions
//!   through the same embedder that logs [`PrincipalFault`]s today.
//!
//! [`identity`]: crate::identity
//! [`petitions`]: crate::petitions
//! [`enforcement`]: crate::enforcement
//! [`Verifier`]: crate::identity::Verifier
//! [`Verifier::verify`]: crate::identity::Verifier::verify
//! [`VerifyOutcome`]: crate::identity::VerifyOutcome
//! [`allocate_id`]: PrincipalServer::allocate_id
//! [`GrantTable`]: crate::grants::GrantTable
//! [`PetitionRegistry::expire_due`]: crate::petitions::PetitionRegistry::expire_due
//! [`UseKind`]: crate::enforcement::UseKind
//! [`Chokepoint::enforce_use`]: crate::enforcement::Chokepoint::enforce_use

use std::collections::BTreeMap;
use std::fmt;
use std::os::fd::BorrowedFd;
use std::time::{Duration, Instant};

use vitrin_ipc::{Message, PeerCred, TransportError};
use vitrin_protocol::error::DecodeError;
use vitrin_protocol::generated::vitrin_actuator_pointer as pointer;
use vitrin_protocol::generated::vitrin_actuator_text as text;
use vitrin_protocol::generated::vitrin_consent as consent;
use vitrin_protocol::generated::vitrin_consent::ConsentState;
use vitrin_protocol::generated::vitrin_grant as grant;
use vitrin_protocol::generated::vitrin_grant::{Outcome, Persistence as WirePersistence, Verb};
use vitrin_protocol::generated::vitrin_handshake as handshake;
use vitrin_protocol::generated::vitrin_handshake::Error as WireError;
use vitrin_protocol::generated::vitrin_principal as principal;
use vitrin_protocol::generated::vitrin_realm as realm;
use vitrin_protocol::generated::vitrin_view as view;
use vitrin_protocol::generated::PROTOCOL_VERSION;

use crate::capture::RealmViewFrame;
use crate::enforcement::{Chokepoint, UseEnv, UseKind, UseRequest};
use crate::grants::{GrantId, GrantTable, InsertError};
use crate::identity::{PresentedCredential, PrincipalIdentity, Verifier, VerifyOutcome};
use crate::input::{PhysicalPresence, SeatInput, SeatInputKind};
use crate::petitions::{
    Admission, ConnectionId, PetitionRegistry, PetitionRequest, Resolution, Verdict,
};

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

/// Cap on petitions per connection -- the live-object cap's petition half:
/// every `request_grant` permanently allocates five object ids (no
/// destructors in version 1), so unbounded petitioning is unbounded server
/// state. 256 petitions (1280 objects) is generous for a legitimate
/// long-lived agent that re-petitions after expiries and denials, and
/// tiny as a denial-of-service bound; breach is fatal
/// `resource_exhausted`, confining the DoS to the offending connection.
pub(crate) const MAX_LIVE_PETITIONS: usize = 256;

/// Burst capacity of the per-connection petition-rate token bucket (the
/// conventions' "server-side petition-rate ceiling"; breach is fatal
/// `resource_exhausted`). A compliant client sends a handful of petitions
/// at startup; 8 in one burst is plenty, and the bucket refills at
/// [`PETITION_REFILL_PER_SEC`].
pub(crate) const PETITION_RATE_BURST: u32 = 8;

/// Sustained petition-rate refill, tokens per second. One petition per
/// second sustained keeps even a busy-refused retry loop wire-legal while
/// bounding how fast a hostile connection can burn ids and prompt slots.
pub(crate) const PETITION_REFILL_PER_SEC: u32 = 1;

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
    /// The frame did not decode as the selected message; maps onto the
    /// conventions' fatal code via [`DecodeError::to_wire_error`]. Carries
    /// the id the frame targeted so the goodbye cites the object where the
    /// error occurred (the IDL's `error.object_id` "may be 1" -- it is not
    /// always 1: a malformed `get_realm` cites the principal, not the
    /// handshake object).
    Malformed { object_id: u32, source: DecodeError },
    /// `invalid_argument`: a petition with an empty verb set (the IDL:
    /// "verbs ... MUST be non-zero (an empty petition is fatal
    /// invalid_argument)") -- argument validation the generated decoder
    /// cannot see, since `Verb(0)` is a legal wire bitmask elsewhere.
    ZeroVerbs { object_id: u32 },
    /// `invalid_argument`: `type` text containing a C0 or C1 control
    /// character other than newline or tab (IDL `vitrin_actuator_text`:
    /// "a correct client never emits them") -- argument validation the
    /// generated decoder cannot see, like [`PrincipalViolation::ZeroVerbs`].
    ForbiddenControl { object_id: u32, codepoint: u32 },
    /// `resource_exhausted`: a documented per-connection bound was
    /// breached ([`MAX_LIVE_REALMS`], object-id exhaustion).
    ResourceExhausted(&'static str),
    /// `internal`: a server-side failure poisoned this request (a
    /// can't-happen condition surfacing typed rather than as a panic --
    /// the TCB does not panic on reachable input).
    ServerError { object_id: u32, detail: String },
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
            PrincipalViolation::Malformed { source, .. } => source.to_wire_error(),
            PrincipalViolation::ZeroVerbs { .. } | PrincipalViolation::ForbiddenControl { .. } => {
                WireError::InvalidArgument
            }
            PrincipalViolation::ResourceExhausted(_) => WireError::ResourceExhausted,
            PrincipalViolation::ServerError { .. } | PrincipalViolation::ConnectionDead => {
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
            | PrincipalViolation::Malformed { object_id, .. }
            | PrincipalViolation::ZeroVerbs { object_id }
            | PrincipalViolation::ForbiddenControl { object_id, .. }
            | PrincipalViolation::ServerError { object_id, .. } => *object_id,
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
            PrincipalViolation::InvalidObject { detail, .. } => (*detail).into(),
            PrincipalViolation::Malformed { source, .. } => source.to_string(),
            PrincipalViolation::ZeroVerbs { .. } => "petition verb set is empty".into(),
            PrincipalViolation::ForbiddenControl { codepoint, .. } => {
                format!("text contains forbidden control character U+{codepoint:04X}")
            }
            PrincipalViolation::ResourceExhausted(detail) => (*detail).into(),
            PrincipalViolation::ServerError { .. } => "server-side failure".into(),
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
            PrincipalViolation::Malformed { object_id, source } => {
                write!(f, "malformed message on object {object_id}: {source}")
            }
            PrincipalViolation::ZeroVerbs { object_id } => {
                write!(
                    f,
                    "invalid_argument: empty petition verb set on object {object_id}"
                )
            }
            PrincipalViolation::ForbiddenControl {
                object_id,
                codepoint,
            } => {
                write!(
                    f,
                    "invalid_argument: forbidden control character U+{codepoint:04X} \
                     in text on object {object_id}"
                )
            }
            PrincipalViolation::ResourceExhausted(detail) => {
                write!(f, "resource_exhausted: {detail}")
            }
            PrincipalViolation::ServerError { object_id, detail } => {
                write!(f, "internal: {detail} (object {object_id})")
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

/// Why [`PrincipalServer::deliver_resolution`] refused or failed to
/// deliver. `WrongConnection` and `UnknownGrantObject` are core-side
/// routing bugs and `AlreadyResolved` a replayed resolution -- nothing is
/// sent, the client connection stays intact, and the embedder logs the
/// bug (killing an innocent connection over a misrouted resolution would
/// punish the wrong party). `ConnectionDead` is an expected race, not a
/// bug: the embedder drops the resolution. `Transport` and `Insert` are
/// terminal: the embedder closes and tears down.
#[derive(Debug)]
pub(crate) enum DeliveryError {
    /// The resolution belongs to a different connection (embedder routing
    /// bug); nothing was sent. Checked before the phase gate so a
    /// misrouted resolution is never silently dropped as `ConnectionDead`
    /// -- its rightful (live) connection still owes its client the
    /// terminal.
    WrongConnection {
        expected: ConnectionId,
        got: ConnectionId,
    },
    /// The connection is no longer in its bound steady state: a fatal
    /// already killed it (the `error` goodbye is the connection's terminal
    /// event -- nothing may follow it) or teardown already ran. Nothing is
    /// sent, and dropping the resolution is safe by construction: granted
    /// rows are minted at delivery, so an undeliverable approval leaves no
    /// authority behind. An expected race (the consent decider resolves
    /// while the connection dies), not an embedder bug.
    ConnectionDead,
    /// No grant handle with this wire id exists on this connection
    /// (routing bug); nothing was sent.
    UnknownGrantObject { wire_id: u32 },
    /// The grant handle already resolved -- the exactly-once guard
    /// (a replayed resolution is refused before any row is minted or
    /// anything is sent).
    AlreadyResolved { wire_id: u32 },
    /// The delivery-time row insert failed -- unreachable (every decision
    /// is narrowing-validated before it becomes an approval; defense in
    /// depth). The grant handle would otherwise stay pending forever (its
    /// registry entry is already consumed), hanging the client, so the
    /// connection has been killed: fatal `internal` goodbye sent
    /// best-effort, phase dead. The embedder closes and tears down.
    Insert(InsertError),
    /// Sending an event failed; the connection is dying.
    Transport(TransportError),
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeliveryError::WrongConnection { expected, got } => {
                write!(f, "resolution for {got} routed to {expected}")
            }
            DeliveryError::ConnectionDead => {
                write!(f, "connection dead or torn down; resolution dropped")
            }
            DeliveryError::UnknownGrantObject { wire_id } => {
                write!(f, "no grant handle with wire id {wire_id}")
            }
            DeliveryError::AlreadyResolved { wire_id } => {
                write!(f, "grant handle {wire_id} already resolved (exactly-once)")
            }
            DeliveryError::Insert(e) => {
                write!(f, "delivery-time grant insert failed: {e}")
            }
            DeliveryError::Transport(e) => write!(f, "transport: {e}"),
        }
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

/// What a client-minted object id names on this connection: the dispatch
/// table's row, one entry per allocated id (the principal object and the
/// bootstrap handshake object are held separately, as before). Version 1
/// has no destructors, so entries are permanent -- an id's kind never
/// changes, only a grant's resolution state does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ObjectKind {
    /// A realm address handle (`get_realm`), remembering the requested
    /// name -- naming is not authority; the name is judged at petition
    /// time.
    Realm { name: String },
    /// A grant handle co-minted by `request_grant`.
    Grant(GrantHandleState),
    /// A consent observer co-minted by `request_grant` (events only).
    Consent,
    /// The observation facet, inert until its grant confers `observe`;
    /// `grant` is the co-minted grant handle's wire id (the chokepoint's
    /// key, P1.4.4).
    View { grant: u32 },
    /// The pointer facet (see [`ObjectKind::View`]).
    Pointer { grant: u32 },
    /// The text facet (see [`ObjectKind::View`]).
    Text { grant: u32 },
}

/// A grant handle's lifecycle on the wire: born pending, flipped exactly
/// once by [`PrincipalServer::deliver_resolution`] -- the server-side half
/// of the IDL's "resolved fires exactly once ever".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantHandleState {
    /// Awaiting its `resolved` terminal.
    Pending,
    /// Resolved; `row` is the grant-table row iff the outcome was
    /// `granted` (what the P1.4.4 chokepoint queries, and what teardown
    /// removes).
    Resolved { row: Option<GrantId> },
}

/// Everything the embedder provides for one dispatch turn: the shared
/// capability-kernel state, the injected clock, and the realm-facing
/// environment the enforcement chokepoint consults ([`crate::enforcement`]).
/// Built fresh per dispatched message -- `realm_view` borrows the
/// compositor's retained framebuffer for exactly this turn.
pub(crate) struct ServerCtx<'a> {
    /// The one verifier serving every connection ([`hello`]'s gate).
    ///
    /// [`hello`]: PrincipalServer::handle_hello
    pub verifier: &'a dyn Verifier,
    /// The core-global pending-petition registry (also the chokepoint's
    /// `consent_held` source).
    pub petitions: &'a mut PetitionRegistry,
    /// The core-global grant table.
    pub grants: &'a mut GrantTable,
    /// The dispatch turn's injected instant: the server never reads a
    /// clock, so one consistent `now` governs each request's whole
    /// decision -- handshake, petition, and enforcement alike.
    pub now: Instant,
    /// The realm's latest completed view; `None` while the realm has no
    /// surface (the chokepoint's `no_surface` judgement and the capture
    /// source).
    pub realm_view: Option<RealmViewFrame<'a>>,
    /// Physical-input presence, fed at the input router's hook point (the
    /// chokepoint's `preempted` judgement).
    pub presence: &'a PhysicalPresence,
    /// Where chokepoint-admitted, origin-tagged actuations go (M1.1: the
    /// realm's input router toward the shim seat).
    pub actuations: &'a mut dyn FnMut(SeatInput),
}

/// The per-connection principal protocol server. One instance per accepted
/// principal connection; single-threaded, driven by decoded [`Message`]s
/// from the connection's event source. The embedder passes the same
/// [`Verifier`] for the connection's whole lifetime (one verifier serves
/// every connection) and, on `Err`, logs the fault and closes the
/// connection without dispatching further frames.
pub(crate) struct PrincipalServer {
    phase: Phase,
    /// `SO_PEERCRED` recorded by the transport at accept -- the third leg
    /// of the sender-constraint triple, captured at construction and handed
    /// to the verifier on `hello`.
    peer: PeerCred,
    /// This connection's core-global id
    /// ([`PetitionRegistry::register_connection`]): how deferred
    /// resolutions route back, and what teardown withdraws by.
    connection: ConnectionId,
    /// Highest object id allocated on this connection (starts at the
    /// bootstrap id); every `new_id` must exceed it -- strictly increasing,
    /// never reused (conventions 3.1).
    watermark: u32,
    /// The principal object minted by `hello`, live after `bound`.
    principal_id: Option<u32>,
    /// The verifier-canonical bound identity; what P1.4.2 keys grants by.
    identity: Option<PrincipalIdentity>,
    /// The per-connection object table: id -> kind, for every id a request
    /// has minted. `BTreeMap` so iteration (and hence log output) is
    /// deterministic. This map living inside the per-connection server --
    /// and nowhere else -- is what makes handles sender-constrained
    /// (module docs).
    objects: BTreeMap<u32, ObjectKind>,
    /// Live realm handles, against [`MAX_LIVE_REALMS`].
    realm_count: usize,
    /// Petitions ever minted on this connection, against
    /// [`MAX_LIVE_PETITIONS`] (never decremented: version 1 has no
    /// destructors, so every petition's five objects are permanent).
    petition_count: usize,
    /// Petition-rate token bucket: remaining burst tokens.
    petition_tokens: u32,
    /// The bucket's refill anchor (the instant up to which refill has been
    /// credited); `None` until the first petition arms it.
    petition_refill_anchor: Option<Instant>,
    /// This connection's enforcement chokepoint (P1.4.4): the single
    /// authority-check function for every facet use, plus its per-grant
    /// bucket/coalescing state -- which thereby dies with the connection,
    /// exactly as version-1 grants do.
    chokepoint: Chokepoint,
}

impl PrincipalServer {
    /// A fresh server for one accepted connection, with the `SO_PEERCRED`
    /// the transport captured at accept ([`Connection::peer_cred`]) and
    /// the connection id the embedder minted for it
    /// ([`PetitionRegistry::register_connection`]).
    ///
    /// [`Connection::peer_cred`]: vitrin_ipc::Connection::peer_cred
    pub fn new(peer: PeerCred, connection: ConnectionId) -> Self {
        Self {
            phase: Phase::Connected,
            peer,
            connection,
            watermark: HANDSHAKE_ID,
            principal_id: None,
            identity: None,
            objects: BTreeMap::new(),
            realm_count: 0,
            petition_count: 0,
            petition_tokens: PETITION_RATE_BURST,
            petition_refill_anchor: None,
            chokepoint: Chokepoint::new(),
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

    /// Dispatch one decoded frame from the principal connection, against
    /// the embedder-provided [`ServerCtx`]: the core-shared
    /// capability-kernel state, the injected instant (the server never
    /// reads a clock, so one consistent `now` governs each request's
    /// whole decision -- the grant table's injected-clock discipline,
    /// upheld here), and the realm environment the enforcement chokepoint
    /// consults. `send` puts one encoded event frame on the wire, with the
    /// optional fd that rides `SCM_RIGHTS` beside it (`frame_ready`'s
    /// memfd is the only version-1 event fd).
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
        ctx: &mut ServerCtx<'_>,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        let result = self.dispatch(msg, ctx, send);
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
                        let _ = send(&goodbye.encode(HANDSHAKE_ID), None);
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
        ctx: &mut ServerCtx<'_>,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        let object_id = msg.header.object_id;
        let opcode = msg.header.opcode;
        match self.phase {
            Phase::Dead => Err(PrincipalViolation::ConnectionDead.into()),
            Phase::Connected => {
                if object_id == HANDSHAKE_ID && opcode == handshake::requests::Hello::OPCODE {
                    self.handle_hello(msg, ctx.verifier, send)
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
                            // Pending petitions do NOT hold it back --
                            // their lifecycle events are exempt from the
                            // barrier (IDL: done confirms registration,
                            // never resolution).
                            let done = handshake::events::Done {
                                cookie: sync.cookie,
                            };
                            send(&done.encode(HANDSHAKE_ID), None)?;
                            Ok(())
                        }
                        _ => Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into()),
                    }
                } else if Some(object_id) == self.principal_id {
                    match opcode {
                        principal::requests::GetRealm::OPCODE => self.handle_get_realm(msg),
                        _ => Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into()),
                    }
                } else if let Some(kind) = self.objects.get(&object_id).cloned() {
                    match kind {
                        ObjectKind::Realm { name } => match opcode {
                            realm::requests::RequestGrant::OPCODE => {
                                self.handle_request_grant(msg, name, ctx, send)
                            }
                            _ => {
                                Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                            }
                        },
                        // vitrin_grant and vitrin_consent define no
                        // requests in version 1: any opcode on them is
                        // grammar (invalid_opcode), never an authority
                        // judgement.
                        ObjectKind::Grant(_) | ObjectKind::Consent => {
                            Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                        }
                        // Facet use (module docs): decode is grammar and
                        // stays here; the authority question -- all of it
                        // -- is the enforcement chokepoint's, reached
                        // through the single serve_facet_use funnel.
                        ObjectKind::View { grant } => match opcode {
                            view::requests::CaptureFrame::OPCODE => {
                                let (_, _req) =
                                    view::requests::CaptureFrame::decode(&msg.bytes, msg.fd)
                                        .map_err(|source| PrincipalViolation::Malformed {
                                            object_id,
                                            source,
                                        })?;
                                self.serve_facet_use(object_id, grant, UseKind::Capture, ctx, send)
                            }
                            _ => {
                                Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                            }
                        },
                        ObjectKind::Pointer { grant } => {
                            let kind = match opcode {
                                pointer::requests::Move::OPCODE => {
                                    let (_, req) =
                                        pointer::requests::Move::decode(&msg.bytes, msg.fd)
                                            .map_err(|source| PrincipalViolation::Malformed {
                                                object_id,
                                                source,
                                            })?;
                                    SeatInputKind::Motion {
                                        x: f64::from(req.x),
                                        y: f64::from(req.y),
                                    }
                                }
                                pointer::requests::Button::OPCODE => {
                                    let (_, req) =
                                        pointer::requests::Button::decode(&msg.bytes, msg.fd)
                                            .map_err(|source| PrincipalViolation::Malformed {
                                                object_id,
                                                source,
                                            })?;
                                    SeatInputKind::Button {
                                        button: req.button,
                                        state: req.state,
                                    }
                                }
                                pointer::requests::Scroll::OPCODE => {
                                    let (_, req) =
                                        pointer::requests::Scroll::decode(&msg.bytes, msg.fd)
                                            .map_err(|source| PrincipalViolation::Malformed {
                                                object_id,
                                                source,
                                            })?;
                                    SeatInputKind::Scroll {
                                        axis: req.axis,
                                        value120: req.value120,
                                    }
                                }
                                _ => {
                                    return Err(PrincipalViolation::UnknownOpcode {
                                        object_id,
                                        opcode,
                                    }
                                    .into())
                                }
                            };
                            self.serve_facet_use(
                                object_id,
                                grant,
                                UseKind::Pointer(kind),
                                ctx,
                                send,
                            )
                        }
                        ObjectKind::Text { grant } => match opcode {
                            text::requests::Type::OPCODE => {
                                let (_, req) = text::requests::Type::decode(&msg.bytes, msg.fd)
                                    .map_err(|source| PrincipalViolation::Malformed {
                                        object_id,
                                        source,
                                    })?;
                                // The IDL's normative control-character
                                // rule: newline and tab are the two legal
                                // control characters; any other C0
                                // (U+0000..=U+001F) or C1 (U+0080..=U+009F)
                                // control is fatal invalid_argument --
                                // argument validation the generated
                                // decoder cannot see, exactly like the
                                // zero-verb rule. (DEL, U+007F, is in
                                // neither set and deliberately passes: the
                                // server must not be stricter than the
                                // wire contract it serves.)
                                if let Some(c) = req.text.chars().find(|&c| {
                                    matches!(c, '\u{0000}'..='\u{001f}' | '\u{0080}'..='\u{009f}')
                                        && c != '\n'
                                        && c != '\t'
                                }) {
                                    return Err(PrincipalViolation::ForbiddenControl {
                                        object_id,
                                        codepoint: c as u32,
                                    }
                                    .into());
                                }
                                self.serve_facet_use(
                                    object_id,
                                    grant,
                                    UseKind::Text(SeatInputKind::Text { text: req.text }),
                                    ctx,
                                    send,
                                )
                            }
                            _ => {
                                Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                            }
                        },
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

    /// The single funnel from every facet request arm to **THE
    /// enforcement chokepoint** ([`Chokepoint::enforce_use`], P1.4.4):
    /// resolves the connection-scoped facts the chain's first two steps
    /// need -- the facet was found in *this* connection's object table
    /// (sender constraint, already enforced by dispatch), the bound
    /// identity, and the facet's co-minted grant row -- and delegates the
    /// entire authority decision plus the admitted operation. This
    /// function makes no authority judgement of its own.
    fn serve_facet_use<F>(
        &mut self,
        facet_id: u32,
        grant_wire_id: u32,
        kind: UseKind,
        ctx: &mut ServerCtx<'_>,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        // Bound phase implies a bound identity; surfacing the impossible
        // case typed keeps the TCB panic-free on reachable input
        // (fail-closed: no identity, no use).
        let Some(identity) = self.identity.clone() else {
            return Err(PrincipalViolation::ServerError {
                object_id: facet_id,
                detail: "facet use dispatched with no bound identity".into(),
            }
            .into());
        };
        let request = UseRequest {
            facet_id,
            grant_wire_id,
            grant_row: self.grant_row_id(grant_wire_id),
            principal: &identity,
            kind,
        };
        let env = UseEnv {
            realm_view: ctx.realm_view.as_ref(),
            presence: ctx.presence,
            actuations: &mut *ctx.actuations,
        };
        self.chokepoint
            .enforce_use(request, ctx.grants, ctx.petitions, env, ctx.now, send)
            .map(|_outcome| ()) // the P1.4.5 flight recorder consumes the outcome
            .map_err(PrincipalFault::Transport)
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
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
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
                send(&event.encode(hello.principal), None)?;
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
        if self.realm_count >= MAX_LIVE_REALMS {
            return Err(PrincipalViolation::ResourceExhausted("live-realm cap exceeded").into());
        }
        self.allocate_id(req.realm)?;
        self.objects
            .insert(req.realm, ObjectKind::Realm { name: req.name });
        self.realm_count += 1;
        Ok(())
    }

    /// `vitrin_realm.request_grant`: the petition flow's wire half (module
    /// docs). Order of checks: grammar (decode), the non-zero-verb rule,
    /// the per-connection resource bounds (cap then rate -- bounds precede
    /// any allocation, the `get_realm` precedent), the multi-`new_id`
    /// rule, then admission in [`petitions`](crate::petitions), which
    /// either resolves on the spot (delivered immediately through the one
    /// emission path) or leaves the petition pending with its consent
    /// observer `queued`.
    fn handle_request_grant<F>(
        &mut self,
        msg: Message,
        realm_name: String,
        ctx: &mut ServerCtx<'_>,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        let now = ctx.now;
        let object_id = msg.header.object_id;
        let (_, req) = realm::requests::RequestGrant::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        // An empty petition is fatal invalid_argument (IDL): argument
        // validation the decoder cannot do, since Verb(0) is a legal
        // bitmask value in other positions.
        if req.verbs.bits() == 0 {
            return Err(PrincipalViolation::ZeroVerbs { object_id }.into());
        }
        // Per-connection resource bounds precede allocation: every
        // petition permanently allocates five ids, so the cap and the rate
        // ceiling are checked before any state changes.
        if self.petition_count >= MAX_LIVE_PETITIONS {
            return Err(PrincipalViolation::ResourceExhausted("live-petition cap exceeded").into());
        }
        if !self.take_petition_token(now) {
            return Err(
                PrincipalViolation::ResourceExhausted("petition-rate ceiling exceeded").into(),
            );
        }
        // The multi-new_id rule (conventions 3.2): allocating the five ids
        // in argument order through the single watermark allocator is
        // exactly "distinct, strictly increasing in argument order, and
        // all above the watermark" -- any violation dies invalid_object
        // before the petition exists.
        for id in [req.grant, req.consent, req.view, req.pointer, req.text] {
            self.allocate_id(id)?;
        }
        self.objects
            .insert(req.grant, ObjectKind::Grant(GrantHandleState::Pending));
        self.objects.insert(req.consent, ObjectKind::Consent);
        self.objects
            .insert(req.view, ObjectKind::View { grant: req.grant });
        self.objects
            .insert(req.pointer, ObjectKind::Pointer { grant: req.grant });
        self.objects
            .insert(req.text, ObjectKind::Text { grant: req.grant });
        self.petition_count += 1;

        // Bound phase implies a bound identity; surfacing the impossible
        // case typed keeps the TCB panic-free on reachable input.
        let Some(identity) = self.identity.clone() else {
            return Err(PrincipalViolation::ServerError {
                object_id,
                detail: "petition dispatched with no bound identity".into(),
            }
            .into());
        };
        let admission = ctx.petitions.admit(
            PetitionRequest {
                connection: self.connection,
                identity,
                realm_name,
                grant_wire_id: req.grant,
                consent_wire_id: req.consent,
                resource: req.resource,
                verbs: req.verbs,
                expiry_ms: req.expiry_ms,
                max_event_rate: req.max_event_rate,
                persistence: req.persistence,
                flags: req.flags,
            },
            now,
        );
        match admission {
            Admission::Pending { .. } => {
                // The prompt lifecycle began: it is waiting on the consent
                // surface (or, in this build, the timeout).
                let queued = consent::events::State {
                    state: ConsentState::Queued,
                };
                send(&queued.encode(req.consent), None)?;
                Ok(())
            }
            Admission::Resolved(resolution) => {
                self.deliver_resolution(resolution, ctx.grants, now, send)
                    .map_err(|e| {
                        match e {
                            DeliveryError::Transport(t) => PrincipalFault::Transport(t),
                            // Unreachable for a resolution minted for the
                            // petition just registered (the connection is
                            // bound and the handle pending); typed,
                            // fail-closed. An `Insert` failure already
                            // killed the connection inside delivery, so
                            // this mapping only keeps the embedder
                            // contract (log and close) -- the funnel's
                            // was-dead guard prevents a second goodbye.
                            other => PrincipalViolation::ServerError {
                                object_id,
                                detail: other.to_string(),
                            }
                            .into(),
                        }
                    })
            }
        }
    }

    /// Refill-then-take on the petition-rate token bucket
    /// ([`PETITION_RATE_BURST`] / [`PETITION_REFILL_PER_SEC`]), integer
    /// arithmetic on the injected `now`. Returns whether a token was
    /// available; a `false` is the fatal `resource_exhausted` ceiling.
    fn take_petition_token(&mut self, now: Instant) -> bool {
        let anchor = *self.petition_refill_anchor.get_or_insert(now);
        let elapsed_secs = now.saturating_duration_since(anchor).as_secs();
        let refill = elapsed_secs.saturating_mul(u64::from(PETITION_REFILL_PER_SEC));
        if refill > 0 {
            let tokens = (u64::from(self.petition_tokens) + refill)
                .min(u64::from(PETITION_RATE_BURST)) as u32;
            self.petition_tokens = tokens;
            self.petition_refill_anchor = Some(if tokens == PETITION_RATE_BURST {
                // Full bucket: excess credit is dropped, the clock
                // restarts here.
                now
            } else {
                // Credit whole seconds only; the fractional remainder
                // stays banked in the anchor.
                anchor + Duration::from_secs(elapsed_secs)
            });
        }
        if self.petition_tokens == 0 {
            return false;
        }
        self.petition_tokens -= 1;
        true
    }

    /// Deliver one petition resolution on this connection: the **single
    /// emission path** for the petition terminal (module docs), shared by
    /// immediate resolutions (admission refusals, auto-approve) and
    /// deferred ones the embedder routes here (scripted consent, timeout).
    ///
    /// Delivery is phase-gated: on a connection no longer bound -- the
    /// fatal goodbye is the connection's terminal event (conventions 5.2),
    /// and [`teardown`](Self::teardown) ends the conversation just as
    /// finally -- the resolution is refused typed and dropped whole,
    /// nothing sent. For a granted verdict **the grant-table row is minted
    /// here**, at the injected `now`, in the same single-threaded step
    /// that flips the wire handle: authority exists if and only if its
    /// handle resolved, so the teardown scan is complete and an
    /// undelivered approval leaves no row behind (module docs). Emits the
    /// closing consent transition (when a prompt lifecycle existed), then
    /// exactly one `vitrin_grant.resolved`; the handle's
    /// pending-to-resolved flip is the exactly-once guard -- a replayed
    /// resolution is refused typed before any row is minted or anything is
    /// sent.
    pub fn deliver_resolution<F>(
        &mut self,
        resolution: Resolution,
        grants: &mut GrantTable,
        now: Instant,
        send: &mut F,
    ) -> Result<(), DeliveryError>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        // Addressing before the phase gate: a resolution for another
        // connection must surface as the routing bug it is, not vanish as
        // this connection's dead-drop.
        if resolution.connection != self.connection {
            return Err(DeliveryError::WrongConnection {
                expected: self.connection,
                got: resolution.connection,
            });
        }
        // The phase gate (module docs): after the fatal goodbye or after
        // teardown nothing more is ever sent -- and no authority is ever
        // minted -- on this connection.
        if self.phase != Phase::Bound {
            return Err(DeliveryError::ConnectionDead);
        }
        let Some(ObjectKind::Grant(state)) = self.objects.get_mut(&resolution.grant_wire_id) else {
            return Err(DeliveryError::UnknownGrantObject {
                wire_id: resolution.grant_wire_id,
            });
        };
        // The exactly-once guard runs before any state changes: a replayed
        // granted resolution must not mint a second row.
        if matches!(state, GrantHandleState::Resolved { .. }) {
            return Err(DeliveryError::AlreadyResolved {
                wire_id: resolution.grant_wire_id,
            });
        }
        // Authority is born here, not at decision time: the row insert and
        // the handle flip are one delivery step, so no teardown
        // interleaving can observe a row without a resolved handle.
        let row = match &resolution.verdict {
            Verdict::Granted { grant: approved } => match approved.insert_row(grants, now) {
                Ok(id) => Some(id),
                Err(e) => {
                    // Unreachable (decisions are validated before they
                    // become approvals), surfaced as this entry point's
                    // own fatal funnel: the handle can never resolve now
                    // (its registry entry is consumed), and a client hung
                    // on a permanently pending grant is what denial must
                    // never be -- so the connection dies fatal internal,
                    // best-effort goodbye first, DEAD so nothing else
                    // dispatches.
                    tracing::warn!(
                        peer_uid = self.peer.uid,
                        error = %e,
                        "delivery-time grant insert failed; principal connection fatal"
                    );
                    self.phase = Phase::Dead;
                    let goodbye = handshake::events::Error {
                        object_id: resolution.grant_wire_id,
                        code: WireError::Internal,
                        message: "server-side failure".into(),
                    };
                    let _ = send(&goodbye.encode(HANDSHAKE_ID), None);
                    return Err(DeliveryError::Insert(e));
                }
            },
            Verdict::Declined { .. } => None,
        };
        *state = GrantHandleState::Resolved { row };
        if resolution.emit_closed {
            let closed = consent::events::State {
                state: ConsentState::Closed,
            };
            send(&closed.encode(resolution.consent_wire_id), None)
                .map_err(DeliveryError::Transport)?;
        }
        // The terminal: effective authority on granted, zeroed trailing
        // arguments otherwise (IDL).
        let resolved = match resolution.verdict {
            Verdict::Granted { grant: approved } => grant::events::Resolved {
                outcome: Outcome::Granted,
                verbs: approved.effective.verbs,
                persistence: WirePersistence::from(approved.effective.persistence),
                expiry_ms: approved.effective.expiry_ms,
            },
            Verdict::Declined { outcome } => grant::events::Resolved {
                outcome,
                verbs: Verb::default(),
                persistence: WirePersistence::Once,
                expiry_ms: 0,
            },
        };
        send(&resolved.encode(resolution.grant_wire_id), None).map_err(DeliveryError::Transport)?;
        Ok(())
    }

    /// Connection teardown (module docs: the embedder MUST call this when
    /// the connection closes, for any reason): withdraws this connection's
    /// pending petitions -- no events, the petitioner is gone -- and
    /// removes its granted rows from the grant table (version-1 grants die
    /// with their connection; removal, not revocation -- the grant table's
    /// documented teardown contract). The resolved-handle scan is complete
    /// by construction: rows are minted only at delivery, in the same step
    /// that flips the handle, so no row of this connection can exist
    /// behind a still-pending handle -- and a consent decision still in
    /// flight when the connection dies mints nothing, because its late
    /// delivery is phase-refused. Idempotent, and leaves the server DEAD
    /// so a buggy embedder cannot dispatch (or deliver) into a torn-down
    /// connection.
    pub fn teardown(&mut self, petitions: &mut PetitionRegistry, grants: &mut GrantTable) {
        let withdrawn = petitions.withdraw_connection(self.connection);
        let mut removed = 0usize;
        for kind in self.objects.values() {
            if let ObjectKind::Grant(GrantHandleState::Resolved { row: Some(id) }) = kind {
                if grants.remove(*id) {
                    removed += 1;
                }
            }
        }
        if withdrawn > 0 || removed > 0 {
            tracing::info!(
                connection = %self.connection,
                withdrawn_petitions = withdrawn,
                removed_grants = removed,
                "principal connection teardown"
            );
        }
        self.phase = Phase::Dead;
    }

    /// The grant-table row behind a wire grant handle, if that handle
    /// resolved `granted`: the P1.4.4 chokepoint's wire-to-row key, and
    /// the tests' bridge from wire ids to table state.
    pub fn grant_row_id(&self, grant_wire_id: u32) -> Option<GrantId> {
        match self.objects.get(&grant_wire_id) {
            Some(ObjectKind::Grant(GrantHandleState::Resolved { row })) => *row,
            _ => None,
        }
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
    use vitrin_protocol::generated::vitrin_grant::Refusal;
    use vitrin_protocol::generated::vitrin_shim_seat::Origin;

    use crate::grants::GrantState;
    use crate::grants::Issuer;
    use crate::grants::PersistenceRung;
    use crate::identity::{
        BoundPrincipal, RejectionCause, StaticPrincipal, StaticVerifier, STATIC_TOKEN_SCHEME,
    };
    use crate::input::PHYSICAL_HOLD_WINDOW;
    use crate::petitions::{ConsentPolicy, PetitionConfig, ScriptedDecision, ScriptedError};

    use super::*;

    const TOKEN: &str = "9b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a7e5f30619b2f4c1d8a";
    const DEMO_IDENTITY: &str = "vitrin://local/agent/demo";

    /// The full version-1 verb set, as the demo agent petitions for it.
    fn all_verbs() -> Verb {
        Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT
    }

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

    /// The rig's default realm-view size (the SDK-golden pattern's
    /// dimensions, so capture assertions can reuse its bytes).
    const VIEW_W: u32 = 64;
    const VIEW_H: u32 = 40;

    /// The core-shared capability-kernel state one test rig hosts (the
    /// registry and grant table every connection of the rig shares), plus
    /// the injected clock all dispatch runs at -- tests advance `now` as
    /// values, never sleep -- and the enforcement environment: the realm's
    /// retained view (default: the test pattern; `None` = vacant realm),
    /// physical-input presence, and the sink collecting chokepoint-admitted
    /// actuations.
    struct Shared {
        petitions: PetitionRegistry,
        grants: GrantTable,
        now: Instant,
        /// `(rgba, width, height)`; `None` models a vacant realm.
        view: Option<(Vec<u8>, u32, u32)>,
        presence: PhysicalPresence,
        actuations: Vec<SeatInput>,
    }

    impl Shared {
        fn new(policy: ConsentPolicy) -> Self {
            Self {
                petitions: PetitionRegistry::new(policy, PetitionConfig::default()),
                grants: GrantTable::new(),
                now: Instant::now(),
                view: Some((crate::test_pattern::render(VIEW_W, VIEW_H), VIEW_W, VIEW_H)),
                presence: PhysicalPresence::new(),
                actuations: Vec::new(),
            }
        }
    }

    /// A fresh per-connection server + socketpair on `shared`: core end,
    /// client end.
    fn connect(shared: &mut Shared) -> (PrincipalServer, Connection, Connection) {
        let (core, client) = Connection::pair().expect("socketpair");
        let connection = shared.petitions.register_connection();
        (
            PrincipalServer::new(core.peer_cred(), connection),
            core,
            client,
        )
    }

    /// One-rig setup for the single-connection tests, on the fail-closed
    /// default policy (interactive); petition tests that need another
    /// policy build their own [`Shared`].
    fn setup() -> (PrincipalServer, Connection, Connection, Shared) {
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        let (server, core, client) = connect(&mut shared);
        (server, core, client, shared)
    }

    /// Receive and dispatch exactly `n` client messages on the core side,
    /// at the rig's injected `now`, against the rig's enforcement
    /// environment (view, presence, actuation sink).
    fn process_n(
        server: &mut PrincipalServer,
        core: &mut Connection,
        verifier: &dyn Verifier,
        shared: &mut Shared,
        n: usize,
    ) -> Result<(), PrincipalFault> {
        let Shared {
            petitions,
            grants,
            now,
            view,
            presence,
            actuations,
        } = shared;
        let mut sink = |input: SeatInput| actuations.push(input);
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            let mut ctx = ServerCtx {
                verifier,
                petitions,
                grants,
                now: *now,
                realm_view: view.as_ref().map(|(rgba, width, height)| RealmViewFrame {
                    rgba,
                    width: *width,
                    height: *height,
                }),
                presence,
                actuations: &mut sink,
            };
            server.handle_message(msg, &mut ctx, &mut |frame, fd| core.send_message(frame, fd))?;
        }
        Ok(())
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

    /// A schema-legal whole-realm petition minting ids `base..base+5` in
    /// argument order (grant, consent, view, pointer, text): all three
    /// verbs, `while_running`, wire defaults. Tests tweak fields.
    fn petition_at(base: u32) -> realm::requests::RequestGrant {
        realm::requests::RequestGrant {
            grant: base,
            consent: base + 1,
            view: base + 2,
            pointer: base + 3,
            text: base + 4,
            resource: String::new(),
            verbs: all_verbs(),
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: WirePersistence::WhileRunning,
            flags: 0,
        }
    }

    /// The standard petition: ids 4..=8, right after the standard preamble
    /// (principal 2, realm 3).
    fn petition_frame() -> realm::requests::RequestGrant {
        petition_at(4)
    }

    /// Complete a successful handshake with principal id 2 and return the
    /// decoded bound identity.
    fn bind(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
        shared: &mut Shared,
    ) -> String {
        send_hello(client, 2, DEMO_IDENTITY, TOKEN);
        process_n(server, core, verifier, shared, 1).expect("handshake");
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, bound) = principal::events::Bound::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, 2, "bound arrives on the pre-allocated principal");
        bound.identity
    }

    /// The standard petition preamble: bind (principal 2) and mint realm
    /// handle 3 for `realm-0`.
    fn bind_with_realm(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
        shared: &mut Shared,
    ) {
        bind(server, core, client, verifier, shared);
        send_get_realm(client, 2, 3, "realm-0");
        process_n(server, core, verifier, shared, 1).expect("get_realm");
    }

    /// Route one resolution to its connection's emission path, against the
    /// rig's shared grant table at the rig's injected clock (rows are
    /// minted at delivery).
    fn deliver(
        server: &mut PrincipalServer,
        core: &mut Connection,
        shared: &mut Shared,
        resolution: Resolution,
    ) {
        server
            .deliver_resolution(
                resolution,
                &mut shared.grants,
                shared.now,
                &mut |frame, fd| core.send_message(frame, fd),
            )
            .expect("deliver resolution");
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

    /// Assert the next client-visible event is a consent `state`
    /// transition on the given observer.
    fn expect_consent_state(client: &mut Connection, consent_id: u32, want: ConsentState) {
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, ev) = consent::events::State::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(
            object_id, consent_id,
            "consent state arrives on the co-minted observer"
        );
        assert_eq!(ev.state, want);
    }

    /// Assert the next client-visible event is `resolved` on the given
    /// grant handle, returning it for outcome assertions.
    fn expect_resolved(client: &mut Connection, grant_id: u32) -> grant::events::Resolved {
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, ev) = grant::events::Resolved::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(
            object_id, grant_id,
            "resolved arrives on the co-minted grant handle"
        );
        ev
    }

    /// Assert the next client-visible event is `vitrin_grant.refused` on
    /// the given grant handle with the given verb and code, returning the
    /// event for `retry_after_ms` assertions.
    fn expect_refused(
        client: &mut Connection,
        grant_id: u32,
        verb: Verb,
        code: Refusal,
    ) -> grant::events::Refused {
        let msg = client.recv_message().unwrap().unwrap();
        assert!(msg.fd.is_none(), "a refusal carries no fd");
        let (object_id, ev) = grant::events::Refused::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, grant_id, "refused arrives on the grant handle");
        assert_eq!(ev.verb, verb, "the refused verb names the facet");
        assert_eq!(ev.code, code);
        if code != Refusal::RateLimited {
            assert_eq!(
                ev.retry_after_ms, 0,
                "retry_after_ms is nonzero only for rate_limited (IDL)"
            );
        }
        ev
    }

    /// Assert the next client-visible event is `frame_ready` on the given
    /// view facet, returning the decoded frame (whose fd closes on drop).
    fn expect_frame(client: &mut Connection, view_id: u32) -> view::events::FrameReady {
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, ev) = view::events::FrameReady::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, view_id, "frame_ready arrives on the view facet");
        assert_eq!(ev.width, VIEW_W);
        assert_eq!(ev.height, VIEW_H);
        assert_eq!(ev.stride, VIEW_W * 4);
        ev
    }

    /// The standard granted-rig setup: auto-approve policy, bind, realm 3,
    /// one petition (ids 4..=8) resolved granted with the given rate and
    /// expiry -- the walking-skeleton preamble of every enforcement test.
    /// Returns the rig after consuming the consent/resolved events.
    fn granted_rig(
        verifier: &dyn Verifier,
        max_event_rate: u32,
        expiry_ms: u32,
    ) -> (PrincipalServer, Connection, Connection, Shared) {
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, verifier, &mut shared);
        let mut req = petition_frame();
        req.max_event_rate = max_event_rate;
        req.expiry_ms = expiry_ms;
        client.send_message(&req.encode(3), None).unwrap();
        process_n(&mut server, &mut core, verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);
        (server, core, client, shared)
    }

    /// Encode one `capture_frame` on the standard view facet (id 6).
    fn capture_frame() -> Vec<u8> {
        view::requests::CaptureFrame {}.encode(6)
    }

    /// Encode one pointer `move` on the standard pointer facet (id 7).
    fn move_to(x: i32, y: i32) -> Vec<u8> {
        pointer::requests::Move { x, y }.encode(7)
    }

    /// Encode one `type` on the standard text facet (id 8).
    fn type_text(text: &str) -> Vec<u8> {
        text::requests::Type { text: text.into() }.encode(8)
    }

    /// Fence: sync, dispatch it, and require `done` as the *very next*
    /// client-visible event -- proving the connection survived (recoverable
    /// outcomes never kill it) and that no unread event was queued ahead.
    fn sync_fence(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
        shared: &mut Shared,
        cookie: u32,
    ) {
        send_sync(client, cookie);
        process_n(server, core, verifier, shared, 1).expect("sync must dispatch");
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, HANDSHAKE_ID);
        assert_eq!(done.cookie, cookie, "done must be the next event in stream");
    }

    // -- acceptance: bind + refusals ---------------------------------------

    #[test]
    fn successful_handshake_sends_bound_with_the_canonical_identity() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        assert!(!server.is_bound());
        let identity = bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        assert_eq!(identity, DEMO_IDENTITY);
        assert!(server.is_bound());
        assert_eq!(server.bound_identity().unwrap().as_str(), DEMO_IDENTITY);
    }

    #[test]
    fn refused_handshake_is_wire_uniform_across_causes() {
        let _fd = crate::capture::tests::fd_lock();
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
            let (mut server, mut core, mut client, mut shared) = setup();
            send_hello(&mut client, 2, identity, credential);
            expect_violation(
                process_n(&mut server, &mut core, verifier, &mut shared, 1),
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
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        send_hello(&mut client, 2, DEMO_IDENTITY, "super-secret-wrong-token");
        let fault = process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap_err();
        let log_line = fault.to_string();
        assert!(log_line.contains("does not match the registered token"));
        assert!(log_line.contains(DEMO_IDENTITY));
        assert!(!log_line.contains("super-secret-wrong-token"));
    }

    // -- state machine edges -----------------------------------------------

    #[test]
    fn version_check_precedes_verification() {
        let _fd = crate::capture::tests::fd_lock();
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

        let (mut server, mut core, mut client, mut shared) = setup();
        send_hello_versioned(&mut client, PROTOCOL_VERSION + 1, 2, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &counting, &mut shared, 1),
            "version_unsupported",
        );
        let err = expect_error(&mut client, WireError::VersionUnsupported);
        assert_eq!(calls.get(), 0, "the verifier must never see the credential");
        // No supported-version hint: downgrade is refusal, not negotiation.
        assert!(!err.message.contains('1'));
    }

    #[test]
    fn traffic_before_hello_is_pre_handshake() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        send_sync(&mut client, 7);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "pre_handshake",
        );
        expect_error(&mut client, WireError::PreHandshake);
    }

    #[test]
    fn malformed_hello_dies_by_grammar_before_the_verifier_runs() {
        let _fd = crate::capture::tests::fd_lock();
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

        let (mut server, mut core, mut client, mut shared) = setup();
        client.send_message(&frame, None).unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &Panicking, &mut shared, 1),
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
        let _fd = crate::capture::tests::fd_lock();
        // The IDL defines error.object_id as the id of the object where the
        // error occurred, "which may be 1" -- not always 1. A malformed
        // get_realm on the bound principal (object 2) must cite object 2,
        // so client-side debugging is not misdirected to the handshake
        // object. Here the name argument is invalid UTF-8.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
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
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
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
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        send_hello(&mut client, 3, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "second hello",
        );
        expect_error(&mut client, WireError::InvalidOpcode);
    }

    #[test]
    fn sync_answers_done_after_bound() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            0xdead_beef,
        );
    }

    #[test]
    fn unknown_opcodes_are_invalid_opcode() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
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
            let result = process_n(&mut server, &mut core, &verifier, &mut shared, 1);
            expect_violation(result, "invalid_opcode");
            expect_error(&mut client, WireError::InvalidOpcode);
            // Each fatal kills the connection; re-handshake on a fresh one.
            (server, core, client, shared) = setup();
            bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        }
    }

    // -- object graph ------------------------------------------------------

    #[test]
    fn get_realm_mints_under_the_watermark_rule() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        send_get_realm(&mut client, 2, 3, "realm-0");
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            server.objects.get(&3),
            Some(&ObjectKind::Realm {
                name: "realm-0".into()
            })
        );

        // Reusing an id at/below the watermark is fatal invalid_object.
        send_get_realm(&mut client, 2, 3, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "invalid_object",
        );
        expect_error(&mut client, WireError::InvalidObject);
    }

    #[test]
    fn hello_new_id_must_respect_the_watermark_rule() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        // Id 1 is the bootstrap object: claiming it as the principal new_id
        // is at/below the watermark, fatal invalid_object -- before any
        // verification happens.
        let (mut server, mut core, mut client, mut shared) = setup();
        send_hello(&mut client, HANDSHAKE_ID, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "invalid_object",
        );
        expect_error(&mut client, WireError::InvalidObject);

        // The reserved server range is equally out.
        let (mut server, mut core, mut client, mut shared) = setup();
        send_hello(&mut client, 0xff00_0000, DEMO_IDENTITY, TOKEN);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "invalid_object",
        );
        expect_error(&mut client, WireError::InvalidObject);
    }

    #[test]
    fn realm_cap_is_resource_exhausted() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        for i in 0..MAX_LIVE_REALMS as u32 {
            send_get_realm(&mut client, 2, 3 + i, "realm-0");
        }
        process_n(
            &mut server,
            &mut core,
            &verifier,
            &mut shared,
            MAX_LIVE_REALMS,
        )
        .unwrap();
        send_get_realm(&mut client, 2, 100, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "resource_exhausted",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn id_space_exhaustion_is_resource_exhausted() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        server.exhaust_id_space_for_test();
        send_get_realm(&mut client, 2, CLIENT_ID_MAX, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "object-id space exhausted",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    // -- acceptance: petition lifecycle (P1.4.3) ---------------------------

    #[test]
    fn auto_approved_petition_resolves_granted_with_a_usable_row() {
        let _fd = crate::capture::tests::fd_lock();
        // The walking-skeleton flow (IDL flow 1): under the loudly-logged
        // auto-approve policy the petition resolves granted immediately --
        // consent `closed`, then the `resolved` terminal carrying the
        // effective authority -- and the grant handle is backed by a live,
        // usable grant-table row.
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);

        let mut req = petition_frame();
        req.expiry_ms = 300_000;
        client.send_message(&req.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();

        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Granted);
        assert_eq!(resolved.verbs, all_verbs(), "effective = requested");
        assert_eq!(resolved.persistence, WirePersistence::WhileRunning);
        assert_eq!(resolved.expiry_ms, 300_000);

        // The handle is usable end to end: a capture and both actuations
        // through the real facet arms -- the enforcement path itself, not
        // a table-level bridge -- succeed under the granted authority.
        let row = server.grant_row_id(4).expect("wire handle maps to a row");
        assert_eq!(
            shared.grants.get(row, shared.now).unwrap().1,
            GrantState::Active
        );
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&move_to(5, 5), None).unwrap();
        client.send_message(&type_text("hi"), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        drop(expect_frame(&mut client, 6));
        assert_eq!(shared.actuations.len(), 2, "both actuations admitted");
        let (grant_row, _) = shared.grants.get(row, shared.now).unwrap();
        assert_eq!(
            grant_row.constraints.max_event_rate.get(),
            20,
            "wire rate 0 resolves to the documented server default"
        );
        assert_eq!(
            grant_row.constraints.expiry,
            Some(Duration::from_millis(300_000))
        );
        assert_eq!(grant_row.issuer, Issuer::AutoApprovePolicy);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            1,
        );
    }

    #[test]
    fn denied_petition_resolves_denied_cleanly() {
        let _fd = crate::capture::tests::fd_lock();
        // pending -> denied: a clean protocol event -- never a hang, never
        // a connection death (IDL flow 3).
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);

        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let pending = shared.petitions.pending_ids();
        assert_eq!(pending.len(), 1);
        let resolution = shared
            .petitions
            .resolve_scripted(pending[0], ScriptedDecision::Deny)
            .unwrap();
        deliver(&mut server, &mut core, &mut shared, resolution);

        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Denied);
        assert_eq!(resolved.verbs, Verb::default(), "trailing arguments zeroed");
        assert_eq!(resolved.persistence, WirePersistence::Once);
        assert_eq!(resolved.expiry_ms, 0);

        // No authority came into existence, and the connection lives.
        assert_eq!(server.grant_row_id(4), None);
        assert_eq!(shared.grants.rows(shared.now).count(), 0);
        assert_eq!(shared.petitions.pending_total(), 0);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            2,
        );
    }

    #[test]
    fn pending_petition_times_out_cleanly() {
        let _fd = crate::capture::tests::fd_lock();
        // pending -> timeout: expires without consent at exactly the
        // 120-second default deadline (fail-closed, half-open), resolving
        // timed_out as a clean event.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let deadline = shared.now + Duration::from_secs(120);
        assert!(
            shared
                .petitions
                .expire_due(deadline - Duration::from_millis(1))
                .is_empty(),
            "still pending strictly before the deadline"
        );
        let due = shared.petitions.expire_due(deadline);
        assert_eq!(due.len(), 1, "expired at exactly the deadline");
        deliver(
            &mut server,
            &mut core,
            &mut shared,
            due.into_iter().next().unwrap(),
        );

        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::TimedOut);
        assert_eq!(resolved.verbs, Verb::default());
        assert_eq!(shared.petitions.pending_total(), 0);
        assert_eq!(shared.grants.rows(shared.now).count(), 0);
        // Petitioning again later is legal (IDL): the same realm handle
        // petitions anew and pends anew.
        client
            .send_message(&petition_at(9).encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 10, ConsentState::Queued);
        assert_eq!(shared.petitions.pending_total(), 1);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            3,
        );
    }

    #[test]
    fn excess_petitions_resolve_busy_across_connections_of_one_identity() {
        let _fd = crate::capture::tests::fd_lock();
        // The admission cap is per verified identity ACROSS ALL ITS
        // CONNECTIONS (IDL): 3 pending on connection A plus 1 on B fill
        // the cap of 4, and B's next petition resolves busy immediately --
        // with no consent transitions (no prompt lifecycle ever began).
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        let (mut server_a, mut core_a, mut client_a) = connect(&mut shared);
        let (mut server_b, mut core_b, mut client_b) = connect(&mut shared);
        bind_with_realm(
            &mut server_a,
            &mut core_a,
            &mut client_a,
            &verifier,
            &mut shared,
        );
        bind_with_realm(
            &mut server_b,
            &mut core_b,
            &mut client_b,
            &verifier,
            &mut shared,
        );

        for base in [4, 9, 14] {
            client_a
                .send_message(&petition_at(base).encode(3), None)
                .unwrap();
            process_n(&mut server_a, &mut core_a, &verifier, &mut shared, 1).unwrap();
            expect_consent_state(&mut client_a, base + 1, ConsentState::Queued);
        }
        client_b
            .send_message(&petition_at(4).encode(3), None)
            .unwrap();
        process_n(&mut server_b, &mut core_b, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client_b, 5, ConsentState::Queued);
        assert_eq!(shared.petitions.pending_total(), 4);

        // The fifth concurrent petition of this identity: busy, on B, as
        // the very next event (decoding it as resolved also proves no
        // consent transition preceded it).
        client_b
            .send_message(&petition_at(9).encode(3), None)
            .unwrap();
        process_n(&mut server_b, &mut core_b, &verifier, &mut shared, 1).unwrap();
        let resolved = expect_resolved(&mut client_b, 9);
        assert_eq!(resolved.outcome, Outcome::Busy);
        assert_eq!(resolved.verbs, Verb::default());
        assert_eq!(shared.petitions.pending_total(), 4, "busy consumed no slot");

        // Retrying after an outstanding petition resolves is legal: deny
        // one of A's, then B's next petition pends normally.
        let first = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(first, ScriptedDecision::Deny)
            .unwrap();
        deliver(&mut server_a, &mut core_a, &mut shared, resolution);
        expect_consent_state(&mut client_a, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client_a, 4).outcome, Outcome::Denied);

        client_b
            .send_message(&petition_at(14).encode(3), None)
            .unwrap();
        process_n(&mut server_b, &mut core_b, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client_b, 15, ConsentState::Queued);
    }

    #[test]
    fn effective_authority_may_be_narrower_than_requested() {
        let _fd = crate::capture::tests::fd_lock();
        // Scripted approval narrows the petition (fewer verbs, shorter
        // rung, tighter expiry); resolved carries the EFFECTIVE authority
        // and the row states it -- the ungranted verb refuses not_granted
        // at the chokepoint query.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        let mut req = petition_frame();
        req.expiry_ms = 600_000;
        client.send_message(&req.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let petition = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(
                petition,
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::Once,
                    expiry_ms: 60_000,
                },
            )
            .unwrap();
        deliver(&mut server, &mut core, &mut shared, resolution);

        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Granted);
        assert_eq!(resolved.verbs, Verb::OBSERVE, "narrower than requested");
        assert_eq!(resolved.persistence, WirePersistence::Once);
        assert_eq!(resolved.expiry_ms, 60_000);

        let row = server.grant_row_id(4).unwrap();
        let (grant_row, _) = shared.grants.get(row, shared.now).unwrap();
        assert_eq!(grant_row.verbs, Verb::OBSERVE);
        assert_eq!(grant_row.persistence, PersistenceRung::Once);
        assert_eq!(grant_row.issuer, Issuer::ScriptedConsent);
        // Through the real enforcement path: the verb the human did not
        // grant refuses not_granted on its facet; the granted verb admits
        // -- and, Once, is spent by that admission, so the next capture
        // refuses expired (the rung-bounded lifetime passed).
        client.send_message(&type_text("nope"), None).unwrap();
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        expect_refused(&mut client, 4, Verb::ACTUATE_TEXT, Refusal::NotGranted);
        drop(expect_frame(&mut client, 6));
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::Expired);
        assert_eq!(
            shared.grants.get(row, shared.now).unwrap().1,
            GrantState::Spent
        );
        assert!(shared.actuations.is_empty(), "nothing was delivered");
    }

    #[test]
    fn resolved_fires_exactly_once_ever() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let petition = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(
                petition,
                ScriptedDecision::Approve {
                    verbs: all_verbs(),
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 0,
                },
            )
            .unwrap();
        let replay = resolution.clone();
        deliver(&mut server, &mut core, &mut shared, resolution);
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);

        // The registry consumed the petition: a second decision has
        // nothing to decide...
        assert_eq!(
            shared
                .petitions
                .resolve_scripted(petition, ScriptedDecision::Deny)
                .unwrap_err(),
            ScriptedError::NotPending
        );
        // ...the timeout sweep finds nothing...
        assert!(shared
            .petitions
            .expire_due(shared.now + Duration::from_secs(100_000))
            .is_empty());
        // ...and a replayed delivery is refused by the wire-side
        // exactly-once guard before anything is sent -- and before any row
        // is minted: the table still holds exactly the one row.
        let err = server
            .deliver_resolution(replay, &mut shared.grants, shared.now, &mut |frame, fd| {
                core.send_message(frame, fd)
            })
            .unwrap_err();
        assert!(matches!(err, DeliveryError::AlreadyResolved { wire_id: 4 }));
        assert_eq!(shared.grants.rows(shared.now).count(), 1);
        // Nothing further reached the client: done is the next event.
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            4,
        );
    }

    #[test]
    fn pending_petitions_are_withdrawn_on_disconnect() {
        let _fd = crate::capture::tests::fd_lock();
        // Connection teardown withdraws the closing connection's pending
        // petitions (no events -- the petitioner is gone) and removes its
        // granted rows; another connection of the same identity is
        // untouched.
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        let (mut server_a, mut core_a, mut client_a) = connect(&mut shared);
        let (mut server_b, mut core_b, mut client_b) = connect(&mut shared);
        bind_with_realm(
            &mut server_a,
            &mut core_a,
            &mut client_a,
            &verifier,
            &mut shared,
        );
        bind_with_realm(
            &mut server_b,
            &mut core_b,
            &mut client_b,
            &verifier,
            &mut shared,
        );

        // A: one granted row (scripted) + one pending petition.
        client_a
            .send_message(&petition_at(4).encode(3), None)
            .unwrap();
        client_a
            .send_message(&petition_at(9).encode(3), None)
            .unwrap();
        process_n(&mut server_a, &mut core_a, &verifier, &mut shared, 2).unwrap();
        expect_consent_state(&mut client_a, 5, ConsentState::Queued);
        expect_consent_state(&mut client_a, 10, ConsentState::Queued);
        let first = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(
                first,
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 0,
                },
            )
            .unwrap();
        deliver(&mut server_a, &mut core_a, &mut shared, resolution);
        expect_consent_state(&mut client_a, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client_a, 4).outcome, Outcome::Granted);
        let row_a = server_a.grant_row_id(4).unwrap();

        // B: one pending petition.
        client_b
            .send_message(&petition_at(4).encode(3), None)
            .unwrap();
        process_n(&mut server_b, &mut core_b, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client_b, 5, ConsentState::Queued);
        assert_eq!(shared.petitions.pending_total(), 2);

        // A's connection closes: its pending petition is withdrawn and its
        // granted row is removed -- grants die with the connection.
        server_a.teardown(&mut shared.petitions, &mut shared.grants);
        assert_eq!(shared.petitions.pending_total(), 1);
        // Removal, not revocation (the grant table's documented teardown
        // contract): the row is gone outright.
        assert!(shared.grants.get(row_a, shared.now).is_none());
        // The timeout sweep later finds only B's petition.
        let due = shared
            .petitions
            .expire_due(shared.now + Duration::from_secs(3600));
        assert_eq!(due.len(), 1);
        deliver(
            &mut server_b,
            &mut core_b,
            &mut shared,
            due.into_iter().next().unwrap(),
        );
        expect_consent_state(&mut client_b, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client_b, 4).outcome, Outcome::TimedOut);
        // Teardown is idempotent.
        server_a.teardown(&mut shared.petitions, &mut shared.grants);
    }

    #[test]
    fn undelivered_approval_leaves_no_row_behind_teardown() {
        let _fd = crate::capture::tests::fd_lock();
        // The disconnect-in-flight race: a petition is approved (scripted
        // today, E7 human consent tomorrow) but the petitioner's connection
        // dies before the embedder routes the resolution. Because the row
        // is minted at delivery, the approval alone creates no authority,
        // teardown leaves the table empty, and the late delivery is
        // refused whole -- "all of the principal's grants die with the
        // connection" (vitrin_principal) holds with no window.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let petition = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(
                petition,
                ScriptedDecision::Approve {
                    verbs: Verb::OBSERVE,
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 0,
                },
            )
            .unwrap();
        // The decision alone mints nothing.
        assert_eq!(shared.grants.rows(shared.now).count(), 0);

        // The connection dies before the resolution is routed back.
        server.teardown(&mut shared.petitions, &mut shared.grants);
        assert_eq!(shared.grants.rows(shared.now).count(), 0);

        // The embedder's late routing is refused whole: no events, no row,
        // and the handle never resolves (its petitioner no longer exists).
        let mut sent = 0usize;
        let err = server
            .deliver_resolution(resolution, &mut shared.grants, shared.now, &mut |_, _| {
                sent += 1;
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(err, DeliveryError::ConnectionDead));
        assert_eq!(sent, 0, "nothing is sent on a torn-down connection");
        assert_eq!(
            shared.grants.rows(shared.now).count(),
            0,
            "no authority survives its connection"
        );
    }

    #[test]
    fn no_resolution_is_delivered_after_the_fatal_goodbye() {
        let _fd = crate::capture::tests::fd_lock();
        // Error-then-close terminality (conventions 5.2): the fatal
        // goodbye is the connection's terminal event, so a resolution the
        // consent-timeout poll returns between the fatal and the
        // embedder's close/teardown callback must not emit anything.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        // A protocol violation kills the connection: goodbye out, DEAD.
        let mut frame = Vec::new();
        vitrin_protocol::wire::FrameHeader {
            object_id: HANDSHAKE_ID,
            size: 0,
            opcode: 9,
            fd_count: 0,
        }
        .encode_with_placeholder_size(&mut frame);
        vitrin_protocol::wire::patch_size(&mut frame);
        client.send_message(&frame, None).unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "invalid_opcode",
        );
        expect_error(&mut client, WireError::InvalidOpcode);

        // The timeout poll still returns the petition (only teardown
        // withdraws it), but delivery on the dead connection is refused
        // with nothing sent: the goodbye stays the terminal frame.
        let due = shared
            .petitions
            .expire_due(shared.now + Duration::from_secs(120));
        assert_eq!(due.len(), 1);
        let mut sent = 0usize;
        let err = server
            .deliver_resolution(
                due.into_iter().next().unwrap(),
                &mut shared.grants,
                shared.now,
                &mut |_, _| {
                    sent += 1;
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(matches!(err, DeliveryError::ConnectionDead));
        assert_eq!(
            sent, 0,
            "the fatal error event is the connection's terminal"
        );
    }

    #[test]
    fn durable_rungs_reserved_flags_and_finer_resources_resolve_unsupported() {
        let _fd = crate::capture::tests::fd_lock();
        // Well-formed, in-range petitions the deployment declines resolve
        // unsupported -- honest refusal, never a protocol error, never a
        // connection death (IDL). No consent transitions: the prompt
        // lifecycle never began.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);

        let mut until_revoked = petition_at(4);
        until_revoked.persistence = WirePersistence::UntilRevoked;
        let mut always = petition_at(9);
        always.persistence = WirePersistence::Always;
        let mut flagged = petition_at(14);
        flagged.flags = 0b1; // reserved one_shot bit
        let mut finer = petition_at(19);
        finer.resource = "surface:main".into();

        for (req, grant_id) in [(until_revoked, 4), (always, 9), (flagged, 14), (finer, 19)] {
            client.send_message(&req.encode(3), None).unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            let resolved = expect_resolved(&mut client, grant_id);
            assert_eq!(resolved.outcome, Outcome::Unsupported);
            assert_eq!(resolved.verbs, Verb::default());
        }
        assert_eq!(shared.petitions.pending_total(), 0);
        assert_eq!(shared.grants.rows(shared.now).count(), 0);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            5,
        );
    }

    #[test]
    fn unknown_realm_petitions_resolve_unavailable() {
        let _fd = crate::capture::tests::fd_lock();
        // get_realm succeeds structurally for any name; the petition is
        // where absence surfaces, as unavailable -- a race, not a protocol
        // error (IDL flow 5).
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        send_get_realm(&mut client, 2, 3, "realm-does-not-exist");
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Unavailable);
        assert_eq!(shared.petitions.pending_total(), 0);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            6,
        );
    }

    #[test]
    fn zero_verb_petitions_are_fatal_invalid_argument() {
        let _fd = crate::capture::tests::fd_lock();
        // An empty petition is something a correct client can never intend
        // (IDL): fatal invalid_argument, not a resolution.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        let mut req = petition_frame();
        req.verbs = Verb::default();
        client.send_message(&req.encode(3), None).unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "empty petition verb set",
        );
        let err = expect_error(&mut client, WireError::InvalidArgument);
        assert_eq!(err.object_id, 3, "the citation names the realm handle");
    }

    #[test]
    fn petition_new_ids_follow_the_multi_new_id_rule() {
        let _fd = crate::capture::tests::fd_lock();
        // Non-distinct, non-increasing, at/below-watermark, and
        // reserved-range id sets are each fatal invalid_object
        // (conventions 3.2), before any petition exists.
        let verifier = demo_verifier();
        let cases: [(&str, [u32; 5]); 4] = [
            ("duplicate ids", [4, 5, 6, 7, 7]),
            ("non-increasing order", [4, 5, 6, 8, 7]),
            ("at/below the watermark", [3, 5, 6, 7, 8]),
            ("reserved server range", [4, 5, 6, 7, 0xff00_0000]),
        ];
        for (label, [g, c, v, p, t]) in cases {
            let (mut server, mut core, mut client, mut shared) = setup();
            bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
            let mut req = petition_frame();
            (req.grant, req.consent, req.view, req.pointer, req.text) = (g, c, v, p, t);
            client.send_message(&req.encode(3), None).unwrap();
            let result = process_n(&mut server, &mut core, &verifier, &mut shared, 1);
            expect_violation(result, "invalid_object");
            expect_error(&mut client, WireError::InvalidObject);
            assert_eq!(
                shared.petitions.pending_total(),
                0,
                "{label}: no petition may exist"
            );
        }
    }

    #[test]
    fn petition_rate_ceiling_is_fatal_resource_exhausted() {
        let _fd = crate::capture::tests::fd_lock();
        // The burst allows PETITION_RATE_BURST petitions at one instant;
        // one more is the documented DoS-confinement fatal.
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        for i in 0..PETITION_RATE_BURST {
            let base = 4 + 5 * i;
            client
                .send_message(&petition_at(base).encode(3), None)
                .unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            expect_consent_state(&mut client, base + 1, ConsentState::Closed);
            assert_eq!(expect_resolved(&mut client, base).outcome, Outcome::Granted);
        }
        let base = 4 + 5 * PETITION_RATE_BURST;
        client
            .send_message(&petition_at(base).encode(3), None)
            .unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "petition-rate ceiling",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn petition_rate_refills_with_time() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        // Drain the burst.
        for i in 0..PETITION_RATE_BURST {
            let base = 4 + 5 * i;
            client
                .send_message(&petition_at(base).encode(3), None)
                .unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            expect_consent_state(&mut client, base + 1, ConsentState::Closed);
            expect_resolved(&mut client, base);
        }
        // One second refills exactly one token: the next petition is
        // served, the one after (same instant) is the fatal ceiling.
        shared.now += Duration::from_secs(1);
        let base = 4 + 5 * PETITION_RATE_BURST;
        client
            .send_message(&petition_at(base).encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, base + 1, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, base).outcome, Outcome::Granted);
        let base = base + 5;
        client
            .send_message(&petition_at(base).encode(3), None)
            .unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "petition-rate ceiling",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn live_petition_cap_is_fatal_resource_exhausted() {
        let _fd = crate::capture::tests::fd_lock();
        // Every petition permanently allocates five ids; the cap bounds
        // the permanent population and its breach is fatal (conventions
        // 5.2). Time advances one second per petition so the rate bucket
        // never binds first.
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        for i in 0..MAX_LIVE_PETITIONS as u32 {
            shared.now += Duration::from_secs(1);
            let base = 4 + 5 * i;
            client
                .send_message(&petition_at(base).encode(3), None)
                .unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            expect_consent_state(&mut client, base + 1, ConsentState::Closed);
            expect_resolved(&mut client, base);
        }
        shared.now += Duration::from_secs(1);
        let base = 4 + 5 * MAX_LIVE_PETITIONS as u32;
        client
            .send_message(&petition_at(base).encode(3), None)
            .unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "live-petition cap",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn sync_done_does_not_wait_for_pending_petitions() {
        let _fd = crate::capture::tests::fd_lock();
        // PETITION EVENT ORDERING (IDL): done confirms the petition was
        // registered and its consent initiated -- queued precedes it --
        // but never waits for resolution.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        send_sync(&mut client, 42);
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);
        let msg = client.recv_message().unwrap().unwrap();
        let (object_id, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(object_id, HANDSHAKE_ID);
        assert_eq!(done.cookie, 42, "done arrives with the petition unresolved");
        assert_eq!(shared.petitions.pending_total(), 1);
    }

    #[test]
    fn grant_and_consent_objects_define_no_requests() {
        let _fd = crate::capture::tests::fd_lock();
        // Any opcode on the co-minted grant or consent object is grammar
        // (invalid_opcode), never an authority judgement.
        let verifier = demo_verifier();
        for object_id in [4u32, 5] {
            let (mut server, mut core, mut client, mut shared) = setup();
            bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
            client
                .send_message(&petition_frame().encode(3), None)
                .unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            expect_consent_state(&mut client, 5, ConsentState::Queued);
            let mut frame = Vec::new();
            vitrin_protocol::wire::FrameHeader {
                object_id,
                size: 0,
                opcode: 0,
                fd_count: 0,
            }
            .encode_with_placeholder_size(&mut frame);
            vitrin_protocol::wire::patch_size(&mut frame);
            client.send_message(&frame, None).unwrap();
            expect_violation(
                process_n(&mut server, &mut core, &verifier, &mut shared, 1),
                "invalid_opcode",
            );
            expect_error(&mut client, WireError::InvalidOpcode);
        }
    }

    // -- acceptance: the enforcement chokepoint (P1.4.4, issue #28) --------

    #[test]
    fn rate_ceiling_refuses_excess_captures_with_retry_hint_in_order() {
        let _fd = crate::capture::tests::fd_lock();
        // The issue's acceptance shape: 100 captures against a 5/s grant.
        // Exactly 5 frame_ready, then 95 refused(observe, rate_limited)
        // with a nonzero refill hint -- one terminal per capture, in
        // request order, never coalesced (reply-bearing contract).
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 5, 0);

        for _ in 0..100 {
            client.send_message(&capture_frame(), None).unwrap();
        }
        process_n(&mut server, &mut core, &verifier, &mut shared, 100).unwrap();
        for _ in 0..5 {
            drop(expect_frame(&mut client, 6));
        }
        for _ in 0..95 {
            let refused = expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::RateLimited);
            assert_eq!(
                refused.retry_after_ms, 200,
                "5/s: the next whole token accrues in 200 ms"
            );
        }
        // The bucket refills with injected time: one token 200 ms later.
        shared.now += Duration::from_millis(200);
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        drop(expect_frame(&mut client, 6));
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::RateLimited);
        // The connection survived everything: refusals are recoverable.
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            7,
        );
    }

    #[test]
    fn expired_grant_refuses_cleanly_on_capture_and_actuation() {
        let _fd = crate::capture::tests::fd_lock();
        // Acceptance: an expired grant's next call fails cleanly -- typed
        // refused(expired) on both the observe and actuation paths, with
        // the connection alive.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 1_000);

        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        drop(expect_frame(&mut client, 6));

        // At the (half-open, fail-closed) deadline the authority is gone.
        shared.now += Duration::from_millis(1_000);
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&move_to(1, 1), None).unwrap();
        client.send_message(&type_text("late"), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::Expired);
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Expired);
        expect_refused(&mut client, 4, Verb::ACTUATE_TEXT, Refusal::Expired);
        assert!(shared.actuations.is_empty(), "nothing reached the sink");
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            8,
        );
    }

    #[test]
    fn proactive_sweep_flips_expiry_without_a_use_and_the_next_use_refuses() {
        let _fd = crate::capture::tests::fd_lock();
        // The proactive half of the expiry decision: the embedder-polled
        // sweep (the petitions::expire_due pattern) flips the row's state
        // with no use in between; the wire then refuses on the next call.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 1_000);
        let row = server.grant_row_id(4).unwrap();

        shared.now += Duration::from_secs(2);
        assert_eq!(shared.grants.expire_due(shared.now), vec![row]);
        assert_eq!(
            shared.grants.get(row, shared.now).unwrap().1,
            GrantState::Expired,
            "state flipped by the sweep, not by a use"
        );
        assert!(
            shared.grants.expire_due(shared.now).is_empty(),
            "idempotent"
        );

        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::Expired);
    }

    #[test]
    fn ungranted_facets_refuse_not_granted_while_pending_and_after_denial() {
        let _fd = crate::capture::tests::fd_lock();
        // not_granted covers the whole never-active family (IDL): use
        // while the petition is pending, and use after a non-granted
        // resolution. Captures voice every refusal (reply-bearing);
        // actuation refusals coalesce per (verb, code) until a success.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        // Use while pending: two captures -> two refusals (uncoalesced);
        // two moves -> ONE refusal (coalesced per (verb, code)); a type is
        // a different (verb, code) pair and gets its own.
        for frame in [
            capture_frame(),
            capture_frame(),
            move_to(1, 1),
            move_to(2, 2),
            type_text("x"),
        ] {
            client.send_message(&frame, None).unwrap();
        }
        process_n(&mut server, &mut core, &verifier, &mut shared, 5).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NotGranted);
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NotGranted);
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::NotGranted);
        expect_refused(&mut client, 4, Verb::ACTUATE_TEXT, Refusal::NotGranted);
        // The barrier proves the second move's refusal was coalesced away:
        // done is the very next event.
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            9,
        );

        // Denial resolves the petition; the facets stay inert with the
        // same code, and the connection stays alive.
        let petition = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(petition, ScriptedDecision::Deny)
            .unwrap();
        deliver(&mut server, &mut core, &mut shared, resolution);
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Denied);
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NotGranted);
        assert!(shared.actuations.is_empty());
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            10,
        );
    }

    #[test]
    fn revocation_lands_on_the_very_next_request() {
        let _fd = crate::capture::tests::fd_lock();
        // The table's revocation (the hold-Esc UX is #39; panel/policy is
        // the caller here) refuses on the very next facet use -- no grace,
        // no cache -- at the same injected instant.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        let row = server.grant_row_id(4).unwrap();

        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        drop(expect_frame(&mut client, 6));

        assert!(shared.grants.revoke(row));
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&move_to(1, 1), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::Revoked);
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Revoked);
        assert!(shared.actuations.is_empty());
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            11,
        );
    }

    #[test]
    fn physical_input_preempts_actuation_but_never_capture() {
        let _fd = crate::capture::tests::fd_lock();
        // PRD Doc 2 SS8 / IDL: while physical human input owns the target,
        // actuations refuse preempted; observation is concurrent by design
        // and captures on. The window releases with injected time.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);

        // A physically held button owns the target outright.
        shared.presence.note(
            Origin::Physical,
            &SeatInputKind::Button {
                button: 0x110,
                state: pointer::ButtonState::Pressed,
            },
            shared.now,
        );
        client.send_message(&move_to(1, 1), None).unwrap();
        client.send_message(&type_text("blocked"), None).unwrap();
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Preempted);
        expect_refused(&mut client, 4, Verb::ACTUATE_TEXT, Refusal::Preempted);
        drop(expect_frame(&mut client, 6));
        assert!(shared.actuations.is_empty());

        // Release the button; the transient window still holds, then
        // passes -- and the next actuation is admitted, origin-tagged
        // emulated for the delivery path (B2).
        shared.presence.note(
            Origin::Physical,
            &SeatInputKind::Button {
                button: 0x110,
                state: pointer::ButtonState::Released,
            },
            shared.now,
        );
        client.send_message(&move_to(2, 2), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Preempted);

        shared.now += PHYSICAL_HOLD_WINDOW;
        client.send_message(&move_to(3, 4), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(shared.actuations.len(), 1);
        assert_eq!(shared.actuations[0].origin(), Origin::Emulated);
        assert_eq!(
            shared.actuations[0].kind(),
            &SeatInputKind::Motion { x: 3.0, y: 4.0 }
        );
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            12,
        );
    }

    #[test]
    fn vacant_realm_refuses_no_surface_for_capture_and_actuation_alike() {
        let _fd = crate::capture::tests::fd_lock();
        // With no realm surface (shim never attached, crashed, or exited)
        // every use refuses no_surface: a capture must never serve a
        // stale frame, and an actuation must be refused audibly rather
        // than swallowed -- the IDL's refusal entry is verb-neutral,
        // prose pages 07/08 list no_surface in both actuators'
        // applicable sets, and the sync-barrier discovery idiom (IDL
        // sync) relies on the refusal being voiced before done. (The
        // delivery-edge drop the shim session documents covers a live
        // realm whose seat is not yet minted -- not a vacant realm.)
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        shared.view = None;

        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&move_to(9, 9), None).unwrap();
        client
            .send_message(&type_text("into the void"), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NoSurface);
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::NoSurface);
        expect_refused(&mut client, 4, Verb::ACTUATE_TEXT, Refusal::NoSurface);
        assert!(
            shared.actuations.is_empty(),
            "nothing reaches the delivery sink for a vacant realm"
        );

        // The realm coming back serves immediately -- the vacant period
        // burned no quota on any facet (no_surface precedes the bucket)
        // -- and the admitted actuation ends the refusal coalescing
        // windows.
        shared.view = Some((crate::test_pattern::render(VIEW_W, VIEW_H), VIEW_W, VIEW_H));
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&move_to(1, 2), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        drop(expect_frame(&mut client, 6));
        assert_eq!(
            shared.actuations.len(),
            1,
            "admitted once the realm is back"
        );
        assert_eq!(shared.actuations[0].origin(), Origin::Emulated);
        assert_eq!(
            shared.actuations[0].kind(),
            &SeatInputKind::Motion { x: 1.0, y: 2.0 }
        );
    }

    #[test]
    fn consent_held_refuses_actuation_while_the_principals_own_prompt_is_up() {
        let _fd = crate::capture::tests::fd_lock();
        // The documented mapping (petitions module docs): a SHOWN prompt
        // of the principal's own pending petition holds its actuations --
        // queued does not, capture is unaffected, and the hold ends when
        // the petition resolves.
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);

        // Grant A (scripted approval of petition 1): live authority.
        client
            .send_message(&petition_at(4).encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);
        let first = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(
                first,
                ScriptedDecision::Approve {
                    verbs: all_verbs(),
                    persistence: PersistenceRung::WhileRunning,
                    expiry_ms: 0,
                },
            )
            .unwrap();
        deliver(&mut server, &mut core, &mut shared, resolution);
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);

        // Petition 2 pends (queued): actuation under grant A is NOT held.
        client
            .send_message(&petition_at(9).encode(3), None)
            .unwrap();
        client.send_message(&move_to(1, 1), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        expect_consent_state(&mut client, 10, ConsentState::Queued);
        assert_eq!(shared.actuations.len(), 1, "queued does not hold");

        // The prompt goes up (what E7's renderer will do): actuations
        // under grant A refuse consent_held; capture is unaffected.
        let second = shared.petitions.pending_ids()[0];
        assert!(shared.petitions.mark_prompt_shown(second));
        client.send_message(&move_to(2, 2), None).unwrap();
        client.send_message(&type_text("held"), None).unwrap();
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::ConsentHeld);
        expect_refused(&mut client, 4, Verb::ACTUATE_TEXT, Refusal::ConsentHeld);
        drop(expect_frame(&mut client, 6));
        assert_eq!(shared.actuations.len(), 1, "held actuations never deliver");

        // The prompt closes with the petition's resolution: the hold ends.
        let resolution = shared
            .petitions
            .resolve_scripted(second, ScriptedDecision::Deny)
            .unwrap();
        deliver(&mut server, &mut core, &mut shared, resolution);
        expect_consent_state(&mut client, 10, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 9).outcome, Outcome::Denied);
        client.send_message(&move_to(3, 3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(shared.actuations.len(), 2, "the hold ended with the prompt");
    }

    #[test]
    fn capture_terminals_pair_in_request_order_under_mixed_outcomes() {
        let _fd = crate::capture::tests::fd_lock();
        // The reply-bearing contract under a mixed allow/refuse sequence:
        // terminal n answers capture n -- frame, frame, rate-limited,
        // (realm dies) no_surface, (realm returns, tokens refilled) frame.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 2, 0);

        for _ in 0..3 {
            client.send_message(&capture_frame(), None).unwrap();
        }
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        drop(expect_frame(&mut client, 6));
        drop(expect_frame(&mut client, 6));
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::RateLimited);

        shared.view = None;
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NoSurface);

        shared.view = Some((crate::test_pattern::render(VIEW_W, VIEW_H), VIEW_W, VIEW_H));
        shared.now += Duration::from_secs(1);
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        drop(expect_frame(&mut client, 6));
    }

    #[test]
    fn rate_limited_actuation_refusals_coalesce_per_refill_window() {
        let _fd = crate::capture::tests::fd_lock();
        // Fire-and-forget coalescing, rule (a): at most one
        // refused(rate_limited) per grant per bucket-refill window; a new
        // window (token accrued and spent) voices a fresh one.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 5, 0);

        for i in 0..25 {
            client.send_message(&move_to(i, i), None).unwrap();
        }
        process_n(&mut server, &mut core, &verifier, &mut shared, 25).unwrap();
        assert_eq!(shared.actuations.len(), 5, "the burst admits 5 under 5/s");
        let refused = expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::RateLimited);
        assert_eq!(refused.retry_after_ms, 200);
        // Exactly one refusal for the whole 20-event excess: done is next.
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            13,
        );

        // The window turns over: one token accrues and is spent, the next
        // excess opens a new window and voices one fresh refusal.
        shared.now += Duration::from_millis(200);
        for i in 0..3 {
            client.send_message(&move_to(i, i), None).unwrap();
        }
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        assert_eq!(shared.actuations.len(), 6);
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::RateLimited);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            14,
        );
    }

    #[test]
    fn a_transient_refusal_never_burns_single_use_authority() {
        let _fd = crate::capture::tests::fd_lock();
        // Two-phase admission end to end: a once grant refused preempted
        // keeps its single use (the IDL's transient codes promise
        // recovery), and the use that is finally admitted is the one that
        // spends it.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        let mut req = petition_frame();
        req.persistence = WirePersistence::Once;
        client.send_message(&req.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);
        let petition = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_scripted(
                petition,
                ScriptedDecision::Approve {
                    verbs: all_verbs(),
                    persistence: PersistenceRung::Once,
                    expiry_ms: 0,
                },
            )
            .unwrap();
        deliver(&mut server, &mut core, &mut shared, resolution);
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);
        let row = server.grant_row_id(4).unwrap();

        // Human active: the once actuation refuses preempted -- and stays
        // unspent.
        shared.presence.note(
            Origin::Physical,
            &SeatInputKind::Motion { x: 1.0, y: 1.0 },
            shared.now,
        );
        client.send_message(&move_to(1, 1), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Preempted);
        assert_eq!(
            shared.grants.get(row, shared.now).unwrap().1,
            GrantState::Active
        );

        // Human idle: the same actuation is admitted and spends the once;
        // the next use refuses expired (rung-bounded lifetime passed).
        shared.now += PHYSICAL_HOLD_WINDOW;
        client.send_message(&move_to(1, 1), None).unwrap();
        client.send_message(&move_to(2, 2), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        assert_eq!(shared.actuations.len(), 1);
        assert_eq!(
            shared.grants.get(row, shared.now).unwrap().1,
            GrantState::Spent
        );
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Expired);
    }

    #[test]
    fn actuation_coordinates_outside_the_view_are_clamped_not_refused() {
        let _fd = crate::capture::tests::fd_lock();
        // IDL vitrin_actuator_pointer / conventions 6.3: out-of-view
        // coordinates are clamped into the view, never an error.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        client.send_message(&move_to(-50, 1_000_000), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            shared.actuations[0].kind(),
            &SeatInputKind::Motion {
                x: 0.0,
                y: f64::from(VIEW_H - 1),
            }
        );
    }

    #[test]
    fn corrupt_readback_refuses_internal_after_admission() {
        let _fd = crate::capture::tests::fd_lock();
        // A post-admission server-side failure is the recoverable
        // `internal`, never a torn connection and never a no_surface lie.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        // Claimed dimensions disagree with the readback buffer.
        shared.view = Some((vec![0u8; 16], VIEW_W, VIEW_H));
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::Internal);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            15,
        );
    }

    #[test]
    fn forbidden_control_characters_in_type_are_fatal_invalid_argument() {
        let _fd = crate::capture::tests::fd_lock();
        // IDL vitrin_actuator_text: newline and tab are the two legal
        // control characters; any other C0/C1 control is fatal
        // invalid_argument (a correct client never emits them). DEL
        // (U+007F) is in neither set and passes -- the server is exactly
        // as strict as the wire contract, no more.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        client
            .send_message(&type_text("line\nwith\ttabs and del\u{7f} ok"), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            shared.actuations[0].kind(),
            &SeatInputKind::Text {
                text: "line\nwith\ttabs and del\u{7f} ok".into()
            }
        );

        for (label, bad) in [("C0 bell", "ring\u{7}"), ("C1 NEL", "next\u{85}line")] {
            let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
            client.send_message(&type_text(bad), None).unwrap();
            expect_violation(
                process_n(&mut server, &mut core, &verifier, &mut shared, 1),
                "forbidden control character",
            );
            let err = expect_error(&mut client, WireError::InvalidArgument);
            assert_eq!(err.object_id, 8, "{label}: the citation names the facet");
        }
    }

    #[test]
    fn unknown_opcodes_on_facets_stay_grammar_errors() {
        let _fd = crate::capture::tests::fd_lock();
        // An undefined opcode on a facet is invalid_opcode (grammar),
        // never an authority judgement -- even on a fully granted facet.
        let verifier = demo_verifier();
        for object_id in [6u32, 7, 8] {
            let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
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
            expect_violation(
                process_n(&mut server, &mut core, &verifier, &mut shared, 1),
                "invalid_opcode",
            );
            expect_error(&mut client, WireError::InvalidOpcode);
        }
    }

    // -- acceptance: sender constraint -------------------------------------

    #[test]
    fn handles_are_sender_constrained_across_connections() {
        let _fd = crate::capture::tests::fd_lock();
        // Connection A binds and mints realm handle 3. Connection B binds
        // with the same verifier and presents A's handle: B's per-connection
        // table does not know it, so B dies fatal invalid_object -- while A
        // and its handle stay fully live. This is the D2 per-connection id
        // model enforced end to end: the identity layer introduces no
        // cross-connection handle namespace.
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        let (mut server_a, mut core_a, mut client_a) = connect(&mut shared);
        bind(
            &mut server_a,
            &mut core_a,
            &mut client_a,
            &verifier,
            &mut shared,
        );
        send_get_realm(&mut client_a, 2, 3, "realm-0");
        process_n(&mut server_a, &mut core_a, &verifier, &mut shared, 1).unwrap();

        let (mut server_b, mut core_b, mut client_b) = connect(&mut shared);
        bind(
            &mut server_b,
            &mut core_b,
            &mut client_b,
            &verifier,
            &mut shared,
        );
        // B presents A's realm handle (id 3). B's own table has ids 1 and 2
        // only. B could even have minted nothing: the number is meaningless
        // outside the connection that allocated it.
        client_b
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        expect_violation(
            process_n(&mut server_b, &mut core_b, &verifier, &mut shared, 1),
            "unknown or foreign object id",
        );
        expect_error(&mut client_b, WireError::InvalidObject);
        assert_eq!(server_b.phase, Phase::Dead, "B's connection must be dead");

        // A is untouched: its handle still exists and its connection still
        // answers the sync barrier.
        assert!(server_a.is_bound());
        assert_eq!(
            server_a.objects.get(&3),
            Some(&ObjectKind::Realm {
                name: "realm-0".into()
            })
        );
        sync_fence(
            &mut server_a,
            &mut core_a,
            &mut client_a,
            &verifier,
            &mut shared,
            99,
        );
    }

    // -- acceptance: failed handshake drops pipelined traffic --------------

    #[test]
    fn requests_pipelined_behind_a_failed_hello_are_never_processed() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        // Pipeline: bad hello + get_realm in one burst.
        send_hello(&mut client, 2, DEMO_IDENTITY, "wrong-token");
        send_get_realm(&mut client, 2, 3, "realm-0");
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "auth_failed",
        );
        // The embedder contract closes here. Even if a buggy embedder kept
        // dispatching, the DEAD phase refuses to process the queued mint.
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "dead connection",
        );
        assert!(
            server.objects.is_empty(),
            "the queued mint must not execute"
        );
    }

    #[test]
    fn requests_pipelined_behind_a_successful_hello_are_served_after_bound() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        // hello + get_realm + sync pipelined in one burst (Flow 2).
        send_hello(&mut client, 2, DEMO_IDENTITY, TOKEN);
        send_get_realm(&mut client, 2, 3, "realm-0");
        send_sync(&mut client, 5);
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        // Client-visible order: bound first, then done; the mint executed.
        let msg = client.recv_message().unwrap().unwrap();
        let (_, bound) = principal::events::Bound::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(bound.identity, DEMO_IDENTITY);
        let msg = client.recv_message().unwrap().unwrap();
        let (_, done) = handshake::events::Done::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(done.cookie, 5);
        assert_eq!(
            server.objects.get(&3),
            Some(&ObjectKind::Realm {
                name: "realm-0".into()
            })
        );
    }

    // -- acceptance: the trait fits a non-static verifier ------------------

    #[test]
    fn a_mock_nonstatic_verifier_fits_the_trait_shape() {
        let _fd = crate::capture::tests::fd_lock();
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
        let (mut server, mut core, mut client, mut shared) = setup();
        send_hello(
            &mut client,
            2,
            "spiffe://PROD.example/workload/scraper",
            "directory-credential-0123",
        );
        process_n(
            &mut server,
            &mut core,
            &verifier as &dyn Verifier,
            &mut shared,
            1,
        )
        .unwrap();
        let msg = client.recv_message().unwrap().unwrap();
        let (_, bound) = principal::events::Bound::decode(&msg.bytes, msg.fd).unwrap();
        assert_eq!(bound.identity, "spiffe://prod.example/workload/scraper");
        assert_ne!(bound.identity, "spiffe://PROD.example/workload/scraper");

        // An outage is wire-uniform auth_failed, like every refusal.
        verifier.outage.set(true);
        let (mut server, mut core, mut client, mut shared) = setup();
        send_hello(
            &mut client,
            2,
            "spiffe://prod.example/workload/scraper",
            "directory-credential-0123",
        );
        expect_violation(
            process_n(
                &mut server,
                &mut core,
                &verifier as &dyn Verifier,
                &mut shared,
                1,
            ),
            "verifier unavailable",
        );
        let err = expect_error(&mut client, WireError::AuthFailed);
        assert_eq!(err.message, AUTH_REFUSED_PHRASE);
    }
}
