//! The flight-recorder log v0 (P1.4.5, issue #29): the journal seed --
//! a JSON-lines structured event log recording handshakes, grant lifecycle
//! transitions, consent decisions, and **every** enforcement decision, with
//! an observation digest on every delivered capture and epoch-ready
//! reference fields present-but-null from day one (backward requirement
//! B1).
//!
//! # What this is NOT
//!
//! **Not the signed P6 journal.** No signatures, no hash chain, no tamper
//! evidence, no append-only guarantee beyond what the filesystem gives.
//! This is a debugging aid and the future journal's *schema seed*: PRD Doc
//! 2 §3.1 makes the journal a first-class object ("an append-only signed
//! log handle") and Phase-3 E3.4 builds replay on it. E3.4 is unbuildable
//! retroactively if MVP records do not identify what was observed, so the
//! record shape lands now and the cryptography lands with P6. Nothing in
//! this module is consulted by an authority decision -- the enforcement
//! chokepoint ([`crate::enforcement`]) never reads the recorder, and the
//! recorder never answers a question.
//!
//! # Placement: an observer, never a second voice
//!
//! The recorder sits *outside* the chokepoint. `enforce_use` summarizes its
//! own decision in the returned [`UseOutcome`](crate::enforcement::UseOutcome)
//! -- the seam P1.4.4 left for exactly this -- and the connection server
//! records that summary at the single funnel it already owns
//! (`PrincipalServer::serve_facet_use`). So `enforcement.rs` gains no
//! recorder call at all: authority code stays authority code, and the
//! module's grep-provable single-path test is untouched. Every other
//! emission point follows the same rule -- the recorder is wired at sites
//! that *already* were the single site for the thing being recorded
//! (`handle_hello`'s verify arm, `deliver_resolution`'s flip, `teardown`),
//! so no event can be recorded twice and none can be missed by adding a
//! second path.
//!
//! One [`Recorder`] handle per core process, threaded through
//! `ServerCtx::recorder`; there are deliberately no ad-hoc writes anywhere
//! in the crate.
//!
//! # The entry schema (stable, versioned, snake_case)
//!
//! One JSON object per line, `\n`-terminated. Every line carries the
//! envelope:
//!
//! | field | type | meaning |
//! |---|---|---|
//! | `schema_version` | int | [`SCHEMA_VERSION`]; bumped when a field's *meaning* changes (adding a field does not bump it -- readers must ignore unknown fields) |
//! | `run_id` | string | `"<pid>-<unix-ms-at-start>"`; identifies the run even if two runs share a file |
//! | `seq` | int | strictly increasing from 1 within a run: **the ordering authority** |
//! | `mono_us` | int | microseconds since the recorder opened (monotonic; never jumps) |
//! | `wall_ms` | int | Unix-epoch milliseconds (human orientation only; may jump) |
//! | `kind` | string | the entry kind, one of the [`Event`] variants below |
//!
//! **Clock discipline is deliberately inverted here.** The grant table, the
//! petition registry, and the chokepoint never read a clock -- they take an
//! injected `now`. The recorder does the opposite and reads real time
//! itself, because it records *when the core actually did the thing*: a
//! test (or a future replay harness) that injects a synthetic instant must
//! not be able to write a synthetic timestamp into the log. `seq` -- not
//! either timestamp -- is what a reader orders by.
//!
//! ## Epoch-ready reference fields (B1; consumed by Phase-3 E3.4)
//!
//! Epochs do not exist before Phase 2 (PRD Doc 2 §7: "every observation
//! returns an epoch; every action carries `expected_epoch`"), but the
//! schema slots must exist now or replay cannot be built retroactively.
//! Every `use_decision` entry therefore carries an `epoch` object whose
//! members are all `null` in v0:
//!
//! - `epoch.observed` -- the monotonic epoch token stamping the observation
//!   this entry delivered (capture entries). Phase 2 fills it from the same
//!   token `capture_frame` returns to the agent, so a replay can pair a
//!   logged frame with the tree state it was consistent with.
//! - `epoch.expected` -- the `expected_epoch` an actuation carried
//!   (actuation entries). Phase 2 fills it; Phase 3 replay uses it to
//!   re-check the CAS decision the core made.
//! - `epoch.target` -- the reference the epoch applies to (a semantic-tree
//!   node id once trees exist). Null in v0 for a second, independent
//!   reason: version 1 grants whole-realm authority and has no semantic
//!   tree, so no target finer than the realm is expressible.
//!
//! A `null` here means "this version could not state it", never "it was
//! absent". The fields are on every `use_decision`, capture and actuation
//! alike, so a reader never has to branch on entry kind to find them.
//!
//! ## Observation digests (B1)
//!
//! Every entry for an **admitted capture** carries a `frame` object:
//! `width`, `height`, `stride`, `format` (`"xrgb8888"`, pinned by the IDL
//! in version 1), `bytes`, `digest_alg`, and `digest` (lowercase hex). The
//! digest is computed over **the exact bytes delivered to the agent** --
//! the sealed memfd's contents, after the RGBA→xrgb8888 swizzle -- so a
//! replay can prove that a given frame file is the frame this entry
//! records.
//!
//! **Never sampled.** B1 is "every capture carries a digest", and a sampled
//! digest identifies a sampled session. The digest is therefore computed
//! unconditionally on the capture copy path, inside
//! [`crate::capture::render_frame`], which is the only place holding the
//! delivered bytes without re-reading the memfd. A refused capture has
//! `"frame": null` -- honest: nothing was delivered, so there is nothing to
//! identify.
//!
//! **The cost, honestly.** One extra linear pass over the frame. Measured
//! on the default 1280x800 headless view (4,096,000 bytes) on a 13th-gen
//! Core i9: ~1.0 ms/frame at ~4 GB/s. For scale, that capture already pays
//! a swizzle pass (4 MB read + 4 MB written) and a 4 MB `write(2)` into the
//! memfd, so the digest is roughly a third of the per-capture copy cost --
//! real, but not the dominant term, and bounded by the grant's own
//! `max_event_rate` ceiling like every other capture cost. The alternative
//! (digest a subset) was rejected outright: it is exactly the shape B1
//! forbids.
//!
//! ## Secrecy contract (inherited from [`crate::identity`])
//!
//! The recorder MUST NOT contain credential bytes; at most
//! `credential_type` and `credential_bytes` (a length). This module has no
//! way to express a credential -- [`Event::HandshakeRefused`] takes a
//! `usize` length, not bytes -- so the rule is structural, not a
//! convention.
//!
//! Verifier-canonical identities ([`PrincipalIdentity`]) are loggable: they
//! are the server's own value, shape-validated at construction. A raw
//! **claimed** identity from a rejected handshake is client-controlled
//! hostile input; it is recorded (an operator debugging a failed handshake
//! needs to see what was presented) but only through the same exact escaper
//! as every other string, and it is never parsed, trusted, or joined
//! against anything. Rejection *causes* are recorded as a fixed class label
//! ([`Event::HandshakeRefused::cause_class`]), never the free-form `Display`
//! text -- which for `UnsupportedScheme` would embed client text a second
//! time.
//!
//! # Decisions this task settles
//!
//! **Digest algorithm: BLAKE3, tagged in the schema.** The rationale for
//! preferring it over SHA-256 (worst-case throughput without SHA-NI) and
//! the `default-features = false, features = ["pure"]` posture (no `std`,
//! no vendored assembly, no C toolchain in the TCB build) are recorded on
//! the dependency in `Cargo.toml`. Not hand-rolled: hand-rolled crypto is
//! the wrong trade even for a non-adversarial digest, and the algorithm tag
//! makes swapping it a schema-visible, additive change.
//!
//! **Write failures degrade loudly; they never halt captures or
//! actuations.** Argued from what v0 *is*. If a recorder write failure
//! blocked new captures and actuations, a full disk would become a denial
//! of authority, and the log would be load-bearing for enforcement --
//! precisely the property the *signed* journal will have and this one
//! explicitly does not. So: the first write error logs `tracing::error!`
//! once, latches the recorder degraded (no further write attempt is made --
//! a full disk must not produce an error storm at capture rate), and counts
//! every entry dropped from then on ([`Recorder::dropped_entries`]).
//! Degradation is announced three ways, because a truncated log is the one
//! thing that cannot describe itself: the `tracing::error!` at the moment
//! of failure, a second one naming the total at shutdown, and -- for a
//! reader who has only the file -- **a gap in `seq`**, since sequence
//! numbers are assigned to dropped entries too. Silence is what is
//! forbidden, not degradation. When P6's signed journal lands, *its*
//! failure policy is a separate decision and may well be fail-closed: a
//! signed journal is evidence, this is a debugging aid.
//!
//! **Creation failure at startup is fatal.** A different moment with a
//! different answer: an operator who asked for a flight recorder and cannot
//! have one must learn that before the session starts, not after it is
//! unreconstructable. [`Recorder::create`] returns a typed error and
//! `vitrind` exits non-zero.
//!
//! **Flush policy: one unbuffered `write(2)` per entry, no `fsync`.** The
//! entry is assembled fully in memory and handed to a single `write_all` on
//! a file opened `O_APPEND`.
//!
//! - *Against buffering*: the most valuable entries in a debugging aid are
//!   the ones immediately before the crash, and a `BufWriter` loses exactly
//!   those. With an unbuffered write the bytes are in the kernel before
//!   `record` returns, so a panic, `SIGKILL`, or `abort()` loses **nothing**.
//! - *Against `fsync`*: a debugging aid does not owe the operator a disk
//!   barrier per event; that trades ~1 µs/entry for ~1 ms/entry to defend
//!   against machine-level power loss, which is out of scope here.
//! - *Line atomicity*: one entry is one complete `\n`-terminated line in
//!   one `write` on an `O_APPEND` fd, which POSIX makes atomic with respect
//!   to other appenders -- so even two `vitrind` runs pointed at one file
//!   interleave whole lines, never fragments (and `run_id` tells them
//!   apart).
//! - *Crash truncation*: the only way to get a partial line is a write that
//!   is interrupted or fails part-way through (`write_all` looping over a
//!   short write, then erroring). A reader MUST therefore tolerate a
//!   trailing partial final line; every complete line is complete.
//!
//! **The file is opened append, never truncated.** A default path carries
//! the pid, so runs get their own files; an operator who deliberately
//! points two runs at one path gets an interleaved-but-parseable log rather
//! than a clobbered one. Mode `0600` at creation: the log carries principal
//! identities and grant metadata -- session metadata, not secrets, but not
//! world-readable either.
//!
//! **The JSON emitter is hand-rolled** (plan risk R7: no serialization
//! framework in the TCB, and this is a closed set of ~12 entry shapes). Its
//! one sharp edge -- string escaping -- is a single function,
//! [`push_json_string`], exhaustively tested against every C0 control, DEL,
//! quote, backslash, and multi-byte sequence, with round-trip assertions
//! through an in-test JSON reader.
//!
//! # Out of scope (named, not smuggled)
//!
//! Signatures and tamper evidence (P6), replay tooling (E3.4), epochs
//! themselves (Phase 2), log rotation, and any wire-protocol change: the
//! flight recorder is not wire-visible and this task has zero protocol
//! impact.

use std::fmt::Write as _;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use vitrin_ipc::PeerCred;
use vitrin_protocol::generated::vitrin_consent::ConsentState;
use vitrin_protocol::generated::vitrin_grant::{
    Outcome, Persistence as WirePersistence, Refusal, Verb,
};

use crate::enforcement::UseOutcome;
use crate::grants::{GrantId, Issuer, PersistenceRung};
use crate::identity::{PrincipalIdentity, RejectionCause};
use crate::petitions::{ConnectionId, EffectiveAuthority, PetitionId};

/// The entry-schema version stamped on every line. Bump only when an
/// existing field's *meaning* changes; adding a field is additive, and a
/// reader must ignore fields it does not know (the same growth rule the
/// wire protocol uses).
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// The observation-digest algorithm tag written as `frame.digest_alg`.
/// Present so the algorithm is swappable without a schema break.
pub(crate) const DIGEST_ALG: &str = "blake3";

/// The version-1 capture pixel format, pinned by the IDL
/// (`vitrin_view.frame_ready`) and written verbatim so a replay never has
/// to infer it.
const FRAME_FORMAT: &str = "xrgb8888";

/// [`Event::GrantRevoked::scope`] for a single named grant (panel/policy).
pub(crate) const REVOKE_SCOPE_GRANT: &str = "grant";

/// [`Event::GrantRevoked::scope`] for every grant of one principal (the
/// hold-Esc dead-man switch, P1.7.3).
pub(crate) const REVOKE_SCOPE_PRINCIPAL: &str = "principal";

// ---------------------------------------------------------------------------
// Observation digest
// ---------------------------------------------------------------------------

/// The digest identifying one delivered observation (backward requirement
/// B1). Computed over the exact bytes handed to the agent -- see the module
/// docs for why this is unconditional and never sampled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObservationDigest([u8; 32]);

impl ObservationDigest {
    /// Digest `bytes` -- the sealed memfd's contents, post-swizzle.
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Lowercase hex, the schema's `frame.digest` rendering.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(self.0.len() * 2);
        for byte in self.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

impl std::fmt::Debug for ObservationDigest {
    /// Renders as the tagged schema form so a `{:?}` in a diagnostic and
    /// the log line agree.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{DIGEST_ALG}:{}", self.to_hex())
    }
}

/// Everything a `use_decision` entry states about a frame that was actually
/// delivered: the geometry the agent received plus its digest. Built by the
/// enforcement chokepoint from the `frame_ready` it just sent, so the entry
/// describes the delivered artifact, not the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObservedFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// Size of the sealed memfd -- `stride * height` exactly (IDL).
    pub bytes: u64,
    pub digest: ObservationDigest,
}

// ---------------------------------------------------------------------------
// Entry kinds
// ---------------------------------------------------------------------------

/// The requested authority of one petition, exactly as the wire stated it
/// (before admission resolves the `0` defaults) -- what
/// [`Event::PetitionRequested`] records.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequestedAuthority {
    pub verbs: Verb,
    pub persistence: WirePersistence,
    /// Wire-shaped: `0` = bounded by the rung.
    pub expiry_ms: u32,
    /// Wire-shaped: `0` = the server's default ceiling.
    pub max_event_rate: u32,
    /// The reserved constraint bits (`0` in every compliant version-1
    /// petition; a set bit resolves `unsupported`).
    pub flags: u32,
}

/// One recordable event. Borrowed throughout: an entry is serialized
/// immediately and never retained, so nothing is cloned to record it.
///
/// The set is closed by design (the hand-rolled emitter, plan risk R7);
/// adding a kind means adding a variant, a `kind` label, and a body writer
/// -- three edits the compiler forces together.
#[derive(Debug)]
pub(crate) enum Event<'a> {
    /// The run's header line: what produced this log.
    RunStarted {
        pid: u32,
        core_version: &'a str,
        /// `interactive` or `auto-approve` -- the loudly-flagged policy
        /// (plan risk R6) belongs in the reconstruction.
        consent_policy: &'a str,
    },
    /// The run's footer line, written on a clean shutdown.
    RunEnded {
        /// Entries lost after a write failure latched the recorder
        /// degraded (module docs: loud degradation, never silence).
        dropped_entries: u64,
    },
    /// A `hello` bound a principal: the verifier-canonical identity is now
    /// what every grant row and enforcement decision keys on.
    HandshakeBound {
        connection: ConnectionId,
        peer: PeerCred,
        identity: &'a PrincipalIdentity,
        credential_type: &'a str,
        /// Length only -- **never** the bytes (secrecy contract).
        credential_bytes: usize,
    },
    /// A `hello` was refused. Wire-uniform `auth_failed`; the class here is
    /// the log's private cause taxonomy.
    HandshakeRefused {
        connection: ConnectionId,
        peer: PeerCred,
        /// A fixed label from [`auth_cause_class`] -- never free-form
        /// `Display` text, which can embed client-controlled strings.
        cause_class: &'static str,
        /// Client-controlled hostile input: escaped exactly, never trusted.
        claimed_identity: &'a str,
        /// Also client-controlled; same treatment.
        credential_type: &'a str,
        /// Length only -- **never** the bytes (secrecy contract).
        credential_bytes: usize,
    },
    /// A petition reached admission: what it asked for. Emitted before the
    /// policy decision, so a `busy`/`unsupported`/`unavailable` refusal
    /// still leaves a record of the request.
    PetitionRequested {
        connection: ConnectionId,
        identity: &'a PrincipalIdentity,
        /// The name the realm handle was minted with -- client-controlled
        /// until admission judges it.
        realm_name: &'a str,
        grant_wire_id: u32,
        consent_wire_id: u32,
        /// Empty = whole realm; any other value resolves `unsupported` in
        /// version 1. Client-controlled.
        resource: &'a str,
        requested: RequestedAuthority,
    },
    /// A consent-prompt lifecycle transition (`queued`/`shown`/`closed`).
    ConsentTransition {
        connection: ConnectionId,
        consent_wire_id: u32,
        state: ConsentState,
        /// The pending-petition id once one exists (`None` for a policy
        /// resolution that never pended).
        petition: Option<PetitionId>,
    },
    /// A petition resolved: the outcome, and for `granted` the **effective**
    /// authority the row states (possibly narrower than requested) plus the
    /// row id authority is now keyed by.
    PetitionResolved {
        connection: ConnectionId,
        grant_wire_id: u32,
        outcome: Outcome,
        /// `Some` exactly for `granted`.
        effective: Option<EffectiveAuthority>,
        /// The grant-table row minted at this instant; `Some` exactly for
        /// `granted`.
        grant_id: Option<GrantId>,
        /// Which consent path decided; `Some` exactly for `granted`.
        issuer: Option<Issuer>,
    },
    /// One enforcement-chokepoint decision -- allowed or refused, capture
    /// or actuation. The B1 fields (`frame`, `epoch`) live here.
    UseDecision {
        connection: ConnectionId,
        /// The facet object the request arrived on.
        facet_wire_id: u32,
        /// The co-minted grant handle (`refused` addresses it).
        grant_wire_id: u32,
        /// The verb exercised -- one bit, by construction (one facet, one
        /// verb).
        verb: Verb,
        /// The grant-table row behind the facet's handle, as the caller
        /// resolved it before the chain ran. `None` for a facet whose
        /// grant never resolved `granted` -- which has no row at all, and
        /// is exactly the `not_granted` case. Carried separately from
        /// [`UseOutcome`] so a *refused* use still names the row it was
        /// judged against (a rate-limited or revoked use has one; the
        /// outcome alone does not report it).
        grant_row: Option<GrantId>,
        outcome: &'a UseOutcome,
    },
    /// A `once` grant's single use was consumed by an admitted use: the
    /// active-to-spent lifecycle transition.
    GrantSpent {
        connection: ConnectionId,
        grant_id: GrantId,
    },
    /// The proactive expiry sweep found this row dead without a use
    /// ([`GrantTable::expire_due`](crate::grants::GrantTable::expire_due)).
    GrantExpired { grant_id: GrantId },
    /// A grant was revoked (panel, policy, or the hold-Esc dead-man switch).
    GrantRevoked {
        grant_id: GrantId,
        /// `grant` for a single revoke, `principal` for the sweep over one
        /// principal's rows.
        scope: &'static str,
    },
    /// A principal connection closed: its pending petitions were withdrawn
    /// and its grants died with it.
    ConnectionTeardown {
        connection: ConnectionId,
        /// `None` when the connection never bound.
        identity: Option<&'a PrincipalIdentity>,
        withdrawn_petitions: usize,
        removed_grants: usize,
    },
}

impl Event<'_> {
    /// The `kind` label. Stable: a reader switches on this string.
    fn kind(&self) -> &'static str {
        match self {
            Event::RunStarted { .. } => "run_started",
            Event::RunEnded { .. } => "run_ended",
            Event::HandshakeBound { .. } => "handshake_bound",
            Event::HandshakeRefused { .. } => "handshake_refused",
            Event::PetitionRequested { .. } => "petition_requested",
            Event::ConsentTransition { .. } => "consent_transition",
            Event::PetitionResolved { .. } => "petition_resolved",
            Event::UseDecision { .. } => "use_decision",
            Event::GrantSpent { .. } => "grant_spent",
            Event::GrantExpired { .. } => "grant_expired",
            Event::GrantRevoked { .. } => "grant_revoked",
            Event::ConnectionTeardown { .. } => "connection_teardown",
        }
    }

    /// Append this entry's kind-specific fields to the open envelope
    /// object. Every branch writes a fixed field set: absent information is
    /// an explicit `null`, never a missing key, so a reader never has to
    /// distinguish "this version could not say" from "the writer forgot".
    fn write_body(&self, out: &mut String) {
        match *self {
            Event::RunStarted {
                pid,
                core_version,
                consent_policy,
            } => {
                field_u64(out, "pid", u64::from(pid));
                field_str(out, "core_version", core_version);
                field_str(out, "consent_policy", consent_policy);
                // Stated once per run so a reader knows the digest
                // vocabulary before it meets the first capture.
                field_str(out, "digest_alg", DIGEST_ALG);
            }
            Event::RunEnded { dropped_entries } => {
                field_u64(out, "dropped_entries", dropped_entries);
            }
            Event::HandshakeBound {
                connection,
                peer,
                identity,
                credential_type,
                credential_bytes,
            } => {
                field_display(out, "connection", connection);
                write_peer(out, peer);
                field_str(out, "identity", identity.as_str());
                field_str(out, "credential_type", credential_type);
                field_u64(out, "credential_bytes", credential_bytes as u64);
            }
            Event::HandshakeRefused {
                connection,
                peer,
                cause_class,
                claimed_identity,
                credential_type,
                credential_bytes,
            } => {
                field_display(out, "connection", connection);
                write_peer(out, peer);
                field_str(out, "cause_class", cause_class);
                // The verifier bound no identity, so there is none to
                // state; the claimed string beside it is the client's.
                field_null(out, "identity");
                field_str(out, "claimed_identity", claimed_identity);
                field_str(out, "credential_type", credential_type);
                field_u64(out, "credential_bytes", credential_bytes as u64);
            }
            Event::PetitionRequested {
                connection,
                identity,
                realm_name,
                grant_wire_id,
                consent_wire_id,
                resource,
                requested,
            } => {
                field_display(out, "connection", connection);
                field_str(out, "identity", identity.as_str());
                field_str(out, "realm_name", realm_name);
                field_u64(out, "grant_wire_id", u64::from(grant_wire_id));
                field_u64(out, "consent_wire_id", u64::from(consent_wire_id));
                field_str(out, "resource", resource);
                open_object(out, "requested");
                write_verbs(out, requested.verbs);
                field_str(
                    out,
                    "persistence",
                    wire_persistence_label(requested.persistence),
                );
                field_u64(out, "expiry_ms", u64::from(requested.expiry_ms));
                field_u64(out, "max_event_rate", u64::from(requested.max_event_rate));
                field_u64(out, "flags", u64::from(requested.flags));
                close_object(out);
            }
            Event::ConsentTransition {
                connection,
                consent_wire_id,
                state,
                petition,
            } => {
                field_display(out, "connection", connection);
                field_u64(out, "consent_wire_id", u64::from(consent_wire_id));
                field_str(out, "state", consent_state_label(state));
                match petition {
                    Some(id) => field_display(out, "petition", id),
                    None => field_null(out, "petition"),
                }
            }
            Event::PetitionResolved {
                connection,
                grant_wire_id,
                outcome,
                effective,
                grant_id,
                issuer,
            } => {
                field_display(out, "connection", connection);
                field_u64(out, "grant_wire_id", u64::from(grant_wire_id));
                field_str(out, "outcome", outcome_label(outcome));
                match grant_id {
                    Some(id) => field_display(out, "grant_id", id),
                    None => field_null(out, "grant_id"),
                }
                match issuer {
                    Some(i) => field_str(out, "issuer", issuer_label(i)),
                    None => field_null(out, "issuer"),
                }
                match effective {
                    Some(e) => {
                        open_object(out, "effective");
                        write_verbs(out, e.verbs);
                        field_str(out, "persistence", rung_label(e.persistence));
                        field_u64(out, "expiry_ms", u64::from(e.expiry_ms));
                        field_u64(out, "max_event_rate", u64::from(e.max_event_rate.get()));
                        close_object(out);
                    }
                    None => field_null(out, "effective"),
                }
            }
            Event::UseDecision {
                connection,
                facet_wire_id,
                grant_wire_id,
                verb,
                grant_row,
                outcome,
            } => {
                field_display(out, "connection", connection);
                field_u64(out, "facet_wire_id", u64::from(facet_wire_id));
                field_u64(out, "grant_wire_id", u64::from(grant_wire_id));
                field_str(out, "verb", verb_label(verb));
                match *outcome {
                    UseOutcome::Admitted { grant, frame, .. } => {
                        field_str(out, "decision", "allowed");
                        // The admitting row itself -- authoritative, and by
                        // construction the same row `grant_row` names.
                        field_display(out, "grant_id", grant);
                        field_null(out, "refusal");
                        field_null(out, "refusal_voiced");
                        write_frame(out, frame);
                    }
                    UseOutcome::Refused { code, voiced } => {
                        field_str(out, "decision", "refused");
                        // The row the use was judged against, when one
                        // exists: a rate-limited, revoked, or expired use
                        // has a row and must name it. `null` only for a
                        // facet whose grant never resolved `granted`, which
                        // has no row -- the `not_granted` case.
                        match grant_row {
                            Some(id) => field_display(out, "grant_id", id),
                            None => field_null(out, "grant_id"),
                        }
                        field_str(out, "refusal", refusal_label(code));
                        // Whether the event actually reached the wire, or
                        // was coalesced away under the delivery
                        // classification's MAY-bounds.
                        field_bool(out, "refusal_voiced", voiced);
                        write_frame(out, None);
                    }
                }
                write_epoch_reference(out);
            }
            Event::GrantSpent {
                connection,
                grant_id,
            } => {
                field_display(out, "connection", connection);
                field_display(out, "grant_id", grant_id);
                field_str(out, "transition", "active_to_spent");
            }
            Event::GrantExpired { grant_id } => {
                field_display(out, "grant_id", grant_id);
                field_str(out, "transition", "active_to_expired");
                field_str(out, "source", "proactive_sweep");
            }
            Event::GrantRevoked { grant_id, scope } => {
                field_display(out, "grant_id", grant_id);
                field_str(out, "transition", "active_to_revoked");
                field_str(out, "scope", scope);
            }
            Event::ConnectionTeardown {
                connection,
                identity,
                withdrawn_petitions,
                removed_grants,
            } => {
                field_display(out, "connection", connection);
                match identity {
                    Some(id) => field_str(out, "identity", id.as_str()),
                    None => field_null(out, "identity"),
                }
                field_u64(out, "withdrawn_petitions", withdrawn_petitions as u64);
                field_u64(out, "removed_grants", removed_grants as u64);
            }
        }
    }
}

/// The `frame` member of a `use_decision`: the delivered observation's
/// identity (B1), or an explicit `null` when nothing was delivered.
fn write_frame(out: &mut String, frame: Option<ObservedFrame>) {
    match frame {
        Some(f) => {
            open_object(out, "frame");
            field_u64(out, "width", u64::from(f.width));
            field_u64(out, "height", u64::from(f.height));
            field_u64(out, "stride", u64::from(f.stride));
            field_str(out, "format", FRAME_FORMAT);
            field_u64(out, "bytes", f.bytes);
            field_str(out, "digest_alg", DIGEST_ALG);
            field_str(out, "digest", &f.digest.to_hex());
            close_object(out);
        }
        None => field_null(out, "frame"),
    }
}

/// The epoch-ready reference block (B1), null-versioned in v0. See the
/// module docs for each member's Phase-2/3 semantics; the shape is fixed
/// now so E3.4 replay can be written against it.
fn write_epoch_reference(out: &mut String) {
    open_object(out, "epoch");
    field_null(out, "observed");
    field_null(out, "expected");
    field_null(out, "target");
    close_object(out);
}

/// `peer_uid` / `peer_pid` -- the kernel-attested credentials the transport
/// recorded at accept. The pid is `null` when not mappable into the core's
/// pid namespace, and is diagnostic only: uid is the authority anchor.
fn write_peer(out: &mut String, peer: PeerCred) {
    field_u64(out, "peer_uid", u64::from(peer.uid));
    match peer.pid {
        Some(pid) => field_i64(out, "peer_pid", i64::from(pid)),
        None => field_null(out, "peer_pid"),
    }
}

/// `verbs` (names, for humans) plus `verbs_bits` (the exact bitmask, for
/// machines): a reader never has to infer one from the other, and an
/// unknown future bit is still visible in `verbs_bits`.
fn write_verbs(out: &mut String, verbs: Verb) {
    key(out, "verbs");
    out.push('[');
    let mut named = 0u32;
    for (bit, name) in [
        (Verb::OBSERVE, "observe"),
        (Verb::ACTUATE_POINTER, "actuate_pointer"),
        (Verb::ACTUATE_TEXT, "actuate_text"),
    ] {
        if verbs.contains(bit) {
            if named > 0 {
                out.push(',');
            }
            push_json_string(out, name);
            named += 1;
        }
    }
    out.push(']');
    field_u64(out, "verbs_bits", u64::from(verbs.bits()));
}

// ---------------------------------------------------------------------------
// Stable enum labels
// ---------------------------------------------------------------------------

/// The single verb a facet use exercises. Falls back to `unknown` rather
/// than panicking or lying if a future multi-bit value ever reaches here;
/// `verbs_bits` on petition entries always carries the exact mask.
fn verb_label(verb: Verb) -> &'static str {
    match verb {
        Verb::OBSERVE => "observe",
        Verb::ACTUATE_POINTER => "actuate_pointer",
        Verb::ACTUATE_TEXT => "actuate_text",
        _ => "unknown",
    }
}

fn refusal_label(code: Refusal) -> &'static str {
    match code {
        Refusal::NotGranted => "not_granted",
        Refusal::Expired => "expired",
        Refusal::Revoked => "revoked",
        Refusal::RateLimited => "rate_limited",
        Refusal::Preempted => "preempted",
        Refusal::ConsentHeld => "consent_held",
        Refusal::NoSurface => "no_surface",
        Refusal::Internal => "internal",
    }
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Granted => "granted",
        Outcome::Denied => "denied",
        Outcome::TimedOut => "timed_out",
        Outcome::Unavailable => "unavailable",
        Outcome::Unsupported => "unsupported",
        Outcome::Busy => "busy",
    }
}

fn consent_state_label(state: ConsentState) -> &'static str {
    match state {
        ConsentState::Queued => "queued",
        ConsentState::Shown => "shown",
        ConsentState::Closed => "closed",
    }
}

fn wire_persistence_label(p: WirePersistence) -> &'static str {
    match p {
        WirePersistence::Once => "once",
        WirePersistence::WhileRunning => "while_running",
        WirePersistence::UntilRevoked => "until_revoked",
        WirePersistence::Always => "always",
    }
}

fn rung_label(rung: PersistenceRung) -> &'static str {
    match rung {
        PersistenceRung::Once => "once",
        PersistenceRung::WhileRunning => "while_running",
    }
}

fn issuer_label(issuer: Issuer) -> &'static str {
    match issuer {
        Issuer::HumanConsent => "human_consent",
        Issuer::AutoApprovePolicy => "auto_approve_policy",
        #[cfg(any(test, feature = "scripted-consent"))]
        Issuer::ScriptedConsent => "scripted_consent",
    }
}

/// The recorder's private cause taxonomy for a refused handshake. The wire
/// stays uniform `auth_failed` (identity-probing resistance); this label
/// carries no client-controlled text, so a hostile `credential_type` cannot
/// smuggle itself in twice.
pub(crate) fn auth_cause_class(cause: &RejectionCause) -> &'static str {
    match cause {
        RejectionCause::UnsupportedScheme { .. } => "unsupported_scheme",
        RejectionCause::UnknownIdentity => "unknown_identity",
        RejectionCause::BadToken => "bad_token",
        RejectionCause::PeerCredMismatch { .. } => "peercred_mismatch",
    }
}

/// The class for the third handshake outcome: infrastructure failure, not a
/// judgement on the credential ([`crate::identity::VerifyOutcome::Unavailable`]).
pub(crate) const VERIFIER_UNAVAILABLE_CLASS: &str = "verifier_unavailable";

// ---------------------------------------------------------------------------
// The hand-rolled JSON emitter
// ---------------------------------------------------------------------------

/// Open a field: writes the separating comma (unless the enclosing object
/// is still empty) then the quoted key and its colon.
///
/// The "is the object empty" test is `out.ends_with('{')`, which is exact
/// by construction: a fresh object -- top level or nested -- leaves `{` as
/// the last byte, and no *value* this emitter writes can end with `{` (a
/// nested object is closed before its parent continues). That makes comma
/// placement structurally impossible to get wrong, which is the failure
/// mode a hand-rolled emitter actually has.
fn key(out: &mut String, k: &str) {
    if !out.ends_with('{') {
        out.push(',');
    }
    push_json_string(out, k);
    out.push(':');
}

fn field_str(out: &mut String, k: &str, v: &str) {
    key(out, k);
    push_json_string(out, v);
}

fn field_display(out: &mut String, k: &str, v: impl std::fmt::Display) {
    key(out, k);
    // Ids render through their canonical `Display` (`conn-1`, `grant-7`,
    // `petition-3`) -- the same tokens the core's `tracing` output uses, so
    // a human correlating the two sees identical strings. Escaped like any
    // other string even though these renderings are ASCII by construction.
    push_json_string(out, &v.to_string());
}

fn field_u64(out: &mut String, k: &str, v: u64) {
    key(out, k);
    let _ = write!(out, "{v}");
}

fn field_i64(out: &mut String, k: &str, v: i64) {
    key(out, k);
    let _ = write!(out, "{v}");
}

fn field_bool(out: &mut String, k: &str, v: bool) {
    key(out, k);
    out.push_str(if v { "true" } else { "false" });
}

fn field_null(out: &mut String, k: &str) {
    key(out, k);
    out.push_str("null");
}

fn open_object(out: &mut String, k: &str) {
    key(out, k);
    out.push('{');
}

fn close_object(out: &mut String) {
    out.push('}');
}

/// Write `s` as a JSON string literal, quotes included.
///
/// Escapes exactly: `"`, `\`, every C0 control (U+0000..=U+001F, using
/// JSON's two-character forms where they exist and `\u00XX` otherwise), and
/// DEL (U+007F). DEL is not *required* to be escaped by RFC 8259, but a raw
/// DEL in a log line is a terminal hazard and no reader loses information:
/// `` decodes back to exactly the same character.
///
/// Everything else passes through verbatim, including multi-byte UTF-8 and
/// the C1 controls (U+0080..=U+009F, which are legal JSON string content).
/// The input is `&str`, so it is valid UTF-8 by type and the output is
/// therefore always a valid JSON string -- there is no invalid-sequence
/// case to handle, and no truncation: the wire already bounds every string
/// that reaches here, and a silently truncated identity in a log would be a
/// lie about what was presented.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// The recorder
// ---------------------------------------------------------------------------

/// Why [`Recorder::create`] could not open the log. Always fatal at startup
/// (module docs): an operator who asked for a flight recorder and cannot
/// have one learns it before the session, not after.
#[derive(Debug)]
pub(crate) struct RecorderError {
    path: PathBuf,
    source: io::Error,
}

impl std::fmt::Display for RecorderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "flight-recorder log {} could not be opened for append: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for RecorderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The one flight-recorder handle of a core process (module docs: there are
/// deliberately no other write sites). One log file per run.
#[derive(Debug)]
pub(crate) struct Recorder {
    /// `None` once a write failure latched the recorder degraded -- no
    /// further write is attempted, so a full disk cannot produce an error
    /// storm at capture rate.
    file: Option<File>,
    path: PathBuf,
    run_id: String,
    /// Next `seq`; strictly increasing from 1, never reused, and assigned
    /// even for entries that are then dropped, so a gap in `seq` is exactly
    /// the evidence that entries were lost.
    seq: u64,
    started: Instant,
    dropped: u64,
}

impl Recorder {
    /// Open (creating if absent, never truncating) the run's log at `path`,
    /// creating any missing parent directories `0700` and the file `0600`.
    /// Fails loudly rather than degrading: see the module docs on why
    /// creation and mid-run write failures get different answers.
    pub fn create(path: &Path) -> Result<Self, RecorderError> {
        let fail = |source: io::Error| RecorderError {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(fail)?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(fail)?;
        Ok(Self::with_file(file, path.to_path_buf()))
    }

    /// The shared constructor behind [`Recorder::create`]: everything after
    /// the file exists. `run_id` is `<pid>-<unix-ms>`, unique enough to tell
    /// two runs apart inside one file without pulling a randomness
    /// dependency into the TCB.
    fn with_file(file: File, path: PathBuf) -> Self {
        Self {
            file: Some(file),
            path,
            run_id: format!("{}-{}", std::process::id(), wall_ms()),
            seq: 1,
            started: Instant::now(),
            dropped: 0,
        }
    }

    /// The run identifier stamped on every line of this run.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// How many entries were lost after a write failure latched the
    /// recorder degraded. Nonzero means the log is incomplete -- and a gap
    /// in `seq` marks exactly where.
    pub fn dropped_entries(&self) -> u64 {
        self.dropped
    }

    /// Whether a write failure has latched this recorder degraded.
    pub fn is_degraded(&self) -> bool {
        self.file.is_none()
    }

    /// Record one proactive expiry sweep's result -- the ids
    /// [`GrantTable::expire_due`](crate::grants::GrantTable::expire_due)
    /// just flipped, i.e. grants that died *without* a use and would
    /// otherwise appear in no entry at all. One call per sweep, so the
    /// M1.1 calloop timer has exactly one recorder call shape and no loop
    /// of its own; an empty sweep writes nothing.
    pub fn record_expiry_sweep(&mut self, expired: &[GrantId]) {
        for &grant_id in expired {
            self.record(Event::GrantExpired { grant_id });
        }
    }

    /// Record one revocation's result: the ids
    /// [`GrantTable::revoke`](crate::grants::GrantTable::revoke) or
    /// [`GrantTable::revoke_principal`](crate::grants::GrantTable::revoke_principal)
    /// newly revoked, tagged with which of the two acts it was
    /// ([`REVOKE_SCOPE_GRANT`] / [`REVOKE_SCOPE_PRINCIPAL`]).
    pub fn record_revocations(&mut self, revoked: &[GrantId], scope: &'static str) {
        for &grant_id in revoked {
            self.record(Event::GrantRevoked { grant_id, scope });
        }
    }

    /// Record one entry: assemble the whole line in memory, then hand it to
    /// a single `write(2)`. Infallible by design -- a diagnostic must never
    /// be able to fail an authority path (module docs), so a write failure
    /// degrades loudly here instead of propagating to the caller.
    pub fn record(&mut self, event: Event<'_>) {
        let seq = self.seq;
        self.seq += 1;
        let mut line = String::with_capacity(256);
        line.push('{');
        field_u64(&mut line, "schema_version", u64::from(SCHEMA_VERSION));
        field_str(&mut line, "run_id", &self.run_id);
        field_u64(&mut line, "seq", seq);
        // `u64::MAX` microseconds is ~584,000 years of uptime; the
        // saturating cast is a formality that keeps the TCB panic-free.
        field_u64(
            &mut line,
            "mono_us",
            self.started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        );
        field_u64(&mut line, "wall_ms", wall_ms());
        field_str(&mut line, "kind", event.kind());
        event.write_body(&mut line);
        line.push('}');
        line.push('\n');
        self.write_line(&line);
    }

    /// The single write site. One `write_all` of one complete, already
    /// `\n`-terminated line on an `O_APPEND` fd -- the line-atomicity
    /// property the module docs describe.
    fn write_line(&mut self, line: &str) {
        let Some(file) = self.file.as_mut() else {
            self.dropped += 1;
            return;
        };
        if let Err(err) = file.write_all(line.as_bytes()) {
            // Loud, once. The recorder is latched degraded so a full disk
            // cannot turn every subsequent capture into another error line.
            tracing::error!(
                path = %self.path.display(),
                error = %err,
                "flight recorder write failed; recording is now DEGRADED for the rest of \
                 this run (entries will be counted, not written). Captures and actuations \
                 are unaffected -- the flight recorder v0 is a debugging aid, not an \
                 authority input."
            );
            self.file = None;
            self.dropped += 1;
        }
    }
}

/// Unix-epoch milliseconds, or `0` if the system clock is before the epoch
/// (never a panic -- the TCB does not panic on a hostile clock).
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    // -- a minimal in-test JSON reader -------------------------------------
    //
    // The crate takes no serialization dependency, not even as a
    // dev-dependency (plan risk R7; the rejected-png precedent). These ~120
    // lines are what make the emitter's tests *real* rather than string
    // matching: every assertion below parses the line back as JSON, so a
    // missing comma, an unescaped control character, or a stray brace fails
    // loudly instead of being invisible to a `contains` check.

    /// A parsed JSON value.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum Json {
        Null,
        Bool(bool),
        Num(f64),
        Str(String),
        Arr(Vec<Json>),
        Obj(Vec<(String, Json)>),
    }

    impl Json {
        /// Parse one complete JSON value; trailing non-whitespace is an
        /// error, so "the line is exactly one JSON object" is asserted.
        pub fn parse(text: &str) -> Result<Json, String> {
            let bytes: Vec<char> = text.chars().collect();
            let mut at = 0usize;
            let value = parse_value(&bytes, &mut at)?;
            skip_ws(&bytes, &mut at);
            if at != bytes.len() {
                return Err(format!("trailing content at {at}"));
            }
            Ok(value)
        }

        /// Member lookup by dotted path (`"frame.digest"`).
        pub fn path(&self, dotted: &str) -> Option<&Json> {
            let mut cur = self;
            for part in dotted.split('.') {
                let Json::Obj(members) = cur else {
                    return None;
                };
                cur = &members.iter().find(|(k, _)| k == part)?.1;
            }
            Some(cur)
        }

        /// The member at `dotted`, panicking with the path if absent --
        /// assertions read better than `unwrap` chains.
        pub fn at(&self, dotted: &str) -> &Json {
            self.path(dotted)
                .unwrap_or_else(|| panic!("no member {dotted:?} in {self:?}"))
        }

        pub fn str(&self, dotted: &str) -> &str {
            match self.at(dotted) {
                Json::Str(s) => s,
                other => panic!("member {dotted:?} is not a string: {other:?}"),
            }
        }

        pub fn u64(&self, dotted: &str) -> u64 {
            match self.at(dotted) {
                Json::Num(n) => *n as u64,
                other => panic!("member {dotted:?} is not a number: {other:?}"),
            }
        }

        pub fn bool(&self, dotted: &str) -> bool {
            match self.at(dotted) {
                Json::Bool(b) => *b,
                other => panic!("member {dotted:?} is not a bool: {other:?}"),
            }
        }

        pub fn is_null(&self, dotted: &str) -> bool {
            matches!(self.at(dotted), Json::Null)
        }

        pub fn strings(&self, dotted: &str) -> Vec<String> {
            match self.at(dotted) {
                Json::Arr(items) => items
                    .iter()
                    .map(|i| match i {
                        Json::Str(s) => s.clone(),
                        other => panic!("array member is not a string: {other:?}"),
                    })
                    .collect(),
                other => panic!("member {dotted:?} is not an array: {other:?}"),
            }
        }
    }

    fn skip_ws(b: &[char], at: &mut usize) {
        while *at < b.len() && matches!(b[*at], ' ' | '\t' | '\n' | '\r') {
            *at += 1;
        }
    }

    fn parse_value(b: &[char], at: &mut usize) -> Result<Json, String> {
        skip_ws(b, at);
        match b.get(*at) {
            None => Err("unexpected end of input".into()),
            Some('{') => parse_object(b, at),
            Some('[') => parse_array(b, at),
            Some('"') => Ok(Json::Str(parse_string(b, at)?)),
            Some('t') => lit(b, at, "true", Json::Bool(true)),
            Some('f') => lit(b, at, "false", Json::Bool(false)),
            Some('n') => lit(b, at, "null", Json::Null),
            Some(_) => parse_number(b, at),
        }
    }

    fn lit(b: &[char], at: &mut usize, word: &str, value: Json) -> Result<Json, String> {
        for c in word.chars() {
            if b.get(*at) != Some(&c) {
                return Err(format!("bad literal at {at}, expected {word:?}"));
            }
            *at += 1;
        }
        Ok(value)
    }

    fn parse_object(b: &[char], at: &mut usize) -> Result<Json, String> {
        *at += 1; // '{'
        let mut members = Vec::new();
        skip_ws(b, at);
        if b.get(*at) == Some(&'}') {
            *at += 1;
            return Ok(Json::Obj(members));
        }
        loop {
            skip_ws(b, at);
            let k = parse_string(b, at)?;
            skip_ws(b, at);
            if b.get(*at) != Some(&':') {
                return Err(format!("expected ':' at {at}"));
            }
            *at += 1;
            let v = parse_value(b, at)?;
            if members
                .iter()
                .any(|(existing, _): &(String, Json)| *existing == k)
            {
                return Err(format!("duplicate key {k:?}"));
            }
            members.push((k, v));
            skip_ws(b, at);
            match b.get(*at) {
                Some(',') => *at += 1,
                Some('}') => {
                    *at += 1;
                    return Ok(Json::Obj(members));
                }
                _ => return Err(format!("expected ',' or '}}' at {at}")),
            }
        }
    }

    fn parse_array(b: &[char], at: &mut usize) -> Result<Json, String> {
        *at += 1; // '['
        let mut items = Vec::new();
        skip_ws(b, at);
        if b.get(*at) == Some(&']') {
            *at += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(parse_value(b, at)?);
            skip_ws(b, at);
            match b.get(*at) {
                Some(',') => *at += 1,
                Some(']') => {
                    *at += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at {at}")),
            }
        }
    }

    fn parse_string(b: &[char], at: &mut usize) -> Result<String, String> {
        if b.get(*at) != Some(&'"') {
            return Err(format!("expected '\"' at {at}"));
        }
        *at += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = b.get(*at) else {
                return Err("unterminated string".into());
            };
            *at += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(&esc) = b.get(*at) else {
                        return Err("unterminated escape".into());
                    };
                    *at += 1;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => out.push(parse_unicode_escape(b, at)?),
                        other => return Err(format!("bad escape \\{other}")),
                    }
                }
                // RFC 8259: unescaped control characters are illegal in a
                // JSON string. Rejecting them here is what makes the
                // escaper tests meaningful.
                c if c < ' ' => return Err(format!("raw control U+{:04X} in string", c as u32)),
                c => out.push(c),
            }
        }
    }

    fn parse_unicode_escape(b: &[char], at: &mut usize) -> Result<char, String> {
        let unit = hex4(b, at)?;
        if (0xd800..0xdc00).contains(&unit) {
            // High surrogate: a low surrogate must follow.
            if b.get(*at) != Some(&'\\') || b.get(*at + 1) != Some(&'u') {
                return Err("lone high surrogate".into());
            }
            *at += 2;
            let low = hex4(b, at)?;
            if !(0xdc00..0xe000).contains(&low) {
                return Err("bad low surrogate".into());
            }
            let combined = 0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
            return char::from_u32(combined).ok_or_else(|| "bad surrogate pair".into());
        }
        char::from_u32(unit).ok_or_else(|| format!("bad \\u escape {unit:04x}"))
    }

    fn hex4(b: &[char], at: &mut usize) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let Some(&c) = b.get(*at) else {
                return Err("short \\u escape".into());
            };
            let digit = c.to_digit(16).ok_or_else(|| format!("bad hex digit {c}"))?;
            value = value * 16 + digit;
            *at += 1;
        }
        Ok(value)
    }

    fn parse_number(b: &[char], at: &mut usize) -> Result<Json, String> {
        let start = *at;
        while *at < b.len() && !matches!(b[*at], ',' | '}' | ']' | ' ' | '\t' | '\n' | '\r') {
            *at += 1;
        }
        let text: String = b[start..*at].iter().collect();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number {text:?}"))
    }

    // -- shared test helpers (used by `crate::principal`'s session tests) ---
    //
    // Every test here that opens a log takes `crate::capture::tests::
    // fd_lock()`, the crate's convention for anything holding a file
    // descriptor: a live [`Recorder`] holds one, and the capture suite's
    // `/proc/self/fd` baseline assertions must never race it.

    /// A private scratch path for one test's log file.
    pub(crate) fn scratch_log_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir()
            .join(format!(
                "vitrin-recorder-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ))
            .join(format!("{label}.jsonl"))
    }

    /// A recorder writing to a fresh scratch file, plus that path.
    pub(crate) fn scratch_recorder(label: &str) -> (Recorder, PathBuf) {
        let path = scratch_log_path(label);
        let recorder = Recorder::create(&path).expect("scratch log must be creatable");
        (recorder, path)
    }

    /// Every line of the log at `path`, parsed. Asserts each line is
    /// exactly one JSON object and carries the envelope, so no test has to
    /// restate the invariant.
    pub(crate) fn read_log(path: &Path) -> Vec<Json> {
        let text = std::fs::read_to_string(path).expect("log file must be readable");
        let mut entries = Vec::new();
        let mut expected_seq = 1u64;
        for (i, line) in text.lines().enumerate() {
            let value = Json::parse(line)
                .unwrap_or_else(|e| panic!("line {} is not valid JSON ({e}): {line}", i + 1));
            assert_eq!(
                value.u64("schema_version"),
                u64::from(SCHEMA_VERSION),
                "every line carries schema_version"
            );
            assert_eq!(
                value.u64("seq"),
                expected_seq,
                "seq is gap-free and ascending"
            );
            expected_seq += 1;
            assert!(!value.str("run_id").is_empty());
            assert!(!value.str("kind").is_empty());
            // Present on every line; values are real clock readings, so
            // only their presence and type are asserted.
            let _ = value.u64("mono_us");
            let _ = value.u64("wall_ms");
            entries.push(value);
        }
        entries
    }

    /// Remove a scratch log and its private directory.
    pub(crate) fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// The entries of one kind, in order.
    pub(crate) fn of_kind<'e>(entries: &'e [Json], kind: &str) -> Vec<&'e Json> {
        entries.iter().filter(|e| e.str("kind") == kind).collect()
    }

    fn identity(s: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(s).expect("test identity is well-formed")
    }

    fn peer() -> PeerCred {
        PeerCred {
            pid: Some(4242),
            uid: 1000,
            gid: 1000,
        }
    }

    // -- the escaper -------------------------------------------------------

    /// Round-trip one string through the emitter and the in-test reader.
    fn round_trip(s: &str) -> String {
        let mut out = String::new();
        push_json_string(&mut out, s);
        match Json::parse(&out) {
            Ok(Json::Str(parsed)) => parsed,
            other => panic!("escaping {s:?} produced {out:?}, which parsed as {other:?}"),
        }
    }

    #[test]
    fn escaper_round_trips_every_c0_control_del_and_hostile_punctuation() {
        // Every C0 control, individually and all at once.
        for code in 0u32..0x20 {
            let c = char::from_u32(code).expect("C0 is a valid scalar value");
            let s = format!("before{c}after");
            assert_eq!(round_trip(&s), s, "C0 U+{code:04X} must round-trip");
        }
        let all_c0: String = (0u32..0x20)
            .map(|c| char::from_u32(c).expect("C0 is a valid scalar value"))
            .collect();
        assert_eq!(round_trip(&all_c0), all_c0);

        // DEL and the C1 controls (legal JSON content, passed through).
        assert_eq!(round_trip("\u{7f}"), "\u{7f}");
        let c1: String = (0x80u32..0xa0)
            .map(|c| char::from_u32(c).expect("C1 is a valid scalar value"))
            .collect();
        assert_eq!(round_trip(&c1), c1);

        // Quotes, backslashes, and the shapes that break naive emitters.
        for hostile in [
            r#"has "quotes""#,
            r"back\slash",
            r#"\"escaped-looking\""#,
            "{\"injected\": \"object\"}",
            "line\nbreak\ttab\rcarriage",
            "trailing backslash\\",
            "\\\\\\\"",
            "",
        ] {
            assert_eq!(round_trip(hostile), hostile, "must round-trip: {hostile:?}");
        }
    }

    #[test]
    fn escaper_round_trips_multi_byte_utf8() {
        for s in [
            "héllo",                     // 2-byte
            "日本語のテキスト",          // 3-byte
            "emoji: \u{1f680}\u{1f512}", // 4-byte (surrogate pair on the wire)
            "combining: e\u{301}",       // combining mark
            "rtl: \u{202e}reversed",     // bidi override
            "zero width: a\u{200b}b",    // invisible
            "\u{feff}bom",               // BOM
        ] {
            assert_eq!(round_trip(s), s, "must round-trip: {s:?}");
        }
    }

    #[test]
    fn escaper_emits_the_short_forms_and_hex_for_the_rest() {
        // The concrete wire shape, not just the round-trip property.
        let mut out = String::new();
        push_json_string(&mut out, "\u{8}\u{c}\n\r\t\u{0}\u{1f}\u{7f}\"\\");
        assert_eq!(
            out, "\"\\b\\f\\n\\r\\t\\u0000\\u001f\\u007f\\\"\\\\\"",
            "short forms where JSON defines them, lowercase \\u00XX otherwise"
        );
    }

    // -- the digest --------------------------------------------------------

    #[test]
    fn digest_is_algorithm_tagged_and_content_sensitive() {
        let a = ObservationDigest::of(b"frame-a");
        let b = ObservationDigest::of(b"frame-b");
        assert_ne!(a, b, "the digest must vary with content");
        assert_eq!(a, ObservationDigest::of(b"frame-a"), "and be deterministic");
        assert_eq!(a.to_hex().len(), 64, "blake3 is 32 bytes, 64 hex chars");
        assert!(a.to_hex().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(format!("{a:?}"), format!("{DIGEST_ALG}:{}", a.to_hex()));

        // A single flipped bit anywhere changes it (the property replay
        // depends on).
        let mut bytes = vec![0u8; 4096];
        let base = ObservationDigest::of(&bytes);
        bytes[4095] ^= 1;
        assert_ne!(base, ObservationDigest::of(&bytes));

        // The tag is what the schema writes.
        let mut out = String::from("{");
        write_frame(
            &mut out,
            Some(ObservedFrame {
                width: 4,
                height: 2,
                stride: 16,
                bytes: 32,
                digest: a,
            }),
        );
        out.push('}');
        let parsed = Json::parse(&out).expect("frame block is valid JSON");
        assert_eq!(parsed.str("frame.digest_alg"), DIGEST_ALG);
        assert_eq!(parsed.str("frame.digest"), a.to_hex());
        assert_eq!(parsed.str("frame.format"), "xrgb8888");
        assert_eq!(parsed.u64("frame.bytes"), 32);
    }

    // -- the envelope and the entry shapes ---------------------------------

    #[test]
    fn every_line_is_one_json_object_with_the_envelope() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rec, path) = scratch_recorder("envelope");
        rec.record(Event::RunStarted {
            pid: 7,
            core_version: "0.1.0",
            consent_policy: "auto-approve",
        });
        rec.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(3),
        });
        rec.record(Event::RunEnded { dropped_entries: 0 });

        // read_log asserts schema_version, gap-free seq, run_id, kind,
        // mono_us and wall_ms on every line.
        let entries = read_log(&path);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].str("kind"), "run_started");
        assert_eq!(entries[0].str("digest_alg"), DIGEST_ALG);
        assert_eq!(entries[0].str("consent_policy"), "auto-approve");
        assert_eq!(entries[1].str("grant_id"), "grant-3");
        assert_eq!(entries[2].u64("dropped_entries"), 0);
        // One run id across the run.
        assert_eq!(entries[0].str("run_id"), rec.run_id());
        assert_eq!(entries[2].str("run_id"), rec.run_id());
        cleanup(&path);
    }

    #[test]
    fn a_hostile_claimed_identity_round_trips_as_valid_json() {
        let _fd = crate::capture::tests::fd_lock();
        // The secrecy + hostile-input contract, end to end: a rejected
        // handshake's claimed identity is client-controlled text that must
        // survive exactly, and the credential must appear only as a length.
        let hostile = "vitrin://\"\\\n\u{0}\u{7f}\u{1f600}/evil\", \"identity\": \"admin";
        let (mut rec, path) = scratch_recorder("hostile-identity");
        rec.record(Event::HandshakeRefused {
            connection: ConnectionId::from_u64_for_test(1),
            peer: peer(),
            cause_class: auth_cause_class(&RejectionCause::UnknownIdentity),
            claimed_identity: hostile,
            credential_type: "static-token\"; DROP",
            credential_bytes: 64,
        });

        let entries = read_log(&path);
        assert_eq!(entries.len(), 1, "hostile input must not split the line");
        let e = &entries[0];
        assert_eq!(e.str("kind"), "handshake_refused");
        assert_eq!(e.str("claimed_identity"), hostile, "escaped exactly");
        assert_eq!(e.str("credential_type"), "static-token\"; DROP");
        assert_eq!(e.str("cause_class"), "unknown_identity");
        assert_eq!(e.u64("credential_bytes"), 64);
        // The injection attempt did not create a second `identity` member:
        // the real one is null (nothing bound), and the reader rejects
        // duplicate keys outright.
        assert!(e.is_null("identity"));
        // The line's member set is exactly the schema's -- the injection
        // attempt added no key. (The reader rejects duplicate keys, so
        // parsing at all already proved no second `identity` appeared.)
        let Json::Obj(members) = e else {
            panic!("an entry is an object");
        };
        let keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "schema_version",
                "run_id",
                "seq",
                "mono_us",
                "wall_ms",
                "kind",
                "connection",
                "peer_uid",
                "peer_pid",
                "cause_class",
                "identity",
                "claimed_identity",
                "credential_type",
                "credential_bytes",
            ]
        );
        cleanup(&path);
    }

    #[test]
    fn the_cause_taxonomy_covers_every_rejection_and_never_echoes_client_text() {
        for (cause, want) in [
            (
                RejectionCause::UnsupportedScheme {
                    presented: "spiffe-jwt-svid".into(),
                },
                "unsupported_scheme",
            ),
            (RejectionCause::UnknownIdentity, "unknown_identity"),
            (RejectionCause::BadToken, "bad_token"),
            (
                RejectionCause::PeerCredMismatch {
                    required_uid: 1000,
                    peer_uid: 0,
                },
                "peercred_mismatch",
            ),
        ] {
            let class = auth_cause_class(&cause);
            assert_eq!(class, want);
            // The class is a fixed label: client text can never reach it
            // (the Display of the same cause would embed the scheme).
            assert!(!class.contains("spiffe"));
        }
    }

    #[test]
    fn use_decision_entries_carry_null_versioned_epoch_fields() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rec, path) = scratch_recorder("epoch-fields");
        let admitted = UseOutcome::Admitted {
            grant: GrantId::from_u64_for_test(9),
            frame: Some(ObservedFrame {
                width: 64,
                height: 48,
                stride: 256,
                bytes: 12_288,
                digest: ObservationDigest::of(b"pixels"),
            }),
            spent_once: false,
        };
        let refused = UseOutcome::Refused {
            code: Refusal::RateLimited,
            voiced: true,
        };
        // Three shapes: an admitted capture, a refusal judged against a
        // live row (rate limiting), and a refusal with no row at all (a
        // facet whose grant never resolved granted).
        let not_granted = UseOutcome::Refused {
            code: Refusal::NotGranted,
            voiced: true,
        };
        let live_row = Some(GrantId::from_u64_for_test(9));
        for (verb, grant_row, outcome) in [
            (Verb::OBSERVE, live_row, &admitted),
            (Verb::ACTUATE_POINTER, live_row, &refused),
            (Verb::ACTUATE_TEXT, None, &not_granted),
        ] {
            rec.record(Event::UseDecision {
                connection: ConnectionId::from_u64_for_test(1),
                facet_wire_id: 20,
                grant_wire_id: 10,
                verb,
                grant_row,
                outcome,
            });
        }

        let entries = read_log(&path);
        assert_eq!(entries.len(), 3);
        for e in &entries {
            // B1: the epoch reference block exists on every use decision,
            // with every member explicitly null in this version.
            assert!(e.is_null("epoch.observed"), "epoch.observed must be null");
            assert!(e.is_null("epoch.expected"), "epoch.expected must be null");
            assert!(e.is_null("epoch.target"), "epoch.target must be null");
        }
        assert_eq!(entries[0].str("verb"), "observe");
        assert_eq!(entries[0].str("decision"), "allowed");
        assert_eq!(entries[0].str("grant_id"), "grant-9");
        assert!(entries[0].is_null("refusal"));
        assert_eq!(entries[0].u64("frame.width"), 64);
        assert_eq!(entries[1].str("verb"), "actuate_pointer");
        assert_eq!(entries[1].str("decision"), "refused");
        assert_eq!(entries[1].str("refusal"), "rate_limited");
        assert!(entries[1].bool("refusal_voiced"));
        assert_eq!(
            entries[1].str("grant_id"),
            "grant-9",
            "a refusal judged against a live row must name it"
        );
        assert!(
            entries[1].is_null("frame"),
            "a refused use delivered nothing to digest"
        );
        assert_eq!(entries[2].str("verb"), "actuate_text");
        assert_eq!(entries[2].str("refusal"), "not_granted");
        assert!(
            entries[2].is_null("grant_id"),
            "a facet whose grant never resolved granted has no row to name"
        );
        cleanup(&path);
    }

    #[test]
    fn every_enum_label_is_stable_and_total() {
        // A new wire variant must fail to compile here (the matches are
        // exhaustive), and the labels are the reader's contract.
        for (code, want) in [
            (Refusal::NotGranted, "not_granted"),
            (Refusal::Expired, "expired"),
            (Refusal::Revoked, "revoked"),
            (Refusal::RateLimited, "rate_limited"),
            (Refusal::Preempted, "preempted"),
            (Refusal::ConsentHeld, "consent_held"),
            (Refusal::NoSurface, "no_surface"),
            (Refusal::Internal, "internal"),
        ] {
            assert_eq!(refusal_label(code), want);
        }
        for (outcome, want) in [
            (Outcome::Granted, "granted"),
            (Outcome::Denied, "denied"),
            (Outcome::TimedOut, "timed_out"),
            (Outcome::Unavailable, "unavailable"),
            (Outcome::Unsupported, "unsupported"),
            (Outcome::Busy, "busy"),
        ] {
            assert_eq!(outcome_label(outcome), want);
        }
        for (state, want) in [
            (ConsentState::Queued, "queued"),
            (ConsentState::Shown, "shown"),
            (ConsentState::Closed, "closed"),
        ] {
            assert_eq!(consent_state_label(state), want);
        }
        for (p, want) in [
            (WirePersistence::Once, "once"),
            (WirePersistence::WhileRunning, "while_running"),
            (WirePersistence::UntilRevoked, "until_revoked"),
            (WirePersistence::Always, "always"),
        ] {
            assert_eq!(wire_persistence_label(p), want);
        }
        assert_eq!(rung_label(PersistenceRung::Once), "once");
        assert_eq!(rung_label(PersistenceRung::WhileRunning), "while_running");
        assert_eq!(issuer_label(Issuer::HumanConsent), "human_consent");
        assert_eq!(
            issuer_label(Issuer::AutoApprovePolicy),
            "auto_approve_policy"
        );
        assert_eq!(issuer_label(Issuer::ScriptedConsent), "scripted_consent");
        assert_eq!(verb_label(Verb::OBSERVE), "observe");
        assert_eq!(verb_label(Verb::ACTUATE_POINTER), "actuate_pointer");
        assert_eq!(verb_label(Verb::ACTUATE_TEXT), "actuate_text");
        // A multi-bit value is not one facet's verb; it must not be
        // rendered as one of them.
        assert_eq!(verb_label(Verb::OBSERVE | Verb::ACTUATE_TEXT), "unknown");
    }

    #[test]
    fn petition_entries_state_requested_then_effective_authority() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rec, path) = scratch_recorder("petition-authority");
        let who = identity("vitrin://local/agent/demo");
        rec.record(Event::PetitionRequested {
            connection: ConnectionId::from_u64_for_test(1),
            identity: &who,
            realm_name: "realm-0",
            grant_wire_id: 10,
            consent_wire_id: 11,
            resource: "",
            requested: RequestedAuthority {
                verbs: Verb::OBSERVE | Verb::ACTUATE_TEXT,
                persistence: WirePersistence::WhileRunning,
                expiry_ms: 60_000,
                max_event_rate: 0,
                flags: 0,
            },
        });
        rec.record(Event::PetitionResolved {
            connection: ConnectionId::from_u64_for_test(1),
            grant_wire_id: 10,
            outcome: Outcome::Granted,
            effective: Some(EffectiveAuthority {
                verbs: Verb::OBSERVE,
                persistence: PersistenceRung::Once,
                expiry_ms: 30_000,
                max_event_rate: std::num::NonZeroU32::new(20).unwrap(),
            }),
            grant_id: Some(GrantId::from_u64_for_test(1)),
            issuer: Some(Issuer::ScriptedConsent),
        });
        rec.record(Event::PetitionResolved {
            connection: ConnectionId::from_u64_for_test(1),
            grant_wire_id: 20,
            outcome: Outcome::Busy,
            effective: None,
            grant_id: None,
            issuer: None,
        });

        let entries = read_log(&path);
        let requested = &entries[0];
        assert_eq!(
            requested.strings("requested.verbs"),
            vec!["observe", "actuate_text"]
        );
        assert_eq!(requested.u64("requested.verbs_bits"), 5);
        assert_eq!(requested.u64("requested.expiry_ms"), 60_000);
        assert_eq!(
            requested.u64("requested.max_event_rate"),
            0,
            "the wire's `0 = server default` is recorded as asked, not as resolved"
        );
        assert_eq!(requested.str("resource"), "");
        assert_eq!(requested.str("identity"), "vitrin://local/agent/demo");

        let granted = &entries[1];
        assert_eq!(granted.str("outcome"), "granted");
        assert_eq!(granted.strings("effective.verbs"), vec!["observe"]);
        assert_eq!(granted.str("effective.persistence"), "once");
        assert_eq!(granted.u64("effective.expiry_ms"), 30_000);
        assert_eq!(granted.u64("effective.max_event_rate"), 20);
        assert_eq!(granted.str("grant_id"), "grant-1");
        assert_eq!(granted.str("issuer"), "scripted_consent");

        let busy = &entries[2];
        assert_eq!(busy.str("outcome"), "busy");
        assert!(busy.is_null("effective"));
        assert!(busy.is_null("grant_id"));
        assert!(busy.is_null("issuer"));
        cleanup(&path);
    }

    // -- line atomicity ----------------------------------------------------

    #[test]
    fn interleaved_entries_land_as_whole_lines() {
        let _fd = crate::capture::tests::fd_lock();
        // The line-atomicity property: strings full of newlines, braces and
        // quotes -- the shapes that would fragment a naive writer -- still
        // yield exactly one line per entry, each independently parseable,
        // and the entries stay in `seq` order.
        let (mut rec, path) = scratch_recorder("atomicity");
        const N: usize = 64;
        for i in 0..N {
            let hostile = format!("{{\"kind\":\"forged\"}}\n{{\"seq\":{i}}}\n\r\u{0}");
            rec.record(Event::HandshakeRefused {
                connection: ConnectionId::from_u64_for_test(i as u64 + 1),
                peer: peer(),
                cause_class: "bad_token",
                claimed_identity: &hostile,
                credential_type: &hostile,
                credential_bytes: i,
            });
        }
        let entries = read_log(&path);
        assert_eq!(entries.len(), N, "one line per entry, never fragments");
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.u64("seq"), i as u64 + 1);
            assert_eq!(e.str("kind"), "handshake_refused");
            assert_eq!(e.u64("credential_bytes"), i as u64);
        }
        // No forged entry slipped in: every line's kind is ours.
        assert!(of_kind(&entries, "forged").is_empty());
        cleanup(&path);
    }

    #[test]
    fn a_second_run_appends_rather_than_clobbering() {
        let _fd = crate::capture::tests::fd_lock();
        let path = scratch_log_path("append");
        let mut first = Recorder::create(&path).unwrap();
        first.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(1),
        });
        let first_run = first.run_id().to_string();
        drop(first);

        let mut second = Recorder::create(&path).unwrap();
        second.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(2),
        });

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "the first run's line survived");
        let a = Json::parse(lines[0]).unwrap();
        let b = Json::parse(lines[1]).unwrap();
        assert_eq!(a.str("run_id"), first_run);
        assert_eq!(b.str("run_id"), second.run_id());
        // Each run numbers its own entries from 1; run_id disambiguates.
        assert_eq!(a.u64("seq"), 1);
        assert_eq!(b.u64("seq"), 1);
        cleanup(&path);
    }

    // -- the failure policies ----------------------------------------------

    #[test]
    fn creation_failure_is_loud_and_fatal() {
        let _fd = crate::capture::tests::fd_lock();
        // A directory is not an appendable log: creation must fail with the
        // path named, not degrade silently into a recorder that writes
        // nowhere.
        let dir = scratch_log_path("creation-failure")
            .parent()
            .expect("scratch paths have a parent")
            .to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let err = Recorder::create(&dir).expect_err("a directory cannot be a log file");
        let rendered = err.to_string();
        assert!(rendered.contains(&dir.display().to_string()), "{rendered}");
        assert!(rendered.contains("could not be opened"), "{rendered}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_failure_degrades_loudly_and_never_halts_the_caller() {
        let _fd = crate::capture::tests::fd_lock();
        // The documented policy: the first failure latches the recorder
        // degraded, every later entry is *counted* rather than written, and
        // `record` still returns normally -- a diagnostic can never fail an
        // authority path.
        let path = scratch_log_path("write-failure");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        // A read-only handle on a real file: every write fails EBADF,
        // deterministically and on every platform this core targets.
        let read_only = File::open(&path).expect("open the scratch log read-only");
        let mut rec = Recorder::with_file(read_only, path.clone());

        assert!(!rec.is_degraded());
        for i in 1..=5u64 {
            rec.record(Event::GrantExpired {
                grant_id: GrantId::from_u64_for_test(i),
            });
            assert!(rec.is_degraded(), "the first failure latches degradation");
            assert_eq!(rec.dropped_entries(), i, "every lost entry is counted");
        }
        // Nothing reached the file, and nothing panicked.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");

        // The seq counter advanced anyway, so a later reader sees the gap
        // rather than a silently renumbered log.
        let recovered = Recorder::create(&path).unwrap();
        assert_eq!(recovered.dropped_entries(), 0);
        cleanup(&path);
    }

    #[test]
    fn the_log_file_is_owner_only() {
        let _fd = crate::capture::tests::fd_lock();
        use std::os::unix::fs::PermissionsExt;

        let (rec, path) = scratch_recorder("permissions");
        drop(rec);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the log carries identities; keep it private");
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        cleanup(&path);
    }
}
