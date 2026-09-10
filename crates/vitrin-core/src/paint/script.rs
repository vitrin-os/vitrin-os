// SPDX-License-Identifier: MPL-2.0
//! Which codepoints the picker may draw, and which it must transcribe.
//!
//! [`crate::paint::text`] draws one glyph per codepoint, left to right, with
//! no shaping, no bidi and no font fallback. That is not a limitation this
//! module works around — it is the property this module exists to keep. A
//! shaping engine is what makes a joining script legible, and a bidi
//! algorithm is what makes an RTL filename readable; a bidi algorithm is also
//! the RTL-override spoof, in which `annexe.txt` displays as `txt.exennа`.
//! So scripts that need either are transcribed rather than drawn, and that is
//! published as a known limit rather than left for a user to discover.
//!
//! # What a group is for
//!
//! A [`Group`] is named beside the row the human is reading. It is the
//! *second* of three defences against one filename being mistaken for
//! another, and it is worth being precise about which one does what, because
//! they fail in different places:
//!
//! 1. **The mixed-script rule** ([`permitted`]) escapes the minority
//!    characters of a name that mixes groups. This is what stops
//!    `rеsume.pdf` — Cyrillic `е` inside Latin — from reading as `resume.pdf`.
//! 2. **The group label** tells a human that a name they are reading is
//!    Cyrillic rather than Latin, so a wholly-Cyrillic lookalike announces
//!    itself.
//! 3. **The per-listing digest** refuses a directory listing in which two
//!    rows would render to identical pixels.
//!
//! None of the three separates two codepoints of the **same group** that
//! rasterize identically: the mixed-script rule sees one group, the label
//! prints one name, and two such files in *different* directories never meet
//! in one listing for the digest to compare. [`DENIED_APPEARANCE`] closes
//! exactly that gap, and nothing wider.
//!
//! ASCII letters are Unicode `Script=Latin` and are grouped as Latin here.
//! Keeping them apart would let a class count as "separated" that a human
//! reading one label cannot separate at all.
//!
//! # Nothing calls this yet
//!
//! The picker that will is #190's remaining half. Held to the same standard
//! `designation.rs` states for the ledger — proven mechanism, not in service.
//! The closure test runs against the shipped face regardless, so the table
//! cannot rot while it waits.

#![allow(dead_code)]

/// A script group, as named beside a row.
///
/// Deliberately coarse: Hiragana, Katakana and Han are one [`Group::Japanese`]
/// because they are one writing system to the human reading the label, and
/// splitting them would make ordinary Japanese text read as "mixed script".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Group {
    Latin,
    Greek,
    Cyrillic,
    Japanese,
}

/// What the renderer does with one codepoint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Route {
    /// Rasterize the glyph from the embedded vector face.
    Vector,
    /// Transcribe it. Never a fallthrough from a failed coverage query —
    /// always the explicit default, so a codepoint nobody considered is
    /// transcribed rather than drawn on a guess.
    Escape,
}

/// Codepoints denied because another codepoint **of the same group**
/// rasterizes identically at [`super::text::NAME_PX`], leaving nothing to tell them apart.
///
/// Every row is measured, not asserted: `the_appearance_closure_is_exactly_
/// this_table` recomputes the partition from the shipped face and fails if
/// this list is wrong in either direction. Changing the face, changing
/// `NAME_PX`, or widening a range therefore reddens a test rather than
/// quietly opening a hole.
///
/// Within a class the **lowest codepoint of each group** is the one kept.
///
/// | class at 14 px | kept | denied here |
/// |---|---|---|
/// | `U+0020` `U+00A0` | `U+0020` space | `U+00A0` no-break space |
/// | `U+0021` `U+01C3` | `U+0021` `!` | `U+01C3` `ǃ` retroflex click |
/// | `U+002D` `U+00AD` | `U+002D` `-` | `U+00AD` soft hyphen |
/// | `U+0049` `U+0399` `U+0406` `U+04C0` | `I`, `Ι`, `І` | `U+04C0` `Ӏ` — the second Cyrillic |
/// | `U+006C` `U+0196` `U+04CF` | `l`, `ӏ` | `U+0196` `Ɩ` — the second Latin |
/// | `U+00D0` `U+0110` `U+0189` | `U+00D0` `Ð` | `U+0110` `Đ`, `U+0189` `Ɖ` |
/// | `U+003B` `U+037E` | `U+003B` `;` | `U+037E` Greek question mark |
///
/// `U+00A0` and `U+00AD` are denied twice over — they are also `Zs`/`Cf` and
/// would be refused by [`route`] on that ground alone. They are listed anyway
/// because the closure genuinely finds them, and a table that quietly omitted
/// a measured member would be a table nobody could check.
pub(crate) const DENIED_APPEARANCE: &[char] = &[
    '\u{00A0}', '\u{00AD}', '\u{0110}', '\u{0189}', '\u{0196}', '\u{01C3}', '\u{037E}', '\u{04C0}',
];

/// The group a codepoint belongs to, or `None` when it is script-neutral.
///
/// Neutral characters — digits, and the ASCII punctuation a filename is built
/// from — belong to no script and therefore never make a name "mixed". A
/// filename is overwhelmingly likely to contain a dot; treating that dot as
/// Latin would make every Japanese filename mixed-script and transcribe the
/// half the font was added to draw.
pub(crate) fn group_of(ch: char) -> Option<Group> {
    match ch {
        '0'..='9'
        | ' '
        | '.'
        | '-'
        | '_'
        | '+'
        | '~'
        | '('
        | ')'
        | '['
        | ']'
        | '&'
        | '\''
        | ','
        | '!'
        | '@'
        | '#'
        | '$'
        | '%'
        | '^'
        | '='
        | '{'
        | '}'
        | ';' => None,
        'A'..='Z' | 'a'..='z' => Some(Group::Latin),
        '\u{00C0}'..='\u{024F}' => Some(Group::Latin),
        '\u{0370}'..='\u{03FF}' => Some(Group::Greek),
        '\u{0400}'..='\u{04FF}' => Some(Group::Cyrillic),
        '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' => Some(Group::Japanese),
        _ => None,
    }
}

/// Whether a name's set of groups may be drawn as-is.
///
/// UTS #39's Highly Restrictive level, minus the scripts this renderer cannot
/// draw at all. `{Latin, Japanese}` is permitted because ordinary Japanese
/// filenames contain ASCII — `論文2026.docx`, `メモ_v2.txt` — and forbidding
/// the combination would transcribe the Japanese half of nearly every real
/// one. It is not a weakening of the homoglyph defence: kana and Han resemble
/// no Latin letter, whereas `{Latin, Cyrillic}` and `{Latin, Greek}` — where
/// the lookalikes actually live — stay forbidden.
pub(crate) fn permitted(groups: &[Group]) -> bool {
    matches!(
        groups,
        [] | [_] | [Group::Latin, Group::Japanese] | [Group::Japanese, Group::Latin]
    )
}

/// What to do with one codepoint. Total, and `Escape` by default.
pub(crate) fn route(ch: char) -> Route {
    if DENIED_APPEARANCE.contains(&ch) {
        return Route::Escape;
    }
    // Control, format, surrogate, private-use and unassigned; combining marks;
    // and every space but U+0020. A combining mark without mark positioning
    // renders as a spacing glyph beside its base rather than over it, and a
    // default-ignorable can carry zero ink and zero advance — `a\u{200B}b`
    // would render as `ab`.
    if ch.is_control() || is_denied_class(ch) {
        return Route::Escape;
    }
    match ch {
        // ASCII printables, minus the classes above.
        ' '..='~' => Route::Vector,
        // The vector face's covered scripts. Japanese is deliberately absent:
        // the face carries no kana or Han, so it transcribes today. That is
        // stated rather than implied — `group_of` already names the group so
        // the mixed-script rule is correct in advance of the glyphs.
        '\u{00A1}'..='\u{024F}' | '\u{0370}'..='\u{03FF}' | '\u{0400}'..='\u{04FF}' => {
            Route::Vector
        }
        _ => Route::Escape,
    }
}

/// Categories denied wholesale, checked without a Unicode database.
///
/// Only the ranges this renderer could otherwise reach need deciding, so an
/// explicit list is smaller and more auditable than a table, and
/// `no_routed_codepoint_is_in_a_denied_class` pins it against the routed set.
fn is_denied_class(ch: char) -> bool {
    matches!(ch as u32,
        // Combining marks reachable from the routed ranges.
        0x0300..=0x036F | 0x0483..=0x0489 | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF
        // Format, ignorable, and the bidi controls above all.
        | 0x00AD | 0x061C | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF
        // Every space but U+0020, and the line/paragraph separators.
        | 0x00A0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000
        // Surrogates and private use.
        | 0xD800..=0xDFFF | 0xE000..=0xF8FF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::text::{Text, NAME_PX};

    /// **The closure is measured, not asserted.**
    ///
    /// Partition every codepoint this module routes to `Vector` by what it
    /// actually rasterizes to at `NAME_PX`, and demand that no class hold
    /// two members of one group. [`DENIED_APPEARANCE`] is then exactly the
    /// set that has to be removed to make that true — checked in both
    /// directions, so a row that stopped being necessary fails just as loudly
    /// as a missing one.
    #[test]
    fn the_appearance_closure_is_exactly_this_table() {
        let mut text = Text::new();
        let mut classes: std::collections::HashMap<(Vec<u8>, usize, usize, u32), Vec<char>> =
            std::collections::HashMap::new();
        for cp in 0x20u32..=0x4FF {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            // Route as if the table were empty: the closure must be able to
            // rediscover every row from the face alone.
            if DENIED_APPEARANCE.contains(&ch) {
                // fall through: still partitioned, so a stale row is caught
            } else if route(ch) != Route::Vector {
                continue;
            }
            let Some((coverage, w, h, advance)) = text.raster_signature_for_test(ch, NAME_PX)
            else {
                continue;
            };
            classes
                .entry((coverage, w, h, advance))
                .or_default()
                .push(ch);
        }

        let mut must_deny: Vec<char> = Vec::new();
        for members in classes.values() {
            if members.len() < 2 {
                continue;
            }
            // Two members of a class are distinguishable only if a human
            // reading the row's script label sees different labels for the
            // two names. That is weaker than "different groups":
            //
            // * A **neutral** member distinguishes nothing. `hello!.txt` and
            //   `helloǃ.txt` differ only at a neutral `!` versus a Latin `ǃ`,
            //   yet both labels read "Latin" because `hello` already supplied
            //   it. So a neutral member is safe only when it is the class's
            //   sole survivor.
            // * Two members of **different** non-neutral groups are safe,
            //   because a name carrying both groups is mixed-script and
            //   `permitted` transcribes it before it can be drawn at all.
            let mut sorted = members.clone();
            sorted.sort();
            let any_neutral = sorted.iter().any(|&ch| group_of(ch).is_none());
            let mut kept: Vec<Group> = Vec::new();
            let mut kept_anything = false;
            for ch in sorted {
                let keep = match group_of(ch) {
                    _ if any_neutral => !kept_anything,
                    Some(g) if !kept.contains(&g) => {
                        kept.push(g);
                        true
                    }
                    _ => false,
                };
                if keep {
                    kept_anything = true;
                } else {
                    must_deny.push(ch);
                }
            }
        }
        must_deny.sort();
        must_deny.dedup();

        let mut declared = DENIED_APPEARANCE.to_vec();
        declared.sort();
        // Print codepoints, never the characters. A confusable report that
        // renders its confusables is unreadable by construction: U+003B and
        // U+037E both print as `;`, which is the entire reason U+037E is on
        // this list.
        let show = |v: &[char]| -> String {
            v.iter()
                .map(|c| format!("U+{:04X}", *c as u32))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (required, declared_s) = (show(&must_deny), show(&declared));
        assert_eq!(
            must_deny, declared,
            "the appearance closure over the shipped face at {NAME_PX}px does not match \
             DENIED_APPEARANCE.\n  required by the face: [{required}]\n  declared in the \
             table: [{declared_s}]\nA codepoint required but not declared is an open hole; \
             one declared but not required is a row that has stopped doing anything and \
             should be removed rather than left to look protective."
        );
    }

    /// Nothing that routes to `Vector` may sit in a class denied wholesale.
    #[test]
    fn no_routed_codepoint_is_in_a_denied_class() {
        for cp in 0x20u32..=0x4FF {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            if route(ch) == Route::Vector {
                assert!(
                    !is_denied_class(ch) && !ch.is_control(),
                    "U+{cp:04X} routes to Vector but is in a denied class"
                );
            }
        }
    }

    /// The mixed-script rule permits exactly the sets the owner decided.
    #[test]
    fn only_the_decided_group_sets_are_permitted() {
        use Group::*;
        for g in [Latin, Greek, Cyrillic, Japanese] {
            assert!(permitted(&[g]), "a single-script name is always permitted");
        }
        assert!(
            permitted(&[]),
            "a name of only neutral characters is permitted"
        );
        assert!(
            permitted(&[Latin, Japanese]),
            "Latin+Japanese is permitted: ordinary Japanese filenames contain ASCII"
        );
        // Where the lookalikes actually live.
        assert!(!permitted(&[Latin, Cyrillic]));
        assert!(!permitted(&[Latin, Greek]));
        assert!(!permitted(&[Greek, Cyrillic]));
        assert!(!permitted(&[Latin, Greek, Cyrillic]));
    }

    /// The homoglyph this whole scheme exists for is a mixed-script name.
    #[test]
    fn the_cyrillic_lookalike_is_not_a_permitted_name() {
        let name = "rеsume.pdf"; // the `е` is U+0435
        let mut groups: Vec<Group> = name.chars().filter_map(group_of).collect();
        groups.sort();
        groups.dedup();
        assert_eq!(groups, vec![Group::Latin, Group::Cyrillic]);
        assert!(
            !permitted(&groups),
            "a Latin name carrying one Cyrillic lookalike must not render as-is"
        );
    }

    /// And an ordinary Japanese filename is.
    #[test]
    fn an_ordinary_japanese_filename_is_permitted() {
        for name in [
            "論文2026.docx",
            "メモ_v2.txt",
            "IMG_001_日本.jpg",
            "日本語.txt",
        ] {
            let mut groups: Vec<Group> = name.chars().filter_map(group_of).collect();
            groups.sort();
            groups.dedup();
            assert!(
                permitted(&groups),
                "{name} must be drawable as-is; got groups {groups:?}"
            );
        }
    }
}
