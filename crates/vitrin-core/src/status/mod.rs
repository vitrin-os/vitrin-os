// SPDX-License-Identifier: MPL-2.0
//! **The status strip** (WS-E.2.3, issue #215): clock, battery and the focused
//! realm, drawn by the core **beside** the trusted band and never in it.
//!
//! # Why the core draws this at all
//!
//! There is no status bar and there cannot be a client one: `zwlr_layer_shell_v1`
//! is not in the shim's global contract (`shim/src/globals.c`), and that was
//! measured rather than assumed — waybar connects, binds six globals and never
//! maps a surface; rofi and wofi are the same class. So this is a panel's
//! replacement, not an ornament, and it is published as such in
//! `docs/book/src/limits.md`.
//!
//! # Never in the band, and that is structural rather than reviewed
//!
//! The trusted band's entire security value is that it is **one opaque colour
//! the confined app cannot reproduce** ([`crate::consent::indicator`]), and the
//! property is instrumented: [`crate::backend::band_witness`] exports a running
//! count of composites at which the band's rows changed, and **zero is the
//! property**. A clock in the band would make that counter tick by design and
//! there would be no remaining way to tell a correct session from a leaking
//! one. `TRUST_BAND_HEIGHT` stays exactly 8 rows of exactly one colour.
//!
//! The mechanism that keeps it that way is not a rule someone has to remember:
//! [`render::rasterize`] returns a buffer that is the *strip's* height and
//! [`StatusStrip::composite_over`] blits it at `y = TRUST_BAND_HEIGHT`. **There
//! is no coordinate expressible in the renderer that lands in the band.**
//!
//! # The strip is unspoofable in pixels and is *not* self-authenticating
//!
//! The strip always wins the composite, so a confined app cannot cover it. An
//! app *can* paint a convincing fake strip one row lower. The band above it is
//! the anchor, and the rule taught to the human is **"trusted content is
//! everything above the coloured line"**. That is strictly weaker than the
//! band's own guarantee, and the difference is written down here, in
//! `docs/book/src/limits.md` and on the `known-limit` surfaces rather than
//! blurred: the band proves itself, the strip only inherits position from it.
//!
//! # Where it sits, against the two surfaces that got here first
//!
//! Nobody had designed this strip as a whole. Three issues draw in the rows
//! immediately below the band and each landed without the others; this module
//! is the third and therefore owns the reconciliation. The composite order in
//! [`crate::backend::human_visible_from_view`] is now, in full:
//!
//! ```text
//!   realm view
//!     -> consent overlay      (scrim + framed card)
//!     -> lock cover           (WS-E.2.2, opaque, hides everything above)
//!     -> STATUS STRIP         (this module)
//!     -> trusted band         (rows [0, 8), one colour, always last of these)
//!     -> attention marker     (WS-E.1.7, in its own lane)
//!   ...and the dead-man hold indicator after everything, on both backends.
//! ```
//!
//! Each edge is a decision:
//!
//! - **After the lock cover, not under it.** Issue #215 left this open for
//!   whoever landed second and #214 landed first, so it is decided here: the
//!   strip is drawn *over* the lock. A clock is the one thing a human wants on
//!   a lock screen, and the stronger reason is that "the strip is always there"
//!   must have no exceptions — a rule with an exception is a rule a human
//!   cannot apply. It leaks nothing the lock exists to hide: the lock's job is
//!   that *client* content stops being legible, and every field here is
//!   core-owned (two integers from the sampler and a `realm.toml` id the lock
//!   card itself already prints in its "Realms held" row).
//! - **Before the band.** Nothing — core-drawn or not — may sit on the band.
//! - **Before the attention marker, in a lane the strip leaves blank.** The
//!   marker occupies `x < MARKER_W` of exactly these rows. Ordering alone would
//!   be enough to keep the marker visible, but it would leave a glyph
//!   half-covered whenever the human's window is open, so [`render::CONTENT_X`]
//!   starts past the lane and a `const` assertion makes an overgrown marker a
//!   compile error. #232's rule — the band's appearance must not get fuzzier —
//!   is untouched: the marker still never enters the band, and neither does
//!   this.
//! - **Under the dead-man hold indicator, which stays last of all**, so nothing
//!   here can hide a hold in progress.
//!
//! # Human-visible only
//!
//! [`StatusStrip::composite_over`]'s one call site is
//! [`crate::backend::human_visible_from_view`], downstream of the fork where
//! capture takes the bare realm view. So the clock — a timing oracle — and the
//! battery level can no more reach an agent's `frame_ready` than the consent
//! card can, and that is a property of *where the code runs*. The negative is
//! asserted in [`crate::backend::headless`] beside the consent card's own, and
//! it is asserted **after** the positive (the pattern is present in the
//! human-visible output) so it cannot pass vacuously.
//!
//! # Off by default
//!
//! `--status` is opt-in, on the `--agent-cursor` precedent and for the same
//! measured reason: the headless backend's human-visible framebuffer is
//! compared byte-for-byte against the realm view by [`crate::backend::band_witness`]
//! and by `tests/integration/test_real_trust_band.py`. A strip on by default
//! would make that comparison — and every human-visible golden — a function of
//! wall-clock time for every session that never asked for a status bar. It also
//! costs the app rows (see the next section) — now as a smaller `configure`
//! rather than as overdraw, but still rows — which a session that did not ask
//! for a strip should not pay.
//!
//! # The realm view IS inset, and the strip's rows are the conditional half
//!
//! Issue #215 asked for the app to be *configured* smaller by
//! `TRUST_BAND_HEIGHT + STATUS_STRIP_HEIGHT` so it lays out correctly instead
//! of having its top rows silently overdrawn. **That landed under issue #304**,
//! as [`crate::view::ViewGeometry`]: one value carrying the output's size and
//! the rows the core keeps, threaded through every path that places a client's
//! pixels — [`crate::scene::Scene::compose`], [`crate::scene::Scene::take_damage_view`],
//! the input router's `surface_local`, [`crate::dmabuf::human_visible_frame`] —
//! and through the `configure` the shim is told
//! ([`crate::shim::ShimServer::send_configure`]). The failure the issue itself
//! named — *"two places computing the usable view rectangle is how the CPU and
//! GPU paths drift"* — is answered by there being one: every one of those sites
//! asks [`crate::view::ViewGeometry::place`], and the reservation is derived
//! from [`STRIP_TOP`] rather than restated.
//!
//! **Which rows are conditional, and which are not.** The two halves of the
//! reservation do not have the same status and the difference is the one a
//! reader is most likely to get wrong:
//!
//! - The **trusted band's [`TRUST_BAND_HEIGHT`] rows are unconditional**. The
//!   band is drawn on every human-visible frame, last of all, with no arm of
//!   either compositor omitting it — so a session that never passed `--status`
//!   is still inset by 8. It was inset by nothing before #304, which is why
//!   the band's own overdraw was part of the same defect rather than a separate
//!   one.
//! - The **strip's [`DEFAULT_HEIGHT`] rows are conditional**, because
//!   `--status` is opt-in. With it off [`StatusStrip::height`] is `0` and the
//!   reservation is the band's rows alone; `ViewGeometry::new` takes that
//!   height rather than a flag, so "no strip" is `0` and not a special case.
//!
//! What a session pays for `--status`, then, is exactly the strip's own rows —
//! not the band's, which every session pays — and it pays them by being
//! *configured* for them rather than by losing them.

pub(crate) mod battery;
pub(crate) mod render;
pub(crate) mod sample;

use crate::consent::TRUST_BAND_HEIGHT;
use crate::grants::RealmId;
use crate::paint::canvas::Canvas;

use render::Strip;
pub(crate) use sample::{Sampler, StatusSnapshot, UtcOffset};

/// The strip's first row: immediately below the band, never inside it.
///
/// Derived from [`TRUST_BAND_HEIGHT`] and never restated, so the two cannot
/// drift — the discipline [`crate::dmabuf::trust_band_rect`]'s doc comment
/// exists to enforce for the band.
pub(crate) const STRIP_TOP: u32 = TRUST_BAND_HEIGHT;

/// The default `--status-height`, in rows.
///
/// **Measured, not chosen.** `Text::line_metrics(12.0)` on the bundled face
/// reports a 14-row line box with an 11-row ascent, so 14 rows of type plus 3
/// rows of padding above and below is 20. Nothing here is scaled to the output:
/// a status strip that grew with the display would change the app's usable
/// height with the monitor.
pub(crate) const DEFAULT_HEIGHT: u32 = 20;

/// The accepted `--status-height` range.
///
/// The floor is the smallest strip that can hold the 14-row line box at all
/// (plus one row for the rule); a shorter one would clip glyphs, and a clipped
/// digit on a trusted surface is a wrong digit. The ceiling is arbitrary in the
/// sense that no measurement picks it, and it is stated as such: it exists so a
/// typo cannot hand a session a strip taller than the display.
pub(crate) const MIN_HEIGHT: u32 = 16;
pub(crate) const MAX_HEIGHT: u32 = 64;

/// The refusal `--status-height` prints. A `const` so the operator's message
/// and the test's expectation are one string.
pub(crate) const HEIGHT_REFUSAL: &str = "`--status-height` takes a number of rows between 16 and \
                                         64. Below 16 the 14-row line box the bundled face \
                                         reports at 12 px would be clipped, and a clipped digit \
                                         on a trusted surface is a wrong digit.";

/// The session's status-strip policy, as the command line asked for it.
///
/// Validated at parse time, exactly as [`crate::lock::LockConfig`] and the chord
/// flags are, so a session with an unusable strip is a startup error rather than
/// a surface that quietly renders wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatusConfig {
    /// `--status`. Off by default (module docs).
    pub enabled: bool,
    /// `--status-height`, in `[MIN_HEIGHT, MAX_HEIGHT]`.
    pub height: u32,
    /// `--status-utc-offset`. UTC by default; the core holds no timezone
    /// database ([`sample`]).
    pub utc_offset: UtcOffset,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            height: DEFAULT_HEIGHT,
            utc_offset: UtcOffset::UTC,
        }
    }
}

impl StatusConfig {
    /// The off configuration, spelled out. Used by the parse tests and by every
    /// embedder that has no strip.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn off() -> Self {
        Self::default()
    }
}

/// The core's status strip at the backend output stage: the sampled snapshot,
/// its cached raster, and the generation counter presentation caches key on.
///
/// The [`crate::lock::LockSurface`] and [`crate::consent::ConsentSurface`]
/// pattern, deliberately down to the field names — three surfaces on one output
/// stage that behaved differently under a resize or a texture cache would be
/// three bugs waiting.
///
/// Both backends own one. It is always present and always composited through;
/// with `--status` off, [`Self::composite_over`] is a single branch and the
/// human-visible output is byte-identical to a build without this module.
pub(crate) struct StatusStrip {
    config: StatusConfig,
    /// The impure half. Held here rather than in the runtime because the
    /// snapshot is presentation state — like [`crate::consent::ConsentSurface`]'s
    /// prompt, it changes what a human sees and nothing else.
    sampler: Sampler,
    snapshot: Option<StatusSnapshot>,
    /// Bumped on every change to `snapshot` — the [`crate::scene::Scene`]
    /// pattern, which is what forces a recomposite when a minute ticks over.
    generation: u64,
    strip: Option<Strip>,
    strip_generation: u64,
    /// The view width the cached raster was drawn for. A resize invalidates it;
    /// keyed separately from the generation because a resize changes no field
    /// of the snapshot.
    strip_width: u32,
}

impl StatusStrip {
    pub fn new(config: StatusConfig) -> Self {
        Self {
            sampler: Sampler::new(config.utc_offset),
            config,
            snapshot: None,
            generation: 0,
            strip: None,
            strip_generation: 0,
            strip_width: 0,
        }
    }

    /// A strip with the sampler pointed at a fixture tree. Tests only; production
    /// has exactly one battery root and it is a constant ([`battery::SYSFS_ROOT`]).
    #[cfg(test)]
    pub fn with_root(config: StatusConfig, root: &'static std::path::Path) -> Self {
        Self {
            sampler: Sampler::with_root(config.utc_offset, root),
            config,
            snapshot: None,
            generation: 0,
            strip: None,
            strip_generation: 0,
            strip_width: 0,
        }
    }

    /// The strip's height in rows, or **0** when it is off.
    ///
    /// The single source every other reader derives geometry from: the CPU
    /// composite, the GPU draw list ([`crate::dmabuf::status_strip_rect`]) and
    /// the trusted-band witness all take this number rather than restating the
    /// constant, so no two of them can disagree about where the strip is.
    pub fn height(&self) -> u32 {
        if self.config.enabled {
            self.config.height
        } else {
            0
        }
    }

    /// The snapshot generation: changes exactly when composited output may have
    /// changed. Presentation caches key on it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Sample the world and update the snapshot; `true` when it changed.
    ///
    /// Offered once per dispatch round, exactly as
    /// [`crate::session::Presenter::set_attention`] is, and returning "changed"
    /// for the same reason: the round marks the session dirty only when the
    /// human-visible output really moved. A strip that is off never samples, so
    /// a session without one makes no clock read and no filesystem read at all.
    pub fn refresh(
        &mut self,
        now: std::time::SystemTime,
        mono: std::time::Instant,
        realm: Option<&RealmId>,
    ) -> bool {
        if !self.config.enabled {
            return false;
        }
        let snapshot = self.sampler.sample(now, mono, realm);
        if self.snapshot.as_ref() == Some(&snapshot) {
            return false;
        }
        self.snapshot = Some(snapshot);
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// The rasterized strip at `width`, rendering it if the cache is cold or
    /// stale. `None` when the strip is off or nothing has been sampled yet.
    fn strip(&mut self, width: u32) -> Option<&Strip> {
        if !self.config.enabled {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        if self.strip.is_none()
            || self.strip_generation != self.generation
            || self.strip_width != width
        {
            self.strip = Some(render::rasterize(snapshot, width, self.config.height));
            self.strip_generation = self.generation;
            self.strip_width = width;
        }
        self.strip.as_ref()
    }

    /// The rasterized strip's bytes at `width`, for the zero-copy path's texture
    /// upload. Same cache, same generation key — the GPU never rasterizes its
    /// own copy, which is what keeps the two paths from drawing different
    /// strips.
    pub fn raster(&mut self, width: u32) -> Option<(&[u8], u32, u32)> {
        let strip = self.strip(width)?;
        Some((strip.rgba.as_slice(), strip.width, strip.height))
    }

    /// Composite the strip over an already-composed human-visible view, in
    /// place.
    ///
    /// **Human-visible output only.** The one call site is
    /// [`crate::backend::human_visible_from_view`]; nothing on the capture path
    /// calls it, which is the whole of "the clock is not a timing oracle for an
    /// agent" — a property of where the code runs, not of a flag.
    ///
    /// `view` must be `width * height * 4` bytes of RGBA8888. A mismatch is
    /// refused rather than panicking, as everywhere else on this path.
    ///
    /// The blit lands at `y = `[`STRIP_TOP`], and the source buffer is the
    /// strip's own height, so **the band's rows are unreachable from here**.
    pub fn composite_over(&mut self, view: &mut [u8], width: u32, height: u32) {
        let Some(strip) = self.strip(width) else {
            return;
        };
        let (rgba, sw, sh) = (&strip.rgba, strip.width, strip.height);
        let Some(mut canvas) = Canvas::new(view, width, height) else {
            return;
        };
        canvas.blit_opaque(rgba, sw, sh, 0, STRIP_TOP as i32);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::scene::BYTES_PER_PIXEL;
    use battery::BatteryReading;
    use sample::ClockReading;
    use std::time::{Duration, Instant, SystemTime};

    const W: u32 = 640;
    const H: u32 = 400;

    fn nowhere() -> &'static std::path::Path {
        std::path::Path::new("/nonexistent/vitrin/power_supply")
    }

    pub(crate) fn on() -> StatusConfig {
        StatusConfig {
            enabled: true,
            ..StatusConfig::default()
        }
    }

    fn flat_view(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut view = Vec::with_capacity(w as usize * h as usize * BYTES_PER_PIXEL);
        for _ in 0..(w as usize * h as usize) {
            view.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 0xff]);
        }
        view
    }

    fn px(view: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
        [view[at], view[at + 1], view[at + 2], view[at + 3]]
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn a_strip_that_is_off_changes_no_pixel_and_reads_no_clock() {
        let mut strip = StatusStrip::with_root(StatusConfig::off(), nowhere());
        assert!(!strip.refresh(at(0), Instant::now(), None));
        assert_eq!(strip.height(), 0);
        let mut view = flat_view(W, H, [0x40, 0x50, 0x60]);
        let before = view.clone();
        strip.composite_over(&mut view, W, H);
        assert_eq!(view, before, "`--status` off must composite nothing");
    }

    /// The load-bearing structural claim, asserted from the outside: after a
    /// composite the band's rows are **byte-identical** to what they were, over
    /// every strip height the flag accepts.
    #[test]
    fn the_strip_can_never_reach_the_bands_rows() {
        for height in [MIN_HEIGHT, DEFAULT_HEIGHT, MAX_HEIGHT] {
            let mut strip = StatusStrip::with_root(
                StatusConfig {
                    enabled: true,
                    height,
                    ..StatusConfig::default()
                },
                nowhere(),
            );
            strip.refresh(at(1_786_244_643), Instant::now(), Some(&RealmId::new("r0")));
            let mut view = flat_view(W, H, [0x40, 0x50, 0x60]);
            let before = view.clone();
            strip.composite_over(&mut view, W, H);
            let band = TRUST_BAND_HEIGHT as usize * W as usize * BYTES_PER_PIXEL;
            assert_eq!(
                view[..band],
                before[..band],
                "a strip {height} rows tall touched the band"
            );
            // ...and it really did draw something, so the assertion above is
            // not passing because nothing happened.
            assert_ne!(view, before, "the strip drew nothing at height {height}");
            // The row immediately below the band is the strip's first row.
            assert_ne!(
                px(&view, W, render::CONTENT_X, TRUST_BAND_HEIGHT),
                px(&before, W, render::CONTENT_X, TRUST_BAND_HEIGHT)
            );
            // ...and the row immediately below the strip is untouched.
            let below = TRUST_BAND_HEIGHT + height;
            assert_eq!(
                px(&view, W, 0, below),
                px(&before, W, 0, below),
                "the strip overran its own height"
            );
        }
    }

    #[test]
    fn the_snapshot_generation_moves_only_when_a_drawn_field_moves() {
        let mut strip = StatusStrip::with_root(on(), nowhere());
        let mono = Instant::now();
        assert!(strip.refresh(at(1_786_244_643), mono, None));
        let g = strip.generation();
        // One second later, same minute: nothing drawn has changed.
        assert!(!strip.refresh(at(1_786_244_644), mono, None));
        assert_eq!(strip.generation(), g);
        // A minute later it has.
        assert!(strip.refresh(at(1_786_244_703), mono, None));
        assert_eq!(strip.generation(), g + 1);
        // So has a focus change, within the same minute.
        assert!(strip.refresh(at(1_786_244_703), mono, Some(&RealmId::new("r1"))));
        assert_eq!(strip.generation(), g + 2);
    }

    /// The raster cache is keyed on the width as well as the generation: a
    /// resize must not present yesterday's strip stretched or clipped.
    #[test]
    fn a_resize_invalidates_the_cached_raster() {
        let mut strip = StatusStrip::with_root(on(), nowhere());
        strip.refresh(at(1_786_244_643), Instant::now(), None);
        let narrow = strip.raster(320).map(|(_, w, h)| (w, h));
        assert_eq!(narrow, Some((320, DEFAULT_HEIGHT)));
        let wide = strip.raster(1280).map(|(_, w, h)| (w, h));
        assert_eq!(wide, Some((1280, DEFAULT_HEIGHT)));
    }

    /// The GPU path's bytes and the CPU path's bytes are the **same raster**,
    /// not two rasters that happen to agree today.
    #[test]
    fn the_cpu_blit_and_the_gpu_upload_are_the_same_bytes() {
        let mut strip = StatusStrip::with_root(on(), nowhere());
        strip.refresh(at(1_786_244_643), Instant::now(), Some(&RealmId::new("r0")));
        let (bytes, w, h) = strip.raster(W).map(|(b, w, h)| (b.to_vec(), w, h)).unwrap();
        let mut view = flat_view(W, H, [0, 0, 0]);
        strip.composite_over(&mut view, W, H);
        for row in 0..h {
            let dst = ((STRIP_TOP + row) as usize * W as usize) * BYTES_PER_PIXEL;
            let src = row as usize * w as usize * BYTES_PER_PIXEL;
            let run = w as usize * BYTES_PER_PIXEL;
            assert_eq!(&view[dst..dst + run], &bytes[src..src + run], "row {row}");
        }
    }

    /// The WS-E.2.3 visual golden: the rendered strip, pinned exactly.
    ///
    /// Byte-stable on x86-64 and aarch64 for the reason the consent and lock
    /// goldens are: the font is vendored and embedded, fontdue's
    /// architecture-dependent SIMD coverage accumulator is disabled (this
    /// crate's `Cargo.toml`), and every other pixel is integer math
    /// ([`crate::paint::canvas`]).
    ///
    /// The fixture is a **pinned [`StatusSnapshot`]**, never a sampled one —
    /// which is the whole point of the pure/impure split.
    #[test]
    fn status_strip_golden() {
        let snapshot = render::tests::fixture();
        let strip = render::rasterize(&snapshot, 640, DEFAULT_HEIGHT);
        let rendered = format!(
            "# vitrind status strip -- WS-E.2.3 visual golden\n\
             # Regenerate: VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core status_strip_golden\n\
             # One character per 4x4 block of the rasterized strip, by mean luminance.\n\
             size {}x{}\n\
             blake3 {}\n\
             {}",
            strip.width,
            strip.height,
            crate::recorder::ObservationDigest::of(&strip.rgba).to_hex(),
            ink_map(&strip)
        );

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/status_strip.txt");
        if std::env::var_os("VITRIN_REGEN_GOLDEN").is_some() {
            std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"))
                .expect("golden directory");
            std::fs::write(path, &rendered).expect("golden regeneration must be writable");
        }
        let committed = std::fs::read_to_string(path)
            .expect("committed status golden exists (regenerate: VITRIN_REGEN_GOLDEN=1)");
        assert_eq!(
            committed, rendered,
            "the rendered status strip no longer matches \
             crates/vitrin-core/tests/golden/status_strip.txt; if the strip's design changed \
             deliberately, regenerate with VITRIN_REGEN_GOLDEN=1 and check the ink-map diff"
        );
    }

    /// One character per 4x4 block by mean luminance — the lock golden's map at
    /// half the block size, because the strip is 20 rows tall and an 8x8 block
    /// would reduce it to two rows of characters.
    fn ink_map(strip: &Strip) -> String {
        const BLOCK: u32 = 4;
        const STEPS: &[(u32, char)] = &[
            (24, ' '),
            (48, '.'),
            (80, ':'),
            (120, '-'),
            (160, '='),
            (200, '+'),
            (240, '*'),
        ];
        let px = |rgba: &[u8], width: u32, x: u32, y: u32| -> [u8; 4] {
            let at = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
            [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
        };
        let mut out = String::new();
        let mut y = 0;
        while y < strip.height {
            let mut x = 0;
            while x < strip.width {
                let (mut sum, mut n) = (0u64, 0u64);
                for yy in y..(y + BLOCK).min(strip.height) {
                    for xx in x..(x + BLOCK).min(strip.width) {
                        let p = px(&strip.rgba, strip.width, xx, yy);
                        sum +=
                            (u64::from(p[0]) * 299 + u64::from(p[1]) * 587 + u64::from(p[2]) * 114)
                                / 1000;
                        n += 1;
                    }
                }
                let luma = (sum / n.max(1)) as u32;
                out.push(
                    STEPS
                        .iter()
                        .find(|(limit, _)| luma < *limit)
                        .map(|(_, ch)| *ch)
                        .unwrap_or('#'),
                );
                x += BLOCK;
            }
            out.push('\n');
            y += BLOCK;
        }
        out
    }

    /// A thousand composites with the clock advancing on every one: the band's
    /// rows never change and the strip's rows do. This is the criterion issue
    /// #215 exists to protect, run against the real surface rather than a
    /// paraphrase of it.
    #[test]
    fn a_thousand_ticking_composites_leave_the_band_alone() {
        let mut strip = StatusStrip::with_root(on(), nowhere());
        let mono = Instant::now();
        let band_bytes = TRUST_BAND_HEIGHT as usize * W as usize * BYTES_PER_PIXEL;
        let strip_bytes =
            (TRUST_BAND_HEIGHT + DEFAULT_HEIGHT) as usize * W as usize * BYTES_PER_PIXEL;
        let mut previous_band: Option<Vec<u8>> = None;
        let mut previous_strip: Option<Vec<u8>> = None;
        let (mut band_changes, mut strip_changes) = (0u32, 0u32);
        for tick in 0..1000u64 {
            // A minute per composite, so every one of them moves the clock.
            strip.refresh(at(1_786_244_643 + tick * 60), mono, None);
            let mut view = flat_view(W, H, [0x40, 0x50, 0x60]);
            strip.composite_over(&mut view, W, H);
            let band = view[..band_bytes].to_vec();
            let rows = view[band_bytes..strip_bytes].to_vec();
            if previous_band.as_ref().is_some_and(|p| *p != band) {
                band_changes += 1;
            }
            if previous_strip.as_ref().is_some_and(|p| *p != rows) {
                strip_changes += 1;
            }
            previous_band = Some(band);
            previous_strip = Some(rows);
        }
        assert_eq!(band_changes, 0, "the band's rows must be invariant");
        assert!(
            strip_changes > 900,
            "the strip must actually be repainting: {strip_changes} changes in 1000 composites"
        );
    }

    #[test]
    fn the_height_flag_range_is_what_the_type_size_needs() {
        // The floor really does hold the measured line box plus the rule row.
        let metrics = crate::paint::text::Text::new().line_metrics(render::TEXT_PX);
        assert!(
            MIN_HEIGHT > metrics.height,
            "MIN_HEIGHT {MIN_HEIGHT} cannot hold a {}-row line box plus the rule",
            metrics.height
        );
        assert_eq!(DEFAULT_HEIGHT, metrics.height + 6);
    }

    #[test]
    fn an_absent_battery_and_a_present_one_are_different_pixels() {
        let mut absent = render::tests::fixture();
        absent.battery = BatteryReading::Absent;
        let a = render::rasterize(&absent, W, DEFAULT_HEIGHT);
        let b = render::rasterize(&render::tests::fixture(), W, DEFAULT_HEIGHT);
        assert_ne!(a.rgba, b.rgba);
    }

    #[test]
    fn a_snapshot_with_no_clock_still_rasterizes() {
        let mut snapshot = render::tests::fixture();
        snapshot.clock = None::<ClockReading>;
        let strip = render::rasterize(&snapshot, W, DEFAULT_HEIGHT);
        assert_eq!(strip.rgba.len(), W as usize * DEFAULT_HEIGHT as usize * 4);
    }
}
