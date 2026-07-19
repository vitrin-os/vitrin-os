//! Text for the consent prompt: the bundled font, glyph rasterization, and
//! the measure/wrap/draw operations [`super::render`] lays the card out with.
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
//! # Only ASCII printables reach the rasterizer
//!
//! [`Text::draw`] substitutes anything outside `0x20..=0x7E` with `?`
//! ([`SUBSTITUTE`]). Today that can never fire — every string the prompt
//! draws is core-derived, and [`crate::identity::PrincipalIdentity`] already
//! restricts identities to an ASCII subset at parse time — so this is
//! defense in depth against a *future* change that widens what may be shown:
//! bidi overrides, zero-width joiners, and combining-mark stacks are exactly
//! the tools for making one string render as another, and a consent prompt is
//! the highest-value place in the system to play that game. Substituting at
//! the last possible moment means no such character can reach glyph selection
//! however it got into a string.
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

use fontdue::{Font, FontSettings, Metrics};

use super::canvas::Canvas;

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
    "the vendored consent-prompt font changed; see crates/vitrin-core/assets/fonts/README.md"
);

/// What [`Text::draw`] renders in place of any non-ASCII-printable character
/// (module docs). Visible on purpose: a silently dropped character is a
/// consent prompt quietly saying something other than what it was given.
const SUBSTITUTE: char = '?';

/// `FontSettings::scale`, pinned rather than defaulted.
///
/// fontdue feeds this to its geometry preprocessing, so it is an input to
/// rasterized output; leaving it at the crate's default would let a fontdue
/// release move the golden with no change on our side. The value matches the
/// prompt's largest type size (`super::render::TITLE_PX` = 19.0), which is
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
        .expect("the bundled consent-prompt font is a compile-time constant and must parse")
    })
}

/// One rasterized glyph: fontdue's metrics plus its 8-bit coverage bitmap.
struct Glyph {
    metrics: Metrics,
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

/// The prompt's text engine: the shared face plus a per-instance glyph cache.
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
    fn glyph(&mut self, ch: char, px: f32) -> &Glyph {
        self.cache.entry((ch, px.to_bits())).or_insert_with(|| {
            let (metrics, coverage) = font().rasterize(ch, px);
            Glyph { metrics, coverage }
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

    /// Advance width of `s` at `px`, in whole pixels — what the layout
    /// measures with and what [`Self::draw`] then advances by, so measured
    /// and drawn text can never disagree.
    pub fn width(&mut self, s: &str, px: f32) -> u32 {
        let mut pen = 0.0f32;
        for ch in s.chars() {
            pen += self.glyph(Self::renderable(ch), px).metrics.advance_width;
        }
        // `max(0.0)` before the cast: a negative accumulated advance is not
        // reachable with this font, but `as u32` on a negative float
        // saturates to 0 silently and a width is unsigned by nature.
        pen.max(0.0).round() as u32
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
        for ch in s.chars() {
            let glyph = self.glyph(Self::renderable(ch), px);
            let advance = glyph.metrics.advance_width;
            let gx = x + pen.round() as i32 + glyph.metrics.xmin;
            // fontdue reports `ymin` from the baseline upward, so the top of
            // the bitmap sits `height + ymin` above it.
            let gy = baseline - glyph.metrics.ymin - glyph.metrics.height as i32;
            let (w, h) = (glyph.metrics.width, glyph.metrics.height);
            // Copy out of the cache before touching the canvas: the borrow
            // checker aside, this keeps the blend loop free of a hash lookup.
            let coverage = glyph.coverage.clone();
            for row in 0..h {
                for col in 0..w {
                    let alpha = coverage[row * w + col];
                    canvas.blend_pixel(gx + col as i32, gy + row as i32, rgb, alpha);
                }
            }
            pen += advance;
        }
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
            current = carry;
            current.push(ch);
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
