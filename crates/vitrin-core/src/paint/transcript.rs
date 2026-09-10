// SPDX-License-Identifier: MPL-2.0
//! Turning a filename's bytes into something the picker can draw, without
//! ever turning two different filenames into the same thing.
//!
//! # The property
//!
//! `encode` is **injective**: distinct byte strings produce distinct
//! transcripts. That is not asserted, it is witnessed — [`decode`] is a total
//! left inverse, and `decode(encode(b).text) == Some(b)` for every `b`. A
//! function with a left inverse is injective, so the test is the proof.
//!
//! Injectivity of the *transcript* is where this module's job ends. Two
//! transcripts can still rasterize alike, and that is caught elsewhere: by
//! [`super::script::DENIED_APPEARANCE`] for same-group confusables, and by
//! the per-listing digest for everything else.
//!
//! # Why the sigil is doubled unconditionally
//!
//! `\` is escaped as `\\` **everywhere**, never only where it could be
//! mistaken for a form. That single rule is what stops the escape syntax
//! colliding with a filename that contains the escape syntax:
//!
//! | filename | transcript |
//! |---|---|
//! | `é.pdf` | `\u{00E9}.pdf` |
//! | `\u{00E9}.pdf` (12 literal bytes) | `\\u{00E9}.pdf` |
//!
//! A contextual rule — "escape the sigil only when what follows looks like a
//! form" — reads as the cheaper option and is not: it makes the encoder's
//! output depend on lookahead, and the two cases above collapse onto each
//! other the moment the lookahead is wrong about anything.
//!
//! Every form is brace-delimited and fixed-shape, so there is exactly one
//! termination rule. A grammar mixing terminated and unterminated forms in
//! one alphabet can be ambiguous, and an ambiguous encoder has no inverse.

#![allow(dead_code)]

use super::script::{group_of, permitted, route, Group, Route};
use std::collections::BTreeSet;
use std::ops::Range;

/// The sigil. Chosen because it is legal in a POSIX filename but rare in one,
/// so the doubling rule is cheap in practice and correct regardless.
const SIGIL: char = '\\';

/// What a run of the transcript is, so the renderer can tint it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Class {
    /// Drawn as itself, and ASCII — the unremarkable case, drawn untinted.
    Ascii,
    /// Drawn as itself, but outside ASCII. Tinted, and its script named.
    Native,
    /// Not drawn as itself. Tinted differently again, so an escape is a
    /// second, independent tell beyond the character shapes.
    Escaped,
}

/// A filename prepared for drawing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Transcript {
    /// What is actually drawn.
    pub(crate) text: String,
    /// Byte ranges of `text`, in order, each with what it is. Adjacent runs
    /// of one class are merged, so a run boundary is always a class change.
    pub(crate) runs: Vec<(Range<usize>, Class)>,
    /// The scripts present among the natively-drawn characters, for the label
    /// beside the row. Escaped characters contribute nothing: they are not
    /// drawn in their own script, so naming it would describe something the
    /// human cannot see.
    pub(crate) groups: BTreeSet<Group>,
}

/// Prepare `name` for drawing.
pub(crate) fn encode(name: &[u8]) -> Transcript {
    // Pass 1: decode to scalars, remembering which bytes were not valid
    // UTF-8 at all. An invalid byte can never be drawn, so it does not
    // participate in the script vote.
    let mut items: Vec<Item> = Vec::new();
    let mut rest = name;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                items.extend(s.chars().map(Item::Scalar));
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                if good > 0 {
                    // Safe by construction: `valid_up_to` is a UTF-8 boundary.
                    let s = std::str::from_utf8(&rest[..good]).expect("valid_up_to is valid");
                    items.extend(s.chars().map(Item::Scalar));
                }
                let bad = e.error_len().unwrap_or(rest.len() - good).max(1);
                items.extend(rest[good..good + bad].iter().copied().map(Item::RawByte));
                rest = &rest[good + bad..];
            }
        }
    }

    // Pass 2: the script vote, taken **per word** rather than per name.
    //
    // A filename is not one word. `отчёт.pdf` is a Cyrillic word and an ASCII
    // extension, and voting over the whole name makes it Latin+Cyrillic — so
    // a whole-name rule refuses it and escapes either the stem or the `pdf`.
    // Every Cyrillic, Greek or Japanese filename in existence carries an
    // ASCII extension, so that rule would transcribe the half the font exists
    // to draw.
    //
    // Mixing scripts *within* a word is the attack — `rеsume` with a Cyrillic
    // `е` — and mixing them across a separator is ordinary naming. So a word
    // is what votes, and a separator resets the vote. This is strictly
    // stronger than a whole-name rule where it matters: `rеsume.pdf` still
    // escapes, because the mixing is inside the word.
    let mut drawable: Vec<bool> = vec![false; items.len()];
    let mut word: Vec<usize> = Vec::new();
    let settle = |word: &mut Vec<usize>, drawable: &mut Vec<bool>| {
        let mut counts: Vec<(Group, usize, usize)> = Vec::new();
        for &i in word.iter() {
            let Item::Scalar(ch) = items[i] else { continue };
            if route(ch) != Route::Vector {
                continue;
            }
            let Some(g) = group_of(ch) else { continue };
            match counts.iter_mut().find(|(cg, _, _)| *cg == g) {
                Some(entry) => entry.1 += 1,
                None => counts.push((g, 1, i)),
            }
        }
        let mut distinct: Vec<Group> = counts.iter().map(|(g, _, _)| *g).collect();
        distinct.sort();
        // The surviving group, used only when the combination is refused.
        // Most characters wins; a tie goes to whichever appeared first, so
        // the choice is a function of the name and not of iteration order.
        let survivor: Option<Group> = if permitted(&distinct) {
            None
        } else {
            counts
                .iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)))
                .map(|(g, _, _)| *g)
        };
        for &i in word.iter() {
            let Item::Scalar(ch) = items[i] else { continue };
            drawable[i] = route(ch) == Route::Vector
                && match (survivor, group_of(ch)) {
                    (None, _) => true,
                    (Some(_), None) => true,
                    (Some(s), Some(g)) => s == g,
                };
        }
        word.clear();
    };
    for i in 0..items.len() {
        let is_separator = match items[i] {
            Item::Scalar(ch) => is_word_separator(ch),
            // An undecodable byte is always escaped, and it separates: two
            // words either side of a raw byte were never one word.
            Item::RawByte(_) => true,
        };
        if is_separator {
            settle(&mut word, &mut drawable);
            if let Item::Scalar(ch) = items[i] {
                drawable[i] = route(ch) == Route::Vector;
            }
        } else {
            word.push(i);
        }
    }
    settle(&mut word, &mut drawable);

    // Pass 3: emit.
    let mut text = String::new();
    let mut runs: Vec<(Range<usize>, Class)> = Vec::new();
    let mut groups = BTreeSet::new();
    for (idx, item) in items.iter().enumerate() {
        let start = text.len();
        let class = match *item {
            Item::RawByte(b) => {
                text.push_str(&format!("{SIGIL}x{{{b:02X}}}"));
                Class::Escaped
            }
            Item::Scalar(ch) if ch == SIGIL => {
                text.push(SIGIL);
                text.push(SIGIL);
                Class::Escaped
            }
            Item::Scalar(ch) => {
                if drawable[idx] {
                    text.push(ch);
                    if let Some(g) = group_of(ch) {
                        groups.insert(g);
                    }
                    if ch.is_ascii() {
                        Class::Ascii
                    } else {
                        Class::Native
                    }
                } else {
                    let cp = ch as u32;
                    if cp <= 0xFFFF {
                        text.push_str(&format!("{SIGIL}u{{{cp:04X}}}"));
                    } else {
                        text.push_str(&format!("{SIGIL}u{{{cp:06X}}}"));
                    }
                    Class::Escaped
                }
            }
        };
        let end = text.len();
        match runs.last_mut() {
            Some((range, last)) if *last == class => range.end = end,
            _ => runs.push((start..end, class)),
        }
    }

    Transcript { text, runs, groups }
}

/// A character that ends a word for the purposes of the script vote.
///
/// Exactly the neutral punctuation a filename is structured with. Digits are
/// neutral but are **not** separators: `論文2026` is one word, and splitting
/// it would let a lookalike hide across a digit.
fn is_word_separator(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '.'
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
            | ';'
    )
}

enum Item {
    Scalar(char),
    RawByte(u8),
}

/// The left inverse. Total: every input either yields the unique byte string
/// that encodes to it, or `None`.
///
/// This is the injectivity witness, so it lives beside `encode` rather than
/// in the test module — a proof kept somewhere else is a proof that stops
/// being maintained.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != SIGIL {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next()? {
            SIGIL => out.push(b'\\'),
            'x' => {
                let digits = braced(&mut chars)?;
                if digits.len() != 2 {
                    return None;
                }
                out.push(u8::from_str_radix(&digits, 16).ok()?);
            }
            'u' => {
                let digits = braced(&mut chars)?;
                if digits.len() != 4 && digits.len() != 6 {
                    return None;
                }
                let cp = u32::from_str_radix(&digits, 16).ok()?;
                // A 4-digit form must not carry a value that `encode` would
                // have written with 6, or two transcripts would decode alike.
                if digits.len() == 6 && cp <= 0xFFFF {
                    return None;
                }
                let ch = char::from_u32(cp)?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Read `{HEX}` and return the hex digits. Uppercase only: accepting both
/// cases would give two transcripts that decode alike.
fn braced(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if chars.next()? != '{' {
        return None;
    }
    let mut digits = String::new();
    loop {
        match chars.next()? {
            '}' => break,
            c if c.is_ascii_digit() || ('A'..='F').contains(&c) => digits.push(c),
            _ => return None,
        }
    }
    Some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole property, over the corpus most likely to break it.
    #[test]
    fn encode_is_injective_because_decode_inverts_it() {
        let mut cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"notes.txt".to_vec(),
            "résumé.pdf".as_bytes().to_vec(),
            "отчёт.pdf".as_bytes().to_vec(),
            "论文.txt".as_bytes().to_vec(),
            "rеsume.pdf".as_bytes().to_vec(), // Cyrillic e
            // The self-collision pair: these must not encode alike.
            "é.pdf".as_bytes().to_vec(),
            b"\\u{00E9}.pdf".to_vec(),
            // Sigils in every arrangement.
            b"\\".to_vec(),
            b"\\\\".to_vec(),
            b"a\\b".to_vec(),
            b"\\x{41}".to_vec(),
            // Invalid UTF-8.
            vec![0xFF, 0xFE],
            vec![b'a', 0x80, b'b'],
            vec![0xE2, 0x28, 0xA1],
            // Denied classes.
            "a\u{200B}b".as_bytes().to_vec(),
            "\u{202E}txt.exe".as_bytes().to_vec(),
            "a\u{0301}".as_bytes().to_vec(),
            "\u{00A0}".as_bytes().to_vec(),
            ";\u{037E}".as_bytes().to_vec(),
        ];
        // A deterministic sweep over arbitrary bytes: no dependency, and a
        // fixed seed so a failure is reproducible from the test name alone.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..4000 {
            let mut next = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let len = (next() % 24) as usize;
            cases.push((0..len).map(|_| (next() % 256) as u8).collect());
        }

        for name in &cases {
            let t = encode(name);
            assert_eq!(
                decode(&t.text).as_ref(),
                Some(name),
                "encode is not invertible for {name:?} -> {:?}",
                t.text
            );
        }

        // Injectivity restated directly, so a broken `decode` that happens to
        // be self-consistent cannot satisfy this test on its own.
        let mut seen: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
        for name in &cases {
            let text = encode(name).text;
            if let Some(prev) = seen.insert(text.clone(), name.clone()) {
                assert_eq!(
                    &prev, name,
                    "two distinct filenames produced the same transcript {text:?}"
                );
            }
        }
    }

    /// The pair the doubling rule exists for.
    #[test]
    fn a_filename_containing_the_escape_syntax_does_not_collide_with_it() {
        // `é` is drawable and `é.pdf` is a pure-Latin word, so it is drawn as
        // itself — that is decision 2 working, not a missing escape. The
        // collision this test exists for is with the *literal* text.
        let real = encode("é.pdf".as_bytes()).text;
        let literal = encode(b"\\u{00E9}.pdf").text;
        assert_eq!(real, "é.pdf");
        assert_eq!(literal, "\\\\u{00E9}.pdf");
        assert_ne!(real, literal);

        // And the pair that really tests the doubling rule: a character that
        // *does* escape, against a filename spelling that escape literally.
        let escaped = encode("\u{05D0}.txt".as_bytes()).text; // Hebrew alef: denied
        let spelled = encode(b"\\u{05D0}.txt").text;
        assert_eq!(escaped, "\\u{05D0}.txt");
        assert_eq!(spelled, "\\\\u{05D0}.txt");
        assert_ne!(escaped, spelled);
    }

    /// A permitted name is drawn as itself; a refused one loses its minority.
    #[test]
    fn the_minority_script_is_what_escapes() {
        // Latin + Cyrillic is refused: the lone Cyrillic `е` escapes.
        let t = encode("rеsume.pdf".as_bytes());
        assert_eq!(t.text, "r\\u{0435}sume.pdf");
        assert_eq!(t.groups, BTreeSet::from([Group::Latin]));

        // A Cyrillic word with an ASCII extension is drawn whole. This is
        // the case a whole-name vote gets wrong: it would call the name
        // Latin+Cyrillic, refuse it, and escape one side or the other.
        let t = encode("отчёт.pdf".as_bytes());
        assert_eq!(t.text, "отчёт.pdf");
        // `groups` is the honest set of what is *on screen*, and `pdf` really
        // is Latin. Deciding that a label should name only the unexpected
        // scripts is the renderer's job, not this module's — recording the
        // truth here keeps that choice reviewable rather than baked in.
        assert_eq!(t.groups, BTreeSet::from([Group::Latin, Group::Cyrillic]));

        // The same shape in Japanese, which is the case decision 3 bought the
        // font asset for.
        let t = encode("論文2026.docx".as_bytes());
        assert_eq!(
            t.text, "\\u{8AD6}\\u{6587}2026.docx",
            "kana and Han transcribe until the atlas lands, but the *word* \
             structure is already right: the extension survives"
        );
    }

    /// An escaped character does not contribute its script to the label: the
    /// human cannot see a script that was not drawn.
    #[test]
    fn an_escaped_script_is_not_named_beside_the_row() {
        let t = encode("rеsume.pdf".as_bytes());
        assert!(
            !t.groups.contains(&Group::Cyrillic),
            "the Cyrillic character was escaped, so naming Cyrillic would \
             describe something not on screen"
        );
    }

    /// Runs partition the text exactly, so a tint cannot leave a gap.
    #[test]
    fn runs_tile_the_transcript_without_gap_or_overlap() {
        for name in [
            "résumé.pdf".as_bytes(),
            b"plain.txt",
            b"a\\b",
            "rеsume.pdf".as_bytes(),
        ] {
            let t = encode(name);
            let mut at = 0usize;
            for (range, _) in &t.runs {
                assert_eq!(range.start, at, "runs must tile {:?}", t.text);
                at = range.end;
            }
            assert_eq!(at, t.text.len(), "runs must cover all of {:?}", t.text);
            assert!(
                t.runs.windows(2).all(|w| w[0].1 != w[1].1),
                "adjacent runs of one class must be merged in {:?}",
                t.text
            );
        }
    }
}
