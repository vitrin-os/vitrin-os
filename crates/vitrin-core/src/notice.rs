// SPDX-License-Identifier: MPL-2.0
//! **A core-drawn notice the human can read on the panel they are trying to
//! leave** (WS-E.3.5): a bounded, expiring, human-visible-only band the core
//! raises when something it cannot fix has happened to the session itself.
//!
//! # Why this exists, and why a log line was not enough
//!
//! On 2026-08-09 the bare-metal backend ran on hardware for the first time and
//! the human could not leave his own VT: `Ctrl-Alt-F<n>` did nothing, because
//! once a compositor holds DRM master the kernel stops handling that chord.
//! [`crate::vt`] implements it now. But `Session::change_vt` can be **refused**
//! by seatd/logind, and it can be **accepted and never happen** — and a
//! failure on that path lands on a screen whose owner is, by definition,
//! trying to get away from it.
//!
//! A human staring at a panel cannot read `stderr`, and if they could they
//! would not be trapped. So a VT switch that fails silently reproduces the
//! original defect exactly: a key that does nothing and says nothing. This
//! module is the surface that says it.
//!
//! # What may appear on it — a closed vocabulary, not a message channel
//!
//! [`NoticeContent`] is an enum with no `String` field and no free-form text.
//! Every glyph comes from a `const` in this file plus small integers the core
//! itself chose (a VT number it resolved, a VT number it was asked for). There
//! is deliberately **no** variant carrying an errno message, a path, a
//! principal id or a realm id: [`crate::screenshot::capture_to_file`]'s rule
//! for the journal — "a field an errno *message* can reach is a field an
//! attacker's filename can reach" — applies with more force to a trusted
//! surface a human is being asked to act on. [`crate::paint::text`]
//! substitutes anything outside the ASCII printables at the last moment
//! regardless.
//!
//! # Never in the band, and never in a capture
//!
//! Two structural facts, not two rules to remember:
//!
//! * **The band is unreachable from here.** [`CoreNotice::composite_over`]
//!   blits at `y = max(TRUST_BAND_HEIGHT, height - band_height)`, so no
//!   content, view size or arithmetic mistake in this module can land a pixel
//!   in rows `[0, TRUST_BAND_HEIGHT)`. That is [`crate::status`]'s discipline
//!   and it is load-bearing for the same reason: the band's entire value is
//!   that it is one colour nothing composites into (D-026, D-030(1)).
//! * **An agent cannot observe it.** The one call site is
//!   [`crate::backend::winit::window_pixels`], downstream of the output-stage
//!   fork where a capture takes [`crate::scene::Scene::compose`]. So this can
//!   no more reach `vitrin_view.frame_ready` than the consent card can — and
//!   that matters here specifically, because "the human's session is trapped"
//!   is exactly the fact an adversarial agent would most like to learn.
//!
//! # It expires, and it forces the CPU composite while it is up
//!
//! [`NOTICE_DEADLINE`] clears it. A permanent notice would be a second
//! always-on surface competing with the status strip for the human's trust in
//! "core chrome is what is above the line", and an alarm that never goes away
//! stops being an alarm. While it *is* up,
//! [`crate::backend::winit::overlay_needs_the_window`] answers `true`, so the
//! zero-copy branch is refused and the frame is CPU-composited — the same cost
//! the lock cover and the consent card already pay, accepted here for the ten
//! seconds a notice lives.

use std::time::{Duration, Instant};

use crate::consent::TRUST_BAND_HEIGHT;
use crate::paint::canvas::{Canvas, Rect};
use crate::paint::text::Text;
use crate::scene::BYTES_PER_PIXEL;

/// How long a raised notice stays on the panel.
///
/// Long enough that a human who looked away from the screen to press a key
/// still reads it; short enough that the session does not acquire a permanent
/// second chrome surface. Ten seconds is a judgement, not a measurement, and
/// is stated as one.
pub(crate) const NOTICE_DEADLINE: Duration = Duration::from_secs(10);

/// Type size. Larger than the status strip's 12 px on purpose: this is read
/// once, under stress, by somebody who has just pressed a key that did
/// nothing.
const TEXT_PX: f32 = 16.0;

/// Padding inside the band, in rows/columns.
const PAD: u32 = 10;

/// The band's ground.
///
/// A dark red that **cannot be mistaken for a [`crate::consent::TrustedIndicator`]
/// colour**: session colours are drawn from `[64, 255]` on every channel, and
/// two of these three are far below that floor. Also nothing like the status
/// strip's near-black ground, because the two surfaces mean opposite things —
/// the strip reports, this one interrupts.
const GROUND: [u8; 4] = [0x5a, 0x12, 0x12, 0xff];
/// A rule along the band's top edge, so it reads as a fixed piece of core
/// chrome rather than as something the app drew.
const RULE: [u8; 4] = [0xd2, 0x4b, 0x4b, 0xff];
const HEADLINE_FG: [u8; 3] = [0xff, 0xe8, 0xe8];
const BODY_FG: [u8; 3] = [0xf0, 0xc9, 0xc9];

/// How many wrapped body lines a notice may draw. A bound rather than a
/// scroll: a surface with more to say than this has to say less.
const MAX_BODY_LINES: usize = 3;

/// What a notice may say. **A closed vocabulary** (module docs): no variant
/// carries a `String`, a path, or anything an app or a principal can
/// influence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the only producer is the bare-metal backend's VT drain; the surface itself is \
                  compiled and tested in every build so a default build cannot drift from it"
    )
)]
pub(crate) enum NoticeContent {
    /// A `Ctrl-Alt-F<n>` the human pressed did not move them.
    ///
    /// `target` is the VT they asked for (1..=12, chosen by the core from the
    /// key they pressed); `own` is this session's own VT if it could be
    /// resolved at startup, and `None` when it could not — in which case the
    /// notice deliberately does not invent one.
    VtSwitchFailed { target: i32, own: Option<i32> },
}

impl NoticeContent {
    /// The headline: one short line, in capitals, naming what happened.
    fn headline(&self) -> String {
        match *self {
            NoticeContent::VtSwitchFailed { target, .. } => {
                format!("VT {target} SWITCH FAILED - this session cannot leave this terminal.")
            }
        }
    }

    /// The body: what the human can still do. Assembled from `const`
    /// fragments and core-chosen integers, then wrapped to the view width.
    fn body(&self) -> String {
        match *self {
            NoticeContent::VtSwitchFailed { own, .. } => {
                let mut body = String::from(
                    "Hold Esc to revoke every grant. To stop the session you need a shell from \
                     elsewhere and SIGTERM.",
                );
                if let Some(own) = own {
                    body.push_str(&format!(" (vitrind is on VT {own}.)"));
                }
                body
            }
        }
    }
}

/// One rasterized notice band.
struct Raster {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// The core's notice surface at the backend output stage: what is up, when it
/// expires, and the cached raster the composite blits.
///
/// The [`crate::status::StatusStrip`] / [`crate::lock::LockSurface`] /
/// [`crate::consent::ConsentSurface`] shape, deliberately down to the
/// generation-and-width cache key: four surfaces on one output stage that
/// behaved differently under a resize would be four bugs waiting.
pub(crate) struct CoreNotice {
    /// The raised content and the instant it stops being shown.
    up: Option<(NoticeContent, Instant)>,
    /// Bumped whenever `up` changes, so the raster cache has a key.
    generation: u64,
    raster: Option<Raster>,
    raster_generation: u64,
    text: Text,
}

impl Default for CoreNotice {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreNotice {
    pub fn new() -> Self {
        Self {
            up: None,
            generation: 0,
            raster: None,
            raster_generation: 0,
            text: Text::new(),
        }
    }

    /// Raise a notice, replacing whatever was up.
    ///
    /// Replacing rather than queueing: two notices in one ten-second window
    /// means something is going badly wrong, and the *newest* fact is the one
    /// the human needs. A queue would show them a stale alarm first.
    #[cfg_attr(
        not(feature = "drm-backend"),
        allow(
            dead_code,
            reason = "the only production caller is the bare-metal backend's VT drain; the \
                      behaviour is exercised by this module's own tests in every build"
        )
    )]
    pub fn raise(&mut self, content: NoticeContent, now: Instant) {
        self.up = Some((content, now + NOTICE_DEADLINE));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Expire a notice whose deadline has passed; `true` when the
    /// human-visible output changed and a frame is owed.
    ///
    /// Level-triggered and idempotent, like every other "service one turn"
    /// entry point in this core: safe to call from a frame, from a timer, and
    /// from an input turn.
    #[cfg_attr(
        not(feature = "drm-backend"),
        allow(
            dead_code,
            reason = "the only production caller is the bare-metal notice timer; the behaviour                       is exercised by this module's own tests in every build"
        )
    )]
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.up {
            Some((_, until)) if now >= until => {
                self.up = None;
                self.generation = self.generation.wrapping_add(1);
                true
            }
            _ => false,
        }
    }

    /// When the raised notice stops being shown, for an embedder that has to
    /// arm a timer to take it off an otherwise idle session's screen.
    ///
    /// Read out rather than assumed from the raise instant, on
    /// `DeadManSwitch::deadline`'s pattern: a second raise moves the deadline,
    /// and a timer armed against the first one would clear the screen early or
    /// never.
    #[cfg_attr(
        not(feature = "drm-backend"),
        allow(
            dead_code,
            reason = "the only production caller is the bare-metal notice timer; the behaviour                       is exercised by this module's own tests in every build"
        )
    )]
    pub fn until(&self) -> Option<Instant> {
        self.up.map(|(_, until)| until)
    }

    /// Whether a notice is on the panel right now.
    ///
    /// Read by [`crate::backend::winit::overlay_needs_the_window`]: a notice
    /// is composited on the CPU, so the zero-copy branch would present the
    /// client's texture with the notice nowhere on it.
    pub fn is_up(&self) -> bool {
        self.up.is_some()
    }

    /// The band's height at this type size: a headline, up to
    /// [`MAX_BODY_LINES`] body lines, and padding above and below.
    fn band_height(&self, lines: usize) -> u32 {
        let metrics = self.text.line_metrics(TEXT_PX);
        PAD * 2 + metrics.height * (1 + lines as u32)
    }

    /// Rasterize (or reuse) the band at `width`.
    fn raster(&mut self, width: u32) -> Option<&Raster> {
        let (content, _) = self.up?;
        if width == 0 {
            return None;
        }
        let stale = self.raster.as_ref().is_none_or(|raster| {
            raster.width != width || self.raster_generation != self.generation
        });
        if stale {
            self.raster = Some(self.rasterize(content, width));
            self.raster_generation = self.generation;
        }
        self.raster.as_ref()
    }

    fn rasterize(&mut self, content: NoticeContent, width: u32) -> Raster {
        let inner = width.saturating_sub(PAD * 2).max(1);
        let body = self
            .text
            .wrap(&content.body(), TEXT_PX, inner, MAX_BODY_LINES);
        let height = self.band_height(body.len());
        let mut rgba = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
        let metrics = self.text.line_metrics(TEXT_PX);
        if let Some(mut canvas) = Canvas::new(&mut rgba, width, height) {
            canvas.fill_rect(
                Rect {
                    x: 0,
                    y: 0,
                    w: width,
                    h: height,
                },
                GROUND,
            );
            canvas.fill_rect(
                Rect {
                    x: 0,
                    y: 0,
                    w: width,
                    h: 2,
                },
                RULE,
            );
            let mut baseline = (PAD + metrics.ascent) as i32;
            self.text.draw(
                &mut canvas,
                &content.headline(),
                TEXT_PX,
                PAD as i32,
                baseline,
                HEADLINE_FG,
            );
            for line in &body {
                baseline += metrics.height as i32;
                self.text
                    .draw(&mut canvas, line, TEXT_PX, PAD as i32, baseline, BODY_FG);
            }
        }
        Raster {
            rgba,
            width,
            height,
        }
    }

    /// Composite the band over an already-composed human-visible view, in
    /// place.
    ///
    /// **Anchored to the bottom edge, and floored at [`TRUST_BAND_HEIGHT`]**:
    /// the bottom because no other core surface claims it, and the floor
    /// because that is what makes "this module cannot draw on the band" a
    /// structural fact rather than a review item. On a view too short to hold
    /// both, the band wins and the notice is clipped — which is the correct
    /// direction, since the band is the thing every other trust claim hangs
    /// off.
    ///
    /// `view` must be `width * height * 4` bytes of RGBA8888; a mismatch is
    /// refused rather than panicking, as everywhere else on this path.
    pub fn composite_over(&mut self, view: &mut [u8], width: u32, height: u32) {
        let Some(raster) = self.raster(width) else {
            return;
        };
        let (rgba, sw, sh) = (raster.rgba.clone(), raster.width, raster.height);
        let Some(mut canvas) = Canvas::new(view, width, height) else {
            return;
        };
        let y = height.saturating_sub(sh).max(TRUST_BAND_HEIGHT);
        canvas.blit_opaque(&rgba, sw, sh, 0, y as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 800;
    const H: u32 = 600;

    fn blank() -> Vec<u8> {
        vec![0u8; W as usize * H as usize * BYTES_PER_PIXEL]
    }

    fn row(view: &[u8], y: u32, width: u32) -> &[u8] {
        let start = y as usize * width as usize * BYTES_PER_PIXEL;
        &view[start..start + width as usize * BYTES_PER_PIXEL]
    }

    fn content() -> NoticeContent {
        NoticeContent::VtSwitchFailed {
            target: 3,
            own: Some(2),
        }
    }

    /// **A raised notice really paints, and an unraised one paints nothing.**
    ///
    /// The negative first would pass vacuously against a surface that never
    /// draws at all, so the positive leads and the negative is the control.
    #[test]
    fn a_raised_notice_paints_and_an_idle_one_does_not() {
        let t0 = Instant::now();
        let mut notice = CoreNotice::new();

        let mut idle = blank();
        notice.composite_over(&mut idle, W, H);
        assert_eq!(idle, blank(), "an unraised notice must draw nothing at all");
        assert!(!notice.is_up());

        notice.raise(content(), t0);
        assert!(notice.is_up());
        let mut raised = blank();
        notice.composite_over(&mut raised, W, H);
        assert_ne!(raised, blank(), "a raised notice must reach the panel");
        // ...and it really is the notice's ground, not stray text: the last
        // row of the view is inside the band by construction.
        assert_eq!(
            &row(&raised, H - 1, W)[..BYTES_PER_PIXEL],
            &GROUND,
            "the notice is anchored to the bottom edge"
        );
    }

    /// **The trusted band's rows are unreachable from this module** — the one
    /// property that would be a security regression rather than a cosmetic
    /// one (D-026: nothing may be composited into the band).
    ///
    /// Asserted at a view size where the notice is taller than the whole view,
    /// which is the only way the blit could ever be pushed upward into the
    /// band's rows.
    #[test]
    fn a_notice_can_never_paint_a_pixel_of_the_trusted_band() {
        let t0 = Instant::now();
        for height in [TRUST_BAND_HEIGHT, TRUST_BAND_HEIGHT + 1, 40, H] {
            let mut notice = CoreNotice::new();
            notice.raise(content(), t0);
            let mut view = vec![0u8; W as usize * height as usize * BYTES_PER_PIXEL];
            notice.composite_over(&mut view, W, height);
            for y in 0..TRUST_BAND_HEIGHT.min(height) {
                assert!(
                    row(&view, y, W).iter().all(|byte| *byte == 0),
                    "the notice reached band row {y} at view height {height}: the band is one \
                     colour nothing composites into, and every other trust claim hangs off it"
                );
            }
        }
    }

    /// **A notice expires**, and the expiry is what asks for the frame that
    /// takes it off the screen.
    #[test]
    fn a_notice_clears_itself_and_says_so_once() {
        let t0 = Instant::now();
        let mut notice = CoreNotice::new();
        notice.raise(content(), t0);

        assert_eq!(
            notice.until(),
            Some(t0 + NOTICE_DEADLINE),
            "an embedder with no frame clock of its own has to arm a timer against this; a \
             deadline it cannot read is a notice that stays on screen forever"
        );
        assert!(!notice.tick(t0 + NOTICE_DEADLINE - Duration::from_millis(1)));
        assert!(notice.is_up(), "it must still be up a millisecond early");
        // A second raise moves the deadline, so a timer armed against the
        // first one must be re-armed rather than allowed to clear the screen
        // early.
        let later = t0 + Duration::from_secs(5);
        notice.raise(content(), later);
        assert_eq!(notice.until(), Some(later + NOTICE_DEADLINE));
        assert!(!notice.tick(t0 + NOTICE_DEADLINE));
        notice.raise(content(), t0);
        assert!(
            notice.tick(t0 + NOTICE_DEADLINE),
            "the expiry must report a change, or nothing asks for the frame that erases it"
        );
        assert!(!notice.is_up());
        assert!(
            !notice.tick(t0 + NOTICE_DEADLINE + Duration::from_secs(60)),
            "an already-expired notice must not keep asking for frames"
        );

        let mut view = blank();
        notice.composite_over(&mut view, W, H);
        assert_eq!(view, blank(), "an expired notice must paint nothing");
    }

    /// **A resize re-rasterizes**, and a raise replaces rather than queues.
    ///
    /// Both halves are cache-key properties: a stale raster at the old width
    /// would be blitted at the new one, and a `raise` that did not bump the
    /// generation would leave the *first* message on screen while the state
    /// said the second.
    #[test]
    fn the_raster_is_keyed_on_width_and_on_what_is_being_said() {
        let t0 = Instant::now();
        let mut notice = CoreNotice::new();
        notice.raise(content(), t0);

        let narrow = notice.raster(320).expect("a raised notice rasterizes");
        let (narrow_w, narrow_h) = (narrow.width, narrow.height);
        assert_eq!(narrow_w, 320);
        let wide = notice.raster(1600).expect("a raised notice rasterizes");
        assert_eq!(wide.width, 1600, "a resize must re-rasterize");
        // The narrow band wraps onto more lines, so it is taller.
        assert!(
            narrow_h > wide.height,
            "narrow: {narrow_h}, wide: {}",
            wide.height
        );

        let before = notice.raster(1600).expect("still up").rgba.clone();
        notice.raise(
            NoticeContent::VtSwitchFailed {
                target: 9,
                own: None,
            },
            t0,
        );
        let after = notice.raster(1600).expect("still up").rgba.clone();
        assert_ne!(
            before, after,
            "a second raise must replace what is drawn, not be swallowed by the raster cache"
        );
    }

    /// **Nothing an app or a principal owns can travel onto this surface.**
    ///
    /// The variant's fields are integers, so this asserts the property that is
    /// actually at risk: the strings are `const` fragments of this file, and
    /// the numbers are formatted by the core.
    #[test]
    fn every_glyph_comes_from_this_file_or_from_an_integer() {
        let content = NoticeContent::VtSwitchFailed {
            target: 3,
            own: Some(2),
        };
        let headline = content.headline();
        assert!(headline.contains("VT 3"), "{headline}");
        let body = content.body();
        assert!(body.contains("Hold Esc"), "{body}");
        assert!(body.contains("SIGTERM"), "{body}");
        assert!(body.contains("VT 2"), "{body}");
        // With no resolved VT the notice says nothing about one rather than
        // inventing a number the human would then try to switch back to.
        let unknown = NoticeContent::VtSwitchFailed {
            target: 3,
            own: None,
        };
        assert!(
            !unknown.body().contains("vitrind is on"),
            "{}",
            unknown.body()
        );
    }
}
