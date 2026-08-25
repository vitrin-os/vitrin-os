// SPDX-License-Identifier: MPL-2.0
//! **The usable realm view** (issue #304, the unmet half of issue #215): one
//! value carrying the output's size *and* the rows the core draws its own
//! chrome into, so every path that places a client's pixels — and the
//! `configure` the shim is told — reserves the same rows.
//!
//! # Why a carrier exists at all
//!
//! The core draws two surfaces above the client on **every** presented frame:
//! the trusted band ([`crate::consent::TRUST_BAND_HEIGHT`], rows `[0, 8)`) and,
//! when `--status` is on, the status strip immediately below it
//! ([`crate::status::STRIP_TOP`]). Until this module existed nothing subtracted
//! those rows from anything: the app was configured at the *output's* size, laid
//! out for rows it would never see, and had them overdrawn. `crate::status`'s
//! module docs carried that as a named unmet part of #215, and named the reason
//! it stayed unmet — the three paths that place client pixels
//! ([`crate::scene::Scene::compose`], the input router's `surface_local`,
//! [`crate::dmabuf::human_visible_frame`]) receive the view size from three
//! different places and *shared no carrier for a second number*.
//!
//! This is that carrier. It is a value, not a process-global: a global inside
//! the TCB is one more piece of mutable state a test harness cannot isolate,
//! and the session already threads its view size by hand through exactly these
//! call chains.
//!
//! # The band's rows are unconditional; the strip's are not
//!
//! This is the distinction the type exists to keep straight, and it is easy to
//! get wrong in the direction that publishes a false claim:
//!
//! * The **trusted band is drawn on every human-visible frame**, last of all,
//!   on both compositor paths and with no arm of either omitting it
//!   ([`crate::dmabuf::human_visible_frame`]'s docs). So its 8 rows are
//!   reserved for **every** session, including one that never passed
//!   `--status`.
//! * The **status strip is opt-in**. `--status` off means
//!   [`crate::status::StatusStrip::height`] is `0`, and the reservation is the
//!   band's 8 rows alone.
//!
//! So there is no such thing as a session whose view is not inset, and
//! [`ViewGeometry::new`] takes the strip's height rather than a flag: `0` is the
//! honest spelling of "no strip", not a special case.
//!
//! # One derivation, extending the one that was already there
//!
//! [`crate::status::STRIP_TOP`] is derived from [`TRUST_BAND_HEIGHT`] and never
//! restated, and [`crate::dmabuf::status_strip_rect`] is derived from
//! `STRIP_TOP` and never restates that. [`ViewGeometry::reserved_top`] is the
//! next link in the same chain — `STRIP_TOP + strip_h`, the row immediately
//! below the strip — so the band's height appears in this module **nowhere**.
//! Changing `TRUST_BAND_HEIGHT` moves the band, the strip and the inset
//! together or the build does not compile.
//!
//! # What each consumer takes from it
//!
//! | Consumer | Asks for |
//! |---|---|
//! | [`crate::scene::Scene::compose`] | [`ViewGeometry::output`] (the buffer's size) and [`ViewGeometry::place`] (where the client goes in it) |
//! | [`crate::scene::Scene::take_damage_view`] | [`ViewGeometry::place`], so damage lands where the composite drew |
//! | the input router's `surface_local` | [`ViewGeometry::place`], so a pointer maps to the pixel under it |
//! | [`crate::dmabuf::human_visible_frame`] | [`ViewGeometry::output`], [`ViewGeometry::place`] **and** [`ViewGeometry::strip_height`] — it used to take that last one as a second, parallel argument |
//! | [`crate::shim::ShimServer::send_configure`] | [`ViewGeometry::usable`], which is what the app is told it has |
//!
//! The `status_h: u32` parameter `human_visible_frame` used to carry is
//! **subsumed**, deliberately: it was partial awareness of exactly this problem
//! (the GPU path knew the strip's height because it had to draw it) and leaving
//! it beside a `ViewGeometry` would be the second carrier this module exists to
//! remove.

use crate::scene::layout::{self, Placement};
use crate::status::STRIP_TOP;

/// One output's geometry, split into the rows the core keeps and the rows the
/// client gets.
///
/// ```text
///   y = 0                 ┌──────────────────────────┐  ─┐
///                         │ trusted band (always)    │   │ reserved_top()
///   y = STRIP_TOP         ├──────────────────────────┤   │  = STRIP_TOP + strip_h
///                         │ status strip (--status)  │   │
///   y = reserved_top()    ├──────────────────────────┤  ─┘
///                         │                          │
///                         │  usable() — the client   │
///                         │                          │
///   y = output().1        └──────────────────────────┘
/// ```
///
/// `Copy` and small on purpose: it travels everywhere a `(u32, u32)` used to,
/// and a carrier that cost a clone at each hop would have grown a cache instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViewGeometry {
    /// The whole output: what a composed frame's buffer measures, band and
    /// strip included.
    output: (u32, u32),
    /// [`crate::status::StatusStrip::height`] — `0` when `--status` is off.
    strip_h: u32,
}

impl ViewGeometry {
    /// The geometry of an `output`-sized view whose status strip is `strip_h`
    /// rows tall (`0` when `--status` is off).
    ///
    /// **The only constructor a shipping build has.** The band's rows are not a
    /// parameter, because no session may decline them; a caller that wants "no
    /// strip" passes `0` and still reserves the band. That is the whole reason
    /// this takes a height rather than an `Option` or a bool.
    pub(crate) const fn new(output: (u32, u32), strip_h: u32) -> Self {
        Self { output, strip_h }
    }

    /// The whole output, band and strip included — the size a composed frame's
    /// buffer measures and the space the cursor sprites are clipped against.
    pub(crate) const fn output(&self) -> (u32, u32) {
        self.output
    }

    /// The status strip's height in rows, `0` when `--status` is off.
    ///
    /// The reason [`crate::dmabuf::human_visible_frame`] needs no second
    /// argument: the number it used to be handed separately is in here.
    pub(crate) const fn strip_height(&self) -> u32 {
        self.strip_h
    }

    /// The rows the core keeps at the top: the band's, plus the strip's when
    /// there is one.
    ///
    /// Derived from [`STRIP_TOP`] — itself derived from
    /// [`TRUST_BAND_HEIGHT`](crate::consent::TRUST_BAND_HEIGHT) — and never
    /// restated, which is the discipline
    /// [`crate::dmabuf::status_strip_rect`] documents applied one level up.
    /// **Clamped to the output**, so a view shorter than its own chrome
    /// reserves all of it and leaves the client nothing rather than wrapping a
    /// subtraction.
    pub(crate) const fn reserved_top(&self) -> u32 {
        let reserved = STRIP_TOP.saturating_add(self.strip_h);
        if reserved > self.output.1 {
            self.output.1
        } else {
            reserved
        }
    }

    /// What the client actually gets: the output minus the reserved rows. This
    /// is the size the shim is sent at `configure`, and the rectangle
    /// [`Self::place`] centres a surface inside.
    pub(crate) const fn usable(&self) -> (u32, u32) {
        (self.output.0, self.output.1 - self.reserved_top())
    }

    /// Where a `surface`-sized buffer's top-left corner lands **in output
    /// coordinates**.
    ///
    /// [`layout::place`] against the *usable* rectangle, translated down by the
    /// reserved rows. That module is the MVP's entire layout policy and its
    /// docs say it must never grow, so it is asked the same question it always
    /// was — centre one surface in one rectangle — and this is the one place in
    /// the core that knows the rectangle does not start at `y = 0`.
    ///
    /// **Every consumer of the placement goes through here**, which is what
    /// makes "the compositor, the router and the zero-copy path cannot disagree
    /// about where the surface is" a property of the code: they already shared
    /// `layout::place`, and they now share the translation too.
    pub(crate) fn place(&self, surface: (u32, u32)) -> Placement {
        let placement = layout::place(self.usable(), surface);
        Placement {
            x: placement.x,
            y: placement.y + i64::from(self.reserved_top()),
        }
    }
}

/// A `--status`-off session at this output size.
///
/// **Test-only, and `cfg(test)` rather than a convention**, so no shipping
/// build can spell a geometry without saying which strip height it means. What
/// it produces is a real configuration — `--status` is off by default, so most
/// sessions have exactly this geometry — which is why the unit tests that reach
/// for it still exercise a reservation (the band's 8 rows) rather than a
/// chrome-free view no session ever has.
#[cfg(test)]
impl From<(u32, u32)> for ViewGeometry {
    fn from(output: (u32, u32)) -> Self {
        Self::new(output, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::TRUST_BAND_HEIGHT;
    use crate::status::DEFAULT_HEIGHT;

    /// **The negative case #304 asks for by name**: with `--status` off the
    /// inset is the band's rows and nothing else.
    #[test]
    fn a_status_off_session_reserves_the_bands_rows_and_no_more() {
        let geom = ViewGeometry::new((1280, 800), 0);
        assert_eq!(geom.reserved_top(), TRUST_BAND_HEIGHT);
        assert_eq!(geom.usable(), (1280, 800 - TRUST_BAND_HEIGHT));
        assert_eq!(geom.output(), (1280, 800));
        assert_eq!(geom.strip_height(), 0);
    }

    /// ...and with `--status` on it is the band's plus the strip's, which is
    /// the `TRUST_BAND_HEIGHT + STATUS_STRIP_HEIGHT` issue #215 asked for.
    #[test]
    fn a_status_on_session_reserves_the_band_and_the_strip() {
        let geom = ViewGeometry::new((1280, 800), DEFAULT_HEIGHT);
        assert_eq!(geom.reserved_top(), TRUST_BAND_HEIGHT + DEFAULT_HEIGHT);
        assert_eq!(
            geom.usable(),
            (1280, 800 - TRUST_BAND_HEIGHT - DEFAULT_HEIGHT)
        );
        assert_eq!(geom.strip_height(), DEFAULT_HEIGHT);
    }

    /// The reservation is **derived**, not restated: it is exactly the strip's
    /// top plus the strip's height, so the row immediately below the strip is
    /// the client's first row and there is no second arithmetic to drift.
    #[test]
    fn the_reservation_is_the_row_below_the_strip() {
        for strip_h in [
            0,
            crate::status::MIN_HEIGHT,
            DEFAULT_HEIGHT,
            crate::status::MAX_HEIGHT,
        ] {
            let geom = ViewGeometry::new((640, 480), strip_h);
            assert_eq!(geom.reserved_top(), STRIP_TOP + strip_h);
            // ...and that is where the status strip's own rectangle ends, the
            // one `dmabuf::status_strip_rect` derives from the same constant.
            let rect = crate::dmabuf::status_strip_rect((640, 480).into(), strip_h);
            assert_eq!(
                (rect.loc.y + rect.size.h) as u32,
                geom.reserved_top(),
                "the client's first row must be the row after the strip's last"
            );
        }
    }

    /// A surface exactly the size it was configured at lands **immediately
    /// below the chrome**, flush, with no letterbox between them — which is the
    /// whole point of telling the app a smaller size.
    #[test]
    fn a_surface_at_the_configured_size_starts_at_the_first_free_row() {
        for strip_h in [0, DEFAULT_HEIGHT] {
            let geom = ViewGeometry::new((320, 240), strip_h);
            let placed = geom.place(geom.usable());
            assert_eq!(placed.x, 0);
            assert_eq!(placed.y, i64::from(geom.reserved_top()));
        }
    }

    /// Placement is still centred and still 1:1 — inside the usable rectangle,
    /// which is the only thing that moved.
    #[test]
    fn a_smaller_surface_is_centred_inside_the_usable_rectangle() {
        let geom = ViewGeometry::new((100, 100), 12);
        // reserved = 8 + 12 = 20, usable = 100x80, surface 40x20.
        let placed = geom.place((40, 20));
        assert_eq!(placed.x, 30);
        assert_eq!(placed.y, 20 + 30);
    }

    /// A view shorter than its own chrome reserves everything it has and
    /// answers a zero-height usable rectangle, rather than wrapping.
    #[test]
    fn a_view_shorter_than_its_chrome_does_not_wrap() {
        let geom = ViewGeometry::new((64, 4), DEFAULT_HEIGHT);
        assert_eq!(geom.reserved_top(), 4);
        assert_eq!(geom.usable(), (64, 0));
        let geom = ViewGeometry::new((64, 0), 0);
        assert_eq!(geom.reserved_top(), 0);
        assert_eq!(geom.usable(), (64, 0));
    }

    /// **The inset has exactly one derivation, and these are the only places a
    /// shipping build states its two inputs.**
    ///
    /// This is `the_blank_cover_has_exactly_one_chokepoint`'s discipline applied
    /// to the failure `crate::status`'s module docs named before the inset
    /// existed: *"a half-done inset — one path reserving rows the others do not
    /// — is strictly worse than none"*. The compiler holds most of it (every
    /// placement consumer takes a [`ViewGeometry`], and there is no other way to
    /// place a surface), and what it cannot hold is this: a fourth site minting
    /// a geometry with a strip height of its own would be a second answer to
    /// "how many rows does this session reserve", and the N+1st presentation
    /// path is the one that gets it wrong.
    ///
    /// The three allowed sites, each for a reason a reader can check:
    ///
    /// * `session.rs` — `Presenter::view_geometry`, the provided method every
    ///   backend inherits. This is the one production answer.
    /// * `backend/headless.rs` — `HeadlessOutput::geometry`, because
    ///   `RetainedOutput::scrub_retained_frame` composites holding that struct
    ///   alone, with no `Presenter` in reach.
    /// * `backend/headless.rs` — `render_once`, the golden harness, which
    ///   composes an **empty** scene and has no status strip to ask.
    #[test]
    fn the_view_geometry_has_one_derivation() {
        fn rust_sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("the crate source tree is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    rust_sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("a readable source file");
                    // Truncated at the test module, exactly as the blank
                    // cover's scan is: a test that mints a geometry is not a
                    // production decision about one. **Both spellings**, and
                    // that is not pedantry -- this crate's `scene`, `input` and
                    // `status` test modules are `pub(crate) mod tests`, so a
                    // scan that knew only the bare `mod tests` counted their
                    // fixtures as production sites, which is how the first cut
                    // of this test failed on files it never meant to read.
                    let production = [
                        "\n#[cfg(test)]\nmod tests {",
                        "\n#[cfg(test)]\npub(crate) mod tests {",
                    ]
                    .iter()
                    .filter_map(|marker| text.find(marker))
                    .min()
                    .map(|at| text[..at].to_string())
                    .unwrap_or(text);
                    out.push((path, production));
                }
            }
        }
        let mut sources = Vec::new();
        rust_sources(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut sources,
        );
        assert!(
            sources.iter().any(|(p, _)| p.ends_with("view.rs")),
            "the scan must cover this file, or it proves nothing"
        );

        let mut sites: Vec<(String, usize)> = Vec::new();
        for (path, text) in &sources {
            let count = text.matches("ViewGeometry::new(").count();
            if count > 0 {
                let name = path
                    .strip_prefix(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
                    .unwrap_or(path)
                    .to_string_lossy()
                    .trim_start_matches('/')
                    .to_string();
                sites.push((name, count));
            }
        }
        sites.sort();
        assert_eq!(
            sites,
            vec![
                ("backend/headless.rs".to_string(), 2),
                ("session.rs".to_string(), 1),
            ],
            "a new `ViewGeometry::new` call site is a second decision about how many rows this \
             session reserves. If it is deliberate, argue it in this test's doc comment and add \
             it here; if it is not, take the geometry from `Presenter::view_geometry` instead"
        );
    }

    /// **The five sites agree, checked against each other rather than against
    /// this module's own arithmetic.**
    ///
    /// The threading is only worth anything if the compositor, the damage path,
    /// the router, the zero-copy draw list and the `configure` all reserve the
    /// same rows. Each is asked here for the one number it exposes, and the
    /// answers are compared — so a site that quietly stopped taking its
    /// placement from the geometry fails here rather than at a pixel a human
    /// has to notice.
    ///
    /// Run for both halves of the reservation: `--status` off (the band's rows
    /// alone) and `--status` on at the default height.
    #[test]
    fn every_placement_consumer_reserves_the_same_rows() {
        use crate::scene::{Scene, SurfaceContent, BYTES_PER_PIXEL, LETTERBOX_RGBA};

        for strip_h in [0, DEFAULT_HEIGHT] {
            let geom = ViewGeometry::new((80, 100), strip_h);
            let (uw, uh) = geom.usable();
            let inset = geom.reserved_top();
            assert_eq!(inset, TRUST_BAND_HEIGHT + strip_h);

            // 1. `configure`: the app is told the usable view, never the output.
            let mut server = crate::shim::ShimServer::new(crate::shim::ShimConfig {
                realm: "realm-0".into(),
                geom,
            });
            assert_eq!(server.configured_size(), (uw, uh));
            let mut sent = 0usize;
            assert!(
                !server
                    .reconfigure(geom, &mut |_frame: &[u8]| {
                        sent += 1;
                        Ok(())
                    })
                    .expect("an unchanged geometry sends nothing"),
                "re-configuring to the same geometry must send nothing"
            );
            assert_eq!(sent, 0);

            // 2. `Scene::compose`: an app committing that size lands flush under
            //    the reserved rows, which are matte.
            let mut scene = Scene::new();
            let client = [0x21u8, 0x43, 0x65, 0xff].repeat((uw * uh) as usize);
            scene.commit(SurfaceContent::from_rgba(client, uw, uh).expect("a valid buffer"));
            let composed = scene.compose(geom);
            let px = |y: u32| {
                let at = (y as usize * 80) * BYTES_PER_PIXEL;
                [
                    composed[at],
                    composed[at + 1],
                    composed[at + 2],
                    composed[at + 3],
                ]
            };
            for y in 0..inset {
                assert_eq!(px(y), LETTERBOX_RGBA, "row {y} must be reserved");
            }
            assert_eq!(
                px(inset),
                [0x21, 0x43, 0x65, 0xff],
                "the client's first row is the row after the reservation"
            );

            // 3. The zero-copy draw list places the content at the same origin.
            let draws = crate::dmabuf::human_visible_frame(
                geom,
                (uw, uh),
                crate::consent::TrustedIndicator::for_test(),
                None,
                None,
            );
            let content = draws
                .iter()
                .find_map(|draw| match draw {
                    crate::dmabuf::Draw::Content(rect) => Some(*rect),
                    _ => None,
                })
                .expect("every frame draws the content");
            assert_eq!(content.loc.y, inset as i32);
            assert_eq!(content.size.h, uh as i32);

            // 4. The router maps a pointer on the client's first row to the
            //    surface's row 0 — the inverse of the placement above.
            let mut router = crate::input::InputRouter::detached(crate::input::NoopHook);
            let realm = crate::input::tests::test_realm();
            assert!(router.bind_to(&realm).is_none());
            let delivery = router
                .route_physical(
                    crate::input::tests::phys(crate::input::SeatInputKind::Motion {
                        x: 0.0,
                        y: f64::from(inset),
                    }),
                    geom,
                    Some((uw, uh)),
                )
                .expect("a pointer on the client's first row is inside the app");
            match delivery.kind() {
                crate::input::SeatDeliveryKind::Motion { x, y } => {
                    assert_eq!(x.to_f64(), 0.0);
                    assert_eq!(
                        y.to_f64(),
                        0.0,
                        "the app's origin is the first row it was configured for"
                    );
                }
                other => panic!("expected motion, got {other:?}"),
            }

            // 5. Damage the app names at its own origin lands on the client's
            //    first row, never on the band's. The first commit is unbounded
            //    (a fresh surface changes the whole view), so it is drained
            //    before the bounded one -- otherwise this would assert on the
            //    `None` that means "assume everything changed".
            let mut damaged = Scene::new();
            let client = [0x21u8, 0x43, 0x65, 0xff].repeat((uw * uh) as usize);
            damaged
                .commit(SurfaceContent::from_rgba(client.clone(), uw, uh).expect("a valid buffer"));
            damaged.take_damage_view(geom);
            damaged.commit_with_damage(
                SurfaceContent::from_rgba(client, uw, uh).expect("a valid buffer"),
                Some(crate::scene::DamageRect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 4,
                }),
            );
            let rect = damaged
                .take_damage_view(geom)
                .expect("a bounded damage rectangle");
            assert_eq!(
                rect.y, inset as i32,
                "damage at the app's origin is damage at the first unreserved row"
            );
        }
    }

    /// The test-only conversion is a **real** geometry: it reserves the band.
    /// If this ever answered zero, every unit test that reaches for it would
    /// stop exercising the inset at all — the exact way this kind of helper
    /// stops checking.
    #[test]
    fn the_test_conversion_still_reserves_the_band() {
        let geom: ViewGeometry = (800u32, 600u32).into();
        assert_eq!(geom.reserved_top(), TRUST_BAND_HEIGHT);
        assert_ne!(geom.reserved_top(), 0);
    }
}
