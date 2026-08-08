// SPDX-License-Identifier: MPL-2.0
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
//! everything policy-scoped (reserved flags, durable rungs, resource
//! granularity, admission caps, the consent decision) -- so the
//! petition-policy razor has one home and the object-graph razor has
//! another. Realm *existence* is a third module's, [`realm`]'s: this
//! module carries the name a `get_realm` handle was minted with, and
//! admission resolves it against the realm registry.
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
//! `move`, `button`, `scroll`, `type`, and since version 2 `launch` -- is
//! decoded here (grammar and
//! argument validation stay connection-scoped: `type`'s forbidden-control-
//! character rule is fatal `invalid_argument`, like the zero-verb rule),
//! then handed as one [`UseKind`] to the **single enforcement function**,
//! [`Chokepoint::enforce_use`], which owns the whole `connection ->
//! principal -> grant -> verbs -> constraints` decision, every
//! `vitrin_grant.refused`, and the admitted operation (frame delivery /
//! origin-tagged actuation intake). This module never answers an authority
//! question itself -- no second enforcement voice exists, and
//! [`enforcement`]'s single-path test greps this file to prove it.
//!
//! **The version-2 launch facet is minted here, not co-minted.**
//! `request_grant`'s five `new_id` arguments are frozen forever, so
//! [`get_launcher`](PrincipalServer::handle_get_launcher) is a structural
//! mint **on the grant** -- the route every facet added after version 1
//! must take. The mint is always legal (object-graph rules only: the
//! live-object cap and the watermark rule, both fatal); the *use* is what
//! is judged, and it funnels through the same `serve_facet_use` as every
//! other facet -- answering `launched(realm)` when the chokepoint admits
//! it (WS-E.1.1, issue #207) and a recoverable `refused(realm_launch, …)`
//! with the connection intact when it does not. Answering the mint itself
//! with `invalid_opcode` -- the shape this dispatch had before the facet
//! existed -- would kill a conformant client for sending a documented
//! request, which is the razor's fatal-vs-recoverable line exactly. Every
//! *other* opcode on a grant, and every opcode at all on a consent
//! object (which defines no requests at any version), stays fatal
//! `invalid_opcode`: grammar, not authority.
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
//! Both are now filled, by [`crate::session`], and both stay *seams* rather
//! than becoming this module's business -- the server still reads no clock
//! and owns no timer.
//!
//! - The **unauthenticated deadline** (conventions 7.1 SHOULD) is a calloop
//!   timer the runtime arms at accept and disarms the first time
//!   [`is_bound`](PrincipalServer::is_bound) returns true; on elapse the
//!   runtime closes the connection and tears it down.
//! - The **consent-timeout timer** is the runtime's advisory sweep: it polls
//!   [`PetitionRegistry::expire_due`] (petitions' module docs) and routes
//!   each returned resolution to its connection's
//!   [`deliver_resolution`](PrincipalServer::deliver_resolution).
//!
//! # The flight recorder's emission sites (P1.4.5, issue #29)
//!
//! This module is where most of the [`recorder`]'s entries are written,
//! and each one is written at a site that was *already* the single site
//! for the thing it records -- so an event cannot be recorded twice and
//! cannot be missed by adding a second path:
//!
//! - [`handle_hello`](PrincipalServer::handle_hello)'s verify arm: the two
//!   handshake outcomes, honoring the secrecy contract (canonical identity
//!   on a bind; a fixed cause class plus the *claimed* identity and the
//!   credential's **length** on a refusal -- never its bytes);
//! - [`handle_request_grant`](PrincipalServer::handle_request_grant): the
//!   petition's requested authority, recorded before admission judges it,
//!   plus the `queued` consent transition;
//! - [`deliver_resolution`](PrincipalServer::deliver_resolution): the
//!   `closed` transition and the resolution's outcome, effective authority,
//!   row id and issuer -- recorded at the flip, before the sends (see that
//!   method's docs for why);
//! - [`serve_facet_use`](PrincipalServer::serve_facet_use): every
//!   enforcement decision, read out of the chokepoint's returned
//!   [`UseOutcome`] rather than from inside it, so [`enforcement`] holds no
//!   recorder call at all;
//! - [`teardown`](PrincipalServer::teardown): the connection's end.
//!
//! The remaining entries have no connection to write them -- the proactive
//! expiry and revocation sweeps -- and are recorded by the embedder through
//! [`Recorder::record_expiry_sweep`] / [`Recorder::record_revocations`], on
//! the same poll cadence that drives the sweeps themselves.
//!
//! [`recorder`]: crate::recorder
//! [`Recorder::record_expiry_sweep`]: crate::recorder::Recorder::record_expiry_sweep
//! [`Recorder::record_revocations`]: crate::recorder::Recorder::record_revocations
//! [`UseOutcome`]: crate::enforcement::UseOutcome
//! [`identity`]: crate::identity
//! [`petitions`]: crate::petitions
//! [`realm`]: crate::realm
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
use vitrin_protocol::generated::vitrin_launcher as launcher;
use vitrin_protocol::generated::vitrin_layout_arrange as layout_arrange;
use vitrin_protocol::generated::vitrin_layout_focus as layout_focus;
use vitrin_protocol::generated::vitrin_principal as principal;
use vitrin_protocol::generated::vitrin_realm as realm;
use vitrin_protocol::generated::vitrin_view as view;
use vitrin_protocol::generated::PROTOCOL_VERSION;

use crate::capture::RealmViewFrame;
use crate::enforcement::{Chokepoint, LayoutMode, UseEnv, UseKind, UseOutcome, UseRequest};
use crate::grants::{GrantId, GrantTable, InsertError, RealmId};
use crate::identity::{PresentedCredential, PrincipalIdentity, Verifier, VerifyOutcome};
use crate::input::{PhysicalPresenceMap, SeatInput, SeatInputKind};
use crate::petitions::{
    Admission, ConnectionId, PetitionRegistry, PetitionRequest, PromptRoute, Resolution, Verdict,
};
use crate::realm::RealmRegistry;
use crate::recorder::{
    self, auth_cause_class, ActuationDetail, Event, Recorder, RequestedAuthority,
    VERIFIER_UNAVAILABLE_CLASS,
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

/// Cap on launch facets per connection -- the live-object cap's
/// `get_launcher` half. The IDL permits minting a second, equivalent
/// facet on the same grant (no destructors, ids never reused, each
/// checked against the same grant at use time, so a duplicate confers no
/// authority); what bounds the repetition is this cap, and the IDL says
/// so. Derived rather than invented: a compliant client needs exactly one
/// launcher per grant, and a connection can hold no more grants than
/// [`MAX_LIVE_PETITIONS`]. Breach is fatal `resource_exhausted`, the same
/// denial-of-service confinement every other bound uses.
pub(crate) const MAX_LIVE_LAUNCHERS: usize = MAX_LIVE_PETITIONS;

/// Cap on **layout facets** per connection, counting `get_layout_focus`
/// and `get_layout_arrange` together. Same derivation and same fatal
/// `resource_exhausted` breach as [`MAX_LIVE_LAUNCHERS`], and shared
/// between the two mints rather than split: the bound exists to limit
/// permanent per-connection allocations, and two independent half-caps
/// would let a connection hold twice as many objects for the same reason
/// one cap admits.
///
/// A compliant client needs at most two per grant, so the cap is deliberately
/// generous against the bound a real client hits ([`MAX_LIVE_PETITIONS`]
/// grants) and tight against a connection minting facets in a loop.
pub(crate) const MAX_LIVE_LAYOUT_FACETS: usize = 2 * MAX_LIVE_PETITIONS;

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
    /// `version_unsupported`: `hello` offered a version this server does
    /// not implement. The protocol reserves the code for a version
    /// **above** the server's maximum (conventions section 7.3: additive
    /// growth means a maximum-N server implements 1..=N), but this core
    /// accepts exactly [`PROTOCOL_VERSION`] and nothing else, so it also
    /// raises this for version 1 -- *below* the maximum. That divergence
    /// is the disclosed gap in conventions section 7.3's implementation-
    /// status note, not a second reading of the code's meaning; the
    /// wire-visible behavior is identical either way (fatal, no
    /// supported-version hint).
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

/// Why [`PrincipalServer::emit_consent_shown`] refused or failed to
/// announce a raised prompt (P1.7.2).
///
/// Deliberately separate from [`DeliveryError`]: that type's variants are
/// about a *grant terminal* -- minting a row, the exactly-once handle flip
/// -- and none of them apply to an advisory `state` event. Folding the two
/// would mean a caller handling `AlreadyResolved` for a message that
/// resolves nothing.
///
/// Every variant means **nothing was sent**. The consequence of a refusal
/// is bounded by the protocol itself: `vitrin_consent` is advisory ("a
/// threadless blocking client MAY ignore consent events entirely"), so a
/// prompt whose `shown` announcement could not be delivered is still a
/// prompt the human answers, and the authoritative terminal still arrives
/// on the grant.
#[derive(Debug)]
pub(crate) enum PromptEmitError {
    /// The prompt belongs to a different connection (embedder routing bug).
    WrongConnection {
        expected: ConnectionId,
        got: ConnectionId,
    },
    /// The connection is no longer in its bound steady state -- the fatal
    /// goodbye is its terminal event, and teardown ends it just as finally.
    /// An expected race (the petitioner dies as its prompt goes up), not a
    /// bug.
    ConnectionDead,
    /// No `vitrin_consent` object with this wire id exists on this
    /// connection (routing bug).
    UnknownConsentObject { wire_id: u32 },
    /// Sending failed; the connection is dying.
    Transport(TransportError),
}

impl fmt::Display for PromptEmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PromptEmitError::WrongConnection { expected, got } => {
                write!(f, "consent prompt for {got} routed to {expected}")
            }
            PromptEmitError::ConnectionDead => {
                write!(f, "connection dead or torn down; prompt not announced")
            }
            PromptEmitError::UnknownConsentObject { wire_id } => {
                write!(f, "no consent object with wire id {wire_id}")
            }
            PromptEmitError::Transport(e) => write!(f, "transport failure: {e}"),
        }
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

/// The flight recorder's fixed cause taxonomy for an undelivered
/// resolution ([`Event::PetitionUndelivered`]). A class label, never the
/// `Display` text: the log's cause vocabulary stays closed and free of
/// anything a peer could shape, exactly as the handshake taxonomy does.
fn undelivered_reason_class(err: &DeliveryError) -> &'static str {
    match err {
        DeliveryError::WrongConnection { .. } => recorder::UNDELIVERED_WRONG_CONNECTION,
        DeliveryError::ConnectionDead => recorder::UNDELIVERED_CONNECTION_DEAD,
        DeliveryError::UnknownGrantObject { .. } => recorder::UNDELIVERED_UNKNOWN_GRANT,
        DeliveryError::AlreadyResolved { .. } => recorder::UNDELIVERED_ALREADY_RESOLVED,
        DeliveryError::Insert(_) => recorder::UNDELIVERED_INSERT_FAILED,
        DeliveryError::Transport(_) => recorder::UNDELIVERED_TRANSPORT,
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
    /// The launch facet (see [`ObjectKind::View`]), minted **on** the
    /// grant by `get_launcher` rather than co-minted by `request_grant`,
    /// whose five `new_id` arguments are frozen forever. Inert on exactly
    /// the same terms as the co-minted three: it carries its grant's wire
    /// id and nothing else, and the chokepoint judges every use.
    Launcher { grant: u32 },
    /// The focus facet (see [`ObjectKind::Launcher`]), minted on the grant
    /// by `get_layout_focus`.
    LayoutFocus { grant: u32 },
    /// The arrangement facet (see [`ObjectKind::Launcher`]), minted on the
    /// grant by `get_layout_arrange`.
    ///
    /// A **separate kind** from [`ObjectKind::LayoutFocus`] rather than one
    /// `Layout { grant, verb }`, because a facet interface declares exactly
    /// one verb and that is what generates the single-site authority check.
    /// One kind carrying a verb field would put the verb in the object
    /// table, where a later edit could set it from something other than
    /// which mint created the object.
    LayoutArrange { grant: u32 },
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
    /// The core-global realm registry (P1.5.1): the single source of realm
    /// existence, consulted by petition admission for its `unavailable`
    /// judgement. Shared and immutable per dispatch turn -- realm state
    /// changes belong to the spawn manager (P1.5.2/P1.5.3), never to a
    /// petitioner's connection.
    pub realms: &'a RealmRegistry,
    /// The core-global grant table.
    pub grants: &'a mut GrantTable,
    /// The dispatch turn's injected instant: the server never reads a
    /// clock, so one consistent `now` governs each request's whole
    /// decision -- handshake, petition, and enforcement alike.
    pub now: Instant,
    /// **This named realm's** latest completed view; `None` when that realm
    /// has composited nothing (or has no scene at all).
    ///
    /// A function of the realm id rather than a value since WS-E.1.3 (issue
    /// #209), and that is a **confidentiality** property rather than a
    /// rendering nicety. It used to be one frame for the whole session --
    /// there was one scene, holding at most one committed surface, so this
    /// was whichever realm painted last, and an `observe` grant over realm A
    /// returned realm B's pixels the instant B committed. Nothing in that
    /// code was wrong (there was one realm) and nothing in it prevented the
    /// leak either. There is now no "the view" to hand the chokepoint by
    /// mistake: a frame can only be obtained by naming a realm.
    ///
    /// Never read directly by a use. [`Self::realm_is_live`] gates it, and
    /// [`PrincipalServer::serve_facet_use`] is the one place that pairs the
    /// two -- so what reaches the chokepoint as `UseEnv::realm_view` is
    /// always "the view of the realm *this grant* names", which is the
    /// thing `no_surface` is a statement about.
    ///
    /// A callback rather than a map for the same reason
    /// [`Self::realm_is_live`] is one: the dispatch path allocates nothing
    /// per message, and the embedder answers straight out of the per-realm
    /// cache it already holds.
    pub realm_view: &'a dyn Fn(&RealmId) -> Option<RealmViewFrame<'a>>,
    /// **Does this named realm have a live view?**
    ///
    /// `RealmLifecycle::view_is_live` for one realm, asked by id, and the
    /// distinction is authority-relevant rather than cosmetic: with several
    /// realms attached, "some realm is live" is *fail-open* -- a grant over a
    /// dead realm would pass a liveness check a living sibling satisfied, and
    /// then go on to be served a frame.
    ///
    /// **Whose frame is no longer this gate's to worry about, and that is a
    /// weakening of the failure, not of the gate.** Since WS-E.1.3 the frame
    /// beside it is [`Self::realm_view`], resolved from the same realm id, and
    /// the embedder prunes a dead realm's cached view on the next composite
    /// (`session::refresh_view_cache`). So a leak through this hole would
    /// serve the dead realm's *own* last composition, and only inside the
    /// dispatch round its death landed in -- not the sibling's pixels it used
    /// to serve. That is one property milder and still exactly what
    /// `no_surface` forbids ("never a stale frame"). A dead realm is never
    /// photographable, whatever its siblings are doing, and this is the gate
    /// that says so.
    ///
    /// A callback rather than a set so the dispatch path allocates nothing
    /// per message: the embedder answers straight out of its live-realm map
    /// and the scene it already holds.
    pub realm_is_live: &'a dyn Fn(&RealmId) -> bool,
    /// **Which realm the human's own physical input currently follows**, or
    /// `None` when no realm is bound (`session::physical_seat_target`).
    ///
    /// Passed straight through to [`crate::enforcement::UseEnv::physical_realm`],
    /// which consults it for exactly one judgement — `preempted` for a
    /// **layout** request, the one attention-contending use that is not delivered
    /// into a realm and so steals from wherever the human already is.
    ///
    /// It used to be the write-side sibling of [`Self::realm_is_live`]:
    /// "does the session's one seat serve the realm this grant names", compared
    /// here and refused `internal` when it did not. WS-E.1.6 (issue #212) made
    /// seat delivery per realm, so an actuation always reaches the realm its
    /// grant names and that comparison has no question left to answer.
    ///
    /// A value rather than a callback because there is exactly one answer
    /// per dispatch turn — unlike liveness, which is a question *per realm*
    /// and so has to stay a function of the id.
    pub physical_realm: Option<&'a RealmId>,
    /// Physical-input presence **per realm**, fed at the input router's hook
    /// point (the chokepoint's `preempted` judgement).
    pub presence: &'a PhysicalPresenceMap,
    /// **The human's own attention signal** (WS-E.1.7), opened at the input
    /// router's innermost hook and passed straight through to
    /// [`crate::enforcement::UseEnv::attention`], which reads it for exactly
    /// one judgement — the exemption nested inside step 5c, for the two layout
    /// verbs only — and writes it at step-6 admission.
    ///
    /// A `&RefCell` rather than a borrow of the contents because the
    /// chokepoint claims the window; nothing else in the dispatch turn holds a
    /// borrow of it, since the hook that opens one runs in
    /// `session::route_physical_turn`, after connection dispatch has finished.
    pub attention: &'a std::cell::RefCell<crate::attention::AttentionSignal>,
    /// Where chokepoint-admitted, origin-tagged actuations go, **naming the
    /// realm the grant is over** (M1.1: that realm's seat state in the
    /// session's input router, toward its shim seat).
    pub actuations: &'a mut dyn FnMut(&RealmId, SeatInput),
    /// Where chokepoint-admitted **layout acts** go (WS-E.1.4: the
    /// session's output binding and the realm's `configure`).
    ///
    /// A second sink beside [`Self::actuations`] rather than a widened one:
    /// an actuation is an input event the delivery edge may legitimately
    /// drop, and a layout act is a change to the session's presentation
    /// that nothing downstream may reinterpret. Collapsing them would put
    /// the two under one drop policy.
    pub layout: &'a mut dyn FnMut(crate::enforcement::LayoutAct),
    /// **Where a chokepoint-admitted launch forks** (WS-E.1.1): given the
    /// realm the grant names, `session::launch_realm` mints an instance id
    /// and creates the process, or answers `capacity`/`internal`.
    ///
    /// A third sink rather than a widened `layout`, on exactly the grounds
    /// that kept `layout` out of `actuations`: this one *returns a value the
    /// client is told* (a realm id, on a reply-bearing request), where both
    /// others are one-way. Folding it in would put a terminal event's
    /// payload behind a sink that cannot fail.
    pub launch:
        &'a mut dyn FnMut(
            crate::enforcement::LaunchAsk<'_>,
        )
            -> Result<crate::realm::MintedRealmId, crate::enforcement::LaunchRefusal>,
    /// The core's single flight-recorder handle (P1.4.5,
    /// [`crate::recorder`]): every handshake outcome, petition lifecycle
    /// transition, consent transition, and enforcement decision this
    /// connection produces is recorded through *this* handle and no other
    /// write site.
    pub recorder: &'a mut Recorder,
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
    /// Launch facets ever minted on this connection, against
    /// [`MAX_LIVE_LAUNCHERS`] -- `get_launcher` is the one mint a bound
    /// principal can repeat on an object it already holds, so it needs
    /// the same bound `get_realm` has.
    launcher_count: usize,
    /// Layout facets ever minted on this connection (both mints together),
    /// against [`MAX_LIVE_LAYOUT_FACETS`] -- the same bound and the same
    /// reason [`Self::launcher_count`] has one.
    layout_facet_count: usize,
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
            launcher_count: 0,
            layout_facet_count: 0,
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
                    self.handle_hello(msg, ctx.verifier, ctx.recorder, send)
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
                        // vitrin_grant carries exactly three requests, all
                        // since="2" structural mints; every
                        // other opcode on it, and every opcode at all on
                        // vitrin_consent (which defines no requests at any
                        // version), is grammar (invalid_opcode), never an
                        // authority judgement.
                        ObjectKind::Grant(_) => match opcode {
                            grant::requests::GetLauncher::OPCODE => self.handle_get_launcher(msg),
                            grant::requests::GetLayoutFocus::OPCODE => {
                                self.handle_get_layout_focus(msg)
                            }
                            grant::requests::GetLayoutArrange::OPCODE => {
                                self.handle_get_layout_arrange(msg)
                            }
                            _ => {
                                Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                            }
                        },
                        ObjectKind::Consent => {
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
                                // control characters; every other Unicode
                                // Cc control -- C0 (U+0000..=U+001F), DEL
                                // (U+007F), and C1 (U+0080..=U+009F) -- is
                                // fatal invalid_argument, argument
                                // validation the generated decoder cannot
                                // see, exactly like the zero-verb rule. DEL
                                // is forbidden alongside C0/C1 (issue #82,
                                // and the IDL now names it): it is a
                                // destructive editing keystroke that no
                                // "deliver this Unicode string" ever means,
                                // and the shim refuses it too -- the core
                                // holds the line at the chokepoint rather
                                // than leaning on an untrusted shim to catch
                                // it.
                                if let Some(c) = req.text.chars().find(|&c| {
                                    matches!(c, '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}')
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
                        ObjectKind::Launcher { grant } => match opcode {
                            launcher::requests::Launch::OPCODE => {
                                let (_, _req) =
                                    launcher::requests::Launch::decode(&msg.bytes, msg.fd)
                                        .map_err(|source| PrincipalViolation::Malformed {
                                            object_id,
                                            source,
                                        })?;
                                self.serve_facet_use(object_id, grant, UseKind::Launch, ctx, send)
                            }
                            _ => {
                                Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                            }
                        },
                        // Both layout facets funnel through the same
                        // `serve_facet_use` as every other use. `focus`
                        // takes no arguments at all -- the realm comes from
                        // the grant row, never the wire -- so decoding it
                        // is a pure grammar check.
                        ObjectKind::LayoutFocus { grant } => match opcode {
                            layout_focus::requests::Focus::OPCODE => {
                                let (_, _req) =
                                    layout_focus::requests::Focus::decode(&msg.bytes, msg.fd)
                                        .map_err(|source| PrincipalViolation::Malformed {
                                            object_id,
                                            source,
                                        })?;
                                self.serve_facet_use(
                                    object_id,
                                    grant,
                                    UseKind::LayoutFocus,
                                    ctx,
                                    send,
                                )
                            }
                            _ => {
                                Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into())
                            }
                        },
                        ObjectKind::LayoutArrange { grant } => match opcode {
                            layout_arrange::requests::SetFullscreen::OPCODE => {
                                // An out-of-range `mode` is a fatal
                                // `invalid_argument` from the generated
                                // decoder (plain enums decode by whole-value
                                // membership), never a recoverable refusal:
                                // it is grammar the client could have known,
                                // which is the error razor's own test.
                                let (_, req) = layout_arrange::requests::SetFullscreen::decode(
                                    &msg.bytes, msg.fd,
                                )
                                .map_err(|source| PrincipalViolation::Malformed {
                                    object_id,
                                    source,
                                })?;
                                let mode = match req.mode {
                                    layout_arrange::Mode::Fullscreen => LayoutMode::Fullscreen,
                                    layout_arrange::Mode::Windowed => LayoutMode::Windowed,
                                };
                                self.serve_facet_use(
                                    object_id,
                                    grant,
                                    UseKind::LayoutArrange(mode),
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
    ///
    /// It is also the flight recorder's single `use_decision` site
    /// (P1.4.5): the chokepoint's returned [`UseOutcome`] carries every
    /// fact the entry states -- allowed or refused, the refusal code and
    /// whether it was voiced, the grant row, the delivered frame's B1
    /// observation digest, and whether a `once` rung was spent -- so the
    /// recorder observes enforcement from outside it and
    /// [`enforcement`](crate::enforcement) needs no recorder call at all.
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
        let verb = kind.verb();
        // Summarized before the kind moves into the request: an entry that
        // named only the verb could not tell a move from a button press
        // from a scroll, nor say how much was typed. Text is summarized by
        // shape and digest only -- never verbatim (see [`crate::recorder`]
        // on why the log must not become a keylogger).
        let detail = ActuationDetail::of(&kind);
        // Read before `kind` moves into the request below; see
        // `grant_realm`, which is resolved owned only for the uses that
        // need it. **Asked of the kind rather than re-derived here**
        // (WS-E.1.1): this was a duplicate `matches!` listing the four
        // attention-contending uses, and a launch -- which also needs the
        // grant's realm, as its template -- would have been handed `None`
        // and refused `internal` while the predicate a reader would check
        // said otherwise.
        let names_a_realm = kind.names_a_realm();
        let grant_row = self.grant_row_id(grant_wire_id);
        let request = UseRequest {
            facet_id,
            grant_wire_id,
            grant_row,
            principal: &identity,
            kind,
        };
        // **Whose realm this use is about**, resolved once and in one place:
        // the row names the realm, the embedder knows which realms are live,
        // and the chokepoint is handed the answers rather than any of the
        // machinery.
        //
        // - **Liveness and the frame** (`realm_is_live`, and `realm_view`).
        //   With one realm this could not differ from "is anything live"; with
        //   several, judging liveness against *any* realm is fail-open -- a
        //   grant over a dead realm would clear the `no_surface` gate on a
        //   living sibling's account and then capture the scene that sibling
        //   committed into. Since WS-E.1.3 the **frame** is resolved by realm
        //   here too, on the same line: a live realm's grant used to be
        //   handed the session's one view, so a capture over realm A could
        //   carry live sibling B's pixels. Both halves of "the view of the
        //   realm this grant names" are now resolved together, from the same
        //   `realm`, or not at all.
        // - **The realm's NAME**, which since WS-E.1.6 is what *addresses* an
        //   admitted actuation as well as a layout act. The write-side
        //   question used to be a comparison -- "does the session's one seat
        //   serve this grant's realm", refused `internal` when it did not --
        //   and it is gone with the one-target placeholder it defended: the
        //   name travels with the event instead, so an actuation reaches the
        //   realm its grant is over and nothing has to be compared.
        //
        // Both are the closed answer when the row is gone -- fail closed, and
        // unreachable anyway: a missing row is refused `not_granted` at step
        // 3, before either is consulted.
        //
        // Not an authority judgement (this function makes none): it resolves
        // *environment* facts the same way `presence` is resolved outside
        // the chain. The chokepoint still decides, in its own order --
        // `not_granted` for a row that is missing, then expiry and
        // revocation, and only then the use-context gates. A row that fails
        // an earlier step never reaches this frame at all.
        //
        // Neither a layout request nor an actuator request carries a realm on
        // the wire -- precisely so a holder can only ever act on the realm the
        // human saw named on its consent prompt. Resolving the name here, from
        // the same row, on the same line as everything else that is "about the
        // realm this grant names", is what keeps a second site from
        // disagreeing.
        //
        // Owned, and **only for a use that names a realm**
        // ([`UseKind::names_a_realm`]): the two actuators and the two layout
        // requests, which either name a realm to the embedder or are judged
        // against a realm's physical presence, plus a **launch**, whose
        // template is the realm the grant is over. The chokepoint takes
        // `&mut GrantTable`, so a borrow of the row's realm cannot survive
        // into the call. That is one realm-name clone -- a `String` the wire
        // caps at 64 bytes (`vitrin_realm`) -- per such request, and it is
        // the cost of the event carrying its own destination; a **capture**
        // still allocates nothing here, which is the high-rate path this
        // rule was written for. Read before `kind` moves into the request.
        let grant_realm = if names_a_realm {
            grant_row.and_then(|row| ctx.grants.realm_of(row)).cloned()
        } else {
            None
        };
        let realm_view = grant_row
            .and_then(|row| ctx.grants.realm_of(row))
            // The liveness gate and the selection, side by side: a dead realm
            // is never photographable whatever its siblings are doing, and a
            // live one is photographed through its own view and no other.
            .filter(|realm| (ctx.realm_is_live)(realm))
            .and_then(|realm| (ctx.realm_view)(realm));
        let env = UseEnv {
            realm_view: realm_view.as_ref(),
            presence: ctx.presence,
            attention: ctx.attention,
            physical_realm: ctx.physical_realm,
            actuations: &mut *ctx.actuations,
            grant_realm: grant_realm.as_ref(),
            layout: &mut *ctx.layout,
            launch: &mut *ctx.launch,
        };
        let outcome = self
            .chokepoint
            .enforce_use(request, ctx.grants, ctx.petitions, env, ctx.now, send)
            .map_err(PrincipalFault::Transport)?;
        ctx.recorder.record(Event::UseDecision {
            connection: self.connection,
            facet_wire_id: facet_id,
            grant_wire_id,
            verb,
            grant_row,
            detail,
            outcome: &outcome,
        });
        // The `once` rung's active-to-spent transition, recorded from the
        // same outcome so the grant table never learns a recorder exists.
        // (A transport death inside `enforce_use` returns above, so a use
        // whose frame never reached the wire records nothing -- the
        // connection is already dying and its teardown entry follows.)
        if let UseOutcome::Admitted {
            grant,
            spent_once: true,
            ..
        } = outcome
        {
            ctx.recorder.record(Event::GrantSpent {
                connection: self.connection,
                grant_id: grant,
            });
        }
        // The human's attention window, if this admission spent it (WS-E.1.7):
        // read off the same outcome, so the chokepoint still never learns a
        // recorder exists and no third code path appears. Journaling *who*
        // took the press is the one narrowing available against a session-wide
        // window (issue #232 decision 10): the core cannot tell which of two
        // layout holders the human meant, so what it can do is say afterwards
        // which one actually took it.
        if let UseOutcome::Admitted {
            grant,
            attention_claimed: true,
            ..
        } = outcome
        {
            ctx.recorder.record(Event::AttentionClaimed {
                connection: self.connection,
                principal: &identity,
                grant_id: grant,
                verb,
            });
        }
        Ok(())
    }

    /// `hello`: the IDL's fixed check order -- grammar (decode), version,
    /// `principal` new_id allocation, then credential verification -- so
    /// `version_unsupported` and `invalid_object` reveal nothing about the
    /// credential, and refused verification reveals nothing beyond the
    /// uniform `auth_failed`.
    ///
    /// Both handshake outcomes are recorded here, at the single site that
    /// decides them (P1.4.5), honoring the secrecy contract: the
    /// verifier-canonical identity on a bind; on a refusal a fixed cause
    /// class plus the *claimed* identity and scheme (client-controlled,
    /// exactly escaped, never trusted) and the credential's **length**
    /// only. The credential bytes are not passed to the recorder at all.
    fn handle_hello<F>(
        &mut self,
        msg: Message,
        verifier: &dyn Verifier,
        recorder: &mut Recorder,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        let object_id = msg.header.object_id;
        let (_, hello) = handshake::requests::Hello::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        if hello.version != PROTOCOL_VERSION {
            // This core accepts **exactly** its maximum
            // ([`PROTOCOL_VERSION`], 2 today) and refuses every other
            // integer -- a later version, the never-issued 0, and also
            // version 1, which is *below* the maximum.
            //
            // Refusing 1 is a deliberate, disclosed gap rather than the
            // protocol's rule: conventions section 7.3 says a server
            // whose maximum is N implements every version from 1 to N,
            // and its "Implementation status" note records that the
            // shipped core does not yet do so. Serving 1 and 2
            // concurrently needs a per-connection version matrix (which
            // messages each connection may send, which events it may
            // receive); that is P2.1.2's deliverable, and inventing half
            // of it here would leave a version-1 connection able to reach
            // `since="2"` opcodes. Refusing is the honest fail-closed
            // answer until the matrix lands; nothing outside this repo
            // speaks version 1, so it costs no deployed client.
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
        let (cause, cause_class) = match verifier.verify(&presented) {
            VerifyOutcome::Bound(bound) => {
                let event = principal::events::Bound {
                    identity: bound.identity.as_str().to_owned(),
                };
                send(&event.encode(hello.principal), None)?;
                self.principal_id = Some(hello.principal);
                self.phase = Phase::Bound;
                // Recorded only once the connection is *actually* bound.
                // Unlike a granted resolution -- where authority is minted
                // before the send, so the entry must precede it -- nothing
                // exists here until the phase flips: a `bound` event that
                // failed to send leaves no principal and no authority, so
                // an entry claiming one would be a lie the teardown entry
                // (`identity: null`) would then contradict.
                recorder.record(Event::HandshakeBound {
                    connection: self.connection,
                    peer: self.peer,
                    identity: &bound.identity,
                    credential_type: &hello.credential_type,
                    credential_bytes: hello.credential.len(),
                });
                self.identity = Some(bound.identity);
                return Ok(());
            }
            // Both non-Bound outcomes are wire-uniform auth_failed; they
            // differ only in the logged cause (rejected client vs. broken
            // verifier infrastructure).
            VerifyOutcome::Rejected(cause) => (cause.to_string(), auth_cause_class(&cause)),
            VerifyOutcome::Unavailable(detail) => (
                format!("verifier unavailable: {detail}"),
                VERIFIER_UNAVAILABLE_CLASS,
            ),
        };
        recorder.record(Event::HandshakeRefused {
            connection: self.connection,
            peer: self.peer,
            cause_class,
            claimed_identity: &hello.identity,
            credential_type: &hello.credential_type,
            credential_bytes: hello.credential.len(),
        });
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

    /// `vitrin_grant.get_launcher` (since version 2): a structural mint,
    /// exactly like `get_realm` and like `request_grant`'s co-minted
    /// facets -- no reply, no refusal, no wire acknowledgement.
    ///
    /// **The mint is always legal; the use is what refuses.** This
    /// request is defined for every grant whatever verbs it holds and
    /// whether or not it has resolved, because mint-freely-and-check-at-
    /// use is the pattern the co-minted facets already establish, and
    /// because refusing at mint time would turn the mint into an
    /// authority oracle -- it would tell the petitioner something about
    /// its own pending petition that only `resolved` may say. The facet
    /// is born inert and every `launch` is judged at the single
    /// enforcement chokepoint, which refuses `not_granted` unless the
    /// grant this facet was minted on resolved `granted` carrying
    /// `realm_launch`.
    ///
    /// Only the object-graph rules can fail here: the live-object cap
    /// ([`MAX_LIVE_LAUNCHERS`]) and the watermark rule, both fatal.
    fn handle_get_launcher(&mut self, msg: Message) -> Result<(), PrincipalFault> {
        // The request's target object *is* the grant (dispatch resolved
        // it to an `ObjectKind::Grant` before calling), so the facet's
        // chokepoint key is the frame's own object id -- no second
        // lookup, and no way for the two to disagree.
        let object_id = msg.header.object_id;
        let (_, req) = grant::requests::GetLauncher::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        // The cap precedes the mint (the `get_realm` precedent): no
        // version defines destructors, so every launcher is a permanent
        // per-connection allocation.
        if self.launcher_count >= MAX_LIVE_LAUNCHERS {
            return Err(PrincipalViolation::ResourceExhausted("live-launcher cap exceeded").into());
        }
        self.allocate_id(req.launcher)?;
        self.objects
            .insert(req.launcher, ObjectKind::Launcher { grant: object_id });
        self.launcher_count += 1;
        Ok(())
    }

    /// `vitrin_grant.get_layout_focus` (since version 2): a structural
    /// mint, on exactly [`handle_get_launcher`]'s terms -- always legal,
    /// born inert, duplicates permitted, only the object-graph rules can
    /// fail. Unlike the launcher's, the *use* this mints can succeed:
    /// `layout_focus` is in
    /// [`SERVED_VERB_BITS`](crate::grants::SERVED_VERB_BITS).
    ///
    /// [`handle_get_launcher`]: PrincipalServer::handle_get_launcher
    fn handle_get_layout_focus(&mut self, msg: Message) -> Result<(), PrincipalFault> {
        let object_id = msg.header.object_id;
        let (_, req) = grant::requests::GetLayoutFocus::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        if self.layout_facet_count >= MAX_LIVE_LAYOUT_FACETS {
            return Err(
                PrincipalViolation::ResourceExhausted("live-layout-facet cap exceeded").into(),
            );
        }
        self.allocate_id(req.layout_focus)?;
        self.objects.insert(
            req.layout_focus,
            ObjectKind::LayoutFocus { grant: object_id },
        );
        self.layout_facet_count += 1;
        Ok(())
    }

    /// `vitrin_grant.get_layout_arrange` (since version 2): the sibling of
    /// [`handle_get_layout_focus`], and a separate request for the reason
    /// the IDL gives -- one facet interface declares one verb, and the two
    /// layout verbs must stay independently attenuable.
    ///
    /// Both mints share [`MAX_LIVE_LAYOUT_FACETS`] rather than counting
    /// separately: the cap exists to bound permanent per-connection
    /// allocations, and two half-caps would let a connection hold twice as
    /// many objects for the same reason.
    ///
    /// [`handle_get_layout_focus`]: PrincipalServer::handle_get_layout_focus
    fn handle_get_layout_arrange(&mut self, msg: Message) -> Result<(), PrincipalFault> {
        let object_id = msg.header.object_id;
        let (_, req) = grant::requests::GetLayoutArrange::decode(&msg.bytes, msg.fd)
            .map_err(|source| PrincipalViolation::Malformed { object_id, source })?;
        if self.layout_facet_count >= MAX_LIVE_LAYOUT_FACETS {
            return Err(
                PrincipalViolation::ResourceExhausted("live-layout-facet cap exceeded").into(),
            );
        }
        self.allocate_id(req.layout_arrange)?;
        self.objects.insert(
            req.layout_arrange,
            ObjectKind::LayoutArrange { grant: object_id },
        );
        self.layout_facet_count += 1;
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
        let petition = PetitionRequest {
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
        };
        // Recorded *before* the policy decision, so a petition refused
        // `busy`/`unsupported`/`unavailable` still leaves a record of what
        // it asked for -- the requested authority is only knowable here.
        ctx.recorder.record(Event::PetitionRequested {
            connection: petition.connection,
            identity: &petition.identity,
            realm_name: &petition.realm_name,
            grant_wire_id: petition.grant_wire_id,
            consent_wire_id: petition.consent_wire_id,
            resource: &petition.resource,
            requested: RequestedAuthority {
                verbs: petition.verbs,
                persistence: petition.persistence,
                expiry_ms: petition.expiry_ms,
                max_event_rate: petition.max_event_rate,
                flags: petition.flags,
            },
        });
        // D-018(4)'s single-holder question, asked of the grant table here
        // rather than inside admission: the petition registry holds no
        // grant state, and giving it the table would give the core a second
        // view of authority beside the chokepoint's.
        let layout_arrange_held = ctx.grants.any_live_holder_of(Verb::LAYOUT_ARRANGE, now);
        let admission = ctx
            .petitions
            .admit(petition, now, ctx.realms, layout_arrange_held);
        match admission {
            Admission::Pending { petition } => {
                // The prompt lifecycle began: it is waiting on the consent
                // surface (or, in this build, the timeout).
                ctx.recorder.record(Event::ConsentTransition {
                    connection: self.connection,
                    consent_wire_id: req.consent,
                    state: ConsentState::Queued,
                    petition: Some(petition),
                });
                let queued = consent::events::State {
                    state: ConsentState::Queued,
                };
                send(&queued.encode(req.consent), None)?;
                Ok(())
            }
            Admission::Resolved(resolution) => {
                self.deliver_resolution(resolution, ctx.grants, ctx.recorder, now, send)
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

    /// Announce that this petition's consent prompt is now on screen:
    /// `vitrin_consent.state(shown)` (P1.7.2).
    ///
    /// Called by the embedder immediately after
    /// [`crate::consent::grab::ConsentGrab::raise`] returns the
    /// [`PromptRoute`] naming this connection -- that call is what put the
    /// pixels up, set the registry's `prompt_shown` flag (making the
    /// chokepoint's `consent_held` refusal true), seized the input grab,
    /// and wrote the `consent_transition{shown}` entry to the flight
    /// recorder. This method is the last part of the same moment, and the
    /// only one that has to travel the wire.
    ///
    /// # Why the recorder entry is not written here
    ///
    /// It used to be, and that inverted the discipline
    /// [`Self::deliver_resolution`] argues for and this comment used to
    /// restate: the log's subject is the moment core state changed, and a
    /// failure to *tell the client* must not erase the fact that a human
    /// was asked. All three guards below return before any recording could
    /// happen, and one of them -- `ConnectionDead` -- is a documented,
    /// expected race (the petitioner dies as its prompt goes up), not a
    /// bug. On that path the card was on screen, the human's input was
    /// grabbed and the chokepoint was refusing `consent_held`, while the
    /// log contained nothing to say a prompt had ever been shown. Recording
    /// in `raise`, where the state changes, makes that unrepresentable.
    ///
    /// # The ordering guarantee, and why it holds structurally
    ///
    /// `docs/protocol/05-vitrin_consent.md`: "all of them are delivered
    /// **before** that petition's `resolved`". Nothing here checks that.
    /// It holds because `raise` refuses a petition that is not pending, and
    /// a petition stops being pending in exactly the step that produces its
    /// [`Resolution`] ([`PetitionRegistry::finish`]) -- so a `shown` event
    /// can only be produced while the terminal has not been decided yet,
    /// let alone sent. The closing `state(closed)` is likewise emitted
    /// inside [`Self::deliver_resolution_inner`], on the line before the
    /// `resolved` send. The ordering is a property of when the events can
    /// be *constructed*, not of the order someone remembered to write them
    /// in.
    ///
    pub fn emit_consent_shown<F>(
        &mut self,
        route: PromptRoute,
        send: &mut F,
    ) -> Result<(), PromptEmitError>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        // Addressing before the phase gate, the `deliver_resolution`
        // precedent: a misrouted prompt must surface as the routing bug it
        // is rather than vanish as this connection's dead-drop.
        if route.connection != self.connection {
            return Err(PromptEmitError::WrongConnection {
                expected: self.connection,
                got: route.connection,
            });
        }
        if self.phase != Phase::Bound {
            return Err(PromptEmitError::ConnectionDead);
        }
        if !matches!(
            self.objects.get(&route.consent_wire_id),
            Some(ObjectKind::Consent)
        ) {
            return Err(PromptEmitError::UnknownConsentObject {
                wire_id: route.consent_wire_id,
            });
        }
        let shown = consent::events::State {
            state: ConsentState::Shown,
        };
        send(&shown.encode(route.consent_wire_id), None).map_err(PromptEmitError::Transport)
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
    ///
    /// **Recording order (P1.4.5, decided here).** Both entries -- the
    /// closing consent transition and the resolution itself -- are recorded
    /// after the handle flip and row mint but *before* either send. The
    /// recorder's subject is the moment authority changes, not the moment
    /// the client learns of it: written after the sends, a transport
    /// failure between the mint and the wire would leave a live grant row
    /// with no line naming it -- exactly the unreconstructable state the
    /// recorder exists to prevent. On a dying connection the log is
    /// therefore a superset of what reached the wire, and it preserves the
    /// wire's own order (`closed`, then `resolved`).
    ///
    /// **A refused delivery is recorded too, from this one funnel.** The
    /// resolution has already been consumed from the pending registry by
    /// the time it arrives here, so every early return below *destroys* a
    /// decision: the petition is gone, the handle never resolves, and
    /// nothing will ever retry. A human's yes or no would then exist
    /// nowhere. Wrapping the whole body and recording
    /// [`Event::PetitionUndelivered`] on any `Err` covers all five refusal
    /// paths -- and any added later -- structurally, rather than by
    /// remembering to add a call beside each `return`.
    pub fn deliver_resolution<F>(
        &mut self,
        resolution: Resolution,
        grants: &mut GrantTable,
        recorder: &mut Recorder,
        now: Instant,
        send: &mut F,
    ) -> Result<(), DeliveryError>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        // Captured before the body consumes the resolution: on failure the
        // decision must still be nameable.
        let connection = resolution.connection;
        let grant_wire_id = resolution.grant_wire_id;
        let (outcome, effective, issuer) = match &resolution.verdict {
            Verdict::Granted { grant: approved } => (
                Outcome::Granted,
                Some(approved.effective),
                Some(approved.issuer),
            ),
            Verdict::Declined { outcome } => (*outcome, None, None),
        };
        let result = self.deliver_resolution_inner(resolution, grants, recorder, now, send);
        if let Err(err) = &result {
            recorder.record(Event::PetitionUndelivered {
                connection,
                grant_wire_id,
                outcome,
                effective,
                issuer,
                reason: undelivered_reason_class(err),
            });
        }
        result
    }

    /// Emit `vitrin_principal.attention` on this connection (WS-E.1.7, issue
    /// #232): the human pressed the core's attention key.
    ///
    /// **It carries nothing and confers nothing.** The event is argument-free
    /// forever (IDL `vitrin_principal.attention`) and this function makes no
    /// authority judgement whatsoever: *whether* a connection is told is the
    /// caller's, decided by the grant table's delivery filter
    /// ([`GrantTable::holds_verb`]), and *whether anything may then happen*
    /// stays the chokepoint's. Adding a check here would be the second
    /// enforcement site this crate does not have.
    ///
    /// `Ok(false)` for a connection that is not bound — nothing was sent, and
    /// that is not an error: a connection mid-handshake holds no grant, so it
    /// could not have been selected anyway. `Err` is transport death, which
    /// the caller logs; a dropped attention event costs the client one window
    /// it can neither observe nor be harmed by missing.
    pub fn deliver_attention<F>(&mut self, send: &mut F) -> Result<bool, TransportError>
    where
        F: FnMut(&[u8], Option<BorrowedFd<'_>>) -> Result<(), TransportError>,
    {
        if self.phase != Phase::Bound {
            return Ok(false);
        }
        let Some(principal_id) = self.principal_id else {
            return Ok(false);
        };
        send(&principal::events::Attention {}.encode(principal_id), None)?;
        Ok(true)
    }

    /// The delivery body proper -- see [`Self::deliver_resolution`], which
    /// wraps it to record decisions this refuses.
    fn deliver_resolution_inner<F>(
        &mut self,
        resolution: Resolution,
        grants: &mut GrantTable,
        recorder: &mut Recorder,
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
        // Recorded here, at the instant authority changed and before any
        // send can fail (doc comment above). The prompt is down because the
        // petition left the pending table, so no petition id remains to
        // name.
        if resolution.emit_closed {
            recorder.record(Event::ConsentTransition {
                connection: self.connection,
                consent_wire_id: resolution.consent_wire_id,
                state: ConsentState::Closed,
                petition: None,
            });
        }
        let (outcome, effective, issuer) = match &resolution.verdict {
            Verdict::Granted { grant: approved } => (
                Outcome::Granted,
                Some(approved.effective),
                Some(approved.issuer),
            ),
            Verdict::Declined { outcome } => (*outcome, None, None),
        };
        recorder.record(Event::PetitionResolved {
            connection: self.connection,
            grant_wire_id: resolution.grant_wire_id,
            outcome,
            effective,
            grant_id: row,
            issuer,
        });
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
    ///
    /// Recorded unconditionally (P1.4.5), unlike the `tracing` line beside
    /// it: a connection that withdrew nothing and held nothing still ends a
    /// session, and a reconstruction that cannot see a principal leave is
    /// incomplete.
    pub fn teardown(
        &mut self,
        petitions: &mut PetitionRegistry,
        grants: &mut GrantTable,
        recorder: &mut Recorder,
    ) {
        let withdrawn = petitions.withdraw_connection(self.connection);
        let mut removed = 0usize;
        for kind in self.objects.values() {
            if let ObjectKind::Grant(GrantHandleState::Resolved { row: Some(id) }) = kind {
                if grants.remove(*id) {
                    removed += 1;
                    // Named per row, not merely counted: teardown is the
                    // most common way a version-1 grant dies, and the
                    // revocation and expiry paths already name their ids
                    // so an E3.4 replay can apply the transition. A count
                    // alone says how many authorities died, never which.
                    recorder.record(Event::GrantRemoved {
                        connection: self.connection,
                        grant_id: *id,
                    });
                }
            }
        }
        recorder.record(Event::ConnectionTeardown {
            connection: self.connection,
            identity: self.identity.as_ref(),
            withdrawn_petitions: withdrawn,
            removed_grants: removed,
        });
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
pub(crate) mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::os::fd::AsFd;
    use std::path::PathBuf;

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
    use crate::petitions::PetitionId;
    use crate::petitions::{ConsentPolicy, PetitionConfig, ScriptedDecision, ScriptedError};
    use vitrin_protocol::generated::vitrin_actuator_pointer::{Axis, ButtonState};

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
        /// The rig's realm registry (P1.5.1): one configured realm under
        /// the well-known default id, which is what `bind_with_realm`
        /// addresses. Tests that need another shape replace it.
        realms: RealmRegistry,
        grants: GrantTable,
        now: Instant,
        /// **Each realm's** latest completed view, `(rgba, width, height)`;
        /// an absent entry models a realm with no live surface -- the
        /// chokepoint's use-time `no_surface` judgement, which is
        /// deliberately NOT the same thing as a realm being vacant at
        /// petition time (that is [`Shared::realms`], and see
        /// [`crate::realm`]'s vacancy decision).
        ///
        /// A map rather than one frame since WS-E.1.3 (issue #209), for the
        /// same reason `ServerCtx::realm_view` became a function of the realm
        /// id: with one frame for the rig, a test could not tell a capture
        /// that served the granted realm's pixels from one that served a
        /// sibling's, because there were no sibling pixels to serve. See
        /// [`Shared::set_view`].
        views: std::collections::BTreeMap<RealmId, (Vec<u8>, u32, u32)>,
        /// Which realms have a **live view** right now -- the rig's stand-in
        /// for `RealmLifecycle::view_is_live`, asked per realm id exactly as
        /// the runtime asks it (`ServerCtx::realm_is_live`).
        ///
        /// Distinct from [`Shared::realms`] on purpose, and the pair is what
        /// makes a dead-realm test possible at all: the registry answers
        /// petition-time *vacancy*, this answers use-time *liveness*, and a
        /// realm can be in the registry, still `Running` there, and have no
        /// view. Removing an id here is this rig's "that realm's shim died".
        live_realms: std::collections::BTreeSet<RealmId>,
        /// Which realm the **human's own physical input** follows right now
        /// -- the rig's stand-in for `session::physical_seat_target`, which
        /// follows the output binding. Read by exactly one judgement,
        /// `preempted` for a layout request (WS-E.1.6).
        ///
        /// It used to be "which realm the seat delivers into", compared
        /// against the grant's realm and refused `internal` when they
        /// differed. Seat delivery is per realm now, so a test that wants a
        /// cross-realm actuation simply performs one and reads
        /// [`Shared::actuations`], which names each event's realm.
        physical_realm: Option<RealmId>,
        presence: PhysicalPresenceMap,
        /// The human's **attention signal** (WS-E.1.7), the rig's stand-in for
        /// `session::Kernel::attention`. A test opens a window on it with
        /// [`AttentionSignal::open`] naming the principals the `attention`
        /// event reached — which is what `session::open_attention_window`
        /// resolves at press time — and then reads it back to prove the window
        /// was, or was not, claimed.
        attention: std::cell::RefCell<crate::attention::AttentionSignal>,
        /// Every actuation the chokepoint admitted, **with the realm it was
        /// addressed to** -- this rig's stand-in for `session::route_seat`.
        /// The realm is the assertion WS-E.1.6 made possible: before it,
        /// every admitted actuation went to the session's one target and
        /// there was nothing to record.
        actuations: Vec<(RealmId, SeatInput)>,
        /// Every **layout act** the chokepoint admitted, in order — this
        /// rig's stand-in for `session::apply_layout`. A test asserts on
        /// this to prove a `focus` or a `set_fullscreen` really reached the
        /// embedder rather than being swallowed on the way.
        layout: Vec<crate::enforcement::LayoutAct>,
        /// Every **launch** the chokepoint admitted, in order: the template
        /// it named, who asked, and the grant row it was judged against --
        /// this rig's stand-in for `session::launch_realm`, minus the fork.
        ///
        /// The rig deliberately does *not* create a process: what these
        /// tests are about is whether a launch reached the embedder at all,
        /// with the right template and the right asker. That the fork then
        /// happens is `tests/integration/test_launch.py`'s subject, against
        /// the shipped binary.
        launches: Vec<(RealmId, PrincipalIdentity, crate::grants::GrantId)>,
        /// What the launch sink answers. `None` mints an id from
        /// [`Shared::realms`], exactly as the runtime does; `Some(refusal)`
        /// forces the post-admission refusal a test wants to see voiced
        /// (`capacity`, or a spawn failure's `internal`) without needing a
        /// session at its realm cap.
        launch_answer: Option<crate::enforcement::LaunchRefusal>,
        /// The rig's single flight-recorder handle (P1.4.5): every test in
        /// this module drives the real recorder, so the emission wiring is
        /// exercised by the whole suite and not only by the tests that
        /// read the log back.
        recorder: Recorder,
        /// Where [`Shared::recorder`] writes; `read_log` reads it.
        log_path: PathBuf,
    }

    impl Shared {
        fn new(policy: ConsentPolicy) -> Self {
            let (recorder, log_path) = crate::recorder::tests::scratch_recorder("principal");
            Self {
                petitions: PetitionRegistry::new(policy, PetitionConfig::default()),
                realms: crate::realm::tests::registry_with(&[crate::realm::WELL_KNOWN_REALM_ID]),
                grants: GrantTable::new(),
                now: Instant::now(),
                views: [(
                    RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
                    (crate::test_pattern::render(VIEW_W, VIEW_H), VIEW_W, VIEW_H),
                )]
                .into_iter()
                .collect(),
                // Every realm the rig serves is live by default, so a test
                // that says nothing about liveness behaves as it did while
                // "the realm" was the only realm.
                live_realms: [RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)]
                    .into_iter()
                    .collect(),
                // The human is in the rig's one realm, so a test that says
                // nothing about routing behaves as it did while "the realm"
                // was the only realm.
                physical_realm: Some(RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)),
                presence: PhysicalPresenceMap::new(),
                attention: std::cell::RefCell::new(crate::attention::AttentionSignal::detached()),
                actuations: Vec::new(),
                layout: Vec::new(),
                launches: Vec::new(),
                launch_answer: None,
                recorder,
                log_path,
            }
        }

        /// Set (or clear) **one realm's** latest completed view.
        ///
        /// `None` is this rig's "that realm has composited nothing", which
        /// the chokepoint turns into `no_surface`. A helper rather than a
        /// public field write so every test that changes a view has to say
        /// *whose* view it changed — the whole point of the map.
        fn set_view(&mut self, realm: &RealmId, frame: Option<(Vec<u8>, u32, u32)>) {
            match frame {
                Some(frame) => {
                    self.views.insert(realm.clone(), frame);
                }
                None => {
                    self.views.remove(realm);
                }
            }
        }

        /// Every entry this rig has recorded so far, parsed (the envelope
        /// invariants are asserted by `read_log` itself).
        fn log(&self) -> Vec<crate::recorder::tests::Json> {
            crate::recorder::tests::read_log(&self.log_path)
        }
    }

    impl Drop for Shared {
        fn drop(&mut self) {
            crate::recorder::tests::cleanup(&self.log_path);
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
            realms,
            grants,
            now,
            views,
            live_realms,
            physical_realm,
            presence,
            attention,
            actuations,
            layout,
            launches,
            launch_answer,
            recorder,
            ..
        } = shared;
        let mut sink = |realm: &RealmId, input: SeatInput| actuations.push((realm.clone(), input));
        let mut layout_sink = |act: crate::enforcement::LayoutAct| layout.push(act);
        // The rig's launch sink: record the ask, then answer exactly as the
        // runtime would -- a registry-minted instance id, or the forced
        // post-admission refusal. Minting through `realms` rather than
        // fabricating a string is what keeps these tests honest about where
        // an instance id comes from; there is no other way to build one.
        let launch_realms = &*realms;
        let mut launch_sink = |ask: crate::enforcement::LaunchAsk<'_>| {
            launches.push((ask.template.clone(), ask.principal.clone(), ask.grant));
            match *launch_answer {
                Some(refusal) => Err(refusal),
                None => launch_realms
                    .mint_instance(ask.template)
                    .ok_or(crate::enforcement::LaunchRefusal::Internal),
            }
        };
        let realm_is_live = |realm: &RealmId| live_realms.contains(realm);
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            // Built inside the loop, unlike `realm_is_live`: the frame it
            // hands back borrows `views`, so `ServerCtx<'a>`'s lifetime is
            // pinned to this closure's — and a closure hoisted out of the
            // loop would pin `petitions`/`grants`/`recorder`'s `&mut`
            // reborrows to the whole function, which is one dispatch's worth
            // of borrow held across every dispatch. The runtime has the same
            // shape for the same reason (`session::dispatch_principal`
            // builds it per message).
            let realm_view = |realm: &RealmId| {
                views
                    .get(realm)
                    .map(|(rgba, width, height)| RealmViewFrame {
                        rgba: rgba.as_slice(),
                        width: *width,
                        height: *height,
                    })
            };
            let mut ctx = ServerCtx {
                verifier,
                petitions,
                realms,
                grants,
                attention,
                now: *now,
                // Resolved by realm id, exactly as `session::dispatch_principal`
                // resolves it from `Runtime::view_cache` (WS-E.1.3).
                realm_view: &realm_view,
                realm_is_live: &realm_is_live,
                physical_realm: physical_realm.as_ref(),
                presence,
                actuations: &mut sink,
                layout: &mut layout_sink,
                launch: &mut launch_sink,
                recorder,
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
                &mut shared.recorder,
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

    /// What one `capture_frame` produced at the enforcement chokepoint --
    /// **which terminal the client saw**, never what any state says.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum CaptureOutcome {
        /// A `frame_ready` arrived, carrying these dimensions.
        Frame { width: u32, height: u32 },
        /// A `refused` arrived with this code, and no `frame_ready`
        /// exists on the wire at all.
        Refused(Refusal),
    }

    /// Drive exactly one `vitrin_view.capture_frame` end to end --
    /// handshake, realm, petition, auto-approved grant, then the capture --
    /// against `view` as the realm's live view, and report the single
    /// terminal the client saw.
    ///
    /// Exists for [`crate::lifecycle`], whose acceptance criterion is that
    /// a capture after a shim's death yields **no frame at all**. Asserting
    /// "the delivered bytes differ from the last good ones" would be
    /// satisfied by a stale frame that happened to differ, so the assertion
    /// has to be about which terminal arrived -- and that means running a
    /// real chokepoint decision over a real socketpair rather than
    /// inspecting core state.
    ///
    /// Mints socketpairs and a memfd, so **the caller must already hold
    /// [`crate::capture::tests::fd_lock`]** -- it deliberately does not take
    /// it itself, because every caller is a test that is already counting
    /// descriptors under that lock and the mutex is not reentrant.
    pub(crate) fn capture_once(view: Option<crate::capture::RealmViewFrame<'_>>) -> CaptureOutcome {
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        shared.set_view(
            &RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
            view.map(|v| (v.rgba.to_vec(), v.width, v.height)),
        );

        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("dispatch the capture");

        let msg = client.recv_message().unwrap().unwrap();
        let outcome = match msg.header.object_id {
            // The view facet: the only object `frame_ready` addresses.
            6 => {
                let (_, ev) = view::events::FrameReady::decode(&msg.bytes, msg.fd)
                    .expect("a well-formed frame_ready");
                CaptureOutcome::Frame {
                    width: ev.width,
                    height: ev.height,
                }
            }
            // The co-minted grant handle: the only object `refused`
            // addresses.
            4 => {
                let (_, ev) = grant::events::Refused::decode(&msg.bytes, msg.fd)
                    .expect("a well-formed refused");
                CaptureOutcome::Refused(ev.code)
            }
            other => panic!("unexpected terminal on object {other}"),
        };

        // Exactly one terminal per `capture_frame` (IDL), and -- the whole
        // point of this helper -- when that terminal is a refusal there is
        // no `frame_ready` anywhere behind it. A stale frame would show up
        // here.
        let flags = rustix::fs::fcntl_getfl(client.as_fd()).unwrap();
        rustix::fs::fcntl_setfl(client.as_fd(), flags | rustix::fs::OFlags::NONBLOCK).unwrap();
        match client.recv_message() {
            Err(TransportError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("exactly one terminal per capture_frame; a second arrived: {other:?}"),
        }
        outcome
    }

    /// [`capture_once`]'s multi-grant sibling: drive one `capture_frame` on
    /// **this** view facet and report the single terminal, which arrives
    /// either on the facet (`frame_ready`) or on its co-minted grant handle
    /// (`refused`).
    ///
    /// Exists because `capture_once` builds its own one-realm rig and hard
    /// codes ids 6 and 4; a test about *two* realms needs two facets on one
    /// connection, so the ids have to be arguments.
    fn capture_facet(
        server: &mut PrincipalServer,
        core: &mut Connection,
        client: &mut Connection,
        verifier: &dyn Verifier,
        shared: &mut Shared,
        view_id: u32,
        grant_id: u32,
    ) -> CaptureOutcome {
        client
            .send_message(&view::requests::CaptureFrame {}.encode(view_id), None)
            .unwrap();
        process_n(server, core, verifier, shared, 1).expect("dispatch the capture");
        let msg = client.recv_message().unwrap().unwrap();
        match msg.header.object_id {
            id if id == view_id => {
                let (_, ev) = view::events::FrameReady::decode(&msg.bytes, msg.fd)
                    .expect("a well-formed frame_ready");
                CaptureOutcome::Frame {
                    width: ev.width,
                    height: ev.height,
                }
            }
            id if id == grant_id => {
                let (_, ev) = grant::events::Refused::decode(&msg.bytes, msg.fd)
                    .expect("a well-formed refused");
                CaptureOutcome::Refused(ev.code)
            }
            other => panic!("unexpected terminal on object {other}"),
        }
    }

    /// Read the bytes a `frame_ready` actually delivered out of its sealed
    /// memfd — what the **agent** holds, never what any state says.
    fn frame_bytes(client: &mut Connection, view_id: u32) -> Vec<u8> {
        use std::os::unix::fs::FileExt;
        let frame = expect_frame(client, view_id);
        let len = (frame.stride * frame.height) as usize;
        let mut bytes = vec![0u8; len];
        std::fs::File::from(frame.fd)
            .read_exact_at(&mut bytes, 0)
            .expect("the delivered memfd is readable");
        bytes
    }

    /// **A grant over one realm captures that realm's pixels and no
    /// sibling's** (WS-E.1.3, issue #209, decision 1) — byte-exact, at the
    /// chokepoint, over the wire.
    ///
    /// This is issue #209's first acceptance criterion stated where the
    /// decision is actually made. `ServerCtx::realm_view` used to be one
    /// `Option<RealmViewFrame>` for the session, built from one scene holding
    /// at most one committed surface, so an `observe` grant over realm A was
    /// served realm B's pixels the instant B painted. Nothing in that code was
    /// wrong — there was one realm — and nothing in it prevented the leak
    /// either. Making the frame a **function of the realm id** is what makes
    /// the leak unrepresentable; this is the test that says so in bytes.
    ///
    /// Both realms are alive and both deliver, which is what makes it a
    /// *selection* test rather than a refusal test: a chokepoint that refused
    /// the second grant, or served it a black frame, would satisfy "no
    /// sibling's pixels" and be useless. The fixtures are two distinct
    /// full-view images, asserted `assert_ne!` first, so "A got A's" cannot
    /// pass by the two happening to be equal.
    ///
    /// Its siblings each disclaim a property by pointing here:
    /// [`a_grant_over_a_dead_realm_cannot_photograph_a_live_siblings_scene`]
    /// asks only the liveness question, and
    /// [`a_grant_over_one_realm_cannot_actuate_into_a_siblings_app`] only the
    /// delivery-target one. `session.rs`'s
    /// `a_capture_returns_the_granted_realms_pixels_and_never_the_outputs`
    /// makes the same claim end to end through the shipped runtime, where the
    /// per-realm cache and the real compositor fill in; this one holds the
    /// chokepoint's own half, so a regression in `serve_facet_use`'s pairing
    /// of realm-to-frame fails here rather than only there.
    #[test]
    fn a_grant_over_one_realm_captures_that_realms_pixels_and_no_siblings() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        shared.realms = crate::realm::tests::registry_with(&["realm-0", "editor"]);
        shared.live_realms.insert(RealmId::new("editor"));

        // Two views that are each a full frame and are obviously different:
        // the checkerboard test pattern, and its byte-wise inverse. Both are
        // the same size, so a swap is a content difference and nothing else.
        let view_a = crate::test_pattern::render(VIEW_W, VIEW_H);
        let view_b: Vec<u8> = view_a.iter().map(|b| !b).collect();
        assert_ne!(
            view_a, view_b,
            "the two realms' fixtures must actually differ, or nothing below is a test"
        );
        shared.set_view(
            &RealmId::new("realm-0"),
            Some((view_a.clone(), VIEW_W, VIEW_H)),
        );
        shared.set_view(
            &RealmId::new("editor"),
            Some((view_b.clone(), VIEW_W, VIEW_H)),
        );

        let (mut server, mut core, mut client) = connect(&mut shared);
        // Realm handle 3 = realm-0, grant over it at ids 4..=8 (view facet 6).
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);

        // Realm handle 9 = editor, grant over it at ids 10..=14 (view facet
        // 12). The ids climb because the watermark rule says they must.
        send_get_realm(&mut client, 2, 9, "editor");
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("get_realm editor");
        client
            .send_message(&petition_at(10).encode(9), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 11, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 10).outcome, Outcome::Granted);

        // The two captures, in the order that makes a shared frame visible:
        // A first, then B. Under one frame for the session both would carry
        // whatever `realm_view` last resolved to, which is the leak.
        let (wire_a, wire_b) = (
            crate::capture::tests::xrgb_of(&view_a),
            crate::capture::tests::xrgb_of(&view_b),
        );
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let got_a = frame_bytes(&mut client, 6);
        client
            .send_message(&view::requests::CaptureFrame {}.encode(12), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let got_b = frame_bytes(&mut client, 12);

        // Diagnosed rather than dumped: two full frames in an `assert_eq!`
        // message is a wall of bytes nobody reads, and the fact that matters
        // is *whose* pixels arrived.
        let whose = |got: &[u8]| match got {
            g if g == wire_a => "realm-0's",
            g if g == wire_b => "the editor's",
            _ => "neither realm's",
        };
        assert_eq!(
            whose(&got_a),
            "realm-0's",
            "a grant over realm-0 must be served realm-0's own pixels; it got {} instead",
            whose(&got_a)
        );
        assert_eq!(
            whose(&got_b),
            "the editor's",
            "a grant over the editor was served {} pixels: with one frame for the session a \
             capture returns whatever realm painted last, which is the cross-realm leak \
             WS-E.1.3 exists to close",
            whose(&got_b)
        );
        assert_ne!(got_a, got_b);

        // ...and the selection follows the *content*, not the facet id: swap
        // what each realm is painting and each grant's capture swaps with it.
        // A chokepoint that had cached a frame per facet, or resolved the
        // realm once at grant time, passes everything above and fails here.
        shared.set_view(&RealmId::new("realm-0"), Some((view_b, VIEW_W, VIEW_H)));
        shared.set_view(&RealmId::new("editor"), Some((view_a, VIEW_W, VIEW_H)));
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(whose(&frame_bytes(&mut client, 6)), "the editor's");
        client
            .send_message(&view::requests::CaptureFrame {}.encode(12), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(whose(&frame_bytes(&mut client, 12)), "realm-0's");
    }

    /// **A grant over a dead realm refuses `no_surface`, even while a live
    /// sibling is painting** (WS-E.1.2 review, HIGH 1).
    ///
    /// The bug this pins is specific to there being more than one realm:
    /// liveness used to be derived as "does *any* attached realm have a live
    /// view", which is fail-**open** across realms. Realm A's grant passed
    /// the gate on realm B's account and then captured the one scene -- B's
    /// pixels, delivered under a grant over a realm that no longer exists.
    /// One realm could not express the bug at all, which is why nothing
    /// before this caught it.
    ///
    /// Since WS-E.1.3 each realm has its own view here, so the survivor's
    /// capture is its *own* frame rather than the session's -- see
    /// [`a_grant_over_one_realm_captures_that_realms_pixels_and_no_siblings`]
    /// for the byte-exact statement of that. This test still asks only the
    /// liveness question.
    ///
    /// Both directions are asserted, because only the pair is evidence: the
    /// dead realm's facet must refuse **and** the survivor's must still
    /// deliver, or a fix that simply refused everything would pass.
    #[test]
    fn a_grant_over_a_dead_realm_cannot_photograph_a_live_siblings_scene() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        shared.realms = crate::realm::tests::registry_with(&["realm-0", "editor"]);
        shared.live_realms.insert(RealmId::new("editor"));
        // Its own view, because a realm's capture is its own frame since
        // WS-E.1.3: a live realm with no entry here has composited nothing
        // and is refused `no_surface` for that reason, which would make the
        // liveness assertion below pass for the wrong reason.
        shared.set_view(
            &RealmId::new("editor"),
            Some((vec![0x33u8; (VIEW_W * VIEW_H * 4) as usize], VIEW_W, VIEW_H)),
        );
        let (mut server, mut core, mut client) = connect(&mut shared);

        // Realm handle 3 = realm-0, then a grant over it (ids 4..=8).
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);

        // Realm handle 9 = editor, then a grant over it (ids 10..=14). The
        // ids climb because the watermark rule says they must, which is also
        // why the second `get_realm` comes after the first petition.
        send_get_realm(&mut client, 2, 9, "editor");
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("get_realm editor");
        client
            .send_message(&petition_at(10).encode(9), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 11, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 10).outcome, Outcome::Granted);

        // Both realms live: both grants observe their own view. (That the
        // two views are each realm's own is
        // `a_grant_over_one_realm_captures_that_realms_pixels_and_no_siblings`'
        // subject, not this test's -- this is the baseline the refusal below
        // is measured against.)
        for (view_id, grant_id) in [(6, 4), (12, 10)] {
            assert!(
                matches!(
                    capture_facet(
                        &mut server,
                        &mut core,
                        &mut client,
                        &verifier,
                        &mut shared,
                        view_id,
                        grant_id
                    ),
                    CaptureOutcome::Frame { .. }
                ),
                "a grant over a live realm must capture (facet {view_id})"
            );
        }

        // The editor realm's shim dies. Every view is untouched -- realm-0
        // is still painting its own, and the editor's last frame is still in
        // the rig's map -- so the only thing that changed is *whose* realm
        // has a live view. Leaving the frame there is the point: liveness,
        // not the absence of pixels, is what must refuse.
        shared.live_realms.remove(&RealmId::new("editor"));

        assert_eq!(
            capture_facet(
                &mut server,
                &mut core,
                &mut client,
                &verifier,
                &mut shared,
                12,
                10
            ),
            CaptureOutcome::Refused(Refusal::NoSurface),
            "a grant over a dead realm must refuse no_surface, whatever its siblings are doing"
        );
        assert!(
            matches!(
                capture_facet(
                    &mut server,
                    &mut core,
                    &mut client,
                    &verifier,
                    &mut shared,
                    6,
                    4
                ),
                CaptureOutcome::Frame { .. }
            ),
            "a sibling's death must not cost the survivor's grant its captures"
        );
    }

    /// **A grant naming realm B actuates into realm B's app, while the human
    /// is in realm A** (WS-E.1.6, issue #212) -- the write-side mirror of the
    /// capture test above, and the criterion this issue exists for.
    ///
    /// # What this replaced, and why the assertion is inverted
    ///
    /// WS-E.1.2 raised `MAX_REALMS` above 1 while the session still had one
    /// input router and one delivery target, so an actuation admitted under a
    /// grant naming any other realm would have been *delivered into a
    /// sibling's app*. The stopgap refused it `internal`, and the test here
    /// asserted the refusal. That is now the wrong behaviour: an agent working
    /// in a realm the human is not looking at is the concurrent-operation
    /// claim the project rests on, so the refusal is gone and the delivery is
    /// the property.
    ///
    /// The sink is `session::route_seat`'s stand-in and each entry names the
    /// realm it was addressed to, which is the assertion that could not be
    /// written before: previously every admitted actuation went to the one
    /// target and the realm was not part of the event.
    ///
    /// Both directions again, because only the pair is evidence: delivering
    /// everything to one realm would satisfy "the editor's grant actuated"
    /// and be exactly the bug.
    #[test]
    fn a_grant_over_a_hidden_realm_actuates_into_that_realm_and_no_other() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        shared.realms = crate::realm::tests::registry_with(&["realm-0", "editor"]);
        shared.live_realms.insert(RealmId::new("editor"));
        // The editor's own view: without it the editor's actuations would be
        // refused `no_surface` at step 5a and this test would prove nothing
        // about routing.
        shared.set_view(
            &RealmId::new("editor"),
            Some((vec![0x33u8; (VIEW_W * VIEW_H * 4) as usize], VIEW_W, VIEW_H)),
        );
        let (mut server, mut core, mut client) = connect(&mut shared);

        // Grant over realm-0 (facets 4..=8), then over editor (10..=14) --
        // the same ladder the capture test climbs, for the same watermark
        // reason.
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);

        send_get_realm(&mut client, 2, 9, "editor");
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("get_realm editor");
        client
            .send_message(&petition_at(10).encode(9), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 11, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 10).outcome, Outcome::Granted);

        // The human is in realm-0 and the editor is hidden. Both grants
        // actuate, and each event is addressed to its own grant's realm.
        assert_eq!(shared.physical_realm, Some(RealmId::new("realm-0")));
        client.send_message(&move_to(3, 4), None).unwrap();
        client
            .send_message(&pointer::requests::Move { x: 5, y: 6 }.encode(13), None)
            .unwrap();
        client
            .send_message(&text::requests::Type { text: "hi".into() }.encode(14), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 3).unwrap();
        assert_eq!(
            shared
                .actuations
                .iter()
                .map(|(realm, input)| (realm.as_str().to_string(), input.kind().clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "realm-0".to_string(),
                    SeatInputKind::Motion { x: 3.0, y: 4.0 }
                ),
                (
                    "editor".to_string(),
                    SeatInputKind::Motion { x: 5.0, y: 6.0 }
                ),
                (
                    "editor".to_string(),
                    SeatInputKind::Text { text: "hi".into() }
                ),
            ],
            "each admitted actuation must be addressed to the realm ITS OWN grant names, \
             hidden or not -- an agent that can only work in the realm on screen is the \
             concurrent-operation claim withdrawn"
        );

        // Nothing was refused: no `refused` event is waiting on either grant.
        // Asserted through a `sync` fence, which is the only bounded way to
        // say "and nothing else arrived".
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            77,
        );

        // Moving the human's attention to the editor changes **nothing**
        // about either grant's delivery, which is the second half of the
        // property: physical attention and grant authority are two different
        // addressing rules and neither may move the other.
        shared.actuations.clear();
        shared.physical_realm = Some(RealmId::new("editor"));
        client
            .send_message(&pointer::requests::Move { x: 7, y: 8 }.encode(13), None)
            .unwrap();
        client.send_message(&move_to(1, 2), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        assert_eq!(
            shared
                .actuations
                .iter()
                .map(|(realm, input)| (realm.as_str().to_string(), input.kind().clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "editor".to_string(),
                    SeatInputKind::Motion { x: 7.0, y: 8.0 }
                ),
                (
                    "realm-0".to_string(),
                    SeatInputKind::Motion { x: 1.0, y: 2.0 }
                ),
            ],
            "an agent's actuation follows its grant, not the human's attention"
        );

        // Observation is untouched. (Which realm's pixels a capture returns
        // is
        // `a_grant_over_one_realm_captures_that_realms_pixels_and_no_siblings`'
        // subject, not this test's.)
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        drop(expect_frame(&mut client, 6));

        // Recoverable throughout: the connection is still serving.
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            78,
        );
    }

    /// **Per-realm `preempted`** (WS-E.1.6, issue #212, decision 4): a human
    /// working in realm A preempts an agent actuating in realm A and **does
    /// not** preempt one actuating in realm B.
    ///
    /// This is the criterion issue #212 names as "the test that fails today".
    /// It failed because [`Shared::presence`] was one session-wide tracker:
    /// "physical human input owns *the target*" was answered for the whole
    /// session, so a hand anywhere muted every agent everywhere.
    ///
    /// **Both halves, and the positive half first.** A per-realm split that
    /// simply stopped refusing would satisfy "B is not preempted" and silently
    /// remove the gate; the realm-A refusal is what says the gate still
    /// exists. And the realm-A half is asserted at the *same instant* as the
    /// realm-B half -- one `now`, one presence note -- so neither can pass on
    /// the hold window having elapsed.
    #[test]
    fn physical_presence_in_one_realm_preempts_an_agent_only_in_that_realm() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        shared.realms = crate::realm::tests::registry_with(&["realm-0", "editor"]);
        shared.live_realms.insert(RealmId::new("editor"));
        shared.set_view(
            &RealmId::new("editor"),
            Some((vec![0x33u8; (VIEW_W * VIEW_H * 4) as usize], VIEW_W, VIEW_H)),
        );
        let (mut server, mut core, mut client) = connect(&mut shared);

        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);

        send_get_realm(&mut client, 2, 9, "editor");
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("get_realm editor");
        client
            .send_message(&petition_at(10).encode(9), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 11, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 10).outcome, Outcome::Granted);

        // The human touches realm-0's input, at this turn's instant. Fed the
        // way the router's hook point feeds it: with the realm the event was
        // addressed to, which for physical input is the bound realm.
        shared.presence.note(
            Some(&RealmId::new("realm-0")),
            Origin::Physical,
            &SeatInputKind::Motion { x: 1.0, y: 1.0 },
            shared.now,
        );

        // The agent actuates in both realms, in one dispatch round.
        client.send_message(&move_to(3, 4), None).unwrap();
        client
            .send_message(&pointer::requests::Move { x: 5, y: 6 }.encode(13), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();

        // The editor's actuation landed; realm-0's did not.
        assert_eq!(
            shared
                .actuations
                .iter()
                .map(|(realm, _)| realm.as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["editor".to_string()],
            "a hand in realm-0 must preempt realm-0's agent and no other realm's: muting \
             every agent everywhere is the blanket answer WS-E.1.6 replaced"
        );
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::Preempted);

        // The gate is transient, not a wedge: past the hold window realm-0's
        // own agent actuates again, with the presence untouched.
        shared.now += crate::input::PHYSICAL_HOLD_WINDOW;
        shared.actuations.clear();
        client.send_message(&move_to(9, 9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            shared.actuations.len(),
            1,
            "preemption is a transient suspension (PRD Doc 2 SS8), not a lock"
        );
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
        // No supported-version hint: downgrade is refusal, not
        // negotiation. Pinned to the maximum this build actually accepts
        // rather than to a literal digit -- the assertion was written
        // when that maximum was 1, and a literal would have kept passing
        // while guarding nothing after the bump.
        assert!(
            !err.message.contains(&PROTOCOL_VERSION.to_string()),
            "error.message must not name the server's maximum version: {:?}",
            err.message
        );
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

    // -- acceptance: human consent decisions (P1.7.2) ----------------------

    /// Raise `petition`'s prompt the way the M1.1 embedder will: arm the
    /// grab against a real consent surface, then announce `shown` on the
    /// petitioner's connection. Returns the surface so the caller can lower
    /// it, mirroring the real ownership (the backend owns the surface).
    fn raise_prompt(
        server: &mut PrincipalServer,
        core: &mut Connection,
        shared: &mut Shared,
        grab: &mut crate::consent::grab::ConsentGrab,
        surface: &mut crate::consent::ConsentSurface,
        petition: PetitionId,
    ) {
        let route = grab
            .raise(
                petition,
                std::time::Instant::now(),
                &mut shared.petitions,
                surface,
                &mut shared.recorder,
            )
            .expect("the petition is pending");
        server
            .emit_consent_shown(route, &mut |frame, fd| core.send_message(frame, fd))
            .expect("announce the prompt");
    }

    #[test]
    fn allowing_a_prompt_activates_the_grant_the_agent_then_uses() {
        let _fd = crate::capture::tests::fd_lock();
        // The P1.7.2 acceptance criterion "Allow activates the grant; the
        // agent's next request under it succeeds", closed end to end over
        // the real wire: queued -> shown -> (human clicks Allow) -> closed
        // -> resolved(granted), and the very next actuation is admitted.
        use crate::consent::grab::ConsentGrab;
        use crate::consent::{Choice, ConsentSurface};

        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let petition = shared.petitions.pending_ids()[0];
        let mut grab = ConsentGrab::new();
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        grab.set_view((VIEW_W, VIEW_H));
        raise_prompt(
            &mut server,
            &mut core,
            &mut shared,
            &mut grab,
            &mut surface,
            petition,
        );
        expect_consent_state(&mut client, 5, ConsentState::Shown);

        // Mid-prompt actuation reaches nothing. On *this* facet the
        // refusal is `not_granted` rather than `consent_held`, and that is
        // the chokepoint being honest rather than a gap: the facet's own
        // grant has not resolved, so it confers nothing yet, and the chain
        // answers the authority question before the use-context one
        // (`enforcement`, step 3 before step 5b). `consent_held` is the
        // refusal for a principal holding an *already live* grant while a
        // new petition's prompt is up -- proved end to end, all the way to
        // a mock shim's seat, in `crate::input`'s
        // `an_actuation_sent_mid_prompt_never_reaches_the_app`. Either way
        // the property that matters here holds: the sink is the only route
        // to the app, and nothing entered it.
        client.send_message(&move_to(5, 5), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::ACTUATE_POINTER, Refusal::NotGranted);
        assert!(
            shared.actuations.is_empty(),
            "no actuation may reach the app while the prompt is up"
        );

        // The human clicks "Allow while running".
        let choice = Choice::Allow(crate::grants::PersistenceRung::WhileRunning);
        let resolution = shared
            .petitions
            .resolve_human(petition, choice)
            .expect("still pending");
        grab.lower(&mut shared.petitions, &mut surface);
        deliver(&mut server, &mut core, &mut shared, resolution);

        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Granted);
        assert_eq!(
            resolved.verbs,
            all_verbs(),
            "the petitioned verbs, verbatim"
        );
        assert_eq!(resolved.persistence, WirePersistence::WhileRunning);

        // The grant is live and the agent's next request under it succeeds
        // -- the criterion, asserted through the real facet arm rather than
        // by inspecting the table.
        let row = server.grant_row_id(4).expect("wire handle maps to a row");
        assert_eq!(
            shared.grants.get(row, shared.now).unwrap().0.issuer,
            Issuer::HumanConsent
        );
        client.send_message(&move_to(7, 8), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            shared.actuations.len(),
            1,
            "the prompt is closed: the agent's next actuation is admitted"
        );
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
    fn denying_a_prompt_returns_a_clean_denial_event() {
        let _fd = crate::capture::tests::fd_lock();
        // "Deny returns a denial event (a clean protocol event, not a hang
        // or a disconnect)": the terminal arrives with zeroed effective
        // arguments, no authority exists, and the connection is still
        // serving requests afterwards.
        use crate::consent::grab::ConsentGrab;
        use crate::consent::{Choice, ConsentSurface};

        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let petition = shared.petitions.pending_ids()[0];
        let mut grab = ConsentGrab::new();
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        grab.set_view((VIEW_W, VIEW_H));
        raise_prompt(
            &mut server,
            &mut core,
            &mut shared,
            &mut grab,
            &mut surface,
            petition,
        );
        expect_consent_state(&mut client, 5, ConsentState::Shown);

        let resolution = shared
            .petitions
            .resolve_human(petition, Choice::Deny)
            .expect("still pending");
        grab.lower(&mut shared.petitions, &mut surface);
        deliver(&mut server, &mut core, &mut shared, resolution);

        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Denied);
        assert_eq!(resolved.verbs, Verb::default(), "trailing arguments zeroed");
        assert_eq!(resolved.expiry_ms, 0);
        assert_eq!(server.grant_row_id(4), None);
        assert_eq!(shared.grants.rows(shared.now).count(), 0);

        // Not a hang and not a disconnect: the connection still answers.
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
    fn every_consent_state_is_delivered_before_the_grant_terminal() {
        let _fd = crate::capture::tests::fd_lock();
        // The IDL's ordering guarantee, asserted over the *whole* event
        // stream rather than event by event: every `state` this petition
        // produced precedes its `resolved`, on the wire and in the log.
        use crate::consent::grab::ConsentGrab;
        use crate::consent::{Choice, ConsentSurface};

        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();

        let petition = shared.petitions.pending_ids()[0];
        let mut grab = ConsentGrab::new();
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        grab.set_view((VIEW_W, VIEW_H));
        raise_prompt(
            &mut server,
            &mut core,
            &mut shared,
            &mut grab,
            &mut surface,
            petition,
        );
        let resolution = shared
            .petitions
            .resolve_human(petition, Choice::Deny)
            .expect("still pending");
        grab.lower(&mut shared.petitions, &mut surface);
        deliver(&mut server, &mut core, &mut shared, resolution);

        // The wire: three states in lifecycle order, then the terminal.
        for want in [
            ConsentState::Queued,
            ConsentState::Shown,
            ConsentState::Closed,
        ] {
            expect_consent_state(&mut client, 5, want);
        }
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Denied);

        // The log preserves the same order (the recorder's subject is the
        // moment core state changed, and it writes before it sends).
        let log = shared.log();
        let order: Vec<&str> = log
            .iter()
            .filter(|e| matches!(e.str("kind"), "consent_transition" | "petition_resolved"))
            .map(|e| {
                if e.str("kind") == "petition_resolved" {
                    "resolved"
                } else {
                    e.str("state")
                }
            })
            .collect();
        assert_eq!(order, ["queued", "shown", "closed", "resolved"]);
    }

    #[test]
    fn every_consent_decision_lands_in_the_flight_recorder_with_its_cause() {
        let _fd = crate::capture::tests::fd_lock();
        // "Every consent decision -- including auto-approvals -- lands in
        // the recorder with its cause." One run per cause, each asserting
        // the outcome and the issuer the log states.
        use crate::consent::grab::ConsentGrab;
        use crate::consent::{Choice, ConsentSurface};

        // 1. Human allow -> granted, issuer human_consent.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let petition = shared.petitions.pending_ids()[0];
        let mut grab = ConsentGrab::new();
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        grab.set_view((VIEW_W, VIEW_H));
        raise_prompt(
            &mut server,
            &mut core,
            &mut shared,
            &mut grab,
            &mut surface,
            petition,
        );
        let resolution = shared
            .petitions
            .resolve_human(
                petition,
                Choice::Allow(crate::grants::PersistenceRung::WhileRunning),
            )
            .expect("pending");
        grab.lower(&mut shared.petitions, &mut surface);
        deliver(&mut server, &mut core, &mut shared, resolution);
        let entry = last_resolution(&shared);
        assert_eq!(entry.str("outcome"), "granted");
        assert_eq!(entry.str("issuer"), "human_consent");
        // The `shown` transition is recorded too, naming its petition --
        // without it the log could not say a human was ever asked.
        let shown = shared
            .log()
            .into_iter()
            .find(|e| e.str("kind") == "consent_transition" && e.str("state") == "shown")
            .expect("the raised prompt is recorded");
        assert!(!shown.is_null("petition"));

        // 2. Human deny -> denied, no issuer (nothing was granted).
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let petition = shared.petitions.pending_ids()[0];
        let resolution = shared
            .petitions
            .resolve_human(petition, Choice::Deny)
            .expect("pending");
        deliver(&mut server, &mut core, &mut shared, resolution);
        let entry = last_resolution(&shared);
        assert_eq!(entry.str("outcome"), "denied");
        assert!(entry.is_null("issuer"));

        // 3. Timeout -> timed_out (a decision by default, and the one a
        //    human never made -- so the log must say it happened).
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        shared.now += Duration::from_secs(120);
        let due = shared.petitions.expire_due(shared.now);
        assert_eq!(due.len(), 1);
        deliver(
            &mut server,
            &mut core,
            &mut shared,
            due.into_iter().next().unwrap(),
        );
        assert_eq!(last_resolution(&shared).str("outcome"), "timed_out");

        // 4. Auto-approve -> granted, issuer auto_approve_policy. The
        //    loudest case: a grant no human ever saw must be as legible in
        //    the log as one they clicked.
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let entry = last_resolution(&shared);
        assert_eq!(entry.str("outcome"), "granted");
        assert_eq!(entry.str("issuer"), "auto_approve_policy");
    }

    /// The most recent `petition_resolved` entry in this rig's log.
    fn last_resolution(shared: &Shared) -> crate::recorder::tests::Json {
        shared
            .log()
            .into_iter()
            .rfind(|e| e.str("kind") == "petition_resolved")
            .expect("a resolution was recorded")
    }

    #[test]
    fn a_prompt_cannot_be_announced_for_another_connection_or_a_dead_one() {
        let _fd = crate::capture::tests::fd_lock();
        // The emission guards: a misrouted prompt surfaces as the routing
        // bug it is rather than vanishing, and a connection past its
        // terminal never receives another event.
        use crate::consent::grab::ConsentGrab;
        use crate::consent::ConsentSurface;

        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);
        let petition = shared.petitions.pending_ids()[0];

        let mut grab = ConsentGrab::new();
        let mut surface = ConsentSurface::new(crate::consent::TrustedIndicator::for_test());
        grab.set_view((VIEW_W, VIEW_H));
        let route = grab
            .raise(
                petition,
                std::time::Instant::now(),
                &mut shared.petitions,
                &mut surface,
                &mut shared.recorder,
            )
            .expect("pending");

        // Wrong connection: refused, nothing sent.
        let stranger = crate::petitions::PromptRoute {
            connection: shared.petitions.register_connection(),
            consent_wire_id: route.consent_wire_id,
        };
        assert!(matches!(
            server.emit_consent_shown(stranger, &mut |f, fd| core.send_message(f, fd)),
            Err(PromptEmitError::WrongConnection { .. })
        ));

        // An id that is not a consent object on this connection: refused.
        let bogus = crate::petitions::PromptRoute {
            connection: route.connection,
            consent_wire_id: 4, // the grant handle, not the consent object
        };
        assert!(matches!(
            server.emit_consent_shown(bogus, &mut |f, fd| core.send_message(f, fd)),
            Err(PromptEmitError::UnknownConsentObject { wire_id: 4 })
        ));

        // The real route works, and after teardown nothing more is sent --
        // an expected race (the petitioner dies as its prompt goes up).
        assert!(server
            .emit_consent_shown(route, &mut |f, fd| core.send_message(f, fd))
            .is_ok());
        expect_consent_state(&mut client, 5, ConsentState::Shown);
        server.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );
        assert!(matches!(
            server.emit_consent_shown(route, &mut |f, fd| core.send_message(f, fd)),
            Err(PromptEmitError::ConnectionDead)
        ));
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
            .deliver_resolution(
                replay,
                &mut shared.grants,
                &mut shared.recorder,
                shared.now,
                &mut |frame, fd| core.send_message(frame, fd),
            )
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
        server_a.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );
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
        server_a.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );
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
        server.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );
        assert_eq!(shared.grants.rows(shared.now).count(), 0);

        // The embedder's late routing is refused whole: no events, no row,
        // and the handle never resolves (its petitioner no longer exists).
        let mut sent = 0usize;
        let err = server
            .deliver_resolution(
                resolution,
                &mut shared.grants,
                &mut shared.recorder,
                shared.now,
                &mut |_, _| {
                    sent += 1;
                    Ok(())
                },
            )
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
                &mut shared.recorder,
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
    fn realm_zero_petitions_resolve_through_the_realm_registry() {
        // P1.5.1 acceptance criteria 1 and 2, end to end on the wire: the
        // realm is addressable by its stable id, and a petition naming it
        // resolves through the *registry*. The proof that this is not a
        // hardcode is the inversion: with a registry that serves "kiosk"
        // and not "realm-0", the same "realm-0" petition that succeeds
        // above now resolves unavailable, and a "kiosk" petition pends.
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        let (mut server, mut core, mut client) = connect(&mut shared);

        // Against the default registry (realm-0 configured), the petition
        // is admitted: it pends for consent rather than resolving.
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);
        assert_eq!(shared.petitions.pending_total(), 1);

        // The row the grant will state names that same registry id.
        assert_eq!(
            shared
                .realms
                .resolve_for_petition("realm-0")
                .map(crate::grants::RealmId::as_str),
            Some("realm-0")
        );

        // Now the inversion, on a fresh rig whose registry holds a
        // differently named realm. Version-0 *config* cannot produce that
        // registry -- `realm.toml` pins the id to the IDL's well-known name
        // -- but the addressing path underneath is name-agnostic, and that
        // is what this asserts: nothing between the wire and the registry
        // privileges "realm-0".
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        shared.realms = crate::realm::tests::registry_with(&["kiosk"]);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind(&mut server, &mut core, &mut client, &verifier, &mut shared);
        send_get_realm(&mut client, 2, 3, "realm-0");
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        assert_eq!(
            expect_resolved(&mut client, 4).outcome,
            Outcome::Unavailable,
            "the well-known name is not privileged: existence comes from the registry"
        );

        // ... and the configured name is what this session serves.
        send_get_realm(&mut client, 2, 10, "kiosk");
        client
            .send_message(&petition_at(11).encode(10), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        expect_consent_state(&mut client, 12, ConsentState::Queued);
        assert_eq!(shared.petitions.pending_total(), 1);
    }

    #[test]
    fn a_vacant_realm_petitions_resolve_unavailable() {
        // The IDL folds "unknown" and "vacant" into one client-visible
        // answer. Version 0's registry answers vacancy by presence, so an
        // empty registry -- the shape P1.5.3 will also produce when a realm
        // goes vacant -- resolves every petition unavailable, without any
        // change at this layer.
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::Interactive);
        shared.realms = crate::realm::tests::registry_with(&[]);
        let (mut server, mut core, mut client) = connect(&mut shared);

        // get_realm still succeeds structurally: naming is not authority.
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Unavailable);
        assert_eq!(resolved.verbs, Verb::default(), "declined carries no verbs");
        // No consent lifecycle ever began, and no authority was minted.
        assert_eq!(shared.petitions.pending_total(), 0);
        assert_eq!(shared.grants.rows(shared.now).count(), 0);
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            9,
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
    fn a_consent_defines_no_requests_and_a_grant_defines_only_its_three_mints() {
        let _fd = crate::capture::tests::fd_lock();
        // vitrin_consent defines no requests at any version, and
        // vitrin_grant defines exactly three, all since=2 structural mints:
        // opcode 0 `get_launcher`, 1 `get_layout_focus`, 2
        // `get_layout_arrange`. Everything else on either object is grammar
        // (invalid_opcode), never an authority judgement -- including
        // opcode 0 on the consent object, which is `get_launcher`'s
        // opcode on the *other* interface and must not be routed by
        // number alone.
        //
        // The grant's probes are 3 and 9, one past the last defined mint
        // and well past it: 1 and 2 stopped being invalid when WS-E.1.4
        // appended the layout mints, and this test going red on that append
        // is exactly what an append-only opcode space should do.
        let verifier = demo_verifier();
        for (object_id, opcode) in [(4u32, 3u8), (4, 9), (5, 0), (5, 1)] {
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
                opcode,
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

    // -- acceptance: the version-2 launch facet (WS-E.1.1, issue #207) -----

    /// Encode one `get_launcher(launcher)` on the standard grant handle
    /// (id 4) -- the version-2 structural mint.
    fn get_launcher(launcher_id: u32) -> Vec<u8> {
        grant::requests::GetLauncher {
            launcher: launcher_id,
        }
        .encode(4)
    }

    /// Encode one `launch` on a launch facet.
    fn launch(launcher_id: u32) -> Vec<u8> {
        launcher::requests::Launch {}.encode(launcher_id)
    }

    /// Assert the next client-visible event is `vitrin_launcher.launched`
    /// on the given facet, returning it for the realm-id assertion.
    fn expect_launched(client: &mut Connection, facet_id: u32) -> launcher::events::Launched {
        let msg = client.recv_message().unwrap().unwrap();
        assert!(msg.fd.is_none(), "launched carries no fd");
        let (object_id, ev) = launcher::events::Launched::decode(&msg.bytes, msg.fd)
            .expect("the terminal of a launch is `launched`");
        assert_eq!(object_id, facet_id, "launched arrives on the launch facet");
        ev
    }

    #[test]
    fn get_launcher_mints_an_inert_facet_and_never_kills_the_connection() {
        let _fd = crate::capture::tests::fd_lock();
        // The regression this test exists for: before the facet was
        // routed, EVERY opcode on a grant was fatal invalid_opcode, so a
        // conformant client sending the newly-documented `get_launcher`
        // had its socket killed. A structural mint answers nothing and
        // must leave the connection alive (conventions §6: no terminal
        // event, no wire acknowledgement).
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        client.send_message(&get_launcher(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("the mint is legal");
        assert_eq!(
            server.objects.get(&9),
            Some(&ObjectKind::Launcher { grant: 4 }),
            "the facet remembers its grant's wire id -- the chokepoint's key"
        );
        // Minting twice on the same grant is explicitly legal (IDL): a
        // second, equivalent facet, conferring nothing extra.
        client.send_message(&get_launcher(10), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("a second mint");
        assert_eq!(
            server.objects.get(&10),
            Some(&ObjectKind::Launcher { grant: 4 })
        );
        // The mint is silent and the connection is alive: `done` is the
        // very next event on the wire.
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
    fn minting_the_launch_facet_is_legal_before_the_petition_resolves() {
        let _fd = crate::capture::tests::fd_lock();
        // Mint-freely, check-at-use: refusing the mint while the petition
        // is pending would make it an authority oracle, telling the
        // petitioner something only `resolved` may say.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        client.send_message(&get_launcher(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1)
            .expect("the mint is legal on a pending grant");
        assert_eq!(
            server.objects.get(&9),
            Some(&ObjectKind::Launcher { grant: 4 })
        );
        // ...and the inert facet refuses recoverably on use, exactly as
        // the co-minted three do while pending.
        client.send_message(&launch(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).expect("a launch is admitted");
        expect_refused(&mut client, 4, Verb::REALM_LAUNCH, Refusal::NotGranted);
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
    fn launching_without_the_verb_refuses_not_granted_and_leaves_the_socket_alive() {
        let _fd = crate::capture::tests::fd_lock();
        // A grant that does not carry `realm_launch` -- `granted_rig`
        // petitions the three original facet verbs -- refuses every launch
        // at step 4, whatever else is true. This used to hold for the whole
        // deployment (the bit was absent from `SERVED_VERB_BITS`, so no row
        // could carry it); since WS-E.1.1 the verb is served and the
        // property is the narrower, permanent one: **authority you were not
        // granted is refused**. Note *which* code: there is
        // no `refused(verb, unsupported)` on the wire -- `unsupported` is
        // a petition OUTCOME, and the refusal enum's `not_granted` entry
        // covers "the verb is outside its effective set" by name. The
        // property that matters is the razor's: a well-formed request the
        // deployment will not serve is RECOVERABLE.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        client.send_message(&get_launcher(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();

        // Two launches, two refusals: a launch is reply-bearing, so its
        // refusals are never coalesced (conventions §6.1), unlike an
        // actuation's.
        client.send_message(&launch(9), None).unwrap();
        client.send_message(&launch(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2)
            .expect("launches are admitted");
        expect_refused(&mut client, 4, Verb::REALM_LAUNCH, Refusal::NotGranted);
        expect_refused(&mut client, 4, Verb::REALM_LAUNCH, Refusal::NotGranted);
        assert!(
            shared.actuations.is_empty(),
            "a launch is not an actuation and reaches no seat"
        );
        // ALIVE: the fence is the assertion. It proves both that the
        // connection survived and that no `launched` was queued behind
        // the refusals.
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            13,
        );
    }

    /// A rig whose grant carries `realm_launch` beside the three original
    /// facet verbs, on the auto-approve policy.
    fn launch_granted_rig(
        verifier: &dyn Verifier,
    ) -> (PrincipalServer, Connection, Connection, Shared) {
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, verifier, &mut shared);
        let mut req = petition_frame();
        req.verbs = all_verbs() | Verb::REALM_LAUNCH;
        client.send_message(&req.encode(3), None).unwrap();
        process_n(&mut server, &mut core, verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        let resolved = expect_resolved(&mut client, 4);
        assert_eq!(resolved.outcome, Outcome::Granted);
        assert!(
            resolved.verbs.contains(Verb::REALM_LAUNCH),
            "a petition naming realm_launch is granted the bit since WS-E.1.1: \
             it stopped resolving `unsupported` when the core gained a spawn path"
        );
        (server, core, client, shared)
    }

    /// **`launched` names a realm the CORE minted, and the client cannot
    /// influence which** (WS-E.1.1, issue #207).
    ///
    /// The two halves this pins are the two the issue calls out as easiest
    /// to get wrong: the reply is a terminal (one per request, in order),
    /// and its `realm` argument is `<template>.<n>` derived from the
    /// **grant's** realm -- `launch` carries no arguments at all, so there
    /// is nothing the client could have named.
    #[test]
    fn a_launch_answers_launched_with_a_core_minted_instance_id() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = launch_granted_rig(&verifier);
        client.send_message(&get_launcher(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();

        // Two launches, pipelined: replies pair in order, and each is a
        // NEW realm -- nothing is ever relaunched (IDL `launch`).
        client.send_message(&launch(9), None).unwrap();
        client.send_message(&launch(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).expect("launches admitted");
        let first = expect_launched(&mut client, 9);
        let second = expect_launched(&mut client, 9);
        assert_eq!(
            (first.realm.as_str(), second.realm.as_str()),
            ("realm-0.1", "realm-0.2"),
            "instance ids are <template>.<n>, minted by the registry and never reused"
        );
        // The embedder was asked about the realm the GRANT names, by the
        // bound identity, against the row the chain judged -- the three
        // facts `LaunchAsk` carries and the only three there are.
        assert_eq!(shared.launches.len(), 2);
        for (template, principal, _) in &shared.launches {
            assert_eq!(template.as_str(), crate::realm::WELL_KNOWN_REALM_ID);
            assert_eq!(principal.as_str(), DEMO_IDENTITY);
        }
        assert!(
            shared.actuations.is_empty() && shared.layout.is_empty(),
            "a launch is neither an actuation nor a layout act"
        );
        sync_fence(
            &mut server,
            &mut core,
            &mut client,
            &verifier,
            &mut shared,
            14,
        );
    }

    /// **A post-admission failure is voiced, never swallowed**: the two
    /// refusals only a launch can produce, each uncoalesced because
    /// `launch` is reply-bearing and a coalesced terminal would leave the
    /// client waiting forever.
    #[test]
    fn a_launch_the_embedder_could_not_serve_is_refused_capacity_or_internal() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = launch_granted_rig(&verifier);
        client.send_message(&get_launcher(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();

        for (answer, code) in [
            (
                crate::enforcement::LaunchRefusal::Capacity,
                Refusal::Capacity,
            ),
            (
                crate::enforcement::LaunchRefusal::Internal,
                Refusal::Internal,
            ),
        ] {
            shared.launch_answer = Some(answer);
            // Twice, to prove neither is coalesced away: an actuation's
            // second identical refusal is muted, a launch's is not.
            client.send_message(&launch(9), None).unwrap();
            client.send_message(&launch(9), None).unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
            expect_refused(&mut client, 4, Verb::REALM_LAUNCH, code);
            expect_refused(&mut client, 4, Verb::REALM_LAUNCH, code);
        }
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
    fn get_launcher_mints_under_the_watermark_rule() {
        let _fd = crate::capture::tests::fd_lock();
        // A structural mint's only failure mode is the object graph
        // (conventions §3.1/§6): a `new_id` at or below the watermark is
        // fatal invalid_object, exactly as `get_realm`'s is. The rig's
        // watermark is 8 (the petition's fifth co-minted id).
        let verifier = demo_verifier();
        for bad_id in [8u32, 4, 1] {
            let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
            client.send_message(&get_launcher(bad_id), None).unwrap();
            expect_violation(
                process_n(&mut server, &mut core, &verifier, &mut shared, 1),
                "invalid_object",
            );
            expect_error(&mut client, WireError::InvalidObject);
        }
    }

    #[test]
    fn launch_facets_are_bounded_by_the_live_object_cap() {
        let _fd = crate::capture::tests::fd_lock();
        // `get_launcher` is the one mint a bound principal can repeat on
        // an object it already holds, so the IDL's "the per-connection
        // live-object cap is what bounds it" needs a cap to be true.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        for i in 0..MAX_LIVE_LAUNCHERS as u32 {
            client.send_message(&get_launcher(9 + i), None).unwrap();
        }
        process_n(
            &mut server,
            &mut core,
            &verifier,
            &mut shared,
            MAX_LIVE_LAUNCHERS,
        )
        .expect("every mint up to the cap is legal");
        client
            .send_message(&get_launcher(9 + MAX_LIVE_LAUNCHERS as u32), None)
            .unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "live-launcher cap",
        );
        expect_error(&mut client, WireError::ResourceExhausted);
    }

    #[test]
    fn unknown_opcodes_on_a_launch_facet_stay_grammar_errors() {
        let _fd = crate::capture::tests::fd_lock();
        // `launch` is opcode 0 and the interface defines nothing else, so
        // opcode 1 is invalid_opcode -- grammar, like every other facet's.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        client.send_message(&get_launcher(9), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        let mut frame = Vec::new();
        vitrin_protocol::wire::FrameHeader {
            object_id: 9,
            size: 0,
            opcode: 1,
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
            Some(&RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)),
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
            Some(&RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)),
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
        assert_eq!(shared.actuations[0].1.origin(), Origin::Emulated);
        assert_eq!(
            shared.actuations[0].1.kind(),
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
        shared.set_view(&RealmId::new(crate::realm::WELL_KNOWN_REALM_ID), None);

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
        shared.set_view(
            &RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
            Some((crate::test_pattern::render(VIEW_W, VIEW_H), VIEW_W, VIEW_H)),
        );
        client.send_message(&capture_frame(), None).unwrap();
        client.send_message(&move_to(1, 2), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 2).unwrap();
        drop(expect_frame(&mut client, 6));
        assert_eq!(
            shared.actuations.len(),
            1,
            "admitted once the realm is back"
        );
        assert_eq!(shared.actuations[0].1.origin(), Origin::Emulated);
        assert_eq!(
            shared.actuations[0].1.kind(),
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

        shared.set_view(&RealmId::new(crate::realm::WELL_KNOWN_REALM_ID), None);
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NoSurface);

        shared.set_view(
            &RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
            Some((crate::test_pattern::render(VIEW_W, VIEW_H), VIEW_W, VIEW_H)),
        );
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
            Some(&RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)),
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
            shared.actuations[0].1.kind(),
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
        shared.set_view(
            &RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
            Some((vec![0u8; 16], VIEW_W, VIEW_H)),
        );
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
        // control characters; every other Unicode Cc control -- C0, DEL
        // (U+007F), and C1 -- is fatal invalid_argument (a correct client
        // never emits them). DEL is forbidden alongside C0/C1 (issue #82):
        // it is a destructive Delete keystroke, never text.
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 0, 0);
        client
            .send_message(&type_text("line\nwith\ttabs ok"), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            shared.actuations[0].1.kind(),
            &SeatInputKind::Text {
                text: "line\nwith\ttabs ok".into()
            }
        );

        for (label, bad) in [
            ("C0 bell", "ring\u{7}"),
            ("DEL", "delete\u{7f}me"),
            ("C1 NEL", "next\u{85}line"),
        ] {
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

    // -- acceptance: the flight recorder (P1.4.5, issue #29) ---------------

    /// The B1 acceptance, at the level that matters: over a multi-capture
    /// run, EVERY delivered capture has a `use_decision` entry carrying a
    /// digest; the digest varies with frame content; and it equals an
    /// independently computed hash of the bytes the agent actually
    /// received out of the sealed memfd.
    #[test]
    fn every_delivered_capture_carries_a_digest_of_the_bytes_the_agent_got() {
        use std::os::unix::fs::FileExt;

        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 20, 0);

        // Three captures, with the realm's content changed between them so
        // the digests must differ; the middle content is restored at the
        // end so a repeat of an identical view digests identically.
        let mut delivered: Vec<(Vec<u8>, String)> = Vec::new();
        for view in [
            crate::test_pattern::render(VIEW_W, VIEW_H),
            vec![0x20u8; (VIEW_W * VIEW_H * 4) as usize],
            crate::test_pattern::render(VIEW_W, VIEW_H),
        ] {
            shared.set_view(
                &RealmId::new(crate::realm::WELL_KNOWN_REALM_ID),
                Some((view, VIEW_W, VIEW_H)),
            );
            client.send_message(&capture_frame(), None).unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            let frame = expect_frame(&mut client, 6);

            // What the agent actually holds: the sealed memfd's bytes.
            let len = (frame.stride * frame.height) as usize;
            let mut bytes = vec![0u8; len];
            std::fs::File::from(frame.fd)
                .read_exact_at(&mut bytes, 0)
                .expect("the delivered memfd is readable");
            let independent = crate::recorder::ObservationDigest::of(&bytes).to_hex();
            delivered.push((bytes, independent));
        }

        let entries = shared.log();
        let captures: Vec<&crate::recorder::tests::Json> =
            crate::recorder::tests::of_kind(&entries, "use_decision")
                .into_iter()
                .filter(|e| e.str("verb") == "observe" && e.str("decision") == "allowed")
                .collect();
        assert_eq!(captures.len(), 3, "one entry per admitted capture");

        for (entry, (bytes, independent)) in captures.iter().zip(&delivered) {
            // B1: never sampled -- every one of them carries a digest ...
            assert!(!entry.is_null("frame"), "every capture entry has a frame");
            assert_eq!(entry.str("frame.digest_alg"), crate::recorder::DIGEST_ALG);
            // ... and it is the digest of exactly the delivered bytes.
            assert_eq!(
                entry.str("frame.digest"),
                independent,
                "the entry must identify the bytes the agent received"
            );
            assert_eq!(entry.u64("frame.bytes"), bytes.len() as u64);
            assert_eq!(entry.u64("frame.width"), u64::from(VIEW_W));
            assert_eq!(entry.u64("frame.height"), u64::from(VIEW_H));
            assert_eq!(entry.str("frame.format"), "xrgb8888");
        }
        // Content sensitivity, and determinism on identical content.
        assert_ne!(
            captures[0].str("frame.digest"),
            captures[1].str("frame.digest"),
            "a different realm view must produce a different digest"
        );
        assert_eq!(
            captures[0].str("frame.digest"),
            captures[2].str("frame.digest"),
            "identical content digests identically"
        );

        // A refused capture delivers nothing, so it identifies nothing --
        // the honest null, not a fabricated digest.
        shared.grants.revoke(server.grant_row_id(4).unwrap());
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::Revoked);
        let entries = shared.log();
        let last = entries.last().expect("entries exist");
        assert_eq!(last.str("kind"), "use_decision");
        assert_eq!(last.str("decision"), "refused");
        assert_eq!(last.str("refusal"), "revoked");
        assert!(last.is_null("frame"));
        assert_eq!(
            last.str("grant_id"),
            server.grant_row_id(4).unwrap().to_string(),
            "a revoked use still names the row that died"
        );
    }

    /// The headline acceptance criterion: a demo run's log lets a human
    /// reconstruct the session -- who connected, what was granted, and what
    /// was done. Drives one full flow (handshake -> petition -> consent ->
    /// captures -> actuations -> refusals -> teardown) and reads the story
    /// back out of the file.
    #[test]
    fn a_demo_runs_log_reconstructs_the_whole_session() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        // A rate of 2/s so an over-rate capture is refused inside the run.
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 2, 0);

        // Two captures (the burst the 2/s bucket allows), then a third that
        // the ceiling refuses.
        for _ in 0..2 {
            client.send_message(&capture_frame(), None).unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
            expect_frame(&mut client, 6);
        }
        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::RateLimited);

        // One pointer move and one typed string, a token apart so both are
        // admitted.
        shared.now += Duration::from_millis(500);
        client.send_message(&move_to(10, 12), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        shared.now += Duration::from_millis(500);
        client.send_message(&type_text("hello"), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(shared.actuations.len(), 2);

        // A second petition, refused by policy, so the log carries a
        // request that never became authority.
        let mut bad = petition_at(20);
        bad.persistence = WirePersistence::Always;
        client.send_message(&bad.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        assert_eq!(
            expect_resolved(&mut client, 20).outcome,
            Outcome::Unsupported
        );

        server.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );

        // --- now read the session back out of the log ---------------------
        let entries = shared.log();
        let kinds: Vec<&str> = entries.iter().map(|e| e.str("kind")).collect();

        // WHO connected: one bind, naming the verifier-canonical identity
        // and the kernel-attested peer -- and no credential bytes anywhere.
        let bound = crate::recorder::tests::of_kind(&entries, "handshake_bound");
        assert_eq!(bound.len(), 1, "kinds seen: {kinds:?}");
        assert_eq!(bound[0].str("identity"), DEMO_IDENTITY);
        assert_eq!(bound[0].str("credential_type"), STATIC_TOKEN_SCHEME);
        assert_eq!(bound[0].u64("credential_bytes"), TOKEN.len() as u64);
        assert_eq!(bound[0].u64("peer_uid"), u64::from(my_uid()));
        let raw = std::fs::read_to_string(&shared.log_path).unwrap();
        assert!(
            !raw.contains(TOKEN),
            "the credential must NEVER appear in the log"
        );

        // WHAT WAS ASKED and WHAT WAS GRANTED: two petitions requested, two
        // resolved, one granted with its effective authority and row id.
        let requested = crate::recorder::tests::of_kind(&entries, "petition_requested");
        assert_eq!(requested.len(), 2);
        assert_eq!(
            requested[0].strings("requested.verbs"),
            vec!["observe", "actuate_pointer", "actuate_text"]
        );
        assert_eq!(requested[0].str("realm_name"), "realm-0");
        assert_eq!(requested[1].str("requested.persistence"), "always");

        let resolved = crate::recorder::tests::of_kind(&entries, "petition_resolved");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].str("outcome"), "granted");
        assert_eq!(resolved[0].str("issuer"), "auto_approve_policy");
        assert_eq!(resolved[0].u64("effective.max_event_rate"), 2);
        assert_eq!(
            resolved[0].strings("effective.verbs"),
            vec!["observe", "actuate_pointer", "actuate_text"]
        );
        let granted_row = resolved[0].str("grant_id").to_string();
        assert_eq!(resolved[1].str("outcome"), "unsupported");
        assert!(resolved[1].is_null("grant_id"));

        // The consent lifecycle: auto-approve stands in for the prompt and
        // announces its close (the IDL's only-closed shape).
        let consent = crate::recorder::tests::of_kind(&entries, "consent_transition");
        assert_eq!(consent.len(), 1);
        assert_eq!(consent[0].str("state"), "closed");

        // WHAT WAS DONE: every chokepoint decision, in order, each naming
        // the grant it was judged against.
        let uses = crate::recorder::tests::of_kind(&entries, "use_decision");
        let story: Vec<(&str, &str)> = uses
            .iter()
            .map(|e| (e.str("verb"), e.str("decision")))
            .collect();
        assert_eq!(
            story,
            vec![
                ("observe", "allowed"),
                ("observe", "allowed"),
                ("observe", "refused"),
                ("actuate_pointer", "allowed"),
                ("actuate_text", "allowed"),
            ]
        );
        assert_eq!(uses[2].str("refusal"), "rate_limited");
        assert!(uses[2].bool("refusal_voiced"));
        // Every decision names the row it was judged against -- the
        // refusal included, since a rate-limited use has a live grant.
        for e in &uses {
            assert_eq!(e.str("grant_id"), granted_row);
            assert_eq!(e.u64("grant_wire_id"), 4);
            assert_eq!(
                e.u64("facet_wire_id"),
                if e.str("verb") == "observe" {
                    6
                } else if e.str("verb") == "actuate_pointer" {
                    7
                } else {
                    8
                }
            );
        }
        // Captures identify what was observed; actuations have no frame.
        assert!(!uses[0].is_null("frame"));
        assert!(uses[3].is_null("frame"), "an actuation delivers no frame");

        // B1: epoch reference slots on every decision, explicitly null.
        for e in &uses {
            assert!(e.is_null("epoch.observed"));
            assert!(e.is_null("epoch.expected"));
            assert!(e.is_null("epoch.target"));
        }

        // AND HOW IT ENDED.
        let teardown = crate::recorder::tests::of_kind(&entries, "connection_teardown");
        assert_eq!(teardown.len(), 1);
        assert_eq!(teardown[0].str("identity"), DEMO_IDENTITY);
        assert_eq!(teardown[0].u64("removed_grants"), 1);
        assert_eq!(teardown[0].u64("withdrawn_petitions"), 0);

        // Nothing was lost, and the whole run is one connection's story.
        assert_eq!(shared.recorder.dropped_entries(), 0);
        assert!(!shared.recorder.is_degraded());
        for e in &entries {
            if let Some(c) = e.path("connection") {
                assert_eq!(*c, crate::recorder::tests::Json::Str("conn-1".into()));
            }
        }
    }

    /// The actuation half of "a human can reconstruct what was done": a
    /// capture entry identifies what was observed, so an actuation entry
    /// must identify what was actuated. The verb alone cannot distinguish
    /// a move from a button press from a scroll, nor say how much was
    /// typed -- and the typed string itself must never be written down.
    #[test]
    fn actuation_entries_reconstruct_what_was_done_without_recording_the_text() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 20, 0);

        // Every actuation shape the version-1 wire can carry, end to end.
        let secret = "correct horse battery staple";
        for frame in [
            move_to(37, -4),
            pointer::requests::Button {
                button: 0x110,
                state: ButtonState::Pressed,
            }
            .encode(7),
            pointer::requests::Button {
                button: 0x110,
                state: ButtonState::Released,
            }
            .encode(7),
            pointer::requests::Scroll {
                axis: Axis::Vertical,
                value120: -120,
            }
            .encode(7),
            type_text(secret),
        ] {
            client.send_message(&frame, None).unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        }
        assert_eq!(shared.actuations.len(), 5, "all five were admitted");

        let entries = shared.log();
        let uses: Vec<&crate::recorder::tests::Json> =
            crate::recorder::tests::of_kind(&entries, "use_decision")
                .into_iter()
                .filter(|e| e.str("decision") == "allowed" && e.str("verb") != "observe")
                .collect();
        assert_eq!(uses.len(), 5);

        // Each is distinguishable from the others -- the property a
        // verb-only entry cannot provide.
        let actions: Vec<&str> = uses.iter().map(|e| e.str("input.action")).collect();
        assert_eq!(
            actions,
            vec!["move", "button", "button", "scroll", "type"],
            "the entry must say WHICH actuation, not just which verb"
        );
        // ... and each carries the parameters that reproduce it.
        assert_eq!(
            uses[0].at("input.x"),
            &crate::recorder::tests::Json::Num(37.0)
        );
        assert_eq!(
            uses[0].at("input.y"),
            &crate::recorder::tests::Json::Num(-4.0)
        );
        assert_eq!(uses[1].u64("input.button"), 0x110);
        assert!(uses[1].bool("input.pressed"));
        assert!(!uses[2].bool("input.pressed"), "press and release differ");
        assert_eq!(uses[3].str("input.axis"), "vertical");
        assert_eq!(
            uses[3].at("input.value120"),
            &crate::recorder::tests::Json::Num(-120.0)
        );

        // The typed string: shape and identity, never the bytes. Agent
        // text is arbitrary user data -- a flight recorder that wrote it
        // out would be a keylogger.
        assert_eq!(uses[4].u64("input.chars"), secret.chars().count() as u64);
        assert_eq!(uses[4].u64("input.bytes"), secret.len() as u64);
        assert_eq!(
            uses[4].str("input.digest"),
            crate::recorder::ObservationDigest::of(secret.as_bytes()).to_hex()
        );
        let raw = std::fs::read_to_string(&shared.log_path).unwrap();
        assert!(
            !raw.contains(secret),
            "typed text must NEVER appear in the log"
        );
        assert!(!raw.contains("correct horse"), "not even a prefix of it");
    }

    /// Connection teardown is the most common way a version-1 grant dies,
    /// and it is a lifecycle transition an E3.4 replay must be able to
    /// apply. Revocation and expiry name their ids for exactly that
    /// reason; teardown must too -- a bare count says how many authorities
    /// died, never which.
    #[test]
    fn teardown_names_every_grant_row_it_removed() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = granted_rig(&verifier, 20, 0);
        // A second granted petition, so "which rows died" is a real
        // question rather than a one-row triviality.
        let second = petition_at(20);
        client.send_message(&second.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 21, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 20).outcome, Outcome::Granted);

        let rows = [
            server.grant_row_id(4).expect("first row"),
            server.grant_row_id(20).expect("second row"),
        ];
        assert_eq!(shared.grants.rows(shared.now).count(), 2);

        server.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );

        let entries = shared.log();
        let removed = crate::recorder::tests::of_kind(&entries, "grant_removed");
        let named: Vec<&str> = removed.iter().map(|e| e.str("grant_id")).collect();
        let expected: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            named,
            expected.iter().map(String::as_str).collect::<Vec<_>>(),
            "every row teardown deleted is named, not just counted"
        );
        for e in &removed {
            assert_eq!(e.str("transition"), "active_to_removed");
            assert_eq!(e.str("source"), "connection_teardown");
            assert_eq!(e.str("connection"), "conn-1");
        }
        // The aggregate stays, as the summary it always was.
        let teardown = crate::recorder::tests::of_kind(&entries, "connection_teardown");
        assert_eq!(teardown[0].u64("removed_grants"), 2);
    }

    /// A consent decision is consumed from the pending registry *before*
    /// delivery is attempted, so a refused delivery destroys it: the
    /// petition is gone, the handle never resolves, and nothing retries.
    /// Without a record, a human's yes or no becomes unrecoverable -- and
    /// the issue requires consent decisions to be recorded.
    #[test]
    fn a_consent_decision_that_could_not_be_delivered_is_still_recorded() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        // A human says YES, narrowing to observe.
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
        // ... and the petitioner disconnects before the embedder routes it.
        server.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );
        let err = server
            .deliver_resolution(
                resolution,
                &mut shared.grants,
                &mut shared.recorder,
                shared.now,
                &mut |_, _| Ok(()),
            )
            .unwrap_err();
        assert!(matches!(err, DeliveryError::ConnectionDead));

        let entries = shared.log();
        // No authority changed, so there is no `petition_resolved` ...
        assert!(
            crate::recorder::tests::of_kind(&entries, "petition_resolved").is_empty(),
            "nothing was granted, so nothing may claim to have been"
        );
        // ... but the decision itself survives in the log.
        let lost = crate::recorder::tests::of_kind(&entries, "petition_undelivered");
        assert_eq!(lost.len(), 1, "the yes must not vanish");
        assert_eq!(lost[0].str("outcome"), "granted", "the human said yes");
        assert_eq!(lost[0].str("reason"), "connection_dead");
        assert_eq!(lost[0].str("issuer"), "scripted_consent");
        assert_eq!(lost[0].u64("grant_wire_id"), 4);
        assert_eq!(
            lost[0].strings("effective.verbs"),
            vec!["observe"],
            "including what the decision would have conferred"
        );
        assert!(
            lost[0].is_null("grant_id"),
            "no row was minted, so none may be named"
        );
    }

    /// The other undelivered shape: a decision routed to the wrong
    /// connection. Same funnel, different class -- the point being that
    /// the funnel covers every refusal path, not a hand-picked one.
    #[test]
    fn a_misrouted_resolution_is_recorded_with_its_own_cause_class() {
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
        let mut resolution = shared
            .petitions
            .resolve_scripted(petition, ScriptedDecision::Deny)
            .unwrap();
        // The embedder routes it to a connection that is not its own.
        resolution.connection = ConnectionId::from_u64_for_test(999);
        let err = server
            .deliver_resolution(
                resolution,
                &mut shared.grants,
                &mut shared.recorder,
                shared.now,
                &mut |_, _| Ok(()),
            )
            .unwrap_err();
        assert!(matches!(err, DeliveryError::WrongConnection { .. }));

        let entries = shared.log();
        let lost = crate::recorder::tests::of_kind(&entries, "petition_undelivered");
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].str("reason"), "wrong_connection");
        assert_eq!(lost[0].str("outcome"), "denied", "the human said no");
        assert!(lost[0].is_null("effective"));
    }

    /// The DoS the flight recorder must not become: the chokepoint refuses
    /// `not_granted` at its FIRST step, before the token bucket, so a
    /// facet whose grant never resolved granted is judged with no rate
    /// ceiling at all. The wire coalesces those refusals; the log must be
    /// bounded too, or an ungranted principal gets an unbounded,
    /// unratelimited disk-growth and compositor-stall vector for free.
    #[test]
    fn an_ungranted_principals_refusal_flood_cannot_grow_the_log_without_bound() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        // Interactive policy: the petition pends forever, so every facet
        // use is refused `not_granted` with no rate ceiling on the path.
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        const FLOOD: usize = 2_000;
        for _ in 0..FLOOD {
            client.send_message(&move_to(1, 1), None).unwrap();
            process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        }
        let during_flood = std::fs::metadata(&shared.log_path).unwrap().len();
        // Teardown ends the run and flushes its outstanding count, so
        // nothing is left unaccounted for when the session's story ends.
        server.teardown(
            &mut shared.petitions,
            &mut shared.grants,
            &mut shared.recorder,
        );

        let entries = shared.log();
        let uses = crate::recorder::tests::of_kind(&entries, "use_decision");
        assert_eq!(
            uses.len(),
            1,
            "the condition is recorded once in full, not {FLOOD} times"
        );
        assert_eq!(uses[0].str("refusal"), "not_granted");
        // Never silent, though: the repeats are accounted for.
        let summaries = crate::recorder::tests::of_kind(&entries, "use_refusal_summary");
        let counted: u64 = summaries.iter().map(|s| s.u64("repeats")).sum();
        assert_eq!(counted + 1, FLOOD as u64, "every refusal is accounted for");
        assert_eq!(
            summaries.last().unwrap().u64("total_in_run"),
            FLOOD as u64,
            "and the run's total is stated outright"
        );
        assert!(
            during_flood < 4_096,
            "a {FLOOD}-request flood wrote {during_flood} bytes; it must stay bounded"
        );
    }

    /// A facet whose grant never resolved `granted` has no row at all, and
    /// the entry says so with an explicit null rather than inventing one --
    /// the honest counterpart to a refusal that *does* name its row.
    #[test]
    fn an_ungranted_facets_refusal_names_no_row() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        // Interactive policy: the petition pends, so the facets are inert.
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_refused(&mut client, 4, Verb::OBSERVE, Refusal::NotGranted);

        let entries = shared.log();
        let uses = crate::recorder::tests::of_kind(&entries, "use_decision");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].str("decision"), "refused");
        assert_eq!(uses[0].str("refusal"), "not_granted");
        assert!(
            uses[0].is_null("grant_id"),
            "a pending grant has no row, so the entry must not invent one"
        );
        assert_eq!(uses[0].u64("grant_wire_id"), 4, "but the handle is named");
        assert!(uses[0].is_null("frame"));
    }

    /// A refused handshake is recorded with the cause class, the *claimed*
    /// identity (client-controlled, exactly escaped), and the credential's
    /// length only -- never its bytes. The wire stays uniform.
    #[test]
    fn a_refused_handshake_is_recorded_without_the_credential() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        // A hostile claimed identity: quotes, a brace, a control char, and
        // an attempt to forge a second `identity` member.
        // Quotes, a brace, a newline, DEL, and an attempt to forge a
        // second `identity` member. (No NUL: the wire decoder rejects an
        // embedded NUL as `invalid_argument` before verification runs, so
        // that byte can never reach the recorder from a hello -- the
        // NUL case is covered by the recorder's own escaper tests.)
        let hostile = "vitrin://local/{\", \"identity\": \"admin\n\u{7f}";
        let secret = "super-secret-credential-bytes-0123456789";
        send_hello(&mut client, 2, hostile, secret);
        expect_violation(
            process_n(&mut server, &mut core, &verifier, &mut shared, 1),
            "auth_failed",
        );
        expect_error(&mut client, WireError::AuthFailed);

        let entries = shared.log();
        let refused = crate::recorder::tests::of_kind(&entries, "handshake_refused");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].str("cause_class"), "unknown_identity");
        assert_eq!(
            refused[0].str("claimed_identity"),
            hostile,
            "recorded exactly, escaped -- never trusted, never reshaped"
        );
        assert!(
            refused[0].is_null("identity"),
            "nothing bound, so there is no canonical identity to state"
        );
        assert_eq!(refused[0].u64("credential_bytes"), secret.len() as u64);
        let raw = std::fs::read_to_string(&shared.log_path).unwrap();
        assert!(
            !raw.contains(secret),
            "credential bytes must never be logged"
        );
    }

    /// A `once` grant's spend and the two grant-lifecycle sweeps
    /// (proactive expiry, revocation) are recorded through the same single
    /// handle -- the transitions a grant undergoes with no wire traffic at
    /// all, which are exactly the ones a log must not miss.
    #[test]
    fn grant_lifecycle_transitions_without_wire_traffic_are_recorded() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let mut shared = Shared::new(ConsentPolicy::AutoApprove);
        let (mut server, mut core, mut client) = connect(&mut shared);
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);

        // A `once` grant, spent by its first capture.
        let mut once = petition_frame();
        once.persistence = WirePersistence::Once;
        client.send_message(&once.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::Granted);
        let once_row = server.grant_row_id(4).unwrap();

        client.send_message(&capture_frame(), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_frame(&mut client, 6);

        let so_far = shared.log();
        let spent = crate::recorder::tests::of_kind(&so_far, "grant_spent")
            .iter()
            .map(|e| e.str("grant_id").to_string())
            .collect::<Vec<_>>();
        assert_eq!(spent, vec![once_row.to_string()]);

        // A second, time-bounded grant that the proactive sweep kills
        // without any use touching it.
        let mut timed = petition_at(20);
        timed.expiry_ms = 1_000;
        client.send_message(&timed.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 21, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 20).outcome, Outcome::Granted);
        let timed_row = server.grant_row_id(20).unwrap();

        let expired = shared
            .grants
            .expire_due(shared.now + Duration::from_secs(2));
        assert_eq!(expired, vec![timed_row]);
        shared.recorder.record_expiry_sweep(&expired);

        // And a revocation, both shapes.
        let mut third = petition_at(30);
        third.persistence = WirePersistence::WhileRunning;
        client.send_message(&third.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 31, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 30).outcome, Outcome::Granted);
        let third_row = server.grant_row_id(30).unwrap();
        assert!(shared.grants.revoke(third_row));
        shared.recorder.record_revocations(
            &[third_row],
            crate::recorder::REVOKE_SCOPE_GRANT,
            crate::recorder::REVOKE_CAUSE_OPERATOR,
        );
        // The dead-man switch names every row it newly revoked -- the
        // spent `once` and the swept-expired one included (the table's
        // documented "revoking an expired or spent grant is permitted"),
        // but not the one already revoked above.
        let by_principal = shared
            .grants
            .revoke_principal(&PrincipalIdentity::parse(DEMO_IDENTITY).unwrap());
        assert_eq!(by_principal, vec![once_row, timed_row]);
        shared.recorder.record_revocations(
            &by_principal,
            crate::recorder::REVOKE_SCOPE_PRINCIPAL,
            crate::recorder::REVOKE_CAUSE_DEAD_MAN,
        );

        let entries = shared.log();
        let sweep = crate::recorder::tests::of_kind(&entries, "grant_expired");
        assert_eq!(sweep.len(), 1);
        assert_eq!(sweep[0].str("grant_id"), timed_row.to_string());
        assert_eq!(sweep[0].str("source"), "proactive_sweep");
        assert_eq!(sweep[0].str("transition"), "active_to_expired");

        let revoked = crate::recorder::tests::of_kind(&entries, "grant_revoked");
        assert_eq!(
            revoked
                .iter()
                .map(|e| (e.str("grant_id").to_string(), e.str("scope").to_string()))
                .collect::<Vec<_>>(),
            vec![
                (third_row.to_string(), "grant".to_string()),
                (once_row.to_string(), "principal".to_string()),
                (timed_row.to_string(), "principal".to_string()),
            ]
        );
    }

    /// A pending petition's `queued` transition and its `timed_out`
    /// terminal are both recorded, naming the pending petition while it
    /// exists -- the interactive path a demo without a consent surface
    /// actually takes.
    #[test]
    fn a_pending_petition_records_queued_then_its_timeout() {
        let _fd = crate::capture::tests::fd_lock();
        let verifier = demo_verifier();
        let (mut server, mut core, mut client, mut shared) = setup();
        bind_with_realm(&mut server, &mut core, &mut client, &verifier, &mut shared);
        client
            .send_message(&petition_frame().encode(3), None)
            .unwrap();
        process_n(&mut server, &mut core, &verifier, &mut shared, 1).unwrap();
        expect_consent_state(&mut client, 5, ConsentState::Queued);

        let so_far = shared.log();
        let queued = crate::recorder::tests::of_kind(&so_far, "consent_transition");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].str("state"), "queued");
        assert_eq!(queued[0].str("petition"), "petition-1");
        assert_eq!(queued[0].u64("consent_wire_id"), 5);

        shared.now += Duration::from_secs(120);
        let due = shared.petitions.expire_due(shared.now);
        assert_eq!(due.len(), 1);
        deliver(
            &mut server,
            &mut core,
            &mut shared,
            due.into_iter().next().unwrap(),
        );
        expect_consent_state(&mut client, 5, ConsentState::Closed);
        assert_eq!(expect_resolved(&mut client, 4).outcome, Outcome::TimedOut);

        let entries = shared.log();
        let transitions = crate::recorder::tests::of_kind(&entries, "consent_transition");
        assert_eq!(
            transitions
                .iter()
                .map(|e| e.str("state"))
                .collect::<Vec<_>>(),
            vec!["queued", "closed"]
        );
        // The prompt is down, so the closed transition names no petition.
        assert!(transitions[1].is_null("petition"));
        let resolved = crate::recorder::tests::of_kind(&entries, "petition_resolved");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].str("outcome"), "timed_out");
        assert!(resolved[0].is_null("effective"));
    }
}
