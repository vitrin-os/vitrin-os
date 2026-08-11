// SPDX-License-Identifier: MPL-2.0
//! Layout and rasterization of the lock card (WS-E.2.2, issue #214):
//! [`rasterize`] turns one [`LockContent`] into an opaque RGBA8888 buffer.
//!
//! # It is the consent card's sibling, not its copy
//!
//! Every primitive comes from [`crate::paint`] — the same canvas, the same
//! embedded face, the same integer-only compositing — which is the whole reason
//! that module exists (issue #214: "factor the shared pieces out of `consent::`
//! rather than copying them"). What differs is content and one structural fact:
//!
//! - **There are no buttons.** The consent card is answered with a pointer and
//!   deliberately refuses the keyboard ([`crate::consent::grab`]'s keyboard
//!   section: type-through would turn an in-flight keystroke into a grant). The
//!   lock card is the exact inverse — it is answered with the keyboard and
//!   deliberately refuses the pointer, because there is no click that could
//!   mean "this is me" and a lock screen with a clickable affordance is a lock
//!   screen a cat unlocks. So there is no [`ChoiceBox`]-equivalent here, and
//!   nothing to hit-test.
//!
//!   [`ChoiceBox`]: crate::consent::render::ChoiceBox
//! - **It is wider and shorter.** It has one job and says four things.
//!
//! # The copy is the honest part, and it took the most deciding
//!
//! A lock screen makes an authority claim, and on this stack the claim that is
//! *true* is narrower than the one a human will assume. The card therefore says
//! what the lock does — takes the keyboard and the mouse away from every realm —
//! and says, on the card rather than only in `docs/book/src/limits.md`, that it
//! does **not** stop an agent that already holds a grant. That is the owner's
//! decision of 2026-08-08 (D-025), and the alternative — a lock screen that
//! implies a session is frozen while an agent keeps capturing it — is the exact
//! honesty failure this repository's docs are written to avoid.
//!
//! It also names the instrument that *does* stop everything: the dead-man
//! chord, which still arms and fires while the lock consumes every event
//! ([`crate::lock::gate`]). A surface that tells a human "this does not stop
//! agents" without telling them what does is a warning they cannot act on,
//! which [`crate::consent::render`] already rejects in as many words.
//!
//! # Every string here is a `const` or a typed core value
//!
//! [`LockContent`] has no string field ([`crate::lock`]'s docs), so the only
//! characters this module can draw are the constants below, a
//! [`RealmId`](crate::grants::RealmId) out of the operator's `realm.toml`, and a
//! chord name out of a closed vocabulary. [`crate::paint::text`] substitutes
//! anything outside ASCII printables at the last moment regardless.

use crate::grants::RealmId;
use crate::paint::canvas::{Canvas, Rect};
use crate::paint::text::Text;
use crate::scene::BYTES_PER_PIXEL;

use super::{LockCause, LockContent, UnlockMethod};

// ---------------------------------------------------------------------------
// Type sizes
// ---------------------------------------------------------------------------

const TITLE_PX: f32 = 22.0;
const LABEL_PX: f32 = 11.0;
const VALUE_PX: f32 = 14.0;

// ---------------------------------------------------------------------------
// Geometry (output pixels)
// ---------------------------------------------------------------------------

/// Card width. Wider than the consent card's 560 because the honesty lines are
/// sentences rather than field values, and a sentence that wraps to four lines
/// reads as fine print — which is the one thing this particular sentence must
/// not read as.
pub(crate) const CARD_WIDTH: u32 = 640;
const BORDER: u32 = 2;
const PAD_X: u32 = 28;
const PAD_TOP: u32 = 24;
const PAD_BOTTOM: u32 = 24;
const GROUP_GAP: u32 = 14;
const LABEL_GAP: u32 = 2;
const RULE_H: u32 = 1;
/// How many lines one wrapped block may run to before it is truncated with a
/// visible marker ([`Text::wrap`]).
const MAX_VALUE_LINES: usize = 4;

const CONTENT_X: u32 = BORDER + PAD_X;
const CONTENT_W: u32 = CARD_WIDTH - 2 * CONTENT_X;

// ---------------------------------------------------------------------------
// Palette (RGB; the card is opaque, so no alpha travels with these)
// ---------------------------------------------------------------------------

/// The full-screen cover the lock paints before the card.
///
/// **Opaque, not a scrim.** The consent card darkens the realm view because
/// *which* realm is being asked about is part of the decision; the lock card
/// covers it completely because the whole point is that whatever was on screen
/// is not on screen any more. A human who walks away must not leave their mail
/// legible behind a 60%-black wash.
pub(crate) const COVER_RGBA: [u8; 4] = [0x0a, 0x0b, 0x0f, 0xff];

/// Card background: a shade above the cover, so the card reads as a panel and
/// not as a hole.
const CARD_BG: [u8; 4] = [0x16, 0x18, 0x1f, 0xff];
/// The border. Deliberately a **different** hue from the consent card's blue
/// accent: the two core-drawn cards must not be mistakable for one another at a
/// glance, because one of them asks for authority and the other asks for a
/// passphrase.
const ACCENT: [u8; 4] = [0xd0, 0x9a, 0x3c, 0xff];
const TITLE_FG: [u8; 3] = [0xf6, 0xf7, 0xfa];
const MUTED_FG: [u8; 3] = [0x93, 0x9e, 0xb0];
const VALUE_FG: [u8; 3] = [0xe4, 0xe9, 0xf2];
/// The honesty block, drawn in the accent hue rather than the muted grey: it is
/// the one thing on this card a human is most likely to be wrong about, so it
/// is not styled as fine print.
const WARN_FG: [u8; 3] = [0xe8, 0xba, 0x62];
const RULE_RGBA: [u8; 4] = [0x2b, 0x30, 0x3b, 0xff];

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

const TITLE: &str = "Session locked";
/// What the lock actually did. Present tense and mechanical: it names the thing
/// that is true (physical input goes nowhere) rather than a feeling ("your
/// session is secure"), which is a claim this stack cannot make.
const SUBTITLE: &str = "Your keyboard and mouse reach no application until you unlock.";

const LABEL_LOCKED_BY: &str = "Locked by";
const LABEL_REALMS: &str = "Realms held";
const LABEL_UNLOCK: &str = "To unlock";
const LABEL_STILL_RUNNING: &str = "Still running";

const CAUSE_CHORD: &str = "the lock chord";
const CAUSE_IDLE: &str = "no physical input for the configured idle time";
/// The seat-change cause (issue #246). It names the **event**, not a gesture:
/// a session can be deactivated for reasons this core never sees, so the card
/// says what is certainly true and the policy that turned it into a lock is
/// named beside it.
const CAUSE_SEAT: &str = "the seat went to another VT, and this session is set to lock when it \
                          does";

const UNLOCK_PASSPHRASE: &str =
    "Type the session passphrase and press Enter. Backspace edits; Escape clears.";
/// The no-passphrase build's instruction, and it says what it is.
const UNLOCK_ANY: &str = "Press Enter. No passphrase is configured, so this is a privacy \
                          screen and not an authentication boundary.";

/// The honesty block (module docs). Two sentences, in this order on purpose:
/// the surprising fact first, the instrument that answers it second.
const STILL_RUNNING_1: &str = "Agents holding grants keep observing and acting in these realms. \
                               A lock takes YOUR input away, not their authority.";
const STILL_RUNNING_2_PREFIX: &str = "To revoke every grant in the session, hold ";
const STILL_RUNNING_2_SUFFIX: &str = " - the dead-man chord still works while locked.";

/// What is drawn when a session holds no realms at all.
const NO_REALMS: &str = "none";

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// A rasterized lock card: opaque, tightly packed RGBA8888, rows top-down.
#[derive(Debug, Clone)]
pub(crate) struct Card {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// One laid-out element of the card, in vertical order.
enum Row {
    Space(u32),
    Rule,
    Line { text: String, px: f32, rgb: [u8; 3] },
}

/// Lay out and draw `content`.
///
/// Two passes over one row list — measure for the height, then draw — the same
/// shape [`crate::consent::render::rasterize`] uses, so a card is exactly as
/// tall as its content and nothing is padded to a guessed maximum.
pub(crate) fn rasterize(content: &LockContent) -> Card {
    let mut text = Text::new();
    let rows = rows(content, &mut text);

    let height = PAD_TOP
        + PAD_BOTTOM
        + 2 * BORDER
        + rows.iter().map(|row| row_height(row, &text)).sum::<u32>();

    let mut rgba = vec![0u8; CARD_WIDTH as usize * height as usize * BYTES_PER_PIXEL];
    // A canvas of the exact size just allocated cannot be refused; the `else`
    // arm keeps a core bug from becoming a panic in a compositor loop.
    let Some(mut canvas) = Canvas::new(&mut rgba, CARD_WIDTH, height) else {
        return Card {
            rgba,
            width: CARD_WIDTH,
            height,
        };
    };

    canvas.fill_rect(
        Rect {
            x: 0,
            y: 0,
            w: CARD_WIDTH,
            h: height,
        },
        CARD_BG,
    );
    canvas.stroke_rect(
        Rect {
            x: 0,
            y: 0,
            w: CARD_WIDTH,
            h: height,
        },
        ACCENT,
        BORDER,
    );

    let mut y = (BORDER + PAD_TOP) as i32;
    for row in &rows {
        match row {
            Row::Space(_) => {}
            Row::Rule => canvas.fill_rect(
                Rect {
                    x: CONTENT_X as i32,
                    y,
                    w: CONTENT_W,
                    h: RULE_H,
                },
                RULE_RGBA,
            ),
            Row::Line { text: s, px, rgb } => {
                let metrics = text.line_metrics(*px);
                text.draw(
                    &mut canvas,
                    s,
                    *px,
                    CONTENT_X as i32,
                    y + metrics.ascent as i32,
                    *rgb,
                );
            }
        }
        y += row_height(row, &text) as i32;
    }

    Card {
        rgba,
        width: CARD_WIDTH,
        height,
    }
}

/// The card's rows, top to bottom.
fn rows(content: &LockContent, text: &mut Text) -> Vec<Row> {
    let mut rows = Vec::new();
    let block = |rows: &mut Vec<Row>, text: &mut Text, s: &str, px: f32, rgb: [u8; 3]| {
        for wrapped in text.wrap(s, px, CONTENT_W, MAX_VALUE_LINES) {
            rows.push(Row::Line {
                text: wrapped,
                px,
                rgb,
            });
        }
    };

    rows.push(Row::Line {
        text: TITLE.to_string(),
        px: TITLE_PX,
        rgb: TITLE_FG,
    });
    rows.push(Row::Space(6));
    block(&mut rows, text, SUBTITLE, LABEL_PX, MUTED_FG);
    rows.push(Row::Space(GROUP_GAP));
    rows.push(Row::Rule);
    rows.push(Row::Space(GROUP_GAP));

    let field = |rows: &mut Vec<Row>, text: &mut Text, label: &str, value: &str, rgb| {
        rows.push(Row::Line {
            text: label.to_string(),
            px: LABEL_PX,
            rgb: MUTED_FG,
        });
        rows.push(Row::Space(LABEL_GAP));
        block(rows, text, value, VALUE_PX, rgb);
        rows.push(Row::Space(GROUP_GAP));
    };

    field(
        &mut rows,
        text,
        LABEL_LOCKED_BY,
        match content.cause {
            LockCause::Chord => CAUSE_CHORD,
            LockCause::Idle => CAUSE_IDLE,
            LockCause::SeatChange => CAUSE_SEAT,
        },
        VALUE_FG,
    );
    field(
        &mut rows,
        text,
        LABEL_REALMS,
        &realm_list(&content.realms),
        VALUE_FG,
    );
    field(
        &mut rows,
        text,
        LABEL_UNLOCK,
        match content.unlock {
            UnlockMethod::Passphrase => UNLOCK_PASSPHRASE,
            UnlockMethod::AnyKey => UNLOCK_ANY,
        },
        VALUE_FG,
    );

    rows.push(Row::Rule);
    rows.push(Row::Space(GROUP_GAP));
    rows.push(Row::Line {
        text: LABEL_STILL_RUNNING.to_string(),
        px: LABEL_PX,
        rgb: MUTED_FG,
    });
    rows.push(Row::Space(LABEL_GAP));
    block(&mut rows, text, STILL_RUNNING_1, VALUE_PX, WARN_FG);
    rows.push(Row::Space(6));
    block(
        &mut rows,
        text,
        &format!(
            "{STILL_RUNNING_2_PREFIX}{}{STILL_RUNNING_2_SUFFIX}",
            content.deadman_chord
        ),
        VALUE_PX,
        WARN_FG,
    );

    rows
}

/// The realm list as one comma-separated value.
///
/// `RealmId`s come from the operator's `realm.toml`, which is the same
/// provenance the consent card's realm field has. Empty is spelled out rather
/// than rendered as an empty line: a blank field reads as a rendering bug on the
/// one surface that must not look broken.
fn realm_list(realms: &[RealmId]) -> String {
    if realms.is_empty() {
        return NO_REALMS.to_string();
    }
    realms
        .iter()
        .map(RealmId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn row_height(row: &Row, text: &Text) -> u32 {
    match row {
        Row::Space(h) => *h,
        Row::Rule => RULE_H,
        Row::Line { px, .. } => text.line_metrics(*px).height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::tests::lock_fixture;

    #[test]
    fn rasterization_is_deterministic_across_instances() {
        let a = rasterize(&lock_fixture());
        let b = rasterize(&lock_fixture());
        assert_eq!(a.rgba, b.rgba);
        assert_eq!((a.width, a.height), (b.width, b.height));
    }

    #[test]
    fn the_card_is_opaque_everywhere() {
        // The composite blits rather than blends, so a transparent pixel would
        // show the app through the lock screen.
        let card = rasterize(&lock_fixture());
        assert!(card
            .rgba
            .chunks_exact(BYTES_PER_PIXEL)
            .all(|p| p[3] == 0xff));
    }

    #[test]
    fn the_cause_and_the_unlock_method_change_the_pixels() {
        // Both fields are drawn, so a card that ignored one would be a card
        // that lies about how the session got locked.
        let chord = rasterize(&lock_fixture());
        let mut idle_content = lock_fixture();
        idle_content.cause = LockCause::Idle;
        assert_ne!(chord.rgba, rasterize(&idle_content).rgba);

        // The third cause (issue #246) is drawn as its own line rather than
        // falling back to one of the other two, which is how a card would come
        // to tell a human their session locked on an idle timer they never set.
        let mut seat = lock_fixture();
        seat.cause = LockCause::SeatChange;
        let seat_card = rasterize(&seat);
        assert_ne!(chord.rgba, seat_card.rgba);
        assert_ne!(rasterize(&idle_content).rgba, seat_card.rgba);

        let mut any_key = lock_fixture();
        any_key.unlock = UnlockMethod::AnyKey;
        assert_ne!(chord.rgba, rasterize(&any_key).rgba);
    }

    #[test]
    fn the_realm_list_is_drawn_from_the_content() {
        let one = rasterize(&lock_fixture());
        let mut two = lock_fixture();
        two.realms.push(RealmId::new("realm-1"));
        assert_ne!(one.rgba, rasterize(&two).rgba);
        assert_eq!(realm_list(&[]), NO_REALMS);
        assert_eq!(
            realm_list(&[RealmId::new("a"), RealmId::new("b")]),
            "a, b".to_string()
        );
    }

    #[test]
    fn the_card_names_the_dead_man_chord_it_was_built_with() {
        // The honesty block's second sentence is only actionable if it names
        // the chord this session actually armed, so a card built with a
        // different chord must be different pixels.
        let esc = rasterize(&lock_fixture());
        let mut other = lock_fixture();
        other.deadman_chord = "f12";
        assert_ne!(esc.rgba, rasterize(&other).rgba);
    }
}
