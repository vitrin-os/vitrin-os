// SPDX-License-Identifier: MPL-2.0
//! Which keys drive the picker, and the one that deliberately does not.
//!
//! # Escape is absent, and its absence is the decision
//!
//! Escape is the dead-man chord. `consent/grab.rs` records that no key answers
//! a consent prompt and that Escape belongs to the chord alone; a picker adds
//! keys but must not take that one, because a human reaching for the chord
//! during a designation is a human trying to stop something, and a picker that
//! swallowed it would turn the panic key into a dialog dismissal.
//!
//! Escape is still *consumed* by the grab like every other key — the picker
//! holds an exclusive input grab, so nothing reaches the app behind it — it is
//! simply not decoded here. The dead-man watcher is unaffected either way:
//! `PreemptionHook::observe` sees every event at intake, before and regardless
//! of gating.
//!
//! Cancelling is therefore a button on the surface, reachable by Tab, not a
//! key with a hidden meaning.
//!
//! # Why a table of raw keysyms and not a keymap lookup
//!
//! `vitrin_shim_seat.key` carries a keysym the core resolved, and the core's
//! layout-invariant table is what resolves it. Navigation keys — arrows,
//! Return, Tab, Backspace — occupy the function-key keysym block, which is
//! layout-invariant by construction: a Turkish, Japanese or Dvorak layout all
//! report `0xff52` for Up. So the table below is not an approximation of a
//! keymap, it is the part of the keysym space that no layout moves.

#![allow(dead_code)]

use crate::paint::script::{route, Route};

/// One step a key asks the picker to take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PickerStep {
    /// Move the selection within the listing.
    Move(Motion),
    /// Move keyboard focus between the listing and the surface's buttons.
    FocusNext,
    FocusPrev,
    /// Append one character to the type-to-filter query.
    Filter(char),
    /// Remove the last character of the query.
    FilterBackspace,
    /// Commit the selected row.
    Confirm,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Motion {
    Up,
    Down,
    /// Leave the current directory, or descend into the selected one.
    Out,
    In,
    PageUp,
    PageDown,
    First,
    Last,
}

impl Motion {
    /// Every motion the picker has.
    ///
    /// The one enumeration, on [`crate::grants::PersistenceRung::ALL`]'s
    /// precedent and for its reason: anything that has to cover the whole set
    /// -- today the out-of-process injector's word table and its round-trip
    /// test -- reads it from here rather than restating it, so a motion that
    /// becomes representable is covered without anyone remembering. Kept
    /// beside the type so adding a variant without extending this is a
    /// visible omission rather than an invisible one.
    pub(crate) const ALL: [Motion; 8] = [
        Motion::Up,
        Motion::Down,
        Motion::Out,
        Motion::In,
        Motion::PageUp,
        Motion::PageDown,
        Motion::First,
        Motion::Last,
    ];
}

// The layout-invariant function-key block.
const XK_BACKSPACE: u32 = 0xff08;
const XK_TAB: u32 = 0xff09;
const XK_RETURN: u32 = 0xff0d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_HOME: u32 = 0xff50;
const XK_LEFT: u32 = 0xff51;
const XK_UP: u32 = 0xff52;
const XK_RIGHT: u32 = 0xff53;
const XK_DOWN: u32 = 0xff54;
const XK_PAGE_UP: u32 = 0xff55;
const XK_PAGE_DOWN: u32 = 0xff56;
const XK_END: u32 = 0xff57;
const XK_KP_ENTER: u32 = 0xff8d;
const XK_ISO_LEFT_TAB: u32 = 0xfe20;

/// Keysyms in this range are the Unicode scalar plus this bias.
const XK_UNICODE_BASE: u32 = 0x0100_0000;

/// What `keysym` asks the picker to do, or `None` if it asks for nothing.
///
/// Total, and `None` by default: a keysym nobody considered does nothing
/// rather than doing whatever the nearest arm happens to be.
pub(crate) fn decode(keysym: u32) -> Option<PickerStep> {
    // Named first so the reader meets the exclusion where it is decided, not
    // as an absence they have to notice. Listing it explicitly also means a
    // later edit that adds a catch-all cannot swallow it by accident.
    if keysym == XK_ESCAPE {
        return None;
    }
    match keysym {
        XK_UP => Some(PickerStep::Move(Motion::Up)),
        XK_DOWN => Some(PickerStep::Move(Motion::Down)),
        XK_LEFT => Some(PickerStep::Move(Motion::Out)),
        XK_RIGHT => Some(PickerStep::Move(Motion::In)),
        XK_PAGE_UP => Some(PickerStep::Move(Motion::PageUp)),
        XK_PAGE_DOWN => Some(PickerStep::Move(Motion::PageDown)),
        XK_HOME => Some(PickerStep::Move(Motion::First)),
        XK_END => Some(PickerStep::Move(Motion::Last)),
        XK_RETURN | XK_KP_ENTER => Some(PickerStep::Confirm),
        XK_TAB => Some(PickerStep::FocusNext),
        XK_ISO_LEFT_TAB => Some(PickerStep::FocusPrev),
        XK_BACKSPACE => Some(PickerStep::FilterBackspace),
        _ => printable(keysym).map(PickerStep::Filter),
    }
}

/// The character a keysym types, if it types one the picker may draw.
///
/// Two encodings reach here: Latin-1 keysyms, which are their own codepoint,
/// and the `0x01000000 + scalar` form for everything else.
///
/// The result is filtered through [`crate::paint::script::route`], so the
/// query line can only ever contain characters the renderer draws as
/// themselves. That matters less than it does for a filename — the human
/// typed this, so nobody is spoofing them with it — but a query that could
/// hold an undrawable character would need the transcription machinery too,
/// and an echo that silently dropped characters would be worse than one that
/// never accepts them.
fn printable(keysym: u32) -> Option<char> {
    let scalar = match keysym {
        // Latin-1, minus the C0/C1 control bands.
        0x20..=0x7E | 0xA0..=0xFF => keysym,
        k if k >= XK_UNICODE_BASE => k - XK_UNICODE_BASE,
        _ => return None,
    };
    let ch = char::from_u32(scalar)?;
    (route(ch) == Route::Vector).then_some(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The one that must not decode.**
    #[test]
    fn escape_asks_the_picker_for_nothing() {
        assert_eq!(
            decode(XK_ESCAPE),
            None,
            "Escape is the dead-man chord. A picker that decoded it would turn \
             the key a human reaches for to stop something into a dialog \
             dismissal."
        );
    }

    #[test]
    fn the_navigation_block_decodes() {
        assert_eq!(decode(XK_UP), Some(PickerStep::Move(Motion::Up)));
        assert_eq!(decode(XK_DOWN), Some(PickerStep::Move(Motion::Down)));
        assert_eq!(decode(XK_RETURN), Some(PickerStep::Confirm));
        assert_eq!(decode(XK_KP_ENTER), Some(PickerStep::Confirm));
        assert_eq!(decode(XK_TAB), Some(PickerStep::FocusNext));
        assert_eq!(decode(XK_ISO_LEFT_TAB), Some(PickerStep::FocusPrev));
        assert_eq!(decode(XK_BACKSPACE), Some(PickerStep::FilterBackspace));
    }

    #[test]
    fn printable_keys_filter() {
        assert_eq!(decode(u32::from(b'a')), Some(PickerStep::Filter('a')));
        assert_eq!(decode(u32::from(b'Z')), Some(PickerStep::Filter('Z')));
        assert_eq!(decode(u32::from(b'.')), Some(PickerStep::Filter('.')));
        // Latin-1 as its own codepoint, and the Unicode-biased form, agree.
        assert_eq!(decode(0xE9), Some(PickerStep::Filter('é')));
        assert_eq!(
            decode(XK_UNICODE_BASE + 0xE9),
            Some(PickerStep::Filter('é'))
        );
    }

    /// The query line inherits the renderer's alphabet rather than widening it.
    #[test]
    fn a_key_the_renderer_cannot_draw_types_nothing() {
        // Hebrew alef: covered by the face, deliberately not routed, because
        // drawing it would need bidi.
        assert_eq!(decode(XK_UNICODE_BASE + 0x05D0), None);
        // A combining mark: no mark positioning, so it would render beside its
        // base rather than over it.
        assert_eq!(decode(XK_UNICODE_BASE + 0x0301), None);
        // A zero-width space carries no ink and no advance: `a\u{200B}b` would
        // render as `ab`, so an echo containing one would lie about itself.
        assert_eq!(decode(XK_UNICODE_BASE + 0x200B), None);
    }

    /// A keysym nobody considered does nothing.
    #[test]
    fn an_unconsidered_keysym_decodes_to_nothing() {
        for keysym in [0xffeb_u32, 0xff61, 0xffc2, 0xffe1, 0x0] {
            assert_eq!(decode(keysym), None, "keysym {keysym:#x} must be inert");
        }
    }
}
