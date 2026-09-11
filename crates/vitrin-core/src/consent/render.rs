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

use super::{Choice, PanelContent, PromptContent};
use crate::designation::AskedFor;
use crate::grants::RealmId;
use crate::identity::PrincipalIdentity;
use crate::paint::canvas::{Canvas, Rect};
use crate::paint::script::Group;
use crate::paint::text::{Text, Vetted, NAME_PX};
use crate::paint::transcript::{Class, Transcript};
use crate::picker::session::{Focus, NAME_FIELD_PX};
use crate::scene::BYTES_PER_PIXEL;

// ---------------------------------------------------------------------------
// Type sizes
// ---------------------------------------------------------------------------

/// Title size. Also pins `crate::paint::text::GEOMETRY_SCALE` — the largest size the
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

/// How many slots an interactive panel draws — **always**, whatever it holds.
///
/// Fixed, and the fixedness is the security property rather than a layout
/// convenience. [`row_height`] returns [`PANEL_H`] for `Row::Panel` without
/// consulting the content, so a panel refresh cannot change the card's
/// height; the card's origin comes from [`crate::paint::centered`] over that
/// height, so the origin cannot move either; and the choice row sits below a
/// constant, so **the Deny button cannot travel under a human's finger
/// between the press and the release**. That is the same hazard
/// [`super::grab::GUARD_INTERVAL`] exists for, one step further in: the guard
/// stops a card appearing under a descending finger, and this stops the card
/// re-laying itself out under a resting one.
///
/// The cost is stated rather than hidden: a panel with fewer entries than
/// this draws empty slots, and one with more scrolls. Neither is free, and
/// both are cheaper than a card that resizes while it is being read.
pub(crate) const PANEL_ROWS: u16 = 8;
/// Height of one panel slot.
const PANEL_ROW_H: u32 = 22;
/// The panel's total height. A constant, by [`PANEL_ROWS`]' argument.
const PANEL_H: u32 = PANEL_ROWS as u32 * PANEL_ROW_H;
/// Width of the scroll thumb's gutter, inset from the content column's right.
const PANEL_THUMB_W: u32 = 4;
/// Width of the **collision gutter**: the reserved column
/// [`crate::picker::listing`]'s repair mark is drawn in.
///
/// Wide enough for `#999`, the largest mark that module will emit, at
/// [`NAME_PX`] — `every_gutter_mark_fits_its_reserved_column` measures that
/// against the shipped face rather than trusting this sentence.
///
/// It is a *reserved* column in the strong sense: the name field is elided to
/// [`NAME_FIELD_PX`] and this sits beyond both that width and the script
/// column, so no name can grow into it. That is the property the repair
/// depends on — a mark inside the name would be the first thing elision cut,
/// on exactly the long-shared-prefix rows that need it.
const PANEL_GUTTER_W: u32 = 40;
/// Gap between the name field and the script column, and between the script
/// column and the gutter.
const PANEL_GUTTER_GAP: u32 = 8;

/// Left edge of the script-label column, card-local.
fn script_x() -> u32 {
    CONTENT_X + NAME_FIELD_PX + PANEL_GUTTER_GAP
}

/// Left edge of the collision gutter, card-local. Right-anchored: the gutter
/// hugs the thumb so its position does not move when a script label is long.
fn gutter_x() -> u32 {
    CONTENT_X + CONTENT_W - PANEL_THUMB_W - 4 - PANEL_GUTTER_W
}

/// The three columns must fit inside the content column, or the gutter would
/// overlap the name field and the reservation above would be a sentence
/// rather than a fact. A build-time check, because the alternative is finding
/// out by reading a golden.
///
/// This says nothing about the *script label* fitting: a label's width is a
/// property of the shipped face, which no `const` can measure. What it
/// guarantees is only that `script_x() <= gutter_x()` — that the column
/// exists. `the_widest_script_label_fits_its_column` is what measures whether
/// anything can be written in it, and it is a separate, runtime check for
/// that reason.
const _: () = assert!(
    CONTENT_X + NAME_FIELD_PX + 2 * PANEL_GUTTER_GAP + PANEL_GUTTER_W + PANEL_THUMB_W + 4
        <= CONTENT_X + CONTENT_W,
    "the picker's name field, script column and gutter do not fit the card"
);

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
/// The highlighted panel row's fill. A lifted grey rather than the accent —
/// see [`draw_panel`] on why a resting cursor must not look armed.
const PANEL_SELECTED_BG: [u8; 4] = [0x2b, 0x33, 0x42, 0xff];

// ---------------------------------------------------------------------------
// The transcript palette (P2.6.6)
// ---------------------------------------------------------------------------

/// How a [`Transcript`]'s three run classes are coloured.
///
/// A struct rather than three loose constants because the *set* is the
/// safety property: what makes an escape legible is that its colour differs
/// from the other two, and a palette that lost that would still compile as
/// three constants. `the_three_run_classes_are_three_distinct_colours` holds
/// it.
struct RunPalette {
    ascii: [u8; 3],
    native: [u8; 3],
    escaped: [u8; 3],
}

impl RunPalette {
    fn of(&self, class: Class) -> [u8; 3] {
        match class {
            Class::Ascii => self.ascii,
            Class::Native => self.native,
            Class::Escaped => self.escaped,
        }
    }
}

/// A filename's own colours.
///
/// - `ascii` is the unremarkable case and is deliberately the same
///   [`VALUE_FG`] the rest of the card's values use: the overwhelming
///   majority of rows are ASCII, and tinting them all would make the tint
///   mean nothing.
/// - `native` is a character drawn as itself outside ASCII. Warm, so a
///   Cyrillic or Japanese run reads as *different* without reading as
///   *wrong*: it is not an error, and colouring it like one would train the
///   human to distrust their own language's filenames.
/// - `escaped` is a character that was **not** drawn as itself. Cold and
///   saturated, because this is the one that says the transcript intervened —
///   a bidi override, a combining mark, an undecodable byte, or a
///   minority-script letter inside a word. It is a second tell, independent
///   of the glyph shapes, which is the point: a human who cannot see that
///   `\u{0435}` was a Cyrillic letter can still see that the row changed
///   colour where an all-Latin row would not have.
const NAME_PALETTE: RunPalette = RunPalette {
    ascii: VALUE_FG,
    native: [0xe8, 0xc0, 0x6a],
    escaped: [0xff, 0x8b, 0x8b],
};

/// The gutter mark's colours. Core-minted `#N` is ASCII and takes the accent,
/// so the mark reads as the core speaking about the row rather than as part
/// of the row's own name. The other two arms are not decoration: a mark that
/// somehow stopped being core-minted ASCII would still announce itself rather
/// than passing as a normal mark.
const TAG_PALETTE: RunPalette = RunPalette {
    ascii: [0x4d, 0x9d, 0xe0],
    native: [0xe8, 0xc0, 0x6a],
    escaped: [0xff, 0x8b, 0x8b],
};

/// The script label beside a row. Muted: it is metadata about the name, not
/// part of it.
const SCRIPT_FG: [u8; 3] = MUTED_FG;

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
/// The `realm_launch` field's label. "Launches", not "Command": the human is
/// deciding whether this principal may **start** that program, and a label
/// reading like a field of the realm's description would invite them to skim
/// it as configuration trivia rather than as the thing being approved.
const LABEL_LAUNCHES: &str = "Launches";
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
/// [`Verb::VALID_MASK`]: the wire defines verbs this core refuses
/// `unsupported` at admission (D-017/D-018 — `realm_launch` was in that list
/// until WS-E.1.1 gave it a facet, a chokepoint arm and prompt copy, and it
/// is served now), and a verb that can never reach a prompt
/// must not get prompt copy that implies it can. `observe_cursor` is the
/// standing example, and `designate_file` (P2.6.5) was one beside it until
/// P2.6.6 moved the bit into the served set and put a minimum honest line in
/// this table; P2.6.8 (issue #192, D-048) replaced that line with the
/// considered copy under Q13's first prompt-design review, and the entry's
/// own comment traces every clause of it to the IDL. `egress` is the next
/// verb whose copy is written, and it is deliberately NOT in this table:
/// D-048 stages its line as prose until P2.7.3's proxy makes the verb
/// enforceable, because a line here for authority no code enforces is the
/// exact lie this table exists to prevent.
///
/// **What a line here may claim, stated as the rule the review applied:** a
/// line states the GRANT's reach -- what approving it lets this principal
/// do -- and never the SANDBOX's denial. "This app gets only what you pick"
/// is a sentence about what confinement withholds, it is false at
/// `--isolation=off`, and a consent card that is true on one isolation tier
/// and false on another is not a consent card. Every line below is written
/// to stay true on every tier the core admits the verb on.
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
///
/// The two layout lines say what the human loses rather than what the
/// principal gains, because that is the decision in front of them. "Show
/// this realm and send your keyboard and mouse to it" is what
/// `layout_focus` actually costs: the human's own typing follows the
/// binding, so approving it is approving that this principal may move
/// where the human's next keystroke lands. Naming it "direct keyboard
/// focus", the IDL's phrasing, would be accurate and would not say that.
///
/// **Both layout lines also name the attention key** (WS-E.1.7, issue #232),
/// and that is Q13's rule applied to a *widened* verb rather than a new one:
/// a verb whose consequence changed ships admitted-but-refused `unsupported`
/// until its copy says so. What changed is that the grant now receives the
/// human's attention key — a session-level fact about the human delivered to
/// this principal's connection, and a moment in which this verb is not
/// refused `preempted`. The human is told, in the same sentence, both halves:
/// the grant is *told* when they press it, and it may *act* in that moment.
/// It is deliberately not phrased as delegation ("you give it the key"),
/// because the press delegates nothing — see [`crate::attention`].
/// **`realm_launch`'s line says what the human loses, like the layout
/// pair**, and what it costs is different in kind from every other verb
/// here: approving it lets this principal make the trusted core **start a
/// program**, repeatedly, for as long as the grant lives. The line names
/// the act ("start"), its repeatability ("as often as its rate limit
/// allows"), and defers the *identity* of the program to the `Launches`
/// field above it — which is the one place the path is shown, so the two
/// cannot disagree about which program is meant.
///
/// It deliberately does not say "and observe it": launching confers nothing
/// over what was launched, and a line that implied otherwise would ask for
/// consent to authority this grant does not carry.
const VERB_CATALOGUE: [(Verb, &str, &str); 7] = [
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
    (
        Verb::LAYOUT_ARRANGE,
        "layout_arrange",
        "make this realm fill the screen, or shrink it back - and act on your attention key",
    ),
    (
        Verb::LAYOUT_FOCUS,
        "layout_focus",
        "show this realm and send your keyboard and mouse to it - and act on your attention \
         key",
    ),
    (
        Verb::REALM_LAUNCH,
        "realm_launch",
        "start the program named above, as a new app, as often as its rate limit allows",
    ),
    // **`designate_file`'s considered copy** (P2.6.8, issue #192, D-048 --
    // Q13's first prompt-design review). P2.6.6 served the verb with a
    // minimum honest line so the test below would not let a servable verb go
    // unnamed on a card; this is the line that review replaced it with. Six
    // drafts from three angles, four judge passes, two syntheses; the record
    // is D-048's. The string is FINAL, and no protocol prose page restates
    // it (a second unchecked copy is this repository's dominant defect
    // class): `docs/protocol/13-vitrin_powerbox.md` points here instead.
    // The decision log records it as the decided text, dated, which is a
    // record and not a second surface.
    //
    // **The four facts the line encodes**, each traceable to the IDL so the
    // card and the protocol cannot disagree about what is being approved:
    //
    //   (1) "ask you ... you pick each time" -- the verb is authority to ASK,
    //       exercised per ask, and the human picks in the core-drawn chooser
    //       (`vitrin_grant.verb` `designate_file`; `vitrin_powerbox`'s
    //       description; `request_file`). Not authority to read anything.
    //   (2) "one file ... or one folder and everything in it" -- one ask
    //       yields ONE handle: a file, or a directory subtree as a single
    //       descriptor (the `designation` event; `request_dir`). "everything
    //       in it" is what a directory handle is, said in words.
    //   (3) "to read or to read and change" -- the mode CEILING a file ask
    //       may carry, stated as the pair. The per-ask mode is shown on the
    //       PICKER card at pick time (`CONSEQUENCE_READ` / `CONSEQUENCE_WRITE`
    //       below); the folder card there deliberately names no mode, and
    //       this line claims none for a folder either -- the pair is
    //       attached to the file clause, not to the sentence.
    //   (4) "keeps what you hand over until it exits, even if you revoke this
    //       grant" -- the residue survives revocation until the realm dies
    //       (the `designation` event's description; `designate_file`:
    //       "revocation ... keeps every fd already handed over until its
    //       realm dies"). The one clause a human cannot infer, and the one
    //       whose omission would describe an authority the deployment cannot
    //       take back as one it can.
    //
    // **Three things it deliberately does NOT say**, each refused for a
    // reason rather than cut for length:
    //
    //   - "nothing else about your files is reachable" / "this app gets only
    //     what you pick". That is a claim about the SANDBOX's denial, not
    //     about the grant, and it is false at `--isolation=off`. This
    //     catalogue's own rule (module docs) is that a line states the
    //     GRANT's reach, and only the grant's, so it stays true on every
    //     tier.
    //   - "the chooser says which" (read or read-and-change). True for a file
    //     ask, false for a folder ask, whose card names no mode -- a sentence
    //     that is half-true on a consent surface is a lie on it.
    //   - that the agent holds its own copy of the descriptor. A cross-
    //     principal fact about the relay, which the card must not surface:
    //     the human is deciding what THIS app may ask for, and naming a third
    //     party's holdings would invite consent to something this grant does
    //     not confer.
    //
    // Fit: `- designate_file: ` plus this string measures 1318 px of
    // Liberation Sans advance at `VALUE_PX` (measured through `Text::width`,
    // not estimated; the review's pre-landing estimate was about 1408) against
    // a three-line budget of 3 x `CONTENT_W` = 1524 px, and wraps to exactly
    // three lines, untruncated. `every_catalogue_line_fits_untruncated` below
    // holds that for every line here, present and future, so the number is a
    // note and the test is the fact.
    (
        Verb::DESIGNATE_FILE,
        "designate_file",
        "ask you to hand over one file, to read or to read and change, or one folder and everything \
         in it - you pick each time, and this app keeps what you hand over until it exits, even if \
         you revoke this grant",
    ),
];

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// What pressing one of a card's buttons means.
///
/// Two card kinds share one renderer, and they ask different questions:
/// a grant-request card offers [`Choice`]es, and a picker card
/// ([`PickerContent`]) offers Confirm and Cancel. This enum is what keeps
/// them one seam without letting them be confused for one another — see
/// [`ButtonBox::as_choice`], which is the only way a button becomes something
/// a petition can be resolved with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardAction {
    /// A decision on the pending petition.
    Decide(Choice),
    /// The picker's commit: designate the highlighted row.
    Confirm,
    /// The picker's refusal.
    Cancel,
}

impl CardAction {
    /// The button caption. Exhaustive over the enum for [`Choice::label`]'s
    /// reason: an action added here fails to compile until somebody decides
    /// what to call it in front of a human.
    pub fn label(self) -> &'static str {
        match self {
            CardAction::Decide(choice) => choice.label(),
            // Not "OK". The verb the human is taking is handing over one
            // named thing, and a caption that named no act would read as
            // acknowledging a message rather than as taking one.
            CardAction::Confirm => "Hand over the selected item",
            // Not "Close". Cancelling *refuses* the ask -- the agent is told
            // `cancelled`, and a `once` rung is spent by it -- and a caption
            // that read as dismissing a window would understate that.
            CardAction::Cancel => "Cancel - hand over nothing",
        }
    }
}

/// One rendered button: what pressing it means and where it was drawn, in
/// **card-local** pixels.
///
/// The P1.7.2 seam. That task maps a grabbed pointer event into card-local
/// coordinates (the card's origin comes from
/// [`super::ConsentSurface::card_origin`]) and hits it against these
/// rectangles; the rectangles are produced by the same pass that paints the
/// buttons, so what the human clicks and what the human sees cannot diverge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ButtonBox {
    pub action: CardAction,
    pub rect: Rect,
}

impl ButtonBox {
    /// This button as a petition decision, or `None` if it is not one.
    ///
    /// **The narrowing [`super::grab::ConsentGrab`] snapshots through.** A
    /// grab's armed prompt holds only the buttons this returns `Some` for, so
    /// a picker's Confirm and Cancel are not merely ignored by
    /// `ConsentGrab::commit` — they are *absent* from the geometry it
    /// hit-tests, and a [`super::grab::Decision`] (which names a
    /// `PetitionId`, and a designation has none) is therefore unconstructible
    /// from a picker card rather than merely unconstructed. That is
    /// `raise_picker`'s "no button rectangles exist" property, kept now that
    /// a picker card really does paint buttons.
    pub fn as_choice(&self) -> Option<ChoiceBox> {
        match self.action {
            CardAction::Decide(choice) => Some(ChoiceBox {
                choice,
                rect: self.rect,
            }),
            CardAction::Confirm | CardAction::Cancel => None,
        }
    }
}

/// One rendered choice: the decision it stands for and where it was drawn, in
/// **card-local** pixels. [`ButtonBox`] narrowed to the buttons that resolve a
/// petition; see [`ButtonBox::as_choice`].
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
    /// Every **petition decision** offered, in render order (left to right).
    ///
    /// [`Self::controls`] filtered through [`ButtonBox::as_choice`], and the
    /// filtering is where the safety lives rather than in any caller: a
    /// picker card paints Confirm and Cancel, neither of which is a
    /// [`Choice`], so a picker card's `buttons` is **empty by construction**.
    /// [`super::grab::ConsentGrab`] snapshots exactly this field, so the
    /// rectangles it hit-tests into a [`super::grab::Decision`] cannot
    /// include a button that answers no petition — which matters, because a
    /// `Decision` names a `PetitionId` and a designation has none.
    pub buttons: Vec<ChoiceBox>,
    /// Every button the card actually painted, in render order, whether or
    /// not it decides a petition.
    ///
    /// The paint-and-report seam [`ButtonBox`] describes, unfiltered: a
    /// rectangle here is a rectangle the same pass painted, so what a test
    /// (or a future pointer path) aims at and what the human sees cannot
    /// diverge. [`Self::buttons`] is this narrowed, never a second traversal.
    pub controls: Vec<ButtonBox>,
    /// Every interactive slot and where it was drawn, card-local, in render
    /// order — `Some` **exactly when [`draw_panel`] ran**.
    ///
    /// This field is the interactive-mode switch, and it is deliberately not
    /// a flag beside one. A card is navigable because a panel was *painted*,
    /// on [`ChoiceBox`]' own reasoning one field up: the rectangles come from
    /// the same pass that paints them, so what the human clicks and what the
    /// human sees cannot diverge. A separate `interactive: bool` could
    /// disagree with the pixels; this cannot.
    pub panel: Option<Vec<SlotBox>>,
}

/// One rendered interactive slot: which slot it is and where it was drawn, in
/// **card-local** pixels.
///
/// [`ChoiceBox`]' sibling and produced the same way, by the pass that paints
/// it. `index` is the slot's position in the panel, never an index into the
/// content behind it — the panel draws [`PANEL_ROWS`] slots whatever the
/// content holds, and mapping a slot back to a thing is the embedder's job.
/// Keeping that translation outside the grab is what lets the grab stay
/// ignorant of what is being chosen, which is the same separation
/// [`super::PromptContent`] keeps by having no string field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotBox {
    pub index: u16,
    pub rect: Rect,
}

/// One laid-out element of the card, in vertical order.
enum Row {
    /// Vertical whitespace.
    Space(u32),
    /// A full-width hairline separator.
    Rule,
    /// One line of text, already wrapped to fit [`CONTENT_W`].
    Line { text: String, px: f32, rgb: [u8; 3] },
    /// One line of **transcribed** text — a filename, a path, or the human's
    /// filter query — drawn through the widened door
    /// ([`Text::draw_runs`]) with each [`Class`] run in its own colour, and
    /// elided to `width` rather than wrapped.
    ///
    /// Separate from [`Row::Line`] because the two doors are separate
    /// ([`crate::paint::text`]'s module docs): `Line` substitutes anything
    /// outside ASCII printables, which is right for an operator-written
    /// `realm.toml` path and would silently destroy the very distinctions a
    /// transcript exists to preserve.
    Transcribed { text: Transcript, width: u32 },
    /// The button row: the actions it offers in render order, and which of
    /// them (if any) holds keyboard focus.
    Buttons {
        actions: Vec<CardAction>,
        focused: Option<usize>,
    },
    /// The interactive panel. Its height is [`PANEL_H`] and does not depend
    /// on what it holds — see [`PANEL_ROWS`].
    Panel(PanelContent),
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
    draw_card(&mut text, &rows)
}

/// Lay out and draw the **picker card** (P2.6.6, issue #190).
///
/// [`rasterize`]'s sibling, through the same row list, the same two passes and
/// the same [`draw_card`] — which is the point. A second card in the TCB is a
/// second place for a golden to drift ([`crate::paint`]'s rule), so the only
/// thing that differs between a grant-request card and a picker card is the
/// rows they build.
pub(crate) fn rasterize_picker(content: &PickerContent) -> Card {
    let mut text = Text::new();
    let rows = picker_rows(content, &mut text);
    draw_card(&mut text, &rows)
}

/// Measure `rows`, allocate the card, and draw them.
fn draw_card(text: &mut Text, rows: &[Row]) -> Card {
    let height = PAD_TOP
        + PAD_BOTTOM
        + 2 * BORDER
        + rows.iter().map(|row| row_height(row, text)).sum::<u32>();

    let mut rgba = vec![0u8; CARD_WIDTH as usize * height as usize * BYTES_PER_PIXEL];
    let mut controls: Vec<ButtonBox> = Vec::new();
    let mut panel = None;
    // A canvas of the exact size we just allocated cannot be refused; the
    // `else` arm keeps a core bug from becoming a panic in a compositor loop
    // and returns a card that is merely empty.
    let Some(mut canvas) = Canvas::new(&mut rgba, CARD_WIDTH, height) else {
        return Card {
            rgba,
            width: CARD_WIDTH,
            height,
            buttons: Vec::new(),
            controls,
            // An empty card drew no panel, so it is not navigable. Stated
            // rather than defaulted: the alternative -- reporting slots that
            // were never painted -- is the exact divergence `Card::panel`'s
            // doc says this field cannot have.
            panel,
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
    for row in rows {
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
            Row::Transcribed { text: t, width } => {
                let metrics = text.line_metrics(NAME_PX);
                draw_transcribed(
                    &mut canvas,
                    text,
                    t,
                    CONTENT_X as i32,
                    y + metrics.ascent as i32,
                    *width,
                    &NAME_PALETTE,
                );
            }
            Row::Buttons { actions, focused } => {
                controls = draw_buttons(&mut canvas, text, actions, *focused, y);
            }
            Row::Panel(content) => {
                panel = Some(draw_panel(&mut canvas, text, content, y));
            }
        }
        y += row_height(row, text) as i32;
    }

    Card {
        // Narrowed here and nowhere else, so the two lists come from one
        // paint pass: see `Card::buttons`.
        buttons: controls.iter().filter_map(ButtonBox::as_choice).collect(),
        rgba,
        width: CARD_WIDTH,
        height,
        controls,
        panel,
    }
}

/// Paint the interactive panel and report where each slot landed.
///
/// Paints and reports in one pass, for [`ButtonBox`]' reason: a slot the human
/// can hit is a slot that was drawn, and there is no second traversal that
/// could compute a rectangle the pixels disagree with.
///
/// # What a row draws, and the three independent tells
///
/// This drew **no text at all** until P2.6.6 — every slot was a bare
/// rectangle, because [`PanelContent`] carried no name to put in one. It
/// carries [`Transcript`]s now, and that type's doc makes the argument for
/// why that is admissible. What this function owes is drawing them so that a
/// human can act on the distinctions the transcript preserved:
///
/// 1. **The name**, transcribed, with each [`Class`] run in its own colour —
///    [`NAME_PALETTE`]`.ascii` for ASCII, `.native` for a character drawn as
///    itself outside ASCII, `.escaped` for one that was not drawn as itself
///    at all. An escape is therefore a *second*, independent tell beyond the
///    character shapes: a human who cannot tell `\u{0435}` from a Cyrillic
///    letter by its glyphs can still see that the row changed colour.
/// 2. **The script label**, naming any non-Latin group present, so a wholly
///    Cyrillic lookalike announces itself where the mixed-script rule (which
///    only fires *within* a word) has nothing to say.
/// 3. **The gutter mark**, when [`crate::picker::listing`] found this row
///    rendering the same as another in the same listing.
///
/// Each is drawn from its own field, so losing one does not silently lose the
/// others.
///
/// # The gutter is reserved, and the name column is exactly the digest's
///
/// The name is elided to [`NAME_FIELD_PX`] — the constant
/// [`crate::picker::session`] elides and digests at — so the column the
/// collision repair reasons about and the column this paints are the same
/// number rather than two numbers that happen to agree. That module used to
/// record the obligation as owed and unmet ("nothing checks it against a
/// renderer, because there is no renderer"); this is the renderer, and
/// `the_name_column_is_exactly_the_width_the_digest_elides_at` is the check.
/// The gutter sits [`PANEL_GUTTER_GAP`] plus a script column to the right of
/// that width, so elision cannot reach it — which is the whole reason the
/// repair's mark is a gutter and not a suffix.
///
/// Elision bounds the name's **advance**, not its ink: 148 of the 5460 routed
/// glyphs spill right of their advance, by a pixel or two. The gap after the
/// name column absorbs that; the gutter is more than a hundred pixels
/// further right and cannot be reached by it.
fn draw_panel(
    canvas: &mut Canvas<'_>,
    text: &mut Text,
    content: &PanelContent,
    y: i32,
) -> Vec<SlotBox> {
    let mut slots = Vec::with_capacity(PANEL_ROWS as usize);
    let slot_w = CONTENT_W - PANEL_THUMB_W - 4;
    let metrics = text.line_metrics(NAME_PX);
    for index in 0..PANEL_ROWS {
        let rect = Rect {
            x: CONTENT_X as i32,
            y: y + (index as u32 * PANEL_ROW_H) as i32,
            w: slot_w,
            h: PANEL_ROW_H,
        };
        // A slot beyond what the content fills is drawn as background: it is
        // still a rectangle and still reported, so the geometry a refresh
        // produces never changes shape, only appearance.
        let row = content.rows.get(index as usize);
        if content.highlight == Some(index) && row.is_some() {
            // The highlighted row is filled *and* stroked. On the picker this
            // is the row a confirm commits, so it has to be unmissable; the
            // fill is a quiet grey rather than the accent, because filling it
            // in the accent colour would make a resting cursor look like the
            // armed state a press produces.
            canvas.fill_rect(rect, PANEL_SELECTED_BG);
            canvas.stroke_rect(rect, ACCENT, BORDER);
        }
        if let Some(row) = row {
            let baseline = panel_baseline(rect.y, metrics);
            draw_transcribed(
                canvas,
                text,
                &row.name,
                CONTENT_X as i32,
                baseline,
                NAME_FIELD_PX,
                &NAME_PALETTE,
            );
            if let Some(label) = script_label(&row.name) {
                let ly = rect.y
                    + ((PANEL_ROW_H.saturating_sub(text.line_metrics(LABEL_PX).height)) / 2) as i32
                    + text.line_metrics(LABEL_PX).ascent as i32;
                text.draw(canvas, &label, LABEL_PX, script_x() as i32, ly, SCRIPT_FG);
            }
            if let Some(tag) = row.tag.as_ref() {
                // The mark is drawn in the accent, not in a name colour: it
                // is the core speaking about the row rather than part of the
                // row's own name, and a human must be able to tell those
                // apart on a surface whose whole job is that distinction.
                draw_transcribed(
                    canvas,
                    text,
                    tag,
                    gutter_x() as i32,
                    baseline,
                    PANEL_GUTTER_W,
                    &TAG_PALETTE,
                );
            }
        }
        slots.push(SlotBox { index, rect });
    }

    // The thumb: a proportional marker in the right-hand gutter. Drawn from
    // `offset`/`total` rather than from `filled`, so a panel showing eight of
    // eight hundred says so.
    if content.total > PANEL_ROWS as u32 {
        let track_h = PANEL_H;
        let thumb_h = ((PANEL_ROWS as u64 * track_h as u64) / content.total as u64).max(4) as u32;
        let travel = track_h.saturating_sub(thumb_h);
        let scrollable = content.total.saturating_sub(PANEL_ROWS as u32).max(1);
        let thumb_y =
            ((content.offset.min(scrollable) as u64 * travel as u64) / scrollable as u64) as u32;
        canvas.fill_rect(
            Rect {
                x: (CONTENT_X + CONTENT_W - PANEL_THUMB_W) as i32,
                y: y + thumb_y as i32,
                w: PANEL_THUMB_W,
                h: thumb_h,
            },
            ACCENT,
        );
    }

    slots
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
    // **The program**, on a `realm_launch` petition and nowhere else
    // (WS-E.1.1, issue #207). Placed under the realm and above the verb
    // list because it *narrows* the realm: this card is about starting one
    // named program, and a human reading top to bottom should know which
    // one before they read what the principal may do with it.
    if let Some(command) = &prompt.command {
        field(
            &mut rows,
            text,
            LABEL_LAUNCHES,
            &[command.as_path().display().to_string()],
        );
    }
    field(&mut rows, text, LABEL_REQUESTS, &verb_lines(prompt.verbs));
    field(&mut rows, text, LABEL_EXPIRES, &[expiry_line(prompt)]);

    // The interactive panel, between the fields and the choice row: what is
    // being chosen sits above the choice, and the choice row stays last so
    // its position in the card is the same whether or not a panel is drawn.
    //
    // **The only condition anywhere that makes a card navigable.** Nothing in
    // the shipped tree constructs a `PanelContent`, so this is `None` in
    // every card this core can build today -- see `PromptContent::panel`.
    if let Some(panel) = prompt.panel.as_ref() {
        rows.push(Row::Space(12));
        rows.push(Row::Panel(panel.clone()));
    }

    // The last field's trailing gap plus this one give the separator the same
    // breathing room the header's has.
    rows.push(Row::Space(4));
    rows.push(Row::Rule);
    rows.push(Row::Space(16));
    rows.push(Row::Buttons {
        actions: prompt
            .choices()
            .into_iter()
            .map(CardAction::Decide)
            .collect(),
        // A grant-request card is answered by pointer, never by keyboard
        // (`grab`'s keyboard section: no key answers a consent prompt), so
        // there is no focus to draw.
        focused: None,
    });
    rows
}

/// Vertical extent of one row.
fn row_height(row: &Row, text: &Text) -> u32 {
    match row {
        Row::Space(n) => *n,
        Row::Rule => RULE_H,
        Row::Line { px, .. } => text.line_metrics(*px).height,
        Row::Transcribed { .. } => text.line_metrics(NAME_PX).height,
        Row::Buttons { .. } => BUTTON_H,
        // A CONSTANT, and never a function of the panel's content. See
        // `PANEL_ROWS`: this is what keeps a refresh from resizing the card,
        // moving its origin, or moving the choice row under a finger.
        Row::Panel(_) => PANEL_H,
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

/// Draw the button row at `y` and report where each button landed.
///
/// Equal widths, computed once and used for every button; the row is centered
/// inside the content column and any remainder from the integer division goes
/// to the outer margins (module docs: no choice may be a pixel wider than
/// another).
///
/// **The equal-geometry rule applies to the picker's two buttons too**, and
/// for the same reason one level over: Confirm hands a descriptor to an agent
/// and Cancel hands over nothing, so a TCB that drew one of them larger,
/// brighter or first-among-equals would be steering a decision it exists to
/// ask.
fn draw_buttons(
    canvas: &mut Canvas<'_>,
    text: &mut Text,
    actions: &[CardAction],
    focused: Option<usize>,
    y: i32,
) -> Vec<ButtonBox> {
    let n = actions.len() as u32;
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

    let mut boxes = Vec::with_capacity(actions.len());
    for (i, action) in actions.iter().enumerate() {
        let rect = Rect {
            x,
            y,
            w: width,
            h: BUTTON_H,
        };
        canvas.fill_rect(rect, BUTTON_BG);
        // The focus ring is the *only* thing that distinguishes one button
        // from another here, and it moves with Tab rather than being a
        // property of which button this is. The fill, the size and the type
        // stay identical, so this is a statement about where the keyboard is
        // and not about which answer the card prefers.
        if focused == Some(i) {
            canvas.stroke_rect(rect, ACCENT, BORDER);
        } else {
            canvas.stroke_rect(rect, BUTTON_BORDER, 1);
        }

        // Caption centered in the box. Measured with the same engine that
        // draws it, so a caption that grows past its button is visibly
        // clipped by the canvas rather than silently mis-centered.
        let label = action.label();
        let label_w = text.width(label, BUTTON_PX);
        let metrics = text.line_metrics(BUTTON_PX);
        let tx = x + ((width.saturating_sub(label_w)) / 2) as i32;
        let ty = y + ((BUTTON_H.saturating_sub(metrics.height)) / 2) as i32 + metrics.ascent as i32;
        text.draw(canvas, label, BUTTON_PX, tx, ty, BUTTON_FG);

        boxes.push(ButtonBox {
            action: *action,
            rect,
        });
        x += (width + BUTTON_GAP) as i32;
    }
    boxes
}

// ---------------------------------------------------------------------------
// The picker card (P2.6.6, issue #190)
// ---------------------------------------------------------------------------

/// Everything the picker card is allowed to say about one raised designation.
///
/// [`PromptContent`]'s sibling, and it keeps the same rule with one widening
/// that [`PanelContent`] argues for at length: the fields are typed
/// core-derived values, and the only variable *text* on the card is
/// [`Transcript`]s, which nothing but
/// [`crate::paint::transcript::encode`] can produce. There is no free-text
/// field here either.
///
/// **Why this is a second card rather than a panelled [`PromptContent`].** A
/// designation grants nothing: it hands over one descriptor the human picked,
/// and it resolves no petition. A grant-request card asks "may this principal
/// have this authority" and offers Allow/Deny. Drawing that card for a
/// designation would put Allow and Deny under a question they do not answer,
/// which is what [`super::grab::ConsentGrab::raise_picker`] refuses to do.
/// The two share the renderer, the palette, the panel and the placement; they
/// do not share the question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerContent {
    /// The principal that asked. From `vitrin_principal.bound`, by way of the
    /// designation ticket — never anything the client sent with the ask.
    pub principal: PrincipalIdentity,
    /// The realm the grant named when the ask was admitted.
    pub realm: RealmId,
    /// What was asked for. Decides the title, the consequence line, and
    /// nothing else.
    pub ask: AskedFor,
    /// Where inside the picker root the human is standing, as the components
    /// walked from it joined by `/` and put through `encode`.
    ///
    /// **Attacker-influenced, exactly like a row**: a directory the human
    /// descended into was named by whoever created it. It is a `Transcript`
    /// for that reason and not for symmetry.
    pub location: Transcript,
    /// The listing.
    pub panel: PanelContent,
    /// Which control the keyboard is on.
    pub focus: Focus,
}

/// The picker card's title, by what was asked for.
const TITLE_PICK_FILE: &str = "Choose a file to hand over";
const TITLE_PICK_DIR: &str = "Choose a folder to hand over";

/// What confirming actually does, per ask.
///
/// Each line says the same three things the verb catalogue's `designate_file`
/// entry says, because they are the facts a human cannot infer and the IDL
/// states outright: **one** item, a handle that lasts until the app exits,
/// and a handle revoking the grant does not take back.
///
/// The directory line deliberately does **not** claim read-only. The
/// descriptor is opened `O_RDONLY`, but a directory descriptor is an anchor
/// rather than a permission: what the app opens *through* it carries its own
/// mode. Saying "read-only" would be the kind of narrow-sounding claim a human
/// would rely on and this core does not enforce.
const CONSEQUENCE_READ: &str =
    "This app gets a read-only handle to the one file you pick, and keeps it until it exits. \
     Revoking the grant does not take it back.";
const CONSEQUENCE_WRITE: &str =
    "This app gets a read-and-write handle to the one file you pick, and keeps it until it \
     exits. Revoking the grant does not take it back.";
const CONSEQUENCE_DIR: &str =
    "This app gets a handle to the one folder you pick and can reach everything under it, and \
     keeps it until it exits. Revoking the grant does not take it back.";

const LABEL_LOCATION: &str = "Folder";
const LABEL_FILTER: &str = "Filter";
/// What the location field says at the top of the picker root. Not the root's
/// filesystem path: that is the operator's business, it is not something the
/// human chose, and printing it would put an operator-written path on the one
/// surface this card is trying to keep small.
const LOCATION_ROOT: &str = "(the folder this deployment offers)";
/// What the filter field says before anything is typed.
const FILTER_EMPTY: &str = "(type to filter)";

/// **Escape is named, and named as not working here.**
///
/// A human in front of a modal card reaches for Escape, and on this one it
/// does nothing: it belongs to the dead-man chord ([`crate::picker::keys`]),
/// which is a decision this card must not leave the human to discover by
/// pressing it. So the hint says which keys do work *and* says what Escape is
/// instead — a card that silently swallowed the key a human reaches for to
/// stop something would be the worst possible place to be quiet.
const PICKER_HINT: &str =
    "Arrows move, Tab reaches the buttons, typing filters. Escape does not close this card - it \
     is the emergency chord.";

/// The picker card's rows, top to bottom.
fn picker_rows(content: &PickerContent, text: &mut Text) -> Vec<Row> {
    let mut rows = Vec::new();
    let line = |rows: &mut Vec<Row>, s: &str, px: f32, rgb: [u8; 3]| {
        rows.push(Row::Line {
            text: s.to_string(),
            px,
            rgb,
        });
    };

    let (title, consequence) = match content.ask {
        AskedFor::File { write: false } => (TITLE_PICK_FILE, CONSEQUENCE_READ),
        AskedFor::File { write: true } => (TITLE_PICK_FILE, CONSEQUENCE_WRITE),
        AskedFor::Dir => (TITLE_PICK_DIR, CONSEQUENCE_DIR),
    };
    line(&mut rows, title, TITLE_PX, TITLE_FG);
    rows.push(Row::Space(6));
    for wrapped in text.wrap(consequence, LABEL_PX, CONTENT_W, 3) {
        line(&mut rows, &wrapped, LABEL_PX, MUTED_FG);
    }
    rows.push(Row::Space(14));
    rows.push(Row::Rule);
    rows.push(Row::Space(14));

    // Who is asking, and where. Both are the same values the grant-request
    // card draws, from the same types, so a human who has seen one card knows
    // where to look on the other.
    let field = |rows: &mut Vec<Row>, text: &mut Text, label: &str, value: &str| {
        line(rows, label, LABEL_PX, MUTED_FG);
        rows.push(Row::Space(LABEL_GAP));
        for wrapped in text.wrap(value, VALUE_PX, CONTENT_W, MAX_VALUE_LINES) {
            line(rows, &wrapped, VALUE_PX, VALUE_FG);
        }
        rows.push(Row::Space(GROUP_GAP));
    };
    field(
        &mut rows,
        text,
        LABEL_PRINCIPAL,
        &content.principal.to_string(),
    );
    field(&mut rows, text, LABEL_REALM, &content.realm.to_string());

    // Where the human is standing. Transcribed, because a directory name is
    // whatever whoever created it chose.
    line(&mut rows, LABEL_LOCATION, LABEL_PX, MUTED_FG);
    rows.push(Row::Space(LABEL_GAP));
    if content.location.text.is_empty() {
        line(&mut rows, LOCATION_ROOT, NAME_PX, MUTED_FG);
    } else {
        rows.push(Row::Transcribed {
            text: content.location.clone(),
            width: CONTENT_W,
        });
    }
    rows.push(Row::Space(GROUP_GAP));

    // The query echo. A human filtering has to be able to see what the core
    // thinks they typed, or a swallowed or mis-decoded keystroke reads as a
    // directory that suddenly has fewer files in it.
    line(&mut rows, LABEL_FILTER, LABEL_PX, MUTED_FG);
    rows.push(Row::Space(LABEL_GAP));
    if content.panel.query.text.is_empty() {
        line(&mut rows, FILTER_EMPTY, NAME_PX, MUTED_FG);
    } else {
        rows.push(Row::Transcribed {
            text: content.panel.query.clone(),
            width: CONTENT_W,
        });
    }
    rows.push(Row::Space(10));

    rows.push(Row::Panel(content.panel.clone()));

    rows.push(Row::Space(10));
    rows.push(Row::Rule);
    rows.push(Row::Space(14));
    rows.push(Row::Buttons {
        actions: vec![CardAction::Confirm, CardAction::Cancel],
        focused: match content.focus {
            Focus::Listing => None,
            Focus::Confirm => Some(0),
            Focus::Cancel => Some(1),
        },
    });
    rows.push(Row::Space(10));
    for wrapped in text.wrap(PICKER_HINT, LABEL_PX, CONTENT_W, 2) {
        line(&mut rows, &wrapped, LABEL_PX, MUTED_FG);
    }
    rows
}

/// Draw one transcript at `baseline`, tinting each run and eliding to
/// `max_w`; returns the advance actually drawn.
///
/// # Why this measures with `width_vetted` and never `Text::width`
///
/// [`Text::width`] measures through the ASCII substitution, so it measures a
/// natively-drawn `é` or `漢` as `?` — roughly half the advance of a wide
/// glyph. A column laid out with `width` and drawn with [`Text::draw_runs`]
/// would be laid out for text it is not drawing, and a Japanese filename
/// would run past the end of the name field and into the gutter the collision
/// repair reserves. Every measurement here goes through
/// [`Text::elide_vetted`], which is the same pen over the same characters as
/// the draw.
///
/// # The ellipsis is its own colour
///
/// A cut name is a name the human is not seeing all of, which on this surface
/// is a fact about trust and not about typography — two files sharing a long
/// prefix are exactly the pair a spoof is built from. So the marker is drawn
/// in [`MUTED_FG`] rather than in the colour of the run it follows, and the
/// gutter mark (drawn separately, and reserved from elision) is what actually
/// separates such a pair.
fn draw_transcribed(
    canvas: &mut Canvas<'_>,
    text: &mut Text,
    transcript: &Transcript,
    x: i32,
    baseline: i32,
    max_w: u32,
    palette: &RunPalette,
) -> u32 {
    let Some(whole) = Vetted::new(&transcript.text) else {
        // Unreachable: `encode` emits only characters `route` admits, which
        // is exactly `Vetted`'s check. Answered rather than asserted, on this
        // module's standing rule -- a core bug must not become a panic in a
        // compositor loop -- and loudly, because a transcript this renderer
        // cannot draw would mean the two halves of #190 had drifted apart.
        tracing::error!("a transcript reached the picker card that the renderer cannot draw");
        return 0;
    };
    let cut = text.elide_vetted(&whole, max_w);
    let end = cut.unwrap_or(transcript.text.len());

    let mut runs: Vec<(Vetted<'_>, [u8; 3])> = Vec::new();
    for (range, class) in &transcript.runs {
        let (start, stop) = (range.start.min(end), range.end.min(end));
        if start >= stop {
            continue;
        }
        // Run boundaries are character boundaries (`encode` emits whole
        // characters) and so is `end` (`elide_vetted` walks `char_indices`),
        // so this slice cannot split a codepoint.
        let Some(v) = Vetted::new(&transcript.text[start..stop]) else {
            continue;
        };
        runs.push((v, palette.of(*class)));
    }
    if cut.is_some() {
        let ellipsis = Vetted::new(crate::paint::text::ELLIPSIS)
            .expect("`...` is ASCII, which the router admits");
        runs.push((ellipsis, MUTED_FG));
    }
    text.draw_runs(canvas, &runs, x, baseline)
}

/// The text baseline of a panel row whose slot starts at `top`.
///
/// Factored out because the picker card's tests reconstruct a row's expected
/// pixels and have to place them where the renderer really placed them; a
/// test that recomputed this from its own arithmetic would pass while drifting
/// from the thing it is checking.
fn panel_baseline(top: i32, metrics: crate::paint::text::LineMetrics) -> i32 {
    top + ((PANEL_ROW_H.saturating_sub(metrics.height)) / 2) as i32 + metrics.ascent as i32
}

/// The scripts named beside a row, or `None` when there is nothing to say.
///
/// **Latin is not named**, and that is a decision rather than an oversight.
/// [`Transcript::groups`] is the honest set of what is on screen, and an ASCII
/// filename really is Latin — but a label on every row of every listing is a
/// label nobody reads, and this label's whole job is to make an *unusual* row
/// announce itself. So the renderer names what a reader of this deployment's
/// alphabet would not otherwise expect, which is exactly the case the label
/// exists for: the mixed-script rule fires only *within* a word, so a wholly
/// Cyrillic lookalike passes it cleanly and this is what tells the human.
///
/// Escaped characters contribute nothing, because `encode` already leaves
/// them out of `groups`: naming a script that was not drawn would describe
/// something the human cannot see.
///
/// **Elision is the one exception, and it errs loud.** `groups` is a property
/// of the whole transcript, and [`draw_panel`] labels a row from it even when
/// [`NAME_FIELD_PX`] cut the only Cyrillic word off the end. So a long name
/// can carry a label for a script that is not on screen. Left that way on
/// purpose: the alternative is recomputing the set over the surviving prefix,
/// which would *drop* the warning on exactly the rows elision makes hardest
/// to read, and this label's failure direction has to be "says more than is
/// visible", never "says less".
///
/// # `Japanese` cannot reach this today, and that is a gap rather than a
/// design
///
/// [`crate::paint::transcript::encode`] marks a character drawable only when
/// [`crate::paint::script::route`] returns `Route::Vector`, and Japanese
/// routes to `Route::Atlas`. So a kana or Han character is escaped, never
/// added to `groups`, and this arm is unreachable — the pre-rasterized atlas
/// that exists to draw it is not reached from a filename at all.
/// `the_script_label_is_not_vacuous` measures the two groups that *do* reach
/// it and states this one as unreachable rather than leaving a reader to
/// assume all four work. Closing it is a change to `encode`'s drawability
/// test, not to this function, and it is not this task's.
fn script_label(transcript: &Transcript) -> Option<String> {
    let named: Vec<&'static str> = transcript
        .groups
        .iter()
        .filter(|group| **group != Group::Latin)
        .map(|group| group_name(*group))
        .collect();
    (!named.is_empty()).then(|| named.join(SCRIPT_JOIN))
}

/// Separator when a name draws characters from more than one non-Latin group.
/// Reachable across word boundaries (`αβ.где`), which
/// [`crate::paint::script::permitted`] allows because the mixing is not inside
/// one word.
const SCRIPT_JOIN: &str = "+";

/// A group's name, exhaustively matched so a group added to the enum fails to
/// compile until somebody decides what to call it in front of a human.
fn group_name(group: Group) -> &'static str {
    match group {
        Group::Latin => "Latin",
        Group::Greek => "Greek",
        Group::Cyrillic => "Cyrillic",
        Group::Japanese => "Japanese",
    }
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

    /// **A `realm_launch` card names the program, and no other card does**
    /// (WS-E.1.1, issue #207).
    ///
    /// Both halves are the assertion. Naming it is Q13's requirement — a
    /// human approving "start apps in this realm" without being told *which*
    /// app is being asked to consent to the one fact the verb's whole
    /// security story rests on. Not naming it elsewhere is the card's own
    /// rule: a prompt says what is being asked for and nothing else.
    #[test]
    fn a_launch_prompt_names_the_program_and_only_a_launch_prompt_does() {
        let mut text = Text::new();
        let program = "/usr/bin/kiosk-browser";

        let mut launch = prompt_fixture();
        launch.verbs = launch.verbs | Verb::REALM_LAUNCH;
        launch.command = Some(crate::realm::AuditedCommand::for_test(program));
        let lines = line_texts(&launch, &mut text);
        assert!(
            lines.iter().any(|l| l == LABEL_LAUNCHES),
            "the launch field must be labelled: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains(program)),
            "the program the human is approving must be on the card: {lines:?}"
        );
        // ...and it is drawn, not merely laid out: the card really got
        // taller than the same petition without the field.
        let mut without = launch.clone();
        without.command = None;
        assert!(
            rasterize(&launch).height > rasterize(&without).height,
            "the Launches field must occupy real rows on the card"
        );

        // The ordinary petition is unchanged -- no label, no path.
        let plain = line_texts(&prompt_fixture(), &mut text);
        assert!(
            !plain.iter().any(|l| l == LABEL_LAUNCHES),
            "a petition that does not ask to launch must not name a program: {plain:?}"
        );
    }

    /// Every text line one card renders, in order — the lines a human reads.
    fn line_texts(prompt: &PromptContent, text: &mut Text) -> Vec<String> {
        rows(prompt, text)
            .into_iter()
            .filter_map(|row| match row {
                Row::Line { text, .. } => Some(text),
                _ => None,
            })
            .collect()
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
            "VERB_CATALOGUE does not cover every verb this core can grant"
        );
        // Verbs the IDL defines but this core refuses `unsupported` are
        // absent on purpose: no petition naming one ever reaches a prompt.
        assert_eq!(
            union & crate::grants::UNSERVED_VERB_BITS,
            0,
            "VERB_CATALOGUE names a verb this core refuses at admission"
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
        //
        // **Re-pinned by WS-E.1.1 (issue #207) with a decision on the bit
        // that moved, not a mechanical subtraction.** `realm_launch` left
        // this pin because the core gained the thing its refusal stood for:
        // a chokepoint arm that forks, the realm cap, a core-minted instance
        // id, and — the part this file owns — a catalogue line and a
        // `Launches` field naming the program. That is Q13's rule applied
        // exactly: the verb shipped admitted-but-refused `unsupported` until
        // its copy existed, and its copy exists here.
        //
        // WS-E.1.4 (issue #210) moved `layout_arrange` and `layout_focus`
        // out on the same terms.
        //
        // vitrin-verb-set: unserved-verbs = observe_cursor, egress
        //
        // **Two verbs are pinned here: `observe_cursor` and `egress`.** The
        // marker is not the enumeration -- this sentence is, and it is what a
        // reader believes -- so both are named on it rather than left to the
        // paragraphs below.
        //
        // `observe_cursor` stays, and for a
        // reason that has not moved: per-principal cursor *delivery* is
        // still M2's (D-017/D-019 both say so in as many words), so serving
        // the verb would widen a capture with a cursor the core does not
        // have.
        //
        // **`designate_file` (64) LEFT this pin at P2.6.6 (issue #190)**,
        // the third shrink. It was appended to the IDL at P2.6.5 and pinned
        // unserved on two grounds — no picker to mint a descriptor, and no
        // consent copy under Q13's rule. The first ground is gone: the
        // core-drawn picker ([`crate::picker`]) exists and the chokepoint has
        // a sink to reach it.
        //
        // **The second ground went at P2.6.8 (issue #192, D-048)**, two
        // tasks after the first, and the gap between them is recorded rather
        // than smoothed over: from P2.6.6 to P2.6.8 the verb was served with
        // a minimum honest line, so Q13's rule was met in the letter (the
        // verb was named on the card) and not yet in spirit (nobody had
        // reviewed what the line asked a human to approve). That was a
        // weaker state than `realm_launch`'s, whose copy was written by the
        // task that served it. The considered copy is the catalogue entry
        // above, with every clause traced to the IDL in its comment, and
        // `every_catalogue_line_fits_untruncated` holds that a human can
        // read all of it.
        //
        // **`egress` (128) JOINED the pin at P2.7.2 (issue #196)**, the
        // second growth, and again
        // this tripwire firing exactly as designed: the IDL gained a bit and
        // this line went red until a human classified it. It is classified
        // **unserved**, and there is no catalogue line above for it, because
        // the mechanism the verb names does not exist: the out-of-core
        // mediating proxy is P2.7.3's. Prompt copy for authority no code
        // enforces would be the exact lie the catalogue exists to prevent.
        //
        // **Its facet landed and this pin did not move**, which is the
        // distinction worth having in writing here. P2.7.2's second half
        // added `vitrin_egress` — an interface of its own rather than a
        // request on P2.6.5's filesystem powerbox, since `interface/@verb` is
        // one value per interface — so the verb now has a request to be
        // exercised through and still has nothing to answer with. A facet
        // changes what the wire can express; only a mechanism changes what
        // this core can enforce, and only the second moves a bit out of
        // `UNSERVED_VERB_BITS`. The same is true of `designate_file` and
        // `vitrin_powerbox`, whose facet landed a release before its
        // mechanism did. `egress` leaves this pin when P2.7.3 lands the
        // proxy, not before.
        assert_eq!(
            crate::grants::UNSERVED_VERB_BITS,
            (Verb::OBSERVE_CURSOR | Verb::EGRESS).bits(),
            "the IDL defines a verb this module has not classified as served \
             or unserved (D-017/D-018)"
        );
        // ...and the two really do partition the wire bitfield. Implied by the
        // derivation above; asserted so the invariant is written down.
        assert_eq!(
            crate::grants::SERVED_VERB_BITS | crate::grants::UNSERVED_VERB_BITS,
            Verb::VALID_MASK
        );
        // **A layout verb served with the old copy fails here** (WS-E.1.7).
        // Q13's rule -- "each new verb ships admitted-but-refused
        // `unsupported` until its copy exists" -- applies to a *widened* verb
        // too, and the widening is not cosmetic: the grant now receives a
        // session-level event about the human, and is not refused `preempted`
        // in the moment after they press the key. A prompt that omitted it
        // would be asking a human to approve authority it does not name, which
        // is the exact failure the catalogue exists to prevent.
        for (verb, name, what) in VERB_CATALOGUE {
            if !crate::attention::EXEMPT_VERBS.contains(&verb) {
                continue;
            }
            assert!(
                what.contains("attention key"),
                "`{name}` is exempted by the attention key, so its consent copy must say so \
                 (Q13, applied to a widened verb)"
            );
        }
        // ...and the copy really reaches a rendered prompt, not just the
        // table: the line a human reads is the one asserted above.
        for verb in crate::attention::EXEMPT_VERBS {
            let lines = verb_lines(verb);
            assert_eq!(lines.len(), 1);
            assert!(
                lines[0].contains("attention key"),
                "the rendered line for {verb:?} must name the attention key: {}",
                lines[0]
            );
        }
        // Names match the IDL's spelling, so prompt and protocol cannot drift.
        let names: Vec<&str> = VERB_CATALOGUE.iter().map(|(_, name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "observe",
                "actuate_pointer",
                "actuate_text",
                "layout_arrange",
                "layout_focus",
                "realm_launch",
                "designate_file"
            ]
        );
    }

    /// **Every catalogue line reaches the human whole** (P2.6.8, issue #192,
    /// D-048).
    ///
    /// The card wraps each line `verb_lines` produces independently into
    /// at most [`MAX_VALUE_LINES`] lines of [`CONTENT_W`] at [`VALUE_PX`], and
    /// [`Text::wrap`] cuts overflow with a visible
    /// [`crate::paint::text::ELLIPSIS`] -- the right
    /// behaviour for an operator-registered identity that is too long, and
    /// the wrong outcome for consent copy, where the clause that gets cut is
    /// by construction the last one, and the last clause of every line here
    /// is the one a human cannot infer ("even if you revoke this grant").
    /// Before this test the only answer to "does the copy fit" was a pixel
    /// measurement in a comment; this is the mechanical answer, for every
    /// line present and future, in the exact code path the card uses.
    ///
    /// Two checks that must both hold. The wrap at the card's real limit must
    /// equal the wrap with no limit at all -- if they differ, the limit is
    /// what made them differ -- and no produced line may end in the marker,
    /// which is the same fact asserted from the human's side of the glass.
    #[test]
    fn every_catalogue_line_fits_untruncated() {
        let mut text = Text::new();
        for (verb, name, _) in VERB_CATALOGUE {
            // The renderer's own line, not a rebuilt `- {name}: {what}`: a
            // wider prefix or a changed separator in `verb_lines` must be
            // measured here too, or this test measures a string the card
            // never draws.
            let lines = verb_lines(verb);
            assert_eq!(
                lines.len(),
                1,
                "one catalogue verb renders exactly one line"
            );
            let value = &lines[0];
            let on_card = text.wrap(value, VALUE_PX, CONTENT_W, MAX_VALUE_LINES);
            let unbounded = text.wrap(value, VALUE_PX, CONTENT_W, usize::MAX);
            assert!(
                on_card.len() <= MAX_VALUE_LINES,
                "`{name}` wraps to {} lines; the card draws at most {MAX_VALUE_LINES}",
                on_card.len()
            );
            assert_eq!(
                on_card, unbounded,
                "`{name}`'s copy is cut by the card's {MAX_VALUE_LINES}-line budget: the human \
                 would read a truncated consequence"
            );
            for line in &on_card {
                assert!(
                    !line.ends_with(crate::paint::text::ELLIPSIS),
                    "`{name}` renders a truncation marker on a consent card: {line:?}"
                );
            }
        }
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

    // -----------------------------------------------------------------
    // The picker card (P2.6.6, issue #190)
    // -----------------------------------------------------------------

    use crate::consent::tests::{picker_fixture, picker_row_names};
    use crate::consent::PanelRow;
    use crate::paint::transcript::encode;

    /// A picker card whose panel holds exactly `names`, nothing highlighted
    /// and nothing marked — so a row's pixels are the row's alone, with card
    /// background above and below it and no fill under it.
    fn card_of(names: &[&[u8]]) -> Card {
        let mut content = picker_fixture();
        content.panel.rows = names
            .iter()
            .map(|name| PanelRow {
                name: encode(name),
                tag: None,
            })
            .collect();
        content.panel.highlight = None;
        content.panel.offset = 0;
        content.panel.total = names.len() as u32;
        rasterize_picker(&content)
    }

    /// The pixels of one panel row's **name field** — the column the digest
    /// elides at, and nothing else: the script label and the gutter live to
    /// the right of it and are outside this crop by construction.
    fn name_field(card: &Card, index: usize) -> Vec<[u8; 4]> {
        let slot = card.panel.as_ref().expect("a picker card paints a panel")[index];
        let mut out = Vec::new();
        for row in 0..PANEL_ROW_H {
            for col in 0..NAME_FIELD_PX {
                out.push(px(card, CONTENT_X + col, slot.rect.y as u32 + row));
            }
        }
        out
    }

    /// The same crop, drawn from scratch out of runs a test spells out in
    /// ASCII — the reference a rasterized row is compared against.
    fn reference_name_field(runs: &[(&str, [u8; 3])]) -> Vec<[u8; 4]> {
        let (w, h) = (NAME_FIELD_PX, PANEL_ROW_H);
        let mut buf = vec![0u8; w as usize * h as usize * BYTES_PER_PIXEL];
        let mut text = Text::new();
        let baseline = panel_baseline(0, text.line_metrics(NAME_PX));
        {
            let mut canvas = Canvas::new(&mut buf, w, h).expect("scratch canvas");
            canvas.fill_rect(Rect { x: 0, y: 0, w, h }, CARD_BG);
            let vetted: Vec<(Vetted<'_>, [u8; 3])> = runs
                .iter()
                .map(|(s, rgb)| {
                    (
                        Vetted::new(s).expect("the reference runs are drawable"),
                        *rgb,
                    )
                })
                .collect();
            text.draw_runs(&mut canvas, &vetted, 0, baseline);
        }
        buf.chunks_exact(BYTES_PER_PIXEL)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect()
    }

    /// **A bidi override renders transcribed — checked on the pixels.**
    ///
    /// `U+202E RIGHT-TO-LEFT OVERRIDE` is the classic filename spoof: with a
    /// bidi algorithm, `\u{202E}txt.exe` displays as `exe.txt`. This renderer
    /// has no bidi algorithm and must not grow one
    /// ([`crate::paint::script`]), so the override has to be *shown* rather
    /// than obeyed or dropped.
    ///
    /// Asserted against the **rasterized card**, not against the transcript
    /// string, and that distinction is the whole point of the test: a
    /// renderer that produced the right transcript and then drew something
    /// else — substituted it, reordered it, dropped the escape — would pass a
    /// string comparison and fail here. The reference is built by drawing the
    /// escape form's own ASCII characters through the same primitive, so what
    /// this asserts is "the pixels on the card are the pixels of
    /// `\u{202E}txt.exe` spelled out".
    #[test]
    fn a_bidi_override_is_drawn_transcribed_on_the_card_itself() {
        let card = card_of(&["\u{202E}txt.exe".as_bytes()]);
        assert_eq!(
            name_field(&card, 0),
            reference_name_field(&[
                ("\\u{202E}", NAME_PALETTE.escaped),
                ("txt.exe", NAME_PALETTE.ascii),
            ]),
            "the override must be drawn as its escape form, in the escaped tint"
        );

        // ...and it did not silently vanish: a row named `txt.exe` with no
        // override draws different pixels. Without this the assertion above
        // would also pass on a renderer that dropped the character and drew
        // an escape form out of nowhere.
        let plain = card_of(&[b"txt.exe"]);
        assert_ne!(
            name_field(&card, 0),
            name_field(&plain, 0),
            "an override that made no difference to the pixels would be an \
             override the human cannot see"
        );
    }

    /// **A combining-mark stack renders transcribed — checked on the pixels.**
    ///
    /// The other half of the same hazard. There is no mark positioning here,
    /// so a combining acute would render as a spacing glyph *beside* its base
    /// rather than over it — `a\u{0301}` drawn naively is two characters that
    /// look like one wrong one. Transcribing it is the honest answer, and
    /// this checks the card really does it.
    #[test]
    fn a_combining_mark_is_drawn_transcribed_on_the_card_itself() {
        let card = card_of(&["a\u{0301}\u{0302}.txt".as_bytes()]);
        assert_eq!(
            name_field(&card, 0),
            reference_name_field(&[
                ("a", NAME_PALETTE.ascii),
                ("\\u{0301}\\u{0302}", NAME_PALETTE.escaped),
                (".txt", NAME_PALETTE.ascii),
            ]),
            "each combining mark must be drawn as its own escape form"
        );
    }

    /// A natively-drawn non-ASCII run is drawn as itself, in the native tint.
    ///
    /// The complement of the two tests above, and it has to be here or they
    /// would be equally satisfied by a renderer that escaped *everything*.
    #[test]
    fn a_drawable_non_ascii_run_is_drawn_as_itself_and_tinted() {
        let card = card_of(&["отчёт.pdf".as_bytes()]);
        assert_eq!(
            name_field(&card, 0),
            reference_name_field(&[("отчёт", NAME_PALETTE.native), (".pdf", NAME_PALETTE.ascii),]),
            "a Cyrillic word with an ASCII extension is drawn, not escaped"
        );
    }

    /// **The escape tint is a second, independent tell.**
    ///
    /// The mixed-script rule escapes the Cyrillic `е` inside `rеsume.pdf`, and
    /// the escape form is what a human reads. That form is drawn in a colour
    /// no ASCII run uses, so a human who does not parse `\u{0435}` still sees
    /// that this row is not what it looks like.
    #[test]
    fn a_mixed_script_word_escapes_its_minority_in_the_escaped_tint() {
        let card = card_of(&["rеsume.pdf".as_bytes()]);
        assert_eq!(
            name_field(&card, 0),
            reference_name_field(&[
                ("r", NAME_PALETTE.ascii),
                ("\\u{0435}", NAME_PALETTE.escaped),
                ("sume.pdf", NAME_PALETTE.ascii),
            ])
        );
        // The lookalike it defends against draws differently, which is the
        // only thing that makes the defence worth anything.
        assert_ne!(
            name_field(&card, 0),
            name_field(&card_of(&[b"resume.pdf"]), 0)
        );
    }

    /// The three run classes are three **distinct** colours.
    ///
    /// Asserted on the palette directly rather than through the golden,
    /// because the golden's readable half is a mean-luma reduction and cannot
    /// witness a colour at all (see `picker_card_golden`). Luminance is
    /// checked too: a palette whose three entries differed only in hue would
    /// be invisible to a colour-blind human, and this surface is not a place
    /// to rely on hue alone.
    #[test]
    fn the_three_run_classes_are_three_distinct_colours() {
        let p = &NAME_PALETTE;
        assert_ne!(p.ascii, p.native);
        assert_ne!(p.ascii, p.escaped);
        assert_ne!(p.native, p.escaped);
        let luma = |c: [u8; 3]| {
            (u32::from(c[0]) * 299 + u32::from(c[1]) * 587 + u32::from(c[2]) * 114) / 1000
        };
        for (a, b) in [
            (p.ascii, p.native),
            (p.ascii, p.escaped),
            (p.native, p.escaped),
        ] {
            assert!(
                luma(a).abs_diff(luma(b)) >= 8,
                "{a:?} and {b:?} differ by less than 8 in luminance, so the tint \
                 would carry no signal without colour vision"
            );
        }
    }

    /// The pixels of one panel row across **everything left of the gutter** —
    /// the name field plus the space beyond it a name must never occupy.
    ///
    /// Wider than [`name_field`] on purpose: a column that elided too
    /// generously would draw *past* the name field, and a crop that stopped
    /// at the field would be blind to exactly the defect it is looking for.
    fn row_span(card: &Card, index: usize) -> Vec<[u8; 4]> {
        let slot = card.panel.as_ref().expect("a picker card paints a panel")[index];
        let mut out = Vec::new();
        for row in 0..PANEL_ROW_H {
            for col in CONTENT_X..gutter_x() {
                out.push(px(card, col, slot.rect.y as u32 + row));
            }
        }
        out
    }

    /// **The name column really is the width the digest elides at.**
    ///
    /// The obligation [`crate::picker::session`] recorded as owed while there
    /// was no renderer to check it against. It is checked through both
    /// shipped implementations rather than by comparing two constants, which
    /// would be an identity — and it is built **at the cut point itself**,
    /// because a pair that differs far past the cut would go on drawing alike
    /// however generously the renderer elided, and a pair that differs far
    /// before it would go on drawing differently however meanly it did.
    ///
    /// So the fixture asks [`Text::elide_vetted`] where the cut falls and
    /// puts the difference on either side of exactly that character:
    ///
    /// - differing at the **first dropped** character: the digest says alike,
    ///   and the card must draw them alike. A column even one character wider
    ///   than the digest's would draw the difference and fail.
    /// - differing at the **last kept** character: the digest says apart, and
    ///   the card must draw them apart. A column one character narrower would
    ///   cut the difference away and fail.
    #[test]
    fn the_name_column_is_exactly_the_width_the_digest_elides_at() {
        use crate::picker::session::digest_for_test;

        // Where the renderer's own elision falls on a name of this shape.
        let long = "a".repeat(200);
        let mut text = Text::new();
        let cut = text
            .elide_vetted(
                &Vetted::new(&long).expect("ASCII is drawable"),
                NAME_FIELD_PX,
            )
            .expect("200 characters do not fit the name field");
        assert!(
            cut > 8 && cut < 200,
            "the fixture must really be elided: cut at {cut}"
        );

        let at = |index: usize, differing: char| {
            let mut name = "a".repeat(index);
            name.push(differing);
            name.push_str(&"z".repeat(40));
            name.into_bytes()
        };

        // Differing at the first character elision drops.
        let (a, b) = (at(cut, 'b'), at(cut, 'c'));
        assert_eq!(
            digest_for_test(&a),
            digest_for_test(&b),
            "fixture check: a difference past the cut must be invisible to the digest, \
             or the assertion below is a statement about nothing"
        );
        let card = card_of(&[&a, &b]);
        assert_eq!(
            row_span(&card, 0),
            row_span(&card, 1),
            "the digest judged these rows identical, so the card must draw them \
             identically -- a name column wider than the digest's would draw the \
             character the digest could not see"
        );

        // Differing at the last character elision keeps.
        let (a, b) = (at(cut - 1, 'b'), at(cut - 1, 'c'));
        assert_ne!(
            digest_for_test(&a),
            digest_for_test(&b),
            "fixture check: a difference before the cut must be visible to the digest"
        );
        let card = card_of(&[&a, &b]);
        assert_ne!(
            row_span(&card, 0),
            row_span(&card, 1),
            "the digest judged these rows different, so the card must draw them \
             differently -- a name column narrower than the digest's would cut away \
             the difference the repair thought the human could see"
        );
    }

    /// **Nothing a name can contain reaches the reserved gutter.**
    ///
    /// The property the whole gutter design rests on: elision shortens the
    /// name, and the mark lives where elision cannot reach. Driven with the
    /// widest thing the router admits rather than with a plausible filename,
    /// because "a realistic name fits" is not the claim.
    #[test]
    fn no_name_can_reach_the_reserved_gutter() {
        // Repeated `W` and `ё`: the widest ASCII glyph and a non-ASCII one,
        // long enough that no elision could leave it short.
        let long = "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW"
            .repeat(3)
            .into_bytes();
        let wide = "ёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёёё"
            .repeat(3)
            .into_bytes();
        let escapes = "\u{202E}\u{202E}\u{202E}\u{202E}\u{202E}\u{202E}\u{202E}\u{202E}"
            .repeat(4)
            .into_bytes();
        let card = card_of(&[&long, &wide, &escapes]);
        let slots = card.panel.as_ref().expect("a panel");
        for (index, slot) in slots.iter().enumerate().take(3) {
            for row in 0..PANEL_ROW_H {
                for col in gutter_x()..(gutter_x() + PANEL_GUTTER_W) {
                    assert_eq!(
                        px(&card, col, slot.rect.y as u32 + row),
                        CARD_BG,
                        "row {index} put ink at ({col}, {row}) inside the gutter the \
                         collision repair reserves"
                    );
                }
            }
        }

        // ...and the gutter is not merely empty because nothing is ever drawn
        // there: a marked row inks it.
        let mut content = picker_fixture();
        content.panel.rows = vec![PanelRow {
            name: encode(&long),
            tag: Some(encode(b"#12")),
        }];
        content.panel.highlight = None;
        let marked = rasterize_picker(&content);
        let slot = marked.panel.as_ref().expect("a panel")[0];
        let inked = (0..PANEL_ROW_H)
            .flat_map(|row| {
                (gutter_x()..(gutter_x() + PANEL_GUTTER_W))
                    .map(move |col| (col, slot.rect.y as u32 + row))
            })
            .filter(|&(x, y)| px(&marked, x, y) != CARD_BG)
            .count();
        assert!(
            inked > 0,
            "a marked row must actually paint its gutter mark"
        );
    }

    /// The largest mark the repair can emit fits the column reserved for it.
    #[test]
    fn every_gutter_mark_fits_its_reserved_column() {
        let mut text = Text::new();
        // `#999` is the widest, by `crate::picker::listing::MAX_TAG`.
        let widest = encode(b"#999");
        let v = Vetted::new(&widest.text).expect("core-minted ASCII is drawable");
        assert!(
            text.width_vetted(&v) <= PANEL_GUTTER_W,
            "the widest gutter mark is {} px in a {PANEL_GUTTER_W} px column",
            text.width_vetted(&v)
        );
    }

    /// The script label names what a reader would not expect, and is not
    /// vacuous — it really appears for the groups that can reach it.
    ///
    /// `Japanese` is deliberately absent from the reachable set: `encode`
    /// admits only vector-routed characters, and Japanese routes to the
    /// atlas, so it is always escaped and never lands in `groups`. Recorded
    /// here as a measured fact rather than left for a reader to assume the
    /// fourth group works like the other three.
    #[test]
    fn the_script_label_is_not_vacuous() {
        assert_eq!(
            script_label(&encode("отчёт.pdf".as_bytes())).as_deref(),
            Some("Cyrillic")
        );
        assert_eq!(
            script_label(&encode("λόγος.txt".as_bytes())).as_deref(),
            Some("Greek")
        );
        // Pure Latin says nothing: a label on every row is a label nobody
        // reads, and this one exists to make an unusual row stand out.
        assert_eq!(script_label(&encode(b"notes.txt")), None);
        // The escaped minority is not named, because it was not drawn.
        assert_eq!(script_label(&encode("rеsume.pdf".as_bytes())), None);
        // The measured gap.
        let japanese = encode("論文.txt".as_bytes());
        assert_eq!(
            script_label(&japanese),
            None,
            "Japanese is escaped by `encode`, so it never reaches `groups` -- \
             the atlas is unreachable from a filename today"
        );
        assert!(japanese.text.contains("\\u{"), "...because it is escaped");
    }

    /// The label is drawn beside the row, not merely computed.
    #[test]
    fn a_non_latin_row_paints_its_script_label() {
        let labelled = card_of(&["отчёт.pdf".as_bytes()]);
        let plain = card_of(&[b"notes.txt"]);
        let slot = labelled.panel.as_ref().expect("a panel")[0];
        let ink = |card: &Card| {
            (0..PANEL_ROW_H)
                .flat_map(|row| {
                    (script_x()..gutter_x()).map(move |col| (col, slot.rect.y as u32 + row))
                })
                .filter(|&(x, y)| px(card, x, y) != CARD_BG)
                .count()
        };
        assert!(ink(&labelled) > 0, "the script column must be painted");
        assert_eq!(ink(&plain), 0, "an all-Latin row must not be labelled");
    }

    /// **The widest label the enum can produce fits between the name field
    /// and the gutter.**
    ///
    /// The const assert beside [`PANEL_GUTTER_W`] proves only that the column
    /// *exists* (`script_x() <= gutter_x()`); a label's width is a property
    /// of the shipped face, which no `const` can measure. Without this, a
    /// widened gutter or a longer group name would push the label into the
    /// reserved column and the only witness would be a blessed golden nobody
    /// reads closely — the same shape as
    /// `every_gutter_mark_fits_its_reserved_column`, on the other side of the
    /// same gap.
    ///
    /// Measured with [`Text::width`] and not `width_vetted`, because
    /// [`draw_panel`] draws the label with [`Text::draw`]: the label is
    /// `&'static str` core copy, not a transcript, and the measurement has to
    /// be the one that matches the door it goes through.
    #[test]
    fn the_widest_script_label_fits_its_column() {
        let all = [Group::Latin, Group::Greek, Group::Cyrillic, Group::Japanese];
        // A group added to the enum fails to compile here until somebody
        // decides whether it belongs in `all` above. The match is what the
        // compiler checks; extending the array is still a human step, and
        // saying so is cheaper than implying the array is exhaustive by
        // construction.
        for group in all {
            match group {
                Group::Latin | Group::Greek | Group::Cyrillic | Group::Japanese => {}
            }
        }
        let widest = all
            .iter()
            .filter(|group| **group != Group::Latin)
            .map(|group| group_name(*group))
            .collect::<Vec<_>>()
            .join(SCRIPT_JOIN);
        let mut text = Text::new();
        let w = text.width(&widest, LABEL_PX);
        let column = gutter_x() - script_x();
        assert!(
            w <= column,
            "the widest script label {widest:?} is {w} px in a {column} px column, so it \
             would be drawn into the gutter the collision repair reserves"
        );
    }

    /// **A picker card offers no petition decision, and cannot.**
    ///
    /// [`Card::buttons`] is [`Card::controls`] narrowed by
    /// [`ButtonBox::as_choice`], so this is a property of the split rather
    /// than of what `picker_rows` happens to push — and it is what keeps
    /// `ConsentGrab`'s snapshot unable to produce a `Decision` for a
    /// designation.
    #[test]
    fn a_picker_card_paints_confirm_and_cancel_and_no_choice() {
        let card = rasterize_picker(&picker_fixture());
        assert_eq!(
            card.controls.iter().map(|b| b.action).collect::<Vec<_>>(),
            vec![CardAction::Confirm, CardAction::Cancel]
        );
        assert!(
            card.buttons.is_empty(),
            "a designation grants nothing, so its card must offer no Allow and no Deny"
        );
        // The mirror: a grant-request card offers only decisions.
        let prompt = rasterize(&crate::consent::tests::prompt_fixture());
        assert_eq!(prompt.buttons.len(), prompt.controls.len());
        assert!(prompt
            .controls
            .iter()
            .all(|b| matches!(b.action, CardAction::Decide(_))));
    }

    /// The picker's two buttons get identical geometry, and are painted where
    /// they are reported.
    #[test]
    fn the_pickers_buttons_are_equal_and_painted_where_reported() {
        let card = rasterize_picker(&picker_fixture());
        let rects: Vec<Rect> = card.controls.iter().map(|b| b.rect).collect();
        assert_eq!(rects.len(), 2);
        assert_eq!(
            rects[0].w, rects[1].w,
            "Confirm and Cancel must be equally wide"
        );
        assert_eq!(rects[0].h, rects[1].h);
        assert_eq!(rects[0].y, rects[1].y);
        assert!(rects[0].x + rects[0].w as i32 <= rects[1].x);
        for b in &card.controls {
            let (x, y) = (b.rect.x as u32, b.rect.y as u32);
            assert_eq!(px(&card, x, y), BUTTON_BORDER, "button border corner");
            assert_eq!(px(&card, x + 2, y + 2), BUTTON_BG, "button interior");
        }
        let mut text = Text::new();
        for b in &card.controls {
            let w = text.width(b.action.label(), BUTTON_PX);
            assert!(
                w <= b.rect.w,
                "caption {:?} is {w} px in a {} px button -- it would be clipped",
                b.action.label(),
                b.rect.w
            );
        }
    }

    /// Tab moves a focus ring between the two buttons, and the ring is the
    /// only thing that moves: the card's size, the button geometry and the
    /// captions are identical in all three focus states.
    #[test]
    fn focus_moves_a_ring_and_nothing_else() {
        let mut listing = picker_fixture();
        listing.focus = Focus::Listing;
        let mut on_confirm = picker_fixture();
        on_confirm.focus = Focus::Confirm;
        let mut on_cancel = picker_fixture();
        on_cancel.focus = Focus::Cancel;

        let cards = [
            rasterize_picker(&listing),
            rasterize_picker(&on_confirm),
            rasterize_picker(&on_cancel),
        ];
        for pair in cards.windows(2) {
            assert_eq!(
                pair[0].height, pair[1].height,
                "focus must not resize the card"
            );
            assert_eq!(
                pair[0].controls, pair[1].controls,
                "focus must not move a button under a human's hand"
            );
            assert_ne!(pair[0].rgba, pair[1].rgba, "focus must be visible");
        }
        // The ring is on the focused button and nowhere else.
        let ring = |card: &Card, i: usize| {
            let r = card.controls[i].rect;
            px(card, r.x as u32, r.y as u32 + r.h / 2) == ACCENT
        };
        assert!(!ring(&cards[0], 0) && !ring(&cards[0], 1));
        assert!(ring(&cards[1], 0) && !ring(&cards[1], 1));
        assert!(!ring(&cards[2], 0) && ring(&cards[2], 1));
    }

    /// Every row the panel was given is actually drawn.
    ///
    /// Cheap, and it is the assertion that would have caught a windowing bug
    /// silently showing the human seven of eight files.
    #[test]
    fn every_row_the_panel_carries_is_painted() {
        let names = picker_row_names();
        let refs: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
        let card = card_of(&refs);
        let slots = card.panel.as_ref().expect("a panel");
        assert_eq!(slots.len(), PANEL_ROWS as usize);
        for (index, slot) in slots.iter().enumerate() {
            let inked = (0..PANEL_ROW_H)
                .flat_map(|row| {
                    (CONTENT_X..(CONTENT_X + NAME_FIELD_PX))
                        .map(move |col| (col, slot.rect.y as u32 + row))
                })
                .filter(|&(x, y)| px(&card, x, y) != CARD_BG)
                .count();
            assert!(inked > 0, "row {index} was given a name and drew nothing");
        }
        // A shorter panel leaves the remaining slots empty rather than
        // reusing the previous content, and the card is the same height.
        let short = card_of(&refs[..2]);
        assert_eq!(
            short.height, card.height,
            "the panel's height is a constant"
        );
        let slot = short.panel.as_ref().expect("a panel")[5];
        for row in 0..PANEL_ROW_H {
            for col in CONTENT_X..(CONTENT_X + NAME_FIELD_PX) {
                assert_eq!(px(&short, col, slot.rect.y as u32 + row), CARD_BG);
            }
        }
    }

    /// The query the human typed is echoed, and an empty one says so rather
    /// than leaving a blank the human could read as "no matches".
    #[test]
    fn the_filter_query_is_echoed() {
        let mut typed = picker_fixture();
        typed.panel.query = encode("réport".as_bytes());
        let mut empty = picker_fixture();
        empty.panel.query = encode(b"");

        let with = rasterize_picker(&typed);
        let without = rasterize_picker(&empty);
        assert_ne!(with.rgba, without.rgba, "the echo must be visible");
        assert_eq!(
            with.height, without.height,
            "typing must not resize the card: the buttons stay where the human's hand is"
        );

        let mut text = Text::new();
        let lines = line_texts_of(picker_rows(&empty, &mut text));
        assert!(
            lines.iter().any(|l| l == FILTER_EMPTY),
            "an empty query must say what the field is for: {lines:?}"
        );
    }

    /// The card names who is asking, in which realm, and where the human is
    /// standing — and the location goes through the transcript, because a
    /// directory's name is whatever whoever made it chose.
    #[test]
    fn the_card_names_the_asker_the_realm_and_the_location() {
        let mut text = Text::new();
        let content = picker_fixture();
        let lines = line_texts_of(picker_rows(&content, &mut text));
        assert!(lines.iter().any(|l| l == LABEL_PRINCIPAL));
        assert!(lines
            .iter()
            .any(|l| l.contains(crate::consent::tests::PROMPT_IDENTITY)));
        assert!(lines.iter().any(|l| l == LABEL_REALM));
        assert!(lines.iter().any(|l| l == LABEL_LOCATION));

        // At the root the location field says so instead of naming a path the
        // operator chose and the human did not.
        let mut at_root = picker_fixture();
        at_root.location = encode(b"");
        let root_lines = line_texts_of(picker_rows(&at_root, &mut text));
        assert!(root_lines.iter().any(|l| l == LOCATION_ROOT));

        // ...and a location really reaches the pixels, not just the row list.
        assert_ne!(
            rasterize_picker(&content).rgba,
            rasterize_picker(&at_root).rgba
        );
    }

    /// The consequence line says the three things a human cannot infer, and
    /// says the right one for each ask.
    #[test]
    fn the_card_says_what_confirming_actually_costs() {
        for (ask, expected) in [
            (AskedFor::File { write: false }, CONSEQUENCE_READ),
            (AskedFor::File { write: true }, CONSEQUENCE_WRITE),
            (AskedFor::Dir, CONSEQUENCE_DIR),
        ] {
            let mut content = picker_fixture();
            content.ask = ask;
            let mut text = Text::new();
            let lines = line_texts_of(picker_rows(&content, &mut text)).join(" ");
            // The wrapped line list is compared word by word, because `wrap`
            // may break the sentence anywhere.
            for word in expected.split_whitespace() {
                assert!(
                    lines.contains(word),
                    "the {ask:?} card must say {word:?}: {lines}"
                );
            }
        }
        for line in [CONSEQUENCE_READ, CONSEQUENCE_WRITE, CONSEQUENCE_DIR] {
            assert!(
                line.contains("until it exits"),
                "the card must say the handle outlives the ask: {line}"
            );
            assert!(
                line.to_lowercase().contains("revok"),
                "the card must say revocation does not take it back: {line}"
            );
        }
        assert!(
            !CONSEQUENCE_DIR.contains("read-only"),
            "a directory descriptor is an anchor, not a permission: what the app opens \
             through it carries its own mode, so the card must not claim read-only"
        );
    }

    /// **Escape is named, and named as not working.**
    ///
    /// A human in front of a modal card reaches for Escape; on this one it is
    /// the dead-man chord and does nothing. A card that stayed silent about
    /// that would leave the human to find out by pressing the key they reach
    /// for to stop something.
    #[test]
    fn the_card_says_escape_does_not_close_it() {
        let lower = PICKER_HINT.to_lowercase();
        assert!(lower.contains("escape"));
        assert!(lower.contains("tab"));
        let mut text = Text::new();
        let lines = line_texts_of(picker_rows(&picker_fixture(), &mut text)).join(" ");
        assert!(
            lines.to_lowercase().contains("escape"),
            "the hint must survive wrapping onto the card: {lines}"
        );
    }

    /// The picker card, like the consent card, draws only what it was given.
    #[test]
    fn the_picker_only_draws_content_it_was_given() {
        let base = rasterize_picker(&picker_fixture());
        assert_eq!(base.rgba, rasterize_picker(&picker_fixture()).rgba);

        let mut other_principal = picker_fixture();
        other_principal.principal =
            crate::identity::PrincipalIdentity::parse("vitrin://local/agent/other").unwrap();
        assert_ne!(base.rgba, rasterize_picker(&other_principal).rgba);

        let mut other_realm = picker_fixture();
        other_realm.realm = crate::grants::RealmId::new("kiosk");
        assert_ne!(base.rgba, rasterize_picker(&other_realm).rgba);

        let mut other_rows = picker_fixture();
        other_rows.panel.rows.truncate(3);
        assert_ne!(base.rgba, rasterize_picker(&other_rows).rgba);

        let mut other_highlight = picker_fixture();
        other_highlight.panel.highlight = Some(3);
        assert_ne!(base.rgba, rasterize_picker(&other_highlight).rgba);

        let mut other_ask = picker_fixture();
        other_ask.ask = AskedFor::Dir;
        assert_ne!(base.rgba, rasterize_picker(&other_ask).rgba);
    }

    /// The picker card is opaque everywhere, which is what lets
    /// `Canvas::blit_opaque` composite it as a row copy.
    #[test]
    fn the_picker_card_is_opaque_everywhere() {
        let card = rasterize_picker(&picker_fixture());
        assert_eq!(
            card.rgba.len(),
            card.width as usize * card.height as usize * BYTES_PER_PIXEL
        );
        assert!(card
            .rgba
            .chunks_exact(BYTES_PER_PIXEL)
            .all(|p| p[3] == 0xff));
    }

    /// Every line one picker card renders, in order.
    fn line_texts_of(rows: Vec<Row>) -> Vec<String> {
        rows.into_iter()
            .filter_map(|row| match row {
                Row::Line { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }
}
