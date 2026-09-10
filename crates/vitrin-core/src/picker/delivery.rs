// SPDX-License-Identifier: MPL-2.0
//! Descriptors the core has opened and not yet handed over.
//!
//! # Why a descriptor is ever held at all
//!
//! [`crate::designation::Ledger`] holds obligations that carry **no
//! descriptor**, deliberately: a ticket lives across seconds of human time, and
//! nothing that long-lived may pin a file open. This table is the other side of
//! that decision and it is short-lived by construction.
//!
//! Between the two sits one unavoidable gap. The redemption judgement and the
//! `openat2` happen together, in the turn the human confirmed — but `reply`,
//! the only fd-capable write, needs a dispatch turn on the *agent's*
//! connection, and that turn is asked for rather than taken
//! ([`vitrin_ipc::Outbox::wake`]). So a descriptor exists for the few
//! milliseconds between "opened" and "written", and this table is what owns it
//! for exactly that long.
//!
//! # Everything here is about closing that gap safely
//!
//! * [`DELIVERY_DEADLINE`] bounds it. A woken turn that never arrives — the
//!   peer stopped reading, or died between the wake and the dispatch — must not
//!   leave a file pinned open for the rest of the session.
//! * A connection going away releases its held descriptors immediately, rather
//!   than waiting for that deadline. `Outbox` is `Clone` and outlives its
//!   source, so `wake()` on a dead connection succeeds and no turn ever comes;
//!   `vitrin-ipc`'s own docs now say so, and this is the half that acts on it.
//! * [`InFlight`] is `#[must_use]` and holds an `OwnedFd`, so a path that drops
//!   one closes the descriptor rather than leaking it, and a path that ignores
//!   one is a compiler warning.
//!
//! # The agent's half goes first, and its failure stops the shim's
//!
//! The IDL states as normative fact that "the agent's own answer was already
//! delivered on its own connection before this event was sent". So the two
//! halves are ordered, and the ordering is load-bearing rather than tidy: if
//! the agent's copy fails to go out, the shim's is **not attempted** and the
//! descriptor is closed, because sending it would make the IDL's premise false
//! on the wire and hand a realm a descriptor its agent never received.
//!
//! # One open, two receivers, and a shared offset
//!
//! There is exactly one `openat2`. The same descriptor is sent twice, and
//! `SCM_RIGHTS` installs a descriptor referring to the same open file
//! description in each receiver — which is what `dup` produces, so no explicit
//! `dup(2)` appears here. Two opens would be two race windows and could yield
//! two different inodes, which would make the single `(st_dev, st_ino)` pair
//! the journal records a claim about only one of them.
//!
//! The consequence is that the agent's copy and the shim's **share a file
//! offset**: a read by one moves the other's cursor. That is inherent to the
//! decision rather than a defect of it, and it is stated in the powerbox prose
//! so a client meets it as a documented property.

#![allow(dead_code)]

use crate::designation::{AskedFor, DesignationId};
use crate::grants::GrantId;
use crate::grants::RealmId;
use crate::identity::PrincipalIdentity;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

/// How long a descriptor may sit here waiting for the turn it needs.
///
/// Two seconds is two of the runtime's one-second sweeps, so a held descriptor
/// is seen by at least one sweep before it expires and cannot slip between
/// them. It bounds a *machine* interval — the time from a wake to the dispatch
/// it asks for — and so has nothing to do with `PICKER_DEADLINE`, which bounds
/// a human's.
pub(crate) const DELIVERY_DEADLINE: Duration = Duration::from_secs(2);

/// Which receiver still owes a copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Half {
    Owed,
    Sent,
    Failed,
}

/// A descriptor opened for a designation and not yet fully handed over.
#[must_use = "an InFlight that is dropped closes a descriptor nobody received; \
              route it through the funnel so the failure is journalled"]
#[derive(Debug)]
pub(crate) struct InFlight {
    pub(crate) id: DesignationId,
    /// The one descriptor. Sent to both receivers; never opened twice.
    pub(crate) fd: OwnedFd,
    /// `fstat`'d off `fd` once, at open. What the journal records and what the
    /// path-race gate compares against the row the picker displayed.
    pub(crate) dev_ino: (u64, u64),
    pub(crate) ask: AskedFor,
    pub(crate) grant: GrantId,
    pub(crate) principal: PrincipalIdentity,
    /// The connection the ask arrived on — the one the agent's copy is
    /// written to, and the one a journal line names. Carried rather than
    /// looked up from the principal, because a principal may hold more than
    /// one connection and only the one that asked is owed a terminal.
    pub(crate) connection: crate::petitions::ConnectionId,
    pub(crate) realm: RealmId,
    /// The basename the human chose, **display only**, for both terminals'
    /// `name` argument. Never a path, and never used to resolve anything: the
    /// descriptor is already open, and the only thing this string can do is
    /// appear in an app's title bar.
    pub(crate) name: Vec<u8>,
    pub(crate) facet_id: u32,
    pub(crate) grant_wire_id: u32,
    pub(crate) agent: Half,
    pub(crate) shim: Half,
    pub(crate) deadline: Instant,
}

impl InFlight {
    /// Nothing more is owed: both receivers have an answer.
    pub(crate) fn settled(&self) -> bool {
        self.agent != Half::Owed && self.shim != Half::Owed
    }
}

/// Why a held descriptor was released without reaching both receivers.
///
/// Each is a fact about the world, and each must reach the journal: a
/// descriptor that was opened and then closed is a designation the human made
/// and the agent never received, which is precisely the kind of silence this
/// repository is written to avoid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Abandoned {
    /// The turn asked for never arrived inside [`DELIVERY_DEADLINE`].
    NoTurn,
    /// The agent's connection went away before its copy was written.
    ConnectionGone,
    /// The realm died before its copy was written.
    RealmDied,
    /// A write failed. The descriptor is closed rather than half-delivered.
    SendFailed,
}

/// Descriptors held between opening and delivery.
#[derive(Debug, Default)]
pub(crate) struct DeliveryTable {
    held: BTreeMap<DesignationId, InFlight>,
}

impl DeliveryTable {
    /// Take ownership of a freshly opened descriptor.
    pub(crate) fn park(&mut self, entry: InFlight) {
        // Ids are never reused, so a collision here would mean the ledger
        // handed out one twice — a core bug, and one that would otherwise
        // silently drop a descriptor by overwriting it.
        debug_assert!(
            !self.held.contains_key(&entry.id),
            "a designation id was parked twice"
        );
        self.held.insert(entry.id, entry);
    }

    pub(crate) fn get_mut(&mut self, id: DesignationId) -> Option<&mut InFlight> {
        self.held.get_mut(&id)
    }

    /// Remove an entry whatever it still owes.
    ///
    /// The abandon path's counterpart to [`Self::take_settled`], and named to
    /// be read beside it: this one is for a delivery the caller has decided is
    /// over — a write failed, a wake produced no turn inside its own round —
    /// and the caller owes the journal a line for it, because a descriptor
    /// removed and dropped without one is exactly the silence this module's
    /// docs forbid.
    pub(crate) fn take_any(&mut self, id: DesignationId) -> Option<InFlight> {
        self.held.remove(&id)
    }

    /// Remove an entry that has nothing left to owe.
    pub(crate) fn take_settled(&mut self, id: DesignationId) -> Option<InFlight> {
        if self.held.get(&id).is_some_and(InFlight::settled) {
            self.held.remove(&id)
        } else {
            None
        }
    }

    pub(crate) fn outstanding(&self) -> usize {
        self.held.len()
    }

    /// The designation this connection is owed a copy of, if any.
    ///
    /// Asked on a `Woken` turn, which carries nothing at all: the wake is an
    /// *occasion to write*, not a message, so the core has to look up what it
    /// woke itself for. At most one per connection, by the ledger's
    /// one-outstanding-ask-per-principal rule; the first by id if that rule
    /// were ever relaxed, so this is deterministic either way.
    pub(crate) fn owed_by_connection(
        &self,
        connection: crate::petitions::ConnectionId,
    ) -> Option<DesignationId> {
        self.held
            .values()
            .find(|e| e.connection == connection && e.agent == Half::Owed)
            .map(|e| e.id)
    }

    /// The designation this realm is owed a copy of, if any.
    ///
    /// Only ever answers for an entry whose **agent half already went out**,
    /// which is this table's ordering rule expressed as a query rather than
    /// as a rule somebody has to remember: a shim woken for an entry the
    /// agent has not received gets nothing, so the IDL's premise that the
    /// agent was served first cannot be broken by a stray wake.
    pub(crate) fn owed_by_realm(&self, realm: &RealmId) -> Option<DesignationId> {
        self.held
            .values()
            .find(|e| &e.realm == realm && e.agent == Half::Sent && e.shim == Half::Owed)
            .map(|e| e.id)
    }

    /// Entries whose turn never came. Ascending by id, so a caller journals
    /// them in a stable order.
    pub(crate) fn expire_due(&mut self, now: Instant) -> Vec<(InFlight, Abandoned)> {
        let due: Vec<DesignationId> = self
            .held
            .iter()
            .filter(|(_, e)| now >= e.deadline)
            .map(|(id, _)| *id)
            .collect();
        due.into_iter()
            .filter_map(|id| self.held.remove(&id).map(|e| (e, Abandoned::NoTurn)))
            .collect()
    }

    /// Release everything owed to a connection that has gone away.
    ///
    /// Called on `Disconnected` and on `Fault`, not left to the deadline: a
    /// wake on a dead connection succeeds and produces no turn, so waiting
    /// would pin the file for the full [`DELIVERY_DEADLINE`] every time an
    /// agent dies mid-designation.
    pub(crate) fn withdraw_connection(
        &mut self,
        principal: &PrincipalIdentity,
    ) -> Vec<(InFlight, Abandoned)> {
        let gone: Vec<DesignationId> = self
            .held
            .iter()
            .filter(|(_, e)| &e.principal == principal)
            .map(|(id, _)| *id)
            .collect();
        gone.into_iter()
            .filter_map(|id| {
                self.held
                    .remove(&id)
                    .map(|e| (e, Abandoned::ConnectionGone))
            })
            .collect()
    }

    /// Release everything owed to a connection that has gone away, by its
    /// connection id.
    ///
    /// [`Self::withdraw_connection`]'s sibling, and the one the runtime
    /// actually calls: teardown knows the `ConnectionId` it is closing, and a
    /// principal may hold more than one connection — so withdrawing by
    /// *identity* would release a descriptor owed to a sibling connection
    /// that is still perfectly alive.
    pub(crate) fn withdraw_connection_by_id(
        &mut self,
        connection: crate::petitions::ConnectionId,
    ) -> Vec<(InFlight, Abandoned)> {
        let gone: Vec<DesignationId> = self
            .held
            .iter()
            .filter(|(_, e)| e.connection == connection)
            .map(|(id, _)| *id)
            .collect();
        gone.into_iter()
            .filter_map(|id| {
                self.held
                    .remove(&id)
                    .map(|e| (e, Abandoned::ConnectionGone))
            })
            .collect()
    }

    /// Release everything owed into a realm that has died.
    pub(crate) fn forget_realm(&mut self, realm: &RealmId) -> Vec<(InFlight, Abandoned)> {
        let gone: Vec<DesignationId> = self
            .held
            .iter()
            .filter(|(_, e)| &e.realm == realm)
            .map(|(id, _)| *id)
            .collect();
        gone.into_iter()
            .filter_map(|id| self.held.remove(&id).map(|e| (e, Abandoned::RealmDied)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn a_descriptor() -> OwnedFd {
        // Any real descriptor: the table's job is ownership, not what is open.
        rustix::fs::open(
            "/dev/null",
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .expect("/dev/null opens")
    }

    /// A fixture identity, on `designation.rs`'s precedent — the same shape
    /// its own tests use, so the two modules cannot drift on what a principal
    /// looks like.
    fn ident(who: &str) -> PrincipalIdentity {
        PrincipalIdentity::parse(&format!("vitrin://local/principal/{who}"))
            .expect("fixture identity parses")
    }

    fn entry(id: u32, who: &str, realm: &str, at: Instant) -> InFlight {
        InFlight {
            id: DesignationId::from_u32_for_test(id),
            fd: a_descriptor(),
            dev_ino: (1, u64::from(id)),
            ask: AskedFor::File { write: false },
            grant: GrantId::from_u64_for_test(u64::from(id)),
            principal: ident(who),
            connection: crate::petitions::PetitionRegistry::new(
                crate::petitions::ConsentPolicy::Interactive,
                crate::petitions::PetitionConfig::default(),
            )
            .register_connection(),
            realm: RealmId::new(realm),
            name: b"fixture.txt".to_vec(),
            facet_id: 7,
            grant_wire_id: 9,
            agent: Half::Owed,
            shim: Half::Owed,
            deadline: at + DELIVERY_DEADLINE,
        }
    }

    /// **A descriptor held here is closed when it is released**, not leaked.
    ///
    /// Asserted against the process's own descriptor table rather than against
    /// the type: `InFlight` owning an `OwnedFd` is what makes it true, and a
    /// refactor that replaced it with a raw fd would keep every other test
    /// green.
    #[test]
    fn releasing_an_entry_closes_its_descriptor() {
        let now = Instant::now();
        let mut table = DeliveryTable::default();
        let e = entry(1, "agent-a", "realm-0", now);
        let raw = e.fd.as_raw_fd();
        table.park(e);

        // Still open while held.
        assert!(
            rustix::fs::fstat(unsafe { std::os::fd::BorrowedFd::borrow_raw(raw) }).is_ok(),
            "the descriptor must stay open while the table holds it"
        );

        let released = table.expire_due(now + DELIVERY_DEADLINE);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].1, Abandoned::NoTurn);
        drop(released);

        assert!(
            rustix::fs::fstat(unsafe { std::os::fd::BorrowedFd::borrow_raw(raw) }).is_err(),
            "once released and dropped the descriptor must be closed: a picker \
             that leaked one would pin a file for the life of the session"
        );
    }

    /// A dead connection releases its descriptors at once rather than waiting
    /// out the deadline — the wake it would need can never produce a turn.
    #[test]
    fn a_dead_connection_releases_immediately() {
        let now = Instant::now();
        let mut table = DeliveryTable::default();
        table.park(entry(1, "agent-a", "realm-0", now));
        table.park(entry(2, "agent-b", "realm-0", now));

        let released = table.withdraw_connection(&ident("agent-a"));
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].1, Abandoned::ConnectionGone);
        assert_eq!(
            table.outstanding(),
            1,
            "another principal's obligation is untouched"
        );
    }

    #[test]
    fn a_dead_realm_releases_what_was_owed_into_it() {
        let now = Instant::now();
        let mut table = DeliveryTable::default();
        table.park(entry(1, "agent-a", "realm-0", now));
        table.park(entry(2, "agent-a", "realm-1", now));

        let released = table.forget_realm(&RealmId::new("realm-0"));
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].1, Abandoned::RealmDied);
        assert_eq!(table.outstanding(), 1);
    }

    /// An entry is only removed once nothing is owed, so a half-delivered
    /// designation cannot be dropped as if it were finished.
    #[test]
    fn a_half_delivered_entry_is_not_taken_as_settled() {
        let now = Instant::now();
        let mut table = DeliveryTable::default();
        table.park(entry(1, "agent-a", "realm-0", now));
        let id = DesignationId::from_u32_for_test(1);

        table.get_mut(id).unwrap().agent = Half::Sent;
        assert!(
            table.take_settled(id).is_none(),
            "the shim still owes a copy, so this entry is not finished"
        );

        table.get_mut(id).unwrap().shim = Half::Sent;
        assert!(table.take_settled(id).is_some());
        assert_eq!(table.outstanding(), 0);
    }

    /// A descriptor is not held past its deadline, and one that is still
    /// within it is left alone.
    #[test]
    fn expiry_is_bounded_and_does_not_take_the_living() {
        let now = Instant::now();
        let mut table = DeliveryTable::default();
        table.park(entry(1, "agent-a", "realm-0", now));

        assert!(
            table
                .expire_due(now + DELIVERY_DEADLINE - Duration::from_millis(1))
                .is_empty(),
            "an entry inside its deadline is still waiting for its turn"
        );
        assert_eq!(table.expire_due(now + DELIVERY_DEADLINE).len(), 1);
    }
}
