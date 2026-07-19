//! The enforcement chokepoint (P1.4.4, issue #28): the **single function**
//! through which every capture and every actuation passes --
//! [`Chokepoint::enforce_use`] -- checking `connection -> principal ->
//! grant -> verbs -> constraints` in exactly that order (PRD Doc 2 §5) and
//! voicing every refusal as `vitrin_grant.refused` from one emission site.
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
//!   crate -- [`Chokepoint::voice_refusal`], one refusal voice for capture
//!   and actuation alike (IDL `vitrin_grant`);
//! - the capture mechanics entry [`crate::capture::render_frame`] and the
//!   emulated-input constructor `SeatInput::emulated` each have exactly
//!   one caller outside their home modules -- here, *after* admission;
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
//!    a. `no_surface` (capture and actuation alike): the realm has no
//!       live view. A capture must serve "never a stale frame", and an
//!       actuation into a dead realm must be refused audibly, never
//!       swallowed -- the IDL's refusal entry is verb-neutral, prose
//!       pages 07/08 list `no_surface` in both actuators' applicable
//!       sets, and the sync-barrier discovery idiom (IDL `sync`) relies
//!       on every enforcement failure being voiced as an event. Judged
//!       first among the use-context gates and before the bucket, so a
//!       vacant realm never drains quota and a transient hold never
//!       masks the realm's death.
//!    b. `consent_held` (actuation only): the principal's own prompt is
//!       up ([`PetitionRegistry::prompt_up_for`] -- the mapping is
//!       documented in [`crate::petitions`]).
//!    c. `preempted` (actuation only): physical human input owns the
//!       target ([`PhysicalPresence::owns_target`], fed at the input
//!       router's hook point).
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
//!    embedder's delivery sink.
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
//! # One natural emission site (the P1.4.5 seam)
//!
//! Every decision -- allow or refuse, voiced or coalesced -- funnels
//! through the tail of [`Chokepoint::enforce_use`] and is summarized in
//! its returned [`UseOutcome`]; every refusal frame is built in
//! [`Chokepoint::voice_refusal`]. The flight recorder (issue #29) hooks
//! those two points and sees every enforcement decision without a third
//! code path appearing.
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

use crate::capture::{self, RealmViewFrame};
use crate::grants::{GrantId, GrantTable};
use crate::identity::PrincipalIdentity;
use crate::input::{PhysicalPresence, SeatInput, SeatInputKind};
use crate::petitions::PetitionRegistry;

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
/// `actuate_text` -- one facet, one verb bit (IDL `request_grant`).
pub(crate) enum UseKind {
    /// `vitrin_view.capture_frame` (reply-bearing).
    Capture,
    /// `vitrin_actuator_pointer.move`/`button`/`scroll` (fire-and-forget).
    Pointer(SeatInputKind),
    /// `vitrin_actuator_text.type` (fire-and-forget).
    Text(SeatInputKind),
}

impl UseKind {
    /// The verb this use exercises (also the `verb` argument of any
    /// refusal, which identifies the facet to the client).
    fn verb(&self) -> Verb {
        match self {
            UseKind::Capture => Verb::OBSERVE,
            UseKind::Pointer(_) => Verb::ACTUATE_POINTER,
            UseKind::Text(_) => Verb::ACTUATE_TEXT,
        }
    }

    /// Whether refusals of this use MAY be coalesced (fire-and-forget
    /// actuations) or are per-request terminals (reply-bearing captures)
    /// -- the delivery classification, conventions §6.
    fn coalescible(&self) -> bool {
        !matches!(self, UseKind::Capture)
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
    /// Physical-input presence, fed at the input router's hook point:
    /// the `preempted` judgement.
    pub presence: &'a PhysicalPresence,
    /// Where admitted actuations go, already origin-tagged `emulated`
    /// (M1.1 wiring: the realm's input router toward the shim seat;
    /// tests: a capture buffer). Delivery beyond this sink -- including
    /// dropping events for a seatless realm -- is the delivery edge's
    /// business, never an authority question.
    pub actuations: &'a mut dyn FnMut(SeatInput),
}

/// How one use was decided -- the chokepoint's summary of its own
/// decision, and the shape the flight recorder (P1.4.5) will consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseOutcome {
    /// Admitted: the operation ran (frame sent, or actuation handed to
    /// the sink) under this grant row.
    Admitted { grant: GrantId },
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
            // stale frame, and never input swallowed by a dead realm.
            if live_view(env.realm_view).is_none() {
                break 'decide Err(Refuse::code(Refusal::NoSurface));
            }
            if let UseKind::Pointer(_) | UseKind::Text(_) = &req.kind {
                // 5b, consent_held (actuation only): the principal's own
                // prompt is up.
                if petitions.prompt_up_for(req.principal) {
                    break 'decide Err(Refuse::code(Refusal::ConsentHeld));
                }
                // 5c, preempted (actuation only): physical human input
                // owns the target right now.
                if env.presence.owns_target(now) {
                    break 'decide Err(Refuse::code(Refusal::Preempted));
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
        grants.commit_use(allowed.grant_id);
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
                    Ok(frame) => {
                        // Exactly one terminal per capture, in request
                        // order: the frame, with its fresh sealed memfd
                        // riding SCM_RIGHTS. The server's copy of the fd
                        // drops with `frame` after the send.
                        send(&frame.encode(req.facet_id), Some(frame.fd.as_fd()))?;
                        Ok(UseOutcome::Admitted {
                            grant: allowed.grant_id,
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
                // Realm-view coordinates outside the view are clamped,
                // not refused (IDL vitrin_actuator_pointer; conventions
                // §6.3 lists it legal-but-noteworthy).
                let kind = clamp_to_view(kind, live_view(env.realm_view));
                // B2: the origin tag is bound here, at the single
                // admitted-actuation intake -- the delivery sink (and
                // through it the router, shim seat, and app) sees exactly
                // who caused this event.
                (env.actuations)(SeatInput::emulated(kind));
                Ok(UseOutcome::Admitted {
                    grant: allowed.grant_id,
                })
            }
        }
    }

    /// The single site where every `vitrin_grant.refused` is built and
    /// sent -- capture and actuation alike, one refusal voice (IDL). For
    /// coalescible (actuation) refusals it applies the delivery
    /// classification's two MAY-bounds and returns whether the event was
    /// actually voiced; capture refusals are always voiced. Defensively
    /// re-enforces the IDL invariant that `retry_after_ms` is nonzero
    /// only for `rate_limited`, in release builds too.
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
        /// A longer phrase whose occurrences are excused everywhere by
        /// subtraction: it contains the needle without naming the
        /// guarded item (the chokepoint's own outcome variant shares
        /// the wire event's `Refused` name).
        excused: Option<&'r str>,
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
                excused: None,
                home: "grants.rs",
                in_enforcement: 1,
            },
            Rule {
                what: "refusal event name",
                needle: &refusal_name,
                excused: Some(&outcome_variant),
                home: "enforcement.rs",
                in_enforcement: 2,
            },
            Rule {
                what: "capture mechanics entry",
                needle: &capture_entry,
                excused: None,
                home: "capture.rs",
                in_enforcement: 1,
            },
            Rule {
                what: "emulated-input mint",
                needle: &emulated_ctor,
                excused: None,
                home: "input/mod.rs",
                in_enforcement: 1,
            },
        ];
        for rule in &rules {
            let net = |text: &str| {
                count(text, rule.needle) - rule.excused.map_or(0, |excused| count(text, excused))
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
