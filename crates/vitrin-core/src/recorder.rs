// SPDX-License-Identifier: MPL-2.0
//! The flight-recorder log v0 (P1.4.5, issue #29): the journal seed --
//! a JSON-lines structured event log recording handshakes, grant lifecycle
//! transitions, consent decisions, realm launches, and **every** enforcement
//! decision, with an observation digest on every delivered capture and
//! epoch-ready reference fields present-but-null from day one (backward
//! requirement B1).
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
//! second path. Realm launches (P1.5.2) are the same shape taken one step
//! further: [`crate::spawn`] takes a `&mut Recorder` as a *required*
//! argument and journals both outcomes in one funnel wrapped around the
//! spawn, so an error path added inside it is covered structurally rather
//! than by remembering to add a call.
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
//! ## Actuation detail (the other half of "what was done")
//!
//! A capture entry identifies what was *observed* (`frame`, above). The
//! symmetric question -- what was *actuated* -- needs more than the verb:
//! `actuate_pointer` alone cannot distinguish a move from a button press
//! from a scroll, and a reconstruction that cannot tell those apart has
//! not reconstructed anything. Every `use_decision` for an actuation
//! therefore carries an `input` object ([`ActuationDetail`]), on refusals
//! as well as admissions -- what was *attempted* is exactly what a
//! debugging aid is for. Captures carry `"input": null`; they have a
//! `frame` instead.
//!
//! Pointer parameters are recorded **in full**: `x`/`y`, the evdev button
//! code and its pressed/released state, the scroll axis and `value120`.
//! They reconstruct behaviour precisely and leak nothing -- a coordinate
//! is not a secret. (`x`/`y` are the values *as the agent requested
//! them*; an admitted motion outside the realm view is clamped at delivery
//! per the IDL, so a replay pairs these with the view geometry the capture
//! entries already state. They arrive as `i32` on the wire and are
//! recorded as integers.)
//!
//! **Typed text is recorded by shape only, never verbatim -- and there is
//! no flag to change that in v0.** `vitrin_actuator_text.type` carries
//! arbitrary agent-chosen Unicode: passwords, API tokens, private
//! correspondence. Writing it out would make a debugging aid a keylogger
//! and a credential store, which is precisely what the secrecy contract
//! below forbids for *handshake* credentials -- and there is no principled
//! reason the same bytes become safe because they arrived through a
//! different interface. So a text entry states `chars`, `bytes`, and a
//! `digest` over the UTF-8 (same algorithm and tagging as a frame digest):
//! enough to see that something was typed, how much, and whether the same
//! string was typed twice or matches a known corpus -- without the log
//! ever holding the string.
//!
//! An opt-in verbatim flag (the `--consent auto-approve` precedent) was
//! considered and **rejected for v0**, deliberately: a flag is a code path
//! that *can* leak, and adding one to the TCB to serve a debugging
//! convenience nobody has needed yet is the wrong direction. Adding the
//! flag later is additive and reversible; un-leaking a log that already
//! captured a password is neither.
//!
//! ## Secrecy contract (inherited from [`crate::identity`])
//!
//! The recorder MUST NOT contain credential bytes; at most
//! `credential_type` and `credential_bytes` (a length). This module has no
//! way to express a credential -- [`Event::HandshakeRefused`] takes a
//! `usize` length, not bytes -- so the rule is structural, not a
//! convention. [`ActuationDetail::Text`] is built the same way: it holds a
//! length and a digest, and has no field a string could be put in.
//!
//! ## Bounding refusal floods (never at the cost of B1)
//!
//! Every entry costs one synchronous `write(2)` on the compositor thread.
//! For *admitted* uses that cost is bounded by the grant's own
//! `max_event_rate`, and B1 makes it non-negotiable anyway. For
//! **refusals** it is not bounded at all: the chokepoint judges
//! `not_granted` at its very first step, *before* the token bucket, so a
//! facet whose grant never resolved `granted` is refused with no rate
//! ceiling whatsoever -- and nothing downstream supplies one either.
//! Actuation refusals do coalesce on the wire ([`crate::enforcement`]),
//! but that bounds the *wire*: before this, one muted wire refusal still
//! bought a full ~350-byte synchronous write. Capture refusals are not
//! even coalesced, since the IDL mandates one terminal per
//! `capture_frame`. Either way an ungranted principal could grow the disk
//! and stall the compositor for free, at whatever rate it could send.
//!
//! So the recorder keeps a **refusal run** per `(connection, grant)`: the
//! key `(verb, refusal code)` currently repeating, and how many repeats
//! have been swallowed. The first refusal of a run is written in full;
//! repeats are counted, not written; and the count surfaces as a
//! [`Event::UseRefusalSummary`] when the run ends (a different verb/code
//! on that grant, an admitted use, teardown, or shutdown) or every
//! [`REFUSAL_SUMMARY_INTERVAL`] while it persists, so a *sustained* flood
//! is visible while it happens rather than only in the postmortem. The
//! condition is therefore never silent -- which is the actual requirement
//! -- while a flood costs ~1 line per second per grant instead of one per
//! request. Run state is capped at [`MAX_REFUSAL_RUNS`]; at the cap the
//! open runs are flushed and cleared, which bounds memory and still
//! amortizes to at most one extra line per refusal.
//!
//! **B1 is untouched by any of this.** The bounding is reachable only from
//! the [`UseOutcome::Refused`] arm; an `Admitted` entry is never
//! suppressed, never sampled, never aggregated, and always carries its own
//! frame digest. That is asserted directly, not left to inspection
//! (`admitted_captures_are_never_suppressed_by_refusal_bounding`).
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
//! once, latches the recorder degraded (no write is attempted at capture
//! rate while degraded -- a full disk must not produce an error storm),
//! and counts every entry dropped from then on
//! ([`Recorder::dropped_entries`]). Silence is what is forbidden, not
//! degradation. When P6's signed journal lands, *its* failure policy is a
//! separate decision and may well be fail-closed: a signed journal is
//! evidence, this is a debugging aid.
//!
//! **Degradation is recoverable on a bounded budget -- which is what makes
//! the file-only evidence real.** A latch that could never lift would make
//! the "gap in `seq`" this module used to promise structurally
//! *unproducible*: with no further write ever attempted, dropped entries
//! are always a contiguous **tail**, never an interior hole, so the file
//! just ends -- indistinguishable from `SIGKILL` -- and even the closing
//! `run_ended` naming the loss could never be written. A signal that
//! cannot occur is not a signal. So a degraded recorder retries, on a
//! budget a full disk cannot turn into a storm:
//!
//! - at most [`RECOVERY_ATTEMPTS`] reopen attempts per run, and never two
//!   within [`RECOVERY_BACKOFF`] -- so the worst case for a disk that
//!   stays full is a handful of extra failed `write(2)`s across the whole
//!   run, not one per capture;
//! - a successful reopen writes a [`Event::RecordingResumed`] entry
//!   **first**, naming how many entries the gap swallowed, so the hole is
//!   self-describing rather than something a reader must infer;
//! - [`Recorder::finish`] gets one *forced* attempt outside the budget
//!   (shutdown happens once, so a single extra syscall cannot storm), so a
//!   transient failure still ends with a `run_ended` stating the total.
//!
//! What a reader holding **only the file** therefore sees. If recovery
//! ever succeeded: an interior gap in `seq`, an explicit
//! `recording_resumed` naming the loss, and a `run_ended` naming the run
//! total. If the disk stayed full through shutdown: the file ends
//! mid-run with no footer -- honestly conceded here, because that case is
//! genuinely indistinguishable from `SIGKILL` from inside the file alone,
//! and the two `tracing::error!` lines are then the only evidence. The
//! recorder does not pretend otherwise.
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
//! - *Partial lines, and why they cannot poison the next line*: the one
//!   way to get a fragment is a write that succeeds part-way and then
//!   fails (`write_all` loops over short writes, so a nearly-full
//!   filesystem can land a prefix and then `ENOSPC`). Left alone, that
//!   prefix would swallow whatever a *later* appender writes -- gluing
//!   run B's first line onto run A's fragment and producing a line that is
//!   neither a tolerable trailing fragment nor a valid entry, which would
//!   falsify the interleaving property one bullet up. So the failure path
//!   makes one best-effort `write(2)` of a lone `\n`, terminating whatever
//!   landed as its own (invalid) line; recovery likewise prefixes its
//!   first line with `\n` in case even that terminator failed. Two rules
//!   for a reader, both cheap: **tolerate at most one invalid line per
//!   interrupted run, and skip empty lines.** Every other line is a
//!   complete entry.
//!
//! **The file is opened append, never truncated.** A default path carries
//! the pid, so runs get their own files; an operator who deliberately
//! points two runs at one path gets an interleaved-but-parseable log rather
//! than a clobbered one.
//!
//! **Mode `0600` is enforced on the open descriptor, not merely requested
//! at creation.** The log carries verifier-canonical identities, peer uids
//! and pids, realm names and grant rows -- session metadata, not secrets,
//! but nothing to hand out either. `OpenOptions::mode` applies *only when
//! the file is created*, and `create(true)` follows an existing symlink,
//! so a `--recorder` pointed at a pre-existing `0644` file, or at a
//! symlink into a world-readable directory, would silently append all of
//! that with whatever permissions were already there -- the stated
//! justification defeated by the one case it exists for. [`open_append`]
//! therefore `fstat`s the descriptor it just opened, requires a **regular
//! file** (a symlink to a fifo, device, or directory is refused, not
//! written into), and `fchmod`s it to `0600` unconditionally. `fchmod` on
//! the fd, never a path, so there is no TOCTOU window; and because a file
//! this process does not own cannot be `fchmod`ed, a symlink aimed at
//! *someone else's* file fails closed at startup instead of appending to
//! it.
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

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vitrin_ipc::PeerCred;
use vitrin_protocol::generated::vitrin_actuator_pointer::{Axis, ButtonState};
use vitrin_protocol::generated::vitrin_consent::ConsentState;
use vitrin_protocol::generated::vitrin_grant::{
    Outcome, Persistence as WirePersistence, Refusal, Verb,
};

use crate::enforcement::{UseKind, UseOutcome};
use crate::grants::{GrantId, Issuer, PersistenceRung, RealmId};
use crate::identity::{PrincipalIdentity, RejectionCause};
use crate::input::SeatInputKind;
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

/// [`Event::GrantRevoked::cause`] for the hold-chord dead-man switch
/// (P1.7.3): a human ended this authority with their own hands.
///
/// Separate from `scope` because the two are genuinely orthogonal, not two
/// spellings of one fact: `scope` says how *wide* the revocation was (one
/// row, or every row of a principal), `cause` says *who or what* decided.
/// A future panel that revokes all of a principal's rows is
/// `scope=principal, cause=panel`, and a replay that had to infer the cause
/// from an adjacent entry would be reading a coincidence of write order.
pub(crate) const REVOKE_CAUSE_DEAD_MAN: &str = "dead_man_chord";

/// [`Event::GrantRevoked::cause`] for a revocation taken through a
/// deliberate single-grant act (the P2 panel; policy). Not yet reachable at
/// runtime -- the panel is a later phase -- but named here so the vocabulary
/// is closed from the start rather than growing a default.
pub(crate) const REVOKE_CAUSE_OPERATOR: &str = "operator";

/// How many times one run may try to reopen its log after a write failure
/// latched it degraded. Small and fixed: enough that a *transient* failure
/// (a full filesystem the operator cleared, an `EIO` blip) resumes
/// recording and leaves the interior gap the module docs promise, few
/// enough that a filesystem which stays full costs a handful of failed
/// `write(2)`s across the whole run rather than one per capture.
const RECOVERY_ATTEMPTS: u32 = 8;

/// Minimum monotonic spacing between two recovery attempts. With
/// [`RECOVERY_ATTEMPTS`] this bounds recovery cost twice over -- by count
/// and by rate -- so neither a fast capture loop nor a long session can
/// turn retrying into the error storm the latch exists to prevent.
const RECOVERY_BACKOFF: Duration = Duration::from_secs(5);

/// How often a *persisting* refusal run surfaces its repeat count while it
/// is still running (module docs: a sustained flood must be visible while
/// it happens, not only in the postmortem). One line per second per grant
/// is the ceiling a flood can impose on the log.
const REFUSAL_SUMMARY_INTERVAL: Duration = Duration::from_secs(1);

/// Cap on concurrently tracked refusal runs. Reaching it flushes and
/// clears every open run, which bounds the recorder's memory and still
/// amortizes to at most one extra line per refusal.
const MAX_REFUSAL_RUNS: usize = 64;

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
// Actuation detail
// ---------------------------------------------------------------------------

/// What an actuation actually did, beyond naming its verb -- the `input`
/// member of a `use_decision`. See the module docs for the secrecy
/// judgement this type encodes: pointer parameters in full, typed text by
/// shape only.
///
/// The `Text` variant has **no field a string can be put in**, so
/// "the log is not a keylogger" is a property of the type rather than a
/// convention a later edit could quietly drop -- the same structural trick
/// [`Event::HandshakeRefused`] uses for credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActuationDetail {
    /// `vitrin_actuator_pointer.move`, as requested (pre-clamp).
    Motion { x: i64, y: i64 },
    /// `vitrin_actuator_pointer.button`: evdev code plus edge.
    Button { button: u32, pressed: bool },
    /// `vitrin_actuator_pointer.scroll`: axis plus high-resolution value.
    Scroll { axis: Axis, value120: i32 },
    /// `vitrin_layout_arrange.set_fullscreen`: which of the two
    /// arrangements was asked for. The audit question a reader brings to a
    /// layout entry is "what did this principal do to the human's screen",
    /// and the verb alone cannot distinguish filling the output from
    /// giving it back.
    ///
    /// `vitrin_layout_focus.focus` has **no** variant here on purpose: it
    /// takes no argument and names no realm, so the `use_decision` entry's
    /// verb and the grant row it already carries say everything there is
    /// to say. A variant with no fields would be noise in every entry.
    Arrange { fullscreen: bool },
    /// `vitrin_actuator_text.type` -- shape only, never the string.
    Text {
        /// Unicode scalar values, the unit a human reasons in.
        chars: u64,
        /// UTF-8 bytes, the unit the wire and the digest agree on.
        bytes: u64,
        /// Over the UTF-8 bytes: identifies the string without holding it.
        digest: ObservationDigest,
    },
}

impl ActuationDetail {
    /// Summarize one use's payload, or `None` for a capture (which has a
    /// `frame` instead).
    ///
    /// Lives here rather than in [`crate::enforcement`] on purpose: what
    /// the recorder may write down is the recorder's judgement, so the
    /// secrecy decision sits in the module whose docs argue it, and the
    /// chokepoint keeps knowing nothing about a log existing.
    pub fn of(kind: &UseKind) -> Option<Self> {
        match kind {
            // A capture has a `frame` instead; a launch has no payload at
            // all -- `vitrin_launcher.launch` takes no arguments, which is
            // the security property rather than an economy (the template
            // names the program and no command ever crosses the wire), so
            // there is nothing for the log to summarize beyond the verb
            // the `use_decision` entry already names.
            UseKind::Capture | UseKind::Launch | UseKind::LayoutFocus => None,
            UseKind::LayoutArrange(mode) => Some(Self::Arrange {
                fullscreen: matches!(mode, crate::enforcement::LayoutMode::Fullscreen),
            }),
            UseKind::Pointer(input) | UseKind::Text(input) => match input {
                // The wire carries `i32` and the intake widens it, so the
                // narrowing is exact for every value that can reach here.
                SeatInputKind::Motion { x, y } => Some(Self::Motion {
                    x: *x as i64,
                    y: *y as i64,
                }),
                SeatInputKind::Button { button, state } => Some(Self::Button {
                    button: *button,
                    pressed: matches!(state, ButtonState::Pressed),
                }),
                SeatInputKind::Scroll { axis, value120 } => Some(Self::Scroll {
                    axis: *axis,
                    value120: *value120,
                }),
                SeatInputKind::Text { text } => Some(Self::Text {
                    chars: text.chars().count() as u64,
                    bytes: text.len() as u64,
                    digest: ObservationDigest::of(text.as_bytes()),
                }),
                // Physical key input never reaches a facet use (the agent
                // text path is `Text`); typed and shape-only if it ever
                // does, so no future variant can leak by default.
                SeatInputKind::Key { .. } => None,
            },
        }
    }
}

/// The `input` member of a `use_decision`: what the actuation did, or an
/// explicit `null` for a capture.
fn write_input(out: &mut String, detail: Option<ActuationDetail>) {
    let Some(detail) = detail else {
        return field_null(out, "input");
    };
    open_object(out, "input");
    match detail {
        ActuationDetail::Motion { x, y } => {
            field_str(out, "action", "move");
            field_i64(out, "x", x);
            field_i64(out, "y", y);
        }
        ActuationDetail::Button { button, pressed } => {
            field_str(out, "action", "button");
            field_u64(out, "button", u64::from(button));
            field_bool(out, "pressed", pressed);
        }
        ActuationDetail::Scroll { axis, value120 } => {
            field_str(out, "action", "scroll");
            field_str(out, "axis", axis_label(axis));
            field_i64(out, "value120", i64::from(value120));
        }
        ActuationDetail::Arrange { fullscreen } => {
            field_str(out, "action", "set_fullscreen");
            field_bool(out, "fullscreen", fullscreen);
        }
        ActuationDetail::Text {
            chars,
            bytes,
            digest,
        } => {
            field_str(out, "action", "type");
            // Shape and identity only -- module docs. There is deliberately
            // no `text` member, in any version-1 configuration.
            field_u64(out, "chars", chars);
            field_u64(out, "bytes", bytes);
            field_str(out, "digest_alg", DIGEST_ALG);
            field_str(out, "digest", &digest.to_hex());
        }
    }
    close_object(out);
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
    /// The log resumed after a write failure: the first line written past
    /// the gap, so the hole in `seq` immediately before it is
    /// self-describing rather than something a reader must infer.
    RecordingResumed {
        /// Entries lost so far this run -- the gap's size.
        dropped_entries: u64,
        /// Which recovery attempt this was (1-based), against the run's
        /// [`RECOVERY_ATTEMPTS`] budget.
        attempt: u32,
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
        /// What the actuation did -- `None` for a capture, which carries a
        /// `frame` instead. Present on refusals too: what was *attempted*
        /// is exactly what a debugging aid is for.
        detail: Option<ActuationDetail>,
        outcome: &'a UseOutcome,
    },
    /// Repeats of a refusal run that were counted rather than written
    /// individually (module docs: bounding a refusal flood without ever
    /// letting the condition go silent). Never emitted for an admitted
    /// use -- B1 forbids aggregating those.
    UseRefusalSummary {
        connection: ConnectionId,
        grant_wire_id: u32,
        /// The row the run was judged against; `None` for a facet whose
        /// grant never resolved `granted` -- the `not_granted` flood.
        grant_row: Option<GrantId>,
        verb: Verb,
        code: Refusal,
        /// Refusals swallowed since the last line written for this run.
        repeats: u64,
        /// Refusals in the run so far, including the one written in full.
        total: u64,
    },
    /// One routed seat event the core delivered to the realm's shim, tagged
    /// with the origin intake bound (backward requirement B2). This is the
    /// audit half of the input path: [`crate::input::SeatDelivery`] makes the
    /// physical-vs-agent distinction unforgeable through the type system, and
    /// this entry is what makes it *auditable after the fact* -- an unforgeable
    /// tag nobody records is a guarantee you cannot investigate an incident
    /// with.
    ///
    /// **Shape only, never payload**: the event *kind* and its origin, with no
    /// coordinates, keysym, or typed bytes. An agent's own actuations are
    /// already reproduced in full by [`Event::UseDecision`] at the chokepoint;
    /// what this adds is the delivery-point record (post-routing -- what
    /// actually went to the shim's seat) and, once physical input exists
    /// (Phase 3), the origin of events that never cross a chokepoint at all.
    /// Recording raw keysyms or typed bytes here would make the recorder a
    /// keylogger, which the secrecy contract [`ActuationDetail`] documents
    /// forbids.
    SeatDelivered {
        /// **Which realm's app received it** -- the one fact that makes this
        /// entry answerable rather than merely present. Chosen at *runtime*
        /// by whichever addressing rule the event answers to: an agent's
        /// actuation goes to the realm its grant names, a human's physical
        /// input to the realm their attention is bound to
        /// (`session::route_seat` and `session::physical_seat_target`).
        /// Physical input crosses no chokepoint at all, so for half these
        /// entries there is no grant row to derive a realm from; without this
        /// field the journal can say a keystroke was delivered and not which
        /// app got it, which is not an audit trail.
        realm: &'a RealmId,
        /// The seat event kind -- `motion`/`button`/`scroll`/`key`/`text`,
        /// from [`crate::input::SeatDelivery::event_label`].
        event: &'static str,
        /// `physical` or `emulated`: the tag intake bound, never reconstructed
        /// here (B2).
        origin: &'static str,
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
        /// What decided ([`REVOKE_CAUSE_DEAD_MAN`] /
        /// [`REVOKE_CAUSE_OPERATOR`]) -- so each line says why on its own
        /// rather than by adjacency to another entry.
        cause: &'static str,
    },
    /// The human held the dead-man chord to completion (P1.7.3): every
    /// delegated authority in the session was revoked and every pending
    /// petition denied, on a physical gesture nothing can veto.
    ///
    /// Written **before** the `grant_revoked` lines it explains, and written
    /// **even when it revoked nothing**. The second half is the load-bearing
    /// one: a session where the human hit the off-switch and it had no
    /// authority to destroy would otherwise be indistinguishable, from the
    /// log alone, from one where they never touched it -- so "the switch
    /// works" would be unverifiable exactly when it mattered most.
    DeadManTriggered {
        /// The configured chord's name (`esc`), from a closed vocabulary
        /// ([`crate::deadman::Chord`]) -- never free-form text.
        chord: &'static str,
        /// How long the key was **measured** to be held when the switch
        /// fired, not how long it was configured to need. A late elapse
        /// check (a coalesced timer, a stalled frame) reports the real
        /// number; a replay reading the configured one would be reading a
        /// value nobody observed.
        held_ms: u64,
        /// Rows this gesture newly revoked; each also gets its own
        /// `grant_revoked` line.
        revoked_grants: usize,
        /// Pending petitions this gesture denied. They resolve through the
        /// ordinary human-decision path, so each appears in its own
        /// `petition_resolved` entry at delivery.
        denied_petitions: usize,
    },
    /// One grant row deleted outright by connection teardown -- the way a
    /// version-1 grant most commonly dies (they die with their
    /// connection).
    ///
    /// Named per row for the same reason revocation and expiry are: an
    /// E3.4 replay reconstructs the grant table by applying lifecycle
    /// transitions, and a bare count on the teardown line says *how many*
    /// authorities died without saying *which*. Teardown was the one
    /// transition still leaving no per-row line; it no longer is. The
    /// count on [`Event::ConnectionTeardown`] stays, as the summary it
    /// always was.
    GrantRemoved {
        connection: ConnectionId,
        grant_id: GrantId,
    },
    /// A resolution this core decided was **not delivered** to its
    /// connection -- and therefore appears in no `petition_resolved`
    /// entry, because no authority changed.
    ///
    /// The decision is consumed from the pending registry *before*
    /// delivery is attempted, so a refused delivery destroys it: the
    /// petition is gone from the registry, the handle never resolves, and
    /// without this entry a human's yes or no would be unrecoverable from
    /// the log. Recorded from one place --
    /// `PrincipalServer::deliver_resolution`'s error funnel -- so every
    /// refusal reason, present and future, is covered structurally rather
    /// than by remembering to add a call.
    PetitionUndelivered {
        /// The connection the resolution was addressed to (not
        /// necessarily the one that refused it -- see `reason`).
        connection: ConnectionId,
        grant_wire_id: u32,
        /// What was decided, so the yes/no survives.
        outcome: Outcome,
        /// The authority a granted decision would have conferred.
        effective: Option<EffectiveAuthority>,
        /// Which consent path decided; `Some` exactly for `granted`.
        issuer: Option<Issuer>,
        /// A fixed class label, never free-form `Display` text.
        reason: &'static str,
    },
    /// A realm's app was launched (P1.5.2): the identity-at-fork moment.
    /// The reconstruction needs it because every later realm fact -- which
    /// process painted, what a capture observed -- hangs off this pid, and
    /// because *what the trusted core executed* is the single most
    /// security-relevant act of a session.
    RealmSpawned {
        realm: &'a RealmId,
        /// The shim's pid: the process holding the other end of the
        /// identity socketpair.
        pid: u32,
        command: &'a Path,
        /// The realm's private runtime directory (mode `0700`) -- where the
        /// app-facing socket the child's `WAYLAND_DISPLAY` names lives.
        runtime_dir: &'a Path,
        /// Environment variable **names** passed through from the core's
        /// environment -- never their values. A value is whatever the
        /// operator's session holds (`SSH_AUTH_SOCK`, an API token in a
        /// generously-configured allowlist) and the log's secrecy contract
        /// applies to it exactly as it does to credential bytes.
        env_allow: &'a [String],
    },
    /// A realm's app could not be launched. Recorded because a refusal is
    /// the *fail-closed* outcome: nothing was created, so nothing else in
    /// the log would otherwise say the realm was ever meant to run.
    RealmSpawnFailed {
        realm: &'a RealmId,
        command: &'a Path,
        /// A fixed label from [`crate::spawn::SpawnError::cause_class`] --
        /// never free-form `Display` text, per this log's convention.
        cause_class: &'static str,
    },
    /// A realm lost its surface (P1.5.3): its shim connection ended, or its
    /// process did, and the realm is over -- the MVP has no restart policy.
    ///
    /// **The security-relevant half of a realm's death**, and the reason it
    /// is a separate entry from [`Event::RealmExited`] rather than one entry
    /// carrying an exit status. This is the instant the surface left the
    /// scene and every subsequent capture began refusing `no_surface`; the
    /// exit status may not be known yet (a shim that closes its core
    /// connection but keeps running is dead to the core for a whole grace
    /// period before it is a corpse), and a log that waited for it would
    /// leave the authority-visible transition unrecorded for exactly as
    /// long as the anomaly lasted. Two facts, two entries, each written
    /// when it becomes true.
    ///
    /// Emitted **once** per realm death: [`crate::lifecycle`] latches the
    /// transition, so the EOF and the `SIGCHLD` for one death produce one
    /// of these no matter which arrives first, or whether both do.
    RealmDied {
        realm: &'a RealmId,
        /// The shim that was serving the realm. It may already be reaped.
        pid: u32,
        /// Which observation ended the realm, from a closed vocabulary
        /// ([`crate::lifecycle::DeathCause::label`]) -- never free-form
        /// `Display` text, per this log's convention.
        cause: &'static str,
    },
    /// A realm's shim process was reaped (P1.5.3): the bookkeeping half of
    /// the death above, carrying the classified exit status. Its presence
    /// is also the log's evidence that **no zombie was left behind** -- the
    /// core waited on this pid and collected it.
    RealmExited {
        realm: &'a RealmId,
        pid: u32,
        /// `exited`, `signaled`, or `unknown`
        /// ([`crate::lifecycle::ExitClass`]).
        disposition: &'static str,
        /// The exit code, for `exited`; `null` otherwise.
        code: Option<i32>,
        /// The terminating signal number, for `signaled`; `null` otherwise.
        /// A `kill -9` of a shim mid-frame lands here as 9.
        signal: Option<i32>,
        /// Whether the core sent that signal itself (the shutdown ladder)
        /// or merely observed it (a crash). The difference between "we
        /// terminated the realm" and "the realm died" is not recoverable
        /// from the signal number alone -- SIGKILL is both the ladder's
        /// last rung and the classic external `kill -9`.
        core_initiated: bool,
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
            Event::RecordingResumed { .. } => "recording_resumed",
            Event::HandshakeBound { .. } => "handshake_bound",
            Event::HandshakeRefused { .. } => "handshake_refused",
            Event::PetitionRequested { .. } => "petition_requested",
            Event::ConsentTransition { .. } => "consent_transition",
            Event::PetitionResolved { .. } => "petition_resolved",
            Event::PetitionUndelivered { .. } => "petition_undelivered",
            Event::UseDecision { .. } => "use_decision",
            Event::UseRefusalSummary { .. } => "use_refusal_summary",
            Event::SeatDelivered { .. } => "seat_delivered",
            Event::GrantSpent { .. } => "grant_spent",
            Event::GrantExpired { .. } => "grant_expired",
            Event::GrantRevoked { .. } => "grant_revoked",
            Event::DeadManTriggered { .. } => "dead_man_triggered",
            Event::GrantRemoved { .. } => "grant_removed",
            Event::RealmSpawned { .. } => "realm_spawned",
            Event::RealmSpawnFailed { .. } => "realm_spawn_failed",
            Event::RealmDied { .. } => "realm_died",
            Event::RealmExited { .. } => "realm_exited",
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
            Event::RecordingResumed {
                dropped_entries,
                attempt,
            } => {
                field_u64(out, "dropped_entries", dropped_entries);
                field_u64(out, "attempt", u64::from(attempt));
                field_u64(out, "attempt_budget", u64::from(RECOVERY_ATTEMPTS));
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
                write_effective(out, effective);
            }
            Event::PetitionUndelivered {
                connection,
                grant_wire_id,
                outcome,
                effective,
                issuer,
                reason,
            } => {
                field_display(out, "connection", connection);
                field_u64(out, "grant_wire_id", u64::from(grant_wire_id));
                field_str(out, "outcome", outcome_label(outcome));
                field_str(out, "reason", reason);
                // No row was minted (except on `transport`, where the
                // `petition_resolved` beside this line names it), so there
                // is nothing to name here.
                field_null(out, "grant_id");
                match issuer {
                    Some(i) => field_str(out, "issuer", issuer_label(i)),
                    None => field_null(out, "issuer"),
                }
                write_effective(out, effective);
            }
            Event::UseDecision {
                connection,
                facet_wire_id,
                grant_wire_id,
                verb,
                grant_row,
                detail,
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
                // What was actuated (or `null` for a capture, whose
                // `frame` above answers the same question).
                write_input(out, detail);
                write_epoch_reference(out);
            }
            Event::UseRefusalSummary {
                connection,
                grant_wire_id,
                grant_row,
                verb,
                code,
                repeats,
                total,
            } => {
                field_display(out, "connection", connection);
                field_u64(out, "grant_wire_id", u64::from(grant_wire_id));
                match grant_row {
                    Some(id) => field_display(out, "grant_id", id),
                    None => field_null(out, "grant_id"),
                }
                field_str(out, "verb", verb_label(verb));
                field_str(out, "decision", "refused");
                field_str(out, "refusal", refusal_label(code));
                field_u64(out, "repeats", repeats);
                field_u64(out, "total_in_run", total);
            }
            Event::SeatDelivered {
                realm,
                event,
                origin,
            } => {
                field_display(out, "realm", realm);
                field_str(out, "event", event);
                field_str(out, "origin", origin);
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
            Event::GrantRevoked {
                grant_id,
                scope,
                cause,
            } => {
                field_display(out, "grant_id", grant_id);
                field_str(out, "transition", "active_to_revoked");
                field_str(out, "scope", scope);
                field_str(out, "cause", cause);
            }
            Event::DeadManTriggered {
                chord,
                held_ms,
                revoked_grants,
                denied_petitions,
            } => {
                field_str(out, "chord", chord);
                field_u64(out, "held_ms", held_ms);
                field_u64(out, "revoked_grants", revoked_grants as u64);
                field_u64(out, "denied_petitions", denied_petitions as u64);
            }
            Event::GrantRemoved {
                connection,
                grant_id,
            } => {
                field_display(out, "connection", connection);
                field_display(out, "grant_id", grant_id);
                // Removal, not revocation: the row is deleted, not marked
                // dead (the grant table's documented teardown contract).
                field_str(out, "transition", "active_to_removed");
                field_str(out, "source", "connection_teardown");
            }
            Event::RealmSpawned {
                realm,
                pid,
                command,
                runtime_dir,
                env_allow,
            } => {
                field_display(out, "realm", realm);
                field_u64(out, "pid", u64::from(pid));
                field_display(out, "command", command.display());
                field_display(out, "runtime_dir", runtime_dir.display());
                // Names only -- see the variant's docs. The core's two
                // injections are stated as facts of the model rather than
                // as data, because they are not configurable: the child's
                // WAYLAND_DISPLAY is always its own shim's socket inside
                // `runtime_dir`, and its XDG_RUNTIME_DIR is always
                // `runtime_dir` itself.
                write_string_array(out, "env_allow", env_allow);
                field_bool(out, "env_cleared", true);
            }
            Event::RealmSpawnFailed {
                realm,
                command,
                cause_class,
            } => {
                field_display(out, "realm", realm);
                field_display(out, "command", command.display());
                field_str(out, "cause_class", cause_class);
                field_null(out, "pid");
            }
            Event::RealmDied { realm, pid, cause } => {
                field_display(out, "realm", realm);
                field_u64(out, "pid", u64::from(pid));
                field_str(out, "cause", cause);
                // Stated as a fact of this build rather than left for a
                // reader to infer from the absence of a later spawn: the
                // MVP has no restart policy, so this death is terminal and
                // the realm stops admitting petitions here.
                field_bool(out, "restarting", false);
            }
            Event::RealmExited {
                realm,
                pid,
                disposition,
                code,
                signal,
                core_initiated,
            } => {
                field_display(out, "realm", realm);
                field_u64(out, "pid", u64::from(pid));
                field_str(out, "disposition", disposition);
                match code {
                    Some(code) => field_i64(out, "code", i64::from(code)),
                    None => field_null(out, "code"),
                }
                match signal {
                    Some(signal) => field_i64(out, "signal", i64::from(signal)),
                    None => field_null(out, "signal"),
                }
                field_bool(out, "core_initiated", core_initiated);
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

/// The `effective` member: the authority a granted decision states (or
/// would have stated, for one that never reached its connection). Shared
/// by `petition_resolved` and `petition_undelivered` so the two render
/// identically -- a reader parses one shape.
fn write_effective(out: &mut String, effective: Option<EffectiveAuthority>) {
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
///
/// Every verb the IDL defines is named here, including the ones this core
/// refuses `unsupported` at admission (D-017/D-018, plus `realm_launch`) -- a
/// journal entry for a refused petition must say *what was asked for*, and "a
/// defined verb rendered as no name at all" is exactly the audit gap this pair
/// of fields exists to close. Appending a verb to the IDL means appending it
/// here in the same change; the unserved-set catalogue test in
/// [`crate::consent::render`] is what notices a new bit at all.
fn write_verbs(out: &mut String, verbs: Verb) {
    key(out, "verbs");
    out.push('[');
    let mut named = 0u32;
    for (bit, name) in [
        (Verb::OBSERVE, "observe"),
        (Verb::ACTUATE_POINTER, "actuate_pointer"),
        (Verb::ACTUATE_TEXT, "actuate_text"),
        (Verb::OBSERVE_CURSOR, "observe_cursor"),
        (Verb::LAYOUT_ARRANGE, "layout_arrange"),
        (Verb::LAYOUT_FOCUS, "layout_focus"),
        (Verb::REALM_LAUNCH, "realm_launch"),
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

/// A JSON array of strings, each escaped like any other string value.
fn write_string_array(out: &mut String, k: &str, values: &[String]) {
    key(out, k);
    out.push('[');
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_string(out, value);
    }
    out.push(']');
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

/// The scroll axis of an `actuate_pointer` scroll.
fn axis_label(axis: Axis) -> &'static str {
    match axis {
        Axis::Vertical => "vertical",
        Axis::Horizontal => "horizontal",
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
        // Reachable only through `realm_launch`, which this core does not
        // serve yet -- the label exists because the match is deliberately
        // exhaustive (an appended IDL code must fail the build here rather
        // than journal as something else), not because anything emits it.
        Refusal::Capacity => "capacity",
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
        Outcome::LayoutHeld => "layout_held",
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

/// [`Event::PetitionUndelivered::reason`] classes -- a fixed taxonomy, so
/// the entry never embeds a `Display` rendering that could carry
/// client-controlled text.
///
/// The connection was no longer bound (the fatal goodbye, or teardown):
/// the decision was consumed from the pending registry and is gone. This
/// is the common one -- an agent that dies while its prompt is up.
pub(crate) const UNDELIVERED_CONNECTION_DEAD: &str = "connection_dead";

/// The resolution was addressed to a different connection: a routing bug
/// in the embedder, recorded rather than swallowed.
pub(crate) const UNDELIVERED_WRONG_CONNECTION: &str = "wrong_connection";

/// The grant handle the resolution names is not a grant object on this
/// connection.
pub(crate) const UNDELIVERED_UNKNOWN_GRANT: &str = "unknown_grant_object";

/// The handle had already resolved: the exactly-once guard refused a
/// second resolution before anything was minted or sent.
pub(crate) const UNDELIVERED_ALREADY_RESOLVED: &str = "already_resolved";

/// The grant-table insert failed at delivery time (unreachable in
/// practice; the connection dies fatal `internal`).
pub(crate) const UNDELIVERED_INSERT_FAILED: &str = "insert_failed";

/// The decision was recorded and the row minted, but the terminal never
/// reached the wire. Unlike every other class this one accompanies a
/// `petition_resolved` entry: authority *did* change, the client just
/// never learned of it.
pub(crate) const UNDELIVERED_TRANSPORT: &str = "transport";

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

/// One refusal run: the `(verb, code)` currently repeating on one
/// `(connection, grant)` and how many repeats have been swallowed since
/// the last line written for it. See the module docs for why refusals are
/// bounded this way and why admissions never are.
#[derive(Debug)]
struct RefusalRun {
    verb: Verb,
    code: Refusal,
    grant_row: Option<GrantId>,
    /// Swallowed since the last line written for this run.
    suppressed: u64,
    /// Refusals in the run so far, the individually written one included.
    total: u64,
    /// Monotonic offset at which the current accumulation window opened.
    window_opened: Duration,
}

/// The one flight-recorder handle of a core process (module docs: there are
/// deliberately no other write sites). One log file per run.
#[derive(Debug)]
pub(crate) struct Recorder {
    /// `None` while a write failure has this recorder latched degraded --
    /// no write is attempted at capture rate, so a full disk cannot
    /// produce an error storm. Lifted only by [`Recorder::try_recover`],
    /// on the bounded budget below.
    file: Option<File>,
    path: PathBuf,
    run_id: String,
    /// Next `seq`; strictly increasing from 1, never reused, and assigned
    /// even for entries that are then dropped, so a gap in `seq` is exactly
    /// the evidence that entries were lost.
    seq: u64,
    started: Instant,
    dropped: u64,
    /// Recovery attempts spent this run, against [`RECOVERY_ATTEMPTS`].
    recovery_attempts: u32,
    /// Monotonic offset before which no further recovery may be attempted
    /// ([`RECOVERY_BACKOFF`] after the last one).
    recovery_not_before: Duration,
    /// Whether the next line must be prefixed with `\n` because a write
    /// failed part-way and may have left an unterminated fragment.
    fragment_pending: bool,
    /// Open refusal runs, keyed by `(connection, wire grant id)`. Bounded
    /// by [`MAX_REFUSAL_RUNS`].
    refusal_runs: BTreeMap<(ConnectionId, u32), RefusalRun>,
}

/// Open `path` for append with the file's privacy enforced on the
/// descriptor rather than merely requested at creation -- the module docs
/// argue why `OpenOptions::mode` alone is not enough.
fn open_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    // `fstat` through the descriptor: a symlink resolved to something that
    // is not a regular file (a fifo would block the compositor, a device
    // could be anything) is refused rather than written into.
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "flight-recorder log must be a regular file",
        ));
    }
    // `fchmod` on the fd -- no path, so no TOCTOU window -- and
    // unconditional, so it costs one syscall at startup and needs no
    // "whose uid is this" logic: a file this process cannot chmod is a
    // file it must not append identities to, and the error says so.
    let mode = meta.permissions().mode() & 0o777;
    file.set_permissions(Permissions::from_mode(0o600))?;
    if mode != 0o600 {
        tracing::warn!(
            path = %path.display(),
            previous_mode = format!("{mode:04o}"),
            "flight-recorder log existed with wider permissions; tightened to 0600"
        );
    }
    Ok(file)
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
        let file = open_append(path).map_err(fail)?;
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
            recovery_attempts: 0,
            recovery_not_before: Duration::ZERO,
            fragment_pending: false,
            refusal_runs: BTreeMap::new(),
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

    /// Recovery attempts spent this run -- how the bounding of
    /// [`Recorder::try_recover`] is asserted rather than assumed.
    #[cfg(test)]
    pub fn recovery_attempts(&self) -> u32 {
        self.recovery_attempts
    }

    /// Record one proactive expiry sweep's result -- the ids
    /// [`GrantTable::expire_due`](crate::grants::GrantTable::expire_due)
    /// just flipped, i.e. grants that died *without* a use and would
    /// otherwise appear in no entry at all. One call per sweep, so the
    /// runtime's armed calloop timer (`session::sweep`) has exactly one
    /// recorder call shape and no loop of its own; an empty sweep writes
    /// nothing, which is what lets that timer run every second without
    /// turning a quiet session's log into a heartbeat file.
    pub fn record_expiry_sweep(&mut self, expired: &[GrantId]) {
        for &grant_id in expired {
            self.record(Event::GrantExpired { grant_id });
        }
    }

    /// Record one revocation's result: the ids
    /// [`GrantTable::revoke`](crate::grants::GrantTable::revoke) or
    /// [`GrantTable::revoke_principal`](crate::grants::GrantTable::revoke_principal)
    /// newly revoked, tagged with how wide the act was
    /// ([`REVOKE_SCOPE_GRANT`] / [`REVOKE_SCOPE_PRINCIPAL`]) and what
    /// decided it ([`REVOKE_CAUSE_DEAD_MAN`] / [`REVOKE_CAUSE_OPERATOR`]).
    ///
    /// An empty slice writes nothing, which is why the dead-man switch
    /// records its own [`Event::DeadManTriggered`] separately: a gesture
    /// that revoked nothing still happened.
    pub fn record_revocations(
        &mut self,
        revoked: &[GrantId],
        scope: &'static str,
        cause: &'static str,
    ) {
        for &grant_id in revoked {
            self.record(Event::GrantRevoked {
                grant_id,
                scope,
                cause,
            });
        }
    }

    /// Record one entry: assemble the whole line in memory, then hand it to
    /// a single `write(2)`. Infallible by design -- a diagnostic must never
    /// be able to fail an authority path (module docs), so a write failure
    /// degrades loudly here instead of propagating to the caller.
    ///
    /// This is also where the two bounding policies live, both of them
    /// *before* any line is rendered so neither can be bypassed by a
    /// future caller: the degraded-recorder recovery attempt, and the
    /// refusal-run aggregation. An [`UseOutcome::Admitted`] entry passes
    /// through both untouched -- B1.
    pub fn record(&mut self, event: Event<'_>) {
        // A degraded recorder gets its bounded chance to come back before
        // this entry is considered lost, so recovery lands *inside* the
        // gap it describes rather than after it.
        self.try_recover(false);
        // Refusal bounding, and the run-ending flushes that make an
        // aggregated count land before the line that ended its run.
        if !self.admit_use_entry(&event) {
            return;
        }
        let line = self.render(&event);
        self.write_line(&line);
    }

    /// Close the run: one *forced* recovery attempt (shutdown happens
    /// once, so a single extra `open`+`write` cannot storm), every open
    /// refusal run flushed, then the footer. A transient write failure
    /// therefore still ends with a `run_ended` naming the total lost --
    /// the file-only evidence the module docs promise.
    pub fn finish(&mut self) {
        self.try_recover(true);
        self.flush_all_refusal_runs();
        let dropped = self.dropped;
        self.record(Event::RunEnded {
            dropped_entries: dropped,
        });
    }

    /// Assemble one complete line: the envelope, then the kind-specific
    /// body. Consumes the entry's `seq` whether or not the line is
    /// ultimately written, so a dropped entry leaves a hole rather than a
    /// silent renumbering.
    fn render(&mut self, event: &Event<'_>) -> String {
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
        line
    }

    /// Try to lift a degraded latch, on the budget the module docs state:
    /// at most [`RECOVERY_ATTEMPTS`] per run and never two within
    /// [`RECOVERY_BACKOFF`] -- unless `forced`, which [`Recorder::finish`]
    /// uses for its single shutdown attempt.
    ///
    /// A successful reopen writes [`Event::RecordingResumed`] first, so
    /// the gap in `seq` immediately before it is self-describing.
    fn try_recover(&mut self, forced: bool) {
        if self.file.is_some() {
            return;
        }
        let elapsed = self.started.elapsed();
        if !forced
            && (self.recovery_attempts >= RECOVERY_ATTEMPTS || elapsed < self.recovery_not_before)
        {
            return;
        }
        self.recovery_attempts += 1;
        self.recovery_not_before = elapsed.saturating_add(RECOVERY_BACKOFF);
        let Ok(file) = open_append(&self.path) else {
            return;
        };
        self.file = Some(file);
        let resumed = Event::RecordingResumed {
            dropped_entries: self.dropped,
            attempt: self.recovery_attempts,
        };
        let line = self.render(&resumed);
        // Straight to the write site: `record` would re-enter recovery.
        // If this write fails too, the latch simply re-arms and the
        // attempt is spent -- which is what bounds the retry.
        self.write_line(&line);
        if self.file.is_some() {
            tracing::warn!(
                path = %self.path.display(),
                dropped_entries = self.dropped,
                attempt = self.recovery_attempts,
                "flight recorder RESUMED after a write failure; the gap in `seq` before this \
                 point is the entries lost while degraded"
            );
        }
    }

    /// The refusal-flood bound (module docs). Returns whether `event`
    /// should be written as its own line.
    ///
    /// **B1 lives in the first arm**: an admitted use is always written,
    /// and the only thing its arm does besides return `true` is *end* any
    /// refusal run on that grant -- it is structurally impossible for this
    /// function to suppress an admission.
    fn admit_use_entry(&mut self, event: &Event<'_>) -> bool {
        match *event {
            Event::UseDecision {
                connection,
                grant_wire_id,
                outcome: &UseOutcome::Admitted { .. },
                ..
            } => {
                // A success ends the run, exactly as it clears the
                // chokepoint's wire-side coalescing marks.
                self.flush_refusal_run(&(connection, grant_wire_id));
                true
            }
            Event::UseDecision {
                connection,
                grant_wire_id,
                verb,
                grant_row,
                outcome: &UseOutcome::Refused { code, .. },
                ..
            } => self.note_refusal(connection, grant_wire_id, grant_row, verb, code),
            // A closing connection's runs are summarized before its
            // teardown line, so the story ends with nothing outstanding.
            Event::ConnectionTeardown { connection, .. } => {
                let keys: Vec<(ConnectionId, u32)> = self
                    .refusal_runs
                    .range((connection, 0)..=(connection, u32::MAX))
                    .map(|(k, _)| *k)
                    .collect();
                for key in keys {
                    self.flush_refusal_run(&key);
                }
                true
            }
            _ => true,
        }
    }

    /// Fold one refusal into its run. Returns whether it earns its own
    /// line: `true` for the first refusal of a run (and for the first
    /// after a run's key changes), `false` for a repeat, whose count
    /// surfaces in a [`Event::UseRefusalSummary`] instead.
    fn note_refusal(
        &mut self,
        connection: ConnectionId,
        grant_wire_id: u32,
        grant_row: Option<GrantId>,
        verb: Verb,
        code: Refusal,
    ) -> bool {
        let key = (connection, grant_wire_id);
        let now = self.started.elapsed();
        if let Some(run) = self.refusal_runs.get_mut(&key) {
            if run.verb == verb && run.code == code {
                run.total += 1;
                run.suppressed += 1;
                // A *sustained* flood must stay visible while it is
                // happening, so a long run reports periodically instead of
                // only when it ends.
                if now.saturating_sub(run.window_opened) >= REFUSAL_SUMMARY_INTERVAL {
                    self.emit_refusal_summary(key, now);
                }
                return false;
            }
            // The key changed: the old run ends here, and this refusal
            // begins a new one and is written in full.
            self.flush_refusal_run(&key);
        }
        if self.refusal_runs.len() >= MAX_REFUSAL_RUNS {
            // Bounded memory. Flushing everything costs at most
            // MAX_REFUSAL_RUNS lines and can only recur after that many
            // new keys, so it amortizes to <= 1 extra line per refusal.
            self.flush_all_refusal_runs();
        }
        self.refusal_runs.insert(
            key,
            RefusalRun {
                verb,
                code,
                grant_row,
                suppressed: 0,
                total: 1,
                window_opened: now,
            },
        );
        true
    }

    /// Write the pending summary for one run (if it swallowed anything)
    /// and open a fresh accumulation window, keeping the run itself.
    fn emit_refusal_summary(&mut self, key: (ConnectionId, u32), now: Duration) {
        let Some(run) = self.refusal_runs.get_mut(&key) else {
            return;
        };
        if run.suppressed == 0 {
            // The individual line already told the whole story; a summary
            // of nothing would be noise.
            run.window_opened = now;
            return;
        }
        let summary = Event::UseRefusalSummary {
            connection: key.0,
            grant_wire_id: key.1,
            grant_row: run.grant_row,
            verb: run.verb,
            code: run.code,
            repeats: run.suppressed,
            total: run.total,
        };
        run.suppressed = 0;
        run.window_opened = now;
        let line = self.render(&summary);
        self.write_line(&line);
    }

    /// End one run: emit its outstanding count, then forget it.
    fn flush_refusal_run(&mut self, key: &(ConnectionId, u32)) {
        if !self.refusal_runs.contains_key(key) {
            return;
        }
        self.emit_refusal_summary(*key, self.started.elapsed());
        self.refusal_runs.remove(key);
    }

    /// End every open run (the cap, and shutdown).
    fn flush_all_refusal_runs(&mut self) {
        let keys: Vec<(ConnectionId, u32)> = self.refusal_runs.keys().copied().collect();
        for key in keys {
            self.flush_refusal_run(&key);
        }
    }

    /// The single write site. One `write_all` of one complete, already
    /// `\n`-terminated line on an `O_APPEND` fd -- the line-atomicity
    /// property the module docs describe.
    fn write_line(&mut self, line: &str) {
        let Some(file) = self.file.as_mut() else {
            self.dropped += 1;
            return;
        };
        // A previous failure may have landed a partial line; open this one
        // on a fresh line so the fragment cannot swallow it (module docs).
        let fragment_pending = std::mem::take(&mut self.fragment_pending);
        let result = if fragment_pending {
            file.write_all(b"\n")
                .and_then(|()| file.write_all(line.as_bytes()))
        } else {
            file.write_all(line.as_bytes())
        };
        if let Err(err) = result {
            // `write_all` loops over short writes, so a failure part-way
            // through can have left a prefix on disk. Terminate it as its
            // own (invalid) line, best effort: without this the *next*
            // appender's first line would be glued onto the fragment.
            let terminated = file.write_all(b"\n").is_ok();
            self.fragment_pending = !terminated;
            // Loud, once. The recorder is latched degraded so a full disk
            // cannot turn every subsequent capture into another error line.
            tracing::error!(
                path = %self.path.display(),
                error = %err,
                "flight recorder write failed; recording is now DEGRADED (entries will be \
                 counted, not written) until one of the bounded recovery attempts succeeds. \
                 Captures and actuations are unaffected -- the flight recorder v0 is a \
                 debugging aid, not an authority input."
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

    /// Every line of the log at `path`, parsed, asserting `seq` is
    /// gap-free -- the normal, undegraded case every test but the
    /// degradation ones expects.
    pub(crate) fn read_log(path: &Path) -> Vec<Json> {
        let entries = read_log_allowing_gaps(path);
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.u64("seq"), i as u64 + 1, "seq is gap-free and ascending");
        }
        entries
    }

    /// Every line of the log at `path`, parsed, tolerating the gaps a
    /// degraded run leaves. Asserts each line is exactly one JSON object
    /// carrying the envelope, and that `seq` is strictly increasing (the
    /// ordering authority is never reused), so no test has to restate the
    /// invariant. Empty lines are skipped, per the reader rules in the
    /// module docs.
    pub(crate) fn read_log_allowing_gaps(path: &Path) -> Vec<Json> {
        let text = std::fs::read_to_string(path).expect("log file must be readable");
        let mut entries = Vec::new();
        let mut previous_seq = 0u64;
        for (i, line) in text.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let value = Json::parse(line)
                .unwrap_or_else(|e| panic!("line {} is not valid JSON ({e}): {line}", i + 1));
            assert_eq!(
                value.u64("schema_version"),
                u64::from(SCHEMA_VERSION),
                "every line carries schema_version"
            );
            let seq = value.u64("seq");
            assert!(
                seq > previous_seq,
                "seq is strictly increasing and never reused ({seq} after {previous_seq})"
            );
            previous_seq = seq;
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
    fn a_seat_delivery_records_its_kind_and_origin_and_no_payload() {
        let _fd = crate::capture::tests::fd_lock();
        // Issue #83: the delivery-point audit entry carries the origin tag
        // (B2) and the event kind -- and, deliberately, nothing else. A
        // recorder that wrote keysyms or the typed string here would be a
        // keylogger, so the entry has no field that could hold either.
        let (mut rec, path) = scratch_recorder("seat-delivered");
        let editor = crate::grants::RealmId::new("editor");
        let browser = crate::grants::RealmId::new("browser");
        rec.record(Event::SeatDelivered {
            realm: &editor,
            event: "text",
            origin: "emulated",
        });
        rec.record(Event::SeatDelivered {
            realm: &browser,
            event: "button",
            origin: "physical",
        });

        let entries = read_log(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].str("kind"), "seat_delivered");
        assert_eq!(entries[0].str("event"), "text");
        assert_eq!(entries[0].str("origin"), "emulated");
        // Which app received it -- the question the entry could not answer
        // before, and the one an incident starts from.
        assert_eq!(entries[0].str("realm"), "editor");
        assert_eq!(entries[1].str("event"), "button");
        assert_eq!(entries[1].str("origin"), "physical");
        assert_eq!(entries[1].str("realm"), "browser");

        // Shape only: no member that could carry a coordinate, keysym, scroll
        // delta, or the typed string's bytes/digest.
        let raw = std::fs::read_to_string(&path).unwrap();
        for forbidden in ["keysym", "chars", "digest", "value120"] {
            assert!(
                !raw.contains(forbidden),
                "a seat-delivery entry must not carry payload; found {forbidden}"
            );
        }
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
        for (verb, grant_row, detail, outcome) in [
            (Verb::OBSERVE, live_row, None, &admitted),
            (
                Verb::ACTUATE_POINTER,
                live_row,
                Some(ActuationDetail::Motion { x: 3, y: 4 }),
                &refused,
            ),
            (
                Verb::ACTUATE_TEXT,
                None,
                Some(ActuationDetail::Text {
                    chars: 5,
                    bytes: 5,
                    digest: ObservationDigest::of(b"hello"),
                }),
                &not_granted,
            ),
        ] {
            rec.record(Event::UseDecision {
                connection: ConnectionId::from_u64_for_test(1),
                facet_wire_id: 20,
                grant_wire_id: 10,
                verb,
                grant_row,
                detail,
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

    /// A recorder whose fd rejects writes *and* whose path cannot be
    /// reopened: the unrecoverable case, where degradation is permanent.
    /// The path is a directory, so `open_append` fails every time.
    fn unrecoverable_recorder(label: &str) -> (Recorder, PathBuf) {
        let dir = scratch_log_path(label)
            .parent()
            .expect("scratch paths have a parent")
            .to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let probe = dir.join("probe");
        std::fs::write(&probe, b"").unwrap();
        // A read-only handle on a real file: every write fails EBADF,
        // deterministically and on every platform this core targets.
        let read_only = File::open(&probe).expect("open the probe read-only");
        (Recorder::with_file(read_only, dir.clone()), dir)
    }

    #[test]
    fn a_write_failure_degrades_loudly_and_never_halts_the_caller() {
        let _fd = crate::capture::tests::fd_lock();
        // The documented policy: the first failure latches the recorder
        // degraded, every later entry is *counted* rather than written, and
        // `record` still returns normally -- a diagnostic can never fail an
        // authority path.
        let (mut rec, dir) = unrecoverable_recorder("write-failure");
        let probe = dir.join("probe");

        assert!(!rec.is_degraded());
        for i in 1..=5u64 {
            rec.record(Event::GrantExpired {
                grant_id: GrantId::from_u64_for_test(i),
            });
            assert!(rec.is_degraded(), "the first failure latches degradation");
            assert_eq!(rec.dropped_entries(), i, "every lost entry is counted");
        }
        // Nothing reached the file, and nothing panicked.
        assert_eq!(std::fs::read_to_string(&probe).unwrap(), "");

        // The seq counter advanced anyway, so a later reader sees the gap
        // rather than a silently renumbered log.
        let recovered = Recorder::create(&probe).unwrap();
        assert_eq!(recovered.dropped_entries(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_is_bounded_so_a_permanently_failing_disk_cannot_storm() {
        let _fd = crate::capture::tests::fd_lock();
        // The counterweight to recovery existing at all: a filesystem that
        // stays broken must not buy one reopen+write per capture. The
        // backoff is what bounds a *burst* (the attempt budget bounds the
        // run), so a tight loop must spend exactly one attempt.
        let (mut rec, dir) = unrecoverable_recorder("recovery-bounded");
        for i in 1..=200u64 {
            rec.record(Event::GrantExpired {
                grant_id: GrantId::from_u64_for_test(i),
            });
        }
        assert!(rec.is_degraded(), "the failure never cleared");
        assert_eq!(rec.dropped_entries(), 200, "every entry is still counted");
        assert_eq!(
            rec.recovery_attempts(),
            1,
            "200 entries inside one backoff window must cost exactly one \
             recovery attempt, not 200"
        );
        // Shutdown gets its one forced attempt on top -- bounded because
        // shutdown happens once.
        rec.finish();
        assert_eq!(rec.recovery_attempts(), 2);
        assert!(rec.recovery_attempts() <= RECOVERY_ATTEMPTS + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transient_write_failure_leaves_an_interior_gap_and_a_footer() {
        let _fd = crate::capture::tests::fd_lock();
        // The file-only evidence of degradation, which is the whole point
        // of the policy: dropped entries must be able to form an INTERIOR
        // hole -- bracketed by real entries and named by an explicit
        // marker -- not merely a tail that ends the file indistinguishably
        // from a SIGKILL.
        let path = scratch_log_path("transient-failure");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        // A writable recorder first, so the gap has something before it.
        let mut rec = Recorder::create(&path).unwrap();
        rec.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(1),
        });
        // Now break the descriptor while leaving the path openable: the
        // transient-failure shape (a full filesystem the operator clears).
        rec.file = Some(File::open(&path).expect("a read-only handle rejects writes"));
        rec.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(2),
        });
        assert!(rec.is_degraded(), "the write failed and latched");
        assert_eq!(rec.dropped_entries(), 1);
        // The next entry finds the recorder degraded, recovers, and lands.
        rec.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(3),
        });
        assert!(!rec.is_degraded(), "recovery lifted the latch");
        rec.finish();

        let entries = read_log_allowing_gaps(&path);
        let kinds: Vec<&str> = entries.iter().map(|e| e.str("kind")).collect();
        assert_eq!(
            kinds,
            vec![
                "grant_expired",
                "recording_resumed",
                "grant_expired",
                "run_ended"
            ],
            "the marker sits INSIDE the log, between real entries"
        );
        // The gap is interior: entries exist on both sides of it, and the
        // skipped `seq` is exactly the dropped entry.
        let seqs: Vec<u64> = entries.iter().map(|e| e.u64("seq")).collect();
        assert_eq!(seqs, vec![1, 3, 4, 5], "seq 2 was consumed and lost");
        // And the gap describes itself rather than needing to be inferred.
        assert_eq!(entries[1].u64("dropped_entries"), 1);
        assert_eq!(entries[1].u64("attempt"), 1);
        // The footer -- unwritable before recovery existed -- states the
        // run total.
        assert_eq!(entries[3].str("kind"), "run_ended");
        assert_eq!(entries[3].u64("dropped_entries"), 1);
        cleanup(&path);
    }

    #[test]
    fn a_partial_line_is_terminated_so_the_next_line_stays_parseable() {
        let _fd = crate::capture::tests::fd_lock();
        // A write that lands a prefix and then fails leaves an
        // unterminated fragment. Left alone it would swallow whatever is
        // appended next -- gluing a later line onto it and producing
        // something that is neither a tolerable trailing fragment nor a
        // valid entry. Every line after the fragment must still parse.
        let path = scratch_log_path("partial-line");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Stand in for the prefix a failed `write_all` left behind.
        std::fs::write(&path, b"{\"schema_version\":1,\"run_id\":\"tru").unwrap();
        let mut rec = Recorder::with_file(
            File::open(&path).expect("a read-only handle rejects writes"),
            path.clone(),
        );
        // This write fails (and so does its `\n` terminator), so the
        // fragment is still unterminated and the recorder knows it.
        rec.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(1),
        });
        assert!(rec.is_degraded());
        // Recovery reopens the writable path; its first line must not be
        // glued onto the fragment.
        rec.record(Event::GrantExpired {
            grant_id: GrantId::from_u64_for_test(2),
        });
        assert!(!rec.is_degraded());
        rec.finish();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0], "{\"schema_version\":1,\"run_id\":\"tru",
            "the fragment stays exactly one (invalid) line"
        );
        assert!(
            Json::parse(lines[0]).is_err(),
            "and it is the one invalid line a reader must tolerate"
        );
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                continue;
            }
            Json::parse(line).unwrap_or_else(|e| {
                panic!("line {} after the fragment must parse ({e}): {line}", i + 1)
            });
        }
        // The reader helper (which skips empties) sees a clean log after
        // the fragment is discarded.
        let after_fragment: String = text
            .lines()
            .skip(1)
            .map(|l| format!("{l}\n"))
            .collect::<Vec<_>>()
            .concat();
        std::fs::write(&path, after_fragment).unwrap();
        let entries = read_log_allowing_gaps(&path);
        let kinds: Vec<&str> = entries.iter().map(|e| e.str("kind")).collect();
        assert_eq!(
            kinds,
            vec!["recording_resumed", "grant_expired", "run_ended"]
        );
        cleanup(&path);
    }

    #[test]
    fn the_log_file_is_owner_only() {
        let _fd = crate::capture::tests::fd_lock();

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

    #[test]
    fn a_pre_existing_world_readable_log_is_tightened_before_anything_is_written() {
        let _fd = crate::capture::tests::fd_lock();
        // `OpenOptions::mode` applies only when the file is CREATED, so
        // appending identities, peer uids and grant rows to an operator's
        // existing 0644 file would leave them world-readable -- the 0600
        // justification defeated by the one case it exists for.
        let path = scratch_log_path("pre-existing-mode");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644,
            "the fixture really is world-readable"
        );

        let mut rec = Recorder::create(&path).expect("an existing log is appendable");
        rec.record(Event::HandshakeBound {
            connection: ConnectionId::from_u64_for_test(1),
            peer: peer(),
            identity: &identity("vitrin://local/agent/demo"),
            credential_type: "static-token",
            credential_bytes: 32,
        });
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "the log must be private before an identity reaches it"
        );
        cleanup(&path);
    }

    #[test]
    fn a_symlinked_log_has_its_target_tightened_not_its_link() {
        let _fd = crate::capture::tests::fd_lock();
        // The finding's exact shape: `--recorder` aimed at a symlink into a
        // world-readable directory. `create(true)` follows the link, so the
        // privacy check must land on the descriptor -- and therefore on the
        // *target* -- or identities get appended to a 0644 file elsewhere.
        let dir = scratch_log_path("symlinked")
            .parent()
            .expect("scratch paths have a parent")
            .to_path_buf();
        let elsewhere = dir.join("world-readable");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, Permissions::from_mode(0o755)).unwrap();
        let target = elsewhere.join("shared.jsonl");
        std::fs::write(&target, b"").unwrap();
        std::fs::set_permissions(&target, Permissions::from_mode(0o644)).unwrap();
        let link = dir.join("log.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut rec = Recorder::create(&link).expect("a symlinked log is appendable");
        rec.record(Event::HandshakeBound {
            connection: ConnectionId::from_u64_for_test(1),
            peer: peer(),
            identity: &identity("vitrin://local/agent/demo"),
            credential_type: "static-token",
            credential_bytes: 32,
        });
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600,
            "the symlink's TARGET is what holds the identities, so it is what \
             must be private"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_target_that_is_not_a_regular_file_is_refused() {
        let _fd = crate::capture::tests::fd_lock();
        // A character device opens for append perfectly happily, so
        // without an explicit check the core would report "flight recorder
        // open" and write the session into nothing. A fifo would be worse:
        // a blocking write on the compositor thread. Neither is a log.
        let err = Recorder::create(Path::new("/dev/null"))
            .expect_err("a character device is not a log file");
        assert!(err.to_string().contains("regular file"), "{err}");
    }

    // -- actuation detail and its secrecy judgement ------------------------

    #[test]
    fn pointer_actuations_record_every_parameter_that_reconstructs_them() {
        let _fd = crate::capture::tests::fd_lock();
        // The verb alone cannot tell a move from a button press from a
        // scroll, so an entry carrying only `verb` fails the "a human can
        // reconstruct what was done" criterion for actuations even while
        // meeting it for captures. Coordinates, button codes and scroll
        // deltas leak nothing, so they are recorded in full.
        let (mut rec, path) = scratch_recorder("pointer-detail");
        let admitted = UseOutcome::Admitted {
            grant: GrantId::from_u64_for_test(1),
            frame: None,
            spent_once: false,
        };
        for (i, detail) in [
            ActuationDetail::Motion { x: -7, y: 1024 },
            ActuationDetail::Button {
                button: 0x110,
                pressed: true,
            },
            ActuationDetail::Button {
                button: 0x110,
                pressed: false,
            },
            ActuationDetail::Scroll {
                axis: Axis::Horizontal,
                value120: -240,
            },
        ]
        .into_iter()
        .enumerate()
        {
            rec.record(Event::UseDecision {
                connection: ConnectionId::from_u64_for_test(1),
                facet_wire_id: 20,
                // Distinct grants: an admitted use is never aggregated
                // anyway, but this keeps the test about detail alone.
                grant_wire_id: 10 + i as u32,
                verb: Verb::ACTUATE_POINTER,
                grant_row: Some(GrantId::from_u64_for_test(1)),
                detail: Some(detail),
                outcome: &admitted,
            });
        }

        let entries = read_log(&path);
        assert_eq!(entries.len(), 4);
        // Every one is distinguishable from the others -- the property the
        // verb alone cannot provide.
        let actions: Vec<&str> = entries.iter().map(|e| e.str("input.action")).collect();
        assert_eq!(actions, vec!["move", "button", "button", "scroll"]);
        assert_eq!(entries[0].at("input.x"), &Json::Num(-7.0));
        assert_eq!(entries[0].u64("input.y"), 1024);
        assert_eq!(entries[1].u64("input.button"), 0x110);
        assert!(entries[1].bool("input.pressed"), "press and release differ");
        assert!(!entries[2].bool("input.pressed"));
        assert_eq!(entries[3].str("input.axis"), "horizontal");
        assert_eq!(entries[3].at("input.value120"), &Json::Num(-240.0));
        cleanup(&path);
    }

    #[test]
    fn typed_text_is_recorded_by_shape_and_digest_but_never_verbatim() {
        let _fd = crate::capture::tests::fd_lock();
        // The secrecy judgement, asserted rather than documented: agent
        // text is arbitrary user data -- passwords, tokens, private
        // correspondence -- so the flight recorder records how much was
        // typed and which string it was, never the string. There is no
        // opt-in flag in v0 that changes this.
        let secret = "hunter2-\u{1f512}-correct-horse-battery-staple";
        let (mut rec, path) = scratch_recorder("text-secrecy");
        let admitted = UseOutcome::Admitted {
            grant: GrantId::from_u64_for_test(1),
            frame: None,
            spent_once: false,
        };
        let detail = ActuationDetail::of(&UseKind::Text(SeatInputKind::Text {
            text: secret.to_string(),
        }));
        rec.record(Event::UseDecision {
            connection: ConnectionId::from_u64_for_test(1),
            facet_wire_id: 20,
            grant_wire_id: 10,
            verb: Verb::ACTUATE_TEXT,
            grant_row: Some(GrantId::from_u64_for_test(1)),
            detail,
            outcome: &admitted,
        });

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains(secret),
            "typed text must NEVER appear in the log"
        );
        assert!(
            !raw.contains("hunter2"),
            "not even a prefix of it -- no truncated echo either"
        );
        let entries = read_log(&path);
        let e = &entries[0];
        assert_eq!(e.str("input.action"), "type");
        // Shape: enough to reconstruct that something substantial was
        // typed, and to pair two identical actuations.
        assert_eq!(e.u64("input.chars"), secret.chars().count() as u64);
        assert_eq!(e.u64("input.bytes"), secret.len() as u64);
        assert_eq!(e.str("input.digest_alg"), DIGEST_ALG);
        assert_eq!(
            e.str("input.digest"),
            ObservationDigest::of(secret.as_bytes()).to_hex(),
            "the digest identifies the string without holding it"
        );
        // The entry has no member a string could hide in.
        let Json::Obj(input) = e.at("input") else {
            panic!("input is an object");
        };
        let keys: Vec<&str> = input.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["action", "chars", "bytes", "digest_alg", "digest"],
            "no `text` member exists, in any configuration"
        );
        cleanup(&path);
    }

    #[test]
    fn a_capture_carries_a_frame_and_no_input_block() {
        let _fd = crate::capture::tests::fd_lock();
        let (mut rec, path) = scratch_recorder("capture-no-input");
        let admitted = UseOutcome::Admitted {
            grant: GrantId::from_u64_for_test(1),
            frame: Some(ObservedFrame {
                width: 4,
                height: 2,
                stride: 16,
                bytes: 32,
                digest: ObservationDigest::of(b"pixels"),
            }),
            spent_once: false,
        };
        assert_eq!(
            ActuationDetail::of(&UseKind::Capture),
            None,
            "a capture actuates nothing"
        );
        rec.record(Event::UseDecision {
            connection: ConnectionId::from_u64_for_test(1),
            facet_wire_id: 20,
            grant_wire_id: 10,
            verb: Verb::OBSERVE,
            grant_row: Some(GrantId::from_u64_for_test(1)),
            detail: None,
            outcome: &admitted,
        });
        let entries = read_log(&path);
        assert!(entries[0].is_null("input"), "a capture observes, not acts");
        assert!(!entries[0].is_null("frame"));
        cleanup(&path);
    }

    // -- bounding refusal floods (without ever touching B1) ----------------

    /// One refusal on `(conn-1, grant 10)`, as an ungranted facet's flood
    /// produces it: no row, `not_granted`, muted on the wire.
    fn record_not_granted(rec: &mut Recorder, verb: Verb) {
        let refused = UseOutcome::Refused {
            code: Refusal::NotGranted,
            voiced: false,
        };
        rec.record(Event::UseDecision {
            connection: ConnectionId::from_u64_for_test(1),
            facet_wire_id: 20,
            grant_wire_id: 10,
            verb,
            grant_row: None,
            detail: None,
            outcome: &refused,
        });
    }

    #[test]
    fn a_refusal_flood_costs_a_bounded_number_of_lines_but_is_never_silent() {
        let _fd = crate::capture::tests::fd_lock();
        // The chokepoint refuses `not_granted` at its FIRST step, before
        // the token bucket, so a facet whose grant never resolved granted
        // is judged with no rate ceiling at all. The wire coalesces; the
        // log must not be the remaining unbounded, unratelimited
        // disk-growth and compositor-stall vector.
        let (mut rec, path) = scratch_recorder("refusal-flood");
        const FLOOD: usize = 5_000;
        for _ in 0..FLOOD {
            record_not_granted(&mut rec, Verb::ACTUATE_POINTER);
        }
        // Nothing outstanding at shutdown.
        rec.finish();

        let entries = read_log(&path);
        let uses = of_kind(&entries, "use_decision");
        assert_eq!(
            uses.len(),
            1,
            "the first refusal of a run is written in full; repeats are not"
        );
        assert_eq!(uses[0].str("refusal"), "not_granted");
        assert!(uses[0].is_null("grant_id"), "an ungranted facet has no row");

        // ... but the condition is never silent: the repeats are counted.
        let summaries = of_kind(&entries, "use_refusal_summary");
        assert!(!summaries.is_empty(), "the flood must still be visible");
        assert!(
            entries.len() < FLOOD / 10,
            "a {FLOOD}-request flood must not buy {FLOOD} synchronous writes \
             (got {} lines)",
            entries.len()
        );
        let counted: u64 = summaries.iter().map(|s| s.u64("repeats")).sum();
        assert_eq!(
            counted + 1,
            FLOOD as u64,
            "every refusal is accounted for -- one in full, the rest counted"
        );
        let last = summaries.last().unwrap();
        assert_eq!(last.u64("total_in_run"), FLOOD as u64);
        assert_eq!(last.str("refusal"), "not_granted");
        assert_eq!(last.str("verb"), "actuate_pointer");
        assert_eq!(last.str("decision"), "refused");
        cleanup(&path);
    }

    #[test]
    fn a_refusal_run_ends_when_its_verb_or_code_changes() {
        let _fd = crate::capture::tests::fd_lock();
        // Aggregation must never merge distinct conditions: a different
        // verb or a different refusal code is a different fact and earns
        // its own full line, with the previous run's count flushed first.
        let (mut rec, path) = scratch_recorder("refusal-run-boundary");
        for _ in 0..3 {
            record_not_granted(&mut rec, Verb::ACTUATE_POINTER);
        }
        record_not_granted(&mut rec, Verb::ACTUATE_TEXT);
        rec.finish();

        let entries = read_log(&path);
        let kinds: Vec<&str> = entries.iter().map(|e| e.str("kind")).collect();
        assert_eq!(
            kinds,
            vec![
                "use_decision",        // first pointer refusal, in full
                "use_refusal_summary", // its 2 repeats, flushed by the change
                "use_decision",        // the text refusal, a new condition
                "run_ended",
            ]
        );
        assert_eq!(entries[0].str("verb"), "actuate_pointer");
        assert_eq!(entries[1].u64("repeats"), 2);
        assert_eq!(entries[1].str("verb"), "actuate_pointer");
        assert_eq!(entries[2].str("verb"), "actuate_text");
        cleanup(&path);
    }

    #[test]
    fn admitted_captures_are_never_suppressed_by_refusal_bounding() {
        let _fd = crate::capture::tests::fd_lock();
        // B1 is absolute and outranks the flood bound: every ALLOWED
        // capture keeps its own entry with its own frame digest -- never
        // sampled, never aggregated, no exceptions -- even when it is
        // buried in a refusal flood on the same grant.
        let (mut rec, path) = scratch_recorder("b1-vs-bounding");
        let mut digests = Vec::new();
        for i in 0..40u8 {
            for _ in 0..25 {
                record_not_granted(&mut rec, Verb::ACTUATE_POINTER);
            }
            let digest = ObservationDigest::of(&[i; 64]);
            digests.push(digest.to_hex());
            let admitted = UseOutcome::Admitted {
                grant: GrantId::from_u64_for_test(1),
                frame: Some(ObservedFrame {
                    width: 8,
                    height: 2,
                    stride: 32,
                    bytes: 64,
                    digest,
                }),
                spent_once: false,
            };
            rec.record(Event::UseDecision {
                connection: ConnectionId::from_u64_for_test(1),
                facet_wire_id: 20,
                // The SAME grant the flood is on: the admission must end
                // the run rather than be swallowed by it.
                grant_wire_id: 10,
                verb: Verb::OBSERVE,
                grant_row: Some(GrantId::from_u64_for_test(1)),
                detail: None,
                outcome: &admitted,
            });
        }
        rec.finish();

        let entries = read_log(&path);
        let allowed: Vec<&Json> = of_kind(&entries, "use_decision")
            .into_iter()
            .filter(|e| e.str("decision") == "allowed")
            .collect();
        assert_eq!(allowed.len(), 40, "every admitted capture kept its entry");
        let logged: Vec<&str> = allowed.iter().map(|e| e.str("frame.digest")).collect();
        assert_eq!(logged, digests, "each with its OWN digest, in order");
        // The refusals around them were bounded, so this is not merely
        // "nothing was aggregated at all".
        assert!(
            !of_kind(&entries, "use_refusal_summary").is_empty(),
            "the surrounding refusals really were aggregated"
        );
        cleanup(&path);
    }
}
