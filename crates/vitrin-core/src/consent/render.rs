// SPDX-License-Identifier: MPL-2.0
//! Layout and rasterization of the consent card: [`rasterize`] turns one
//! [`PromptContent`] into an opaque RGBA8888 buffer plus the rectangle of
//! every choice it offers.
//!
//! # The visual design, and why it is this shape (an open question #37 asked)
//!
//! A fixed-width modal card, centered over a darkened realm view. Sharp
//! corners, a two-pixel accent border, one type family at four sizes, and
//! labelled fields — principal, realm, requests, expiry — above a row of
//! choice buttons. Four decisions inside that are load-bearing rather than
//! taste:
//!
//! - **The three choices are styled identically.** Same width, same height,
//!   same fill, same border, same text color. A consent surface that renders
//!   "Allow" as a primary action and "Deny" as a quiet secondary one is
//!   steering the human, and PRD P2's honest-UI rule does not let the TCB put
//!   its thumb on the scale of a decision it exists to ask. Equal geometry is
//!   also why the button row is centered with any odd leftover pixels pushed
//!   to the *outside*: distributing them between the buttons would make one
//!   choice a pixel wider than another.
//! - **The card is fixed-width and unscaled.** [`CARD_WIDTH`] is in output
//!   pixels and does not track the view size. Scaling is presentation policy,
//!   which PRD Doc 2 §2 keeps out of the core for the same reason
//!   [`crate::scene`] letterboxes instead of resampling; a prompt that
//!   resized itself would also mean the golden could only ever pin one size.
//! - **No countdown.** The prompt shows the *grant's* expiry, never the
//!   consent timeout's remaining seconds. A clock on the card would make its
//!   pixels a function of `Instant::now()` — untestable by construction — and
//!   the deadline is the petition's business ([`crate::petitions`]), which
//!   resolves it whether or not anything was drawn.
//! - **Everything is opaque.** The card carries no shadow, no translucency
//!   and no rounded corners, so compositing it is a row copy
//!   ([`Canvas::blit_opaque`]) rather than a per-pixel blend, and the only
//!   coverage values in the whole surface come from glyph anti-aliasing.
//!
//! # Which choices exist is derived, not hardcoded
//!
//! See [`PromptContent::choices`]. The short version: the button row is
//! generated from [`PersistenceRung`]'s variants filtered by the same
//! narrowing rule the petition registry enforces, so a rung the core would
//! refuse cannot be drawn, and a rung that exists cannot be forgotten.

use vitrin_protocol::generated::vitrin_grant::Verb;

use super::canvas::{Canvas, Rect};
use super::text::Text;
use super::{Choice, PromptContent};
use crate::scene::BYTES_PER_PIXEL;

// ---------------------------------------------------------------------------
// Type sizes
// ---------------------------------------------------------------------------

/// Title size. Also pins `super::text::GEOMETRY_SCALE` — the largest size the
/// prompt uses is what fontdue's geometry preprocessing should be tuned for.
pub(super) const TITLE_PX: f32 = 19.0;
/// The anti-spoofing subtitle and the field labels: small, quiet, secondary.
const LABEL_PX: f32 = 11.0;
/// Field values — the identity, realm, verbs and expiry a human actually
/// reads to decide. The largest body size on the card, deliberately.
const VALUE_PX: f32 = 14.0;
/// Button captions.
const BUTTON_PX: f32 = 13.0;

// ---------------------------------------------------------------------------
// Geometry (output pixels)
// ---------------------------------------------------------------------------

/// Card width. Wide enough that a typical `vitrin://` identity fits on one
/// line, narrow enough to sit inside the smallest view the CLI's `--size`
/// realistically gets pointed at.
pub(crate) const CARD_WIDTH: u32 = 560;
const BORDER: u32 = 2;
const PAD_X: u32 = 24;
const PAD_TOP: u32 = 20;
const PAD_BOTTOM: u32 = 20;
/// Vertical space between one labelled field and the next.
const GROUP_GAP: u32 = 12;
/// Vertical space between a label and its value.
const LABEL_GAP: u32 = 2;
const RULE_H: u32 = 1;
const BUTTON_H: u32 = 32;
const BUTTON_GAP: u32 = 10;
/// How many lines one field value may wrap to before it is truncated with a
/// visible marker (see [`Text::wrap`] on why truncation is marked).
const MAX_VALUE_LINES: usize = 3;

/// Horizontal inset of the content column from the card's left edge.
const CONTENT_X: u32 = BORDER + PAD_X;
/// Width available to content inside the border and padding.
const CONTENT_W: u32 = CARD_WIDTH - 2 * CONTENT_X;

// ---------------------------------------------------------------------------
// Palette (RGB; the card is opaque, so no alpha travels with these)
// ---------------------------------------------------------------------------

/// Card background: a cool near-black, deliberately darker than
/// [`crate::scene::LETTERBOX_RGBA`] so the card reads as *above* the view
/// rather than as more matte.
const CARD_BG: [u8; 4] = [0x14, 0x16, 0x1c, 0xff];
/// The border, and the one saturated color on the card. Distinct from every
/// color in the test pattern so the prompt cannot be mistaken for content.
const ACCENT: [u8; 4] = [0x4d, 0x9d, 0xe0, 0xff];
const TITLE_FG: [u8; 3] = [0xf4, 0xf6, 0xfa];
const MUTED_FG: [u8; 3] = [0x93, 0x9e, 0xb0];
const VALUE_FG: [u8; 3] = [0xe4, 0xe9, 0xf2];
const RULE_RGBA: [u8; 4] = [0x2b, 0x30, 0x3b, 0xff];
const BUTTON_BG: [u8; 4] = [0x22, 0x27, 0x31, 0xff];
const BUTTON_BORDER: [u8; 4] = [0x5c, 0x66, 0x78, 0xff];
const BUTTON_FG: [u8; 3] = [0xf1, 0xf4, 0xf9];

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

const TITLE: &str = "Grant request";
/// What the decision below means. Deliberately **not** a provenance or
/// anti-spoofing claim — see the note below for why this build may not make
/// one.
///
/// # The card leaves the authenticity claim to the trusted indicator
///
/// This line used to read "Shown by vitrind. No application or agent can draw
/// this prompt," justified as true by construction. It was not: nothing stops
/// a confined app committing a view-sized surface with a pixel-perfect replica
/// of this card — scrim and accent border included —
/// [`crate::scene::Scene::compose`] presents it full-view, and P1.7.1 had no
/// per-session secret, no reserved region, no trusted border outside the
/// client's own rectangle to tell the two apart. So P1.7.1 removed the claim
/// and the card made no authenticity assertion at all.
///
/// Issue #85 closed that gap — a per-session trusted indicator now exists
/// ([`super::super::indicator`]): a secret colour minted at startup, painted
/// as a reserved band along the top of the human-visible output and as the
/// frame around this card, on a display path the confined app cannot observe
/// or reach. That is the Qubes/Nitpicker label PRD line 294 calls for — one
/// the client provably cannot reproduce — living *outside* the client's pixel
/// rectangle, where a claim printed *inside* it could not.
///
/// The subtitle is nonetheless still left free of an authenticity claim, and
/// that is a deliberate copy decision, not a forced one (#85 criterion 4 makes
/// such a claim *permissible* once the indicator lands). The honest-UI rule
/// (PRD P2) cuts both ways: a sentence like "check the coloured frame" only
/// helps a human who already learned the colour, and printing it *in the card*
/// — the very rectangle a forger controls — is a line the forgery would copy
/// verbatim. The trustworthy place to teach the check is the reserved band the
/// app cannot forge, not this string; wiring that human-facing instruction is
/// the M1.4 demo's job, and if it ever belongs in the card it is a deliberate
/// copy change with a re-blessed golden, not a side effect of this one.
///
/// Deliberately not doing the reverse either: the card does not warn "this
/// dialog may be forged". A warning the human has no means to act on buys no
/// safety and would train them to click through a scare line, which is the
/// same failure in the other direction.
const SUBTITLE: &str = "Approving grants the authority listed below.";
const LABEL_PRINCIPAL: &str = "Principal";
const LABEL_REALM: &str = "Realm";
const LABEL_REQUESTS: &str = "Requests";
const LABEL_EXPIRES: &str = "Expires";
/// What an unbounded (`expiry_ms == 0`) petition says. Honest about where the
/// bound then comes from: the rung the human picks below.
const EXPIRY_UNBOUNDED: &str = "no time limit - bounded only by the choice below";

/// Every grantable verb, with the plain-language consequence of granting it.
///
/// A human deciding on "actuate_text" is deciding whether this principal may
/// type into their app; the wire name alone does not say that. The names are
/// the IDL's, so the prompt and the protocol cannot drift apart in what they
/// call a verb.
///
/// `the_verb_catalogue_covers_every_servable_verb` asserts this table's union
/// equals [`crate::grants::SERVED_VERB_BITS`], so a verb this core can
/// actually grant fails a test here rather than silently rendering as nothing
/// on a consent prompt. It is deliberately pinned to the *served* set, not to
/// [`Verb::VALID_MASK`]: the wire defines verbs version 1 refuses
/// `unsupported` at admission (D-017/D-018), and a verb that can never reach a
/// prompt must not get prompt copy that implies it can.
///
/// A served-set pin alone would let a newly appended IDL verb slip past this
/// module in silence — [`crate::grants::UNSERVED_VERB_BITS`] is *derived* from
/// [`Verb::VALID_MASK`], so "served ∪ unserved == the wire bitfield" is an
/// identity and can never fail. That derivation is the right runtime posture
/// (an unclassified verb is unserved, so admission fails closed) but it is not
/// a tripwire. The tripwire is the second assertion in that test: the unserved
/// set is pinned to the exact bits the IDL defines today, so appending a verb
/// turns this test red until a human classifies it — served, and given a line
/// below, or unserved, and added to that pin.
const VERB_CATALOGUE: [(Verb, &str, &str); 3] = [
    (Verb::OBSERVE, "observe", "capture frames of this realm"),
    (
        Verb::ACTUATE_POINTER,
        "actuate_pointer",
        "move the pointer and click in this realm",
    ),
    (
        Verb::ACTUATE_TEXT,
        "actuate_text",
        "type text into this realm",
    ),
];

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// One rendered choice: the decision it stands for and where it was drawn, in
/// **card-local** pixels.
///
/// The P1.7.2 seam. That task maps a grabbed pointer event into card-local
/// coordinates (the card's origin comes from
/// [`super::ConsentSurface::card_origin`]) and hits it against these
/// rectangles; the rectangles are produced by the same pass that paints the
/// buttons, so what the human clicks and what the human sees cannot diverge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChoiceBox {
    pub choice: Choice,
    pub rect: Rect,
}

/// A rasterized consent card: opaque, tightly packed RGBA8888, rows top-down.
#[derive(Debug, Clone)]
pub(crate) struct Card {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Every choice offered, in render order (left to right).
    pub buttons: Vec<ChoiceBox>,
}

/// One laid-out element of the card, in vertical order.
enum Row {
    /// Vertical whitespace.
    Space(u32),
    /// A full-width hairline separator.
    Rule,
    /// One line of text, already wrapped to fit [`CONTENT_W`].
    Line { text: String, px: f32, rgb: [u8; 3] },
    /// The choice row.
    Buttons,
}

/// Lay out and draw `prompt`.
///
/// Two passes over one row list: measure to learn the card's height, then
/// draw. Height is content-derived — a petition for one verb yields a shorter
/// card than one for three — so nothing is padded to a guessed maximum and
/// nothing overflows a guessed minimum.
pub(crate) fn rasterize(prompt: &PromptContent) -> Card {
    let mut text = Text::new();
    let rows = rows(prompt, &mut text);

    let height = PAD_TOP
        + PAD_BOTTOM
        + 2 * BORDER
        + rows.iter().map(|row| row_height(row, &text)).sum::<u32>();

    let mut rgba = vec![0u8; CARD_WIDTH as usize * height as usize * BYTES_PER_PIXEL];
    let mut buttons = Vec::new();
    // A canvas of the exact size we just allocated cannot be refused; the
    // `else` arm keeps a core bug from becoming a panic in a compositor loop
    // and returns a card that is merely empty.
    let Some(mut canvas) = Canvas::new(&mut rgba, CARD_WIDTH, height) else {
        return Card {
            rgba,
            width: CARD_WIDTH,
            height,
            buttons,
        };
    };

    // Opaque background first — every primitive after this blends onto known
    // pixels, which is what lets the finished card be blitted rather than
    // composited (module docs).
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
            Row::Rule => {
                canvas.fill_rect(
                    Rect {
                        x: CONTENT_X as i32,
                        y,
                        w: CONTENT_W,
                        h: RULE_H,
                    },
                    RULE_RGBA,
                );
            }
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
            Row::Buttons => {
                buttons = draw_buttons(&mut canvas, &mut text, prompt, y);
            }
        }
        y += row_height(row, &text) as i32;
    }

    Card {
        rgba,
        width: CARD_WIDTH,
        height,
        buttons,
    }
}

/// The card's rows, top to bottom.
fn rows(prompt: &PromptContent, text: &mut Text) -> Vec<Row> {
    let mut rows = Vec::new();
    let line = |rows: &mut Vec<Row>, s: String, px: f32, rgb: [u8; 3]| {
        rows.push(Row::Line { text: s, px, rgb });
    };
    let field = |rows: &mut Vec<Row>, text: &mut Text, label: &str, values: &[String]| {
        line(rows, label.to_string(), LABEL_PX, MUTED_FG);
        rows.push(Row::Space(LABEL_GAP));
        for value in values {
            for wrapped in text.wrap(value, VALUE_PX, CONTENT_W, MAX_VALUE_LINES) {
                line(rows, wrapped, VALUE_PX, VALUE_FG);
            }
        }
        rows.push(Row::Space(GROUP_GAP));
    };

    line(&mut rows, TITLE.to_string(), TITLE_PX, TITLE_FG);
    rows.push(Row::Space(6));
    for wrapped in text.wrap(SUBTITLE, LABEL_PX, CONTENT_W, 2) {
        line(&mut rows, wrapped, LABEL_PX, MUTED_FG);
    }
    rows.push(Row::Space(16));
    rows.push(Row::Rule);
    rows.push(Row::Space(16));

    field(
        &mut rows,
        text,
        LABEL_PRINCIPAL,
        &[prompt.principal.to_string()],
    );
    field(&mut rows, text, LABEL_REALM, &[prompt.realm.to_string()]);
    field(&mut rows, text, LABEL_REQUESTS, &verb_lines(prompt.verbs));
    field(&mut rows, text, LABEL_EXPIRES, &[expiry_line(prompt)]);

    // The last field's trailing gap plus this one give the separator the same
    // breathing room the header's has.
    rows.push(Row::Space(4));
    rows.push(Row::Rule);
    rows.push(Row::Space(16));
    rows.push(Row::Buttons);
    rows
}

/// Vertical extent of one row.
fn row_height(row: &Row, text: &Text) -> u32 {
    match row {
        Row::Space(n) => *n,
        Row::Rule => RULE_H,
        Row::Line { px, .. } => text.line_metrics(*px).height,
        Row::Buttons => BUTTON_H,
    }
}

/// One line per requested verb, each naming the verb and what granting it
/// lets this principal do.
///
/// An empty verb set cannot arrive from the wire (the IDL makes a zero verb
/// set a protocol error and `request_grant` decode rejects it before
/// admission), so the fallback exists only so a core bug reads as an obviously
/// wrong prompt rather than as a field that silently vanished.
fn verb_lines(verbs: Verb) -> Vec<String> {
    let lines: Vec<String> = VERB_CATALOGUE
        .iter()
        .filter(|(verb, _, _)| verbs.contains(*verb))
        .map(|(_, name, what)| format!("- {name}: {what}"))
        .collect();
    if lines.is_empty() {
        return vec!["- (no verbs)".to_string()];
    }
    lines
}

/// The expiry field's text.
fn expiry_line(prompt: &PromptContent) -> String {
    match prompt.expiry_ms {
        0 => EXPIRY_UNBOUNDED.to_string(),
        ms => format!("{} after approval", duration_text(ms)),
    }
}

/// Format a millisecond bound **exactly** — never rounded.
///
/// A consent prompt that rounds is a consent prompt that lies: rounding up
/// overstates the authority being asked for, and rounding down understates
/// it. So a duration is shown in the largest unit that divides it evenly and
/// falls back to raw milliseconds when none does, which is honest at every
/// input and still reads naturally at the values petitions actually use.
fn duration_text(ms: u32) -> String {
    if ms.is_multiple_of(3_600_000) {
        format!("{} h", ms / 3_600_000)
    } else if ms.is_multiple_of(60_000) {
        format!("{} min", ms / 60_000)
    } else if ms.is_multiple_of(1_000) {
        format!("{} s", ms / 1_000)
    } else {
        format!("{ms} ms")
    }
}

/// Draw the choice row at `y` and report where each button landed.
///
/// Equal widths, computed once and used for every button; the row is centered
/// inside the content column and any remainder from the integer division goes
/// to the outer margins (module docs: no choice may be a pixel wider than
/// another).
fn draw_buttons(
    canvas: &mut Canvas<'_>,
    text: &mut Text,
    prompt: &PromptContent,
    y: i32,
) -> Vec<ChoiceBox> {
    let choices = prompt.choices();
    let n = choices.len() as u32;
    if n == 0 {
        return Vec::new();
    }
    let gaps = BUTTON_GAP * n.saturating_sub(1);
    let width = CONTENT_W.saturating_sub(gaps) / n;
    if width == 0 {
        return Vec::new();
    }
    let used = width * n + gaps;
    let mut x = CONTENT_X as i32 + ((CONTENT_W - used) / 2) as i32;

    let mut boxes = Vec::with_capacity(choices.len());
    for choice in choices {
        let rect = Rect {
            x,
            y,
            w: width,
            h: BUTTON_H,
        };
        canvas.fill_rect(rect, BUTTON_BG);
        canvas.stroke_rect(rect, BUTTON_BORDER, 1);

        // Caption centered in the box. Measured with the same engine that
        // draws it, so a caption that grows past its button is visibly
        // clipped by the canvas rather than silently mis-centered.
        let label = choice.label();
        let label_w = text.width(label, BUTTON_PX);
        let metrics = text.line_metrics(BUTTON_PX);
        let tx = x + ((width.saturating_sub(label_w)) / 2) as i32;
        let ty = y + ((BUTTON_H.saturating_sub(metrics.height)) / 2) as i32 + metrics.ascent as i32;
        text.draw(canvas, label, BUTTON_PX, tx, ty, BUTTON_FG);

        boxes.push(ChoiceBox { choice, rect });
        x += (width + BUTTON_GAP) as i32;
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::tests::{prompt_fixture, PROMPT_IDENTITY, PROMPT_REALM};
    use crate::grants::PersistenceRung;

    /// The RGBA quadruple at `(x, y)` of a card.
    fn px(card: &Card, x: u32, y: u32) -> [u8; 4] {
        let off = (y as usize * card.width as usize + x as usize) * BYTES_PER_PIXEL;
        card.rgba[off..off + BYTES_PER_PIXEL].try_into().unwrap()
    }

    #[test]
    fn the_card_is_opaque_everywhere() {
        // The precondition `Canvas::blit_opaque` relies on: compositing the
        // card is a row copy, which is only correct because no pixel of it is
        // transparent. Glyph coverage blends *into* the card; it never leaves
        // a hole in it.
        let card = rasterize(&prompt_fixture());
        assert_eq!(
            card.rgba.len(),
            card.width as usize * card.height as usize * BYTES_PER_PIXEL
        );
        assert!(
            card.rgba
                .chunks_exact(BYTES_PER_PIXEL)
                .all(|p| p[3] == 0xff),
            "a transparent pixel in the card would make blit_opaque wrong"
        );
    }

    #[test]
    fn the_border_frames_the_card_on_all_four_sides() {
        let card = rasterize(&prompt_fixture());
        for (x, y) in [
            (0, 0),
            (card.width - 1, 0),
            (0, card.height - 1),
            (card.width - 1, card.height - 1),
            (card.width / 2, 0),
            (0, card.height / 2),
        ] {
            assert_eq!(px(&card, x, y), ACCENT, "border missing at ({x}, {y})");
        }
        // Immediately inside the border is card background, not more border.
        assert_eq!(px(&card, BORDER, card.height / 2), CARD_BG);
    }

    #[test]
    fn exactly_the_three_mvp_choices_are_rendered_for_a_while_running_petition() {
        // Acceptance criterion: Allow-once / Allow-while-running / Deny, and
        // no phantom durable rung. The IDL has four persistence values; the
        // two durable ones are not representable as a `PersistenceRung` at
        // all, so they cannot reach a button -- this pins that they do not.
        let card = rasterize(&prompt_fixture());
        let choices: Vec<Choice> = card.buttons.iter().map(|b| b.choice).collect();
        assert_eq!(
            choices,
            vec![
                Choice::Allow(PersistenceRung::Once),
                Choice::Allow(PersistenceRung::WhileRunning),
                Choice::Deny,
            ]
        );
        let labels: Vec<&str> = choices.iter().map(|c| c.label()).collect();
        assert_eq!(
            labels,
            ["Allow once (single use)", "Allow while running", "Deny"]
        );
        for label in labels {
            assert!(
                !label.to_ascii_lowercase().contains("always")
                    && !label.to_ascii_lowercase().contains("until"),
                "a durable rung reached a button caption: {label}"
            );
        }
    }

    #[test]
    fn a_once_petition_is_not_offered_a_longer_rung() {
        // Honest UI, the sharper case: offering "Allow while running" on a
        // petition that asked for `once` would be offering a decision the
        // petition registry refuses as a widening. The button is ABSENT --
        // not disabled, not greyed -- for the same reason durable rungs are.
        let mut prompt = prompt_fixture();
        prompt.persistence = PersistenceRung::Once;
        let card = rasterize(&prompt);
        assert_eq!(
            card.buttons.iter().map(|b| b.choice).collect::<Vec<_>>(),
            vec![Choice::Allow(PersistenceRung::Once), Choice::Deny]
        );
    }

    #[test]
    fn every_choice_gets_an_identically_sized_button() {
        // The no-nudge rule, measured: identical geometry, non-overlapping,
        // in render order, all inside the content column.
        for persistence in [PersistenceRung::Once, PersistenceRung::WhileRunning] {
            let mut prompt = prompt_fixture();
            prompt.persistence = persistence;
            let card = rasterize(&prompt);
            let rects: Vec<Rect> = card.buttons.iter().map(|b| b.rect).collect();
            assert!(rects.len() >= 2);
            for pair in rects.windows(2) {
                assert_eq!(pair[0].w, pair[1].w, "buttons must be equally wide");
                assert_eq!(pair[0].h, pair[1].h, "buttons must be equally tall");
                assert_eq!(pair[0].y, pair[1].y, "buttons share one row");
                assert!(
                    pair[0].x + pair[0].w as i32 <= pair[1].x,
                    "buttons must not overlap"
                );
            }
            for rect in &rects {
                assert!(rect.x >= CONTENT_X as i32);
                assert!(rect.x + rect.w as i32 <= (CARD_WIDTH - CONTENT_X) as i32);
                assert!(rect.y + rect.h as i32 <= card.height as i32);
            }
        }
    }

    #[test]
    fn buttons_are_actually_painted_where_they_are_reported() {
        // The P1.7.2 contract: a rectangle reported here is a rectangle a
        // human can see and aim at. Checked on real pixels -- the border
        // color at the box's corner, and background inside it -- so a layout
        // that reported one geometry and painted another would fail.
        let card = rasterize(&prompt_fixture());
        for b in &card.buttons {
            let (x, y) = (b.rect.x as u32, b.rect.y as u32);
            assert_eq!(px(&card, x, y), BUTTON_BORDER, "button border corner");
            assert_eq!(
                px(&card, x + b.rect.w - 1, y + b.rect.h - 1),
                BUTTON_BORDER,
                "button border opposite corner"
            );
            // Probe just inside the border, where the caption cannot reach:
            // sampling nearer the middle would land on an anti-aliased glyph
            // edge and assert the caption's absence rather than the fill's
            // presence.
            assert_eq!(
                px(&card, x + 2, y + 2),
                BUTTON_BG,
                "button interior at ({x}, {y})"
            );
            assert!(!b.rect.contains(x as i32 - 1, y as i32));
        }
    }

    #[test]
    fn the_verb_catalogue_covers_every_servable_verb() {
        // A verb this core can grant must gain a line here, or a consent
        // prompt would ask a human to approve authority it never names.
        let union = VERB_CATALOGUE
            .iter()
            .fold(0u32, |acc, (verb, _, _)| acc | verb.bits());
        assert_eq!(
            union,
            crate::grants::SERVED_VERB_BITS,
            "VERB_CATALOGUE does not cover every verb version 1 can grant"
        );
        // Verbs the IDL defines but version 1 refuses `unsupported` are
        // absent on purpose: no petition naming one ever reaches a prompt.
        assert_eq!(
            union & crate::grants::UNSERVED_VERB_BITS,
            0,
            "VERB_CATALOGUE names a verb version 1 refuses at admission"
        );
        // The tripwire. `UNSERVED_VERB_BITS` is derived as `VALID_MASK &
        // !SERVED_VERB_BITS`, so asserting the two sets partition the mask
        // would be an identity that no IDL change can break -- it would read
        // like a guard and guard nothing. Pinning the unserved set to the
        // bits the IDL defines *today* is the guard: appending a verb widens
        // `VALID_MASK`, widens the derived unserved set, and turns this red
        // until a human classifies the new bit (served, plus a catalogue line
        // above; or unserved, plus an entry here). The runtime posture is
        // unchanged and stays fail-closed -- an unclassified verb is unserved
        // and admission refuses it `unsupported`; this only refuses to let
        // that happen silently.
        assert_eq!(
            crate::grants::UNSERVED_VERB_BITS,
            Verb::OBSERVE_CURSOR.bits() | Verb::LAYOUT_ARRANGE.bits() | Verb::LAYOUT_FOCUS.bits(),
            "the IDL defines a verb this module has not classified as served \
             or unserved (D-017/D-018)"
        );
        // ...and the two really do partition the wire bitfield. Implied by the
        // derivation above; asserted so the invariant is written down.
        assert_eq!(
            crate::grants::SERVED_VERB_BITS | crate::grants::UNSERVED_VERB_BITS,
            Verb::VALID_MASK
        );
        // Names match the IDL's spelling, so prompt and protocol cannot drift.
        let names: Vec<&str> = VERB_CATALOGUE.iter().map(|(_, name, _)| *name).collect();
        assert_eq!(names, ["observe", "actuate_pointer", "actuate_text"]);
    }

    #[test]
    fn only_the_requested_verbs_are_listed() {
        assert_eq!(
            verb_lines(Verb::OBSERVE),
            vec!["- observe: capture frames of this realm"]
        );
        let both = verb_lines(Verb::OBSERVE | Verb::ACTUATE_TEXT);
        assert_eq!(both.len(), 2);
        assert!(both.iter().all(|l| !l.contains("actuate_pointer")));
        // Defensive only: an empty set is a wire error long before here.
        assert_eq!(verb_lines(Verb::default()), vec!["- (no verbs)"]);
    }

    #[test]
    fn durations_are_exact_never_rounded() {
        // Every case shows the true bound: no rounding in either direction.
        assert_eq!(duration_text(60_000), "1 min");
        assert_eq!(duration_text(30_000), "30 s");
        assert_eq!(duration_text(3_600_000), "1 h");
        assert_eq!(duration_text(7_200_000), "2 h");
        assert_eq!(duration_text(90_000), "90 s");
        assert_eq!(duration_text(1_500), "1500 ms");
        assert_eq!(duration_text(1), "1 ms");
        // ... and the field text built from it.
        let mut prompt = prompt_fixture();
        prompt.expiry_ms = 0;
        assert_eq!(expiry_line(&prompt), EXPIRY_UNBOUNDED);
        prompt.expiry_ms = 45_000;
        assert_eq!(expiry_line(&prompt), "45 s after approval");
    }

    #[test]
    fn card_height_tracks_its_content() {
        // Content-derived, not padded to a guess: fewer verbs is a shorter
        // card, and a wrapping identity is a taller one.
        let one_verb = {
            let mut p = prompt_fixture();
            p.verbs = Verb::OBSERVE;
            rasterize(&p)
        };
        let three_verbs = rasterize(&prompt_fixture());
        assert!(one_verb.height < three_verbs.height);

        let long_identity = {
            let mut p = prompt_fixture();
            p.principal = crate::identity::PrincipalIdentity::parse(
                "vitrin://local/agent/a-deliberately-long-agent-name-that-cannot-fit-on-one-line-of-this-card",
            )
            .expect("shape-valid identity");
            rasterize(&p)
        };
        assert!(long_identity.height > three_verbs.height);
    }

    #[test]
    fn the_prompt_only_draws_content_the_petition_supplied() {
        // Sourcing, asserted as an equivalence rather than by eyeballing the
        // pixels: change any field of the petition and the pixels change;
        // change nothing and they are identical. Combined with
        // `PromptContent` holding no free-text field at all (see
        // `crate::consent`), that is what "an agent cannot put a glyph on
        // screen" means operationally.
        let base = rasterize(&prompt_fixture());
        assert_eq!(base.rgba, rasterize(&prompt_fixture()).rgba);

        let mut other_principal = prompt_fixture();
        other_principal.principal =
            crate::identity::PrincipalIdentity::parse("vitrin://local/agent/other").unwrap();
        assert_ne!(base.rgba, rasterize(&other_principal).rgba);

        let mut other_realm = prompt_fixture();
        other_realm.realm = crate::grants::RealmId::new("kiosk");
        assert_ne!(base.rgba, rasterize(&other_realm).rgba);

        let mut other_verbs = prompt_fixture();
        other_verbs.verbs = Verb::OBSERVE;
        assert_ne!(base.rgba, rasterize(&other_verbs).rgba);

        let mut other_expiry = prompt_fixture();
        other_expiry.expiry_ms = 12_000;
        assert_ne!(base.rgba, rasterize(&other_expiry).rgba);

        // And the fixture's own values are the ones being drawn.
        assert_eq!(prompt_fixture().principal.to_string(), PROMPT_IDENTITY);
        assert_eq!(prompt_fixture().realm.to_string(), PROMPT_REALM);
    }

    /// Every caption must fit inside its own button.
    ///
    /// Captions are centered by measuring, and a caption wider than its box is
    /// clipped by the canvas rather than rejected — so an over-long one degrades
    /// into a truncated word on a security decision. The row's width is derived
    /// from [`CONTENT_W`] and the choice count, so this is a real constraint a
    /// future caption (or a fourth rung) could quietly violate; `Allow once
    /// (single use)` already spends 134 of the 162 available pixels.
    #[test]
    fn every_button_caption_fits_inside_its_button() {
        let mut text = Text::new();
        for requested in PersistenceRung::ALL {
            let mut prompt = prompt_fixture();
            prompt.persistence = requested;
            let card = rasterize(&prompt);
            for b in &card.buttons {
                let w = text.width(b.choice.label(), BUTTON_PX);
                assert!(
                    w <= b.rect.w,
                    "caption {:?} is {}px wide in a {}px button -- it would be \
                     clipped on screen",
                    b.choice.label(),
                    w,
                    b.rect.w
                );
            }
        }
    }
}
