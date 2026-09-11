// SPDX-License-Identifier: MPL-2.0
//! One directory listing, in which no two rows may look alike.
//!
//! # Why a per-listing rule and not a global one
//!
//! Global pixel-injectivity is impossible and it is worth saying so plainly
//! rather than implying it. A row is a bounded raster — finitely many possible
//! images — while the set of legal 255-byte filenames is not remotely finite.
//! Some pair must collide.
//!
//! What is achievable is the property that actually protects the human: **no
//! two rows of the listing in front of them render alike.** Two files in
//! different directories rendering alike is not a confusion reachable inside
//! one designation, because the human never sees them side by side and the
//! descriptor is minted from the row they touched. That boundary is published
//! rather than left implicit.
//!
//! [`crate::paint::transcript`] holds the other half — that distinct bytes produce
//! distinct *text* — and that one is global and proven. This module handles
//! what survives it: two distinct transcripts that happen to rasterize alike.
//!
//! # The gutter, and why a suffix would not work
//!
//! When two rows collide, they are told apart by a mark. The obvious mark is a
//! suffix on the name — and it is wrong, because every truncation primitive in
//! this tree cuts from the **end**. The rows that collide are exactly the ones
//! sharing a long prefix, so they are exactly the ones long enough to be
//! elided, and the mark would be the first thing elided away. The repair would
//! silently undo itself on the only rows that need it.
//!
//! So the mark lives in a **reserved gutter**: a fixed-width column the name
//! field can never grow into, laid out beside the name rather than inside it.
//! Elision shortens the name; it cannot reach the gutter.
//!
//! # Why the rasterizer is injected
//!
//! [`build`] takes the thing that turns a row into pixels rather than calling
//! it directly. That is not indirection for its own sake: real collisions
//! between real filenames in one real directory are rare and font-dependent,
//! so a test that waited for one would be testing almost nothing. With the
//! rasterizer injected, a test **constructs** the collision it wants to see
//! the repair handle, including the pathological cases — a whole directory of
//! rows that render alike, and a group larger than the gutter can label.

#![allow(dead_code)]

use crate::paint::transcript::{encode, Transcript};
use std::collections::HashMap;

/// How many characters the gutter reserves for a disambiguating mark.
///
/// Four is `#` plus up to three digits, so a collision group of up to 999 rows
/// can be labelled. A group larger than that refuses the listing rather than
/// wrapping, which is a condition no real directory reaches and which must not
/// silently produce two identical marks if one ever does.
pub(crate) const GUTTER_CHARS: usize = 4;

/// The largest collision group the gutter can label.
const MAX_TAG: usize = 999;

/// A row's rendered identity: the digest of exactly the pixels the human sees
/// in the name field, after elision.
///
/// A digest rather than the pixels themselves because a listing can be large —
/// a directory of a thousand entries would otherwise hold tens of megabytes of
/// raster — and blake3 is already in this crate's graph for the golden
/// harness, so nothing new enters the trusted core for this.
pub(crate) type RowDigest = [u8; 32];

/// One prepared row.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    /// The original bytes. The descriptor is minted from this, never from the
    /// drawn text — what the human touched is a row, and the row remembers
    /// which name it stands for.
    pub(crate) name: Vec<u8>,
    /// What is drawn in the name field.
    pub(crate) transcript: Transcript,
    /// The mark drawn in the gutter, when this row shares its pixels with
    /// another. `None` on the overwhelming majority of rows.
    pub(crate) tag: Option<String>,
}

/// A listing whose rows are pairwise distinguishable.
#[derive(Clone, Debug)]
pub(crate) struct Listing {
    pub(crate) rows: Vec<Row>,
}

/// Why a listing could not be made distinguishable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListingError {
    /// More rows render alike than the gutter can label. Refusing is the only
    /// honest answer: drawing them would show the human two rows it claims are
    /// different and cannot tell apart.
    Unseparable { alike: usize },
}

/// Build a listing, repairing collisions in the gutter.
///
/// `rasterize` is handed the text that will actually be drawn in the name
/// field — already elided by the caller's own layout — and returns the digest
/// of the pixels it produces. It must be a pure function of that text: a
/// rasterizer that varied with row position would make the digests
/// meaningless.
pub(crate) fn build(
    names: &[Vec<u8>],
    mut rasterize: impl FnMut(&Transcript) -> RowDigest,
) -> Result<Listing, ListingError> {
    let mut rows: Vec<Row> = names
        .iter()
        .map(|name| Row {
            name: name.clone(),
            transcript: encode(name),
            tag: None,
        })
        .collect();

    // Group by what the human will actually see.
    let mut groups: HashMap<RowDigest, Vec<usize>> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        groups
            .entry(rasterize(&row.transcript))
            .or_default()
            .push(i);
    }

    // Deterministic order: a listing must not depend on hash iteration order,
    // or the same directory would label its rows differently between runs and
    // a human comparing two sessions would see marks move.
    let mut collided: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
    collided.sort_by_key(|g| g[0]);

    for group in collided {
        if group.len() > MAX_TAG {
            return Err(ListingError::Unseparable { alike: group.len() });
        }
        for (n, &i) in group.iter().enumerate() {
            rows[i].tag = Some(format!("#{}", n + 1));
        }
    }

    Ok(Listing { rows })
}

impl Listing {
    /// Rows matching `query`, as indices into [`Listing::rows`].
    ///
    /// **No collision re-check happens here, and that is a theorem rather than
    /// an optimization.** Filtering only removes rows, and pairwise
    /// distinctness is inherited by every subset — so a filtered view of a
    /// distinguishable listing is distinguishable. Re-checking on each
    /// keystroke would cost a full re-rasterization of the directory per
    /// character typed, to prove something already proven.
    pub(crate) fn filter(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let needle = query.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.transcript.text.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Digest a transcript by its text, so a test can decide exactly which
    /// rows collide. Rows collide iff their drawn text is equal.
    fn by_text(t: &Transcript) -> RowDigest {
        *blake3::hash(t.text.as_bytes()).as_bytes()
    }

    /// Collide rows whose text shares its first `n` characters, standing in
    /// for the elision that makes long shared-prefix names render alike.
    fn by_prefix(n: usize) -> impl FnMut(&Transcript) -> RowDigest {
        move |t: &Transcript| {
            let cut: String = t.text.chars().take(n).collect();
            *blake3::hash(cut.as_bytes()).as_bytes()
        }
    }

    #[test]
    fn an_ordinary_listing_needs_no_marks() {
        let names: Vec<Vec<u8>> = ["notes.txt", "report.pdf", "photo.jpg"]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let listing = build(&names, by_text).expect("nothing collides");
        assert!(
            listing.rows.iter().all(|r| r.tag.is_none()),
            "a listing with no collisions must draw no marks: the gutter is \
             empty on the overwhelming majority of real directories"
        );
    }

    /// **The case a suffix would have got wrong.** These two names differ only
    /// in their last character, so any elision that shortens them makes them
    /// render alike — and an appended mark would be elided away with the
    /// difference it exists to restore.
    #[test]
    fn rows_that_elide_alike_are_marked_in_the_gutter() {
        let names: Vec<Vec<u8>> = vec![
            b"IMG_20260909_120001.jpg".to_vec(),
            b"IMG_20260909_120002.jpg".to_vec(),
            b"unrelated.txt".to_vec(),
        ];
        // Elide to a width that cuts before the digits diverge.
        let listing = build(&names, by_prefix(12)).expect("two rows, one gutter");
        assert_eq!(listing.rows[0].tag.as_deref(), Some("#1"));
        assert_eq!(listing.rows[1].tag.as_deref(), Some("#2"));
        assert_eq!(
            listing.rows[2].tag, None,
            "a row that collides with nothing is not marked just because \
             another pair did"
        );
    }

    /// The marks are stable: the same directory labels the same rows the same
    /// way every time, so a human is not shown marks that move between runs.
    #[test]
    fn marks_do_not_depend_on_iteration_order() {
        let names: Vec<Vec<u8>> = (0..40)
            .map(|i| format!("shared_prefix_{i:03}.txt").into_bytes())
            .collect();
        let first = build(&names, by_prefix(10)).expect("marked");
        for _ in 0..8 {
            let again = build(&names, by_prefix(10)).expect("marked");
            let a: Vec<_> = first.rows.iter().map(|r| r.tag.clone()).collect();
            let b: Vec<_> = again.rows.iter().map(|r| r.tag.clone()).collect();
            assert_eq!(a, b, "the same listing must label the same rows alike");
        }
    }

    /// A group the gutter cannot label refuses the listing rather than drawing
    /// two rows it cannot tell apart.
    #[test]
    fn a_group_larger_than_the_gutter_refuses_the_listing() {
        let names: Vec<Vec<u8>> = (0..MAX_TAG + 1)
            .map(|i| format!("x{i}").into_bytes())
            .collect();
        // Everything renders alike.
        let err = build(&names, |_| [0u8; 32]).expect_err("must refuse");
        assert_eq!(err, ListingError::Unseparable { alike: MAX_TAG + 1 });
    }

    /// Filtering cannot reintroduce a collision, because a subset of a
    /// pairwise-distinct set is pairwise distinct.
    #[test]
    fn a_filtered_view_is_still_distinguishable() {
        let names: Vec<Vec<u8>> = vec![
            b"alpha.txt".to_vec(),
            b"alpha.log".to_vec(),
            b"beta.txt".to_vec(),
        ];
        let listing = build(&names, by_text).expect("distinct");
        let shown = listing.filter("alpha");
        assert_eq!(shown, vec![0, 1]);

        let mut seen: Vec<(RowDigest, Option<String>)> = Vec::new();
        for &i in &shown {
            let key = (
                by_text(&listing.rows[i].transcript),
                listing.rows[i].tag.clone(),
            );
            assert!(
                !seen.contains(&key),
                "filtering produced two indistinguishable rows, which cannot \
                 happen if the full listing was distinguishable"
            );
            seen.push(key);
        }
    }

    /// The row remembers the bytes, not the drawing. The descriptor is minted
    /// from the name, so a transcribed row must still designate the real file.
    #[test]
    fn a_row_carries_the_original_bytes_not_the_drawn_text() {
        let name = "rеsume.pdf".as_bytes().to_vec(); // Cyrillic e
        let listing = build(std::slice::from_ref(&name), by_text).expect("one row");
        assert_eq!(listing.rows[0].name, name);
        assert_ne!(
            listing.rows[0].transcript.text.as_bytes(),
            name.as_slice(),
            "this row is transcribed, so the drawn text differs from the bytes \
             — which is exactly why the bytes are kept"
        );
    }
}
