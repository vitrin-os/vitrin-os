# Bundled font — Liberation Sans Regular

`LiberationSans-Regular.ttf` is vendored into this repository and embedded into
`vitrind` with `include_bytes!` (see `crates/vitrin-core/src/consent/text.rs`).
It is the **only** font the consent prompt (P1.7.1) will ever use.

## Why the font is vendored rather than loaded from the system

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

## Provenance

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

`crates/vitrin-core/src/consent/text.rs` re-states this hash and asserts the
embedded byte length at compile time, so a swapped font file fails the build
rather than silently moving the golden.

## License

SIL Open Font License, Version 1.1 — full text in `LICENSE-OFL-1.1.txt`, which
is the copy shipped with the font (one mangled em-dash on the line beginning
"or substituting" repaired to `--`, restoring the canonical OFL 1.1 wording).

OFL-1.1 is compatible with this repository's Apache-2.0 licensing: the OFL
governs the font file itself and permits bundling and embedding it in software
under any license, provided the font is not sold on its own and this license
text travels with it. The font is not "part of" the Apache-2.0 work; it is an
aggregated data file carrying its own terms, which is why the license lives here
beside it rather than being folded into the repository's root `LICENSE`.
