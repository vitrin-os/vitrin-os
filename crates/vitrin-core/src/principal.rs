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
//! # Scope seams (marked, not smuggled)
//!
//! - `vitrin_realm.request_grant` on a minted realm handle is **P1.4.3
//!   (issue #27)**: this build refuses it fatal `internal` with a logged
//!   "not implemented" -- honest server limitation, not a protocol
//!   judgement -- and #27 replaces that arm with the petition flow.
//! - The **unauthenticated deadline** (conventions 7.1 SHOULD) is a wall
//!   clock owned by the runtime wiring: nothing at runtime accepts
//!   principal connections yet (the listener wiring lands with M1.1
//!   integration), and the deadline is a calloop timer armed at accept and
//!   disarmed on [`is_bound`](PrincipalServer::is_bound) -- flagged in the
//!   task summary rather than half-built here.
//! - The flight recorder (P1.4.5) will observe handshakes through the same
//!   embedder that logs [`PrincipalFault`]s today.
//!
//! [`identity`]: crate::identity
//! [`Verifier`]: crate::identity::Verifier
//! [`Verifier::verify`]: crate::identity::Verifier::verify
//! [`VerifyOutcome`]: crate::identity::VerifyOutcome

use std::collections::BTreeMap;
use std::fmt;

use vitrin_ipc::{Message, PeerCred, TransportError};
use vitrin_protocol::error::DecodeError;
use vitrin_protocol::generated::vitrin_handshake as handshake;
use vitrin_protocol::generated::vitrin_handshake::Error as WireError;
use vitrin_protocol::generated::vitrin_principal as principal;
use vitrin_protocol::generated::vitrin_realm as realm;
use vitrin_protocol::generated::PROTOCOL_VERSION;

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
    /// conventions' fatal code via [`DecodeError::to_wire_error`].
    Malformed(DecodeError),
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
            PrincipalViolation::Malformed(e) => e.to_wire_error(),
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
            PrincipalViolation::InvalidObject { detail, .. } => (*detail).into(),
            PrincipalViolation::Malformed(e) => e.to_string(),
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
            PrincipalViolation::Malformed(e) => write!(f, "malformed message: {e}"),
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

    /// Dispatch one decoded frame from the principal connection.
    ///
    /// `Err` means the connection must die: the wire goodbye
    /// (`vitrin_handshake.error`) has already been sent best-effort for
    /// protocol violations (never for transport faults -- the queue that
    /// would carry it is the thing that failed), the violation has been
    /// logged, and the embedder closes the connection and dispatches
    /// nothing further.
    pub fn handle_message<F>(
        &mut self,
        msg: Message,
        verifier: &dyn Verifier,
        send: &mut F,
    ) -> Result<(), PrincipalFault>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        let result = self.dispatch(msg, verifier, send);
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
        verifier: &dyn Verifier,
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
                            let (_, sync) = handshake::requests::Sync::decode(&msg.bytes, msg.fd)
                                .map_err(PrincipalViolation::Malformed)?;
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
                        // The P1.4.3 seam: the petition flow (grant + consent
                        // + facets) replaces this arm in issue #27. Until
                        // then the honest answer is a server-side limitation,
                        // fatal `internal` -- never a fake judgement on the
                        // petition.
                        realm::requests::RequestGrant::OPCODE => {
                            Err(PrincipalViolation::Unimplemented {
                                object_id,
                                what: "request_grant (P1.4.3)",
                            }
                            .into())
                        }
                        _ => Err(PrincipalViolation::UnknownOpcode { object_id, opcode }.into()),
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
        let (_, hello) = handshake::requests::Hello::decode(&msg.bytes, msg.fd)
            .map_err(PrincipalViolation::Malformed)?;
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
        let (_, req) = principal::requests::GetRealm::decode(&msg.bytes, msg.fd)
            .map_err(PrincipalViolation::Malformed)?;
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

    /// Receive and dispatch exactly `n` client messages on the core side.
    fn process_n(
        server: &mut PrincipalServer,
        core: &mut Connection,
        verifier: &dyn Verifier,
        n: usize,
    ) -> Result<(), PrincipalFault> {
        for _ in 0..n {
            let msg = core
                .recv_message()
                .expect("core receive")
                .expect("a message must be waiting");
            server.handle_message(msg, verifier, &mut |frame| core.send_message(frame, None))?;
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

    /// A schema-legal whole-realm petition (observe only), used only as a
    /// well-formed frame toward realm handles -- its resolution is #27.
    fn whole_realm_petition() -> realm::requests::RequestGrant {
        realm::requests::RequestGrant {
            grant: 4,
            consent: 5,
            view: 6,
            pointer: 7,
            text: 8,
            resource: String::new(),
            verbs: vitrin_protocol::generated::vitrin_grant::Verb::OBSERVE,
            expiry_ms: 0,
            max_event_rate: 0,
            persistence: vitrin_protocol::generated::vitrin_grant::Persistence::Once,
            flags: 0,
        }
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
        let uid_mismatch = StaticVerifier::from_rows(
            vec![StaticPrincipal {
                identity: PrincipalIdentity::parse(DEMO_IDENTITY).unwrap(),
                token: TOKEN.as_bytes().to_vec(),
                uid: Some(my_uid().wrapping_add(1)),
            }],
            my_uid(),
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
        expect_error(&mut client, WireError::InvalidArgument);
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

    #[test]
    fn request_grant_is_the_p143_seam() {
        // Until issue #27 lands, a petition on a legal realm handle is an
        // honest server-side limitation: fatal internal, logged -- never a
        // fake resolution and never invalid_opcode (the opcode is defined).
        let verifier = demo_verifier();
        let (mut server, mut core, mut client) = setup();
        bind(&mut server, &mut core, &mut client, &verifier);
        send_get_realm(&mut client, 2, 3, "realm-0");
        let grant = whole_realm_petition();
        client.send_message(&grant.encode(3), None).unwrap();
        process_n(&mut server, &mut core, &verifier, 1).unwrap();
        expect_violation(
            process_n(&mut server, &mut core, &verifier, 1),
            "not implemented",
        );
        expect_error(&mut client, WireError::Internal);
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
        let foreign = whole_realm_petition();
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
