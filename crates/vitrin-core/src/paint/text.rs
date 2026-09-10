// SPDX-License-Identifier: MPL-2.0
//! Text for the core's trusted surfaces: the bundled font, glyph
//! rasterization, and the measure/wrap/draw operations
//! [`crate::consent::render`] and [`crate::lock::render`] lay their cards out
//! with.
//!
//! There are two glyph sources: the embedded vector face below, and the
//! pre-rasterized [`super::atlas`] the picker draws Japanese from. They cover
//! disjoint alphabets, which is what lets one cache key have one source; that
//! module's tests measure the disjointness rather than assuming it.
//!
//! # The font is embedded, never looked up
//!
//! [`FONT_BYTES`] is `include_bytes!` of the copy vendored under
//! `crates/vitrin-core/assets/fonts/` — see that directory's `README.md` for
//! provenance, its SHA-256, and the OFL-1.1 terms. Three consequences worth
//! stating where the code is rather than only where the file is:
//!
//! - **The golden can be exact.** Anti-aliased coverage is a function of the
//!   outline data, so a system-font lookup would make every text pixel depend
//!   on the machine's installed fonts and fontconfig rules.
//! - **The prompt cannot be starved or authored from outside.** There is no
//!   path to poison, no `~/.local/share/fonts` to drop a hostile face into,
//!   and no configuration under which the consent surface has no font to draw
//!   with. For a TCB security surface that is the whole point.
//! - **The parser never sees attacker bytes.** fontdue parses exactly this
//!   compile-time constant and nothing else, so the usual "font parsers are a
//!   CVE farm" objection does not reach us.
//!
//! # Two doors, and only one of them is wide
//!
//! [`Text::draw`] and [`Text::width`] substitute anything outside
//! `0x20..=0x7E` with `?` ([`SUBSTITUTE`], via [`Text::renderable`]). On those
//! two that can never fire from the prompt — every string it draws is
//! core-derived, and [`crate::identity::PrincipalIdentity`] already restricts
//! identities to an ASCII subset at parse time — but it is not vestigial:
//! [`crate::consent::render`] draws a filesystem path out of `realm.toml`,
//! which is arbitrary operator-written bytes. Bidi overrides, zero-width
//! joiners and combining-mark stacks are exactly the tools for making one
//! string render as another, and a consent prompt is the highest-value place
//! in the system to play that game. Substituting at the last possible moment
//! means no such character can reach glyph selection however it got into a
//! string.
//!
//! [`Text::width_vetted`] and [`Text::draw_runs`] are the **second** door, and
//! they do not substitute: they take a [`Vetted`], which is a string every
//! character of which [`super::script::route`] has already decided is
//! drawable. Widening the first two would have widened the `realm.toml` path
//! silently; adding a typed door widens exactly one caller — the picker — and
//! makes the widening visible in its signature. This section used to say
//! "only ASCII printables reach the rasterizer", full stop, and that sentence
//! stopped being true the moment `Vetted` landed.
//!
//! # Determinism
//!
//! fontdue is built with `default-features = false`, which disables its
//! x86-only SIMD coverage accumulator (this crate's `Cargo.toml` explains
//! why in full: the SIMD path sums a prefix in a different association order
//! than the scalar path, and floating-point addition is not associative). One
//! scalar IEEE-754 path on every architecture means the same glyph bitmap on
//! every architecture. Glyphs are rasterized at integer origins — fontdue has
//! no subpixel positioning — so identical text at an identical size is
//! identical bytes, which is what makes the raster cacheable *and* golden-able.

use std::collections::HashMap;
use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

use super::atlas;
use super::canvas::Canvas;
use super::script::{route, Route};

/// The bundled face: Liberation Sans Regular, unmodified, SIL OFL-1.1.
/// SHA-256 `baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee`.
const FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/LiberationSans-Regular.ttf");

/// Byte length of the vendored file, asserted at compile time below.
const FONT_LEN: usize = 410_820;

/// A swapped or re-generated font file must fail the *build*, not quietly
/// move every text pixel in the golden and leave a maintainer diffing
/// rasterized output to find out why. The length is a cheap tripwire; the
/// README's SHA-256 is the authoritative check.
const _: () = assert!(
    FONT_BYTES.len() == FONT_LEN,
    "the vendored trusted-surface font changed; see crates/vitrin-core/assets/fonts/README.md"
);

/// Handed to the pen loop on the measure path, where no pixel is blended
/// and therefore no colour is read. Named rather than inlined so a reader
/// meets the reason instead of an arbitrary black.
const MEASURE_RGB: [u8; 3] = [0, 0, 0];

/// The size a filename is drawn at on the picker.
///
/// A constant rather than a caller's choice because
/// [`crate::paint::script::DENIED_APPEARANCE`] is measured at exactly this
/// size: two glyphs that rasterize identically at 14 px need not do so at 19,
/// so a picker drawing names at another size would be relying on a closure
/// computed for a size it does not use.
/// It is also the **only** size [`Self::width_vetted`] and [`Self::draw_runs`]
/// work at, and the size [`super::atlas`] holds its bitmaps at. That is not a
/// convenience: the atlas is single-size, so a widened path that took a size
/// argument could be handed one the atlas cannot serve, and the caller would
/// find out as a wrong-sized glyph rather than as a refusal.
pub(crate) const NAME_PX: f32 = 14.0;

/// Whether the vendored vector face has a glyph for `ch`.
///
/// [`super::script`] uses this so its routing asks the real source instead of
/// approximating it with codepoint ranges. A range is a claim about what the
/// face carries, and the face does not honour it: the Greek block alone holds
/// seventeen codepoints Liberation has no glyph for, and all seventeen
/// rasterize to the one `.notdef` box — so routing them by range would draw
/// seventeen distinct filenames identically.
///
/// `' '` is not special-cased: the face carries a real space glyph, and a
/// caller wanting "blank but advancing" should say so rather than lean on a
/// missing glyph doing it by accident.
pub(crate) fn face_covers(ch: char) -> bool {
    font().lookup_glyph_index(ch) != 0
}

/// What [`Text::draw`] renders in place of any non-ASCII-printable character
/// (module docs). Visible on purpose: a silently dropped character is a
/// consent prompt quietly saying something other than what it was given.
const SUBSTITUTE: char = '?';

/// `FontSettings::scale`, pinned rather than defaulted.
///
/// fontdue feeds this to its geometry preprocessing, so it is an input to
/// rasterized output; leaving it at the crate's default would let a fontdue
/// release move the golden with no change on our side. The value matches the
/// prompt's largest type size (`crate::consent::render::TITLE_PX` = 19.0), which is
/// what the setting is documented to optimize for.
///
/// Deliberately a separate constant rather than an alias of `TITLE_PX`: this
/// is a global tuning input to *every* glyph, so adjusting the title's size
/// should not silently re-rasterize the whole card. Changing either is a
/// golden-visible change; keeping them independent makes it clear which one
/// was intended.
const GEOMETRY_SCALE: f32 = 19.0;

/// The parsed face, built once per process.
///
/// `expect` is right here and nowhere else in this module: the input is a
/// compile-time constant of this binary, so a parse failure means the shipped
/// artifact is malformed — there is no runtime condition, no configuration,
/// and no peer that can cause it, and no fallback that would be honest (a
/// consent prompt that cannot draw its text must not degrade into a prompt
/// with no text). `font_parses` in the tests below makes it a test failure
/// rather than a first-prompt failure.
fn font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| {
        Font::from_bytes(
            FONT_BYTES,
            FontSettings {
                collection_index: 0,
                scale: GEOMETRY_SCALE,
                load_substitutions: false,
            },
        )
        .expect("the bundled trusted-surface font is a compile-time constant and must parse")
    })
}

/// One rasterized glyph: the four metrics the pen loop reads, the advance, and
/// the 8-bit coverage bitmap.
///
/// Deliberately **not** fontdue's `Metrics`. There are two glyph sources now —
/// the vector face and [`super::atlas`] — and a struct owned here is what lets
/// the second one exist without either pretending to be fontdue or the pen loop
/// learning which source it is drawing from.
struct Glyph {
    /// Left bearing: pixels from the pen to the bitmap's left edge.
    xmin: i32,
    /// Bottom bearing: pixels from the baseline up to the bitmap's bottom edge.
    ymin: i32,
    width: usize,
    height: usize,
    advance: f32,
    coverage: Vec<u8>,
}

/// Vertical metrics of one type size, in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineMetrics {
    /// Baseline offset from the top of a line box.
    pub ascent: u32,
    /// Height of a line box: ascent + descent + line gap.
    pub height: u32,
}

/// The trusted surfaces' text engine: the shared face plus a per-instance
/// glyph cache.
///
/// Deliberately **not** a layout engine. There is no shaping, no kerning
/// beyond the font's advance widths, no bidi, and no font fallback — the
/// prompt draws ASCII from a closed set of core-derived strings, and every
/// one of those features would be a reason to link a text stack an order of
/// magnitude larger than the whole consent surface (see this crate's
/// `Cargo.toml` on why cosmic-text was declined).
pub(crate) struct Text {
    cache: HashMap<(char, u32), Glyph>,
}

impl Text {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Rasterize (or fetch) one glyph. Keyed on the size's *bit pattern* so
    /// the cache key is exact: `f32` is not `Eq`, and rounding the key would
    /// let two distinct sizes collide onto one bitmap.
    ///
    /// # Two sources, one key
    ///
    /// A key is served by the pre-rasterized atlas when the atlas holds that
    /// character at that size, and by the vector face otherwise. The two
    /// alphabets are **disjoint** — `the_vector_face_owns_none_of_the_atlas_alphabet`
    /// in [`super::atlas`] measures it against the shipped face — so no key can
    /// be claimed by both, and which arm runs is a function of the character
    /// and the size alone rather than of what was cached first.
    ///
    /// The substitution in the third arm is the one case the disjointness
    /// alone does not cover: an atlas character asked for at a size the atlas
    /// does not hold. The vector face has no glyph for it either, so fontdue
    /// would hand back `.notdef` — a box, silently, at whatever width the face
    /// gives `.notdef`. Unreachable today (the two widened entry points fix the
    /// size at [`NAME_PX`], and every other caller has already been through
    /// [`Self::renderable`]), and it substitutes rather than trusting that.
    fn glyph(&mut self, ch: char, px: f32) -> &Glyph {
        self.cache.entry((ch, px.to_bits())).or_insert_with(|| {
            if let Some(g) = atlas::glyph(ch, px) {
                return Glyph {
                    xmin: g.xmin,
                    ymin: g.ymin,
                    width: g.width,
                    height: g.height,
                    advance: g.advance,
                    coverage: g.coverage.to_vec(),
                };
            }
            let ch = if atlas::covers(ch) { SUBSTITUTE } else { ch };
            let (metrics, coverage) = font().rasterize(ch, px);
            Glyph {
                xmin: metrics.xmin,
                ymin: metrics.ymin,
                width: metrics.width,
                height: metrics.height,
                advance: metrics.advance_width,
                coverage,
            }
        })
    }

    /// The character actually rasterized for `ch` (module docs: ASCII
    /// printables only).
    fn renderable(ch: char) -> char {
        match ch {
            ' '..='~' => ch,
            _ => SUBSTITUTE,
        }
    }

    /// The one pen loop: every measurement and every draw in this module
    /// runs through it.
    ///
    /// Measured and drawn text agree here **by construction**, not because
    /// two loops are kept in step — [`Self::width`] and [`Self::draw`] visit
    /// the same characters in the same order through the same accumulator,
    /// and differ only in whether a canvas was handed in.
    ///
    /// `pen` is borrowed rather than owned so a caller can carry one
    /// accumulator across several calls. That is what makes a multi-run draw
    /// *identical* to a single draw of the concatenation instead of merely
    /// close to it: restarting the pen per run would replace
    /// `round(P + q)` with `round(P) + round(q)`, which differs on the
    /// majority of strings.
    ///
    /// The colour rides on each character rather than on the call so one
    /// pass can change colour mid-string. It is unread when `canvas` is
    /// `None`, since a measurement blends nothing.
    fn pen_run<I>(
        &mut self,
        mut canvas: Option<&mut Canvas<'_>>,
        chars: I,
        px: f32,
        x: i32,
        baseline: i32,
        pen: &mut f32,
    ) where
        I: Iterator<Item = (char, [u8; 3])>,
    {
        for (ch, rgb) in chars {
            let glyph = self.glyph(ch, px);
            let advance = glyph.advance;
            if let Some(canvas) = canvas.as_deref_mut() {
                let gx = x + pen.round() as i32 + glyph.xmin;
                // fontdue reports `ymin` from the baseline upward, so the top
                // of the bitmap sits `height + ymin` above it.
                let gy = baseline - glyph.ymin - glyph.height as i32;
                let (w, h) = (glyph.width, glyph.height);
                // Copy out of the cache before touching the canvas: the borrow
                // checker aside, this keeps the blend loop free of a hash lookup.
                let coverage = glyph.coverage.clone();
                for row in 0..h {
                    for col in 0..w {
                        let alpha = coverage[row * w + col];
                        canvas.blend_pixel(gx + col as i32, gy + row as i32, rgb, alpha);
                    }
                }
            }
            *pen += advance;
        }
    }

    /// Advance width of `s` at `px`, in whole pixels — what the layout
    /// measures with and what [`Self::draw`] then advances by, so measured
    /// and drawn text can never disagree.
    pub fn width(&mut self, s: &str, px: f32) -> u32 {
        let mut pen = 0.0f32;
        // `MEASURE_RGB` is never read: `canvas` is `None`, so no pixel is
        // blended. It exists so measuring and drawing share one loop.
        self.pen_run(
            None,
            s.chars().map(|ch| (Self::renderable(ch), MEASURE_RGB)),
            px,
            0,
            0,
            &mut pen,
        );
        // `max(0.0)` before the cast: a negative accumulated advance is not
        // reachable with this font, but `as u32` on a negative float
        // saturates to 0 silently and a width is unsigned by nature.
        pen.max(0.0).round() as u32
    }

    /// **Test seam.** What `ch` actually rasterizes to at `px`: the coverage
    /// bitmap, its dimensions, and the advance in 1/64ths.
    ///
    /// `None` when the face has no glyph for `ch`, which is distinct from a
    /// glyph that happens to be blank — the appearance closure must not fold
    /// every uncovered codepoint into one class with the space.
    #[cfg(test)]
    pub(crate) fn raster_signature_for_test(
        &mut self,
        ch: char,
        px: f32,
    ) -> Option<(Vec<u8>, usize, usize, u32)> {
        if font().lookup_glyph_index(ch) == 0 {
            return None;
        }
        let glyph = self.glyph(ch, px);
        Some((
            glyph.coverage.clone(),
            glyph.width,
            glyph.height,
            (glyph.advance * 64.0).round() as u32,
        ))
    }

    /// **Test seam.** Whether the *vector face* has a glyph for `ch`, asked of
    /// the face directly rather than through the cache.
    ///
    /// [`super::atlas`]'s disjointness test needs to ask about a character the
    /// atlas serves, which is exactly the case where going through
    /// [`Self::glyph`] would answer about the atlas instead.
    #[cfg(test)]
    pub(crate) fn vector_face_has_glyph_for_test(ch: char) -> bool {
        font().lookup_glyph_index(ch) != 0
    }

    /// Vertical metrics at `px`, rounded up so successive lines never
    /// overlap by a fractional pixel.
    pub fn line_metrics(&self, px: f32) -> LineMetrics {
        // A face without horizontal line metrics cannot happen for the
        // bundled font; the fallback keeps the type total rather than
        // introducing an error path no caller could act on.
        let m = font().horizontal_line_metrics(px);
        match m {
            Some(m) => LineMetrics {
                ascent: m.ascent.max(0.0).ceil() as u32,
                height: m.new_line_size.max(0.0).ceil() as u32,
            },
            None => LineMetrics {
                ascent: px.ceil() as u32,
                height: (px * 1.2).ceil() as u32,
            },
        }
    }

    /// Draw `s` at `px` with its left edge at `x` and its baseline at
    /// `baseline`, blending `rgb` through each glyph's coverage.
    ///
    /// The pen advances in `f32` and is rounded once per glyph rather than
    /// accumulating rounded integers: rounding per glyph would let a long
    /// string drift by a pixel per character, which is visible as uneven
    /// spacing well before it is visible as a wrong width.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas<'_>,
        s: &str,
        px: f32,
        x: i32,
        baseline: i32,
        rgb: [u8; 3],
    ) {
        let mut pen = 0.0f32;
        self.pen_run(
            Some(canvas),
            s.chars().map(|ch| (Self::renderable(ch), rgb)),
            px,
            x,
            baseline,
            &mut pen,
        );
    }

    /// Advance width of a [`Vetted`] string at [`NAME_PX`], in whole pixels.
    ///
    /// **Not a convenience wrapper on [`Self::width`], and the difference is
    /// the point.** `width` measures through [`Self::renderable`], so it
    /// measures a natively-drawn `é` or `漢` as `?` — roughly half the advance
    /// of a wide glyph. A layout that measured with `width` and drew with
    /// [`Self::draw_runs`] would lay its columns out for text it is not
    /// drawing, and a name would run off the end of its column rather than be
    /// truncated. The invariant `width`'s own docs state — measured and drawn
    /// text can never disagree — holds on the widened path because this is the
    /// same [`Self::pen_run`] with the same characters, not because the two
    /// are kept in step.
    #[allow(dead_code)] // the picker (#190) is the caller; see `Vetted`.
    pub fn width_vetted(&mut self, v: &Vetted<'_>) -> u32 {
        let mut pen = 0.0f32;
        self.pen_run(
            None,
            v.0.chars().map(|ch| (ch, MEASURE_RGB)),
            NAME_PX,
            0,
            0,
            &mut pen,
        );
        pen.max(0.0).round() as u32
    }

    /// Draw a sequence of differently-coloured runs as **one** line of text at
    /// [`NAME_PX`], returning the total advance in whole pixels — the same
    /// number [`Self::width_vetted`] returns for the concatenation.
    ///
    /// One `f32` pen crosses every run boundary, which makes a two-run draw
    /// identical to a single draw of the concatenation — for *any* vetted
    /// string, since both sides are this function. Against [`Self::draw`] the
    /// identity is narrower and the difference is deliberate: the two agree
    /// pixel for pixel on a string `draw` does not substitute, i.e. an ASCII
    /// one, and must **not** agree on any other, because drawing `é` rather
    /// than `?` is the entire reason this second door exists.
    /// `a_single_run_is_pixel_identical_to_a_plain_draw` holds the first half
    /// and `the_widened_path_draws_what_the_narrow_one_substitutes` the
    /// second.
    /// The alternative — drawing run two at `x + width(run_one)` — replaces
    /// `round(P + q)` with `round(P) + round(q)` and shifts the second run by a
    /// pixel on a substantial fraction of strings;
    /// `the_naive_alternative_actually_differs` measures that fraction, because
    /// an identity test whose two sides never disagree anyway proves nothing.
    ///
    /// This is what tinting a script run costs: the picker names a filename's
    /// minority script by colouring it, and colouring it must not move it.
    #[allow(dead_code)] // the picker (#190) is the caller; see `Vetted`.
    pub fn draw_runs(
        &mut self,
        canvas: &mut Canvas<'_>,
        runs: &[(Vetted<'_>, [u8; 3])],
        x: i32,
        baseline: i32,
    ) -> u32 {
        let mut pen = 0.0f32;
        for (run, rgb) in runs {
            let rgb = *rgb;
            self.pen_run(
                Some(canvas),
                run.0.chars().map(move |ch| (ch, rgb)),
                NAME_PX,
                x,
                baseline,
                &mut pen,
            );
        }
        pen.max(0.0).round() as u32
    }

    /// Break `s` into at most `max_lines` lines no wider than `max_width`.
    ///
    /// Prefers a space, then one of the URI-ish separators in
    /// [`BREAK_AFTER`] (kept *on* the preceding line, the way a reader
    /// expects a path to break), and finally breaks mid-token — because the
    /// longest single string the prompt shows is a principal identity, which
    /// has no spaces at all.
    ///
    /// **Overflow truncates with a visible marker**, never silently. On a
    /// consent surface a quietly clipped identity is a spoofing primitive:
    /// `vitrin://local/agent/trusted-thing` and
    /// `vitrin://local/agent/trusted-thing-EVIL` must not render as the same
    /// pixels. The `...` marker says "there is more", and the residual risk
    /// is bounded by identities being operator-registered in
    /// `principals.toml` rather than client-chosen — an agent cannot pick a
    /// name designed to overflow the box.
    pub fn wrap(&mut self, s: &str, px: f32, max_width: u32, max_lines: usize) -> Vec<String> {
        if max_lines == 0 || max_width == 0 {
            return Vec::new();
        }
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for ch in s.chars() {
            let mut candidate = current.clone();
            candidate.push(ch);
            if self.width(&candidate, px) <= max_width || current.is_empty() {
                current = candidate;
                continue;
            }
            // The line is full. Retreat to the last break opportunity, if
            // this line has one; otherwise break right here.
            let split = current
                .char_indices()
                .rfind(|(_, c)| *c == ' ' || BREAK_AFTER.contains(*c))
                .map(|(i, c)| i + c.len_utf8());
            let carry = match split {
                Some(at) if at < current.len() => current.split_off(at),
                _ => String::new(),
            };
            lines.push(current.trim_end().to_string());
            if lines.len() == max_lines {
                return Self::truncate_last(self, lines, px, max_width);
            }
            // Seed the next line with the post-break remainder — and measure
            // it, because `carry` plus `ch` can already be over budget. `ch`
            // is by definition the character that just overflowed, and `carry`
            // can be nearly a full line's worth when the break opportunity sat
            // early in it, so this is the one path into `current` that could
            // otherwise reach `lines` wider than `max_width`. Every other path
            // is gated by the fit check above; this one is now gated the same
            // way, which is what makes "no returned line exceeds `max_width`"
            // an invariant of the function rather than of the inputs it
            // happens to be given.
            current = carry;
            let mut seeded = current.clone();
            seeded.push(ch);
            if self.width(&seeded, px) <= max_width || current.is_empty() {
                current = seeded;
            } else {
                // The remainder is a full line on its own: flush it and start
                // the next line at the overflowing character.
                lines.push(current.trim_end().to_string());
                if lines.len() == max_lines {
                    return Self::truncate_last(self, lines, px, max_width);
                }
                current = String::from(ch);
            }
        }
        if !current.is_empty() || lines.is_empty() {
            lines.push(current);
        }
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            return Self::truncate_last(self, lines, px, max_width);
        }
        lines
    }

    /// Re-fit the final line so `ELLIPSIS` fits, dropping characters from its
    /// end until it does. Only reached when the text did not fit at all.
    fn truncate_last(&mut self, mut lines: Vec<String>, px: f32, max_width: u32) -> Vec<String> {
        let Some(last) = lines.last_mut() else {
            return lines;
        };
        let mut fitted = std::mem::take(last);
        loop {
            let candidate = format!("{fitted}{ELLIPSIS}");
            if self.width(&candidate, px) <= max_width || fitted.is_empty() {
                *lines.last_mut().expect("checked non-empty above") = candidate;
                return lines;
            }
            fitted.pop();
        }
    }
}

/// A string whose every character [`route`] admits, checked once at
/// construction.
///
/// **"Admitted by the router" is not the same as "the renderer has a glyph for
/// it", and today the two sets differ by 17 codepoints** — see
/// `vetted_admits_seventeen_codepoints_the_face_cannot_draw` below, which
/// measures them. [`route`] decides by codepoint *range*; it never asks the
/// face. Closing that gap is [`super::script`]'s to make and not this type's,
/// so what this doc may honestly claim is the router's verdict, nothing
/// stronger.
///
/// **This is the only way a non-ASCII codepoint reaches glyph selection.**
/// [`Text::draw`] and [`Text::width`] still put every character through
/// [`Text::renderable`] (module docs), and that is not vestigial: those two are
/// what [`crate::consent::render`] draws a filesystem path out of `realm.toml`
/// with, and a path is arbitrary bytes an operator wrote. Widening *them*
/// would widen that surface silently. So the widened path is a second, typed
/// door rather than a wider one — a caller who wants real Unicode has to
/// produce a `Vetted`, and producing one is exactly the check.
///
/// Constructing one is the whole safety argument, so it is worth saying what
/// the check is and is not. It is [`route`]: every character must have a
/// decision other than [`Route::Escape`]. It is **not** a judgement about
/// whether the resulting string is *safe to show* — the mixed-script rule
/// ([`script::permitted`]) and the per-listing digest are separate defences
/// that a caller applies before it gets here. `Vetted` says "these glyphs
/// exist and this renderer can place them", nothing more.
/// Nothing constructs one yet: the picker that will is #190's remaining half,
/// and this is the door it will come through. Held to the same standard
/// [`super::script`] states for its own unreached table — proven mechanism,
/// not in service — which is why the tests below exercise the pen identity and
/// the router agreement rather than waiting for a caller.
#[allow(dead_code)]
pub(crate) struct Vetted<'a>(&'a str);

#[allow(dead_code)]
impl<'a> Vetted<'a> {
    /// `None` if any character routes to [`Route::Escape`] — which is the
    /// negative form on purpose: a future third route (an atlas route, say)
    /// must be admitted here without an edit, whereas an allowlist of routes
    /// would silently reject it and transcribe a character the renderer had
    /// just learned to draw.
    pub fn new(s: &'a str) -> Option<Self> {
        s.chars()
            .all(|ch| !matches!(route(ch), Route::Escape))
            .then_some(Self(s))
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

/// Characters a wrapped line may end on — the separators a URI-shaped
/// identity is naturally read in chunks around.
const BREAK_AFTER: &str = "/-_.:";

/// Overflow marker. ASCII on purpose (module docs): `U+2026 HORIZONTAL
/// ELLIPSIS` would be the typographically correct glyph and is the one
/// character that would force the whole ASCII-only rule to carry an
/// exception.
const ELLIPSIS: &str = "...";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::BYTES_PER_PIXEL;

    /// Ink coverage of a string drawn onto a scratch canvas: how many pixels
    /// the text actually touched. Zero means nothing was drawn.
    fn ink(text: &mut Text, s: &str, px: f32) -> usize {
        let (w, h) = (600u32, 60u32);
        let mut buf = vec![0u8; w as usize * h as usize * BYTES_PER_PIXEL];
        for p in buf.chunks_exact_mut(BYTES_PER_PIXEL) {
            p[3] = 0xff;
        }
        let mut canvas = Canvas::new(&mut buf, w, h).expect("scratch canvas");
        text.draw(&mut canvas, s, px, 4, 40, [0xff, 0xff, 0xff]);
        buf.chunks_exact(BYTES_PER_PIXEL)
            .filter(|p| p[0] != 0)
            .count()
    }

    /// Every byte of a scratch canvas after `f` has drawn on it — so two draws
    /// can be compared pixel for pixel rather than by an ink count, which is
    /// blind to a run that moved sideways without changing how much it inked.
    fn pixels(f: impl FnOnce(&mut Text, &mut Canvas<'_>)) -> Vec<u8> {
        let (w, h) = (400u32, 40u32);
        let mut buf = vec![0u8; w as usize * h as usize * BYTES_PER_PIXEL];
        for p in buf.chunks_exact_mut(BYTES_PER_PIXEL) {
            p[3] = 0xff;
        }
        let mut text = Text::new();
        let mut canvas = Canvas::new(&mut buf, w, h).expect("scratch canvas");
        f(&mut text, &mut canvas);
        buf
    }

    fn vetted(s: &str) -> Vetted<'_> {
        Vetted::new(s).unwrap_or_else(|| panic!("{s:?} is drawable"))
    }

    /// A deterministic string source over the alphabet the router admits.
    /// Fixed-seed LCG, no `rand` in the TCB (the same shape
    /// `no_wrapped_line_ever_exceeds_the_box` uses), so a failure is
    /// reproducible from the seed alone.
    fn vetted_alphabet() -> Vec<char> {
        (0x20u32..=0x4FF)
            .filter_map(char::from_u32)
            .filter(|&ch| !matches!(route(ch), Route::Escape))
            .collect()
    }

    fn lcg(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 33
        }
    }

    /// One run is one draw. Not "close to": the same bytes.
    #[test]
    fn a_single_run_is_pixel_identical_to_a_plain_draw() {
        // ASCII, because `draw` substitutes anything else and the two paths
        // would then be drawing different text -- which is the *next* test.
        let s = "vitrin://local/agent/demo-2026.txt";
        let rgb = [0x40, 0xd0, 0x90];
        let a = pixels(|t, c| {
            t.draw(c, s, NAME_PX, 7, 25, rgb);
        });
        let b = pixels(|t, c| {
            t.draw_runs(c, &[(vetted(s), rgb)], 7, 25);
        });
        assert_eq!(a, b, "one run must be exactly one draw");
    }

    /// And the widened path is genuinely wider: where `draw` substitutes, this
    /// draws the character. A test that only compared ASCII would pass with
    /// `draw_runs` delegating to `draw`, which would silently transcribe every
    /// accented name the picker exists to show.
    #[test]
    fn the_widened_path_draws_what_the_narrow_one_substitutes() {
        let s = "café Ωμέγα привет";
        let rgb = [0xff, 0xff, 0xff];
        let widened = pixels(|t, c| {
            t.draw_runs(c, &[(vetted(s), rgb)], 4, 25);
        });
        let narrow = pixels(|t, c| {
            t.draw(c, s, NAME_PX, 4, 25, rgb);
        });
        assert_ne!(
            widened, narrow,
            "the widened path must draw the real glyphs, not the substitute"
        );
        // The narrow path is the substitute, exactly -- `renderable` is intact.
        let substituted = pixels(|t, c| {
            t.draw(c, "caf? ????? ??????", NAME_PX, 4, 25, rgb);
        });
        assert_eq!(narrow, substituted, "Text::draw must still substitute");
    }

    /// **`width` is the wrong ruler for a vetted string, and silently so.**
    ///
    /// This is the trap the widened path exists to close: `width` measures
    /// through `renderable`, so it measures every non-ASCII character as `?`.
    /// A layout that measured with it and drew with `draw_runs` would size its
    /// columns for text it is not drawing.
    ///
    /// **Not "they always disagree" — that is false and the number is here so
    /// nobody assumes otherwise.** Measured over every single non-ASCII
    /// character `Vetted` admits below `U+0500`: 606 of 817 measure
    /// differently and **211 measure the same**, because `?` at 14 px advances
    /// 7.79 px and so does most of Latin-1 — `é` is exactly as wide as the
    /// substitute that replaces it. That is what makes the trap silent rather
    /// than obvious: a picker that measured with `width` would be right about
    /// a quarter of the time, which is far too often to notice and nowhere
    /// near often enough to rely on.
    #[test]
    fn width_is_the_wrong_ruler_for_a_vetted_string() {
        let mut text = Text::new();
        // ASCII: the two rulers are the same ruler.
        let ascii = "report-2026.pdf";
        assert_eq!(
            text.width(ascii, NAME_PX),
            text.width_vetted(&vetted(ascii))
        );
        // Non-ASCII: they must not be.
        let greek = "Ωμέγα";
        assert_ne!(
            text.width(greek, NAME_PX),
            text.width_vetted(&vetted(greek)),
            "`width` measures Ω as `?`; a picker that believed it would overflow"
        );
        // And the disagreement is a property of the widened alphabet, not of
        // one lucky string: measure it, in both directions, so that a change
        // making the two rulers agree everywhere (a `width_vetted` that
        // quietly delegated, say) fails here rather than passing on `Ωμέγα`.
        let (mut same, mut differ) = (0usize, 0usize);
        for cp in 0xA1u32..=0x4FF {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            let s = ch.to_string();
            let Some(v) = Vetted::new(&s) else {
                continue;
            };
            if text.width(&s, NAME_PX) == text.width_vetted(&v) {
                same += 1;
            } else {
                differ += 1;
            }
        }
        assert_eq!(
            (same, differ),
            (211, 589),
            "the two rulers' agreement over the widened alphabet moved; if the face or the \
             router changed on purpose, re-measure and update the doc comment above with it. \
             (It moved once already, from 606 differing to 589: `route` stopped admitting \
             the seventeen Greek codepoints the face has no glyph for, and 606 - 589 = 17.)"
        );
    }

    /// What `width_vetted` returns is what `draw_runs` advances by -- the same
    /// invariant `width` states, on the widened path, and by the same
    /// mechanism (one `pen_run`) rather than by agreement between two loops.
    #[test]
    fn width_vetted_is_what_draw_runs_advances() {
        let mut text = Text::new();
        for s in [
            "",
            "a",
            "café_Ωμ_привет.txt",
            "vitrin://local/agent/demo",
            "ΑΒΓΔΕ αβγδε",
        ] {
            let measured = text.width_vetted(&vetted(s));
            let mut drawn = 0;
            let _ = pixels(|t, c| {
                drawn = t.draw_runs(c, &[(vetted(s), [1, 2, 3])], 4, 25);
            });
            assert_eq!(measured, drawn, "{s:?}");
        }
    }

    /// Two runs are one line: colouring a script run must not move it.
    #[test]
    fn runs_join_as_one_pen() {
        let (a, b) = ("отчёт", "_final.pdf");
        let whole = format!("{a}{b}");
        let rgb = [0xc0, 0x30, 0x30];
        let split = pixels(|t, c| {
            t.draw_runs(c, &[(vetted(a), rgb), (vetted(b), rgb)], 5, 25);
        });
        let joined = pixels(|t, c| {
            t.draw_runs(c, &[(vetted(&whole), rgb)], 5, 25);
        });
        assert_eq!(
            split, joined,
            "a run boundary must be invisible when the colour does not change"
        );
        // Three runs, and a boundary mid-word, are the same statement.
        let three = pixels(|t, c| {
            t.draw_runs(
                c,
                &[
                    (vetted("от"), rgb),
                    (vetted("чёт_fin"), rgb),
                    (vetted("al.pdf"), rgb),
                ],
                5,
                25,
            );
        });
        assert_eq!(three, joined);
    }

    /// **The floor under the test above.**
    ///
    /// `runs_join_as_one_pen` compares the shared pen against itself and would
    /// pass vacuously if per-run and single-pen placement happened to agree on
    /// every string. They do not, and this measures how often they disagree:
    /// restarting the pen per run replaces `round(P + q)` with
    /// `round(P) + round(q)`. Without this floor the identity above is not
    /// evidence.
    #[test]
    fn the_naive_alternative_actually_differs() {
        let alphabet = vetted_alphabet();
        let mut next = lcg(0x9E37_79B9_7F4A_7C15);
        let mut differing = 0usize;
        let trials = 300;
        let mut first_differing: Option<(String, String)> = None;
        let mut text = Text::new();
        for _ in 0..trials {
            let (la, lb) = (1 + (next() % 8) as usize, 2 + (next() % 8) as usize);
            let a: String = (0..la)
                .map(|_| alphabet[(next() as usize) % alphabet.len()])
                .collect();
            let b: String = (0..lb)
                .map(|_| alphabet[(next() as usize) % alphabet.len()])
                .collect();
            let joined = text.width_vetted(&vetted(&format!("{a}{b}")));
            let naive = text.width_vetted(&vetted(&a)) + text.width_vetted(&vetted(&b));
            if joined != naive {
                differing += 1;
                first_differing.get_or_insert((a.clone(), b.clone()));
            }
        }
        // Measured at this seed: 62 of 300 pairs, 20.7%. The floor sits below
        // that rather than at it, because the property that matters is "not
        // vanishing"; pinning the exact rate would make an alphabet change
        // read as a regression in the pen.
        assert!(
            differing * 100 / trials >= 15,
            "only {differing}/{trials} random run pairs place differently under a per-run pen;              at that rate `runs_join_as_one_pen` would be near-vacuous"
        );
        // And the divergence is visible in pixels, not only in a rounded width.
        let (a, b) = first_differing.expect("at least one pair differs");
        let rgb = [0xff, 0xff, 0xff];
        let one_pen = pixels(|t, c| {
            t.draw_runs(c, &[(vetted(&a), rgb), (vetted(&b), rgb)], 4, 25);
        });
        let per_run = pixels(|t, c| {
            let first = t.draw_runs(c, &[(vetted(&a), rgb)], 4, 25);
            t.draw_runs(c, &[(vetted(&b), rgb)], 4 + first as i32, 25);
        });
        assert_ne!(
            one_pen, per_run,
            "{a:?} + {b:?} must place differently under the two schemes"
        );
    }

    /// `Vetted` admits exactly what the router does not escape -- checked as a
    /// property over the whole routed range rather than on hand-picked strings,
    /// because the pair this guards is "what may be drawn" and "what is
    /// drawn", and a hand-picked sample cannot show they are the same set.
    #[test]
    fn vetted_admits_exactly_what_the_router_admits() {
        for cp in 0u32..=0x2FFF {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            let s = ch.to_string();
            assert_eq!(
                Vetted::new(&s).is_some(),
                !matches!(route(ch), Route::Escape),
                "U+{cp:04X}"
            );
        }
        // A string is admitted only if every character is: one escaped
        // character anywhere is enough to reject the whole run, which is what
        // makes `Vetted` a statement about the string rather than about most
        // of it.
        assert!(Vetted::new("resume.pdf").is_some());
        for hostile in [
            "resume\u{202e}fdp.txt", // an RTL override: the spoof itself
            "a\u{200d}b",            // a zero-width joiner
            "e\u{0301}",             // a combining mark with no positioning
            "ملف.txt",               // Arabic: needs joining, and is transcribed
            "\u{00A0}gap",           // a no-break space: a space that is not one
        ] {
            assert!(
                Vetted::new(hostile).is_none(),
                "{hostile:?} must not reach glyph selection"
            );
        }
    }

    /// **`Vetted` admits nothing that no glyph source can draw.**
    ///
    /// This began life measuring a hole rather than closing one. `route`
    /// decided by codepoint *range* and never asked the face, and those are
    /// not the same set: seventeen codepoints in `U+0370..=U+03CF` routed to
    /// `Vector` with **no glyph at all** in the shipped face, so fontdue
    /// answered `.notdef` — the *same* `.notdef` for all seventeen. `Ͱ.txt`
    /// and `ͱ.txt` are two assigned Greek letters, two distinct filenames, and
    /// one bitmap.
    ///
    /// [`super::script::DENIED_APPEARANCE`]'s closure test could not have
    /// caught it, which is worth keeping on the record: that test partitions
    /// by `raster_signature_for_test`, which answers `None` for a codepoint
    /// the face has no glyph for, so all seventeen were skipped before they
    /// could collide. A test can only find collisions among things it agrees
    /// exist.
    ///
    /// `route` now asks each source — [`Text::face_covers`] for the vector
    /// face, `atlas::covers` for Japanese — instead of trusting a range, so
    /// the set is empty by construction rather than by enumeration. This test
    /// stays as the thing that says so, and it is the reason the seventeen are
    /// not a table someone has to maintain.
    #[test]
    fn vetted_admits_nothing_no_glyph_source_can_draw() {
        let mut undrawable = Vec::new();
        for cp in 0u32..=0x2FFF {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            if Vetted::new(&ch.to_string()).is_none() {
                continue;
            }
            if !Text::vector_face_has_glyph_for_test(ch) && !atlas::covers(ch) {
                undrawable.push(cp);
            }
        }
        assert!(
            undrawable.is_empty(),
            "`Vetted` admits {} codepoint(s) no glyph source can draw, so that many \
             distinct filenames would render as the same `.notdef` box: {:04X?}. \
             `paint::script::route` must ask the source rather than trust a range.",
            undrawable.len(),
            undrawable
        );
    }

    #[test]
    fn font_parses_and_has_the_glyphs_the_prompt_needs() {
        // The `expect` in `font()` made a test failure out of a first-prompt
        // failure -- this is that test. Every ASCII printable must rasterize,
        // because the prompt draws principal identities, realm ids, verb
        // names, and its own labels out of exactly that set.
        let font = font();
        for ch in ' '..='~' {
            assert!(
                font.lookup_glyph_index(ch) != 0 || ch == ' ',
                "the bundled font lacks a glyph for {ch:?}"
            );
        }
    }

    #[test]
    fn rasterization_is_deterministic_and_cache_transparent() {
        // The golden's precondition: same string, same size, same bytes --
        // and the cache must not change what is drawn, only how fast.
        let mut a = Text::new();
        let mut b = Text::new();
        let s = "vitrin://local/agent/demo";
        assert_eq!(ink(&mut a, s, 14.0), ink(&mut b, s, 14.0));
        assert_eq!(ink(&mut a, s, 14.0), ink(&mut a, s, 14.0), "cached redraw");
        assert_eq!(a.width(s, 14.0), b.width(s, 14.0));
    }

    #[test]
    fn non_ascii_is_substituted_before_glyph_selection() {
        // The defense-in-depth rule (module docs). A bidi override, a
        // zero-width joiner and a combining mark must all render as the
        // substitute -- identically to a literal '?' -- so none of them can
        // make one string display as another.
        let mut text = Text::new();
        let hostile = "a\u{202e}b\u{200d}c\u{0301}d";
        let visible = "a?b?c?d";
        assert_eq!(text.width(hostile, 14.0), text.width(visible, 14.0));
        assert_eq!(ink(&mut text, hostile, 14.0), ink(&mut text, visible, 14.0));
        // And it is a *visible* substitution, not a silent drop.
        assert_ne!(text.width(hostile, 14.0), text.width("abcd", 14.0));
    }

    #[test]
    fn width_is_monotonic_and_matches_what_is_drawn() {
        let mut text = Text::new();
        assert_eq!(text.width("", 14.0), 0);
        assert!(text.width("ii", 14.0) < text.width("WW", 14.0));
        assert!(text.width("abc", 14.0) < text.width("abcd", 14.0));
        // A space advances the pen without inking anything.
        assert!(text.width(" ", 14.0) > 0);
        assert_eq!(ink(&mut text, " ", 14.0), 0);
    }

    /// No line `wrap` returns may exceed the box, whatever it is given.
    ///
    /// A sweep rather than a fixed input, because the bug this replaces was
    /// invisible to a hand-picked identity: seeding a new line with the
    /// post-break remainder plus the overflowing character skipped the fit
    /// check, so a line could come back a glyph too wide. Real identities are
    /// dense with `/` and `-` and never landed in that shape — the defect was
    /// latent, not live — but the guarantee the card's layout leans on is the
    /// unconditional one, so it is tested unconditionally.
    ///
    /// Deterministic (a fixed-seed LCG, no `rand` dependency in the TCB): the
    /// same 400 strings on every run, so a failure is reproducible from the
    /// seed alone.
    #[test]
    fn no_wrapped_line_ever_exceeds_the_box() {
        let mut text = Text::new();
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            seed >> 33
        };
        for case in 0..400 {
            // Mix arbitrary ASCII printables with identity-charset runs, so
            // both the hostile shape (a separator opening a continuation line
            // before a long unbroken token) and realistic content are covered.
            let alphabet: &[u8] = if case % 2 == 0 {
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/-"
            } else {
                b" !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
            };
            let len = 1 + (next() % 80) as usize;
            let s: String = (0..len)
                .map(|_| alphabet[(next() as usize) % alphabet.len()] as char)
                .collect();
            for (max_w, max_l) in [(100u32, 3usize), (200, 4), (508, 2)] {
                for line in text.wrap(&s, 14.0, max_w, max_l) {
                    assert!(
                        text.width(&line, 14.0) <= max_w,
                        "wrap({s:?}, max_width={max_w}) returned {line:?} at \
                         {}px, over the {max_w}px box",
                        text.width(&line, 14.0)
                    );
                }
            }
        }
        // The specific shape that used to fail: a lone break character opening
        // a line, followed by an unbroken run wider than the box.
        let hostile = format!("_{}", "W".repeat(40));
        for line in text.wrap(&hostile, 14.0, 100, 3) {
            assert!(text.width(&line, 14.0) <= 100, "{line:?} overflows");
        }
    }

    #[test]
    fn wrapping_respects_the_width_and_prefers_separators() {
        let mut text = Text::new();
        let px = 14.0;
        let id = "vitrin://local/agent/demonstration-agent-with-a-long-name";
        let lines = text.wrap(id, px, 200, 4);
        assert!(lines.len() > 1, "a long identity must wrap");
        for line in &lines {
            assert!(
                text.width(line, px) <= 200,
                "wrapped line {line:?} overflows the box"
            );
        }
        // Nothing is lost when it fits: the pieces reassemble to the input.
        assert_eq!(lines.concat(), id);
        // Prefers to break after a separator rather than mid-token.
        assert!(
            lines[0].ends_with(['/', '-', '_', '.', ':']),
            "expected a separator break, got {:?}",
            lines[0]
        );
    }

    #[test]
    fn overflow_is_marked_never_silently_clipped() {
        // The anti-spoofing rule: two identities sharing a long prefix must
        // not render identically just because the box ran out.
        let mut text = Text::new();
        let px = 14.0;
        let a = "vitrin://local/agent/trusted-thing";
        let b = "vitrin://local/agent/trusted-thing-EVIL";
        let (wa, wb) = (text.wrap(a, px, 120, 1), text.wrap(b, px, 120, 1));
        assert_eq!(wa.len(), 1);
        assert_eq!(wb.len(), 1);
        assert!(wa[0].ends_with(ELLIPSIS), "truncation must be marked");
        assert!(wb[0].ends_with(ELLIPSIS));
        for line in wa.iter().chain(wb.iter()) {
            assert!(text.width(line, px) <= 120, "marker must fit too: {line:?}");
        }
        // A box too small even for the marker degrades to the marker alone
        // rather than looping or overflowing.
        let tiny = text.wrap(a, px, 4, 1);
        assert_eq!(tiny, vec![ELLIPSIS.to_string()]);
    }

    #[test]
    fn wrapping_degenerate_inputs_terminates() {
        let mut text = Text::new();
        assert!(text.wrap("anything", 14.0, 0, 3).is_empty());
        assert!(text.wrap("anything", 14.0, 100, 0).is_empty());
        assert_eq!(text.wrap("", 14.0, 100, 3), vec![String::new()]);
    }

    #[test]
    fn line_metrics_are_positive_and_grow_with_size() {
        let text = Text::new();
        let small = text.line_metrics(11.0);
        let large = text.line_metrics(19.0);
        assert!(small.ascent > 0 && small.height > small.ascent);
        assert!(large.height > small.height);
    }
}
