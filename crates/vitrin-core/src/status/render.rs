// SPDX-License-Identifier: MPL-2.0
//! The **pure half** of the status strip: one [`StatusSnapshot`] in, one
//! RGBA8888 raster out.
//!
//! Reads no clock, opens no file, and holds no state. That is what makes
//! `tests/golden/status_strip.txt` possible at all — the trap
//! [`crate::consent::render`] names ("a clock on the card would make its pixels
//! a function of `Instant::now()` — untestable by construction") is avoided by
//! keeping every world-reading line in [`super::sample`].
//!
//! # The geometry is the security argument, so it lives in constants
//!
//! Two rows of pixels are spoken for by other core-drawn surfaces, and this
//! module must not touch either:
//!
//! - Rows `[0, TRUST_BAND_HEIGHT)` are the trusted band. **This module cannot
//!   reach them**, and not because it declines to: [`rasterize`] returns a
//!   buffer that is `height` rows tall and [`super::StatusStrip::composite_over`]
//!   blits it at `y = TRUST_BAND_HEIGHT`. There is no coordinate expressible
//!   here that lands in the band. That is the shape issue #215 exists to
//!   protect and it is a structural fact rather than a checked one.
//! - Columns `[0, MARKER_W)` of the strip's own rows are the attention
//!   marker's lane (WS-E.1.7, issue #232). The marker is composited *after*
//!   the band and therefore after this strip, so a glyph drawn there would be
//!   half-covered whenever the human's window is open. [`CONTENT_X`] starts
//!   past it, read from [`crate::attention::MARKER_W`] and never restated, and
//!   the `const` assertions below turn a marker that outgrew the lane into a
//!   **compile error**.
//!
//! # What it may say
//!
//! Three fields and no fourth. Every string below is a `const` or a typed core
//! value ([`crate::grants::RealmId`] out of the operator's `realm.toml`, two
//! integers out of the sampler) — [`crate::lock::render`]'s rule, for the same
//! reason: an app cannot put a glyph on a trusted surface if there is no field
//! through which one could travel. [`crate::paint::text`] substitutes anything
//! outside ASCII printables at the last moment regardless.

use crate::paint::canvas::{Canvas, Rect};
use crate::paint::text::Text;
use crate::scene::BYTES_PER_PIXEL;

use super::battery::BatteryReading;
use super::sample::StatusSnapshot;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Type size. **Measured**: at 12 px the bundled face reports a line box of 14
/// rows with an 11-row ascent (`Text::line_metrics(12.0)`), which is what
/// [`super::DEFAULT_HEIGHT`] is derived from — 14 rows of type plus 3 rows of
/// padding above and below.
pub(crate) const TEXT_PX: f32 = 12.0;

/// Where the strip's own content starts, in strip-local columns.
///
/// Past the attention marker's lane, plus a gap. Derived from
/// [`crate::attention::MARKER_W`]; a change there moves this without an edit
/// here, which is the point.
pub(crate) const CONTENT_X: u32 = crate::attention::MARKER_W + GAP;

/// Horizontal breathing room: between the marker lane and the first glyph,
/// between fields, and at the right edge.
const GAP: u32 = 12;

/// The marker must fit inside the lane this module leaves for it, and inside
/// the strip's own rows. Both are compile-time facts rather than tests, so a
/// later edit to either constant fails the build instead of producing a
/// half-covered clock nobody notices in a screenshot.
const _: () = assert!(super::MIN_HEIGHT >= crate::attention::MARKER_H);
const _: () = assert!(CONTENT_X > crate::attention::MARKER_W);

// ---------------------------------------------------------------------------
// Palette (opaque; the strip replaces whatever is under it)
// ---------------------------------------------------------------------------

/// The strip's ground. Dark, flat and **deliberately not** confusable with the
/// lock screen's cover or the consent card's background: those two surfaces ask
/// for something, and this one asks for nothing. It is also nothing like a
/// [`crate::consent::TrustedIndicator`] colour, which is drawn from `[64, 255]`
/// per channel — this is far below that floor on every channel, so a human
/// cannot mistake the strip for the band even at a glance.
const GROUND: [u8; 4] = [0x11, 0x13, 0x18, 0xff];
/// A one-row rule along the strip's bottom edge, so the strip reads as a fixed
/// piece of chrome rather than as the top of whatever the app is drawing.
const RULE: [u8; 4] = [0x2b, 0x30, 0x3b, 0xff];
const VALUE_FG: [u8; 3] = [0xe4, 0xe9, 0xf2];
const MUTED_FG: [u8; 3] = [0x93, 0x9e, 0xb0];

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/// What the realm field says when the output is bound to no realm. Not a blank:
/// "nothing is bound" is a fact about the session a human benefits from seeing,
/// unlike a battery that is not there.
const NO_REALM: &str = "(no realm)";
/// Appended to the battery percentage when it is charging or full. Two ASCII
/// letters rather than a glyph, because the bundled face is ASCII-only
/// ([`crate::paint::text`]) and a substituted box would read as a fault.
const CHARGING: &str = "AC";
/// The zone label when the offset is zero.
const UTC: &str = "UTC";

/// One rasterized strip.
pub(crate) struct Strip {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Rasterize `snapshot` into a `width x height` strip.
///
/// `height` is the operator's `--status-height`, already clamped to
/// `[MIN_HEIGHT, MAX_HEIGHT]` at parse time; `width` is the view's. A degenerate
/// size yields an empty raster rather than a panic — this runs inside the
/// compositor loop, where failing to draw is bad and taking the session down
/// mid-frame is worse.
pub(crate) fn rasterize(snapshot: &StatusSnapshot, width: u32, height: u32) -> Strip {
    let mut rgba = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
    if width == 0 || height == 0 {
        return Strip {
            rgba,
            width,
            height,
        };
    }
    let mut text = Text::new();
    // A canvas of the exact size just allocated cannot be refused; the `else`
    // arm keeps a core bug from becoming a panic in a compositor loop.
    let Some(mut canvas) = Canvas::new(&mut rgba, width, height) else {
        return Strip {
            rgba,
            width,
            height,
        };
    };
    canvas.fill_rect(
        Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        GROUND,
    );
    canvas.fill_rect(
        Rect {
            x: 0,
            y: height as i32 - 1,
            w: width,
            h: 1,
        },
        RULE,
    );

    let metrics = text.line_metrics(TEXT_PX);
    // Vertically centred in the strip's own rows. Truncating division, so the
    // odd leftover row lands below the type — deterministically, which is what
    // the golden needs.
    let baseline = (height.saturating_sub(metrics.height) / 2 + metrics.ascent) as i32;

    // Right, in reading order: battery then clock, laid out from the right edge
    // inward so the clock's position does not move when the battery appears or
    // disappears. An absent battery is an **empty slot**: nothing is drawn, and
    // nothing stands in for it.
    //
    // **Floored at `CONTENT_X`, not at zero.** `saturating_sub` alone bottoms
    // out at column 0, which is inside the attention marker's lane -- so on a
    // narrow enough output the clock was drawn straight through the marker
    // while this module claimed the lane was structurally unreachable. The
    // `const` assertions really do pin `CONTENT_X`, and the right-aligned
    // fields are the half they never constrained. A field with nowhere to go is
    // DROPPED rather than squeezed: a clock overlapping a battery reading is
    // two numbers a human cannot separate, which is worse than one number.
    let mut right = width.saturating_sub(GAP);
    if let Some(clock) = snapshot.clock {
        let s = format_clock(clock);
        let w = text.width(&s, TEXT_PX);
        if right.saturating_sub(w) >= CONTENT_X {
            right -= w;
            text.draw(&mut canvas, &s, TEXT_PX, right as i32, baseline, VALUE_FG);
        }
    }
    if let BatteryReading::Present { percent, charging } = snapshot.battery {
        let s = format_battery(percent, charging);
        let w = text.width(&s, TEXT_PX);
        if right.saturating_sub(w + GAP) >= CONTENT_X {
            right -= w + GAP;
            text.draw(&mut canvas, &s, TEXT_PX, right as i32, baseline, MUTED_FG);
        }
    }

    // Left, LAST, because it is the only variable-width field and the only one
    // that must yield: realm ids are operator-written and legal up to 64 bytes,
    // so an unbounded draw here runs straight through the battery and clock and
    // blends with their glyphs. `paint::text::wrap` exists because "a quietly
    // clipped identity is a spoofing primitive" on a trusted surface -- so this
    // clips VISIBLY, with an ellipsis, rather than letting glyphs collide or
    // truncating in silence. A human who sees `long-realm-na…` knows they are
    // reading a shortened name; one who sees it merged into `84%` does not.
    let realm = snapshot
        .realm
        .as_ref()
        .map(|realm| realm.as_str())
        .unwrap_or(NO_REALM);
    let realm_fg = if snapshot.realm.is_some() {
        VALUE_FG
    } else {
        MUTED_FG
    };
    let room = right.saturating_sub(CONTENT_X).saturating_sub(GAP);
    let shown = elide_to_width(&mut text, realm, TEXT_PX, room);
    text.draw(
        &mut canvas,
        &shown,
        TEXT_PX,
        CONTENT_X as i32,
        baseline,
        realm_fg,
    );

    Strip {
        rgba,
        width,
        height,
    }
}

/// `"14:04 +09:00"`, or `"05:04 UTC"`.
///
/// The zone is **always** drawn. The core holds no timezone database
/// ([`super::sample`]), so the hour is only as right as the operator's
/// `--status-utc-offset`; a clock that showed the hour without saying which
/// clock it is would turn a wrong flag into a wrong clock.
/// Shorten `s` until it fits `room` pixels, marking the cut with an ellipsis.
///
/// Returns an empty string when even the ellipsis will not fit — drawing
/// nothing is honest, and drawing a lone `…` at a width that cannot hold it
/// would overlap the field to its right, which is the collision this exists to
/// prevent. ASCII `...` rather than U+2026 because `paint::text` substitutes
/// anything outside `0x20..=0x7E` at the last moment, so a real ellipsis would
/// render as the substitution glyph.
fn elide_to_width(text: &mut Text, s: &str, px: f32, room: u32) -> String {
    if text.width(s, px) <= room {
        return s.to_owned();
    }
    const CUT: &str = "...";
    let cut_w = text.width(CUT, px);
    if cut_w > room {
        return String::new();
    }
    let mut end = s.len();
    while end > 0 {
        if !s.is_char_boundary(end) {
            end -= 1;
            continue;
        }
        let candidate = &s[..end];
        if text.width(candidate, px) + cut_w <= room {
            return format!("{candidate}{CUT}");
        }
        end -= 1;
    }
    CUT.to_owned()
}

fn format_clock(clock: super::sample::ClockReading) -> String {
    let minutes = clock.offset.minutes();
    if minutes == 0 {
        return format!("{:02}:{:02} {UTC}", clock.hour, clock.minute);
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let abs = minutes.unsigned_abs();
    format!(
        "{:02}:{:02} {sign}{:02}:{:02}",
        clock.hour,
        clock.minute,
        abs / 60,
        abs % 60
    )
}

/// `"87%"`, or `"87% AC"` while charging.
fn format_battery(percent: u8, charging: bool) -> String {
    if charging {
        format!("{percent}% {CHARGING}")
    } else {
        format!("{percent}%")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The strip's ground colour, re-exported for the backend tests that have
    /// to locate the strip in a composited framebuffer. Exported rather than
    /// restated: a test that hunted for a hard-coded `0x11131 8` would silently
    /// stop measuring anything the day the palette changed.
    pub(crate) const GROUND: [u8; 4] = super::GROUND;
    use crate::grants::RealmId;
    use crate::status::sample::{ClockReading, UtcOffset};

    fn px(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    pub(crate) fn fixture() -> StatusSnapshot {
        StatusSnapshot {
            clock: ClockReading::from_system_time(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_786_244_643),
                UtcOffset::parse("+09:00").expect("a well-formed offset"),
            ),
            battery: BatteryReading::Present {
                percent: 87,
                charging: false,
            },
            realm: Some(RealmId::new("realm-0")),
        }
    }

    #[test]
    fn the_clock_always_names_its_zone() {
        let utc = ClockReading {
            hour: 5,
            minute: 4,
            offset: UtcOffset::UTC,
        };
        assert_eq!(format_clock(utc), "05:04 UTC");
        let jst = ClockReading {
            hour: 14,
            minute: 4,
            offset: UtcOffset::parse("+09:00").unwrap(),
        };
        assert_eq!(format_clock(jst), "14:04 +09:00");
        let marquesas = ClockReading {
            hour: 19,
            minute: 34,
            offset: UtcOffset::parse("-09:30").unwrap(),
        };
        assert_eq!(format_clock(marquesas), "19:34 -09:30");
    }

    #[test]
    fn the_battery_reads_as_a_percentage_and_says_when_it_is_charging() {
        assert_eq!(format_battery(87, false), "87%");
        assert_eq!(format_battery(87, true), "87% AC");
        assert_eq!(format_battery(0, false), "0%");
    }

    /// The load-bearing negative: **nothing this module draws can land in the
    /// attention marker's lane**, whatever the snapshot says. Checked over the
    /// widest field values the strip can carry (a maximum-length realm id),
    /// against the ground colour, on every row.
    ///
    /// **Every width, not just 640.** The first version of this test used one
    /// width, and the `const` assertions it leaned on constrain `CONTENT_X`
    /// only -- the left-aligned field. The clock and battery are laid out from
    /// the RIGHT edge inward, and on a narrow output they walked straight into
    /// the lane while this module's docs claimed no expressible coordinate
    /// could. Narrow widths are where that shows.
    #[test]
    fn no_glyph_lands_in_the_attention_markers_lane() {
        for width in [64, 96, 128, 200, 320, 640, 2560] {
            let mut snapshot = fixture();
            snapshot.realm = Some(RealmId::new("r".repeat(64)));
            let strip = rasterize(&snapshot, width, super::super::DEFAULT_HEIGHT);
            for y in 0..strip.height - 1 {
                for x in 0..crate::attention::MARKER_W.min(strip.width) {
                    assert_eq!(
                        px(&strip.rgba, strip.width, x, y),
                        GROUND,
                        "the marker's lane must stay the strip's ground at ({x}, {y}) \
                         on a {width}px output"
                    );
                }
            }
        }
    }

    /// **A maximum-length realm id is elided, not overrun.** Ids are
    /// operator-written and legal up to 64 bytes; drawn unbounded they run
    /// through the battery and clock and blend with their glyphs.
    /// `paint::text::wrap` exists because "a quietly clipped identity is a
    /// spoofing primitive", and the same argument applies to one that collides.
    #[test]
    fn a_long_realm_id_is_elided_rather_than_overrunning_the_clock() {
        let mut snapshot = fixture();
        snapshot.realm = Some(RealmId::new("r".repeat(64)));

        // **320px, not 640.** At 640 a 64-character id does not reach the
        // clock, so the comparison below is trivially true and the test passes
        // with elision removed -- which is what the mutation run showed. A
        // narrow output is where the overrun is unambiguous: 64 glyphs at this
        // size are wider than the whole strip.
        const NARROW: u32 = 320;
        let long = rasterize(&snapshot, NARROW, super::super::DEFAULT_HEIGHT);
        let mut short_snap = snapshot.clone();
        short_snap.realm = Some(RealmId::new("r"));
        let short = rasterize(&short_snap, NARROW, super::super::DEFAULT_HEIGHT);

        let right_third = (long.width / 3) * 2;
        for y in 0..long.height {
            for x in right_third..long.width {
                assert_eq!(
                    px(&long.rgba, long.width, x, y),
                    px(&short.rgba, short.width, x, y),
                    "a 64-byte realm id changed a pixel in the clock/battery region at \
                     ({x}, {y}): it must be elided at the left, never allowed to run into \
                     fields a human reads as separate values"
                );
            }
        }
    }

    /// An absent battery is an **empty slot**, not a placeholder: the strip with
    /// no battery and the strip with one differ only where the battery would
    /// have been drawn, and the clock does not move.
    #[test]
    fn an_absent_battery_leaves_the_clock_where_it_was() {
        let with = rasterize(&fixture(), 640, super::super::DEFAULT_HEIGHT);
        let mut none = fixture();
        none.battery = BatteryReading::Absent;
        let without = rasterize(&none, 640, super::super::DEFAULT_HEIGHT);
        // The clock is the rightmost field; the battery sits a GAP to its left.
        // Everything from the clock's left edge rightward is byte-identical.
        let mut text = Text::new();
        let clock_w = text.width(&format_clock(fixture().clock.unwrap()), TEXT_PX);
        let clock_x = 640 - GAP - clock_w;
        for y in 0..with.height {
            for x in clock_x..640 {
                assert_eq!(
                    px(&with.rgba, 640, x, y),
                    px(&without.rgba, 640, x, y),
                    "the clock moved when the battery vanished, at ({x}, {y})"
                );
            }
        }
        assert_ne!(
            with.rgba, without.rgba,
            "a present battery must actually draw something"
        );
    }

    #[test]
    fn a_degenerate_size_yields_an_empty_raster_rather_than_a_panic() {
        for (w, h) in [(0, 24), (640, 0), (0, 0)] {
            let strip = rasterize(&fixture(), w, h);
            assert!(strip.rgba.is_empty());
        }
    }

    /// Every pixel is opaque: the strip **replaces** what is under it, so a
    /// client's rows can never show through it.
    #[test]
    fn the_strip_is_entirely_opaque() {
        let strip = rasterize(&fixture(), 640, super::super::DEFAULT_HEIGHT);
        assert!(
            strip.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0xff),
            "a translucent status strip would let client content through it"
        );
    }
}
