//! Single-maximized placement — the **entire** layout policy of the MVP
//! core, quarantined in this module.
//!
//! **NOT THE CORE'S JOB LONG-TERM.** Window-management policy — layout,
//! stacking, decoration, theming — never belongs in the trusted core (PRD
//! §5.1 invariant; PRD Doc 2 §2, the Nitpicker/Qubes lesson): the core does
//! mediation, not window management, and shell UX lives in unprivileged
//! components outside the TCB (e.g. the horizon-tier mission-control
//! shell). This module exists only because the MVP has exactly one realm
//! with exactly one surface, so the "policy" degenerates to *center the
//! surface, unscaled* — plan P1.3.3's "trivial layout". It must never
//! grow: tiling, stacking order, focus policy, decorations, or any second
//! placement rule are a design smell here and belong outside the core.
//!
//! No decorations by construction: the shim disables server-side
//! decorations via xdg-decoration (P1.6.1) and the core draws nothing
//! around the surface — [`place`] returns a bare offset, there is no frame
//! to compute.

/// Where the (single, maximized) surface's top-left corner lands in the
/// view, in view pixels. May be negative: a surface larger than the view
/// is center-cropped by the compositor ([`crate::scene`]'s letterbox
/// decision — 1:1 always, never scaled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    pub x: i64,
    pub y: i64,
}

/// Place a `surface`-sized buffer in a `view`-sized realm view: centered,
/// 1:1. `i64` math so no `u32` pair can overflow or wrap. Odd differences
/// truncate toward zero (the extra pixel goes right/bottom) — any fixed
/// choice is fine as long as it is deterministic.
pub(crate) fn place(view: (u32, u32), surface: (u32, u32)) -> Placement {
    Placement {
        x: (i64::from(view.0) - i64::from(surface.0)) / 2,
        y: (i64::from(view.1) - i64::from(surface.1)) / 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit_places_at_origin() {
        assert_eq!(place((1280, 800), (1280, 800)), Placement { x: 0, y: 0 });
    }

    #[test]
    fn smaller_surface_is_centered() {
        assert_eq!(place((100, 80), (40, 20)), Placement { x: 30, y: 30 });
    }

    #[test]
    fn larger_surface_gets_a_negative_offset() {
        assert_eq!(place((40, 30), (60, 50)), Placement { x: -10, y: -10 });
    }

    #[test]
    fn odd_differences_truncate_toward_zero() {
        assert_eq!(place((101, 101), (100, 100)), Placement { x: 0, y: 0 });
        assert_eq!(place((100, 100), (103, 105)), Placement { x: -1, y: -2 });
    }

    #[test]
    fn extreme_sizes_do_not_overflow() {
        let p = place((u32::MAX, 1), (1, u32::MAX));
        assert_eq!(p.x, (i64::from(u32::MAX) - 1) / 2);
        assert_eq!(p.y, (1 - i64::from(u32::MAX)) / 2);
    }
}
