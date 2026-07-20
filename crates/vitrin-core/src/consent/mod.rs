//! The consent surface's renderer (P1.7.1, issue #37): the core-internal
//! prompt that asks a human to decide one pending petition, drawn by the TCB
//! itself and composited above the realm view.
//!
//! The core owns the screen. There is no privileged system layer to delegate
//! a prompt to and no trusted client to render one — a "consent app" would be
//! a component that can put arbitrary pixels over every realm, which is the
//! authority the prompt exists to adjudicate. So `vitrind` draws the prompt
//! itself: [`render::rasterize`] produces an opaque RGBA card from a
//! [`PromptContent`], and [`ConsentSurface`] composites it over the composed
//! view at the backend output stage.
//!
//! # Where the overlay composites, and why that is structural
//!
//! `docs/protocol/05-vitrin_consent.md` is normative: "The consent overlay is
//! composited into human-visible output only. It never appears in captured
//! frames delivered by `vitrin_view.frame_ready`, so agents cannot watch
//! prompts even with an active observe grant."
//!
//! That is enforced by **where the code runs**, not by a check anyone could
//! forget:
//!
//! ```text
//!   Scene::compose(w, h) ─┬─► [capture] render_frame ──► agent (frame_ready)
//!                         │        overlay-free, structurally
//!                         └─► [backend output] + ConsentSurface ──► human
//! ```
//!
//! [`crate::scene::Scene::compose`] is the realm view and knows nothing about
//! this module — grep it and there is no consent import to remove. The
//! capture path ([`crate::capture::render_frame`]) is fed those realm-view
//! pixels. The overlay is applied *after* that fork, inside
//! [`crate::backend::human_visible_from_view`] — the single overlay-application
//! step, which both backends really do call (the nested one through
//! [`crate::backend::compose_human_visible`], which is that step with a compose
//! in front of it) and neither capture path does. An agent therefore cannot
//! observe a prompt because there is no code path from the prompt to a capture,
//! rather than because a flag was checked correctly.
//!
//! The last hop is closed by build configuration rather than by naming
//! discipline: the headless backend's human-visible readback is `#[cfg(test)]`,
//! so the capture service being wired up in M1.1 cannot reach for it by
//! mistake — it does not exist in a non-test build. See
//! [`crate::backend::headless`].
//!
//! The headless backend makes the fork concrete by retaining **two** images:
//! `view_framebuffer` (the realm view, what `capture_frame` reads back) and
//! `output_framebuffer` (the virtual display's human-visible output, what the
//! P1.7.1 golden reads back). See [`crate::backend::headless`].
//!
//! # What the prompt may say — and why an agent cannot put a glyph on screen
//!
//! The same protocol page: everything the prompt renders is core-validated
//! data — "the verifier-canonical identity from `vitrin_principal.bound`, and
//! the parsed verbs, realm, and expiry the core derived from the petition —
//! never free client text."
//!
//! [`PromptContent`] holds that rule as a type: **it has no string field at
//! all.** Its members are [`PrincipalIdentity`] (parse-validated at bind time
//! to a strict ASCII subset, and drawn from the operator's
//! `principals.toml`), [`RealmId`] (from the operator's `realm.toml`, never
//! the client's `get_realm` name — [`crate::petitions`] stores the registry's
//! id, not the requested string), [`Verb`], [`PersistenceRung`], and a `u32`
//! of milliseconds. Every character the card draws is either one of those
//! typed values rendered by this module, or a `const` string in
//! [`render`]. There is no free-text field to smuggle a glyph through,
//! because there is no free-text field.
//!
//! Where those values come from is **convention**, not structure, and the
//! difference is worth stating precisely in a claim of this kind. The only
//! production constructor is
//! [`crate::petitions::PetitionRegistry::prompt_content`], which reads the
//! pending table and returns `None` once a petition is resolved — but the
//! fields are `pub` within the crate, and Rust cannot express "only that one
//! function may build this". A future caller could assemble a
//! [`PromptContent`] from somewhere else and it would compile. What survives
//! that mistake is the guarantee that matters: every field would still be a
//! core-validated typed value, so an agent still could not put a glyph on
//! screen.
//!
//! [`text`] then adds a last-moment defense: anything outside ASCII
//! printables is substituted before glyph selection, so no bidi override or
//! combining-mark stack could make one string render as another even if a
//! future change widened what may be shown.
//!
//! # Redraw: one more generation counter, not a second mechanism
//!
//! The prompt appearing or disappearing must force a recomposite.
//! [`crate::scene::Scene`] already solves this with [`Scene::generation`] —
//! a counter bumped on every content change, which the nested backend's
//! texture cache keys on. [`ConsentSurface::generation`] is the same pattern
//! for the same reason, and the nested backend's cache key simply becomes the
//! pair. Two counters rather than one merged "output generation" because the
//! two changes have different consumers: the scene's also invalidates
//! capture, the surface's never does.
//!
//! The rasterized card is cached on the same counter, so a prompt that stays
//! up across a thousand frames is rasterized once — an important property
//! given the nested backend redraws at the host's frame cadence.
//!
//! # The two halves, and what is still missing
//!
//! This module is the renderer (P1.7.1). Its sibling [`grab`] is the trust
//! half (P1.7.2): the input grab that makes a shown prompt exclusive, and
//! the routing of a click into a petition resolution. The seam between them
//! is small and deliberate — [`Card::buttons`] reports every choice's
//! rectangle in card-local pixels, [`centered`] places the card in the view,
//! and [`grab::ConsentGrab::raise`] is the single call that shows the
//! prompt, marks it shown in the petition registry (making the enforcement
//! chokepoint's `consent_held` refusal true), and seizes physical input, so
//! those three cannot drift apart.
//!
//! Still unattached:
//!
//! - **P1.7.3 (issue #39) — hold-Esc revocation.** Nothing here. The grab
//!   deliberately gives Escape no meaning so that chord arrives
//!   unencumbered; see [`grab`]'s keyboard section.
//! - **Issue #85 — no trusted indicator.** A confined app can draw a
//!   byte-identical replica of this card, and nothing yet lets a human tell
//!   the difference. Tracked separately; [`render`] refuses to *claim*
//!   otherwise, and [`grab`] notes the one partial mitigation (a replica
//!   gets no input grab).
//!
//! Nothing at runtime shows a prompt yet, because the petition registry that
//! would name a petition is only constructed by the M1.1 listener wiring
//! (issue #77). Stated plainly rather than implied: **a running `vitrind`
//! renders no consent prompt, and therefore never grabs input.** Both
//! backends carry a live, empty [`ConsentSurface`] and composite through it
//! every frame; the nested backend additionally carries a live, idle
//! [`grab::ConsentGrab`] in its input router, so the grab is armed the
//! instant something raises a prompt. The acceptance criteria are held by
//! the tests in these modules, in [`crate::input`] (through the real router
//! and the real wire to a mock shim), and in [`crate::backend::headless`].
//!
//! [`Scene::generation`]: crate::scene::Scene::generation
//! [`PrincipalIdentity`]: crate::identity::PrincipalIdentity
//! [`RealmId`]: crate::grants::RealmId
//! [`PersistenceRung`]: crate::grants::PersistenceRung
//! [`Card::buttons`]: render::Card::buttons

pub(crate) mod canvas;
pub(crate) mod grab;
pub(crate) mod render;
mod text;

use vitrin_protocol::generated::vitrin_grant::Verb;

use crate::grants::{PersistenceRung, RealmId};
use crate::identity::PrincipalIdentity;
use canvas::Canvas;
use render::Card;

/// How far the realm view is darkened behind the prompt (0 = untouched,
/// 255 = black). Strong enough that the card is unmistakably foreground and
/// app content behind it cannot be mistaken for part of the dialog; light
/// enough that the human can still see *which* realm they are being asked
/// about, which is part of the decision.
const SCRIM_ALPHA: u8 = 150;

/// Everything the consent prompt is allowed to say about one pending
/// petition.
///
/// **No string field, deliberately** — see this module's docs. Every member
/// is a typed core value the core derived or validated itself, so "an agent
/// cannot put a glyph on screen" is a property of this type rather than a
/// rule someone must keep applying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptContent {
    /// The petitioner's verifier-canonical identity (`vitrin_principal.bound`).
    pub principal: PrincipalIdentity,
    /// The realm registry's id for the petitioned realm — never the name the
    /// client minted its realm handle with.
    pub realm: RealmId,
    /// The requested verb set, parsed off the wire and validated.
    pub verbs: Verb,
    /// The requested persistence rung. Not rendered as text: it determines
    /// which allow-choices the prompt may offer (see [`Self::choices`]).
    pub persistence: PersistenceRung,
    /// The requested lifetime in milliseconds; `0` = bounded by the rung.
    pub expiry_ms: u32,
}

impl PromptContent {
    /// The choices this prompt offers, in render order.
    ///
    /// **Derived from the petition, never hardcoded**, which is what makes
    /// the honest-UI rule (PRD P2) structural on two fronts at once:
    ///
    /// - *Durable rungs cannot appear.* The IDL has four persistence values;
    ///   [`PersistenceRung`] represents the two version-1 rungs and
    ///   `TryFrom<WirePersistence>` refuses `until_revoked` and `always`
    ///   ([`crate::grants`]). Generating the allow-choices by iterating
    ///   [`PersistenceRung::ALL`] means a durable rung is *absent* — not
    ///   greyed out, not hidden-but-implied — because it is not
    ///   representable, and a rung added later cannot be forgotten here.
    /// - *A rung the core would refuse cannot appear either.* Approval may
    ///   only narrow a petition, so offering "Allow while running" on a
    ///   petition that asked for `once` would be offering a decision
    ///   [`crate::petitions::PetitionRegistry::resolve_scripted`] rejects as
    ///   a widening. The filter is [`PersistenceRung::narrows`] — the same
    ///   predicate that refusal uses, so the button row and the registry
    ///   cannot disagree about what is grantable.
    ///
    /// Deny is always offered and always last: a consent surface with no
    /// refusal is not a consent surface.
    pub fn choices(&self) -> Vec<Choice> {
        let mut choices: Vec<Choice> = PersistenceRung::ALL
            .iter()
            .copied()
            .filter(|rung| rung.narrows(self.persistence))
            .map(Choice::Allow)
            .collect();
        choices.push(Choice::Deny);
        choices
    }
}

/// One decision the human may take on the prompt.
///
/// The `Allow` variant carries the rung it grants rather than naming it, so
/// the set of allow-choices is exactly the set of representable rungs (see
/// [`PromptContent::choices`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    Allow(PersistenceRung),
    Deny,
}

impl Choice {
    /// The button caption.
    ///
    /// Exhaustively matched over the `Choice`/[`PersistenceRung`] product on
    /// purpose: a rung added to the ladder fails to *compile* here, so a new
    /// rung can never reach the screen without someone deciding what to call
    /// it in front of a human.
    ///
    /// # A caption must say what its rung actually confers
    ///
    /// [`PersistenceRung::Once`] reads "(single use)" because it is *not a
    /// time bound*: it is consumed by its first allowed use and refuses
    /// `expired` thereafter ([`crate::grants::GrantTable::check_use`]). The
    /// card's `Expires` field describes the **petition's request** — it is the
    /// same for every button, because that is what was asked for — so a
    /// `while_running` petition for one hour renders "1 h after approval"
    /// beside both allow-choices. A human reading "1 h" and picking a caption
    /// that said only "Allow once" would reasonably parse it as "allow this,
    /// for the next hour, without asking again", and would in fact be
    /// authorizing exactly one use: a mismatch of three orders of magnitude in
    /// the one UI whose entire purpose is keeping the authority a human
    /// believes they granted and the authority that exists in sync.
    ///
    /// So the rung names its own bound, and `Expires` keeps describing the
    /// request. The alternative — making the expiry text rung-aware — would
    /// mean the field changes meaning depending on which button the human is
    /// hovering, which is worse: the fields describe the petition, the buttons
    /// describe the decision, and mixing the two is how a prompt starts
    /// lying about one to explain the other.
    ///
    /// The direction of the old error was fail-safe (the human over-estimated
    /// what they gave), which is why this is a correctness fix rather than a
    /// vulnerability — but a consent prompt that is only accidentally
    /// conservative is not a consent prompt that is honest.
    pub fn label(self) -> &'static str {
        match self {
            Choice::Allow(PersistenceRung::Once) => "Allow once (single use)",
            Choice::Allow(PersistenceRung::WhileRunning) => "Allow while running",
            Choice::Deny => "Deny",
        }
    }
}

/// The core's consent surface at the backend output stage: which prompt (if
/// any) is up, its cached raster, and the generation counter presentation
/// caches key on.
///
/// Both backends own one. It is always present and always composited
/// through; with no prompt up, [`Self::composite_over`] is a single branch
/// and the human-visible output is the realm view unchanged.
pub(crate) struct ConsentSurface {
    prompt: Option<PromptContent>,
    /// Bumped on every change to `prompt` — the [`crate::scene::Scene`]
    /// pattern (module docs), which is what forces a recomposite when a
    /// prompt appears or disappears.
    generation: u64,
    /// The rasterized card and the generation it was rasterized for. The
    /// card's size and content depend only on the prompt, never on the view,
    /// so the view size is not part of the cache key.
    card: Option<Card>,
    card_generation: u64,
}

impl ConsentSurface {
    /// An empty surface: no prompt, nothing composited.
    pub fn new() -> Self {
        Self {
            prompt: None,
            generation: 0,
            card: None,
            card_generation: 0,
        }
    }

    /// Put a prompt up.
    ///
    /// The only non-test caller is [`grab::ConsentGrab::raise`], which is
    /// where the P1.7.2 seam was taken: it calls this, then
    /// [`crate::petitions::PetitionRegistry::mark_prompt_shown`], then arms
    /// the input grab, in one statement sequence — so "on screen", "input
    /// grabbed", and "`consent_held` holds" are one moment rather than three
    /// that can drift. Prefer `raise` over calling this directly; a prompt
    /// shown without the grab is a prompt an agent can act around.
    ///
    /// Replacing a prompt is legal and is how a queue advances; the caller
    /// owns which petition is at the front.
    pub fn show(&mut self, prompt: PromptContent) {
        self.prompt = Some(prompt);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Take the prompt down (a decision, a timeout, or the petitioner
    /// disconnecting — every path that removes the petition from the pending
    /// table). Drops the cached raster: a prompt that is down must not keep
    /// a rendered copy of a decided petition's contents alive.
    ///
    /// The only non-test caller is [`grab::ConsentGrab::lower`], which pairs
    /// this with releasing the input grab for the same reason `show` is
    /// paired with taking it.
    pub fn dismiss(&mut self) {
        if self.prompt.take().is_some() {
            self.card = None;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// The prompt currently up, if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn prompt(&self) -> Option<&PromptContent> {
        self.prompt.as_ref()
    }

    /// The content generation: changes exactly when composited output may
    /// have changed. Presentation caches key on it (module docs).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The rasterized card, rendering it if the cache is cold or stale.
    fn card(&mut self) -> Option<&Card> {
        self.prompt.as_ref()?;
        if self.card.is_none() || self.card_generation != self.generation {
            let prompt = self.prompt.as_ref().expect("checked immediately above");
            self.card = Some(render::rasterize(prompt));
            self.card_generation = self.generation;
        }
        self.card.as_ref()
    }

    /// Where the card's top-left corner sits in a `view_width x view_height`
    /// view: exactly centered.
    ///
    /// Card-local button rectangles ([`render::Card::buttons`]) plus this
    /// origin is the whole coordinate mapping for a hit test.
    ///
    /// P1.7.2 ended up **not** calling this, and the reason is worth
    /// recording rather than leaving as a puzzle: hit-testing happens inside
    /// the input router's preemption hook, which does not hold the backend's
    /// [`ConsentSurface`] (and could not take the `&mut` this needs for its
    /// lazy raster while a frame is being composed). [`grab::ConsentGrab`]
    /// therefore snapshots the card's *size* when it raises a prompt and
    /// derives the origin itself — through [`centered`], the same function
    /// this and [`Self::composite_over`] use, so all three agree by
    /// construction rather than by coincidence. This accessor stays as the
    /// readable statement of that placement, and as the thing the tests
    /// assert the composite against.
    ///
    /// # A view smaller than the card
    ///
    /// The origin goes negative and every drawing primitive clips, so the
    /// card's middle is what shows. That is a legal state — `--size 320x200`
    /// is accepted by the CLI — and it means the button row can land
    /// off-screen, i.e. a prompt the human can see but not answer.
    ///
    /// Accepted deliberately, on three grounds. It **fails closed**: an
    /// unanswerable prompt hits the consent timeout and resolves `timed_out`,
    /// so the outcome is refusal, never a silent grant. It is **not reachable
    /// in practice**: the card is 560x381 against a 1280x800 default. And the
    /// fix would be **responsive layout** — a card that reflows to its output
    /// — which is presentation policy PRD Doc 2 §2 keeps out of the core, and
    /// would mean the golden could only ever pin one of infinitely many
    /// layouts. Cropping is the honest degradation; shrinking type until a
    /// security decision is unreadable would not be.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn card_origin(&mut self, view_width: u32, view_height: u32) -> Option<(i32, i32)> {
        let card = self.card()?;
        Some(centered(card.width, card.height, view_width, view_height))
    }

    /// Composite the prompt over an already-composed realm view, in place.
    ///
    /// **Human-visible output only.** The one call site is
    /// [`crate::backend::compose_human_visible`]; nothing on the capture path
    /// calls it, and that is the whole of the "agents cannot watch prompts"
    /// guarantee (module docs). A no-op when no prompt is up.
    ///
    /// `view` must be `width * height * 4` bytes of RGBA8888 — the layout
    /// [`crate::scene::Scene::compose`] returns. A mismatch is refused rather
    /// than panicking: failing to draw a prompt is bad, but taking down the
    /// compositor mid-frame is worse, and the petition times out fail-closed
    /// either way.
    pub fn composite_over(&mut self, view: &mut [u8], width: u32, height: u32) {
        let Some(card) = self.card() else {
            return;
        };
        let (x, y) = centered(card.width, card.height, width, height);
        let (rgba, cw, ch) = (&card.rgba, card.width, card.height);
        let Some(mut canvas) = Canvas::new(view, width, height) else {
            return;
        };
        // Scrim first, then the card on top of it: the card is opaque, so
        // darkening the whole view and then covering part of it costs one
        // wasted pass over the card's footprint and keeps the order obvious.
        canvas.darken(SCRIM_ALPHA);
        canvas.blit_opaque(rgba, cw, ch, x, y);
    }
}

/// Center a `w x h` box in a `view_w x view_h` view. Truncating division, so
/// an odd leftover pixel lands on the right/bottom — deterministically, which
/// is what the golden needs.
fn centered(w: u32, h: u32, view_w: u32, view_h: u32) -> (i32, i32) {
    (
        (view_w as i64 - w as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32 / 2,
        (view_h as i64 - h as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32 / 2,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::recorder::ObservationDigest;
    use crate::scene::BYTES_PER_PIXEL;

    /// The golden petition's identity and realm, named so the sourcing tests
    /// can assert the prompt draws *these* and not a constant.
    pub(crate) const PROMPT_IDENTITY: &str = "vitrin://local/agent/demo";
    pub(crate) const PROMPT_REALM: &str = "realm-0";

    /// The fixture every prompt test starts from: the shape of a typical
    /// M1.4-demo petition — all three verbs, `while_running`, one minute.
    /// Chosen for the golden because it exercises the widest card (three
    /// verb lines) and the full three-choice button row.
    pub(crate) fn prompt_fixture() -> PromptContent {
        PromptContent {
            principal: PrincipalIdentity::parse(PROMPT_IDENTITY).expect("fixture identity parses"),
            realm: RealmId::new(PROMPT_REALM),
            verbs: Verb::OBSERVE | Verb::ACTUATE_POINTER | Verb::ACTUATE_TEXT,
            persistence: PersistenceRung::WhileRunning,
            expiry_ms: 60_000,
        }
    }

    /// A `width x height` opaque mid-grey RGBA view — a stand-in for a
    /// composed realm view whose every pixel is known, so any change the
    /// overlay makes is unambiguous.
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
    /// This is the *readable* half of the golden. A digest alone pins the
    /// bytes but says nothing about what broke; a committed raw-pixel golden
    /// would say everything but costs ~850 KB of binary blob per size and is
    /// unreadable in a diff (the SDK capture golden is raw bytes because
    /// Python has to consume the actual bytes — nothing outside Rust consumes
    /// these). The ink map makes a layout regression legible as a picture in
    /// `git diff`, and the digest beside it makes the whole thing exact.
    fn ink_map(card: &Card) -> String {
        const BLOCK: u32 = 8;
        // Thresholds are contrast-stretched around the card's own palette
        // (background ~22, button fill ~38, accent border ~140) rather than
        // spread over 0..255, where every one of those would collapse to the
        // same character.
        const STEPS: [(u32, char); 5] =
            [(28, ' '), (50, '.'), (85, ':'), (130, '*'), (u32::MAX, '#')];
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

    /// The P1.7.1 visual golden: the rendered prompt, pinned exactly.
    ///
    /// Determinism is the acceptance criterion, and it rests on three things
    /// this repository controls end to end: the font is vendored and embedded
    /// (`assets/fonts/README.md`), fontdue's architecture-dependent SIMD
    /// coverage accumulator is disabled (this crate's `Cargo.toml`), and
    /// every other pixel is integer math ([`super::canvas`]). So the same
    /// bytes are expected on any machine, in debug and release alike.
    ///
    /// Regeneration — only when the prompt's design deliberately changes:
    ///
    /// ```sh
    /// VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core consent_prompt_golden
    /// ```
    ///
    /// then read the rewritten ink map in the diff and confirm it is the
    /// change you meant to make before committing it.
    #[test]
    fn consent_prompt_golden() {
        let card = render::rasterize(&prompt_fixture());
        let rendered = format!(
            "# vitrind consent prompt -- P1.7.1 visual golden\n\
             # Regenerate: VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core consent_prompt_golden\n\
             # One character per 8x8 block of the rasterized card, by mean luminance.\n\
             size {}x{}\n\
             blake3 {}\n\
             buttons {}\n\
             {}",
            card.width,
            card.height,
            ObservationDigest::of(&card.rgba).to_hex(),
            card.buttons
                .iter()
                .map(|b| format!(
                    "{}@{},{},{}x{}",
                    b.choice.label().replace(' ', "-"),
                    b.rect.x,
                    b.rect.y,
                    b.rect.w,
                    b.rect.h
                ))
                .collect::<Vec<_>>()
                .join(" "),
            ink_map(&card)
        );

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/golden/consent_prompt.txt"
        );
        if std::env::var_os("VITRIN_REGEN_GOLDEN").is_some() {
            std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"))
                .expect("golden directory");
            std::fs::write(path, &rendered).expect("golden regeneration must be writable");
        }
        let committed = std::fs::read_to_string(path)
            .expect("committed consent golden exists (regenerate: VITRIN_REGEN_GOLDEN=1)");
        assert_eq!(
            committed, rendered,
            "the rendered consent prompt no longer matches \
             crates/vitrin-core/tests/golden/consent_prompt.txt; if the prompt's design changed \
             deliberately, regenerate with VITRIN_REGEN_GOLDEN=1 and check the ink-map diff"
        );
    }

    #[test]
    fn rasterization_is_deterministic_across_instances() {
        // The property the golden depends on, asserted independently of the
        // committed file so a determinism regression is diagnosable on its
        // own: two fresh renders of the same content are byte-identical.
        let a = render::rasterize(&prompt_fixture());
        let b = render::rasterize(&prompt_fixture());
        assert_eq!(a.rgba, b.rgba);
        assert_eq!(a.buttons, b.buttons);
    }

    #[test]
    fn an_empty_surface_leaves_the_view_untouched() {
        let mut surface = ConsentSurface::new();
        let mut view = flat_view(64, 48, [0x40, 0x50, 0x60]);
        let before = view.clone();
        surface.composite_over(&mut view, 64, 48);
        assert_eq!(view, before, "no prompt up means no pixel changes");
        assert!(surface.prompt().is_none());
        assert!(surface.card_origin(64, 48).is_none());
    }

    #[test]
    fn a_shown_prompt_darkens_the_view_and_lands_the_card_centered() {
        const W: u32 = 900;
        const H: u32 = 700;
        let bg = [0x40, 0x50, 0x60];
        let mut surface = ConsentSurface::new();
        surface.show(prompt_fixture());
        let mut view = flat_view(W, H, bg);
        surface.composite_over(&mut view, W, H);

        // Scrim: the corners are the realm view, darkened -- not replaced,
        // so the human can still see which realm is being asked about.
        let corner = px(&view, W, 0, 0);
        assert!(corner[0] < bg[0] && corner[1] < bg[1] && corner[2] < bg[2]);
        assert!(corner[0] > 0, "the scrim must not black the view out");
        assert_eq!(corner[3], 0xff);

        // The card is where card_origin says it is, and its border is there.
        let (x, y) = surface.card_origin(W, H).expect("a prompt is up");
        let card = render::rasterize(&prompt_fixture());
        assert_eq!(
            (x, y),
            ((W - card.width) as i32 / 2, (H - card.height) as i32 / 2)
        );
        for (cx, cy) in [
            (0, 0),
            (card.width - 1, 0),
            (0, card.height - 1),
            (card.width - 1, card.height - 1),
        ] {
            assert_eq!(
                px(&view, W, (x as u32) + cx, (y as u32) + cy),
                px(&card.rgba, card.width, cx, cy),
                "card corner ({cx}, {cy}) must appear verbatim in the output"
            );
        }
        // Blitted, not blended: the card's interior is byte-identical.
        for row in 0..card.height {
            let d = ((y as u32 + row) as usize * W as usize + x as usize) * BYTES_PER_PIXEL;
            let s = row as usize * card.width as usize * BYTES_PER_PIXEL;
            let run = card.width as usize * BYTES_PER_PIXEL;
            assert_eq!(&view[d..d + run], &card.rgba[s..s + run], "card row {row}");
        }
    }

    #[test]
    fn dismissing_restores_the_view_and_bumps_the_generation() {
        // The redraw contract: every visible transition changes the
        // generation, so a presentation cache keyed on it cannot show a
        // stale frame with (or without) a prompt on it.
        let mut surface = ConsentSurface::new();
        let g0 = surface.generation();
        surface.show(prompt_fixture());
        let g1 = surface.generation();
        assert_ne!(g0, g1, "showing a prompt must bump the generation");

        let mut with_prompt = flat_view(700, 600, [0x40, 0x50, 0x60]);
        surface.composite_over(&mut with_prompt, 700, 600);

        surface.dismiss();
        let g2 = surface.generation();
        assert_ne!(g1, g2, "dismissing must bump the generation");
        // Idempotent: a second dismiss is not a visible change.
        surface.dismiss();
        assert_eq!(surface.generation(), g2);

        let clean = flat_view(700, 600, [0x40, 0x50, 0x60]);
        let mut after = clean.clone();
        surface.composite_over(&mut after, 700, 600);
        assert_eq!(after, clean, "a dismissed prompt leaves no pixels behind");
        assert_ne!(after, with_prompt, "...and really had been visible");
    }

    #[test]
    fn showing_a_different_petition_re_rasterizes() {
        // The cache is keyed on the generation, so a queue advancing to the
        // next petition cannot present the previous petition's card.
        let mut surface = ConsentSurface::new();
        surface.show(prompt_fixture());
        let mut first = flat_view(700, 600, [0, 0, 0]);
        surface.composite_over(&mut first, 700, 600);

        let mut next = prompt_fixture();
        next.principal = PrincipalIdentity::parse("vitrin://local/agent/other").unwrap();
        surface.show(next);
        let mut second = flat_view(700, 600, [0, 0, 0]);
        surface.composite_over(&mut second, 700, 600);
        assert_ne!(first, second, "the new petition must be what is drawn");

        // The cache still works: an unchanged prompt composites identically.
        let mut again = flat_view(700, 600, [0, 0, 0]);
        surface.composite_over(&mut again, 700, 600);
        assert_eq!(second, again);
    }

    #[test]
    fn a_view_smaller_than_the_card_is_cropped_not_refused() {
        // `--size 320x200` is a legal virtual output. The prompt must still
        // appear -- cropped -- because the alternative is a decision the
        // human is never asked to make.
        const W: u32 = 320;
        const H: u32 = 200;
        let mut surface = ConsentSurface::new();
        surface.show(prompt_fixture());
        let clean = flat_view(W, H, [0x40, 0x50, 0x60]);
        let mut view = clean.clone();
        surface.composite_over(&mut view, W, H);
        assert_ne!(view, clean, "the prompt must still be drawn");
        let (x, y) = surface.card_origin(W, H).expect("a prompt is up");
        assert!(x < 0 && y < 0, "the card overhangs a view this small");
        // The card's *middle* survives the crop, so the button row may be
        // off-screen: an unanswerable prompt that resolves `timed_out`, which
        // is refusal. See `card_origin`'s docs for why that degradation is
        // accepted rather than papered over with a reflowing layout.
        assert!(view.chunks_exact(BYTES_PER_PIXEL).all(|p| p[3] == 0xff));
    }

    #[test]
    fn a_mis_sized_or_degenerate_view_is_refused_not_panicked_on() {
        let mut surface = ConsentSurface::new();
        surface.show(prompt_fixture());
        // Buffer disagrees with the claimed dimensions.
        let mut wrong = vec![0u8; 10];
        surface.composite_over(&mut wrong, 64, 48);
        assert_eq!(wrong, vec![0u8; 10], "a mis-sized view must be left alone");
        // Zero-area views have no pixels to draw on.
        let mut empty: Vec<u8> = Vec::new();
        surface.composite_over(&mut empty, 0, 48);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_prompt_built_from_a_real_petition_renders_that_petition() {
        // The acceptance criterion "the prompt shows requesting principal,
        // realm, verbs, and expiry from the pending grant", closed end to
        // end: a petition is admitted into the real registry, the prompt is
        // taken from that registry, and the pixels it produces are exactly
        // the pixels for that petition's content -- and not for a
        // neighbouring petition that differs in one field.
        use crate::petitions::{
            Admission, ConsentPolicy, PetitionConfig, PetitionRegistry, PetitionRequest,
        };
        use vitrin_protocol::generated::vitrin_grant::Persistence as WirePersistence;

        let now = std::time::Instant::now();
        let realms = crate::realm::tests::registry_with(&["kiosk"]);
        let mut reg = PetitionRegistry::new(ConsentPolicy::Interactive, PetitionConfig::default());
        let conn = reg.register_connection();
        let petition_for = |who: &str, verbs: Verb| PetitionRequest {
            connection: conn,
            identity: PrincipalIdentity::parse(who).expect("identity parses"),
            realm_name: "kiosk".into(),
            grant_wire_id: 10,
            consent_wire_id: 11,
            resource: String::new(),
            verbs,
            expiry_ms: 60_000,
            max_event_rate: 0,
            persistence: WirePersistence::WhileRunning,
            flags: 0,
        };

        let Admission::Pending { petition } = reg.admit(
            petition_for(PROMPT_IDENTITY, Verb::OBSERVE | Verb::ACTUATE_TEXT),
            now,
            &realms,
        ) else {
            panic!("interactive petition must pend");
        };
        let content = reg.prompt_content(petition).expect("pending");

        // The registry's realm id, not the requested name; the petition's
        // verbs; the petition's expiry.
        assert_eq!(content.realm, RealmId::new("kiosk"));
        assert_eq!(content.verbs, Verb::OBSERVE | Verb::ACTUATE_TEXT);
        assert_eq!(content.expiry_ms, 60_000);

        let mut surface = ConsentSurface::new();
        surface.show(content.clone());
        let mut view = flat_view(800, 700, [0x20, 0x20, 0x20]);
        surface.composite_over(&mut view, 800, 700);

        // Identical to rendering that same content directly...
        let mut reference = ConsentSurface::new();
        reference.show(content);
        let mut expected = flat_view(800, 700, [0x20, 0x20, 0x20]);
        reference.composite_over(&mut expected, 800, 700);
        assert_eq!(view, expected);

        // ...and different from a petition that differs only in its verbs,
        // so the pixels really do carry this petition's content.
        let Admission::Pending { petition: other } =
            reg.admit(petition_for(PROMPT_IDENTITY, Verb::OBSERVE), now, &realms)
        else {
            panic!("interactive petition must pend");
        };
        let mut other_surface = ConsentSurface::new();
        other_surface.show(reg.prompt_content(other).expect("pending"));
        let mut other_view = flat_view(800, 700, [0x20, 0x20, 0x20]);
        other_surface.composite_over(&mut other_view, 800, 700);
        assert_ne!(view, other_view);
    }

    #[test]
    fn every_representable_rung_has_a_button_and_a_caption() {
        // The honest-UI ladder, closed over the type: every rung the core can
        // represent is offerable, every caption is distinct, and Deny is
        // always present and always last.
        for requested in PersistenceRung::ALL {
            let mut prompt = prompt_fixture();
            prompt.persistence = requested;
            let choices = prompt.choices();
            assert_eq!(*choices.last().unwrap(), Choice::Deny);
            let allowed: Vec<PersistenceRung> = choices
                .iter()
                .filter_map(|c| match c {
                    Choice::Allow(rung) => Some(*rung),
                    Choice::Deny => None,
                })
                .collect();
            assert!(
                !allowed.is_empty(),
                "the requested rung is always offerable"
            );
            for rung in &allowed {
                assert!(
                    rung.narrows(requested),
                    "{rung:?} would widen a petition for {requested:?}"
                );
            }
            let mut labels: Vec<&str> = choices.iter().map(|c| c.label()).collect();
            let before = labels.len();
            labels.sort_unstable();
            labels.dedup();
            assert_eq!(labels.len(), before, "two choices share a caption");
        }
    }

    #[test]
    fn the_single_use_rung_says_so_on_its_own_button() {
        // `Once` is consumed by its first use, not bounded by the clock, and
        // the `Expires` field describes the petition's request rather than the
        // rung -- so a `while_running` petition for an hour renders "1 h after
        // approval" beside both allow-choices. The caption is the only place
        // that can tell the human the difference, and it must
        // (`Choice::label`'s docs carry the full argument).
        let caption = Choice::Allow(PersistenceRung::Once).label().to_lowercase();
        assert!(
            caption.contains("single use") || caption.contains("one use"),
            "the Once caption must say it grants a single use, got {:?}",
            Choice::Allow(PersistenceRung::Once).label()
        );
        assert!(
            !Choice::Allow(PersistenceRung::WhileRunning)
                .label()
                .to_lowercase()
                .contains("single use"),
            "only the single-use rung may claim to be single use"
        );

        // And the mismatch that motivated it is really reachable: an hour-long
        // petition offers a single-use choice under a "1 h" expiry.
        let mut prompt = prompt_fixture();
        prompt.persistence = PersistenceRung::WhileRunning;
        prompt.expiry_ms = 3_600_000;
        assert!(prompt
            .choices()
            .contains(&Choice::Allow(PersistenceRung::Once)));
    }
}
