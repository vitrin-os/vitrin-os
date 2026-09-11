// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask kana-atlas [--check]` — the picker's Japanese glyph source and
//! the gate that holds it.
//!
//! # What the asset is
//!
//! `crates/vitrin-core/assets/fonts/kana-atlas-14px.bin` is a **pre-rasterized
//! atlas**: 3152 coverage bitmaps at 14 px, one per declared codepoint, with a
//! fixed-size index in front of them. It is not a font. It has no table
//! directory, no `cmap`, and — the property the picker's whole no-shaping
//! argument rests on — **no `GSUB` and no `GPOS`**, which here is not a claim
//! about how the renderer uses the file but about what fits inside it: this
//! check proves that every byte of the file is header, index record, or
//! coverage belonging to exactly one glyph, so there is nowhere to put a
//! shaping table. `crates/vitrin-core/src/paint/atlas.rs` argues the choice
//! against a subsetted font at length.
//!
//! # Why this checks properties rather than regenerating
//!
//! Regeneration needs the 19.5 MB source face (`NotoSansCJK-Regular.ttc`), and
//! neither a CI runner nor a fresh clone has one; `pyftsubset`/fontTools —
//! what a *subsetted font* would have needed — is not installed on the machine
//! this was authored on either. A provenance step CI cannot re-run is a
//! provenance step nobody checks, so the split is:
//!
//! * `cargo xtask kana-atlas` regenerates, on a machine that has the face. It
//!   drives `vitrin-core`'s own generator test with `VITRIN_REGEN_KANA_ATLAS=1`
//!   — the same shape `cargo xtask bless` uses for the pixel goldens, and for
//!   the same reason: one documented entry point, producing a reviewable diff.
//! * `cargo xtask kana-atlas --check` reads only what is checked in.
//!
//! # What `--check` holds, and what it does not
//!
//! Held here: the atlas parses, tiles exactly, is strictly ascending, stays
//! inside the declared Unicode blocks, covers **exactly** the codepoints
//! `kana-atlas.codepoints` declares (both directions), agrees with its own
//! `kana-atlas.provenance` record on every countable field, and matches the
//! `ATLAS_LEN` constant compiled into the core. `NOTICE` must name the asset,
//! because an unlicensed third-party-derived file in this tree is the one
//! failure here that no test in `vitrin-core` would ever notice.
//!
//! **Not held here: the digest.** blake3 is in `vitrin-core`'s dependency
//! graph and not in this crate's, and no new dependency was going to be spent
//! on restating it — so `the_embedded_atlas_is_the_file_its_provenance_names`
//! in `crates/vitrin-core/src/paint/atlas.rs` is what pins the bytes. Both run
//! in CI. Stated rather than left to be discovered, because "the checker did
//! not check the hash" is exactly the kind of gap this repository keeps
//! finding in its own gates.
//!
//! This module parses the atlas **independently** of the core's parser. That
//! is deliberate duplication: two readers of one format is how a format that
//! reads one way to its writer and another way to a reader gets caught.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// The three checked-in files, relative to the workspace root.
const ATLAS: &str = "crates/vitrin-core/assets/fonts/kana-atlas-14px.bin";
const CODEPOINTS: &str = "crates/vitrin-core/assets/fonts/kana-atlas.codepoints";
const PROVENANCE: &str = "crates/vitrin-core/assets/fonts/kana-atlas.provenance";
/// Where the core states the same length as a compile-time constant.
const CORE_MODULE: &str = "crates/vitrin-core/src/paint/atlas.rs";

const MAGIC: &[u8; 16] = b"VITRIN-ATLAS-01\n";
const HEADER_LEN: usize = 16 + 4 + 4 + 4;
const RECORD_LEN: usize = 4 + 4 + 2 + 2 + 2 + 2 + 4;
/// The size the atlas is rasterized at; `f32` bits, little-endian on the wire.
const PX: f32 = 14.0;

/// The blocks the alphabet may draw from — kana and the CJK Unified
/// Ideographs that JIS X 0208 level 1 reaches into. Restated here rather than
/// imported so this check does not agree with the core by construction.
const BLOCKS: [(u32, u32); 3] = [(0x3041, 0x309F), (0x30A0, 0x30FF), (0x4E00, 0x9F8D)];

/// One index record, decoded.
#[derive(Debug)]
struct Record {
    codepoint: u32,
    offset: usize,
    width: usize,
    height: usize,
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let slice = bytes
        .get(at..at + 4)
        .ok_or_else(|| anyhow::anyhow!("{ATLAS}: truncated at byte {at}"))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("4 bytes")))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16> {
    let slice = bytes
        .get(at..at + 2)
        .ok_or_else(|| anyhow::anyhow!("{ATLAS}: truncated at byte {at}"))?;
    Ok(u16::from_le_bytes(slice.try_into().expect("2 bytes")))
}

/// Parse the atlas into its records, failing on any structural violation.
/// Returns the records and the coverage-blob length.
fn parse(bytes: &[u8]) -> Result<(Vec<Record>, usize)> {
    if bytes.len() < HEADER_LEN || &bytes[..MAGIC.len()] != MAGIC {
        bail!(
            "{ATLAS}: not a VITRIN-ATLAS-01 file (magic mismatch). Regenerate with \
             `cargo xtask kana-atlas`."
        );
    }
    let px_bits = u32_at(bytes, MAGIC.len())?;
    if px_bits != PX.to_bits() {
        bail!(
            "{ATLAS}: rasterized at {} px, but the picker draws names at {PX} px",
            f32::from_bits(px_bits)
        );
    }
    let count = u32_at(bytes, MAGIC.len() + 4)? as usize;
    let blob_len = u32_at(bytes, MAGIC.len() + 8)? as usize;
    let index_len = count
        .checked_mul(RECORD_LEN)
        .ok_or_else(|| anyhow::anyhow!("{ATLAS}: declares {count} glyphs, which cannot fit"))?;
    let declared_len = HEADER_LEN
        .checked_add(index_len)
        .and_then(|n| n.checked_add(blob_len))
        .ok_or_else(|| anyhow::anyhow!("{ATLAS}: declared lengths overflow"))?;
    if declared_len != bytes.len() {
        bail!(
            "{ATLAS}: is {} bytes but its header accounts for {declared_len} ({count} glyphs, \
             {blob_len} coverage bytes). A byte the format does not account for is a byte a \
             shaping table could live in, which is the whole reason this is an atlas and not a \
             font.",
            bytes.len()
        );
    }
    let mut records = Vec::with_capacity(count);
    let mut expected_offset = 0usize;
    let mut previous: Option<u32> = None;
    for i in 0..count {
        let at = HEADER_LEN + i * RECORD_LEN;
        let record = Record {
            codepoint: u32_at(bytes, at)?,
            offset: u32_at(bytes, at + 4)? as usize,
            width: u16_at(bytes, at + 8)? as usize,
            height: u16_at(bytes, at + 10)? as usize,
        };
        if let Some(prev) = previous {
            if record.codepoint <= prev {
                bail!(
                    "{ATLAS}: record {i} is U+{:04X} after U+{prev:04X} — the index must be \
                     strictly ascending, or the core's binary search reads the wrong glyph",
                    record.codepoint
                );
            }
        }
        previous = Some(record.codepoint);
        if char::from_u32(record.codepoint).is_none() {
            bail!(
                "{ATLAS}: record {i} is U+{:04X}, not a Unicode scalar value",
                record.codepoint
            );
        }
        if !BLOCKS
            .iter()
            .any(|&(lo, hi)| (lo..=hi).contains(&record.codepoint))
        {
            bail!(
                "{ATLAS}: record {i} is U+{:04X}, outside the kana and JIS X 0208 level-1 \
                 blocks this alphabet is drawn from",
                record.codepoint
            );
        }
        if record.offset != expected_offset {
            bail!(
                "{ATLAS}: record {i} (U+{:04X}) starts its coverage at {} but the previous \
                 glyph ended at {expected_offset}. The blob must tile exactly: a gap is a byte \
                 no glyph owns.",
                record.codepoint,
                record.offset
            );
        }
        expected_offset += record.width * record.height;
        records.push(record);
    }
    if expected_offset != blob_len {
        bail!(
            "{ATLAS}: the coverage blob is {blob_len} bytes and the index accounts for \
             {expected_offset}"
        );
    }
    Ok((records, blob_len))
}

/// Parse the declared alphabet: `U+XXXX` per line, `#` comments, ascending and
/// unique. The ascending/unique demand is not cosmetic — the file is the
/// *declaration*, and a duplicate in it would make "covers exactly this set"
/// ambiguous.
fn parse_declared(text: &str) -> Result<Vec<u32>> {
    let mut out: Vec<u32> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let hex = line
            .strip_prefix("U+")
            .ok_or_else(|| anyhow::anyhow!("{CODEPOINTS}:{}: {line:?} is not `U+XXXX`", n + 1))?;
        let cp = u32::from_str_radix(hex, 16)
            .with_context(|| format!("{CODEPOINTS}:{}: {line:?}", n + 1))?;
        if let Some(&prev) = out.last() {
            if cp <= prev {
                bail!(
                    "{CODEPOINTS}:{}: U+{cp:04X} does not follow U+{prev:04X} — the declaration \
                     must be strictly ascending",
                    n + 1
                );
            }
        }
        out.push(cp);
    }
    if out.is_empty() {
        bail!("{CODEPOINTS}: declares no codepoints at all");
    }
    Ok(out)
}

/// One `key value` line of the provenance record.
fn provenance_field<'a>(text: &'a str, key: &str) -> Result<&'a str> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(' '))
        .ok_or_else(|| anyhow::anyhow!("{PROVENANCE}: states no `{key}`"))
}

/// Run the whole check against a workspace root. Reads only; writes nothing.
pub fn check(root: &Path) -> Result<String> {
    let atlas_path = root.join(ATLAS);
    let bytes =
        std::fs::read(&atlas_path).with_context(|| format!("reading {}", atlas_path.display()))?;
    let (records, blob_len) = parse(&bytes)?;

    let declared_text = std::fs::read_to_string(root.join(CODEPOINTS))
        .with_context(|| format!("reading {CODEPOINTS}"))?;
    let declared = parse_declared(&declared_text)?;

    // -- the atlas covers exactly the declared set, in both directions -------
    let in_atlas: BTreeSet<u32> = records.iter().map(|r| r.codepoint).collect();
    let in_file: BTreeSet<u32> = declared.iter().copied().collect();
    let show = |set: &BTreeSet<u32>| -> String {
        set.iter()
            .take(8)
            .map(|cp| format!("U+{cp:04X}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let missing: BTreeSet<u32> = in_file.difference(&in_atlas).copied().collect();
    let extra: BTreeSet<u32> = in_atlas.difference(&in_file).copied().collect();
    if !missing.is_empty() {
        bail!(
            "{ATLAS}: {} declared codepoints are not in the atlas (first: {}). A declared \
             character the picker cannot draw is a name that silently transcribes.",
            missing.len(),
            show(&missing)
        );
    }
    if !extra.is_empty() {
        bail!(
            "{ATLAS}: {} codepoints are in the atlas that {CODEPOINTS} does not declare (first: \
             {}). An undeclared glyph is one nobody decided to ship.",
            extra.len(),
            show(&extra)
        );
    }

    // -- the provenance record describes this file --------------------------
    let provenance = std::fs::read_to_string(root.join(PROVENANCE))
        .with_context(|| format!("reading {PROVENANCE}"))?;
    for (key, actual) in [
        ("format", "VITRIN-ATLAS-01".to_string()),
        ("px", format!("{PX}")),
        ("glyphs", records.len().to_string()),
        ("bytes", bytes.len().to_string()),
        ("coverage-bytes", blob_len.to_string()),
    ] {
        let stated = provenance_field(&provenance, key)?;
        if stated != actual {
            bail!("{PROVENANCE}: states `{key} {stated}`, but the atlas has `{actual}`");
        }
    }
    // The digest is stated here and checked in vitrin-core (module docs above).
    let digest = provenance_field(&provenance, "blake3")?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{PROVENANCE}: `blake3 {digest}` is not a 256-bit hex digest");
    }

    // -- the core's compile-time length constant names the same file --------
    let module = std::fs::read_to_string(root.join(CORE_MODULE))
        .with_context(|| format!("reading {CORE_MODULE}"))?;
    let stated_len = module
        .lines()
        .find_map(|l| l.trim().strip_prefix("const ATLAS_LEN: usize = "))
        .and_then(|l| {
            l.trim_end_matches(';')
                .replace('_', "")
                .parse::<usize>()
                .ok()
        })
        .ok_or_else(|| anyhow::anyhow!("{CORE_MODULE}: no `const ATLAS_LEN: usize = ...;`"))?;
    if stated_len != bytes.len() {
        bail!(
            "{CORE_MODULE}: ATLAS_LEN is {stated_len} but {ATLAS} is {} bytes. That constant is \
             a compile-time assert, so this is a build failure waiting to happen — update it \
             after regenerating.",
            bytes.len()
        );
    }

    // -- the licence map names the asset ------------------------------------
    // The one failure in this list that no test in vitrin-core could notice:
    // a third-party-derived file with no entry in the normative path -> licence
    // map is a licensing defect, not a rendering one.
    let notice = std::fs::read_to_string(root.join("NOTICE")).with_context(|| "reading NOTICE")?;
    if !notice.contains("kana-atlas") {
        bail!(
            "NOTICE does not name kana-atlas-14px.bin. It is derived from an OFL-1.1 face and \
             NOTICE is this repository's normative path -> licence map."
        );
    }

    Ok(format!(
        "kana-atlas: OK — {} glyphs, {} bytes ({blob_len} of coverage), every byte accounted \
         for, exactly the {} codepoints declared in {CODEPOINTS}. Digest checked by \
         vitrin-core's `the_embedded_atlas_is_the_file_its_provenance_names`.",
        records.len(),
        bytes.len(),
        declared.len(),
    ))
}

/// Regenerate the atlas by driving `vitrin-core`'s generator test.
///
/// Needs the source face; `VITRIN_KANA_ATLAS_SOURCE` overrides where it is
/// looked for. Leaves the tree deliberately red if the new atlas is a
/// different length than `ATLAS_LEN` says: a generator that quietly rewrote
/// its own tripwire would not be one.
pub fn regenerate(root: &Path) -> Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    eprintln!(
        "xtask kana-atlas: regenerating via `VITRIN_REGEN_KANA_ATLAS=1 cargo test -p \
         vitrin-core -- the_atlas_is_regenerated_only_when_asked`"
    );
    let status = std::process::Command::new(&cargo)
        .current_dir(root)
        .args([
            "test",
            "-p",
            "vitrin-core",
            "--",
            "the_atlas_is_regenerated_only_when_asked",
        ])
        .env("VITRIN_REGEN_KANA_ATLAS", "1")
        .status()
        .with_context(|| {
            format!(
                "spawning {} to regenerate the atlas",
                cargo.to_string_lossy()
            )
        })?;
    if !status.success() {
        bail!(
            "the generator failed ({status}). The usual cause is a missing source face: set \
             VITRIN_KANA_ATLAS_SOURCE to a copy of NotoSansCJK-Regular.ttc."
        );
    }
    eprintln!(
        "xtask kana-atlas: done. Update ATLAS_LEN in {CORE_MODULE} and the SHA-256 in the \
         README beside the asset, then review `git diff` before committing."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    /// The gate itself. Lives as a test as well as a subcommand so that
    /// `cargo test --workspace` — which CI already runs — fails on drift even
    /// before anyone wires the subcommand into a job. Same shape as
    /// `protocol_tables`.
    #[test]
    fn the_vendored_atlas_is_exactly_the_declared_alphabet() {
        match check(&root()) {
            Ok(report) => eprintln!("{report}"),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// Non-vacuity for the structural half: the three mutations the format is
    /// shaped to make impossible must each be caught, and by the message that
    /// names them.
    #[test]
    fn a_file_with_room_for_a_shaping_table_is_rejected() {
        let bytes = std::fs::read(root().join(ATLAS)).expect("the atlas");
        parse(&bytes).expect("the shipped atlas parses");

        let mut padded = bytes.clone();
        padded.push(0);
        let err = parse(&padded).expect_err("trailing bytes must be caught");
        assert!(
            format!("{err}").contains("does not account for"),
            "got {err}"
        );

        let mut gapped = bytes.clone();
        let at = HEADER_LEN + RECORD_LEN + 4;
        gapped[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        let err = parse(&gapped).expect_err("a gap in the blob must be caught");
        assert!(format!("{err}").contains("tile exactly"), "got {err}");

        let mut unsorted = bytes;
        let (a, b) = (HEADER_LEN, HEADER_LEN + RECORD_LEN);
        let first: [u8; 4] = unsorted[a..a + 4].try_into().expect("4 bytes");
        let second: [u8; 4] = unsorted[b..b + 4].try_into().expect("4 bytes");
        unsorted[a..a + 4].copy_from_slice(&second);
        unsorted[b..b + 4].copy_from_slice(&first);
        let err = parse(&unsorted).expect_err("an unsorted index must be caught");
        assert!(format!("{err}").contains("strictly ascending"), "got {err}");
    }

    /// Non-vacuity for the declaration half.
    #[test]
    fn a_declaration_that_is_not_a_strictly_ascending_list_is_rejected() {
        assert!(parse_declared("# only comments\n").is_err());
        assert!(parse_declared("U+3042\nU+3041\n").is_err(), "descending");
        assert!(parse_declared("U+3042\nU+3042\n").is_err(), "duplicate");
        assert!(parse_declared("3042\n").is_err(), "no U+ prefix");
        assert_eq!(
            parse_declared("# a comment\n\nU+3041\nU+3042\n").expect("parses"),
            vec![0x3041, 0x3042]
        );
    }
}
