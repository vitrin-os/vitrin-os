// SPDX-License-Identifier: MPL-2.0
//! **The rasterizer the core's trusted surfaces are drawn with**: a borrowed
//! RGBA8888 [`canvas`], a glyph engine over one embedded face ([`text`]), and
//! the one placement function ([`centered`]) every core-drawn card is
//! positioned by.
//!
//! # Why this is its own module rather than part of `consent`
//!
//! It was part of [`crate::consent`] until WS-E.2.2 (issue #214), because for
//! three issues the consent prompt was the only thing the core drew. It is not
//! any more: [`crate::deadman`]'s hold indicator, [`crate::cursor`]'s agent
//! sprite, [`crate::attention`]'s marker and now [`crate::lock`]'s lock screen
//! all reach for the same primitives, and three of those already did it by
//! importing `crate::consent::canvas` — a path that says the dead-man switch
//! draws with the consent surface's private tools, which is not what is
//! happening and not what should be true.
//!
//! So the shared pieces were **moved**, not copied. That is the rule issue #214
//! set for the lock screen ("factor the shared pieces out of `consent::` rather
//! than copying them") and it is the rule with teeth: two rasterizers in one TCB
//! is two places for a golden to drift, two places for a clipping bug, and two
//! answers to "where is a core-drawn card placed". There is one of each.
//!
//! **Nothing about the move changes a pixel.** The files are the same files
//! (`git mv`), the font is the same embedded constant, and the goldens
//! (`tests/golden/consent_prompt.txt`, `tests/golden/lock_screen.txt`) are the
//! check that says so.
//!
//! # What this module is deliberately not
//!
//! Not a 2D graphics library, and not on a path to becoming one. There are no
//! paths, no curves, no transforms, no shaping, no bidi and no font fallback;
//! `crates/vitrin-core/Cargo.toml` records at length why `tiny-skia` (+7
//! crates) and `cosmic-text` (a whole shaping stack) were declined for the TCB.
//! Everything here is integer byte assembly over one scalar glyph rasterizer.
//!
//! # It confers nothing and sees nothing
//!
//! No type here holds session state, reads a clock, or knows what it is
//! drawing. That is what makes it safe for the lock screen and the consent
//! surface to share it: the *authority* claims live in [`crate::consent`] and
//! [`crate::lock`], and this module cannot participate in one.

pub(crate) mod canvas;
pub(crate) mod text;

/// Center a `w x h` box in a `view_w x view_h` view. Truncating division, so
/// an odd leftover pixel lands on the right/bottom — deterministically, which
/// is what the goldens need.
///
/// **One placement function for every core-drawn card**, which is the property
/// [`crate::consent::grab`] leans on: the grab hit-tests a click by deriving
/// the card's origin itself rather than reading the compositor's, and the two
/// agree because neither computes a layout the other does not. The lock screen
/// draws through the same call for the same reason — it is the geometry a
/// future keyboard-answerable surface would hit-test against.
///
/// A view smaller than the box yields a negative origin and every drawing
/// primitive clips, so the box's middle is what shows. That degradation is
/// argued where it has consequences
/// ([`crate::consent::ConsentSurface::card_origin`]).
pub(crate) fn centered(w: u32, h: u32, view_w: u32, view_h: u32) -> (i32, i32) {
    (
        (view_w as i64 - w as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32 / 2,
        (view_h as i64 - h as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32 / 2,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_is_centered_with_the_odd_pixel_pushed_right_and_down() {
        assert_eq!(centered(100, 50, 300, 200), (100, 75));
        // Odd leftover: truncating division puts it on the right/bottom.
        assert_eq!(centered(10, 10, 21, 21), (5, 5));
    }

    #[test]
    fn a_box_larger_than_the_view_overhangs_rather_than_clamping() {
        let (x, y) = centered(560, 400, 320, 200);
        assert!(x < 0 && y < 0, "the card overhangs a view this small");
    }

    #[test]
    fn extreme_dimensions_saturate_instead_of_overflowing() {
        // `u32::MAX` on both axes: the i64 intermediate cannot wrap, and the
        // clamp keeps the result inside i32 rather than panicking in a
        // compositor loop.
        let (x, y) = centered(u32::MAX, u32::MAX, 0, 0);
        assert!(x < 0 && y < 0);
        let (x, y) = centered(0, 0, u32::MAX, u32::MAX);
        assert!(x > 0 && y > 0);
    }
}
