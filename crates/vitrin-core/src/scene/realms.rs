// SPDX-License-Identifier: MPL-2.0
//! **One scene per realm, exactly one bound to the output** (WS-E.1.3,
//! issue #209): the structure that turns "a capture reads the granted
//! realm's pixels" from an accident of there being one realm into a
//! property of the code.
//!
//! # Why this type exists, and why it is not a rendering nicety
//!
//! Until WS-E.1.2 a session held one realm, so one [`Scene`] holding one
//! committed surface was a complete model and every capture was
//! trivially "the realm's own". WS-E.1.2 raised [`crate::realm::MAX_REALMS`]
//! to 16 and left that model in place, which made a **confidentiality**
//! defect real rather than theoretical: with one scene, the last realm to
//! commit owns the only committed surface, so an `observe` grant over realm
//! A would serve realm B's pixels the instant B painted. Nothing in that
//! code was wrong — there was one realm — and nothing in it prevented the
//! leak either.
//!
//! So the selection is made **structural** here: a scene is reached only by
//! naming a realm ([`RealmScenes::scene_mut`], [`RealmScenes::scene`]), and
//! the output composites [`RealmScenes::bound`] — a *different* accessor,
//! spelled differently, so a capture path that reached for the output's
//! scene is a visible mistake rather than the default.
//!
//! # What this deliberately is not
//!
//! Not a window manager. There is **one** bound realm, no stacking, no
//! overlap, and no per-realm resize: every realm composes at the one
//! output's view size, through the same [`super::layout::place`] the single
//! realm always used (that module's own docs say it "must never grow", and
//! PRD §5.1 exiles window-management policy from the core permanently).
//! Everything here is a `BTreeMap`, a bound id and two counters.
//!
//! Nor does it choose *who* may bind: [`RealmScenes::bind`] and
//! [`RealmScenes::unbind`] are the mechanisms, and the authority question is
//! answered outside this module. WS-E.1.4 (issue #210) answered it: a
//! principal holding the `layout_focus` grant verb, through the
//! `vitrin_layout_focus` facet and the enforcement chokepoint, reaching here
//! via `session::apply_layout`. [`crate::session::physical_seat_target`]
//! follows the
//! binding, so the realm a human is looking at and the realm the human's
//! input reaches move as one act. Absent such a client the runtime's own
//! callers still take the first still-serving realm in id order, at first
//! attach and again when the realm holding the output dies.
//!
//! # Hidden realms still hold a scene, and still get composed
//!
//! A realm that is not bound keeps its [`Scene`], keeps receiving commits,
//! and keeps having its view composed for capture. That is decision 2 of
//! issue #209 and it is a correctness requirement, not generosity: a Wayland
//! client throttles on frame callbacks, so a realm that stopped being paced
//! would stop repainting and its capture would become a *stale frame* — the
//! one thing `vitrin_grant.refusal.no_surface` forbids in as many words.
//! Refusing capture on a hidden realm was the alternative and it is a lie:
//! the realm has a surface. The cost is published in
//! `docs/book/src/limits.md`.
//!
//! # `layout_generation` is not `Scene::generation`
//!
//! Each realm carries a [`RealmScene::layout_generation`] bumped on
//! bind/unbind and on a view-size change, separate from the content-only
//! [`Scene::generation`]. D-018(5) binds E2.3's epoch to (frame content +
//! view geometry + stacking), so both counters will be needed; folding
//! geometry into `Scene::generation` instead would make a *bind* look like a
//! repaint to the damage path (`Scene::commit_with_damage`, and the nested
//! backend's `TextureKey`), widening damage for the wrong reason. Nothing
//! reads it yet — see [`RealmScenes::layout_generation`] for why it is here
//! anyway.

use std::collections::BTreeMap;

use crate::grants::RealmId;

use super::Scene;

/// One realm's presentation state: its scene, and the geometry counter that
/// is deliberately not the scene's.
pub(crate) struct RealmScene {
    scene: Scene,
    /// Bumped when this realm is bound to the output, when it is unbound,
    /// and when the output's view size changes under it. **Never** bumped by
    /// a commit — that is [`Scene::generation`]'s job, and the split is
    /// D-018(5)'s (see the module docs).
    layout_generation: u64,
}

/// Every realm's scene, plus which one the output is bound to.
///
/// Owned by each backend's presentation half (`HeadlessView`, `NestedView`,
/// and `session`'s in-crate test host) so the three cannot drift about what
/// "the bound realm" means: they hold the same type and call the same
/// methods, which is the only defence D-019(3) leaves for two presentation
/// paths that must agree.
pub(crate) struct RealmScenes {
    scenes: BTreeMap<RealmId, RealmScene>,
    /// The realm the output composites. `None` before the first realm
    /// attaches, and again once no realm is left to show —
    /// `session::rebind_output_after_death` is the runtime's only caller of
    /// [`Self::unbind`], and it reaches for it only when no realm is still
    /// serving.
    bound: Option<RealmId>,
    /// The output's view size, as last reported by the backend. Held here
    /// only so a change can bump every realm's [`RealmScene::layout_generation`];
    /// nothing composes from it.
    view: (u32, u32),
    /// The scene [`Self::bound`] resolves to when no realm is bound (or the
    /// bound realm somehow has no entry).
    ///
    /// A real, permanently empty [`Scene`] rather than an `Option` return,
    /// because every composite path wants a `&Scene` and an empty scene
    /// already composes the documented deterministic background byte for
    /// byte. **Nothing ever hands out `&mut` to it**: [`Self::scene_mut`] is
    /// the only mutable accessor and it always names a realm, so a commit
    /// cannot land here by accident.
    vacant: Scene,
}

impl RealmScenes {
    /// An empty set at the output's view size: no realm, nothing bound.
    pub fn new(view: (u32, u32)) -> Self {
        Self {
            scenes: BTreeMap::new(),
            bound: None,
            view,
            vacant: Scene::new(),
        }
    }

    /// **This realm's scene**, minted empty on first use.
    ///
    /// The one mutable accessor, and it always names a realm: the shim
    /// server commits through it ([`crate::session::dispatch_shim`]) and the
    /// realm teardown funnel clears through it, so neither can reach a
    /// sibling's pixels.
    pub fn scene_mut(&mut self, realm: &RealmId) -> &mut Scene {
        &mut self
            .scenes
            .entry(realm.clone())
            .or_insert_with(|| RealmScene {
                scene: Scene::new(),
                layout_generation: 0,
            })
            .scene
    }

    /// This realm's scene, or `None` if it has never committed or been
    /// bound. `None` is not an error: a realm that has not been touched has
    /// no scene, and a capture over it meets the chokepoint's `no_surface`
    /// refusal, which is the honest answer.
    pub fn scene(&self, realm: &RealmId) -> Option<&Scene> {
        self.scenes.get(realm).map(|realm| &realm.scene)
    }

    /// **The scene the output composites** — the bound realm's, or a
    /// permanently empty one when nothing is bound.
    ///
    /// Named differently from [`Self::scene`] on purpose. Both return a
    /// `&Scene`, and the whole of issue #209's decision 1 is that a capture
    /// must never call this one.
    pub fn bound(&self) -> &Scene {
        self.bound
            .as_ref()
            .and_then(|id| self.scenes.get(id))
            .map(|realm| &realm.scene)
            .unwrap_or(&self.vacant)
    }

    /// The realm the output is bound to, if any.
    pub fn focused(&self) -> Option<&RealmId> {
        self.bound.as_ref()
    }

    /// Drain the **bound** realm's pending damage into `geom`'s view
    /// coordinates — [`Scene::take_damage_view`] for whichever realm the
    /// output shows.
    ///
    /// A named method rather than a `bound_mut()` accessor on purpose: the
    /// output's only mutable need on a scene is this one-shot presentation
    /// bookkeeping, and handing out `&mut` to the realm the *output* happens
    /// to show is exactly the shape of access decision 1 is about. `None`
    /// with nothing bound, which the callers already treat as "the whole view
    /// changed" — the conservative direction.
    pub fn take_bound_damage(
        &mut self,
        geom: crate::view::ViewGeometry,
    ) -> Option<super::DamageRect> {
        // Two disjoint field borrows, not a clone: this runs on the nested
        // backend's per-redraw path.
        let bound = self.bound.as_ref()?;
        self.scenes.get_mut(bound)?.scene.take_damage_view(geom)
    }

    /// Bind the output to `realm`, minting its scene if it has none.
    ///
    /// Bumps the outgoing realm's and the incoming realm's
    /// [`RealmScene::layout_generation`] — the "unbind" half is why that
    /// counter is per realm rather than one session-wide number: a realm's
    /// geometry changed when it *lost* the output as surely as when it
    /// gained it. Re-binding the realm already bound is a no-op and bumps
    /// nothing, so an idle round costs no counter churn.
    pub fn bind(&mut self, realm: &RealmId) {
        if self.bound.as_ref() == Some(realm) {
            return;
        }
        if let Some(previous) = self.bound.take() {
            if let Some(entry) = self.scenes.get_mut(&previous) {
                entry.layout_generation = entry.layout_generation.wrapping_add(1);
            }
        }
        // Mint before the bump so a realm bound before it ever committed
        // still has an entry to carry the counter.
        self.scene_mut(realm);
        if let Some(entry) = self.scenes.get_mut(realm) {
            entry.layout_generation = entry.layout_generation.wrapping_add(1);
        }
        self.bound = Some(realm.clone());
    }

    /// Bind the output to **no realm**: [`Self::bound`] becomes the
    /// permanently empty scene and [`Self::focused`] answers `None`.
    ///
    /// The counterpart [`Self::bind`] needs and did not have. `bind` was the
    /// only writer of the binding, so an output bound to a realm that then
    /// died stayed bound to it for the rest of the session — compositing the
    /// dead realm's cleared scene (the deterministic background) forever,
    /// while live siblings ran, and suppressing the agent-cursor sprite on the
    /// strength of a `focused()` that named a corpse. This is the way out of
    /// that state, not a focus policy: *who* may move the output is answered
    /// by the `layout_focus` verb (WS-E.1.4, issue #210), and the runtime
    /// reaches for this only when there is no realm left to move it to.
    ///
    /// Bumps the outgoing realm's `layout_generation` exactly as `bind` does —
    /// a realm's geometry changed when it lost the output, whether it lost it
    /// to a sibling or to nobody. A no-op when nothing is bound.
    pub fn unbind(&mut self) {
        let Some(previous) = self.bound.take() else {
            return;
        };
        if let Some(entry) = self.scenes.get_mut(&previous) {
            entry.layout_generation = entry.layout_generation.wrapping_add(1);
        }
    }

    /// Tell the set the output's view size. Bumps **every** realm's
    /// `layout_generation` when it actually changed, because every realm
    /// composes at the output's size — a resize moves every realm's
    /// geometry, bound or not.
    ///
    /// A no-op when the size is unchanged, which is the ordinary case: the
    /// headless backend's virtual output never resizes, and the nested
    /// backend calls this from its `Resized` handler.
    pub fn set_view_size(&mut self, view: (u32, u32)) {
        if self.view == view {
            return;
        }
        self.view = view;
        for entry in self.scenes.values_mut() {
            entry.layout_generation = entry.layout_generation.wrapping_add(1);
        }
    }

    /// This realm's layout generation, or `None` if it has no scene.
    ///
    /// **Nothing in the runtime reads this yet, and that is deliberate.**
    /// D-018(5) binds E2.3's semantic epoch to (frame content + view
    /// geometry + stacking); `Scene::generation` covers only the first.
    /// Landing the geometry counter now, beside the code that actually
    /// changes geometry, is the difference between E2.3 *folding* this in
    /// and E2.3 re-plumbing every bind site to find out when geometry moved.
    /// The risk is named in issue #209 in as many words: an unread counter
    /// rots, and if E2.3 defines its epoch differently this is dead code
    /// that looked load-bearing. The tests below are what keep it honest in
    /// the meantime.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn layout_generation(&self, realm: &RealmId) -> Option<u64> {
        self.scenes.get(realm).map(|realm| realm.layout_generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::tests::client_pixels;
    use crate::scene::SurfaceContent;
    use crate::test_pattern;

    fn realm(id: &str) -> RealmId {
        RealmId::new(id)
    }

    fn commit(scenes: &mut RealmScenes, id: &str, w: u32, h: u32) {
        scenes
            .scene_mut(&realm(id))
            .commit(SurfaceContent::from_rgba(client_pixels(w, h), w, h).unwrap());
    }

    /// Compare two composed views and, on a mismatch, report **where** they
    /// differ rather than dumping both.
    ///
    /// A bare `assert_eq!` on two view-sized buffers prints tens of thousands
    /// of bytes into the failure output, which is the same as printing
    /// nothing. The pixel index is what a reader needs.
    fn assert_same_view(got: &[u8], want: &[u8], what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: view sizes differ");
        if let Some(at) = got.iter().zip(want).position(|(a, b)| a != b) {
            let px = at / crate::scene::BYTES_PER_PIXEL;
            panic!(
                "{what}: views differ from pixel {px} ({}x{} into the buffer of {} pixels): \
                 got {:?}, want {:?}",
                px % 32,
                px / 32,
                want.len() / crate::scene::BYTES_PER_PIXEL,
                &got[at..(at + 4).min(got.len())],
                &want[at..(at + 4).min(want.len())],
            );
        }
    }

    /// **The leak, and the structure that closes it.** Two realms commit
    /// different fixtures; each realm's scene composes its own bytes, and
    /// the bound one is the only thing the output sees.
    #[test]
    fn every_realm_composes_its_own_pixels_whichever_is_bound() {
        let (w, h) = (32, 24);
        let geom: crate::view::ViewGeometry = (w, h).into();
        let (uw, uh) = geom.usable();
        let mut scenes = RealmScenes::new((w, h));
        // A commits the size `configure` gave it -- the usable view (issue
        // #304) -- so its own buffer is what the view holds below the reserved
        // rows. B is half that, and letterboxed inside the same rectangle.
        commit(&mut scenes, "realm-a", uw, uh);
        commit(&mut scenes, "realm-b", uw / 2, uh / 2);

        let view_of = |scenes: &RealmScenes, id: &str| {
            scenes
                .scene(&realm(id))
                .unwrap_or_else(|| panic!("{id} has a scene"))
                .compose(geom)
        };
        let a = view_of(&scenes, "realm-a");
        let b = view_of(&scenes, "realm-b");
        assert!(a != b, "the fixtures must actually differ");
        let mut a_expected =
            crate::scene::LETTERBOX_RGBA.repeat((w * geom.reserved_top()) as usize);
        a_expected.extend_from_slice(&client_pixels(uw, uh));
        assert_same_view(
            &a,
            &a_expected,
            "A's view is the reserved rows, then A's own buffer",
        );

        scenes.bind(&realm("realm-a"));
        assert_same_view(
            &scenes.bound().compose(geom),
            &a,
            "A bound: the output is A",
        );
        // ...and B's own scene is untouched by A holding the output.
        assert_same_view(&view_of(&scenes, "realm-b"), &b, "A bound: B's own view");

        scenes.bind(&realm("realm-b"));
        assert_same_view(
            &scenes.bound().compose(geom),
            &b,
            "B bound: the output is B",
        );
        assert_same_view(&view_of(&scenes, "realm-a"), &a, "B bound: A's own view");
    }

    /// A commit into one realm cannot move another realm's composed view —
    /// the byte-exact form of "one scene per realm".
    #[test]
    fn a_commit_in_one_realm_leaves_its_sibling_byte_identical() {
        let (w, h) = (16, 16);
        let mut scenes = RealmScenes::new((w, h));
        commit(&mut scenes, "realm-a", w, h);
        let before = scenes
            .scene(&realm("realm-a"))
            .expect("A has a scene")
            .compose((w, h).into());
        commit(&mut scenes, "realm-b", w, h);
        commit(&mut scenes, "realm-b", w / 2, h / 2);
        assert_same_view(
            &scenes
                .scene(&realm("realm-a"))
                .expect("A still has a scene")
                .compose((w, h).into()),
            &before,
            "B's commits moved A's view",
        );
    }

    /// With nothing bound the output is the documented deterministic
    /// background, even while realms are committing — and the `vacant` scene
    /// is never written by any of them.
    #[test]
    fn an_unbound_output_composes_the_deterministic_background() {
        let (w, h) = (20, 12);
        let mut scenes = RealmScenes::new((w, h));
        commit(&mut scenes, "realm-a", w, h);
        assert!(scenes.focused().is_none());
        assert_same_view(
            &scenes.bound().compose((w, h).into()),
            &test_pattern::render(w, h),
            "an unbound output",
        );
    }

    /// `layout_generation` moves on bind, on unbind and on a resize — and
    /// **not** on a commit, which is the whole reason it is not
    /// `Scene::generation` (D-018(5), decision 4).
    #[test]
    fn layout_generation_tracks_geometry_and_never_content() {
        let (a, b) = (realm("realm-a"), realm("realm-b"));
        let mut scenes = RealmScenes::new((64, 48));
        scenes.scene_mut(&a);
        scenes.scene_mut(&b);
        let at = |scenes: &RealmScenes, id: &RealmId| {
            scenes
                .layout_generation(id)
                .unwrap_or_else(|| panic!("realm {id} must have a scene, and so a counter"))
        };
        let (a0, b0) = (at(&scenes, &a), at(&scenes, &b));

        // A commit is content, not geometry.
        commit(&mut scenes, "realm-a", 8, 8);
        assert_eq!(at(&scenes, &a), a0, "a commit is not a layout change");
        assert!(
            scenes.scene(&a).unwrap().generation() > 0,
            "the content counter is the one that moved"
        );

        // Binding bumps the incoming realm only.
        scenes.bind(&a);
        assert_eq!(at(&scenes, &a), a0 + 1);
        assert_eq!(at(&scenes, &b), b0);

        // Re-binding the same realm changes nothing.
        scenes.bind(&a);
        assert_eq!(at(&scenes, &a), a0 + 1);

        // Binding elsewhere bumps both: A was unbound, B was bound.
        scenes.bind(&b);
        assert_eq!(at(&scenes, &a), a0 + 2);
        assert_eq!(at(&scenes, &b), b0 + 1);

        // A resize moves every realm's geometry, bound or not.
        scenes.set_view_size((80, 48));
        assert_eq!(at(&scenes, &a), a0 + 3);
        assert_eq!(at(&scenes, &b), b0 + 2);
        // The same size again is not a change.
        scenes.set_view_size((80, 48));
        assert_eq!(at(&scenes, &a), a0 + 3);
        assert_eq!(at(&scenes, &b), b0 + 2);
    }

    /// A realm nothing has touched has no scene and no counter — the honest
    /// `None` a capture over it turns into `no_surface`.
    #[test]
    fn an_untouched_realm_has_no_scene() {
        let scenes = RealmScenes::new((8, 8));
        assert!(scenes.scene(&realm("realm-z")).is_none());
        assert!(scenes.layout_generation(&realm("realm-z")).is_none());
    }
}
