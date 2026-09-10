# Bundled glyph sources

Two assets, both SIL OFL-1.1, both embedded into `vitrind` with `include_bytes!`:
the **vector face** every trusted surface draws Latin, Greek and Cyrillic with,
and the **kana atlas** the file picker draws Japanese with. Each has its own
license copy beside it.

---

## 1. Liberation Sans Regular — the vector face

`LiberationSans-Regular.ttf` is vendored into this repository and embedded into
`vitrind` with `include_bytes!` (see `crates/vitrin-core/src/paint/text.rs` —
that module was `crates/vitrin-core/src/consent/text.rs` until WS-E.2.2 moved
the rasterizer out of `consent::`, and this line said so until issue #190).
It is the **only** font the consent prompt (P1.7.1) will ever use.

### Why the font is vendored rather than loaded from the system

Three reasons, in order of how load-bearing they are:

1. **Golden determinism.** The consent-prompt golden asserts exact rasterized
   pixels. Anti-aliased glyph coverage is a function of the outline data, so a
   different font — or a different *version* of the same font — silently moves
   every text pixel. A system-font lookup would make the golden depend on which
   distribution the machine runs and which fontconfig rules it has, which is
   precisely the flakiness a golden exists to exclude. Vendoring pins the
   outlines to a byte-exact artifact this repository controls.
2. **The consent prompt is a TCB security surface.** It must render before, and
   independently of, anything outside the core. A prompt that cannot draw
   because fontconfig is misconfigured, or that draws through whatever font a
   user dropped into `~/.local/share/fonts`, is a consent surface an attacker
   has partial authorship of. Embedding removes the lookup entirely: there is no
   filesystem path to poison and no failure mode where the prompt has no font.
3. **No runtime dependency.** `include_bytes!` needs no font-discovery library
   (fontconfig, font-kit), which keeps the plan-risk-R7 dependency budget intact.

### Provenance

Byte-identical to the copy shipped by Arch Linux's `ttf-liberation` package
(upstream: <https://github.com/liberationfonts/liberation-fonts>), copied from
`/usr/share/fonts/liberation/LiberationSans-Regular.ttf`:

```
sha256  baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee
size    410820 bytes
```

The file is **unmodified**. That is deliberate: an unmodified copy is a
Modified Version under neither the OFL's definition nor its Reserved Font Name
clause, so bundling needs no renaming, and provenance stays checkable with a
single `sha256sum` against any distribution's copy. A subset font (ASCII only,
~20 KB instead of ~400 KB) was considered and rejected for exactly that reason:
it would be an OFL Modified Version requiring a rename away from the reserved
name "Liberation", and its provenance would rest on a subsetting toolchain that
is not in this repository and could not be re-run in CI.

`crates/vitrin-core/src/paint/text.rs` re-states this hash and asserts the
embedded byte length at compile time, so a swapped font file fails the build
rather than silently moving the golden.

### License

SIL Open Font License, Version 1.1 — full text in `LICENSE-OFL-1.1.txt`, which
is the copy shipped with the font (one mangled em-dash on the line beginning
"or substituting" repaired to `--`, restoring the canonical OFL 1.1 wording).

OFL-1.1 is compatible with this repository's Apache-2.0 licensing: the OFL
governs the font file itself and permits bundling and embedding it in software
under any license, provided the font is not sold on its own and this license
text travels with it. The font is not "part of" the Apache-2.0 work; it is an
aggregated data file carrying its own terms, which is why the license lives here
beside it rather than being folded into the repository's root `LICENSE`.

---

## 2. `kana-atlas-14px.bin` — the picker's Japanese glyphs

A **pre-rasterized atlas**, not a font: 3152 anti-aliased coverage bitmaps at
one size (14 px, `paint::text::NAME_PX`), with a fixed-size index in front of
them. `crates/vitrin-core/src/paint/atlas.rs` holds the format and the argument
for it; the short version is three properties a subsetted CJK font would not
have had:

1. **Nothing here is drawn at another size.** The picker draws filenames at
   exactly 14 px — the size `paint::script::DENIED_APPEARANCE`'s confusable
   closure is measured at — so an outline's scalability buys nothing.
2. **"No shaping" becomes a property of the file.** There is no table
   directory, so there is no `GSUB` and no `GPOS`; and because the format
   accounts for every byte (header, index, coverage — nothing else),
   `cargo xtask kana-atlas --check` can prove there is nowhere in the file to
   put one. With a font, "we never call the shaping tables" would be a promise
   about the renderer instead.
3. **No fontTools.** Subsetting means `pyftsubset`, which is not installed on
   the machine this was generated on and is not on a CI runner either. A
   provenance step CI cannot re-run is a provenance step nobody checks.

What it costs, said plainly: **643 KB**, larger than the 411 KB vector face,
and single-size. A subsetted font of the same 3152 glyphs was *not* measured —
there is no subsetting tool here to measure one with, which is reason 3 above —
so no size comparison is claimed in either direction.

### The alphabet

Declared in `kana-atlas.codepoints`, whose header states the rule and a
re-runnable stock-CPython derivation. In words: all assigned hiragana and
katakana (187 codepoints, the two **combining** sound marks U+3099/U+309A
deliberately excluded — this renderer has no mark positioning) plus the 2965
kanji of **JIS X 0208 level 1**. Level 2 is out; a kanji outside the set is
transcribed rather than drawn, which is a published limit and not a silent gap.

### Provenance

Rasterized from Arch Linux's `noto-fonts-cjk` 20240730-1,
`/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc`, **collection index 0**
(the face named `Noto Sans CJK JP`; the generator asserts that name, because a
collection can be reordered upstream without its file name changing):

```
source  sha256  b76b0433203017ca80401b2ee0dd69350349871c4b19d504c34dbdd80541690a
atlas   sha256  9ec970b47ed811b83030bc14442cca2ecaf24efba55dc6b5ff2a66b36660b4ea
atlas   size    658011 bytes, 3152 glyphs
```

The blake3 digests of both are in `kana-atlas.provenance`, which is what
`vitrin-core`'s `the_embedded_atlas_is_the_file_its_provenance_names` checks.
The SHA-256s above are here so the same files can be verified with `sha256sum`
and no Rust at all; nothing in this repository computes them.

### Regenerating

```sh
cargo xtask kana-atlas            # needs the source face; VITRIN_KANA_ATLAS_SOURCE overrides
cargo xtask kana-atlas --check    # what CI runs; needs nothing but this repository
```

Regeneration overwrites an atlas that already exists; **generating a first one
from nothing is a two-step bootstrap**, because `atlas.rs` embeds the file with
`include_bytes!` and asserts its length at compile time, so the crate does not
build while the file is absent or a different size. Put any parsable atlas
there, set `ATLAS_LEN` to its length, generate, then set `ATLAS_LEN` to the new
length. That is the price of the compile-time tripwire and it is paid once.

Regeneration deliberately leaves the tree red until `ATLAS_LEN` in
`crates/vitrin-core/src/paint/atlas.rs` is updated to the new length: that
constant is a compile-time assert, and a generator that quietly rewrote its own
tripwire would not be one.

### License

SIL Open Font License, Version 1.1 — the copy shipped with Noto CJK is
`LICENSE-OFL-1.1-NotoSansCJK.txt` (it declares **no Reserved Font Name**). The
atlas is derived from that Font Software, so it travels under the OFL with that
text beside it. Its name carries nothing Noto-shaped, which keeps the OFL's
naming clause satisfied whether or not one considers a bitmap atlas a "Modified
Version" — a question this repository does not need to answer, because the
conservative reading costs nothing here.
