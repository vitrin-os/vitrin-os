// SPDX-License-Identifier: MPL-2.0
//! **Obligations the chokepoint owes** (P2.6.6's prerequisite, issue #343):
//! the ledger of designation asks that were admitted and whose one terminal
//! has not been produced yet.
//!
//! # Why this exists
//!
//! [`crate::enforcement::Chokepoint::enforce_use`] completes every admitted
//! operation **inside the call**. A capture renders, an actuation reaches its
//! sink, a launch forks; `UseOutcome::Admitted` is returned afterwards and its
//! fields describe what happened. That is deliberate, and the launch arm's own
//! docs argue it: `launched` is a terminal whose *failure* is discovered in
//! the fork, so deferring it would mean replying success and finding out
//! afterwards.
//!
//! A designation cannot fit that shape. `vitrin_powerbox.request_file` is
//! admitted the moment the authority chain says this principal may ask; its
//! terminal — a designated fd, or a refusal — arrives when a **human** has
//! finished browsing. Seconds. Possibly never.
//!
//! So an admitted ask mints a [`DesignationTicket`] here, and the terminal is
//! produced later by redeeming it.
//!
//! # What a ticket is, and what it is emphatically not
//!
//! **A ticket is a name, never an answer.** Every field on it names something
//! the chokepoint already judged — which connection, which facet, which grant
//! row, which principal, which realm. None of them is the authority decision,
//! and there is no method here that turns one into authority.
//!
//! That is the difference between this type and its two in-tree precedents.
//! [`crate::attention::Exemption`] and `vitrin-realm-init`'s `RungsEntered`
//! are both spent microseconds after they are minted, inside one call, so
//! they may safely *carry a decision*. A ticket lives across seconds of human
//! time, during which a grant can be revoked, expire, or have its realm die.
//! Holding a decision across that gap is precisely the "an fd delivered under
//! a grant that no longer exists" hazard, and a delivered descriptor is
//! kernel authority the core **cannot recall**. So the answer is re-derived
//! from the live table at the instant of delivery, and the ticket contributes
//! only the names that query needs.
//!
//! Four properties are taken from those precedents unchanged:
//!
//! 1. **Private fields, one mint site.** Nothing outside this module can
//!    construct a ticket.
//! 2. **It carries the thing, not a number beside it.** `RungsEntered` learned
//!    this expensively: its first shape held a `u32` the ledger was *told*, so
//!    `entering(1)` beside a rung-4 ruleset compiled.
//! 3. **Neither `Copy` nor `Clone`, consumed by value.** [`Ledger::take`]
//!    removes the ticket and hands it over, and redemption consumes it — so a
//!    second redemption is a *compile error*, not a runtime guard.
//! 4. **`#[must_use]`** for the opposite mistake: minting one and forgetting
//!    to spend it is a client waiting forever.
//!
//! # Three things this module refuses to hold
//!
//! - **No descriptor.** The ledger never holds an open fd, so an outstanding
//!   obligation cannot pin a file open. The fd does not exist until the
//!   redemption has already re-judged the grant — see [`Redeemed`].
//! - **No authority answer.** See above.
//! - **No clock.** Deadlines arrive as an `Instant` from the caller, on this
//!   crate's usual terms.
//!
//! # Nothing in the shipped tree calls this yet, and that is the honest state
//!
//! This module is the **enforcement half** of issue #343 and it landed before
//! its callers. `crate::enforcement` does not mint a ticket yet — a
//! `UseKind::Designate` is still refused `internal` by the arm that has always
//! refused it — and `crate::session` does not sweep or redeem. The picker that
//! would raise a card (P2.6.6, issue #190) does not exist at all.
//!
//! So the `allow` below is a statement, not a convenience: **every item here
//! is exercised by this module's own tests and by nothing else in the binary.**
//! It is written at the module rather than per item so that removing it is one
//! edit on the day the chokepoint wires this up, and so a reader cannot mistake
//! a scattering of per-item allows for individual judgements about each.
//!
//! Hold this module to "proven mechanism, not in service", exactly as
//! `consent`'s interactive panel (issue #341) is held. What its tests prove is
//! that the ledger's rules are the ones written above; they prove nothing about
//! anything reaching them.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Instant;

use crate::grants::{GrantId, RealmId};
use crate::identity::PrincipalIdentity;
use crate::petitions::ConnectionId;

/// The core's id for one designation.
///
/// `u32` because that is the wire width of
/// `vitrin_powerbox.designated.designation_id`. Minted from 1, monotonic, and
/// **never reused**, so a stale id names nothing rather than naming something
/// new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DesignationId(u32);

impl DesignationId {
    /// The wire value.
    pub(crate) fn get(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for DesignationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "designation-{}", self.0)
    }
}

/// Which of the powerbox's two asks this is.
///
/// The payload `UseKind::Designate` did not carry. `mode` rides `File`
/// because `request_file` takes one on the wire and `request_dir` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskedFor {
    /// `request_file`: the mode the picker would open as. The human may
    /// narrow it, and the terminal carries the effective answer — so this is
    /// a ceiling, never a promise.
    File { write: bool },
    /// `request_dir`: a subtree, delivered as a directory fd. No mode
    /// argument exists on the wire.
    Dir,
}

/// What a timeout does to a `once` rung the ask already spent.
///
/// **Owner decision, 2026-09-07.** An explicit cancel *always* spends the
/// rung: the human was asked, they answered, and the answer was no. What a
/// **timeout** does — nobody touched the card at all — is configurable,
/// because the two readings are both defensible and the cost falls in
/// different places.
///
/// The default is [`Self::Spends`], which is also what the code does today
/// with no restore path at all: `commit_use` marks the row `Spent` at
/// admission, fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TimeoutPolicy {
    /// A timed-out ask spends the rung, exactly as a cancel does. The agent
    /// must petition again and the human must approve again.
    ///
    /// Fail-closed, and it is the only option that needs no way to move a
    /// grant row *back* to `Active` — so it is the default.
    #[default]
    Spends,
    /// A timed-out ask restores the rung it spent, so a human who walked away
    /// has not cost the agent its grant.
    ///
    /// **The cost, stated rather than buried.** With this set, an agent whose
    /// ask times out may ask again. `busy` allows only one outstanding
    /// designation per principal, so the rate is bounded by the deadline
    /// rather than by the rate ceiling: one card per deadline, indefinitely,
    /// for the life of the grant. At a 30-second deadline that is 120 cards an
    /// hour, each of which takes the screen. It is a far weaker version of the
    /// unbounded-picker problem that not spending at admission would create,
    /// but it is not zero, and a deployment choosing this should know it is
    /// choosing it.
    Restores,
}

/// **The obligation one admitted designation ask minted.**
///
/// Not authority, and it cannot become authority: see this module's docs.
#[must_use = "an admitted designation owes exactly one terminal; a dropped \
              ticket is a client waiting forever"]
#[derive(Debug)]
pub(crate) struct DesignationTicket {
    id: DesignationId,
    /// The connection the ask arrived on. Carried so a redemption can only
    /// ever address the peer that asked — `ConnectionId`s are never reused,
    /// so a stale one resolves to nothing rather than to somebody else.
    connection: ConnectionId,
    /// The `vitrin_powerbox` facet object the ask arrived on.
    facet_id: u32,
    /// The `vitrin_grant` handle a refusal is addressed to.
    grant_wire_id: u32,
    /// The row the chain judged.
    grant: GrantId,
    /// The verifier-canonical identity bound at `hello`.
    principal: PrincipalIdentity,
    /// **The realm the grant named, at the instant the ask was admitted.**
    ///
    /// Load-bearing, and the reason is a hole none of this issue's three
    /// candidate designs caught. `session::close_realm` clears the clipboard
    /// slot when a realm dies — its comment says the slot "cannot outlive the
    /// realm that authored its bytes" and that an outstanding promotion "is
    /// abandoned on the same line" — and it touches **no grant row**. So a
    /// liveness query alone answers `Live` for a grant over a realm that is
    /// gone. Worse, a *declared* realm keeps its id across a process death,
    /// so the same `RealmId` can name a new process after a relaunch: the
    /// human's editor crashes mid-pick, they restart it, they confirm, and
    /// the descriptor lands in a process they never approved.
    ///
    /// Carrying the realm is half the fix; the other half is that redemption
    /// asks whether *this* realm is still the live one — see [`Redeemed`].
    realm: RealmId,
    /// Which ask it was.
    ask: AskedFor,
    /// **Whether the admission that minted this ticket spent a `once` rung.**
    ///
    /// The sharpest fact in this whole problem, and it is invisible from the
    /// documentation. `GrantTable::commit_use` sets a `once` row's liveness to
    /// `Spent` **at admission**, before the operation, and `refusal_for` maps
    /// `Spent` to `RefusalReason::Expired`. So by the table's own reckoning a
    /// `once` designation grant is already dead at the moment its ticket is
    /// minted, and a redemption that naively re-asked the table would refuse
    /// **every single-use designation, always**.
    ///
    /// This field is what lets the redemption tell "this row reads `Spent`
    /// because *I* spent it" from "this row died". It is carried as a fact on
    /// the ticket rather than inferred at redemption, so the forgiveness can
    /// only ever apply to the one ticket that earned it — never to any other
    /// ticket that happens to name the same row.
    spent_once: bool,
    /// When this obligation dies unredeemed.
    deadline: Instant,
}

impl DesignationTicket {
    pub(crate) fn id(&self) -> DesignationId {
        self.id
    }
    pub(crate) fn connection(&self) -> ConnectionId {
        self.connection
    }
    pub(crate) fn facet_id(&self) -> u32 {
        self.facet_id
    }
    pub(crate) fn grant_wire_id(&self) -> u32 {
        self.grant_wire_id
    }
    pub(crate) fn grant(&self) -> GrantId {
        self.grant
    }
    pub(crate) fn principal(&self) -> &PrincipalIdentity {
        &self.principal
    }
    pub(crate) fn realm(&self) -> &RealmId {
        &self.realm
    }
    pub(crate) fn ask(&self) -> AskedFor {
        self.ask
    }
    pub(crate) fn spent_once(&self) -> bool {
        self.spent_once
    }
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// Why an admitted ask raised no picker.
///
/// Reachable only **after** the authority chain said yes and **inside** the
/// dispatch turn. Deliberately not `crate::enforcement::Refusal`, on
/// `LaunchRefusal`'s precedent: letting a mechanism name an authority code
/// would let the embedder invent an authority answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenRefusal {
    /// This principal already owes a terminal. The IDL's `refusal.busy`:
    /// at most one at a time, because two cards stacked in front of one human
    /// is the consent-fatigue shape.
    Busy,
    /// The ledger is at its resource bound.
    Full,
}

/// The most obligations the ledger will hold at once, across all principals.
///
/// A **resource bound**, not the IDL's single-picker rule — that one is per
/// principal and is [`OpenRefusal::Busy`]. This is the backstop that stops a
/// ledger growing with peers, and it is deliberately small: more than a
/// handful of outstanding designations is not a session anyone is having.
const MAX_OUTSTANDING: usize = 8;

/// Why a redemption did not deliver.
///
/// Every variant is a fact about the world at the instant of delivery, and
/// none of them is recoverable by retrying with the same ticket — the ticket
/// is consumed either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Undelivered {
    /// The grant was revoked, expired, or spent by something other than this
    /// ticket. **The case this whole module is shaped around.**
    AuthorityDied,
    /// The realm the grant named is gone, or the id now names a different
    /// process. See [`DesignationTicket::realm`].
    RealmDied,
    /// Nobody redeemed it before its deadline.
    TimedOut,
    /// The connection went away.
    ConnectionGone,
}

/// A redemption the ledger judged deliverable.
///
/// **Holding one means the grant and the realm were both re-checked and both
/// answered yes**, and it is the only value in this crate that says so. It is
/// produced solely by [`Ledger::redeem`], consumes the ticket, and is itself
/// neither `Copy` nor `Clone` — so the "we checked" claim cannot be made
/// twice from one check, and cannot be made at all without a check having
/// happened.
///
/// **No descriptor has been opened at this point, and that ordering is the
/// point.** The caller opens the file *after* holding one of these, so a
/// revoked grant does not merely fail to send an fd — it never causes the
/// core to touch the human's filesystem at all.
#[must_use = "a judged redemption that is dropped is a terminal the client \
              never receives"]
#[derive(Debug)]
pub(crate) struct Redeemed {
    ticket: DesignationTicket,
}

impl Redeemed {
    pub(crate) fn ticket(&self) -> &DesignationTicket {
        &self.ticket
    }
    /// Consume the judgement and take the ticket back out.
    pub(crate) fn into_ticket(self) -> DesignationTicket {
        self.ticket
    }
}

/// What the ledger needs to know about the world to judge a redemption.
///
/// A trait rather than two closures so the two questions are answered from
/// one place by one owner, and so this module can be tested without a grant
/// table or a realm registry.
pub(crate) trait Liveness {
    /// Is this grant row still usable by this principal for this verb?
    ///
    /// `spent_by_this_ticket` is [`DesignationTicket::spent_once`], and the
    /// implementation **must** use it to forgive a `Spent` reading — and
    /// forgive it *only* then. See that field's docs.
    fn grant_is_live(&self, grant: GrantId, spent_by_this_ticket: bool) -> bool;

    /// Is this realm still the same live realm the ask was admitted against?
    ///
    /// Not merely "does a realm with this id exist": a declared realm keeps
    /// its id across a process death.
    fn realm_is_the_same(&self, realm: &RealmId) -> bool;
}

/// The outstanding-obligation table.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    open: BTreeMap<DesignationId, DesignationTicket>,
    next: u32,
    policy: TimeoutPolicy,
}

impl Ledger {
    pub(crate) fn new(policy: TimeoutPolicy) -> Self {
        Self {
            open: BTreeMap::new(),
            next: 1,
            policy,
        }
    }

    pub(crate) fn timeout_policy(&self) -> TimeoutPolicy {
        self.policy
    }

    /// **The one mint.**
    ///
    /// Refuses [`OpenRefusal::Busy`] when this principal already owes a
    /// terminal, and [`OpenRefusal::Full`] at the resource bound.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        &mut self,
        connection: ConnectionId,
        facet_id: u32,
        grant_wire_id: u32,
        grant: GrantId,
        principal: &PrincipalIdentity,
        realm: &RealmId,
        ask: AskedFor,
        spent_once: bool,
        deadline: Instant,
    ) -> Result<DesignationId, OpenRefusal> {
        if self.owes(principal) {
            return Err(OpenRefusal::Busy);
        }
        if self.open.len() >= MAX_OUTSTANDING {
            return Err(OpenRefusal::Full);
        }
        let id = DesignationId(self.next);
        // Saturating rather than wrapping: ids are never reused, and a core
        // that somehow minted four billion designations should stop minting
        // rather than start reusing. `open` then refuses `Full` forever,
        // which is fail-closed.
        self.next = self.next.saturating_add(1);
        self.open.insert(
            id,
            DesignationTicket {
                id,
                connection,
                facet_id,
                grant_wire_id,
                grant,
                principal: principal.clone(),
                realm: realm.clone(),
                ask,
                spent_once,
                deadline,
            },
        );
        Ok(id)
    }

    /// Whether this principal already owes a terminal.
    pub(crate) fn owes(&self, principal: &PrincipalIdentity) -> bool {
        self.open.values().any(|t| &t.principal == principal)
    }

    pub(crate) fn outstanding(&self) -> usize {
        self.open.len()
    }

    /// **The one spend.**
    ///
    /// Removes the ticket and hands it over by value. A second call with the
    /// same id returns `None` — and because the ticket is neither `Copy` nor
    /// `Clone`, a caller cannot keep a spare either. This, not a flag, is what
    /// makes a double redemption unreachable.
    pub(crate) fn take(&mut self, id: DesignationId) -> Option<DesignationTicket> {
        self.open.remove(&id)
    }

    /// Take a ticket and judge it against the world.
    ///
    /// **The ticket is consumed either way.** An obligation answered `Err` is
    /// still answered: the client is owed exactly one terminal, and a refusal
    /// is one. Leaving it in the ledger so it could be retried would be an
    /// obligation that outlives its own answer.
    pub(crate) fn redeem(
        &mut self,
        id: DesignationId,
        world: &dyn Liveness,
        now: Instant,
    ) -> Result<Redeemed, (Option<DesignationTicket>, Undelivered)> {
        let Some(ticket) = self.open.remove(&id) else {
            // Nothing owed under this id: already redeemed, expired, or the
            // connection was withdrawn. Not an error the caller can act on,
            // and deliberately not distinguished from a forged id -- ids are
            // never reused, so both mean "this names nothing".
            return Err((None, Undelivered::TimedOut));
        };
        if now >= ticket.deadline {
            return Err((Some(ticket), Undelivered::TimedOut));
        }
        // Grant first, then realm: the grant is the authority question and the
        // realm is the destination question, and a reader should see the
        // authority answered first.
        if !world.grant_is_live(ticket.grant, ticket.spent_once) {
            return Err((Some(ticket), Undelivered::AuthorityDied));
        }
        if !world.realm_is_the_same(&ticket.realm) {
            return Err((Some(ticket), Undelivered::RealmDied));
        }
        Ok(Redeemed { ticket })
    }

    /// Tickets whose deadline has passed, removed. Ascending by id.
    pub(crate) fn expire_due(&mut self, now: Instant) -> Vec<DesignationTicket> {
        let due: Vec<DesignationId> = self
            .open
            .iter()
            .filter(|(_, t)| now >= t.deadline)
            .map(|(id, _)| *id)
            .collect();
        due.into_iter()
            .filter_map(|id| self.open.remove(&id))
            .collect()
    }

    /// Every ticket of one connection, removed.
    ///
    /// The teardown path: a peer that disconnects is owed nothing, and an
    /// obligation naming a connection that is gone must not outlive it.
    pub(crate) fn withdraw_connection(
        &mut self,
        connection: ConnectionId,
    ) -> Vec<DesignationTicket> {
        let theirs: Vec<DesignationId> = self
            .open
            .iter()
            .filter(|(_, t)| t.connection == connection)
            .map(|(id, _)| *id)
            .collect();
        theirs
            .into_iter()
            .filter_map(|id| self.open.remove(&id))
            .collect()
    }

    /// Every ticket naming one realm, removed.
    ///
    /// Called when a realm dies, on `clipboard_slot.forget_realm`'s precedent
    /// three lines away in `session::close_realm`: an outstanding designation
    /// addressed to a realm that has died is abandoned rather than left to be
    /// redeemed against whatever next holds that id.
    pub(crate) fn forget_realm(&mut self, realm: &RealmId) -> Vec<DesignationTicket> {
        let theirs: Vec<DesignationId> = self
            .open
            .iter()
            .filter(|(_, t)| &t.realm == realm)
            .map(|(id, _)| *id)
            .collect();
        theirs
            .into_iter()
            .filter_map(|id| self.open.remove(&id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const IDENT_A: &str = "vitrin://local/principal/agent-a";
    const IDENT_B: &str = "vitrin://local/principal/agent-b";

    fn ident(s: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(s).expect("fixture identity parses")
    }

    fn grant(raw: u64) -> GrantId {
        GrantId::from_u64_for_test(raw)
    }

    /// A world where everything is alive. Tests narrow it per case, so a test
    /// that means "the grant died" says only that.
    struct World {
        dead_grants: Vec<GrantId>,
        /// Grants whose row reads `Spent`. Forgiven only for the ticket that
        /// spent it, which is the whole point of the `spent_by_this_ticket`
        /// parameter.
        spent_grants: Vec<GrantId>,
        gone_realms: Vec<RealmId>,
    }

    impl World {
        fn alive() -> Self {
            Self {
                dead_grants: Vec::new(),
                spent_grants: Vec::new(),
                gone_realms: Vec::new(),
            }
        }
    }

    impl Liveness for World {
        fn grant_is_live(&self, grant: GrantId, spent_by_this_ticket: bool) -> bool {
            if self.dead_grants.contains(&grant) {
                return false;
            }
            if self.spent_grants.contains(&grant) {
                // The production rule, in miniature: a `Spent` row is
                // forgiven exactly when THIS ticket is what spent it.
                return spent_by_this_ticket;
            }
            true
        }

        fn realm_is_the_same(&self, realm: &RealmId) -> bool {
            !self.gone_realms.contains(realm)
        }
    }

    struct Fixture {
        ledger: Ledger,
        connection: ConnectionId,
        now: Instant,
    }

    impl Fixture {
        fn new() -> Self {
            let mut registry = crate::petitions::PetitionRegistry::new(
                crate::petitions::ConsentPolicy::Interactive,
                crate::petitions::PetitionConfig::default(),
            );
            Self {
                ledger: Ledger::new(TimeoutPolicy::default()),
                connection: registry.register_connection(),
                now: Instant::now(),
            }
        }

        fn open_for(
            &mut self,
            who: &str,
            row: u64,
            realm: &str,
            spent_once: bool,
        ) -> DesignationId {
            self.ledger
                .open(
                    self.connection,
                    7,
                    9,
                    grant(row),
                    &ident(who),
                    &RealmId::new(realm),
                    AskedFor::File { write: false },
                    spent_once,
                    self.now + Duration::from_secs(30),
                )
                .expect("the fixture ledger admits this ask")
        }
    }

    /// **One terminal per ask, and a second redemption is unreachable.**
    ///
    /// The type carries most of this — `DesignationTicket` is neither `Copy`
    /// nor `Clone` and `redeem` consumes what `take` removed — so what is left
    /// to check at runtime is that the ledger really forgets. If this ever
    /// returns a second `Ok`, the client receives two terminals for one ask,
    /// which the IDL forbids and which for a designation means two
    /// descriptors from one approval.
    #[test]
    fn a_designation_can_be_redeemed_exactly_once() {
        let mut f = Fixture::new();
        let id = f.open_for(IDENT_A, 1, "realm-0", false);
        let world = World::alive();

        assert!(
            f.ledger.redeem(id, &world, f.now).is_ok(),
            "the first redemption must deliver"
        );
        let (ticket, why) = f
            .ledger
            .redeem(id, &world, f.now)
            .expect_err("a second redemption of one designation must not deliver");
        assert!(ticket.is_none(), "there is no ticket left to return");
        assert_eq!(why, Undelivered::TimedOut);
        assert_eq!(f.ledger.outstanding(), 0);
    }

    /// **A revoked grant delivers nothing.** The case this module is shaped
    /// around: a descriptor is authority the core cannot recall, so the answer
    /// is re-derived at the instant of delivery rather than trusted from
    /// admission.
    #[test]
    fn a_grant_that_died_while_the_human_browsed_delivers_nothing() {
        let mut f = Fixture::new();
        let id = f.open_for(IDENT_A, 1, "realm-0", false);
        let world = World {
            dead_grants: vec![grant(1)],
            ..World::alive()
        };
        let (ticket, why) = f
            .ledger
            .redeem(id, &world, f.now)
            .expect_err("a revoked grant must not deliver a descriptor");
        assert_eq!(why, Undelivered::AuthorityDied);
        assert!(
            ticket.is_some(),
            "the caller needs the ticket to voice the refusal to the right facet"
        );
        assert_eq!(
            f.ledger.outstanding(),
            0,
            "a refused obligation is still an answered one and must leave the ledger"
        );
    }

    /// **A `once` rung is forgiven only for the ticket that spent it.**
    ///
    /// `commit_use` marks a `once` row `Spent` at admission and `refusal_for`
    /// reports `Spent` as `Expired`, so a naive re-ask would refuse every
    /// single-use designation, always. The forgiveness that fixes it must be
    /// narrow: this test is the difference between a query that is correct and
    /// one that is correct-for-now.
    #[test]
    fn a_spent_row_is_forgiven_for_its_own_ticket_and_for_no_other() {
        let world = World {
            spent_grants: vec![grant(1)],
            ..World::alive()
        };

        // The ticket that spent it: delivered.
        let mut f = Fixture::new();
        let mine = f.open_for(IDENT_A, 1, "realm-0", true);
        assert!(
            f.ledger.redeem(mine, &world, f.now).is_ok(),
            "the ticket whose own admission spent the rung must still deliver, or no \
             single-use designation could ever be delivered at all"
        );

        // A ticket on the same row that did NOT spend it: refused.
        let mut f = Fixture::new();
        let theirs = f.open_for(IDENT_A, 1, "realm-0", false);
        let (_, why) = f
            .ledger
            .redeem(theirs, &world, f.now)
            .expect_err("a spent row must not be forgiven for a ticket that did not spend it");
        assert_eq!(why, Undelivered::AuthorityDied);
    }

    /// **A realm that died between the ask and the answer delivers nothing.**
    ///
    /// The hole none of this issue's candidate designs caught.
    /// `session::close_realm` touches no grant row, so a liveness query alone
    /// answers `Live` for a grant over a realm that is gone — and a *declared*
    /// realm keeps its id across a process death, so the id can name a new
    /// process after a relaunch. Without this check the human's editor crashes
    /// mid-pick, they restart it, they confirm, and the descriptor lands in a
    /// process they never approved.
    #[test]
    fn a_realm_that_died_between_the_ask_and_the_answer_delivers_nothing() {
        let mut f = Fixture::new();
        let id = f.open_for(IDENT_A, 1, "realm-0", false);
        let world = World {
            gone_realms: vec![RealmId::new("realm-0")],
            ..World::alive()
        };
        let (_, why) = f
            .ledger
            .redeem(id, &world, f.now)
            .expect_err("a designation must not be delivered into a realm that has died");
        assert_eq!(why, Undelivered::RealmDied);
    }

    /// The deadline is checked before authority, so an obligation nobody
    /// answered reads as `TimedOut` rather than as something about the grant.
    #[test]
    fn an_obligation_past_its_deadline_delivers_nothing() {
        let mut f = Fixture::new();
        let id = f.open_for(IDENT_A, 1, "realm-0", false);
        let (ticket, why) = f
            .ledger
            .redeem(id, &World::alive(), f.now + Duration::from_secs(31))
            .expect_err("an obligation past its deadline must not deliver");
        assert_eq!(why, Undelivered::TimedOut);
        assert!(ticket.is_some());
    }

    /// **One card at a time per principal** — the IDL's `refusal.busy`. Two
    /// stacked in front of one human is the consent-fatigue shape.
    #[test]
    fn one_principal_owes_at_most_one_terminal() {
        let mut f = Fixture::new();
        f.open_for(IDENT_A, 1, "realm-0", false);
        let again = f.ledger.open(
            f.connection,
            7,
            9,
            grant(2),
            &ident(IDENT_A),
            &RealmId::new("realm-0"),
            AskedFor::Dir,
            false,
            f.now + Duration::from_secs(30),
        );
        assert_eq!(again, Err(OpenRefusal::Busy));

        // A different principal is unaffected: `busy` is per principal, not a
        // session-wide lock.
        let other = f.ledger.open(
            f.connection,
            7,
            9,
            grant(3),
            &ident(IDENT_B),
            &RealmId::new("realm-0"),
            AskedFor::Dir,
            false,
            f.now + Duration::from_secs(30),
        );
        assert!(other.is_ok(), "busy is per principal, not per session");
    }

    /// The ledger cannot be grown without bound by peers who ask and never
    /// answer.
    #[test]
    fn the_ledger_has_a_resource_bound() {
        let mut f = Fixture::new();
        for i in 0..MAX_OUTSTANDING {
            let who = format!("vitrin://local/principal/agent-{i}");
            f.ledger
                .open(
                    f.connection,
                    7,
                    9,
                    grant(i as u64),
                    &ident(&who),
                    &RealmId::new("realm-0"),
                    AskedFor::Dir,
                    false,
                    f.now + Duration::from_secs(30),
                )
                .expect("under the bound");
        }
        let over = f.ledger.open(
            f.connection,
            7,
            9,
            grant(99),
            &ident("vitrin://local/principal/agent-over"),
            &RealmId::new("realm-0"),
            AskedFor::Dir,
            false,
            f.now + Duration::from_secs(30),
        );
        assert_eq!(over, Err(OpenRefusal::Full));
    }

    /// Ids are never reused, so a stale id names nothing rather than naming
    /// something new. The teardown paths take tickets out; the counter does
    /// not go back.
    #[test]
    fn ids_are_never_reused() {
        let mut f = Fixture::new();
        let first = f.open_for(IDENT_A, 1, "realm-0", false);
        assert_eq!(f.ledger.withdraw_connection(f.connection).len(), 1);
        let second = f.open_for(IDENT_A, 1, "realm-0", false);
        assert_ne!(
            first, second,
            "a reused id would let a stale redemption name a live obligation"
        );
    }

    /// Teardown: an obligation must not outlive the connection that asked, or
    /// the realm it addresses.
    #[test]
    fn teardown_takes_obligations_with_it() {
        let mut f = Fixture::new();
        f.open_for(IDENT_A, 1, "realm-0", false);
        f.open_for(IDENT_B, 2, "realm-1", false);
        assert_eq!(f.ledger.outstanding(), 2);

        // The realm's precedent is `clipboard_slot.forget_realm`, three lines
        // away in `session::close_realm`.
        let forgotten = f.ledger.forget_realm(&RealmId::new("realm-0"));
        assert_eq!(forgotten.len(), 1);
        assert_eq!(forgotten[0].realm().as_str(), "realm-0");
        assert_eq!(f.ledger.outstanding(), 1);

        assert_eq!(f.ledger.withdraw_connection(f.connection).len(), 1);
        assert_eq!(f.ledger.outstanding(), 0);
    }

    /// The sweep removes what is due and leaves what is not.
    #[test]
    fn expiry_removes_only_what_is_due() {
        let mut f = Fixture::new();
        let soon = f
            .ledger
            .open(
                f.connection,
                7,
                9,
                grant(1),
                &ident(IDENT_A),
                &RealmId::new("realm-0"),
                AskedFor::Dir,
                false,
                f.now + Duration::from_secs(5),
            )
            .expect("admitted");
        f.ledger
            .open(
                f.connection,
                7,
                9,
                grant(2),
                &ident(IDENT_B),
                &RealmId::new("realm-0"),
                AskedFor::Dir,
                false,
                f.now + Duration::from_secs(60),
            )
            .expect("admitted");

        let due = f.ledger.expire_due(f.now + Duration::from_secs(10));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id(), soon);
        assert_eq!(f.ledger.outstanding(), 1);
    }

    /// The timeout policy is carried, defaults to the fail-closed reading, and
    /// is the owner's to set.
    #[test]
    fn the_timeout_policy_defaults_to_spending() {
        assert_eq!(TimeoutPolicy::default(), TimeoutPolicy::Spends);
        assert_eq!(
            Ledger::new(TimeoutPolicy::default()).timeout_policy(),
            TimeoutPolicy::Spends
        );
        assert_eq!(
            Ledger::new(TimeoutPolicy::Restores).timeout_policy(),
            TimeoutPolicy::Restores
        );
    }
}
