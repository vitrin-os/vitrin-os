// SPDX-License-Identifier: MPL-2.0
//! The **cursor sprites**: the small crosshairs the core paints at a pointer
//! position, into **human-visible output only**.
//!
//! Two of them since WS-E.3.2 (issue #218), drawn by the same geometry and
//! separated by colour and size: the **agent** cursor (D-019), at an agent
//! principal's own pointer, and the **human** cursor, at the position the
//! physical pointer device has accumulated to. The second one exists only
//! because a bare-metal session has no host desktop to draw it — see
//! [`human_cursor_rects`], which carries the whole argument and the IDL
//! sentence it corrected.
//!
//! # Drawing is not delivery, and this module is only the drawing half
//!
//! Seat delivery to the shim stays exactly what D-017 says it is: **one
//! shared pointer position per realm view**, `vitrin_shim_seat` events
//! carrying `origin` and not principal identity. Per-principal *delivery* is
//! deferred to M2 and that deferral stands untouched — nothing here changes
//! what the confined app receives, in what order, or from whom. What it adds
//! is a display sprite, so a human watching the nested window can see where
//! an agent is pointing instead of being told to "watch the cursor move" at
//! a screen that never had one.
//!
//! The two halves are kept apart on purpose. The app's pointer is
//! [`crate::input::InputRouter::pointer`], written by **both** origins; the
//! sprite's position is [`crate::input::InputRouter::agent_pointer`], written
//! by the emulated origin alone. A sprite drawn on the shared position would
//! follow the human's physical mouse and lie about who is pointing.
//!
//! # Human-visible only, and never a capture
//!
//! This is applied at the output stage, downstream of the point where a
//! capture takes the bare realm view ([`crate::backend`]), exactly like the
//! consent overlay (P1.7.1) and the dead-man hold indicator (P1.7.3). So
//! `vitrin_view.frame_ready` cannot contain it — not because a flag is
//! checked, but because [`crate::scene::Scene::compose`] runs upstream of
//! every caller here. That is what makes the IDL's ordering invariant 4 (no
//! agent principal's cursor is composited into another principal's captured
//! frame) a rule the compositor is now actually held to, rather than one that
//! held vacuously because nothing drew a cursor at all.
//!
//! # Two geometry consumers, one geometry function
//!
//! [`agent_cursor_rects`] is the single description of the sprite, and both
//! human-visible presentation paths derive from it rather than restating it:
//! the CPU composite through [`composite_agent_cursor`], and the nested
//! zero-copy dmabuf path through [`crate::dmabuf::human_visible_frame`]'s
//! draw list (solid rectangles need no new GL machinery). This is the same
//! shape [`crate::dmabuf::trust_band_rect`] already uses for the trusted
//! band, and for the same reason: issue #85's bug was a second presentation
//! path that quietly did not draw what the first one did.
//!
//! # The sprite may never reach the trusted band
//!
//! Every rectangle is clipped to `y >= TRUST_BAND_HEIGHT` before it is
//! returned. An agent controls its own pointer position, so an unclipped
//! sprite at `y = 0` would let an agent paint into the one strip the human
//! reads this session's secret colour from (issue #85) — a new forgery
//! surface, not a cosmetic overlap. Clipping happens in the geometry
//! function, so both presentation paths inherit it and neither can forget.
//!
//! Integer math, const colours, no image assets, no allocation — the posture
//! [`crate::deadman::composite_hold_indicator`] established for a code-drawn
//! human-visible overlay, and [`crate::paint::canvas`]'s reasoning for why
//! a rasterizer dependency does not enter the TCB to make a shape prettier.

use crate::consent::TRUST_BAND_HEIGHT;
use crate::paint::canvas::{Canvas, Rect};

/// Half-length of an **agent** crosshair's arm, in view pixels.
const ARM: u32 = 9;
/// Half-length of the **human** crosshair's arm. Deliberately shorter than
/// [`ARM`]: the two sprites are on screen at the same time, and a human
/// glancing at the display has to be able to say which of them is theirs
/// without reading the colour of a 3-pixel bar. Size and colour both differ,
/// so either one alone suffices.
const HUMAN_ARM: u32 = 5;
/// Thickness of each bar. Odd, so the bars centre on the hotspot pixel
/// rather than straddling it.
const THICK: u32 = 3;
/// Half of [`THICK`], as the offset from the hotspot to a bar's first row.
const HALF: u32 = THICK / 2;
/// Thickness of the dark halo around the bright core, so the sprite reads
/// against a light *and* a dark app.
const EDGE: u32 = 1;

/// The halo: near-black, drawn first and under the core.
pub(crate) const AGENT_CURSOR_HALO: [u8; 4] = [0x08, 0x08, 0x0c, 0xff];
/// The core: a bright cyan nothing else in the human-visible composition
/// paints — the letterbox matte is dark, the hold indicator is amber, and
/// the trusted band is a solid full-width strip rather than a 21×21 cross,
/// so no glance confuses the two.
pub(crate) const AGENT_CURSOR_CORE: [u8; 4] = [0x4c, 0xf0, 0xff, 0xff];

/// The **human** cursor's core: plain white, which nothing else in the
/// human-visible composition paints as a small solid shape (the matte is
/// dark, the band is a full-width strip in this session's secret colour, the
/// hold indicator is amber, the agent's sprite is cyan). White rather than a
/// second saturated colour on purpose: the human's own pointer is the one
/// thing on the display that asserts nothing about authority, so it should
/// read as furniture rather than as a signal.
pub(crate) const HUMAN_CURSOR_CORE: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

/// How many solid rectangles one sprite costs: halo bar ×2, core bar ×2.
/// Public because the zero-copy draw list is a fixed-size array sized from
/// it.
pub(crate) const AGENT_CURSOR_RECTS: usize = 4;

/// The same, for the human sprite — the same crosshair shape, so the same
/// count. Named separately rather than reusing [`AGENT_CURSOR_RECTS`] at the
/// draw list's slot arithmetic so that changing one sprite's shape cannot
/// silently resize the other one's slots.
pub(crate) const HUMAN_CURSOR_RECTS: usize = 4;

/// One axis-aligned, solid-colour piece of the sprite, in **integer view
/// pixels**.
///
/// Integer rather than float on purpose: this is what enters the nested
/// backend's texture cache key (`TextureKey`), and an `f64` in a `PartialEq`
/// cache key is a bug waiting for a rounding change — the argument that
/// constant's `hold_bucket` field already makes in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub rgba: [u8; 4],
}

/// The sprite's hotspot in integer view pixels, or `None` for a position
/// that is not a number.
///
/// Also the quantizer the nested backend's texture key and its
/// damage-widening guard read: two positions with the same hotspot draw
/// byte-identical sprites, so they must not cost a re-upload, and two with
/// different hotspots must.
pub(crate) fn hotspot(x: f64, y: f64) -> Option<(i32, i32)> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let quantize = |v: f64| v.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    Some((quantize(x), quantize(y)))
}

/// Saturate an `i64` coordinate into the canvas domain. Only reachable with
/// an absurd position; a saturated coordinate is off-canvas, which every
/// consumer clips away — the same outcome the honest arithmetic gives.
fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The sprite's rectangles at `(x, y)` on a `width × height` human-visible
/// frame, in draw order (halo, then core on top).
///
/// `None` when there is nothing to draw into: a degenerate view, a view no
/// taller than the trusted band, or a position that is not a number. Every
/// returned rectangle already has its top edge clipped to the first row
/// below the band (module docs); a rectangle the clip empties comes back
/// with `h == 0`, which both consumers skip.
///
/// Rectangles may extend past the view's edges — a pointer near an edge
/// draws a partial cross, exactly as a real cursor does — so both consumers
/// clip in x/y as well.
pub(crate) fn agent_cursor_rects(
    width: u32,
    height: u32,
    x: f64,
    y: f64,
) -> Option<[CursorRect; AGENT_CURSOR_RECTS]> {
    crosshair_rects(width, height, x, y, ARM, AGENT_CURSOR_CORE)
}

/// The **human** pointer's rectangles, on [`agent_cursor_rects`]'s terms and
/// through the very same geometry — a shorter arm and a white core, and
/// nothing else different.
///
/// # Why the core draws this at all, when the IDL said it never would
///
/// `protocol/vitrin-v0.xml` stated flatly that *no* human cursor is
/// composited, "in nested operation the host desktop draws it outside the
/// realm view entirely". That sentence was true of the only two backends that
/// existed and is false of the third: on bare metal (WS-E.3.2, issue #218)
/// there is no host desktop, so either the core paints the human's pointer or
/// the human has none. The IDL sentence is now nested-conditional and this is
/// the code it was made conditional for.
///
/// **It changes nothing about capture.** This is applied at the same output
/// stage as the agent sprite, the consent card and the trusted band — past
/// [`crate::backend::human_visible_from_view`] — so it is excluded from
/// `vitrin_view.frame_ready` by the same structural argument, not by a check.
/// D-018's ordering invariant 4 is about *agent* cursors in *other*
/// principals' captures and is untouched: nothing an agent can observe
/// contains either sprite.
///
/// **Clipped below the trusted band, exactly like the agent's.** The clip is
/// in the shared geometry below, so this sprite inherits it rather than
/// deciding again — one band policy, not two. The cost is real and is not a
/// bug: a human whose pointer is inside the top [`TRUST_BAND_HEIGHT`] rows
/// sees no sprite there. Keeping one rule was preferred to giving the band
/// two correct appearances, since "the band always looks exactly like this"
/// is the only check a human has against a forged prompt.
pub(crate) fn human_cursor_rects(
    width: u32,
    height: u32,
    x: f64,
    y: f64,
) -> Option<[CursorRect; HUMAN_CURSOR_RECTS]> {
    crosshair_rects(width, height, x, y, HUMAN_ARM, HUMAN_CURSOR_CORE)
}

/// The one crosshair description both sprites are derived from: a halo cross
/// and a bright core cross, clipped below the trusted band.
///
/// Parameterised over the arm length and the core colour and over nothing
/// else, so the two sprites cannot drift in the property that matters — the
/// band clip — while still being told apart at a glance.
fn crosshair_rects(
    width: u32,
    height: u32,
    x: f64,
    y: f64,
    arm: u32,
    core: [u8; 4],
) -> Option<[CursorRect; 4]> {
    if width == 0 || height <= TRUST_BAND_HEIGHT {
        return None;
    }
    let (px, py) = hotspot(x, y)?;
    let (px, py) = (i64::from(px), i64::from(py));
    // The full span of a bar: both arms plus the hotspot's own pixel.
    let bar = 2 * arm + 1;
    let piece = |x: i64, y: i64, w: u32, h: u32, rgba: [u8; 4]| -> CursorRect {
        // THE band clip, applied once here so no presentation path can omit
        // it (module docs).
        let band = i64::from(TRUST_BAND_HEIGHT);
        let (y, h) = if y < band {
            let lost = (band - y).min(i64::from(h)) as u32;
            (band, h - lost)
        } else {
            (y, h)
        };
        CursorRect {
            x: clamp_i32(x),
            y: clamp_i32(y),
            w,
            h,
            rgba,
        }
    };
    Some([
        piece(
            px - i64::from(arm + EDGE),
            py - i64::from(HALF + EDGE),
            bar + 2 * EDGE,
            THICK + 2 * EDGE,
            AGENT_CURSOR_HALO,
        ),
        piece(
            px - i64::from(HALF + EDGE),
            py - i64::from(arm + EDGE),
            THICK + 2 * EDGE,
            bar + 2 * EDGE,
            AGENT_CURSOR_HALO,
        ),
        piece(px - i64::from(arm), py - i64::from(HALF), bar, THICK, core),
        piece(px - i64::from(HALF), py - i64::from(arm), THICK, bar, core),
    ])
}

/// Paint the agent cursor onto **human-visible** output, CPU path.
///
/// `view` must be `width * height * 4` bytes of RGBA8888, rows top-down —
/// the layout [`crate::scene::Scene::compose`] returns. A buffer that is not
/// that size, or a view with no room below the trusted band, draws nothing
/// rather than panicking inside a compositor loop.
///
/// The GPU counterpart is [`crate::dmabuf::human_visible_frame`]'s cursor
/// slots, which read the very same [`agent_cursor_rects`].
pub(crate) fn composite_agent_cursor(view: &mut [u8], width: u32, height: u32, x: f64, y: f64) {
    fill(view, width, height, agent_cursor_rects(width, height, x, y));
}

/// Paint the **human** cursor onto human-visible output, CPU path — the
/// counterpart of [`composite_agent_cursor`], on the same terms.
///
/// Reached only from a backend that owns the physical display outright (the
/// bare-metal DRM backend). The nested backend deliberately never calls it:
/// the host desktop is already drawing the human's pointer over the core's
/// window, and a second one would be two cursors for one mouse.
///
/// The GPU counterpart is [`crate::dmabuf::human_visible_frame`]'s human
/// cursor slots, which read the very same [`human_cursor_rects`].
pub(crate) fn composite_human_cursor(view: &mut [u8], width: u32, height: u32, x: f64, y: f64) {
    fill(view, width, height, human_cursor_rects(width, height, x, y));
}

/// Blit one sprite's rectangles onto a CPU canvas. Shared by both
/// compositors so a buffer whose length does not match its dimensions is
/// refused in one place rather than two.
fn fill(view: &mut [u8], width: u32, height: u32, rects: Option<[CursorRect; 4]>) {
    let Some(rects) = rects else {
        return;
    };
    let Some(mut canvas) = Canvas::new(view, width, height) else {
        return;
    };
    for rect in rects {
        canvas.fill_rect(
            Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            },
            rect.rgba,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::BYTES_PER_PIXEL;

    const W: u32 = 64;
    const H: u32 = 48;

    fn blank() -> Vec<u8> {
        vec![0u8; (W * H) as usize * BYTES_PER_PIXEL]
    }

    fn count(px: &[u8], rgba: [u8; 4]) -> usize {
        px.chunks_exact(BYTES_PER_PIXEL)
            .filter(|pixel| *pixel == rgba)
            .count()
    }

    /// The model for this test is `deadman.rs`'s
    /// `the_indicator_paints_a_bar_that_grows_with_progress`: count the
    /// pixels the sprite actually filled, assert the rows it must not touch
    /// are untouched, and feed it degenerate geometry.
    #[test]
    fn the_agent_cursor_paints_a_crosshair_where_the_pointer_is() {
        // Well inside the view, so nothing clips and the count is the
        // sprite's whole area: two 19x3 bars minus their 3x3 overlap.
        let expected_core = (2 * ARM + 1) * THICK * 2 - THICK * THICK;
        let mut px = blank();
        composite_agent_cursor(&mut px, W, H, 32.0, 24.0);
        assert_eq!(count(&px, AGENT_CURSOR_CORE), expected_core as usize);
        // The halo is a ring around the core, so it is the halo bars' area
        // minus every pixel the core overdrew.
        let halo_area = (2 * ARM + 1 + 2 * EDGE) * (THICK + 2 * EDGE) * 2
            - (THICK + 2 * EDGE) * (THICK + 2 * EDGE);
        assert_eq!(
            count(&px, AGENT_CURSOR_HALO),
            (halo_area - expected_core) as usize
        );

        // It really is at the hotspot: the centre pixel is core-coloured,
        // and a pixel a whole sprite away is untouched.
        let at = |x: u32, y: u32| {
            let off = (y as usize * W as usize + x as usize) * BYTES_PER_PIXEL;
            px[off..off + BYTES_PER_PIXEL].to_vec()
        };
        assert_eq!(at(32, 24), AGENT_CURSOR_CORE.to_vec());
        assert_eq!(at(32 - 2 * (ARM + EDGE), 24), vec![0, 0, 0, 0]);

        // Moving the pointer moves the sprite and leaves nothing behind: a
        // fresh buffer at a new position has its core at the new hotspot and
        // nothing at the old one.
        let mut moved = blank();
        composite_agent_cursor(&mut moved, W, H, 12.0, 40.0);
        let at_moved = |x: u32, y: u32| {
            let off = (y as usize * W as usize + x as usize) * BYTES_PER_PIXEL;
            moved[off..off + BYTES_PER_PIXEL].to_vec()
        };
        assert_eq!(at_moved(12, 40), AGENT_CURSOR_CORE.to_vec());
        assert_eq!(at_moved(32, 24), vec![0, 0, 0, 0]);
    }

    /// **The sprite can never reach the trusted band**, at any position an
    /// agent can ask for — including one aimed straight at row 0, which is
    /// what an agent would use to paint over the strip the human reads this
    /// session's secret colour from (issue #85).
    #[test]
    fn the_agent_cursor_never_paints_into_the_trusted_band() {
        let band_bytes = TRUST_BAND_HEIGHT as usize * W as usize * BYTES_PER_PIXEL;
        for y in [-50.0, -1.0, 0.0, 1.0, 4.0, 7.0, 8.0, 9.0, 24.0] {
            let mut px = blank();
            composite_agent_cursor(&mut px, W, H, 32.0, y);
            assert!(
                px[..band_bytes].iter().all(|&b| b == 0),
                "the sprite reached the trusted band's rows at y={y}"
            );
        }
        // ...and at y just below the band it *is* drawn, so the assertion
        // above is not passing because nothing is ever drawn at all.
        let mut px = blank();
        composite_agent_cursor(&mut px, W, H, 32.0, 9.0);
        assert!(count(&px, AGENT_CURSOR_CORE) > 0);
    }

    /// Degenerate geometry is survivable, and a buffer whose length does not
    /// match its dimensions draws nothing rather than indexing past it.
    #[test]
    fn degenerate_geometry_draws_nothing_and_does_not_panic() {
        composite_agent_cursor(&mut [], 0, 0, 0.0, 0.0);
        let mut tiny = vec![0u8; BYTES_PER_PIXEL];
        composite_agent_cursor(&mut tiny, 1, 1, 0.0, 0.0);
        assert_eq!(tiny, vec![0u8; BYTES_PER_PIXEL]);
        // A view no taller than the band has no row the sprite may use.
        let mut short = vec![0u8; W as usize * TRUST_BAND_HEIGHT as usize * BYTES_PER_PIXEL];
        composite_agent_cursor(&mut short, W, TRUST_BAND_HEIGHT, 32.0, 4.0);
        assert!(short.iter().all(|&b| b == 0));
        // A mismatched buffer is refused by the canvas, not indexed.
        let mut wrong = vec![0u8; 7];
        composite_agent_cursor(&mut wrong, W, H, 32.0, 24.0);
        assert_eq!(wrong, vec![0u8; 7]);
        // Non-finite input has no hotspot and draws nothing.
        for (x, y) in [(f64::NAN, 24.0), (32.0, f64::INFINITY)] {
            assert!(hotspot(x, y).is_none());
            let mut px = blank();
            composite_agent_cursor(&mut px, W, H, x, y);
            assert!(px.iter().all(|&b| b == 0));
        }
        // An absurd position saturates into the canvas domain and clips away
        // rather than wrapping into the middle of the view.
        let mut px = blank();
        composite_agent_cursor(&mut px, W, H, 1e300, 1e300);
        assert!(px.iter().all(|&b| b == 0));
    }

    /// **The human's sprite is drawn, is distinguishable from an agent's,
    /// and obeys the same band clip** (WS-E.3.2, issue #218).
    ///
    /// Three properties in one test because they are one decision: a second
    /// sprite that a human cannot tell from an agent's would make D-019's
    /// signal worthless, and a second sprite that could paint into the
    /// trusted band would give the band two correct appearances.
    #[test]
    fn the_human_cursor_is_drawn_distinctly_and_never_reaches_the_band() {
        let mut px = blank();
        composite_human_cursor(&mut px, W, H, 32.0, 24.0);
        // Drawn at all, in its own colour, and NOT in the agent's.
        assert!(count(&px, HUMAN_CURSOR_CORE) > 0);
        assert_eq!(count(&px, AGENT_CURSOR_CORE), 0);
        // Smaller than an agent's, so colour is not the only difference.
        let mut agent = blank();
        composite_agent_cursor(&mut agent, W, H, 32.0, 24.0);
        assert!(count(&px, HUMAN_CURSOR_CORE) < count(&agent, AGENT_CURSOR_CORE));

        // The band clip is the shared geometry's, so it applies here too --
        // including at the position an unclipped sprite would use to sit on
        // the strip the human reads this session's secret colour from.
        let band_bytes = TRUST_BAND_HEIGHT as usize * W as usize * BYTES_PER_PIXEL;
        for y in [-20.0, 0.0, 4.0, 7.0] {
            let mut px = blank();
            composite_human_cursor(&mut px, W, H, 32.0, y);
            assert!(
                px[..band_bytes].iter().all(|&b| b == 0),
                "the human sprite reached the trusted band's rows at y={y}"
            );
        }

        // Degenerate input is survivable on this path too.
        composite_human_cursor(&mut [], 0, 0, 0.0, 0.0);
        let mut wrong = vec![0u8; 7];
        composite_human_cursor(&mut wrong, W, H, 32.0, 24.0);
        assert_eq!(wrong, vec![0u8; 7]);
    }

    /// The hotspot is what the nested backend's cache key quantizes on, so
    /// it must be stable within a pixel and different across one.
    #[test]
    fn the_hotspot_quantizes_to_whole_view_pixels() {
        assert_eq!(hotspot(32.1, 24.4), Some((32, 24)));
        assert_eq!(hotspot(32.4, 23.9), Some((32, 24)));
        assert_ne!(hotspot(32.6, 24.0), hotspot(32.0, 24.0));
        assert_eq!(hotspot(-3.2, -0.4), Some((-3, 0)));
    }
}
