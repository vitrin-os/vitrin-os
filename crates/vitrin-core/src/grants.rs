// SPDX-License-Identifier: MPL-2.0
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
//! *type-level* null where possible: [`FocusCondition`],
//! [`ProvenanceRef`] and [`PinnedAddrs`] are empty enums, so a row cannot
//! even represent a value there -- present-but-null enforced by
//! construction, not convention. [`PinnedAddrs`] is the one column added
//! after the PRD's list (P2.7.2, issue #196): the addresses a `net:`
//! host resolved to at grant time, kept in the row rather than in the
//! egress proxy so a proxy restart cannot re-resolve away from what the
//! human approved. Rows are keyed by the verifier-canonical
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
//! and it matches the chokepoint contract, whose every query carries the
//! use's `now`: P1.4.4 samples the clock once per request at its single
//! site, so one consistent `now` governs the whole decision. Monotonic `Instant` rather than wall time because expiry is a
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
//! **The chokepoint query is grant-scoped.** Every version-1 capture and
//! actuation arrives through a facet co-minted with exactly one grant, and
//! the IDL binds refusal semantics to that grant: "a grant that later
//! expires or is revoked goes dead and its facets go inert" -- inert even
//! while a sibling grant of the same principal covers the same verb, and
//! P1.4.4's chain is spelled `connection -> principal -> grant -> verbs ->
//! constraints`. [`GrantTable::check_use_grant`] is therefore the query
//! behind the enforcement chokepoint. The principal-keyed
//! [`GrantTable::check_use`] answers the broader "would *any* of this
//! principal's rows allow this use" and MUST NOT back facet-borne uses:
//! serving a facet's use from whichever row fits would resurrect a dead
//! grant's inert facets and misattribute the use. No version-1 wire path
//! needs the principal-keyed form (every use is facet-borne) -- P1.4.4
//! landed on [`GrantTable::check_use_grant`] alone, and the chokepoint's
//! single-path test pins `.check_use(` to this module; it remains as the
//! documented seam for later non-facet admission (Phase-2 selectors), to
//! be retired if that phase does not claim it.
//!
//! **Two-phase admission for the chokepoint (P1.4.4, this task's edit).**
//! [`GrantTable::check_use_grant`] is a **pure judgement** (`&self`,
//! consumes nothing); the chokepoint completes an admission with
//! [`GrantTable::commit_use`], which is what spends a `once` rung's single
//! use. Split because the chokepoint judges *more* than the row (rate
//! bucket, consent-held, preemption, surface presence), and the IDL's
//! transient refusals promise recovery: `consent_held` holds "until the
//! prompt closes", `preempted` "right now", `rate_limited` hints a refill
//! -- a `once` grant burned by a refused-and-never-delivered attempt would
//! break every one of those promises. So refusal consumes nothing, and
//! authority is consumed exactly by the use the chokepoint finally admits
//! (`commit_use` runs before the operation itself: a post-admission
//! server-side failure -- `internal` -- still leaves the `once` spent,
//! fail-closed, exactly as documented below). The principal-keyed
//! [`GrantTable::check_use`] keeps its one-call admission semantics: it
//! answers a different question (admission, not judgement) and backs no
//! wire path.
//!
//! **Proactive expiry: an embedder-polled sweep, advisory only.** The
//! issue-#28 decision: expiry is checked on use AND flipped proactively so
//! a dead grant does not *report* itself alive between uses.
//! [`GrantTable::expire_due`] follows the exact pattern of
//! [`petitions::expire_due`](crate::petitions::PetitionRegistry::expire_due)
//! -- injected `now`, embedder-polled (the runtime's armed calloop timer,
//! `session::sweep`; tests reach the registry's own entry point) -- and flips still-`Active` rows whose deadline passed to
//! a *stored* expired state, returning the newly dead ids (the flight
//! recorder's "grant expired without a use" feed, P1.4.5). It is
//! deliberately **never load-bearing for enforcement**: every read surface
//! ([`GrantTable::get`], [`GrantTable::rows`]) and every use-time check
//! already folds `now >= deadline` in, so a late -- or never-run -- poll
//! extends nothing. Version 1 has no expiry push event (the same growth
//! seam as revocation), so the sweep drives no wire traffic.
//!
//! **Refusal precedence (documented determinism, not policy).** Per row,
//! verb membership is judged first: a row whose effective set never
//! conferred the queried verb answers `not_granted` no matter how it later
//! died -- the IDL is unconditional that "a facet whose verb was not
//! granted refuses `not_granted`", and a death code would smear the row's
//! death onto authority it never touched (the SDK would raise `Revoked` --
//! human-stop semantics -- where the honest recovery is petitioning for
//! the verb). Only a row that *does* confer the verb answers with its
//! death code: `revoked` > `expired` (time or spent), matching the IDL's
//! revocation and expiry flows, which exercise granted verbs. Across rows
//! in [`GrantTable::check_use`], when no candidate allows, the aggregate
//! refusal is the most severe seen (`revoked` > `expired` >
//! `not_granted`), and no covering row at all is `not_granted`. Among
//! *allowing* candidates it prefers a `while_running` row over consuming a
//! `once` row (never burn single-use authority a durable row already
//! covers), tie-broken by *newest* (highest) `grant_id` -- the most recent
//! consent decision governs, so a re-granted row's constraints (say, a
//! raised `max_event_rate`) take effect immediately rather than only after
//! the older row dies; deterministic by construction.
//!
//! **An empty verb set is refused, never vacuously allowed.** The wire
//! makes an empty petition fatal and every facet carries exactly one verb
//! bit, but `Verb::contains` is subset semantics, so `Verb(0)` would be
//! "contained" by every row -- admitted, attributed to a grant, and even
//! spending a `once`. Both queries refuse `not_granted` before any row is
//! consulted or consumed, mirroring [`InsertError::EmptyVerbs`]: a
//! chokepoint bug must surface typed and fail-closed, never
//! authority-expanding.
//!
//! **What the table does *not* do.** No rate limiting: `max_event_rate` is
//! stored and handed to the chokepoint in [`Allowed`], but the token bucket
//! is P1.4.4's, in the enforcement chokepoint
//! ([`crate::enforcement`]; PRD Doc 2 sections 5.2/8) -- a
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
use std::num::{NonZeroU16, NonZeroU32};
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

    /// Test-only: a row id without a row, so unit tests of pure consumers
    /// (the flight recorder's entry shapes) need not stand up a table.
    /// Never available outside `cfg(test)`: outside tests, ids are minted
    /// by [`GrantTable::insert`] and nowhere else.
    #[cfg(test)]
    pub fn from_u64_for_test(raw: u64) -> Self {
        Self(raw)
    }
}

impl fmt::Display for GrantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "grant-{}", self.0)
    }
}

/// `realm_id` (PRD Doc 2 section 5.2): the realm a grant attaches to,
/// by name (`"realm-0"` is the single realm of version 1 and a mandatory
/// member at version 2, where a deployment may serve more;
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
/// covers. Version 1 served exactly one granularity -- the whole realm
/// (`request_grant`'s null-or-empty `resource` selector); the finer
/// type-prefixed selectors the wire vocabulary reserves (`surface:...`,
/// `node:...`) arrive as further variants, refining
/// [`ResourceRef::covers`] without changing the row shape.
///
/// [`ResourceRef::Net`] is the first of those to land (P2.7.2, issue
/// #196). **Nothing constructs it from the wire yet**, and that is stated
/// rather than left to be discovered: `PetitionRegistry::admit` refuses
/// every non-empty `resource` selector `unsupported`, so a `net:` petition
/// is answered exactly as any other finer granularity is. What lands here
/// is the *vocabulary* -- the parser, its round-trip, and the containment
/// rule -- so P2.7.3/P2.7.4 wire an already-tested grammar into the
/// admission path instead of writing one under time pressure at the
/// enforcement chokepoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceRef {
    /// Everything in the realm (wire selector: null or empty string).
    WholeRealm,
    /// One outbound TCP endpoint (wire selector: `net:HOST:PORT`), the
    /// resource an `egress` grant names. Exactly one host and exactly one
    /// port: the grammar admits no wildcard, so this variant cannot hold
    /// a set.
    Net(NetSelector),
}

impl ResourceRef {
    /// Whether authority over `self` covers a use targeting `target`.
    ///
    /// The whole-realm arm is trivially total. The `net:` arm is **exact
    /// match only**, and that is forced rather than chosen: the selector
    /// grammar is wildcard-free by construction
    /// (`protocol/vitrin-v0.xml`, `vitrin_grant.verb`), so there is no
    /// subsumption to express and a `covers` that invented one -- port
    /// ranges, parent domains, anything -- would be authority the human
    /// never approved.
    ///
    /// `WholeRealm` does **not** cover a `Net`, deliberately. A grant over
    /// "the whole realm" is authority over what the realm shows and what
    /// is typed into it; reading it as also authorising outbound packets
    /// would make every observe grant an egress grant, which is the exact
    /// ambient-authority default this system exists to remove.
    pub fn covers(&self, target: &ResourceRef) -> bool {
        match (self, target) {
            (ResourceRef::WholeRealm, ResourceRef::WholeRealm) => true,
            (ResourceRef::Net(held), ResourceRef::Net(want)) => held == want,
            (ResourceRef::WholeRealm, ResourceRef::Net(_))
            | (ResourceRef::Net(_), ResourceRef::WholeRealm) => false,
        }
    }
}

/// One `net:HOST:PORT` selector, parsed. Exactly one host and exactly one
/// port, because the wire grammar admits nothing else.
///
/// The host is kept as the **bytes the petition presented**, not as a
/// resolved address: what the human approves is the selector, and the
/// addresses it resolved to at approval time belong in a separate row
/// column ([`GrantRow::pinned_addrs`], **present-but-null until P2.7.4**)
/// so that a rebind cannot silently redirect authority the human already
/// granted. Nothing resolves anything today.
///
/// **Comparison is byte-exact, so one endpoint can have more than one
/// selector string** -- the consequence is stated here rather than left
/// to be met later, because the IDL's port rule is easy to misread as a
/// canonicality guarantee for the whole selector, and it is not one:
///
/// * DNS is case-insensitive, so `net:Example.com:443` and
///   `net:example.com:443` name the same endpoint and are two selectors.
/// * One IPv6 address has many legal literals, and the literal is kept
///   verbatim, so `net:[2001:db8::1]:443` and
///   `net:[2001:0db8:0000:0000:0000:0000:0000:0001]:443` are two
///   selectors.
///
/// Both spellings of each pair parse, both round-trip byte-identically,
/// and neither covers the other -- pinned by
/// `one_endpoint_can_have_several_selector_strings_and_none_covers_another`.
/// That errs **narrow** -- the wrong answer is a refusal, never an
/// unapproved connection -- which is the only direction this type is
/// allowed to be wrong in. Normalising instead would make the row hold a
/// string the human was never shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetSelector {
    host: NetHost,
    port: NonZeroU16,
}

/// The host half of a [`NetSelector`]: a bracketed IPv6 literal or
/// anything else the grammar admits (a DNS name or an IPv4 literal).
///
/// The distinction is kept because it is the one thing re-serialization
/// cannot recover: `2001:db8::1` and `[2001:db8::1]` parse to the same
/// address but only one of them is a legal selector, so a round-trip that
/// dropped the brackets would emit a string this parser then rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NetHost {
    /// A DNS name or an IPv4 literal, verbatim.
    Plain(String),
    /// An IPv6 literal, held **without** its brackets and re-emitted with
    /// them.
    V6(String),
}

/// Why a `net:` selector did not parse. Every variant is a **recoverable**
/// answer (`resolved(unsupported)`), never a fatal decode error: the wire
/// bound on `resource` is a byte length, and its *content* is a policy
/// question the IDL answers with `unsupported` rather than by killing the
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetSelectorError {
    /// The string does not begin with `net:`.
    NotANetSelector,
    /// No `:` separating host from port, or an empty host.
    MalformedHostPort,
    /// The port is not a bare decimal integer in `1..=65535`. A range, a
    /// list, a leading `+`/`-`, whitespace, a leading zero, and `0` all
    /// land here. The leading zero is refused rather than tolerated for a
    /// narrower reason than "one endpoint, one spelling" -- which is
    /// **false** here, see [`NetSelector`]: the port is the one half of
    /// the selector this type *normalises* (it becomes a `NonZeroU16`),
    /// so `0443` would re-serialize to `443` and break the byte-identity
    /// round-trip that makes the stored selector and the string the human
    /// was shown the same string.
    BadPort,
    /// The host contains something the wildcard-free grammar forbids: `*`,
    /// `/` (a CIDR suffix), `,` (a list), an empty label in any position
    /// (a leading `.` -- the any-subdomain spelling -- a doubled `..`, or a
    /// trailing `.`), whitespace, or a control character.
    ForbiddenHostSyntax,
    /// A bracketed host that is not a well-formed IPv6 literal, or an
    /// unbracketed host containing a `:` (which would make the final colon
    /// ambiguous).
    MalformedIpv6,
}

impl NetSelector {
    /// The wire prefix this selector is spelled with.
    pub const PREFIX: &'static str = "net:";

    /// Parse one `net:HOST:PORT` selector.
    ///
    /// **Wildcard-free by construction, and the refusals are the point.**
    /// `*.example.com`, `10.0.0.0/8`, `a.com:443,b.com:443`, `443-8443`,
    /// an empty host, port `0` and port `65536` are each rejected here, so
    /// a blanket egress grant is *inexpressible* rather than refused by a
    /// policy someone can relax later. `covers` is exact match precisely
    /// because this function admits no pattern to be inexact about.
    ///
    /// **What this does NOT do, stated so nobody infers it**: it does not
    /// validate that the host is a well-formed DNS name or IP literal. It
    /// enforces a *denylist* -- `*`, `/`, `,`, `[`, `]`, `:`, whitespace,
    /// control characters, and an empty label in any position (a leading
    /// `.`, a doubled `..`, a trailing `.`) -- and keeps
    /// whatever else it was handed, so `net:-:443`,
    /// `net:user@evil.com:443`, `net:999.999.999.999:443` and a Unicode
    /// homograph of a real name all parse. None of them widens authority
    /// ([`ResourceRef::covers`] is exact match, so no accepted selector can
    /// name more than one endpoint -- that is the whole of what
    /// "wildcard-free" buys), but a homograph is a confusion attack on the
    /// *human*, and P2.7.3 -- the first task that renders one of these
    /// strings on a consent card -- owns deciding the host charset before
    /// it does.
    pub fn parse(selector: &str) -> Result<Self, NetSelectorError> {
        let rest = selector
            .strip_prefix(Self::PREFIX)
            .ok_or(NetSelectorError::NotANetSelector)?;

        // Split at the LAST colon: an IPv6 literal carries its own, and the
        // brackets are what make that unambiguous.
        let (host_part, port_part) = rest
            .rsplit_once(':')
            .ok_or(NetSelectorError::MalformedHostPort)?;
        if host_part.is_empty() {
            return Err(NetSelectorError::MalformedHostPort);
        }

        // The port: a bare decimal integer, nothing else. `parse::<u16>`
        // already refuses `+443`, `443-8443`, `443,80`, whitespace and
        // 65536; `NonZeroU16` refuses 0, which is not an endpoint a
        // connection can be made to. The canonical-spelling check after it
        // refuses `0443`, which `parse` would otherwise accept as 443 and
        // which would then re-serialize to a different string than the one
        // the human approved.
        let port: NonZeroU16 = port_part.parse().map_err(|_| NetSelectorError::BadPort)?;
        if port_part != port.to_string() {
            return Err(NetSelectorError::BadPort);
        }

        let host = if let Some(inner) = host_part.strip_prefix('[') {
            let inner = inner
                .strip_suffix(']')
                .ok_or(NetSelectorError::MalformedIpv6)?;
            // Must be a real IPv6 literal: brackets are not a licence to
            // smuggle arbitrary text past the colon rule.
            if inner.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(NetSelectorError::MalformedIpv6);
            }
            NetHost::V6(inner.to_string())
        } else {
            // An unbracketed host may not contain a colon: it would make
            // the host/port split a guess.
            if host_part.contains(':') || host_part.contains(']') {
                return Err(NetSelectorError::MalformedIpv6);
            }
            if host_part
                .chars()
                .any(|c| matches!(c, '*' | '/' | ',' | '[') || c.is_whitespace() || c.is_control())
            {
                return Err(NetSelectorError::ForbiddenHostSyntax);
            }
            // An EMPTY LABEL IN ANY POSITION, which is three spellings of
            // one rule. A leading dot is the any-subdomain spelling in every
            // syntax that has one; a doubled dot is how a parser is talked
            // into treating one; a trailing dot is the DNS root label, which
            // names the *same* endpoint as the dotless spelling and would
            // therefore be a second selector string for it, bought for
            // nothing. `covers` is byte-exact, so admitting it would not
            // widen authority -- it would only mean a human who approved
            // `net:example.com:443` sees a connection to
            // `net:example.com.:443` refused, which is the surprising
            // direction of a rule with no upside. Erring narrow is free
            // here: nothing renders or accepts one of these strings yet.
            //
            // The trailing case was MISSING while all three carriers of this
            // rule said "an empty label" -- the parse doc, the error
            // variant's doc and docs/protocol/04-vitrin_grant.md's gap list.
            // Issue #196's round-2 review found `net:example.com.:443`
            // parsing and round-tripping. Closed by making the code match
            // the three sentences rather than by narrowing the three
            // sentences, because the rule they state is the one intended.
            if host_part.starts_with('.') || host_part.ends_with('.') || host_part.contains("..") {
                return Err(NetSelectorError::ForbiddenHostSyntax);
            }
            NetHost::Plain(host_part.to_string())
        };

        Ok(Self { host, port })
    }

    /// Re-serialize to the wire selector. `parse(s).unwrap().to_wire() ==
    /// s` for every `s` this parser accepts -- the property the proptest
    /// below holds, and what makes the row's stored selector and the
    /// string the human was shown the same string.
    pub fn to_wire(&self) -> String {
        match &self.host {
            NetHost::Plain(h) => format!("{}{}:{}", Self::PREFIX, h, self.port),
            NetHost::V6(h) => format!("{}[{}]:{}", Self::PREFIX, h, self.port),
        }
    }

    /// The host exactly as the selector spelled it (no brackets).
    pub fn host(&self) -> &str {
        match &self.host {
            NetHost::Plain(h) | NetHost::V6(h) => h,
        }
    }

    /// The single port.
    pub fn port(&self) -> NonZeroU16 {
        self.port
    }
}

/// The verb bits this core actually **serves** -- the exact set it
/// enforces at the chokepoint. Deliberately not labelled by wire version:
/// the served set is a property of *this build*, and it did not widen
/// when the wire went from 1 to 2.
///
/// The wire bitfield ([`Verb::VALID_MASK`]) is deliberately wider, and six
/// bits have been put on it ahead of anyone serving them: D-017 and D-018
/// define `observe_cursor`, `layout_arrange` and `layout_focus` from day one
/// so the decided cursor and layout models are expressible before v0 freezes,
/// version 2 added `realm_launch`, P2.6.5 added `designate_file` and P2.7.2
/// added `egress` -- so a
/// petition for any of them is a *recoverable* `unsupported` rather than an
/// out-of-range bit that kills the connection.
///
/// **Three of those six are now served.** `layout_arrange` (16) and
/// `layout_focus` (32) joined at WS-E.1.4 (issue #210), and
/// `realm_launch` (512) at WS-E.1.1 (issue #207): each has a facet
/// interface, a chokepoint arm and consent-prompt copy naming the
/// consequence in plain language, which is the whole of what "this core
/// serves the verb" means.
///
/// <!-- vitrin-verb-set: unserved-verbs = observe_cursor, designate_file, egress -->
/// **Three stay out**, for three different missing mechanisms.
/// `observe_cursor` (8) because per-principal cursor *delivery* is M2's, so
/// serving the verb would promise a capture widened with a cursor this core
/// does not have; `designate_file` (64) because no picker mints a descriptor
/// (P2.6.6) and no consent copy names what approving it costs (P2.6.8);
/// `egress` (128) because the out-of-core mediating proxy a
/// connection would be made through does not exist. Both of the newer two
/// have facets -- `vitrin_powerbox` and `vitrin_egress` -- and this doc named
/// egress's missing facet as half its reason until it landed: a facet is a
/// request to ask
/// through, and only a mechanism to answer with moves a bit out of this set.
/// None of the three names is transcribed: the set
/// is [`UNSERVED_VERB_BITS`], derived below, and `cargo xtask verb-sets
/// --check` holds every surface that spells it out to this constant.
///
/// **`designate_file` (64) joined the wire at P2.6.5 (issue #189) and is
/// deliberately absent from this constant**, which is the whole of that
/// issue's core-side deliverable, and `egress` (128) joined at P2.7.2 (issue
/// #196) on identical terms. Each verb has a facet interface
/// (`vitrin_powerbox`, `vitrin_egress`) and nothing else: for the first, no
/// picker mints a descriptor
/// (P2.6.6), no chokepoint arm carries a designation, and no consent copy
/// names what approving it costs (P2.6.8, Q13's rule); for the second, no
/// proxy asks the chokepoint per connection (P2.7.3). Leaving both out means
/// [`UNSERVED_VERB_BITS`] picks them up by derivation and
/// [`crate::petitions::PetitionRegistry::admit`] resolves every petition
/// naming either `unsupported` **whole** -- so the failure mode if someone
/// forgets the rest of E2.6 or E2.7 is a refusal, never a grant this core
/// cannot enforce.
///
/// **Moving `realm_launch` in is the single largest widening this
/// constant has taken**, and it is worth naming here rather than only at
/// the chokepoint: a bit in this set is a bit a grant row may carry, and
/// this one makes a wire request able to fork a process in the trusted
/// core. What bounds it is not this constant but everything the
/// chokepoint route buys -- a human's approval, an expiry, revocation,
/// the token bucket, [`crate::realm::MAX_REALMS`], and a journal entry
/// naming who asked.
///
/// Same posture as the durable persistence rungs, one rung up: those are
/// **absent** from [`PersistenceRung`] so a row cannot hold one; these
/// are present in [`Verb`] (the wire type is generated, so the table
/// cannot subtract bits from it) and are instead refused at admission
/// ([`crate::petitions::PetitionRegistry::admit`]). Either way the rule
/// is the same -- a deployment never grants authority it does not
/// enforce, and says so with `unsupported` rather than accepting
/// silently.
/// # A normative rule this constant does NOT implement (a landmine for P2.1.2)
///
/// `protocol/vitrin-v0.xml` states, normatively: *"A verb bit is NOT
/// version-gated: the bitfield is one mask ... so a version-1 connection may
/// name `realm_launch` and is answered `unsupported` rather than killed."* The
/// SDK bakes that in (`VERBS_SERVED_IN_VERSION_1` excludes `realm_launch`,
/// explaining it as a fact "about the VERSION"), and `00-conventions.md`
/// restates it. Under CLAUDE.md the IDL's `<description>` text wins, so this is
/// a normative rule with two readers and **no writer**.
///
/// This constant is version-independent and `PetitionRegistry::admit` takes no
/// protocol version, so nothing here can answer a version-1 connection
/// differently from a version-2 one. That is invisible today only because the
/// core accepts version **2 only** — the disclosed divergence owned by P2.1.2 —
/// so no version-1 connection exists to be answered wrongly.
///
/// It becomes live the moment P2.1.2 serves version 1. Whoever does that owns
/// making `admit` version-aware, or amending the IDL sentence; discovering it
/// then, from a client that is killed where the spec promises `unsupported`,
/// is the expensive way. Flagged here rather than only in a plan document
/// because this constant is where the fix has to land.
pub(crate) const SERVED_VERB_BITS: u32 = 1 | 2 | 4 | 16 | 32 | 512;

/// The verb bits the IDL defines that this core does **not** serve. A
/// petition naming any of these resolves `unsupported` -- whole, never
/// narrowed to the served remainder (narrowing is the human's move at
/// consent time, never a silent server-side edit).
///
/// Derived from [`Verb::VALID_MASK`] rather than listed, so a verb
/// appended to the IDL is unserved by default: forgetting to classify a
/// new bit fails closed.
pub(crate) const UNSERVED_VERB_BITS: u32 = Verb::VALID_MASK & !SERVED_VERB_BITS;

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

impl PersistenceRung {
    /// Every representable rung, shortest-lived first.
    ///
    /// The one enumeration of the ladder. The consent renderer generates its
    /// allow-choices from this (`crate::consent::PromptContent::choices`), so
    /// a rung that becomes representable gains a button without anyone
    /// remembering to add one -- and a durable rung, which cannot be
    /// represented at all, cannot appear on a prompt. Kept beside the type so
    /// adding a variant without extending it is a visible omission rather
    /// than an invisible one.
    pub const ALL: [PersistenceRung; 2] = [PersistenceRung::Once, PersistenceRung::WhileRunning];

    /// Lifetime order: how long this rung's authority can outlive the
    /// decision that granted it.
    fn rank(self) -> u8 {
        match self {
            PersistenceRung::Once => 0,
            PersistenceRung::WhileRunning => 1,
        }
    }

    /// Whether granting `self` **narrows** (or exactly matches) a petition
    /// that asked for `requested` -- i.e. whether a consent decision may
    /// legally choose this rung.
    ///
    /// The single definition of that rule, because it has two consumers that
    /// must not disagree: the petition registry refuses a widening decision
    /// with it ([`crate::petitions::PetitionRegistry::resolve_scripted`]),
    /// and the consent prompt decides which allow-buttons to draw with it.
    /// Two copies would eventually mean a prompt offering a choice the
    /// registry rejects -- a button that does nothing, which on a consent
    /// surface is worse than a missing one.
    pub fn narrows(self, requested: PersistenceRung) -> bool {
        self.rank() <= requested.rank()
    }
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

impl From<PersistenceRung> for WirePersistence {
    /// The wire projection, for the petition flow's
    /// `vitrin_grant.resolved.persistence` (total: every table rung is a
    /// wire rung; only the reverse conversion can fail).
    fn from(rung: PersistenceRung) -> WirePersistence {
        match rung {
            PersistenceRung::Once => WirePersistence::Once,
            PersistenceRung::WhileRunning => WirePersistence::WhileRunning,
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

/// `pinned_addrs`: the IP addresses the grant's `net:` host resolved to
/// **at grant time** -- the addresses the human actually approved when
/// they approved a name.
///
/// **Present-but-null until P2.7.4**, which resolves DNS in the
/// out-of-core egress proxy and fills this column, after which the
/// enforcement chokepoint refuses any connection to an address the pin
/// does not contain -- including a literal-IP connection under a
/// name-scoped grant. An empty enum, on the [`ProvenanceRef`] /
/// [`FocusCondition`] precedent: a row today cannot fabricate a pin, and
/// the null is type-enforced rather than conventional.
///
/// **The column is here rather than in the proxy on purpose.** A pin held
/// in proxy memory is lost on a proxy restart, and a restarted proxy
/// re-resolves -- so a DNS rebind would win simply by outlasting a
/// process. In the row it survives the restart, is revoked with the row,
/// and is auditable in the same journal as every other authority fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinnedAddrs {}

/// `issuer` (PRD Doc 2 section 5.2): which authority created the row.
/// Version 1 has exactly the two consent paths of Phase 1 -- plus, in test
/// builds only, the scripted stand-in below; all name a *core-side*
/// decision -- agents never issue grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Issuer {
    /// The human approved the core-rendered consent prompt (P1.7.1).
    HumanConsent,
    /// The loudly-logged `--consent=auto-approve` policy for headless CI
    /// and demos (P1.7.2, plan risk R6).
    AutoApprovePolicy,
    /// The build-gated scripted-consent injector approved the petition
    /// (P1.4.3): the integration-test stand-in for the consent surface,
    /// recorded honestly rather than masquerading as `HumanConsent`.
    /// Compiled only under `cfg(test)` or the `scripted-consent` feature,
    /// so a deployment build cannot even represent this issuer.
    #[cfg(any(test, feature = "scripted-consent"))]
    ScriptedConsent,
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
    /// Present-but-null until P2.7.4 (see [`PinnedAddrs`]): the addresses
    /// this grant's `net:` host resolved to at grant time. `None` on every
    /// non-egress row, and on every row this core can build today.
    pub pinned_addrs: Option<PinnedAddrs>,
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
/// `parent_grant_id`, `focus_condition`, `one_shot`, or `pinned_addrs`:
/// this core cannot state them, so no caller can smuggle one in.
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
    /// The row that granted this use: for
    /// [`GrantTable::check_use_grant`] the named grant itself, for
    /// [`GrantTable::check_use`] the documented selection among covering
    /// rows.
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
    /// No covering row (none exists, it was removed at connection
    /// teardown, or it is not this principal's), the verb is outside the
    /// row's effective set, or the use named no verb at all (wire
    /// `not_granted`). Lowest severity.
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

/// Stored liveness (what queries cannot derive from time alone -- plus the
/// proactive sweep's stored flip, which queries *could* derive but the
/// sweep records so a row's death without a use is an explicit,
/// once-reported event).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Liveness {
    Active,
    /// The `once` rung's single use is consumed.
    Spent,
    /// The proactive sweep ([`GrantTable::expire_due`]) recorded the time
    /// bound passing. Advisory bookkeeping: the deadline check in every
    /// query is what enforces expiry, sweep or no sweep.
    Expired,
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
    /// This row's answer for one use, verb membership first: a row whose
    /// effective set never conferred the queried verb answers
    /// `not_granted` no matter how it later died (the IDL's unconditional
    /// ungranted-facet rule -- module docs, refusal precedence); only a
    /// row that does confer the verb answers with its death code,
    /// `revoked` then `expired` (time or spent), matching the IDL's
    /// revocation and expiry flows. `None` = this row allows the use.
    fn refusal_for(&self, verb: Verb, now: Instant) -> Option<RefusalReason> {
        if !self.row.verbs.contains(verb) {
            return Some(RefusalReason::NotGranted);
        }
        if self.liveness == Liveness::Revoked {
            return Some(RefusalReason::Revoked);
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return Some(RefusalReason::Expired);
        }
        if matches!(self.liveness, Liveness::Spent | Liveness::Expired) {
            return Some(RefusalReason::Expired);
        }
        None
    }

    /// The row's liveness at `now`: the stored state with time expiry
    /// folded in, same precedence as the refusal path (revoked > expired >
    /// spent > active). Shared by [`GrantTable::get`] and
    /// [`GrantTable::rows`], so no read surface can report a dead row
    /// without its death.
    fn state_at(&self, now: Instant) -> GrantState {
        match self.liveness {
            Liveness::Revoked => GrantState::Revoked,
            _ if self.deadline.is_some_and(|deadline| now >= deadline) => GrantState::Expired,
            Liveness::Expired => GrantState::Expired,
            Liveness::Spent => GrantState::Spent,
            Liveness::Active => GrantState::Active,
        }
    }
}

/// The in-memory grant store (PRD Doc 2 section 5.2): the single source of
/// authority the enforcement chokepoint queries. In-memory only -- grants
/// die with the process; restore tokens are a later phase.
///
/// The table is not itself the chokepoint: it is the *answer* behind it.
/// P1.4.4's one server-side check function calls
/// [`GrantTable::check_use_grant`] for every facet-borne use and nothing
/// else consults rows for authority.
#[derive(Debug)]
pub(crate) struct GrantTable {
    /// Keyed by [`GrantId`]; `BTreeMap` for deterministic ascending-id
    /// iteration (the documented newest-id tie-break falls out of it:
    /// among equal rungs, last seen = highest id wins).
    entries: BTreeMap<GrantId, Entry>,
    /// Next id to assign; ids start at 1 and are never reused, so an id
    /// outlives its row unambiguously in logs.
    next_id: u64,
    /// Set once a human has completed the dead-man chord
    /// ([`Self::seal_dead_man`]). Rows minted afterwards are born
    /// [`Liveness::Revoked`] — see that method for why the table, and not the
    /// caller, is where this belongs.
    dead_man_sealed: bool,
}

/// Same as [`GrantTable::new`]. Hand-written because a derived `Default`
/// would zero-initialize `next_id` and mint `grant-0`, silently diverging
/// from the documented ids-start-at-1 invariant the moment a server-state
/// struct derives `Default` around the table.
impl Default for GrantTable {
    fn default() -> Self {
        Self::new()
    }
}

impl GrantTable {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
            dead_man_sealed: false,
        }
    }

    /// Seal the table: a human completed the dead-man chord, and no row
    /// minted from here on is live.
    ///
    /// **Why the switch needs more than a sweep.** [`crate::deadman::apply`]
    /// revokes every row *present* at the moment the chord completes. Two
    /// kinds of authority slip past a point-in-time sweep:
    ///
    /// - **Decided but not yet delivered.** A granted consent decision becomes
    ///   a row only at `PrincipalServer::deliver_resolution` (module docs of
    ///   [`crate::petitions`] give the full reason — authority exists iff its
    ///   wire handle resolved). A `Verdict::Granted` decided *before* the
    ///   chord and delivered *after* it is neither a pending petition nor an
    ///   existing row, so the sweep sees neither, and delivery then mints
    ///   live authority that outlived the human's off-switch.
    /// - **Granted with no human in the loop.** Under
    ///   `ConsentPolicy::AutoApprove` the agent's very next petition is
    ///   approved by policy, restoring the identical authority within one
    ///   round-trip of the panic button.
    ///
    /// **Why here, rather than a check at each caller.** This is the same
    /// razor the rest of the crate applies: authority questions get exactly
    /// one home. A fence the delivery path had to remember to consult is a
    /// fence a future delivery path forgets. The table is what mints rows, so
    /// the table is what refuses to mint live ones.
    ///
    /// **Why born-revoked rather than a refused insert.** `insert` failing is
    /// `DeliveryError::Insert`, which the delivery path treats as
    /// unreachable-and-fatal and answers with a fatal `internal` goodbye —
    /// the wrong thing to say to a client whose petition was decided legally
    /// and whose only sin is being late. Minting the row `Revoked` keeps every
    /// existing invariant intact (the handle resolves, the exactly-once guard
    /// holds, the teardown scan still finds the row to remove) while making
    /// the authority dead on arrival: the chokepoint's very next
    /// `check_use_grant` refuses [`RefusalReason::Revoked`], which is the same
    /// answer, through the same path, that the sweep produces for every other
    /// row. The wire cannot tell the two apart, and should not.
    ///
    /// **This does not disarm the human.** `resolve_human(Allow)` still works
    /// — its row is minted revoked like any other, so re-authorising after a
    /// panic button is a restart, not a click. That is the intended reading of
    /// the gesture: it ends the session's delegated authority, and it is not
    /// undoable from inside the session it just ended.
    ///
    /// Idempotent; a second chord seals nothing new.
    pub fn seal_dead_man(&mut self) {
        self.dead_man_sealed = true;
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
            // Present-but-null: filled by Phase 3 provenance (E3.7), the
            // version-2+ attenuation seam, and P2.7.4's DNS pinning
            // respectively. `GrantSpec` has no field for any of them, so
            // no caller can smuggle one in through this path.
            provenance_ref: None,
            parent_grant_id: None,
            pinned_addrs: None,
            issued_at,
            issuer: spec.issuer,
        };
        // Born revoked once the human has hit the off-switch
        // ([`Self::seal_dead_man`]): a decision made before the chord must not
        // become live authority after it, and no policy may re-grant without a
        // human. Loud, because a client that believes it holds a grant and is
        // refused `revoked` on first use deserves an explanation in the log.
        let liveness = if self.dead_man_sealed {
            tracing::warn!(
                %grant_id,
                principal = %row.principal_id,
                "grant minted into a dead-man-sealed table: born revoked, never usable"
            );
            Liveness::Revoked
        } else {
            Liveness::Active
        };
        self.entries.insert(
            grant_id,
            Entry {
                row,
                deadline,
                liveness,
            },
        );
        Ok(grant_id)
    }

    /// The principal-scoped admission query: may `principal` perform
    /// `verb` on `(realm, resource)` at `now` under *any* of its rows?
    ///
    /// **Not the facet-borne chokepoint path** -- that is
    /// [`GrantTable::check_use_grant`] (module docs): answering a facet's
    /// use from whichever row fits would resurrect a dead grant's inert
    /// facets and misattribute the use. No version-1 wire path needs this
    /// query; it remains the seam for later non-facet admission and stays
    /// an *admission* query: allowing consumes a `once` row's single use
    /// (so a `once` grant admitted here is spent even if the operation
    /// later fails server-side -- fail-closed). Selection among several
    /// allowing rows and the refusal precedence when none allows are
    /// documented in the module docs; no policy beyond what rows state is
    /// applied.
    pub fn check_use(
        &mut self,
        principal: &PrincipalIdentity,
        realm: &RealmId,
        resource: &ResourceRef,
        verb: Verb,
        now: Instant,
    ) -> Result<Allowed, RefusalReason> {
        // Empty-verb guard (module docs): `Verb(0)` is subset-contained by
        // every row, so without this it would be vacuously admitted --
        // and could spend a `once`. Fail closed before any row is
        // consulted or consumed.
        if verb.bits() == 0 {
            return Err(RefusalReason::NotGranted);
        }
        // Aggregate refusal severity across covering rows; RefusalReason's
        // derived Ord is the documented precedence (NotGranted < Expired <
        // Revoked).
        let mut refusal: Option<RefusalReason> = None;
        // The allowing candidate: prefer WhileRunning over consuming a
        // Once; among equal rungs the newest (highest) id wins, and
        // ascending-id iteration makes "last seen" the highest id.
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
                        // A Once candidate yields to anything later: a
                        // WhileRunning upgrade or a newer Once.
                        Some((_, PersistenceRung::Once)) => true,
                        // A WhileRunning candidate yields only to a newer
                        // WhileRunning, never back down to a Once.
                        Some((_, PersistenceRung::WhileRunning)) => {
                            row.persistence == PersistenceRung::WhileRunning
                        }
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

    /// **Which realm a row is over**, or `None` if the row is gone.
    ///
    /// The `realm_id` column has always been there; nothing read it at use
    /// time while a session held exactly one realm, because "the realm" and
    /// "this grant's realm" could not differ. With several realms they can,
    /// and the use path has to ask it twice -- once per direction:
    ///
    /// - **read**: a grant over a dead realm must not observe a *sibling's*
    ///   pixels (`session::dispatch_principal`,
    ///   `principal::PrincipalServer::serve_facet_use`);
    /// - **write**: an actuation admitted under this grant must not be
    ///   delivered into a *sibling's* app (the same two sites, feeding
    ///   `enforcement::UseEnv::grant_realm`, which travels with the event to
    ///   `session::route_seat` and *is* the delivery address). Until WS-E.1.6
    ///   it fed `seat_reaches_grant_realm`, a comparison against the session's
    ///   one seat target; there is nothing left to compare because the realm
    ///   now goes with the event.
    ///
    /// Deliberately not an authority judgement -- it reports the row's
    /// target so the caller can resolve the *environment* for it. Whether a
    /// use is permitted stays [`Self::check_use_grant`]'s, at the one
    /// chokepoint.
    pub fn realm_of(&self, grant: GrantId) -> Option<&RealmId> {
        self.entries.get(&grant).map(|entry| &entry.row.realm_id)
    }

    /// **Does any live row confer `verb` right now?** D-018(4)'s
    /// single-holder rule, asked at petition admission by
    /// [`crate::petitions::PetitionRegistry::admit`] for `layout_arrange`.
    ///
    /// Deliberately **not** an authority judgement and deliberately not
    /// scoped to a principal: the question is "is this authority already
    /// held anywhere in this session", because at most one holder may carry
    /// `layout_arrange` per output and there is exactly one output.
    ///
    /// **Half the rule, and only half.** A *pending* petition naming the verb
    /// is a holder-in-waiting and takes the slot too — that half is the
    /// petition registry's own state, so it is checked there
    /// ([`crate::petitions::PetitionRegistry::admit`]) and this answers the
    /// live-row half. Both are stated on the wire (IDL `vitrin_grant.outcome`,
    /// `layout_held`).
    /// Scoping it per principal would let one agent hold N arrangement
    /// grants and defeat the rule by fragmenting itself, and would leave
    /// the core arbitrating between the fragments -- which is the
    /// window-management policy PRD §5.1 exiles.
    ///
    /// `Active` only, at `now`, through the same [`Entry::state_at`] every
    /// other read surface uses: a revoked, expired or spent holder is not
    /// holding anything, so re-petitioning after one dies must succeed.
    pub fn any_live_holder_of(&self, verb: Verb, now: Instant) -> bool {
        self.entries.values().any(|entry| {
            entry.row.verbs.contains(verb) && entry.state_at(now) == GrantState::Active
        })
    }

    /// **Does this principal hold a live row carrying `verb`?** WS-E.1.7's
    /// attention-event **delivery filter** (issue #232).
    ///
    /// **A delivery filter and never an authority check**, and the distinction
    /// is the whole reason this has its own doc paragraph. It answers "should
    /// this connection be *told* the human pressed the attention key", so that
    /// the wire stays silent for every client that could not act on it. It
    /// never decides whether anything may happen: that stays
    /// [`Self::check_use_grant`]'s, at the one chokepoint, and this function is
    /// pinned **by name** as an exclusion in `enforcement.rs`'s
    /// `single_enforcement_path` scan -- zero occurrences there, exactly one
    /// outside this module -- so a future call from an enforcement path is a
    /// red test rather than a second authority site nobody noticed.
    ///
    /// Being wrong here costs a notification, in one direction each: a false
    /// negative is a layout holder that does not learn the human pressed the
    /// key (its `focus` is then refused `preempted`, recoverably, exactly as
    /// before this feature existed); a false positive is a client told about a
    /// keypress it cannot use, which is the timing oracle the filter exists to
    /// close and so is the direction that must not drift. It confers nothing
    /// either way -- the *claim* is separately gated on membership of the
    /// delivered-to set, which is resolved from this same answer at press time.
    ///
    /// `Active` only, at `now`, through the same [`Entry::state_at`] every
    /// other read surface uses: a revoked, expired or spent row holds nothing.
    /// Scoped to a principal, unlike [`Self::any_live_holder_of`], because the
    /// question here really is per identity -- authority is keyed by verified
    /// identity, not by connection. The consequence is worth stating: a
    /// principal holding two connections is told on **both**, including on one
    /// that minted no layout grant of its own. That is the honest reading (it
    /// is the same principal, and it already holds the authority the signal is
    /// filtered on) and it leaks nothing across a trust boundary, because there
    /// is no boundary between one identity's own connections.
    pub fn holds_verb(&self, principal: &PrincipalIdentity, verb: Verb, now: Instant) -> bool {
        self.entries.values().any(|entry| {
            entry.row.principal_id == *principal
                && entry.row.verbs.contains(verb)
                && entry.state_at(now) == GrantState::Active
        })
    }

    /// **The chokepoint query** (P1.4.4): may this use of `grant` --
    /// arriving through a facet co-minted with exactly that grant --
    /// perform `verb` at `now`, on behalf of `principal` (the verified
    /// identity bound to the connection the facet lives on)? One call per
    /// capture or actuation, from the single enforcement site
    /// ([`crate::enforcement`]); the IDL makes refusal semantics
    /// grant-scoped (a dead grant's facets go inert even while a sibling
    /// grant of the same principal covers the verb), so the judgement
    /// consults only this grant's row.
    ///
    /// A missing row (never existed, or removed at connection teardown)
    /// and a `principal` that does not own the row both refuse
    /// `not_granted`: the asker holds no such authority. The principal
    /// check is defense in depth -- sender-constrained handles already pin
    /// facets to the connection that minted them -- so a mismatch is a
    /// core bug surfacing typed and fail-closed, never a panic. The row's
    /// own realm and resource are the use's target (version-1 facets
    /// address exactly the granted resource; a finer in-resource target
    /// parameter arrives with Phase-2 selectors).
    ///
    /// **Pure judgement, consumes nothing** (module docs: two-phase
    /// admission): the chokepoint has further checks to run after this
    /// one, and a refused use must never burn single-use authority. The
    /// admission it finally reaches is committed with
    /// [`GrantTable::commit_use`].
    pub fn check_use_grant(
        &self,
        grant: GrantId,
        principal: &PrincipalIdentity,
        verb: Verb,
        now: Instant,
    ) -> Result<Allowed, RefusalReason> {
        // Same empty-verb guard as `check_use` (module docs): fail closed
        // before the row is consulted.
        if verb.bits() == 0 {
            return Err(RefusalReason::NotGranted);
        }
        let Some(entry) = self.entries.get(&grant) else {
            return Err(RefusalReason::NotGranted);
        };
        if entry.row.principal_id != *principal {
            return Err(RefusalReason::NotGranted);
        }
        if let Some(reason) = entry.refusal_for(verb, now) {
            return Err(reason);
        }
        Ok(Allowed {
            grant_id: grant,
            max_event_rate: entry.row.constraints.max_event_rate,
        })
    }

    /// Commit one admitted use of `grant` (P1.4.4, the second phase of
    /// two-phase admission): spends a `once` rung's single use; a no-op
    /// for `while_running`. Called by the enforcement chokepoint exactly
    /// once per finally-admitted use, after every check passed and
    /// *before* the operation runs -- so a post-admission server-side
    /// failure (`internal`) still leaves the `once` consumed, fail-closed
    /// and never authority-expanding (module docs). Defensive: only an
    /// `Active` row can be spent, so a misordered call can never turn a
    /// revoked or expired row into a merely-spent one.
    ///
    /// Returns whether this call **consumed** single-use authority (`true`
    /// exactly for a `once` row that was still active): the active-to-spent
    /// lifecycle transition, reported at the instant it happens so the
    /// flight recorder (P1.4.5) records it without a second query -- and,
    /// being a return value, without the table learning that a recorder
    /// exists.
    pub fn commit_use(&mut self, grant: GrantId) -> bool {
        match self.entries.get_mut(&grant) {
            Some(entry)
                if entry.row.persistence == PersistenceRung::Once
                    && entry.liveness == Liveness::Active =>
            {
                entry.liveness = Liveness::Spent;
                true
            }
            _ => false,
        }
    }

    /// The proactive expiry sweep (P1.4.4; module docs): flip every
    /// still-`Active` row whose time bound has passed at `now` to the
    /// stored expired state and return the newly dead ids, ascending.
    /// Embedder-polled on the same cadence as
    /// [`PetitionRegistry::expire_due`](crate::petitions::PetitionRegistry::expire_due)
    /// (the runtime's armed calloop timer, `session::sweep`); idempotent --
    /// a second poll reports
    /// nothing new -- and advisory: every query already folds the deadline
    /// in, so enforcement never depends on this having run. Revoked and
    /// spent rows are already dead and are not reported.
    pub fn expire_due(&mut self, now: Instant) -> Vec<GrantId> {
        let mut newly_expired = Vec::new();
        for (&id, entry) in &mut self.entries {
            if entry.liveness == Liveness::Active
                && entry.deadline.is_some_and(|deadline| now >= deadline)
            {
                entry.liveness = Liveness::Expired;
                newly_expired.push(id);
            }
        }
        newly_expired
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
    /// principal's grants". Returns the ids this call newly revoked,
    /// ascending (the count is `.len()`); naming them rather than counting
    /// them is what lets the flight recorder (P1.4.5) say *which* authority
    /// a dead-man switch killed, which a bare count cannot.
    pub fn revoke_principal(&mut self, principal: &PrincipalIdentity) -> Vec<GrantId> {
        let mut revoked = Vec::new();
        for (&id, entry) in &mut self.entries {
            if entry.row.principal_id == *principal && entry.liveness != Liveness::Revoked {
                entry.liveness = Liveness::Revoked;
                revoked.push(id);
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
        self.entries
            .get(&grant)
            .map(|entry| (&entry.row, entry.state_at(now)))
    }

    /// All rows paired with their liveness at `now`, ascending
    /// [`GrantId`] (the "connected apps"-style enumeration surface; the
    /// flight recorder's snapshot source). Folds in the same
    /// [`GrantState`] as [`GrantTable::get`] -- there is deliberately no
    /// way to enumerate rows *without* liveness, so a consumer can never
    /// render revoked, expired, or spent authority as live (PRD Doc 2
    /// section 5.4's panel lists live grants): row presence is
    /// bookkeeping, never authority. Never consumes anything.
    pub fn rows(&self, now: Instant) -> impl Iterator<Item = (&GrantRow, GrantState)> {
        self.entries
            .values()
            .map(move |entry| (&entry.row, entry.state_at(now)))
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

    // -- served vs. defined verbs ------------------------------------------

    #[test]
    fn served_verb_bits_are_exactly_the_six_facet_verbs() {
        // Pinned to the generated constants, not to a literal, so a verb
        // that ever changed value would fail here rather than silently
        // widening what this core claims to enforce.
        //
        // Six since WS-E.1.1 (issue #207): `realm_launch` joined the two
        // layout verbs WS-E.1.4 added and the three original facet verbs.
        // Each has an interface declaring it, a chokepoint arm exercising
        // it and a consent-prompt line naming it. The defined verbs that
        // stay out are whatever `UNSERVED_VERB_BITS` derives, and the
        // sibling test is where that set is enumerated and held. This
        // comment names no count of them on purpose: it said
        // "`observe_cursor` is the one defined verb that stays out" and was
        // false from the moment P2.6.5 (issue #189) added a second, and
        // false again by one more the moment P2.7.2 (issue #196) added a
        // third. **Both of those tasks left this constant untouched on
        // purpose**: adding a bit to the IDL must not widen what this core
        // claims to enforce.
        assert_eq!(
            SERVED_VERB_BITS,
            (Verb::OBSERVE
                | Verb::ACTUATE_POINTER
                | Verb::ACTUATE_TEXT
                | Verb::LAYOUT_ARRANGE
                | Verb::LAYOUT_FOCUS
                | Verb::REALM_LAUNCH)
                .bits()
        );
        // Every served bit is a defined wire bit.
        assert_eq!(SERVED_VERB_BITS & !Verb::VALID_MASK, 0);
    }

    #[test]
    fn the_staged_verbs_are_defined_on_the_wire_but_unserved() {
        // The bits still staged: in-range (so naming one is never fatal)
        // and unserved (so a petition for one resolves `unsupported`). Both
        // halves matter -- either alone would be a lie about what this core
        // does.
        //
        // **`realm_launch` left this list at WS-E.1.1 (issue #207), and
        // deliberately rather than mechanically.** It was here because this
        // core had no spawn path reachable from a grant; it now has one, at
        // the chokepoint's `Launch` arm, complete with the realm cap, the
        // core-minted instance id and the consent copy Q13 requires. That
        // is the specific missing mechanism this list is supposed to name,
        // and it is no longer missing.
        //
        // `observe_cursor` stays, and for a reason that has not moved:
        // per-principal cursor *delivery* is M2's (D-017, D-019), so
        // serving the verb would promise a capture widened with a cursor
        // this core does not have. It is not a placeholder for "not got to
        // yet".
        // `designate_file` JOINED it at P2.6.5 (issue #189), and that was the
        // first time this list had grown. It is here for a reason that is
        // scheduled rather than open-ended: there is no picker to mint a
        // descriptor (P2.6.6) and no consent copy naming what approving it
        // costs (P2.6.8). Both must land before this bit may move into
        // `SERVED_VERB_BITS`, and moving it before then would be exactly the
        // "a deployment MUST NOT grant a verb it does not enforce" breach the
        // list exists to make visible.
        //
        // **`egress` joined at P2.7.2 (issue #196)**, the second growth and
        // for the mirror
        // reason `realm_launch` left: the mechanism its refusal stands for
        // does not exist. The out-of-core mediating proxy that would ask
        // the chokepoint per connection is P2.7.3's. Its facet DOES exist --
        // `vitrin_egress`, an interface of its own rather than a request on
        // P2.6.5's filesystem powerbox, because `interface/@verb` is one
        // value per interface -- and having a facet is not being served: a
        // bit on the wire with no enforcement behind it is exactly what
        // `unsupported` is for.
        //
        // A list, deliberately: this is a SET that has shrunk three times
        // (D-018's two verbs, then `realm_launch` at WS-E.1.1) and grown
        // twice, and will move again when cursor delivery, the picker and
        // the egress proxy land. Collapsing it to a straight-line assertion
        // would hide that shape and make the next movement a rewrite rather
        // than an edit.
        for verb in [Verb::OBSERVE_CURSOR, Verb::DESIGNATE_FILE, Verb::EGRESS] {
            assert!(
                Verb::from_bits(verb.bits()).is_ok(),
                "{verb:?} must decode: an out-of-range bit would be fatal, not `unsupported`"
            );
            assert_eq!(
                verb.bits() & SERVED_VERB_BITS,
                0,
                "{verb:?} must not be claimed as served"
            );
            assert_eq!(verb.bits() & UNSERVED_VERB_BITS, verb.bits());
        }
        // ...and the verb that left is really served now, not merely absent
        // from the list above. Asserted here rather than only in the
        // sibling test so the two halves of "moved from unserved to served"
        // are one failure when someone reverts half of it.
        assert_eq!(
            Verb::REALM_LAUNCH.bits() & SERVED_VERB_BITS,
            Verb::REALM_LAUNCH.bits(),
            "realm_launch must be served: WS-E.1.1 gave it a chokepoint arm and prompt copy"
        );
        // The two classifications partition the wire bitfield: a verb
        // appended to the IDL lands in one of them, never in neither.
        assert_eq!(SERVED_VERB_BITS | UNSERVED_VERB_BITS, Verb::VALID_MASK);
        assert_eq!(SERVED_VERB_BITS & UNSERVED_VERB_BITS, 0);
    }

    // -- the net: selector (P2.7.2) ----------------------------------------

    /// Parse-and-round-trip, for a selector the grammar must accept.
    fn net(selector: &str) -> NetSelector {
        NetSelector::parse(selector)
            .unwrap_or_else(|e| panic!("`{selector}` must parse, got {e:?}"))
    }

    #[test]
    fn the_net_selector_accepts_exactly_one_host_and_one_port() {
        let s = net("net:api.example.com:443");
        assert_eq!(s.host(), "api.example.com");
        assert_eq!(s.port().get(), 443);
        assert_eq!(s.to_wire(), "net:api.example.com:443");

        // A literal IPv4 host is a host like any other.
        let s = net("net:192.0.2.7:8443");
        assert_eq!(s.host(), "192.0.2.7");
        assert_eq!(s.port().get(), 8443);

        // IPv6 is bracketed, and the brackets survive the round trip --
        // without them the final colon would not tell host from port.
        let s = net("net:[2001:db8::1]:443");
        assert_eq!(s.host(), "2001:db8::1");
        assert_eq!(s.port().get(), 443);
        assert_eq!(s.to_wire(), "net:[2001:db8::1]:443");
    }

    #[test]
    fn the_net_grammar_refuses_every_form_that_would_widen_a_grant() {
        // The forms that make a selector a *pattern* rather than an
        // endpoint. Each is refused by the parser, so a blanket egress
        // grant is inexpressible rather than refused by policy. (The
        // proptest below is the non-vacuous half: it *generates* these
        // shapes rather than listing them, so a parser that started
        // accepting one is caught even for a spelling nobody wrote here.)
        for bad in [
            "net:*.example.com:443",    // wildcard host
            "net:*:443",                // bare wildcard
            "net:.example.com:443",     // leading dot: the any-subdomain spelling
            "net:example..com:443",     // doubled dot: an empty label mid-name
            "net:example.com.:443",     // trailing dot: the DNS root label
            "net:.:443",                // nothing but the separator
            "net:10.0.0.0/8:443",       // CIDR
            "net:a.com:443,b.com:443",  // list (the comma is in the host)
            "net:example.com:443-8443", // port range
            "net:example.com:443,80",   // port list
            "net::443",                 // empty host
            "net:example.com:0",        // port 0 is not an endpoint
            "net:example.com:65536",    // out of range
            "net:example.com:-1",       // signed
            "net:example.com:+443",     // signed
            "net:example.com: 443",     // whitespace
            "net:example.com",          // no port at all
            "net:2001:db8::1:443",      // unbracketed IPv6: ambiguous colon
            "net:[2001:db8::1:443",     // unclosed bracket
            "net:[not-an-address]:443", // brackets are not a smuggling route
            "net:",                     // nothing at all
            "surface:main",             // a different prefix entirely
            "example.com:443",          // no prefix
        ] {
            assert!(
                NetSelector::parse(bad).is_err(),
                "`{bad}` must not parse: the grammar is wildcard-free by \
                 construction, and a parser that accepts a pattern makes \
                 `covers` a guess"
            );
        }
    }

    #[test]
    fn a_net_selector_covers_exactly_itself() {
        // Acceptance, issue #196: exact match only. A wildcard-free
        // grammar has no subsumption to express, and a `covers` that
        // guessed one would be authority the human never approved.
        let held = ResourceRef::Net(net("net:example.com:443"));

        assert!(held.covers(&held.clone()));
        assert!(!held.covers(&ResourceRef::Net(net("net:example.com:80"))));
        assert!(!held.covers(&ResourceRef::Net(net("net:sub.example.com:443"))));
        assert!(!held.covers(&ResourceRef::Net(net("net:example.com.evil.test:443"))));

        // And it is symmetric in the way exact match is: neither direction
        // subsumes the other.
        let other = ResourceRef::Net(net("net:example.com:80"));
        assert!(!other.covers(&held));

        // Whole-realm authority is not egress authority, in either
        // direction. Reading "the whole realm" as covering an endpoint
        // would make every observe grant an egress grant.
        assert!(!ResourceRef::WholeRealm.covers(&held));
        assert!(!held.covers(&ResourceRef::WholeRealm));
    }

    #[test]
    fn one_endpoint_can_have_several_selector_strings_and_none_covers_another() {
        // The IDL's canonical-port rule reads, at a glance, like a
        // guarantee that one endpoint has exactly one selector string. It
        // is not one, and the IDL says so in as many words -- this test is
        // what keeps the two in step. The host is stored verbatim, so every
        // legal spelling of one host is its own selector.
        //
        // Levered by making `NetHost::V6` re-emit `Ipv6Addr::to_string()`:
        // the round-trip assertion below goes red on the expanded literal,
        // which is the property that would actually be lost.
        let one_endpoint_many_spellings: &[&[&str]] = &[
            // DNS is case-insensitive; these bytes are not.
            &["net:Example.com:443", "net:example.com:443"],
            // One IPv6 address, three legal literals.
            &[
                "net:[2001:db8::1]:443",
                "net:[2001:0db8:0000:0000:0000:0000:0000:0001]:443",
                "net:[2001:DB8::1]:443",
            ],
        ];

        for spellings in one_endpoint_many_spellings {
            for raw in *spellings {
                // Each spelling is accepted and survives the round trip
                // byte-identically: the row stores what the human was shown.
                assert_eq!(
                    net(raw).to_wire(),
                    *raw,
                    "`{raw}` must round-trip byte-identically; normalising it \
                     would make the grant row hold a string nobody approved"
                );
            }
            // ...and no spelling covers any other, which is what "errs
            // narrow" means concretely: the wrong answer is a refusal.
            for held in *spellings {
                for want in *spellings {
                    let covers = ResourceRef::Net(net(held)).covers(&ResourceRef::Net(net(want)));
                    assert_eq!(
                        covers,
                        held == want,
                        "`{held}`.covers(`{want}`) must be exact-match only: \
                         two spellings of one endpoint are two selectors"
                    );
                }
            }
        }
    }

    // The component alphabets the selector generator draws from. Shared by
    // the proptest and by `the_generator_really_emits_the_forbidden_forms`,
    // which is what makes "the refusals are checked by generation" a fact
    // rather than a claim: if a hostile form were dropped from these
    // tables, the proptest would keep passing while testing less, and that
    // sibling test is what goes red instead.
    const PREFIX_FORMS: &[&str] = &["net:", "", "NET:", "net", "surface:", "node:", "net::"];
    const HOST_FORMS: &[&str] = &[
        // forms the grammar admits
        "example.com",
        "api.example.com",
        "a",
        "192.0.2.7",
        "[2001:db8::1]",
        "[::1]",
        "Example.COM",
        // forms it must refuse -- generated, not inspected
        "*",
        "*.example.com",
        ".example.com",
        "example..com",
        "example.com.",
        "10.0.0.0/8",
        "192.0.2.0/24",
        "a.com,b.com",
        "",
        " ",
        "exa mple.com",
        "2001:db8::1",
        "[2001:db8::1",
        "2001:db8::1]",
        "[not-an-address]",
        "exam\u{7f}ple.com",
    ];
    const PORT_FORMS: &[&str] = &[
        // forms the grammar admits
        "1", "80", "443", "8443", "65535", // forms it must refuse
        "0", "65536", "99999", "443-8443", "443,80", "+443", "-1", "", " 443", "443 ", "0443",
        "443a", "0x1bb",
    ];

    /// Every character class the grammar exists to keep out of an accepted
    /// selector. A string containing one of these must never parse.
    const WIDENING_CHARS: &[char] = &['*', '/', ','];

    #[test]
    fn the_generator_really_emits_the_forbidden_forms() {
        // Non-vacuity for the proptest below. Its refusal property
        // ("nothing accepted contains a wildcard, a CIDR or a comma")
        // is worth exactly as much as the generator's willingness to emit
        // those forms; a table that quietly lost them would leave a green
        // property asserting nothing -- the failure mode this repo keeps
        // finding. So pin that each is still in the alphabet.
        for needle in ["*", "/", ","] {
            assert!(
                HOST_FORMS.iter().any(|h| h.contains(needle)),
                "HOST_FORMS no longer emits any host containing `{needle}`, \
                 so the proptest's refusal property tests nothing"
            );
        }
        assert!(HOST_FORMS.contains(&""), "no empty host is generated");
        // An empty label in each of its three positions. The trailing one
        // is here because it was the one the parser missed while three
        // separate comments said it did not: a generator that emits only
        // the leading and doubled spellings tests two thirds of the rule.
        for empty_label in [".example.com", "example..com", "example.com."] {
            assert!(
                HOST_FORMS.contains(&empty_label),
                "`{empty_label}` is no longer generated, so the empty-label \
                 rule is only partly exercised"
            );
        }
        assert!(PORT_FORMS.contains(&"0"), "port 0 is not generated");
        assert!(PORT_FORMS.contains(&"65536"), "port 65536 is not generated");
        assert!(
            PORT_FORMS.iter().any(|p| p.contains('-')),
            "no port range is generated"
        );
        assert!(
            PORT_FORMS.iter().any(|p| p.contains(',')),
            "no port list is generated"
        );
    }

    /// Everything that must hold of a selector string the parser
    /// **accepted**. Written once and called from both the proptest and
    /// the exhaustive sweep below, so the sampled half and the complete
    /// half cannot drift into asserting different things.
    ///
    /// Panics on violation rather than returning: proptest catches the
    /// panic and shrinks on it exactly as it does for `prop_assert!`.
    fn assert_accept_side_properties(raw: &str, parsed: &NetSelector) {
        // Byte-identical re-serialization: the row stores the string the
        // human was shown, not a normalisation of it.
        assert_eq!(
            parsed.to_wire(),
            raw,
            "accepted `{raw}` and re-emitted it differently"
        );
        // Exactly one endpoint, and parsing is idempotent.
        assert_eq!(NetSelector::parse(&parsed.to_wire()).as_ref(), Ok(parsed));
        // ...and it is genuinely one pair, not a pattern.
        assert!(!parsed.host().is_empty());
        for c in WIDENING_CHARS {
            assert!(
                !raw.contains(*c),
                "accepted `{raw}`, which contains `{c}` -- the grammar must \
                 admit no wildcard, no CIDR and no list"
            );
        }
        // An accepted selector covers itself and nothing wider.
        let held = ResourceRef::Net(parsed.clone());
        assert!(held.covers(&held.clone()));
        assert!(!held.covers(&ResourceRef::WholeRealm));
    }

    proptest::proptest! {
        /// Acceptance, issue #196: over *generated* selector strings,
        /// every string the parser accepts round-trips to exactly one
        /// `(host, port)` pair, re-serializes byte-identically, and
        /// contains no wildcard, CIDR or comma.
        ///
        /// The refusals are checked **by generation**: the alphabets above
        /// emit `*.example.com`, `10.0.0.0/8`, `a.com,b.com`, `443-8443`,
        /// an empty host, port `0` and port `65536`, and the property is
        /// stated over whatever the generator produced rather than over a
        /// hand-listed table of bad strings.
        ///
        /// **Its accept side is nearly vacuous on its own** -- only a
        /// small fraction of the cross product parses, so a 256-case run
        /// exercises the round-trip property a handful of times and
        /// occasionally not at all. The cross-product sweep below is the
        /// non-vacuity guard: it walks every combination the alphabets
        /// admit, counts the acceptances, fails if there are none, and
        /// **asserts the fraction is small** rather than leaving a measured
        /// pair of numbers in a comment that the next alphabet edit would
        /// falsify.
        #[test]
        fn every_accepted_net_selector_round_trips_and_names_one_endpoint(
            prefix in proptest::sample::select(PREFIX_FORMS),
            host in proptest::sample::select(HOST_FORMS),
            port in proptest::sample::select(PORT_FORMS),
        ) {
            let raw = format!("{prefix}{host}:{port}");
            if let Ok(parsed) = NetSelector::parse(&raw) {
                assert_accept_side_properties(&raw, &parsed);
            }
        }
    }

    #[test]
    fn the_alphabet_cross_product_accepts_a_selector_and_every_one_holds() {
        // Non-vacuity for the proptest's ACCEPT side, the mirror of what
        // `the_generator_really_emits_the_forbidden_forms` does for its
        // refuse side. The proptest draws 256 samples from a space in which
        // only a small fraction parses, so a run can legitimately accept
        // nothing at all and still pass -- a green property asserting
        // nothing, the failure mode this repo keeps finding.
        //
        // Sweeping the whole cross product is both the guard and strictly
        // more coverage: every accepted combination is checked on every
        // run, not a sampled few.
        //
        // The fraction is MEASURED here and asserted, not written into a
        // comment: an earlier draft stated "35 of 2772", which stopped
        // being true the moment one form was added to one alphabet.
        let total = PREFIX_FORMS.len() * HOST_FORMS.len() * PORT_FORMS.len();
        let mut accepted = 0usize;
        for prefix in PREFIX_FORMS {
            for host in HOST_FORMS {
                for port in PORT_FORMS {
                    let raw = format!("{prefix}{host}:{port}");
                    if let Ok(parsed) = NetSelector::parse(&raw) {
                        accepted += 1;
                        assert_accept_side_properties(&raw, &parsed);
                    }
                }
            }
        }
        assert!(
            accepted > 0,
            "the alphabets no longer cross to a single selector the parser \
             accepts ({accepted} of {total}), so the proptest's round-trip, \
             idempotence and covers-itself properties assert nothing at all"
        );
        // ...and the sampled half really is nearly vacuous, which is the
        // whole reason this sweep exists. If the alphabets are ever
        // rebalanced so that most combinations parse, the proptest stops
        // needing a guard -- and this assertion is where a human is told
        // that, rather than the comment above quietly becoming false.
        assert!(
            accepted * 10 < total,
            "{accepted} of {total} combinations now parse. The proptest's \
             accept side is no longer nearly vacuous, so this sweep's \
             stated reason for existing has changed -- rewrite it rather \
             than deleting the assertion"
        );
    }

    #[test]
    fn pinned_addrs_is_null_on_every_row_this_core_can_build() {
        // Acceptance, issue #196: the column is present-but-null **by
        // construction**. Two independent things hold it, and the test
        // asserts both because either alone can be undone without the
        // other noticing.
        //
        // 1. `GrantSpec` has no field for it, so the one insert path
        //    cannot be handed a value (checked here over the specs a test
        //    can build, and over the row-`Debug` schema test above).
        // 2. `PinnedAddrs` is an empty enum, so no value exists to hand
        //    it. That is what makes the null type-enforced rather than
        //    conventional -- and it is why this assertion reads as
        //    tautological *today*: it stops being one the moment P2.7.4
        //    gives the type a variant, which is exactly when a row could
        //    start carrying an unaudited pin.
        let t0 = t0();
        let mut table = GrantTable::new();
        for verbs in [
            Verb::OBSERVE,
            Verb::OBSERVE | Verb::ACTUATE_TEXT,
            Verb::REALM_LAUNCH,
        ] {
            let id = table.insert(spec(DEMO, verbs, None), t0).unwrap();
            let (row, _) = table.get(id, t0).unwrap();
            assert!(
                row.pinned_addrs.is_none(),
                "a row built from today's GrantSpec carries a DNS pin no \
                 human approved"
            );
        }
    }
    // -- the published renderings of the served/unserved partition ---------

    /// The published spelling of every wire verb bit, plus the assertion that
    /// the table is complete.
    ///
    /// The book spells a verb with a dot where the wire spells it with an
    /// underscore. Written out per bit rather than derived by `replace`, so a
    /// verb whose published spelling is not its wire spelling cannot pass
    /// silently -- and the union assertion makes appending a bit to the IDL
    /// fail here until its spelling is added.
    ///
    /// Shared by the two page-reading tests below rather than copied into
    /// each: two book chapters render this partition, and a spelling table
    /// kept twice is the second copy that drifts.
    fn published_verb_spellings() -> Vec<(Verb, &'static str)> {
        let spellings = vec![
            (Verb::OBSERVE, "observe"),
            (Verb::ACTUATE_POINTER, "actuate.pointer"),
            (Verb::ACTUATE_TEXT, "actuate.text"),
            (Verb::OBSERVE_CURSOR, "observe.cursor"),
            (Verb::LAYOUT_ARRANGE, "layout.arrange"),
            (Verb::LAYOUT_FOCUS, "layout.focus"),
            (Verb::DESIGNATE_FILE, "designate.file"),
            // `egress` carries no underscore, so the replace-the-first-
            // underscore rule has nothing to replace and the dotted name is
            // the wire name unchanged. The IDL says so in as many words,
            // which is exactly why this table is written out rather than
            // derived: a rule with an unhandled case would have invented one.
            (Verb::EGRESS, "egress"),
            (Verb::REALM_LAUNCH, "realm.launch"),
        ];
        assert_eq!(
            spellings.iter().fold(0, |acc, (v, _)| acc | v.bits()),
            Verb::VALID_MASK,
            "this table must name every wire verb bit: a bit appended to the IDL has a \
             published spelling too, and deriving one by rule would invent it"
        );
        spellings
    }

    /// A small count in the register the book writes it in: prose says "two",
    /// not "2".
    fn count_word(n: usize) -> &'static str {
        match n {
            1 => "one",
            2 => "two",
            3 => "three",
            4 => "four",
            5 => "five",
            6 => "six",
            7 => "seven",
            8 => "eight",
            n => panic!(
                "{n} is beyond the range these book sentences have ever spelled. Add the \
                 word here and to the page that now needs it, deliberately"
            ),
        }
    }

    /// `["a", "b"]` -> ``"`a` and `b`"``, the way both bullets list verbs.
    fn backticked_english_list(items: &[&str]) -> String {
        let quoted: Vec<String> = items.iter().map(|i| format!("`{i}`")).collect();
        match quoted.split_last() {
            None => String::new(),
            Some((last, [])) => last.clone(),
            Some((last, head)) => format!("{} and {last}", head.join(", ")),
        }
    }

    /// The one bullet of `text` that starts at `marker`, whitespace-collapsed.
    ///
    /// Prose reflows; a Markdown line break inside a sentence is not drift.
    /// Collapsing runs of whitespace before matching is the same choice
    /// `crates/xtask/src/limits.rs`'s `normalize` makes for an `Anchor`.
    fn collapsed_bullet(text: &str, marker: &str, path: &str) -> String {
        let start = text.find(marker).unwrap_or_else(|| {
            panic!(
                "{path}: no {marker:?} bullet. Either the list was renamed or it was \
                 rewritten into a shape this scan cannot read -- and an empty slice would \
                 otherwise be reported as a missing claim"
            )
        });
        let bullet = &text[start..];
        let bullet = &bullet[..bullet[1..].find("\n- **").map_or(bullet.len(), |i| i + 1)];
        bullet.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The book publishes this partition **in words**, and until P2.6.5 nothing
    /// read the sentence.
    ///
    /// `docs/book/src/03-grants-consent-revocation.md` is on the mdBook the
    /// Pages workflow deploys, and its "What a grant is" bullet states the
    /// served verbs, then how many *more* are defined and refused
    /// `unsupported`, then names them. That is [`UNSERVED_VERB_BITS`] rendered
    /// as English, on a surface a reader reaches before any of this code.
    ///
    /// **It drifted exactly the way this repo's gates exist to catch.** P2.6.5
    /// (issue #189) added `designate_file` to the unserved set and reworded the
    /// count in ten files -- `petitions.rs`, `grants.rs`, `consent/render.rs`,
    /// `decode_errors.rs`, `test_verb_parity.py`, the IDL and four prose pages
    /// -- and left the published book saying "one more is defined and refuses
    /// `unsupported` -- `observe.cursor`". Every gate stayed green, because the
    /// sentence was in no gate's table.
    ///
    /// So it is held here rather than in `cargo xtask limits-check`, and the
    /// choice is deliberate: the value is not a literal any file states, it is
    /// `Verb::VALID_MASK & !SERVED_VERB_BITS`. Reading it from `xtask` would
    /// mean text-parsing a `1 | 2 | 4 | ...` expression out of *this* file and
    /// another out of generated code, which is a second copy of the derivation.
    /// Next to the constant it derives from, the test is the value.
    ///
    /// **Its honest bounds**, both of which are why it can be wrong in the safe
    /// direction only:
    ///
    /// * it matches the one shape this bullet has ever had -- a `- **verbs**`
    ///   list item carrying the phrase `N more are defined and refuse`. A
    ///   rewrite that keeps the fact but changes those words goes RED while
    ///   being correct; the failure names the file and the required phrase, so
    ///   the fix is to move this string with the prose.
    /// * it does not check the *served* half of the sentence. The book collapses
    ///   the two layout verbs into "the two `layout.*` verbs", so there is no
    ///   name-per-bit rendering on that side to compare against.
    #[test]
    fn the_books_grant_chapter_states_the_unserved_verb_count_and_names_them() {
        const BOOK: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/book/src/03-grants-consent-revocation.md"
        );
        /// The register the count sits in: value-free, so rendering a different
        /// count leaves it unmoved, and specific enough that its every
        /// occurrence on the page is this claim (asserted below).
        const CONTEXT: &str = " more are defined and refuse";

        let spellings = published_verb_spellings();
        let unserved: Vec<&str> = spellings
            .iter()
            .filter(|(v, _)| v.bits() & UNSERVED_VERB_BITS != 0)
            .map(|(_, name)| *name)
            .collect();
        // Non-vacuity: with an empty unserved set every assertion below would
        // be trivially satisfiable, and the sentence would need rewriting
        // rather than re-counting.
        assert!(
            !unserved.is_empty(),
            "no verb is unserved any more. That is a real change to what this core promises: \
             say so on {BOOK} deliberately and rewrite this test, rather than deleting it"
        );

        let text = std::fs::read_to_string(BOOK).expect("the book chapter exists");
        let bullet = collapsed_bullet(&text, "- **verbs**", BOOK);
        let page = text.split_whitespace().collect::<Vec<_>>().join(" ");

        // The count, in the surface's own register.
        let word = count_word(unserved.len());
        let rendered = if unserved.len() == 1 {
            "one more is defined and refuses".to_string()
        } else {
            format!("{word}{CONTEXT}")
        };
        assert!(
            bullet.contains(&rendered),
            "{BOOK}: the `- **verbs**` bullet does not say {rendered:?}. \
             `UNSERVED_VERB_BITS` holds {} verb(s) ({}), and the published sentence is the \
             count's only rendering a reader ever sees.\n\nbullet was:\n{bullet}",
            unserved.len(),
            unserved.join(", ")
        );
        // ...and it is the ONLY place on the page carrying that register, so a
        // stale second statement of the count cannot hide behind the first
        // being right. This is the property a bare `contains` cannot give.
        assert_eq!(
            page.matches(CONTEXT).count() + page.matches("more is defined and refuses").count(),
            1,
            "{BOOK} states the unserved-verb count in more than one place (or in none). \
             Every occurrence must be the canonical rendering; a second, disagreeing one \
             is exactly the drift this test exists for"
        );

        // The names, in the same bullet. The count alone would pass while the
        // sentence named the wrong two verbs.
        for name in &unserved {
            assert!(
                bullet.contains(&format!("`{name}`")),
                "{BOOK}: the `- **verbs**` bullet does not name `{name}`, which \
                 `UNSERVED_VERB_BITS` says is defined and refused `unsupported`.\
                 \n\nbullet was:\n{bullet}"
            );
        }
    }

    /// The **other** book chapter that renders this partition -- and the one
    /// that shows why holding chapter 3 alone was not enough.
    ///
    /// `docs/book/src/06-build-your-own-client.md` is chapter 6 of the mdBook
    /// the Pages workflow deploys (`SUMMARY.md` lists it), and its "Carry every
    /// defined verb" bullet is the instruction a third-party client author
    /// actually follows when transcribing the verb bitfield.
    ///
    /// **P2.6.5 (issue #189) left three of its claims false at once, and the
    /// diff never touched the file.** It listed the verbs to carry and omitted
    /// `designate.file`; it said this core "refuses `observe.cursor`" and
    /// stopped there, with the bit added on that very branch unmentioned; and
    /// it explained `realm.launch` = 512 by *"64/128/256 are allocated to verbs
    /// the IDL does not define yet and are still out of range"* on the branch
    /// that defined 64 and made petitioning for it recoverable. The bullet's
    /// own stated failure mode is that omitting a defined verb *"turns a
    /// recoverable `unsupported` refusal into a dead socket"* -- so it was
    /// instructing readers straight into the fault it warns about.
    ///
    /// **Why a sibling test rather than a widening of the one above.** The two
    /// bullets state different things: chapter 3 gives a COUNT of unserved
    /// verbs and names them, chapter 6 gives the whole verb LIST, the served
    /// remainder as a count, and the reserved bits that are still fatal.
    /// Nothing but the spelling table and the number words is common, and those
    /// are shared as functions. Merging the assertions would produce one test
    /// whose failure message could not say which page was wrong.
    ///
    /// **Its honest bounds:**
    ///
    /// * like its sibling, it matches the one shape this bullet has ever had.
    ///   A rewrite that keeps every fact but changes the wording goes RED while
    ///   being correct; each failure prints the phrase it wanted, so the fix is
    ///   to move the string with the prose.
    /// * the served half is checked as a **count**, not name by name. The
    ///   bullet does not name the served verbs -- that is the point of "never
    ///   bake the served set into a client" -- so there is nothing to compare
    ///   per bit.
    /// * the reserved-bit set is derived as *"every power of two below the
    ///   highest defined bit that the mask leaves out"*, which is what makes
    ///   the sentence go red the day one of them is allocated. It is not read
    ///   from the allocation registry in `docs/plan/02-phase-2-semantic-epochs.md`
    ///   §5, so a bit reserved ABOVE the highest defined one is invisible here.
    #[test]
    fn the_books_client_chapter_carries_every_verb_and_names_the_unserved_ones() {
        const BOOK: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/book/src/06-build-your-own-client.md"
        );
        const MARKER: &str = "- **Carry every defined verb";
        /// The register the refusal sits in: value-free, and every occurrence
        /// of it on the page is this claim (asserted below).
        const REFUSES: &str = "this core refuses ";
        /// ...and the register the served remainder sits in.
        const SERVES: &str = ", and serves the other ";

        let spellings = published_verb_spellings();
        let text = std::fs::read_to_string(BOOK).expect("the book chapter exists");
        let bullet = collapsed_bullet(&text, MARKER, BOOK);
        let page = text.split_whitespace().collect::<Vec<_>>().join(" ");

        // 1. "Carry every defined verb" -- so the parenthesised list must name
        //    every one. This is the claim whose failure the bullet itself
        //    describes as a dead socket.
        //
        //    Scoped to the LIST rather than to the bullet, and measured: with
        //    a `bullet.contains` this assertion passed while the list was
        //    missing `designate.file`, because the verb is named again two
        //    sentences later. A membership test whose haystack is the whole
        //    paragraph tests the paragraph, not the list.
        let list = bullet
            .split_once("** (")
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(list, _)| list)
            .unwrap_or_else(|| {
                panic!(
                    "{BOOK}: the {MARKER:?} bullet carries no parenthesised verb list \
                     directly after its bold lead-in.\n\nbullet was:\n{bullet}"
                )
            });
        for (verb, name) in &spellings {
            assert!(
                list.contains(&format!("`{name}`")),
                "{BOOK}: the {MARKER:?} bullet's verb list does not name `{name}` ({:#x}), \
                 which the IDL defines. The bullet's own text says omitting one turns a \
                 recoverable `unsupported` refusal into a dead socket.\n\nlist was:\n{list}",
                verb.bits()
            );
        }
        // ...and names nothing else. A list that grew an entry the IDL does
        // not define would send a client author to a fatal bit, which is the
        // same failure in the other direction.
        assert_eq!(
            list.matches('`').count(),
            spellings.len() * 2,
            "{BOOK}: the {MARKER:?} bullet's verb list holds a backticked name that is not \
             one of the {} the IDL defines.\n\nlist was:\n{list}",
            spellings.len()
        );

        // 2. The unserved half, named -- the same derivation chapter 3 renders
        //    as a count.
        let unserved: Vec<&str> = spellings
            .iter()
            .filter(|(v, _)| v.bits() & UNSERVED_VERB_BITS != 0)
            .map(|(_, name)| *name)
            .collect();
        assert!(
            !unserved.is_empty(),
            "no verb is unserved any more. Rewrite this bullet on {BOOK} and this test \
             deliberately, rather than deleting either"
        );
        let refusal = format!("{REFUSES}{}", backticked_english_list(&unserved));
        assert!(
            bullet.contains(&refusal),
            "{BOOK}: the {MARKER:?} bullet does not say {refusal:?}. `UNSERVED_VERB_BITS` \
             holds {} verb(s) ({}).\n\nbullet was:\n{bullet}",
            unserved.len(),
            unserved.join(", ")
        );
        assert_eq!(
            page.matches(REFUSES).count(),
            1,
            "{BOOK} states what this core refuses in more than one place (or in none). A \
             second, disagreeing statement is exactly the drift this test exists for"
        );

        // 3. The served remainder, as a count. `serves the other six` is a
        //    claim about SERVED_VERB_BITS, and it moves when a verb is
        //    classified either way.
        let served = format!(
            "{SERVES}{}",
            count_word(SERVED_VERB_BITS.count_ones() as usize)
        );
        assert!(
            bullet.contains(&served),
            "{BOOK}: the {MARKER:?} bullet does not say {served:?}. `SERVED_VERB_BITS` holds \
             {} verb(s).\n\nbullet was:\n{bullet}",
            SERVED_VERB_BITS.count_ones()
        );
        assert_eq!(
            page.matches(SERVES).count(),
            1,
            "{BOOK} states the served-verb count in more than one place (or in none)"
        );

        // 4. `realm.launch` is 512 -- the value the bullet tells a client
        //    author to transcribe, and the reason the next assertion exists.
        assert!(
            bullet.contains(&format!("`realm.launch` is {}", Verb::REALM_LAUNCH.bits())),
            "{BOOK}: the {MARKER:?} bullet does not state `realm.launch`'s value as {}.\
             \n\nbullet was:\n{bullet}",
            Verb::REALM_LAUNCH.bits()
        );

        // 5. ...and the bits that are still out of range, which is the claim
        //    that went false. Every power of two below the top defined bit that
        //    the mask leaves out: allocated in the plan's registry, absent from
        //    the IDL, and therefore still fatal rather than `unsupported`.
        let top = u32::BITS - 1 - Verb::VALID_MASK.leading_zeros();
        let reserved: Vec<String> = (0..top)
            .map(|shift| 1u32 << shift)
            .filter(|bit| Verb::VALID_MASK & bit == 0)
            .map(|bit| bit.to_string())
            .collect();
        // Non-vacuity: with no gap left, the sentence explaining the gap has to
        // be rewritten rather than re-numbered.
        assert!(
            !reserved.is_empty(),
            "the verb bitfield has no gap below {:#x} any more, so {BOOK}'s explanation of \
             why `realm.launch` is not 64 no longer describes anything. Rewrite the bullet \
             and this test",
            Verb::VALID_MASK
        );
        let names: Vec<&str> = reserved.iter().map(String::as_str).collect();
        let gap = format!(
            "because {} are allocated to verbs the IDL does not define yet",
            names.join(" and ")
        );
        assert!(
            bullet.contains(&gap),
            "{BOOK}: the {MARKER:?} bullet does not say {gap:?}. The bits still outside \
             `VALID_MASK` ({:#x}) below its top bit are exactly {}; naming a bit the IDL \
             now DEFINES tells a client author a recoverable petition is fatal, which is \
             the error this assertion was added for.\n\nbullet was:\n{bullet}",
            Verb::VALID_MASK,
            reserved.join(", ")
        );
    }

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
    fn dead_rows_answer_not_granted_for_verbs_they_never_conferred() {
        let t0 = t0();
        let mut table = GrantTable::new();
        // A live observe grant beside a revoked pointer grant -- the
        // routine state after hold-Esc followed by a narrower re-grant.
        table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        let revoked_id = table
            .insert(spec(DEMO, Verb::ACTUATE_POINTER, None), t0)
            .unwrap();
        assert!(table.revoke(revoked_id));
        // No grant ever conferred actuate_text: the IDL's unconditional
        // ungranted-facet rule says `not_granted` -- `revoked` would smear
        // the pointer grant's death onto authority it never touched, and
        // the SDK would raise Revoked (human-stop semantics) where the
        // honest recovery is petitioning for the verb.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::ACTUATE_TEXT, t0),
            Err(RefusalReason::NotGranted)
        );
        // The live grant's own verb is untouched by the sibling's death.
        assert!(use_at(&mut table, DEMO, Verb::OBSERVE, t0).is_ok());

        // Same rule for expiry: a lone expired observe-only grant answers
        // `not_granted`, not `expired`, for a verb it never conferred...
        let mut lone = GrantTable::new();
        lone.insert(spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(1))), t0)
            .unwrap();
        let later = t0 + Duration::from_secs(2);
        assert_eq!(
            use_at(&mut lone, DEMO, Verb::ACTUATE_TEXT, later),
            Err(RefusalReason::NotGranted)
        );
        // ...while the verb it DID confer keeps the death code (flow 5).
        assert_eq!(
            use_at(&mut lone, DEMO, Verb::OBSERVE, later),
            Err(RefusalReason::Expired)
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
        let id = table
            .insert(spec(DEMO, Verb::OBSERVE | Verb::ACTUATE_POINTER, None), t0)
            .unwrap();

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
        // Both *granted* verbs of a dead grant refuse the death code (IDL
        // flow 4: one code, two verbs, one chokepoint).
        assert_eq!(
            use_at(&mut table, DEMO, Verb::ACTUATE_POINTER, t0),
            Err(RefusalReason::Revoked)
        );
        // A verb the grant never conferred is `not_granted` even after
        // revocation: the death code belongs only to authority the row
        // actually conferred (IDL's unconditional ungranted-facet rule).
        assert_eq!(
            use_at(&mut table, DEMO, Verb::ACTUATE_TEXT, t0),
            Err(RefusalReason::NotGranted)
        );
    }

    #[test]
    fn revoke_principal_kills_all_and_only_that_principals_grants() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let observe = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        let pointer = table
            .insert(spec(DEMO, Verb::ACTUATE_POINTER, None), t0)
            .unwrap();
        table.insert(spec(OTHER, Verb::OBSERVE, None), t0).unwrap();

        // The ids are *named*, ascending, not merely counted -- what the
        // flight recorder needs to say which authority a dead-man switch
        // killed.
        assert_eq!(
            table.revoke_principal(&principal(DEMO)),
            vec![observe, pointer]
        );
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
        assert!(table.revoke_principal(&principal(DEMO)).is_empty());
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
    fn selection_prefers_the_newest_grant_among_equal_rungs() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let mut old = spec(DEMO, Verb::OBSERVE, None);
        old.max_event_rate = rate(5);
        let old_id = table.insert(old, t0).unwrap();
        let mut renewed = spec(DEMO, Verb::OBSERVE, None);
        renewed.max_event_rate = rate(100);
        let new_id = table.insert(renewed, t0).unwrap();

        // The most recent consent decision governs: the re-grant's raised
        // rate ceiling takes effect immediately, not only after the old
        // while_running row dies.
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0),
            Ok(Allowed {
                grant_id: new_id,
                max_event_rate: rate(100),
            })
        );
        // With the newest revoked, the older durable row serves again.
        assert!(table.revoke(new_id));
        assert_eq!(
            use_at(&mut table, DEMO, Verb::OBSERVE, t0).map(|a| a.grant_id),
            Ok(old_id)
        );
    }

    // -- the grant-scoped chokepoint query ----------------------------------

    #[test]
    fn a_dead_grants_facets_stay_inert_even_beside_a_live_sibling() {
        // IDL: "A grant that later expires or is revoked goes dead and its
        // facets go inert" -- a use arriving through grant A's facet must
        // refuse A's death code even though live grant B covers the same
        // verb, and must never be served by (or attributed to) B.
        let t0 = t0();
        let mut table = GrantTable::new();
        let a = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        let b = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert!(table.revoke(a));
        assert_eq!(
            table.check_use_grant(a, &principal(DEMO), Verb::OBSERVE, t0),
            Err(RefusalReason::Revoked)
        );
        // The sibling's facet still serves, attributed to itself.
        assert_eq!(
            table
                .check_use_grant(b, &principal(DEMO), Verb::OBSERVE, t0)
                .map(|allowed| allowed.grant_id),
            Ok(b)
        );
    }

    #[test]
    fn check_use_grant_scopes_admission_to_exactly_one_row() {
        let t0 = t0();
        let mut table = GrantTable::new();
        let mut once = spec(DEMO, Verb::OBSERVE, None);
        once.persistence = PersistenceRung::Once;
        let once_id = table.insert(once, t0).unwrap();
        let wr_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();

        // The judgement is pure: asking twice consumes nothing (two-phase
        // admission, module docs) -- even for a `once` grant, and even
        // though a durable sibling covers the verb (no cross-row
        // selection: the agent designated which authority it exercised).
        for _ in 0..2 {
            assert_eq!(
                table
                    .check_use_grant(once_id, &principal(DEMO), Verb::OBSERVE, t0)
                    .map(|allowed| allowed.grant_id),
                Ok(once_id)
            );
        }
        assert_eq!(table.get(once_id, t0).unwrap().1, GrantState::Active);
        // The commit is what spends the single use, on this row alone.
        table.commit_use(once_id);
        assert_eq!(table.get(once_id, t0).unwrap().1, GrantState::Spent);
        assert_eq!(
            table.check_use_grant(once_id, &principal(DEMO), Verb::OBSERVE, t0),
            Err(RefusalReason::Expired)
        );
        // Committing a while_running use is a no-op, forever.
        table.commit_use(wr_id);
        assert_eq!(table.get(wr_id, t0).unwrap().1, GrantState::Active);
        // Committing a dead or missing row changes nothing (defensive).
        table.commit_use(once_id);
        assert_eq!(table.get(once_id, t0).unwrap().1, GrantState::Spent);

        // An ungranted verb on a live grant refuses `not_granted`...
        assert_eq!(
            table.check_use_grant(wr_id, &principal(DEMO), Verb::ACTUATE_TEXT, t0),
            Err(RefusalReason::NotGranted)
        );
        // ...a foreign principal refuses `not_granted` (the
        // sender-constrained cross-check, fail-closed)...
        assert_eq!(
            table.check_use_grant(wr_id, &principal(OTHER), Verb::OBSERVE, t0),
            Err(RefusalReason::NotGranted)
        );
        // ...and a removed row refuses `not_granted` (teardown semantics:
        // no row, so the asker never held a live handle).
        assert!(table.remove(wr_id));
        assert_eq!(
            table.check_use_grant(wr_id, &principal(DEMO), Verb::OBSERVE, t0),
            Err(RefusalReason::NotGranted)
        );
    }

    #[test]
    fn proactive_sweep_flips_state_without_a_use() {
        // The issue-#28 acceptance: a time-bounded grant must not linger
        // *reporting itself* usable until its next call. The sweep flips
        // stored state with no use in between; enforcement itself never
        // depends on it (the deadline check runs on every query).
        let t0 = t0();
        let mut table = GrantTable::new();
        let bounded = table
            .insert(spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(5))), t0)
            .unwrap();
        let unbounded = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        let revoked = table
            .insert(spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(5))), t0)
            .unwrap();
        assert!(table.revoke(revoked));

        // Strictly before the deadline: nothing due.
        assert!(table
            .expire_due(t0 + Duration::from_millis(4_999))
            .is_empty());

        // At the (half-open, fail-closed) deadline: exactly the bounded
        // active row flips -- the unbounded row lives, and the revoked row
        // is already dead so it is not re-reported as newly expired.
        let deadline = t0 + Duration::from_secs(5);
        assert_eq!(table.expire_due(deadline), vec![bounded]);
        assert_eq!(table.get(bounded, deadline).unwrap().1, GrantState::Expired);
        assert_eq!(
            table.get(unbounded, deadline).unwrap().1,
            GrantState::Active
        );
        assert_eq!(table.get(revoked, deadline).unwrap().1, GrantState::Revoked);

        // Idempotent: the next poll reports nothing new.
        assert!(table
            .expire_due(deadline + Duration::from_secs(1))
            .is_empty());

        // The flipped row refuses `expired` on use, and revoking it later
        // still wins the precedence (the deliberate act is the loudest
        // fact) -- the stored flip does not shadow revocation.
        assert_eq!(
            table.check_use_grant(bounded, &principal(DEMO), Verb::OBSERVE, deadline),
            Err(RefusalReason::Expired)
        );
        assert!(table.revoke(bounded));
        assert_eq!(
            table.check_use_grant(bounded, &principal(DEMO), Verb::OBSERVE, deadline),
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
        assert_eq!(table.rows(t0).count(), 0);
    }

    #[test]
    fn an_empty_verb_set_is_refused_and_consumes_nothing() {
        // The query-side mirror of `InsertError::EmptyVerbs`:
        // `Verb::contains` is subset semantics, so `Verb(0)` is vacuously
        // contained by every row -- without the guard an empty-verb use
        // would be admitted by any live covering row and could spend a
        // `once`. Both queries must refuse, and burn nothing.
        let t0 = t0();
        let mut table = GrantTable::new();
        let mut once = spec(DEMO, Verb::OBSERVE, None);
        once.persistence = PersistenceRung::Once;
        let once_id = table.insert(once, t0).unwrap();

        assert_eq!(
            use_at(&mut table, DEMO, Verb::default(), t0),
            Err(RefusalReason::NotGranted)
        );
        assert_eq!(
            table.check_use_grant(once_id, &principal(DEMO), Verb::default(), t0),
            Err(RefusalReason::NotGranted)
        );
        assert_eq!(
            table.get(once_id, t0).unwrap().1,
            GrantState::Active,
            "a refused empty-verb use must not spend the once grant"
        );
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
        // rendering -- the row shape is the schema, verbatim -- plus the
        // one column added since (`pinned_addrs`, P2.7.2), which is listed
        // here for the same reason: a column the row carries and this list
        // omits is a column nothing checks the nullity of.
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
            "pinned_addrs",
            "issued_at",
            "issuer",
        ] {
            assert!(
                debug.contains(field),
                "Debug output lacks `{field}`: {debug}"
            );
        }

        // The unfillable fields are present and visibly null -- never
        // omitted, never fabricated.
        for null_field in [
            "focus_condition: None",
            "one_shot: None",
            "provenance_ref: None",
            "parent_grant_id: None",
            "pinned_addrs: None",
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
    fn rows_pairs_every_row_with_its_liveness() {
        // The enumeration surface folds liveness in exactly as `get`
        // does: a panel iterating it can never render dead authority as
        // live, and there is no liveness-free enumeration to misuse.
        let t0 = t0();
        let mut table = GrantTable::new();
        let live_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        let revoked_id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert!(table.revoke(revoked_id));
        let expired_id = table
            .insert(spec(DEMO, Verb::OBSERVE, Some(Duration::from_secs(1))), t0)
            .unwrap();

        let later = t0 + Duration::from_secs(5);
        let snapshot: Vec<(GrantId, GrantState)> = table
            .rows(later)
            .map(|(row, state)| (row.grant_id, state))
            .collect();
        assert_eq!(
            snapshot,
            vec![
                (live_id, GrantState::Active),
                (revoked_id, GrantState::Revoked),
                (expired_id, GrantState::Expired),
            ],
            "ascending-id enumeration, each row with its get()-equal state"
        );
    }

    #[test]
    fn default_matches_new_including_the_first_minted_id() {
        // A derived Default would zero `next_id` and mint `grant-0` the
        // moment a server-state struct derives Default around the table;
        // the hand-written impl keeps both construction paths identical.
        let t0 = t0();
        let mut table = GrantTable::default();
        let id = table.insert(spec(DEMO, Verb::OBSERVE, None), t0).unwrap();
        assert_eq!(id.as_u64(), 1);
        assert_eq!(id.to_string(), "grant-1");
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
