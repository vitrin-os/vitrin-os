# Vitrin OS brand assets

**The code is free; these marks are not.** Everything in this directory is a
trademark of the Vitrin OS project, not a licensed artifact. The root
[`TRADEMARK.md`](../../TRADEMARK.md) governs their use and
[`NOTICE`](../../NOTICE) records that they sit outside the license map.
Nominative use — a screenshot, a talk slide, a blog post about the project,
including a critical one — needs no permission. Putting the mark on your own
build, product, or merchandise does.

## The files

| File | Use |
|---|---|
| `vitrin-mark.svg` | The mark alone, in accent. Default for square contexts. |
| `vitrin-mark-mono.svg` | Same geometry in `currentColor` — inherits surrounding text colour when inlined. |
| `vitrin-favicon.svg` | Wider gaps and a larger opening. **Use this below ~24 px**; the display mark's slits close up at favicon sizes. |
| `vitrin-lockup.svg` | Mark + wordmark, dark text — for light backgrounds. |
| `vitrin-lockup-dark.svg` | Mark + wordmark, light text — for dark backgrounds. Transparent, not a dark plate. |
| `social-card.svg` / `.png` | 1280×640 GitHub social preview and link-unfurl card. |
| `favicon-16.png`, `favicon-32.png`, `apple-touch-icon.png` | Rasterised from `vitrin-favicon.svg`. |
| `vitrin-mark-512.png` | Raster mark for contexts that reject SVG. |

All of them are build output. [`generate.py`](generate.py) is the source:

```sh
python3 assets/brand/generate.py     # needs rsvg-convert for the PNGs
```

Edit the geometry there, never the emitted SVG — a hand-edit is lost on the
next run.

## Palette

| Token | Value | Where it comes from |
|---|---|---|
| Accent | `#4D9DE0` | **Not an arbitrary brand colour.** It is `ACCENT` in [`crates/vitrin-core/src/consent/render.rs`](../../crates/vitrin-core/src/consent/render.rs) — the colour the trusted core paints its own consent card with. The brand colour is the trust-surface colour. |
| Ink | `#0C1116` | Dark backgrounds, wordmark on light. |
| Paper | `#FFFFFF` | Wordmark on dark. |

One caveat worth stating, because the project is careful about this
elsewhere: the accent is **not** the trusted-indicator colour. That one is
randomised per session on purpose — it is unspoofable precisely because it
is not a constant anyone can look up. Do not treat this palette as
security-relevant.

## The mark

A four-blade iris with a **square** opening.

Each blade is one straight inner edge — a side of the opening — plus an outer
arc. That construction is what reads as an aperture; a disc with slits cut
out of it reads as a broken pie chart, which is the shape the first drafts
had. The opening is rotated against the blade divisions (`TWIST`) to give
the pinwheel a real iris has, and gaps are specified as distances rather
than angles so a slit is the same width at the rim as at the opening.

The square is the departure from a camera aperture, and it is the whole
point: an aperture frames a **screen**. Scoped visibility onto a surface —
which is what the project does. Six blades and a hexagonal opening read as a
photography brand; four blades and a square read as a display.

## Using it

- Keep clear space of at least half the mark's radius on every side.
- Don't recolour the mark outside the palette above, rotate it, restretch it,
  or set the wordmark in a different typeface — the wordmark is drawn
  geometry, not type, so there is no font to substitute.
- Don't place the accent mark on a mid-tone background; it is tuned for ink
  or paper.
- The lockups are transparent. On a dark background use
  `vitrin-lockup-dark.svg` rather than putting the light lockup on a plate.
