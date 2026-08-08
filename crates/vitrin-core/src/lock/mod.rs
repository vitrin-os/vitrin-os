// SPDX-License-Identifier: MPL-2.0
//! The lock screen (WS-E.2.2, issue #214): the core-drawn surface that takes
//! the session's physical input away from every realm until a human proves they
//! are still there.
//!
//! # Why this is core code, and why that is not re-litigable
//!
//! A lock screen that is an ordinary client is not a lock screen. It cannot
//! take an exclusive input grab — input routing lives in
//! [`crate::input::InputRouter`] and no client can interpose on it; it
//! composites wherever the scene puts it rather than above everything; and if
//! it crashes the session is simply unlocked.
//!
//! [`crate::consent`] settled the same question three issues ago, in the same
//! words: "there is no privileged system layer to delegate a prompt to and no
//! trusted client to render one". PRD §5.1's invariant exiles window-management
//! *policy* — decorations, layout, arrangement — from the TCB. It does not
//! exile the **trusted path**: the consent surface, the trust band and the
//! dead-man indicator are all core-drawn under exactly that invariant. The
//! discriminator is whether the pixels make an authority claim a human must be
//! able to believe, and a lock screen makes the sharpest one in the system —
//! *nothing behind me can see your input*. Only the component that owns input
//! routing can make that claim true.
//!
//! D-021(4) closes the alternative: the WS-E shell is deliberately a client, and
//! D-021 already names the cost — "a shell crash loses window management". A
//! lock screen in the shell would inherit that cost as "a shell crash leaves the
//! session unlocked", which is not a cost anybody would accept if it were
//! written down first.
//!
//! # What the lock actually does, and the one thing it does not
//!
//! It consumes **all physical input** ([`gate`]) and covers the human-visible
//! output completely ([`LockSurface`]). It does **not** suspend an agent's
//! grants: a principal holding `observe` keeps capturing the realm view across a
//! lock, and a principal holding `actuate_*` keeps acting.
//!
//! That is not an oversight and it is not a stopgap — it is the owner's
//! decision, taken on **2026-08-08** and recorded as D-025 in
//! [`docs/plan/20-decision-log.md`](../../../docs/plan/20-decision-log.md). The
//! IDL says observation is concurrent by design (`vitrin_view`), so `preempted`
//! and `consent_held` never refuse a capture; the lock composites on the
//! human-visible path ([`crate::backend::human_visible_from_view`]), which by
//! construction never reaches [`crate::capture`]. Three alternatives were put to
//! the owner and rejected **with** him: a new refusal code (a v0 wire-semantic
//! change, owned by the protocol track), blanking the realm view for agents (an
//! agent silently receives black frames — a lie by omission), and routing the
//! lock through the chokepoint as a synthetic human principal (which invents a
//! wire principal the identity layer does not have).
//!
//! So the gap is **published loudly** rather than papered over: on the lock card
//! itself ([`render`]'s honesty block), in `docs/book/src/limits.md`, and here.
//! The human's instrument for "stop everything" is unchanged and still works
//! while locked: the dead-man chord, which revokes every grant, seals the table
//! and clears the clipboard slot.
//!
//! # What a lock screen is worth on *this* stack, stated before anyone assumes
//!
//! In nested mode `vitrind` is a client of the host compositor. The host owns
//! the real session and sits above this window; anyone can alt-tab away. Stages
//! 1–2 therefore ship a lock screen that **locks a window**, not a session, and
//! saying otherwise would be exactly the honesty failure this repository's docs
//! exist to prevent. On bare DRM (Stage 3) VT switching is an escape unless the
//! core inhibits it, and inhibiting it means a session a human cannot leave when
//! the compositor wedges — a trade Stage 3 must decide and this issue must not
//! pretend it has.
//!
//! # Content is typed, never free text
//!
//! [`LockContent`] follows [`crate::consent::PromptContent`]'s rule: **no string
//! field.** Its members are a [`LockCause`], a list of
//! [`RealmId`](crate::grants::RealmId)s out of the operator's `realm.toml`, an
//! [`UnlockMethod`], and a chord name from a closed vocabulary. So no
//! app-authored glyph can reach a surface the human is asked to trust, and
//! [`crate::paint::text`] substitutes anything outside ASCII printables at the
//! last moment regardless.
//!
//! # The passphrase, and the keymap question it must not answer by accident
//!
//! [`crate::input::invariant_keysym`] is a fixed scancode table with **no
//! letters and no digits** — asserted, not assumed. Letters reach the core today
//! only in nested mode, because winit resolved `logical_key` and the *host*
//! compositor did the interpreting (issue #118). On bare DRM there is no host,
//! and the deliverable alphabet reduces to F-keys, arrows, editing keys and
//! modifiers — over which a passphrase is absurd.
//!
//! This issue therefore lands the surface, the grab, the renderer and the idle
//! raise (all backend-independent) and makes `--lock-passphrase-file` **refuse
//! at startup** on a backend that cannot deliver the alphabet, naming the
//! reason and exiting non-zero ([`PASSPHRASE_NEEDS_A_KEYMAP`]). A session that
//! comes up with an unenterable passphrase is the fail-open configuration trap
//! [`crate::realm`] and [`crate::deadman`] both refuse. Growing an xkbcommon
//! keymap in the core is **not** done here and is not deferred quietly:
//! [`crate::input`]'s docs record that a real keymap forces key pairing to move
//! from the keysym to the scancode — a router invariant the dead-man switch
//! depends on — and that is Stage 3's decision to take explicitly.
//!
//! # Where it composites, and the two paths that had to be closed
//!
//! [`LockSurface::composite_over`] runs inside
//! [`crate::backend::human_visible_from_view`], **after** the consent overlay
//! and **before** the trusted band:
//!
//! - after the consent card, because an opaque lock must cover a prompt nobody
//!   is there to answer (and the prompt's grab is short-circuited anyway, so it
//!   is inert as well as hidden);
//! - before the band, because the band's whole value is having exactly one
//!   correct appearance and nothing — core-drawn or not — may sit on it;
//! - and below the dead-man hold indicator, which stays composited last, so a
//!   lock can never hide a hold in progress.
//!
//! Being on that side of the fork is what makes "an agent cannot watch the lock
//! screen" structural rather than checked. The **second** human-visible path had
//! to be closed by hand: the nested backend's zero-copy dmabuf branch presents a
//! client texture with no CPU composite at all, so a raised lock joins a consent
//! card and a hold indicator in forcing the CPU path
//! ([`crate::backend::winit`]'s `presentable_texture`). Without that, a locked
//! screen would have shown the app's own pixels, full-window, on the one path a
//! human actually looks at.

pub(crate) mod gate;
pub(crate) mod passphrase;
pub(crate) mod render;

use crate::consent::{TrustedIndicator, TRUST_FRAME_THICKNESS};
use crate::grants::RealmId;
use crate::paint::canvas::{Canvas, Rect};
use crate::paint::centered;

pub(crate) use gate::{lock_gate, LockGate, LockJournal, LockScreen};

use render::Card;

/// The startup refusal for `--lock-passphrase-file` on a backend whose intake
/// cannot deliver letters or digits (module docs).
///
/// A `const` in one place, on [`crate::main`]'s
/// `HEADLESS_INTERACTIVE_REFUSAL` precedent, so the message the operator reads
/// and the message the test asserts are one string.
pub(crate) const PASSPHRASE_NEEDS_A_KEYMAP: &str =
    "`--lock-passphrase-file` needs a backend that can deliver letters and digits, and \
     `--headless` cannot: the core holds no keymap, so a headless session's input intake \
     produces only the layout-invariant scancode table (function keys, arrows, editing keys, \
     modifiers) -- no letter and no digit, so the passphrase could never be typed and the \
     session would come up locked-out-able with no way in. Run `--nested`, where the host \
     compositor interprets the layout, or omit the flag to run an unauthenticated privacy \
     screen.";

/// The default manual lock chord, `ctrl+alt+delete`.
///
/// # Why this one, out of a vocabulary with no letters in it
///
/// [`crate::chord::Trigger`]'s vocabulary is the layout-invariant keys and
/// nothing else — no letters, no digits — so the desktop convention
/// (`super+l`) is **inexpressible** on a backend with no host compositor, which
/// is the same wall D-024 hit and answered the same way. `delete` is in the
/// table, and `ctrl+alt+delete` is the one secure-attention chord a human
/// arriving from any other system already reaches for.
///
/// Disjoint from the dead-man chord (`esc`, no modifiers), the attention chord
/// (the two Supers) and the clipboard pair (`ctrl+shift+insert` /
/// `shift+insert`) — and *checked* to be, at startup, rather than asserted here.
pub(crate) const DEFAULT_LOCK_CHORD: &str = "ctrl+alt+delete";

/// The session's lock policy, as the command line asked for it.
///
/// Validated at parse time, exactly as [`crate::deadman::DeadManConfig`] and
/// the two chord flags are, so a session with an unusable lock is a startup
/// error and never a gesture that quietly cannot fire.
///
/// The passphrase is a **path**, not a loaded file: this type crosses
/// `main`'s `Action`, which is `PartialEq` and printed in parse tests, and a
/// loaded digest has no business in either. The file is read once, at the top
/// of the nested backend's `run_inner`, before the listener accepts anyone —
/// so a malformed or badly-permissioned file is still a startup failure with a
/// non-zero exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockConfig {
    pub chord: crate::chord::ModChord,
    /// `None` = no idle raise. Off by default: an idle lock is a policy about
    /// the human's habits, and a core that guessed one would be a core that
    /// locked somebody out of a demo.
    pub idle: Option<std::time::Duration>,
    pub passphrase: Option<std::path::PathBuf>,
}

/// How the session got locked. Two causes, both human-attributable: a gesture
/// the human made, or a gesture they conspicuously did not make.
///
/// Deliberately **not** carrying a "locked by an agent" variant. No wire message
/// locks this session and none is designed; a principal that could lock a human
/// out of their own display would be authority nobody granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockCause {
    /// The human pressed the lock chord.
    Chord,
    /// No physical input for `--lock-idle` seconds.
    Idle,
}

impl LockCause {
    /// The fixed label the flight recorder writes. A closed vocabulary, never
    /// free-form `Display` text — [`crate::recorder`]'s rule for every
    /// enumerated cause.
    pub fn label(self) -> &'static str {
        match self {
            LockCause::Chord => "chord",
            LockCause::Idle => "idle",
        }
    }
}

/// How this session's lock is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnlockMethod {
    /// A passphrase checked against `--lock-passphrase-file`.
    Passphrase,
    /// No authentication at all: Enter dismisses it. The card says so in as
    /// many words, because a privacy screen that looked like a lock would be
    /// the worst of both.
    AnyKey,
}

/// Everything the lock card is allowed to say.
///
/// **No string field, deliberately** — [`crate::consent::PromptContent`]'s rule,
/// for the same reason and with the same consequence: "an app cannot put a glyph
/// on the trusted surface" is a property of this type rather than a rule someone
/// has to keep applying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockContent {
    pub cause: LockCause,
    /// The realms this session is holding, from the realm registry — the
    /// operator's `realm.toml` ids, never a name a client minted.
    ///
    /// Named on the card because a human returning to a locked screen is
    /// deciding whether to unlock *this* session, and a lock screen that could
    /// belong to any session is a lock screen a forgery need not resemble.
    pub realms: Vec<RealmId>,
    pub unlock: UnlockMethod,
    /// The dead-man chord's name, from [`crate::deadman::Chord`]'s closed
    /// vocabulary. On the card because the honesty block tells the human what
    /// *does* stop an agent, and an instruction that named the wrong key would
    /// be worse than none.
    pub deadman_chord: &'static str,
}

/// The core's lock surface at the backend output stage: whether a lock is up,
/// its cached raster, and the generation counter presentation caches key on.
///
/// The [`crate::consent::ConsentSurface`] pattern, deliberately down to the
/// field names — two surfaces on one output stage that behaved differently
/// under a resize or a texture cache would be two bugs waiting.
///
/// Both backends own one. It is always present and always composited through;
/// with no lock up, [`Self::composite_over`] is a single branch and the
/// human-visible output is unchanged.
pub(crate) struct LockSurface {
    content: Option<LockContent>,
    /// Bumped on every change to `content` — the [`crate::scene::Scene`]
    /// pattern, which is what forces a recomposite when the lock appears or
    /// disappears.
    generation: u64,
    card: Option<Card>,
    card_generation: u64,
    /// This session's trusted indicator (issue #85): the secret colour a
    /// genuine core-drawn card is framed in.
    ///
    /// **Required at construction, exactly as [`crate::consent::ConsentSurface`]
    /// requires one**, and for a threat that is if anything sharper here: a
    /// confined app can draw a pixel-perfect replica of this card and harvest
    /// the passphrase a human types into it. The replica gets no input grab
    /// (so the keystrokes reach the app rather than vanishing, which is a
    /// difference a human cannot see) and — the part they *can* see — no frame
    /// in a colour it never had access to, under a band it cannot forge.
    indicator: TrustedIndicator,
}

impl LockSurface {
    /// An empty surface: no lock, nothing composited.
    pub fn new(indicator: TrustedIndicator) -> Self {
        Self {
            content: None,
            generation: 0,
            card: None,
            card_generation: 0,
            indicator,
        }
    }

    /// Put the lock up.
    ///
    /// Called only by [`crate::session::service_lock_round`], which pairs it
    /// with the [`LockScreen`] state the gate judges against — the same
    /// discipline [`crate::consent::grab::ConsentGrab::raise`] enforces, so
    /// "the pixels are up" and "input is being consumed" cannot drift apart.
    /// Idempotent for identical content, so an embedder that calls it every
    /// round does not invalidate the raster at frame cadence.
    pub fn raise(&mut self, content: LockContent) {
        if self.content.as_ref() == Some(&content) {
            return;
        }
        self.content = Some(content);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Take the lock down. Drops the cached raster. Idempotent.
    pub fn lower(&mut self) {
        if self.content.take().is_some() {
            self.card = None;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Whether a lock is composited right now.
    ///
    /// Read by the nested backend to force the CPU compositing path: the
    /// zero-copy dmabuf branch presents a *client's* texture, so a locked
    /// session that took it would present the app's own pixels full-window
    /// (module docs).
    pub fn is_raised(&self) -> bool {
        self.content.is_some()
    }

    /// The content generation: changes exactly when composited output may have
    /// changed. Presentation caches key on it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The rasterized card, rendering it if the cache is cold or stale.
    fn card(&mut self) -> Option<&Card> {
        self.content.as_ref()?;
        if self.card.is_none() || self.card_generation != self.generation {
            let content = self.content.as_ref().expect("checked immediately above");
            self.card = Some(render::rasterize(content));
            self.card_generation = self.generation;
        }
        self.card.as_ref()
    }

    /// Where the card's top-left corner sits in a `view_width x view_height`
    /// view: exactly centered, through the one placement function every
    /// core-drawn card uses ([`crate::paint::centered`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn card_origin(&mut self, view_width: u32, view_height: u32) -> Option<(i32, i32)> {
        let card = self.card()?;
        Some(centered(card.width, card.height, view_width, view_height))
    }

    /// Composite the lock over an already-composed human-visible view, in
    /// place.
    ///
    /// **Human-visible output only.** The one call site is
    /// [`crate::backend::human_visible_from_view`]; nothing on the capture path
    /// calls it, which is the whole of "an agent cannot watch the lock screen"
    /// — a property of where the code runs, not of a flag. A no-op when no lock
    /// is up.
    ///
    /// `view` must be `width * height * 4` bytes of RGBA8888. A mismatch is
    /// refused rather than panicking, as everywhere else on this path: failing
    /// to draw is bad, taking the compositor down mid-frame is worse.
    ///
    /// # The cover is opaque, and that is the difference from a consent scrim
    ///
    /// [`crate::consent::ConsentSurface::composite_over`] *darkens* the realm
    /// view, because which realm is being asked about is part of the decision.
    /// This one **replaces** it: whatever was on screen when the human walked
    /// away must not be legible behind the card. A translucent lock screen is a
    /// screenshot with a dialog on it.
    pub fn composite_over(&mut self, view: &mut [u8], width: u32, height: u32) {
        // Read the secret before the `&mut self` borrow the lazy raster needs;
        // `TrustedIndicator` is `Copy`, so this takes nothing from `self`.
        let indicator = self.indicator;
        let Some(card) = self.card() else {
            return;
        };
        let (x, y) = centered(card.width, card.height, width, height);
        let (rgba, cw, ch) = (&card.rgba, card.width, card.height);
        let Some(mut canvas) = Canvas::new(view, width, height) else {
            return;
        };
        // Cover, then the trusted frame, then the opaque card — the same order
        // and the same reasoning as the consent overlay, with the scrim
        // replaced by a full opaque fill.
        canvas.fill_rect(
            Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            },
            render::COVER_RGBA,
        );
        let t = TRUST_FRAME_THICKNESS;
        canvas.stroke_rect(
            Rect {
                x: x - t as i32,
                y: y - t as i32,
                w: cw.saturating_add(2 * t),
                h: ch.saturating_add(2 * t),
            },
            indicator.color(),
            t,
        );
        canvas.blit_opaque(rgba, cw, ch, x, y);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::scene::BYTES_PER_PIXEL;

    /// The golden's fixture: the shape of a typical WS-E session — two realms,
    /// locked by the chord, with a passphrase configured. Two rather than one
    /// so the golden actually pins the comma-joined realm list, which is what
    /// a multi-realm session draws and what `MAX_REALMS = 16` makes ordinary.
    ///
    /// Chosen for the golden because it exercises every field the card can
    /// draw: both causes are one enum away, and the passphrase instruction is
    /// the longer of the two unlock blocks.
    pub(crate) fn lock_fixture() -> LockContent {
        LockContent {
            cause: LockCause::Chord,
            // TWO realms, which is what the doc above always claimed and the
            // fixture did not hold. One realm never exercises the comma-joined
            // list, and a multi-realm session is the normal case since
            // `MAX_REALMS` became 16 -- so the golden was pinning the one
            // arrangement a WS-E session is least likely to be in.
            realms: vec![RealmId::new("realm-0"), RealmId::new("editor")],
            unlock: UnlockMethod::Passphrase,
            deadman_chord: "esc",
        }
    }

    /// A verifier at Argon2's cheapest legal parameters, for tests that are
    /// about the lock's state machine rather than about the KDF's cost.
    ///
    /// The default 19 MiB x 2 passes is right for a session and wrong for a
    /// suite that unlocks a few dozen times; `passphrase.rs`'s own tests cover
    /// the default encoding.
    pub(crate) fn cheap_verifier(secret: &[u8]) -> passphrase::PassphraseFile {
        passphrase::PassphraseFile::cheap_for_test(secret)
    }

    fn flat_view(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
        for px in v.chunks_exact_mut(BYTES_PER_PIXEL) {
            px[..3].copy_from_slice(&rgb);
            px[3] = 0xff;
        }
        v
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let off = (y as usize * w as usize + x as usize) * BYTES_PER_PIXEL;
        buf[off..off + BYTES_PER_PIXEL].try_into().unwrap()
    }

    /// Render the card as a coarse ASCII "ink map": one character per 8x8
    /// block, chosen from a five-step ramp by the block's mean luminance.
    ///
    /// The readable half of the golden, on
    /// [`crate::consent`]'s `consent_prompt_golden` precedent and with the same
    /// justification: a digest alone pins the bytes and says nothing about what
    /// broke, a raw-pixel golden says everything and is unreadable in a diff.
    fn ink_map(card: &Card) -> String {
        const BLOCK: u32 = 8;
        // Contrast-stretched around this card's own palette (cover ~10,
        // background ~24, accent border ~160) rather than spread over 0..255.
        const STEPS: [(u32, char); 5] =
            [(30, ' '), (55, '.'), (95, ':'), (140, '*'), (u32::MAX, '#')];
        let mut out = String::new();
        let mut y = 0;
        while y < card.height {
            let mut x = 0;
            while x < card.width {
                let (mut sum, mut n) = (0u64, 0u64);
                for yy in y..(y + BLOCK).min(card.height) {
                    for xx in x..(x + BLOCK).min(card.width) {
                        let p = px(&card.rgba, card.width, xx, yy);
                        // Integer Rec.601 luma: no floating point anywhere in
                        // the golden's own pipeline either.
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

    /// The WS-E.2.2 visual golden: the rendered lock card, pinned exactly.
    ///
    /// Byte-stable on x86-64 and aarch64 for the reason the consent golden is:
    /// the font is vendored and embedded, fontdue's architecture-dependent SIMD
    /// coverage accumulator is disabled (this crate's `Cargo.toml`), and every
    /// other pixel is integer math ([`crate::paint::canvas`]).
    ///
    /// Regenerate — only when the card's design deliberately changes — with
    /// `cargo xtask bless --filter lock`.
    #[test]
    fn lock_screen_golden() {
        let card = render::rasterize(&lock_fixture());
        let rendered = format!(
            "# vitrind lock screen -- WS-E.2.2 visual golden\n\
             # Regenerate: VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core lock_screen_golden\n\
             # One character per 8x8 block of the rasterized card, by mean luminance.\n\
             size {}x{}\n\
             blake3 {}\n\
             {}",
            card.width,
            card.height,
            crate::recorder::ObservationDigest::of(&card.rgba).to_hex(),
            ink_map(&card)
        );

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/lock_screen.txt");
        if std::env::var_os("VITRIN_REGEN_GOLDEN").is_some() {
            std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"))
                .expect("golden directory");
            std::fs::write(path, &rendered).expect("golden regeneration must be writable");
        }
        let committed = std::fs::read_to_string(path)
            .expect("committed lock golden exists (regenerate: VITRIN_REGEN_GOLDEN=1)");
        assert_eq!(
            committed, rendered,
            "the rendered lock screen no longer matches \
             crates/vitrin-core/tests/golden/lock_screen.txt; if the card's design changed \
             deliberately, regenerate with VITRIN_REGEN_GOLDEN=1 and check the ink-map diff"
        );
    }

    #[test]
    fn an_empty_surface_leaves_the_view_untouched() {
        let mut surface = LockSurface::new(TrustedIndicator::for_test());
        let mut view = flat_view(64, 48, [0x40, 0x50, 0x60]);
        let before = view.clone();
        surface.composite_over(&mut view, 64, 48);
        assert_eq!(view, before, "no lock up means no pixel changes");
        assert!(!surface.is_raised());
        assert!(surface.card_origin(64, 48).is_none());
    }

    #[test]
    fn a_raised_lock_covers_the_view_opaquely_and_lands_the_card_centered() {
        const W: u32 = 900;
        const H: u32 = 700;
        let bg = [0x40, 0x50, 0x60];
        let mut surface = LockSurface::new(TrustedIndicator::for_test());
        surface.raise(lock_fixture());
        let mut view = flat_view(W, H, bg);
        surface.composite_over(&mut view, W, H);

        // The cover REPLACES rather than darkens: the corner is the lock's own
        // colour, with nothing of the view left in it. This is the assertion
        // that distinguishes a lock screen from a consent scrim, and it would
        // pass vacuously against a scrim only if the scrim happened to produce
        // exactly COVER_RGBA -- which it cannot, since a scrim is a function of
        // the pixel underneath and this is not.
        assert_eq!(px(&view, W, 0, 0), render::COVER_RGBA);
        assert_eq!(px(&view, W, W - 1, H - 1), render::COVER_RGBA);

        // The card is where card_origin says it is, blitted verbatim.
        let (x, y) = surface.card_origin(W, H).expect("a lock is up");
        let card = render::rasterize(&lock_fixture());
        assert_eq!(
            (x, y),
            ((W - card.width) as i32 / 2, (H - card.height) as i32 / 2)
        );
        for row in 0..card.height {
            let d = ((y as u32 + row) as usize * W as usize + x as usize) * BYTES_PER_PIXEL;
            let s = row as usize * card.width as usize * BYTES_PER_PIXEL;
            let run = card.width as usize * BYTES_PER_PIXEL;
            assert_eq!(&view[d..d + run], &card.rgba[s..s + run], "card row {row}");
        }
    }

    #[test]
    fn a_genuine_lock_card_is_framed_in_the_session_secret() {
        // Issue #85's replica defence, applied to the surface that asks for a
        // passphrase -- the one a forgery most wants to imitate. The ring sits
        // strictly OUTSIDE the card, so the opaque blit never covers it.
        const W: u32 = 900;
        const H: u32 = 700;
        let indicator = TrustedIndicator::for_test();
        let mut surface = LockSurface::new(indicator);
        surface.raise(lock_fixture());
        let mut view = flat_view(W, H, [0, 0, 0]);
        surface.composite_over(&mut view, W, H);
        let (x, y) = surface.card_origin(W, H).unwrap();
        let probe = (
            (x - TRUST_FRAME_THICKNESS as i32 / 2) as u32,
            (y + 20) as u32,
        );
        assert_eq!(
            px(&view, W, probe.0, probe.1),
            indicator.color(),
            "the ring hugging the card's outer edge carries this session's colour"
        );
    }

    #[test]
    fn raising_and_lowering_bump_the_generation_and_a_repeat_does_not() {
        let mut surface = LockSurface::new(TrustedIndicator::for_test());
        let g0 = surface.generation();
        surface.raise(lock_fixture());
        let g1 = surface.generation();
        assert_ne!(g0, g1);
        // Idempotent for identical content: an embedder that ensures the lock
        // is up every round must not invalidate the raster at frame cadence.
        surface.raise(lock_fixture());
        assert_eq!(surface.generation(), g1);
        // Different content is a visible change.
        let mut other = lock_fixture();
        other.cause = LockCause::Idle;
        surface.raise(other);
        let g2 = surface.generation();
        assert_ne!(g1, g2);

        surface.lower();
        assert_ne!(g2, surface.generation());
        let g3 = surface.generation();
        surface.lower();
        assert_eq!(surface.generation(), g3, "a second lower is not a change");
    }

    #[test]
    fn lowering_leaves_no_pixel_behind() {
        const W: u32 = 700;
        const H: u32 = 600;
        let mut surface = LockSurface::new(TrustedIndicator::for_test());
        let clean = flat_view(W, H, [0x40, 0x50, 0x60]);
        surface.raise(lock_fixture());
        let mut locked = clean.clone();
        surface.composite_over(&mut locked, W, H);
        assert_ne!(locked, clean, "...and really had been visible");
        surface.lower();
        let mut after = clean.clone();
        surface.composite_over(&mut after, W, H);
        assert_eq!(after, clean);
    }

    #[test]
    fn a_mis_sized_or_degenerate_view_is_refused_not_panicked_on() {
        let mut surface = LockSurface::new(TrustedIndicator::for_test());
        surface.raise(lock_fixture());
        let mut wrong = vec![0u8; 10];
        surface.composite_over(&mut wrong, 64, 48);
        assert_eq!(wrong, vec![0u8; 10], "a mis-sized view must be left alone");
        let mut empty: Vec<u8> = Vec::new();
        surface.composite_over(&mut empty, 0, 48);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_view_smaller_than_the_card_still_covers_everything() {
        // The degradation that matters here is the opposite of the consent
        // card's: an unanswerable prompt fails closed to a timeout, but a lock
        // whose card is cropped must STILL cover the screen, or a small view
        // would leave app content visible around a clipped card.
        const W: u32 = 320;
        const H: u32 = 200;
        let mut surface = LockSurface::new(TrustedIndicator::for_test());
        surface.raise(lock_fixture());
        let mut view = flat_view(W, H, [0x40, 0x50, 0x60]);
        surface.composite_over(&mut view, W, H);
        let (x, y) = surface.card_origin(W, H).expect("a lock is up");
        assert!(x < 0 && y < 0, "the card overhangs a view this small");
        assert!(
            view.chunks_exact(BYTES_PER_PIXEL)
                .all(|p| p[3] == 0xff && p[..3] != [0x40, 0x50, 0x60]),
            "not one pixel of the realm view may survive the cover"
        );
    }

    #[test]
    fn the_cause_label_vocabulary_is_closed() {
        assert_eq!(LockCause::Chord.label(), "chord");
        assert_eq!(LockCause::Idle.label(), "idle");
    }
}
