// SPDX-License-Identifier: MPL-2.0
//! Pointer constraints: the core's half of `vitrin_shim_session.pointer_constraint`
//! and `pointer_constraint_state` (WS-E.4.2, issue #222).
//!
//! An app confined in a realm asks its shim to lock or confine the pointer; the
//! shim asks the core; the core decides, and keeps deciding. This module is the
//! record and the decision. It sends nothing and knows about no connection:
//! `crate::shim` parks the ask, `crate::session` drains it and delivers the
//! verdicts, exactly as the clipboard's `selection` answer is handled.
//!
//! # The one thing this module exists to make impossible
//!
//! While a constraint is **active** the core hides its own human cursor sprite
//! (D-032 decision 2). It has to: the core owns the sprite — `wp_cursor_shape_manager_v1`
//! is not served, so the app cannot hide it — and a locked pointer with a sprite
//! frozen mid-screen is a lie about where the pointer is.
//!
//! **Every path on which a constraint ends must restore that sprite.** On a
//! bare-metal session there is no host desktop drawing a second one, so a missed
//! un-hide is a human with no visible pointer on a display server they cannot
//! exit. That is worse than any other defect this change could cause, which is
//! why visibility is **derived, never stored**:
//!
//! * [`hides_human_sprite`] is a free function over live gates, evaluated once
//!   per composed frame. Nothing anywhere writes sprite visibility, so no path
//!   can strand the cursor by *omission* — only by a record that is both stuck
//!   *and* accompanied by a focused realm, an active output, no overlay, a
//!   committed surface and a pointer inside the region, all at the same instant.
//! * The deactivation paths therefore split in two. A few are **record
//!   removals** (the app withdrew, the realm died, the seat paused, the dead-man
//!   chord fired) and live in this table. Most are **transient** — a realm
//!   switch, a consent card, the lock cover, the core notice, a dead-man hold,
//!   an unmapped surface, a paused output — and need **no code at all**, because
//!   [`PresentationGates`] is recomputed from live state every frame.
//!
//! The transient half is the load-bearing half. Wayland's `persistent` lifetime
//! requires a lock to *reactivate* when its surface regains focus, so a stored
//! flag would need an un-hide on switch-away and a re-hide on switch-back: two
//! sites, one of which gets forgotten. A predicate gives both for free.
//!
//! # What is deliberately NOT a deactivation path
//!
//! **Grant revocation.** A pointer constraint is asked for by the confined app
//! over its shim connection and is derived from no grant row, so
//! `GrantTable::revoke_principal` neither reaches this table nor may be made to.
//! An edge from an agent's grant lifecycle to a human's cursor visibility is
//! exactly the wrong shape, and writing the "no" down is how it stays absent —
//! `revoking_every_grant_leaves_an_apps_pointer_constraint_alone` is the test.
//!
//! # Blast radius
//!
//! The sprite exists **only on bare metal**. `backend::winit::window_pixels`
//! takes `human_cursor: Option<(f64, f64)>` and every backend CI can run passes
//! `None`, because the host desktop draws the human's pointer over a nested
//! window. So the safety property is verifiable only on hardware, and every mode
//! that is cheap to test is structurally immune — the shape that lets a defect
//! ship green. `backend::winit`'s
//! `the_nested_backend_never_hands_window_pixels_a_human_cursor` pins the
//! immunity so it is a property rather than an accident.

use std::collections::BTreeMap;

use vitrin_protocol::generated::vitrin_shim_session::{
    PointerConstraintKind, PointerConstraintLifetime, PointerConstraintStatus,
};

use crate::grants::RealmId;

/// One shim's `pointer_constraint` request, decoded and parked.
///
/// The wire message with its object id resolved out: `crate::shim` has already
/// checked that `surface` names a live surface on that connection (an id this
/// connection never minted is fatal `invalid_object`, the razor's first
/// clause), so what reaches this module is well-formed by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PointerConstraintAsk {
    /// Shim-minted, strictly increasing within the connection, never reused —
    /// the mirror of `request_selection`'s core-minted serial, with the asking
    /// party reversed.
    pub serial: u32,
    /// The surface the constraint applies to; `None` iff `kind` is
    /// [`PointerConstraintKind::None`].
    pub surface: Option<u32>,
    pub kind: PointerConstraintKind,
    pub lifetime: PointerConstraintLifetime,
    pub region: Region,
}

/// A constraint's region, in **surface-local** pixels.
///
/// `0x0` means the whole surface — Wayland's null-region meaning, carried
/// without a nullable rectangle type on the wire.
///
/// **One rectangle where Wayland allows an arbitrary `wl_region`.** An app whose
/// confinement is genuinely non-rectangular gets its bounding box and is not
/// told; the IDL names that cost rather than leaving it to be discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// The whole-surface region: the wire's `0x0`.
    ///
    /// Test-only, and narrowly so: a production `Region` always comes off the
    /// wire, where `0x0` arrives as four decoded integers rather than as a
    /// named constant. It exists so the tests state which case they mean.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn whole_surface() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    fn is_whole_surface(&self) -> bool {
        self.width == 0 && self.height == 0
    }

    /// Whether a surface-local point lies in this region. Half-open on the far
    /// edges, exactly as [`super::inside`] is for the surface itself.
    fn contains(&self, sx: f64, sy: f64) -> bool {
        let x0 = f64::from(self.x);
        let y0 = f64::from(self.y);
        sx >= x0 && sy >= y0 && sx < x0 + f64::from(self.width) && sy < y0 + f64::from(self.height)
    }
}

/// One realm's recorded constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Record {
    serial: u32,
    kind: PointerConstraintKind,
    lifetime: PointerConstraintLifetime,
    region: Region,
    /// **What the core believes right now**, recomputed from live gates by
    /// [`PointerConstraintTable::reconcile`] once per dispatch round.
    ///
    /// Only ever [`PointerConstraintStatus::Inactive`] or
    /// [`PointerConstraintStatus::Active`]: the other three are terminal
    /// answers about a record that is being removed or was never made, so they
    /// leave nothing behind to hold a state.
    state: PointerConstraintStatus,
    /// The last state this record's shim was *told*, so the event is
    /// edge-reported: the core sends at most one `pointer_constraint_state` per
    /// transition and never coalesces two different states (conventions 6.2's
    /// coalescing carve-out is explicitly overridden for this message).
    reported: Option<PointerConstraintStatus>,
}

impl Record {
    /// The serial of the ask that made this record — what every verdict about
    /// it is addressed by.
    ///
    /// Test-only: production code never reads a serial off a record it did not
    /// just build a [`Verdict`] from, and the verdict carries it.
    #[cfg(test)]
    pub fn serial(&self) -> u32 {
        self.serial
    }
}

/// One `pointer_constraint_state` the core owes a shim.
///
/// Not sent from here: this module holds no connection. `session` delivers it,
/// which is the same division `crate::clipboard`'s parked answer follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub realm: RealmId,
    pub serial: u32,
    pub state: PointerConstraintStatus,
}

/// **The live facts every constraint decision is derived from**, gathered once
/// per frame (the sprite) or once per dispatch round (the app-facing state).
///
/// A struct rather than six parameters for the reason [`super::Tap`] is one:
/// the two readers build it from different sources — the bare-metal composite
/// from `DrmView`'s own fields, the reconciler from the `Presenter` and the
/// router — and a future field must not be silently passed by only one of them.
pub(crate) struct PresentationGates<'a> {
    /// The realm the output is bound to. A constraint in any other realm is
    /// inactive: the human is not looking at that app.
    pub focused: Option<&'a RealmId>,
    /// The backend-owned half: whether an overlay needs the compositor and
    /// whether the output is active at all. See [`OutputGates`].
    pub output: OutputGates,
    /// The focused realm's committed client surface size, if any. `None` — the
    /// shim has not committed, or the app unmapped — is inactive: there is no
    /// surface to constrain to.
    pub surface: Option<(u32, u32)>,
    /// The realm-view geometry the surface is placed inside, for the same
    /// [`super::surface_local`] mapping the router and the compositor use —
    /// reserved rows included, so the hit test agrees with the pixels.
    pub view: crate::view::ViewGeometry,
    /// **The human's own pointer**, in view coordinates, or `None` before their
    /// first motion in this realm.
    ///
    /// The human's, never the shared position: an agent's actuation must not be
    /// able to move a human's cursor visibility (`RealmSeat::human_pointer`).
    pub pointer: Option<(f64, f64)>,
}

/// The two gates only the backend that owns the output can answer.
///
/// Both default to "nothing in the way" for backends that composite no overlay
/// and own no output — headless — which is the honest answer there and costs
/// the dispatch round nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputGates {
    /// A consent card, the lock cover, a dead-man hold indicator or the core
    /// notice needs the compositor this frame
    /// (`backend::winit::overlay_needs_the_window`).
    ///
    /// **This is what keeps the trusted path answerable.** A constraint
    /// deactivates the instant a card goes up, so the pointer unfreezes and the
    /// sprite returns before `ConsentGrab`'s hit test is ever asked to work
    /// against a frozen pointer — a locked app cannot make a consent card
    /// unclickable.
    pub overlay_up: bool,
    /// The output can be presented to. Cleared while the seat is paused (a VT
    /// switch, a suspend), which is independently a record removal — see
    /// `session::suspend_physical_seat`.
    pub active: bool,
    /// The session powered its own panel down after `--blank-idle` seconds idle
    /// (WS-E.4.3, issue #223, `crate::backend::blank`).
    ///
    /// **A third field rather than a fold into `active`**, because the two are
    /// different facts with different consequences and only one of them is this
    /// core's own doing: a paused output is somebody else's VT, a dark output is
    /// this session's own decision that it can undo the instant the human
    /// touches anything.
    ///
    /// It deactivates a pointer constraint on exactly the same footing `!active`
    /// does, and for a sharper reason: a locked pointer that survived a blank is
    /// a constraint the human can neither *see* nor free — the sprite is not on
    /// screen to show them where it is stuck, and the app that took the lock is
    /// not on screen either.
    pub dark: bool,
}

impl Default for OutputGates {
    fn default() -> Self {
        Self {
            overlay_up: false,
            active: true,
            dark: false,
        }
    }
}

impl PresentationGates<'_> {
    /// What `realm`'s record is worth **right now**, from nothing but live
    /// state. The whole transient half of the deactivation list is this
    /// function's early returns, in order.
    fn live_state(&self, realm: &RealmId, record: &Record) -> PointerConstraintStatus {
        let inactive = PointerConstraintStatus::Inactive;
        // Path 3: the realm lost focus, or the output is bound elsewhere. No
        // call site anywhere un-hides on a switch, and none re-hides on a
        // switch back: both fall out of this line.
        if self.focused != Some(realm) {
            return inactive;
        }
        // Path 9: the seat was paused (VT switch, suspend). Also a record
        // removal, deliberately: the devices are closed for the whole pause,
        // so nothing could ever end the constraint from the input side.
        if !self.output.active {
            return inactive;
        }
        // Path 9b (WS-E.4.3, issue #223): the panel is powered down. Not a
        // record removal -- unlike a seat pause, this ends the moment the human
        // touches anything, and the wake is a physical event the input side can
        // and does see -- but a constraint that stayed active across it would be
        // a pointer the human can neither see nor free.
        if self.output.dark {
            return inactive;
        }
        // Paths 6, 7, 12, 13: a consent prompt, the lock screen, the core
        // notice, a dead-man hold in progress. One term each in
        // `overlay_needs_the_window`, and all four cost nothing here.
        if self.output.overlay_up {
            return inactive;
        }
        // Paths 2 and 14: no committed surface. There is nothing to constrain
        // to and no placement to map through.
        let Some(surface) = self.surface else {
            return inactive;
        };
        // Path 15: the pointer is not inside the region yet. Wayland's own
        // rule — a lock activates only when the pointer is inside — and the
        // ask is answered `inactive` rather than `refused`, so it becomes
        // active by itself when the pointer enters.
        //
        // The hit test is `surface_local` + `inside`, the SAME two functions
        // the router and the compositor use (conventions 1.4 invariant 2: the
        // core's own hit test decides). A second hit test written here would be
        // a second answer to that question.
        let Some(pointer) = self.pointer else {
            return inactive;
        };
        let (sx, sy) = super::surface_local(pointer, self.view, surface);
        if !super::inside(sx, sy, surface) {
            return inactive; // the matte is not the app
        }
        if !record.region.is_whole_surface() && !record.region.contains(sx, sy) {
            return inactive;
        }
        PointerConstraintStatus::Active
    }
}

/// **Does the core hide its own cursor sprite this frame?**
///
/// THE sprite chokepoint. There is exactly one call site in this crate — the
/// bare-metal backend's `compose_and_queue`, feeding the one `human_cursor`
/// local that both presentation branches read — and
/// `the_sprite_has_exactly_one_chokepoint` pins that.
///
/// **Two conjuncts, and the order they fail in is the point.** The record must
/// say `Active` (what the core believes and what the app was told, so the sprite
/// and the wire agree) **and** the live gates must still say `Active` this
/// frame. Hiding therefore needs both; un-hiding needs only one to drop, and the
/// live half is recomputed from scratch for every composed frame. A record stuck
/// `Active` by any bug whatsoever still cannot hide the pointer unless the realm
/// is focused, the output is active, no overlay is up, a surface is committed and
/// the human's pointer is inside the region — which is a session in which the
/// pointer really is locked.
#[cfg_attr(
    all(not(test), not(feature = "drm-backend")),
    allow(
        dead_code,
        reason = "the one production caller is the bare-metal composite, which is behind \
                  `drm-backend`. A default build compiles this and runs its tests with no DRM \
                  device -- which is exactly the asymmetry that makes those tests load-bearing, \
                  since no mode CI can run has a sprite to hide at all"
    )
)]
pub(crate) fn hides_human_sprite(
    table: &PointerConstraintTable,
    gates: &PresentationGates<'_>,
) -> bool {
    let Some(realm) = gates.focused else {
        return false;
    };
    let Some(record) = table.records.get(realm) else {
        return false;
    };
    record.state == PointerConstraintStatus::Active
        && gates.live_state(realm, record) == PointerConstraintStatus::Active
}

/// **One recorded constraint per realm**, and the reconciler that keeps them
/// honest.
///
/// A second per-realm map beside `InputRouter::seats`, which is a real risk: two
/// maps can drift. The mitigation is that the clearing lives *inside*
/// [`super::InputRouter::reset_for`] rather than beside its callers — the lesson
/// `forget_presence_of` already wrote down, where a caller's obligation survived
/// exactly one review cycle before the third caller (the realm death funnel) did
/// not know it existed.
#[derive(Debug, Default)]
pub(crate) struct PointerConstraintTable {
    records: BTreeMap<RealmId, Record>,
    /// Set by every mutation that can change what `hides_human_sprite`
    /// answers, drained once per dispatch round by `session::post_dispatch`.
    ///
    /// **The sprite is derived per composed frame, so a deactivation that asks
    /// for no frame leaves the frame composed with the sprite HIDDEN on the
    /// human's screen** until something unrelated happens to want one. The
    /// predicate cannot be stranded by omission; the FRAME can. That is not a
    /// hypothetical: it was found by driving each deactivation path and
    /// watching the present counter stay at zero, after the derived design had
    /// already been reviewed and called sound.
    repaint_owed: bool,
}

impl PointerConstraintTable {
    /// An empty table.
    ///
    /// `pub(super)`, deliberately: the only way anything outside
    /// `crate::input` can obtain one is [`super::InputRouter::constraints`], so
    /// a backend cannot end up composing against a table its own router does
    /// not write. The same discipline `InputRouter::presence` follows, for the
    /// same reason — a second table would be a sprite decided from state
    /// nothing updates.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// `realm`'s record, if it has one.
    ///
    /// Test-only, and genuinely so: no production path asks the table what it
    /// holds — it asks [`Self::is_active`], or it hands the table an event and
    /// takes back verdicts. A reader that could inspect a record would be a
    /// second place decisions get made from one.
    #[cfg(test)]
    pub fn get(&self, realm: &RealmId) -> Option<&Record> {
        self.records.get(realm)
    }

    /// **Is `realm`'s constraint in force?** — the freeze's one question.
    ///
    /// Reads the reconciled state rather than recomputing, so the freeze and the
    /// state the app was told can never disagree: an app that was told `active`
    /// gets deltas and no absolute motion, and an app that was told anything
    /// else gets the ordinary path.
    pub fn is_active(&self, realm: &RealmId) -> bool {
        self.records
            .get(realm)
            .is_some_and(|record| record.state == PointerConstraintStatus::Active)
    }

    /// How many records are held. Test-only, and genuinely so: no production
    /// path asks the table's size — every caller names a realm.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// **Record, replace or withdraw** one shim's ask, and hand back every
    /// verdict that ask settles immediately.
    ///
    /// Immediately, here, means the answers that are facts about the *ask*
    /// rather than about the world: a supersession, a refusal, a withdrawal.
    /// Whether a newly recorded constraint is in force is not one of those — it
    /// is derived, and [`Self::reconcile`] answers it on the same turn.
    ///
    /// One message is the whole state machine's input, so a withdrawal cannot
    /// race a set, and a second ask **replaces** the first rather than queueing
    /// beside it: the superseded serial is answered `superseded` and never
    /// again. Same discipline as the clipboard's parked answer, where a second
    /// answer before the drain replaces the first.
    pub fn ask(&mut self, realm: &RealmId, ask: PointerConstraintAsk) -> Vec<Verdict> {
        if ask.kind == PointerConstraintKind::None {
            // The withdrawal arm. `surface` and the region MUST be empty here,
            // and the core ignores them in that case rather than treating a
            // decorated withdrawal as data -- the discipline `selection`
            // already set for a message whose arms have different fields.
            return self.withdraw(realm).into_iter().collect();
        }
        // The one refusal this deployment makes: a constraining kind with no
        // surface names nothing to constrain to. Recoverable, never fatal --
        // the connection lives, the app's `zwp_locked_pointer_v1` stays inert,
        // and the shim must not re-ask on this serial. A version-1 connection
        // petitioning for `realm_launch` is answered `unsupported` rather than
        // killed by exactly this reasoning (conventions 7.3).
        //
        // The vocabulary is wider than the current policy on purpose: a
        // deployment that later declines constraints outright answers here.
        let Some(_surface) = ask.surface else {
            tracing::debug!(
                realm = %realm,
                serial = ask.serial,
                "pointer_constraint refused: a lock or confine names no surface"
            );
            return vec![Verdict {
                realm: realm.clone(),
                serial: ask.serial,
                state: PointerConstraintStatus::Refused,
            }];
        };
        let mut owed = Vec::new();
        let record = Record {
            serial: ask.serial,
            kind: ask.kind,
            lifetime: ask.lifetime,
            region: ask.region,
            state: PointerConstraintStatus::Inactive,
            reported: None,
        };
        self.repaint_owed = true;
        if let Some(superseded) = self.records.insert(realm.clone(), record) {
            owed.push(Verdict {
                realm: realm.clone(),
                serial: superseded.serial,
                state: PointerConstraintStatus::Superseded,
            });
        }
        owed
    }

    /// **Remove `realm`'s record and say so** — path 1 (the app withdrew), path
    /// 9 (the seat paused) and path 8 (the dead-man chord).
    ///
    /// The verdict carries the **withdrawn record's** serial, not the
    /// withdrawing ask's: the shim maps a serial onto the constraint object it
    /// created, and naming the withdrawal's own serial would name an ask that
    /// recorded nothing. A withdrawal with no record answers nothing at all,
    /// because nothing changed.
    #[must_use = "the shim is owed this verdict; dropping it leaves an app believing it still \
                  holds a lock the core has already forgotten"]
    pub fn withdraw(&mut self, realm: &RealmId) -> Option<Verdict> {
        let record = self.records.remove(realm)?;
        self.repaint_owed = true;
        Some(Verdict {
            realm: realm.clone(),
            serial: record.serial,
            state: PointerConstraintStatus::Withdrawn,
        })
    }

    /// **Withdraw every constraint in the session** — the dead-man chord (path
    /// 8).
    ///
    /// Session-wide by construction, like the switch itself: a constraint set in
    /// a realm the chord was not held over goes too, because the chord is the
    /// human taking their machine back and a locked pointer in *any* realm is
    /// part of what they are taking it back from.
    #[must_use = "every shim is owed its verdict"]
    pub fn withdraw_all(&mut self) -> Vec<Verdict> {
        self.repaint_owed = true;
        std::mem::take(&mut self.records)
            .into_iter()
            .map(|(realm, record)| Verdict {
                realm,
                serial: record.serial,
                state: PointerConstraintStatus::Withdrawn,
            })
            .collect()
    }

    /// **Forget `realm`'s record with no verdict** — path 4, the realm or its
    /// shim died.
    ///
    /// Deliberately silent, unlike [`Self::withdraw`]: there is no connection
    /// left to tell. Called from [`super::InputRouter::reset_for`] and nowhere
    /// else, which covers both arms of the death funnel — the one that goes
    /// through `ShimServer::connection_closed` and the one for a realm that
    /// died before its shim came up — with a single edit.
    pub fn forget(&mut self, realm: &RealmId) -> bool {
        let had = self.records.remove(realm).is_some();
        if had {
            self.repaint_owed = true;
        }
        had
    }

    /// **Take the "a frame is owed" flag**, leaving it clear.
    ///
    /// Drained once per dispatch round. The sprite predicate is derived, so it
    /// can never answer stale; what it cannot do on its own is cause a frame in
    /// which to be answered. On a damage-driven backend nothing else in the
    /// round wants one after a constraint ends, so without this the human's
    /// screen keeps the last frame composed while the constraint was active —
    /// the one with no cursor in it.
    #[must_use = "a dropped repaint leaves the human's cursor missing until something \
                  unrelated asks for a frame"]
    pub fn take_repaint_owed(&mut self) -> bool {
        std::mem::take(&mut self.repaint_owed)
    }

    /// **Recompute every record from live state and report the transitions.**
    ///
    /// Level-triggered, edge-reported: the state is derived unconditionally,
    /// and an event goes out only when it changed. Consecutive identical states
    /// are not sent. That is the shape `Presenter::set_agent_cursor` already
    /// has — recompute always, act only on change — and it is the app-facing
    /// half of the same argument the sprite rests on: nothing *calls* this to
    /// announce a deactivation, so no call site can forget to.
    ///
    /// **A oneshot lifetime ends for good at its first deactivation.** The
    /// record is dropped rather than left inactive, so it can never reactivate,
    /// and the shim destroys its own object on the `withdrawn` this produces.
    /// That is Wayland's own `zwp_locked_pointer_v1` semantics carried rather
    /// than a policy invented here.
    #[must_use = "these are the verdicts the shims are owed"]
    pub fn reconcile(&mut self, gates: &PresentationGates<'_>) -> Vec<Verdict> {
        let mut owed = Vec::new();
        let mut oneshot_done = Vec::new();
        for (realm, record) in self.records.iter_mut() {
            let now = gates.live_state(realm, record);
            record.state = now;
            if record.reported == Some(now) {
                continue;
            }
            // A oneshot that has been active and is not any more is over: it
            // gets `withdrawn` rather than `inactive`, and the record goes.
            let ending_oneshot = record.lifetime == PointerConstraintLifetime::Oneshot
                && record.reported == Some(PointerConstraintStatus::Active)
                && now != PointerConstraintStatus::Active;
            let state = if ending_oneshot {
                oneshot_done.push(realm.clone());
                PointerConstraintStatus::Withdrawn
            } else {
                now
            };
            record.reported = Some(state);
            // A transition is exactly when `hides_human_sprite` may start
            // answering differently, so it is exactly when a frame is owed.
            self.repaint_owed = true;
            owed.push(Verdict {
                realm: realm.clone(),
                serial: record.serial,
                state,
            });
        }
        for realm in oneshot_done {
            self.records.remove(&realm);
        }
        owed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realm(name: &str) -> RealmId {
        RealmId::new(name)
    }

    fn lock_ask(serial: u32) -> PointerConstraintAsk {
        PointerConstraintAsk {
            serial,
            surface: Some(7),
            kind: PointerConstraintKind::Lock,
            lifetime: PointerConstraintLifetime::Persistent,
            region: Region::whole_surface(),
        }
    }

    /// Gates in which a constraint over `focused` is active: the surface fills
    /// the view and the pointer is in the middle of it.
    fn open_gates<'a>(focused: &'a RealmId) -> PresentationGates<'a> {
        PresentationGates {
            focused: Some(focused),
            output: OutputGates::default(),
            surface: Some((100, 100)),
            view: (100u32, 100u32).into(),
            pointer: Some((50.0, 50.0)),
        }
    }

    /// Drive a table to steady state under `gates` and hand back the last
    /// verdict batch, so a test can assert on the transition it cares about.
    fn settle(table: &mut PointerConstraintTable, gates: &PresentationGates<'_>) -> Vec<Verdict> {
        table.reconcile(gates)
    }

    #[test]
    fn a_recorded_lock_becomes_active_and_hides_the_sprite() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        assert!(
            table.ask(&r, lock_ask(1)).is_empty(),
            "a first ask supersedes nothing"
        );
        let gates = open_gates(&r);
        assert!(
            !hides_human_sprite(&table, &gates),
            "the sprite stays until the record has been reconciled AND the app told"
        );
        let owed = settle(&mut table, &gates);
        assert_eq!(
            owed,
            vec![Verdict {
                realm: r.clone(),
                serial: 1,
                state: PointerConstraintStatus::Active
            }]
        );
        assert!(hides_human_sprite(&table, &gates));
        assert!(table.is_active(&r));
        assert!(
            settle(&mut table, &gates).is_empty(),
            "consecutive identical states are never re-sent"
        );
    }

    /// Path 1: the app unsets it.
    #[test]
    fn a_withdrawal_removes_the_record_and_the_sprite_returns_on_the_next_frame() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&r, lock_ask(1));
        let gates = open_gates(&r);
        let _ = settle(&mut table, &gates);
        assert!(hides_human_sprite(&table, &gates));

        let owed = table.ask(
            &r,
            PointerConstraintAsk {
                serial: 2,
                surface: None,
                kind: PointerConstraintKind::None,
                lifetime: PointerConstraintLifetime::Oneshot,
                region: Region::whole_surface(),
            },
        );
        assert_eq!(
            owed,
            vec![Verdict {
                realm: r.clone(),
                // The WITHDRAWN record's serial, not the withdrawal's own.
                serial: 1,
                state: PointerConstraintStatus::Withdrawn
            }]
        );
        assert!(
            !hides_human_sprite(&table, &gates),
            "the sprite is back on the very next composed frame, with no other state changed"
        );
        assert!(!table.is_active(&r));
    }

    /// Path 2 / path 14: the surface the ask named stops being committed.
    #[test]
    fn a_constraint_over_an_uncommitted_surface_never_hides_the_sprite() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&r, lock_ask(1));
        let gates = PresentationGates {
            surface: None,
            ..open_gates(&r)
        };
        let owed = settle(&mut table, &gates);
        assert_eq!(owed[0].state, PointerConstraintStatus::Inactive);
        assert!(!hides_human_sprite(&table, &gates));
    }

    /// Path 3, and the round trip is the point: only a round trip catches a
    /// one-way flag.
    #[test]
    fn losing_focus_deactivates_and_regaining_it_reactivates() {
        let r = realm("a");
        let other = realm("b");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&r, lock_ask(1));
        let _ = settle(&mut table, &open_gates(&r));
        assert!(hides_human_sprite(&table, &open_gates(&r)));

        let elsewhere = PresentationGates {
            focused: Some(&other),
            ..open_gates(&r)
        };
        let owed = settle(&mut table, &elsewhere);
        assert_eq!(owed[0].state, PointerConstraintStatus::Inactive);
        assert!(!hides_human_sprite(&table, &elsewhere));

        let back = open_gates(&r);
        let owed = settle(&mut table, &back);
        assert_eq!(
            owed[0].state,
            PointerConstraintStatus::Active,
            "a persistent lifetime must reactivate with no new ask"
        );
        assert!(hides_human_sprite(&table, &back));
    }

    /// Paths 6, 7, 12 and 13 — **every overlay, named one at a time** — plus
    /// path 9's transient half.
    ///
    /// The four overlays reach this predicate as ONE boolean, because
    /// `backend::winit::overlay_needs_the_window` already folds them: a
    /// dead-man hold, a consent card, the lock cover and the core notice. That
    /// is exactly why none of them needs an un-hide call site of its own — but
    /// each is listed here by name anyway, because a deactivation path with no
    /// test is a path that will strand the cursor, and "they share a boolean"
    /// is an argument a reader has to be able to check rather than take.
    ///
    /// The wiring that makes the shared boolean true — that the bare-metal
    /// backend really passes the hold and the notice into
    /// `overlay_needs_the_window` — is pinned separately by
    /// `backend::drm::tests::the_constraint_gates_read_every_overlay` and by
    /// `backend::winit::tests::every_overlay_forces_the_cpu_path`.
    #[test]
    fn every_overlay_deactivates_a_pointer_constraint_and_restores_the_sprite() {
        for (label, output) in [
            (
                "path 13: a dead-man hold in progress -- the human gets their \
                 cursor back the moment they START asking for their machine back",
                OutputGates {
                    overlay_up: true,
                    active: true,
                    dark: false,
                },
            ),
            (
                "path 6: a consent card is up -- and this is what keeps the card \
                 clickable",
                OutputGates {
                    overlay_up: true,
                    active: true,
                    dark: false,
                },
            ),
            (
                "path 7: the lock cover is raised",
                OutputGates {
                    overlay_up: true,
                    active: true,
                    dark: false,
                },
            ),
            (
                "path 12: the core notice is up -- a trapped human is the one \
                 most likely to be looking for a pointer that is not there",
                OutputGates {
                    overlay_up: true,
                    active: true,
                    dark: false,
                },
            ),
            (
                "path 9 (transient half): a paused output -- VT switch, suspend",
                OutputGates {
                    overlay_up: false,
                    active: false,
                    dark: false,
                },
            ),
            (
                "path 9b (WS-E.4.3): the panel is powered down after --blank-idle \
                 -- a constraint that survived a blank is a pointer the human can \
                 neither see nor free",
                OutputGates {
                    overlay_up: false,
                    active: true,
                    dark: true,
                },
            ),
        ] {
            let r = realm("a");
            let mut table = PointerConstraintTable::new();
            let _ = table.ask(&r, lock_ask(1));
            let _ = settle(&mut table, &open_gates(&r));
            assert!(hides_human_sprite(&table, &open_gates(&r)));

            let gates = PresentationGates {
                output,
                ..open_gates(&r)
            };
            let owed = settle(&mut table, &gates);
            assert_eq!(owed[0].state, PointerConstraintStatus::Inactive, "{label}");
            assert!(
                !hides_human_sprite(&table, &gates),
                "{label}: the human's pointer must be back"
            );
        }
    }

    /// Path 15: asked for while the pointer is on the letterbox matte.
    #[test]
    fn a_lock_asked_for_while_the_pointer_is_on_the_matte_is_answered_inactive_and_becomes_active_when_it_enters(
    ) {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&r, lock_ask(1));
        // A 100x100 surface centered in a 200x200 view: the matte is
        // everything outside [50,150).
        let on_matte = PresentationGates {
            surface: Some((100, 100)),
            view: (200u32, 200u32).into(),
            pointer: Some((10.0, 10.0)),
            ..open_gates(&r)
        };
        let owed = settle(&mut table, &on_matte);
        assert_eq!(
            owed[0].state,
            PointerConstraintStatus::Inactive,
            "inactive, NOT refused: Wayland activates a lock only inside the region"
        );
        assert!(!hides_human_sprite(&table, &on_matte));

        let entered = PresentationGates {
            pointer: Some((100.0, 100.0)),
            ..on_matte
        };
        let owed = settle(&mut table, &entered);
        assert_eq!(owed[0].state, PointerConstraintStatus::Active);
        assert!(hides_human_sprite(&table, &entered));
    }

    /// A confinement region smaller than the surface gates on the region, not
    /// merely on the surface.
    #[test]
    fn a_region_smaller_than_the_surface_gates_on_the_region() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(
            &r,
            PointerConstraintAsk {
                kind: PointerConstraintKind::Confine,
                region: Region {
                    x: 10,
                    y: 10,
                    width: 20,
                    height: 20,
                },
                ..lock_ask(1)
            },
        );
        let outside = PresentationGates {
            pointer: Some((80.0, 80.0)),
            ..open_gates(&r)
        };
        let _ = settle(&mut table, &outside);
        assert!(!hides_human_sprite(&table, &outside));

        let inside = PresentationGates {
            pointer: Some((20.0, 20.0)),
            ..open_gates(&r)
        };
        let _ = settle(&mut table, &inside);
        assert!(hides_human_sprite(&table, &inside));
    }

    /// Path 17: a second ask supersedes the first, once.
    #[test]
    fn a_second_ask_supersedes_the_first_and_the_first_serial_is_answered_once() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&r, lock_ask(1));
        let owed = table.ask(&r, lock_ask(2));
        assert_eq!(
            owed,
            vec![Verdict {
                realm: r.clone(),
                serial: 1,
                state: PointerConstraintStatus::Superseded
            }]
        );
        let gates = open_gates(&r);
        let owed = settle(&mut table, &gates);
        assert_eq!(owed.len(), 1, "only the surviving record is reported");
        assert_eq!(owed[0].serial, 2);
        // ...and serial 1 is never mentioned again, on any later turn.
        let owed = settle(&mut table, &gates);
        assert!(owed.is_empty());
    }

    /// A refusal leaves nothing recorded and nothing hidden — the app's object
    /// stays inert, which is a legal Wayland state.
    #[test]
    fn a_refused_constraint_records_nothing_and_hides_nothing() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let owed = table.ask(
            &r,
            PointerConstraintAsk {
                surface: None,
                ..lock_ask(9)
            },
        );
        assert_eq!(
            owed,
            vec![Verdict {
                realm: r.clone(),
                serial: 9,
                state: PointerConstraintStatus::Refused
            }]
        );
        assert!(table.get(&r).is_none());
        let gates = open_gates(&r);
        assert!(settle(&mut table, &gates).is_empty());
        assert!(!hides_human_sprite(&table, &gates));
    }

    /// Path 4: forgetting one realm leaves a sibling's alone.
    #[test]
    fn forgetting_one_realm_leaves_a_siblings_constraint_alone() {
        let a = realm("a");
        let b = realm("b");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&a, lock_ask(1));
        let _ = table.ask(&b, lock_ask(2));
        assert!(table.forget(&a));
        assert!(!table.forget(&a), "forgetting twice is a no-op");
        assert!(table.get(&a).is_none());
        assert_eq!(table.get(&b).map(Record::serial), Some(2));
        assert_eq!(table.len(), 1);
    }

    /// Path 8: session-wide.
    #[test]
    fn withdraw_all_takes_every_realm_and_names_every_serial() {
        let a = realm("a");
        let b = realm("b");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(&a, lock_ask(1));
        let _ = table.ask(&b, lock_ask(2));
        let mut owed = table.withdraw_all();
        owed.sort_by_key(|v| v.serial);
        assert_eq!(
            owed,
            vec![
                Verdict {
                    realm: a,
                    serial: 1,
                    state: PointerConstraintStatus::Withdrawn
                },
                Verdict {
                    realm: b,
                    serial: 2,
                    state: PointerConstraintStatus::Withdrawn
                },
            ]
        );
        assert_eq!(table.len(), 0);
    }

    /// A `oneshot` lifetime ends for good at its first deactivation, and the
    /// event that says so is `withdrawn` rather than `inactive` — so the shim
    /// destroys its own object rather than waiting for a reactivation that can
    /// never come.
    #[test]
    fn a_oneshot_constraint_ends_for_good_at_its_first_deactivation() {
        let r = realm("a");
        let other = realm("b");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(
            &r,
            PointerConstraintAsk {
                lifetime: PointerConstraintLifetime::Oneshot,
                ..lock_ask(1)
            },
        );
        let _ = settle(&mut table, &open_gates(&r));
        assert!(table.is_active(&r));

        let elsewhere = PresentationGates {
            focused: Some(&other),
            ..open_gates(&r)
        };
        let owed = settle(&mut table, &elsewhere);
        assert_eq!(owed[0].state, PointerConstraintStatus::Withdrawn);
        assert!(table.get(&r).is_none());

        // ...and coming back does NOT reactivate it.
        let back = open_gates(&r);
        assert!(settle(&mut table, &back).is_empty());
        assert!(!hides_human_sprite(&table, &back));
    }

    /// A oneshot that never activated is not ended by an ordinary `inactive`:
    /// the app is still waiting to be let in.
    #[test]
    fn a_oneshot_that_never_activated_is_not_ended_by_an_inactive() {
        let r = realm("a");
        let mut table = PointerConstraintTable::new();
        let _ = table.ask(
            &r,
            PointerConstraintAsk {
                lifetime: PointerConstraintLifetime::Oneshot,
                ..lock_ask(1)
            },
        );
        let on_matte = PresentationGates {
            surface: Some((100, 100)),
            view: (200u32, 200u32).into(),
            pointer: Some((10.0, 10.0)),
            ..open_gates(&r)
        };
        let owed = settle(&mut table, &on_matte);
        assert_eq!(owed[0].state, PointerConstraintStatus::Inactive);
        assert!(table.get(&r).is_some());
    }

    /// The sprite predicate and the app-facing state are computed from ONE
    /// function, so they cannot disagree about the same frame.
    #[test]
    fn the_sprite_and_the_wire_never_disagree() {
        let r = realm("a");
        let other = realm("b");
        for pointer in [None, Some((50.0, 50.0)), Some((5.0, 5.0))] {
            for overlay_up in [false, true] {
                for active in [false, true] {
                    for surface in [None, Some((100, 100))] {
                        for focused in [Some(&r), Some(&other), None] {
                            let mut table = PointerConstraintTable::new();
                            let _ = table.ask(&r, lock_ask(1));
                            let gates = PresentationGates {
                                focused,
                                output: OutputGates {
                                    overlay_up,
                                    active,
                                    dark: false,
                                },
                                surface,
                                view: (200u32, 200u32).into(),
                                pointer,
                            };
                            let _ = table.reconcile(&gates);
                            assert_eq!(
                                hides_human_sprite(&table, &gates),
                                table.is_active(&r) && focused == Some(&r),
                                "sprite and wire disagree at \
                                 focused={focused:?} overlay={overlay_up} active={active} \
                                 surface={surface:?} pointer={pointer:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
