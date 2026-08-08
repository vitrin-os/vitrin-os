// SPDX-License-Identifier: MPL-2.0
//! The core's pixel primitives: a borrowed RGBA8888 canvas and the handful of
//! operations every trusted surface is drawn from — rectangle fill, rectangle
//! stroke, whole-canvas darkening (the scrim), and a clipped opaque blit.
//!
//! **Integer math only, deliberately.** This is the same posture
//! [`crate::scene`] takes for the realm view ("integer math only, no
//! filtering, no renderer state") and it exists here for the same reason:
//! the P1.7.1 and WS-E.2.2 goldens assert exact pixels, and every floating-point
//! operation in a rasterizer is a chance for two machines to disagree in
//! the low bit. The only floating point anywhere in a core-drawn
//! surface is inside glyph rasterization ([`super::text`]), which fontdue does on one
//! scalar IEEE-754 path (see this crate's `Cargo.toml` for why its SIMD
//! feature is off) — everything that composites those glyph bitmaps, and
//! every shape around them, is `u32` arithmetic here.
//!
//! Not a general 2D library and not trying to become one: there are no
//! paths, no curves, no transforms, and no rounded corners. Rounded corners
//! in particular were considered and dropped — they need per-pixel coverage,
//! i.e. a path rasterizer, which is exactly the dependency [`super`] declines
//! to put in the TCB to make a dialog look softer.

use crate::scene::BYTES_PER_PIXEL;

/// An axis-aligned rectangle in canvas pixels. Signed origin so a rectangle
/// may start off-canvas (every operation clips); unsigned extent so an
/// "empty or backwards" rectangle is unrepresentable rather than silently
/// drawing nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// Whether `(px, py)` lies inside this rectangle — the hit test the
    /// P1.7.2 input grab runs against
    /// [`crate::consent::render::Card::buttons`]. Kept here, beside the
    /// geometry it tests, so the renderer and the decision wiring can never
    /// disagree about where a button is.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        // i64 offsets, so a point at `i32::MIN` against an origin at
        // `i32::MAX` subtracts without overflowing.
        let dx = i64::from(px) - i64::from(self.x);
        let dy = i64::from(py) - i64::from(self.y);
        dx >= 0 && dy >= 0 && dx < i64::from(self.w) && dy < i64::from(self.h)
    }
}

/// A borrowed, tightly packed RGBA8888 canvas, rows top-down — the same
/// byte layout [`crate::scene::Scene::compose`] produces, so the prompt can
/// be drawn straight onto a composed realm view without a conversion.
///
/// Borrowed rather than owning, because it is used for both halves of the
/// job: the card is drawn into a `Vec<u8>` this wraps, and the finished card
/// is composited onto the backend's view buffer through another one.
pub(crate) struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
}

impl<'a> Canvas<'a> {
    /// Wrap `pixels` as a `width x height` canvas.
    ///
    /// Returns `None` when the buffer is not exactly `width * height * 4`
    /// bytes. A mismatch is a core bug (every caller owns both the buffer
    /// and the dimensions), but this is a trusted core: it refuses and lets
    /// the caller skip drawing rather than panicking inside a compositor
    /// loop or, worse, indexing past the slice. 128-bit math so no product
    /// of two `u32` dimensions can wrap the check.
    pub fn new(pixels: &'a mut [u8], width: u32, height: u32) -> Option<Self> {
        let expected = u128::from(width) * u128::from(height) * BYTES_PER_PIXEL as u128;
        if pixels.len() as u128 != expected {
            return None;
        }
        Some(Self {
            pixels,
            width,
            height,
        })
    }

    /// Source-over one channel: `src` at `alpha` over `dst`.
    ///
    /// `(v + 127) / 255` is a true rounding divide, not the `(v + 1 + (v >>
    /// 8)) >> 8` approximation the fast-blit literature usually reaches for.
    /// The prompt is a few hundred thousand pixels drawn at most once per
    /// prompt (the raster is cached — see [`super::ConsentSurface`]), so
    /// there is nothing to buy with an approximation, and an exact divide is
    /// one less thing standing between the golden and its bytes.
    fn over(src: u8, dst: u8, alpha: u8) -> u8 {
        let a = u32::from(alpha);
        let v = u32::from(src) * a + u32::from(dst) * (255 - a);
        ((v + 127) / 255) as u8
    }

    /// Clip `rect` to the canvas, yielding inclusive-exclusive pixel bounds,
    /// or `None` when nothing of it is on-canvas. The single clip
    /// implementation every operation below goes through, so no drawing
    /// primitive can grow its own off-by-one.
    fn clip(&self, rect: Rect) -> Option<(u32, u32, u32, u32)> {
        // i64 throughout: `x + w` must not wrap for an off-canvas rectangle
        // with an extreme origin.
        let x0 = i64::from(rect.x).max(0);
        let y0 = i64::from(rect.y).max(0);
        let x1 = (i64::from(rect.x) + i64::from(rect.w)).min(i64::from(self.width));
        let y1 = (i64::from(rect.y) + i64::from(rect.h)).min(i64::from(self.height));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
    }

    /// Blend one pixel: `rgb` at `alpha` over whatever is there. The
    /// destination alpha byte is left at `0xFF` — every canvas the consent
    /// surface draws on is opaque (the card is initialized opaque, the realm
    /// view is opaque by [`crate::scene::Scene::compose`]'s contract), so
    /// there is no transparency to accumulate and nothing downstream ever
    /// reads a partially transparent result.
    pub fn blend_pixel(&mut self, x: i32, y: i32, rgb: [u8; 3], alpha: u8) {
        if alpha == 0 {
            return;
        }
        let Some((x0, y0, _, _)) = self.clip(Rect { x, y, w: 1, h: 1 }) else {
            return;
        };
        let off = (y0 as usize * self.width as usize + x0 as usize) * BYTES_PER_PIXEL;
        for (c, src) in rgb.iter().enumerate() {
            self.pixels[off + c] = Self::over(*src, self.pixels[off + c], alpha);
        }
        self.pixels[off + 3] = 0xff;
    }

    /// Fill `rect` with `rgba`, honoring its alpha byte.
    pub fn fill_rect(&mut self, rect: Rect, rgba: [u8; 4]) {
        let Some((x0, y0, x1, y1)) = self.clip(rect) else {
            return;
        };
        if rgba[3] == 0 {
            return;
        }
        for y in y0..y1 {
            let row = y as usize * self.width as usize;
            for x in x0..x1 {
                let off = (row + x as usize) * BYTES_PER_PIXEL;
                for (c, src) in rgba.iter().take(3).enumerate() {
                    self.pixels[off + c] = Self::over(*src, self.pixels[off + c], rgba[3]);
                }
                self.pixels[off + 3] = 0xff;
            }
        }
    }

    /// `base + delta` in canvas coordinates, saturating at the `i32` domain
    /// rather than wrapping. Only reachable with an absurd rectangle (an
    /// origin at `i32::MAX`); a saturated coordinate is off-canvas, so
    /// [`Self::clip`] then discards it, which is the same outcome the honest
    /// arithmetic would have produced.
    fn offset(base: i32, delta: u32) -> i32 {
        (i64::from(base) + i64::from(delta)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }

    /// Stroke `rect`'s outline `thickness` pixels wide, drawn **inside** the
    /// rectangle (so a stroked rectangle never grows past the geometry the
    /// layout reserved for it). A thickness at or past half the extent
    /// degenerates to a filled rectangle, which is what "the border ate the
    /// box" should look like rather than an artifact.
    pub fn stroke_rect(&mut self, rect: Rect, rgba: [u8; 4], thickness: u32) {
        if thickness == 0 {
            return;
        }
        let t = thickness.min(rect.w).min(rect.h);
        // Top and bottom bars span the full width; the side bars fill only
        // what is left between them, so no pixel is blended twice (which,
        // with a translucent stroke color, would be visible).
        self.fill_rect(Rect { h: t, ..rect }, rgba);
        self.fill_rect(
            Rect {
                y: Self::offset(rect.y, rect.h - t),
                h: t,
                ..rect
            },
            rgba,
        );
        let middle = rect.h.saturating_sub(2 * t);
        if middle > 0 {
            let y = Self::offset(rect.y, t);
            self.fill_rect(
                Rect {
                    x: rect.x,
                    y,
                    w: t,
                    h: middle,
                },
                rgba,
            );
            self.fill_rect(
                Rect {
                    x: Self::offset(rect.x, rect.w - t),
                    y,
                    w: t,
                    h: middle,
                },
                rgba,
            );
        }
    }

    /// Darken the whole canvas toward black by `alpha` — the prompt's scrim.
    ///
    /// The scrim is not decoration: it is what makes the realm view behind
    /// the prompt legible *as* background, so a human cannot mistake app
    /// content for part of the dialog. It is applied to the human-visible
    /// output only, like everything else in this module.
    pub fn darken(&mut self, alpha: u8) {
        if alpha == 0 {
            return;
        }
        // A 256-entry table rather than a divide per channel: the scrim
        // covers the whole view (a megapixel at the default `--size`), and
        // the table is built from the same [`Self::over`] the rest of the
        // module blends with, so it is a speedup with no second definition
        // of the blend and no change to a single output byte.
        let table: [u8; 256] = std::array::from_fn(|dst| Self::over(0, dst as u8, alpha));
        for px in self.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
            for c in px.iter_mut().take(3) {
                *c = table[*c as usize];
            }
            px[3] = 0xff;
        }
    }

    /// Copy an **opaque** `sw x sh` RGBA source onto this canvas with its
    /// top-left at `(dx, dy)`, clipped. A row-wise `copy_from_slice`, the
    /// same move [`crate::scene::Scene::compose`] makes for client content
    /// and for the same reason: the source is known opaque, so blending
    /// would compute the source back out of itself at a cost.
    ///
    /// "Known opaque" is a real precondition, not a hope: the card is
    /// initialized to an opaque background before anything is drawn on it,
    /// and every primitive above pins the destination alpha to `0xFF`.
    /// [`super::render`]'s tests assert it directly.
    pub fn blit_opaque(&mut self, src: &[u8], sw: u32, sh: u32, dx: i32, dy: i32) {
        if src.len() as u128 != u128::from(sw) * u128::from(sh) * BYTES_PER_PIXEL as u128 {
            return;
        }
        let Some((x0, y0, x1, y1)) = self.clip(Rect {
            x: dx,
            y: dy,
            w: sw,
            h: sh,
        }) else {
            return;
        };
        // Where the visible part starts inside the source (nonzero only when
        // the card hangs off the top/left edge of a view smaller than it).
        let src_x = (x0 as i64 - i64::from(dx)) as u32;
        let src_y = (y0 as i64 - i64::from(dy)) as u32;
        let run = (x1 - x0) as usize * BYTES_PER_PIXEL;
        for row in 0..(y1 - y0) {
            let s = ((src_y + row) as usize * sw as usize + src_x as usize) * BYTES_PER_PIXEL;
            let d = ((y0 + row) as usize * self.width as usize + x0 as usize) * BYTES_PER_PIXEL;
            self.pixels[d..d + run].copy_from_slice(&src[s..s + run]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w x h` opaque black buffer.
    fn buffer(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; w as usize * h as usize * BYTES_PER_PIXEL];
        for px in v.chunks_exact_mut(BYTES_PER_PIXEL) {
            px[3] = 0xff;
        }
        v
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let off = (y as usize * w as usize + x as usize) * BYTES_PER_PIXEL;
        buf[off..off + BYTES_PER_PIXEL].try_into().unwrap()
    }

    #[test]
    fn canvas_rejects_a_mis_sized_buffer() {
        let mut short = vec![0u8; 4 * 3];
        assert!(Canvas::new(&mut short, 2, 2).is_none());
        let mut long = vec![0u8; 4 * 5];
        assert!(Canvas::new(&mut long, 2, 2).is_none());
        let mut exact = vec![0u8; 4 * 4];
        assert!(Canvas::new(&mut exact, 2, 2).is_some());
        // A degenerate size is representable and simply has no pixels.
        let mut empty: Vec<u8> = Vec::new();
        assert!(Canvas::new(&mut empty, 0, 7).is_some());
    }

    #[test]
    fn source_over_is_exact_at_the_endpoints() {
        // The two blends every drawing operation ultimately reduces to.
        assert_eq!(Canvas::over(0xaa, 0x11, 0xff), 0xaa, "alpha 255 is a copy");
        assert_eq!(Canvas::over(0xaa, 0x11, 0x00), 0x11, "alpha 0 is a no-op");
        // Half coverage of white over black rounds to the midpoint, not to
        // 127 or 128 by accident of integer truncation.
        assert_eq!(Canvas::over(0xff, 0x00, 0x80), 0x80);
    }

    #[test]
    fn drawing_clips_instead_of_panicking_or_wrapping() {
        // Every primitive must survive geometry that is entirely, partly, or
        // absurdly off-canvas: the prompt is centered in a view whose size
        // is operator-controlled (`--size`) and may be smaller than the card.
        let mut buf = buffer(8, 8);
        let mut canvas = Canvas::new(&mut buf, 8, 8).unwrap();
        let far = [
            Rect {
                x: -100,
                y: -100,
                w: 4,
                h: 4,
            },
            Rect {
                x: 100,
                y: 100,
                w: 4,
                h: 4,
            },
            Rect {
                x: i32::MIN,
                y: i32::MIN,
                w: u32::MAX,
                h: u32::MAX,
            },
            Rect {
                x: i32::MAX,
                y: i32::MAX,
                w: u32::MAX,
                h: u32::MAX,
            },
        ];
        for rect in far {
            canvas.fill_rect(rect, [0xff, 0, 0, 0xff]);
            canvas.stroke_rect(rect, [0xff, 0, 0, 0xff], 3);
        }
        canvas.blend_pixel(-1, -1, [0xff, 0, 0], 0xff);
        canvas.blend_pixel(8, 8, [0xff, 0, 0], 0xff);
        canvas.blit_opaque(&buffer(4, 4), 4, 4, -100, -100);
        canvas.blit_opaque(&buffer(4, 4), 4, 4, 100, 100);
        // The i32::MIN rectangle covers the origin, so only assert that
        // nothing panicked and the buffer kept its shape.
        assert_eq!(buf.len(), 8 * 8 * BYTES_PER_PIXEL);
    }

    #[test]
    fn a_partly_offscreen_blit_lands_its_visible_part_at_the_right_offset() {
        // The case a naive implementation gets wrong: a source hanging off
        // the top-left must draw its *lower-right* part, not its origin.
        let mut src = buffer(4, 4);
        for y in 0..4u32 {
            for x in 0..4u32 {
                let off = (y as usize * 4 + x as usize) * BYTES_PER_PIXEL;
                src[off] = (x * 16 + y) as u8;
            }
        }
        let mut buf = buffer(4, 4);
        {
            let mut canvas = Canvas::new(&mut buf, 4, 4).unwrap();
            canvas.blit_opaque(&src, 4, 4, -2, -1);
        }
        // Destination (0,0) must hold source (2,1).
        assert_eq!(px(&buf, 4, 0, 0)[0], 2 * 16 + 1);
        assert_eq!(px(&buf, 4, 1, 2)[0], 3 * 16 + 3);
        // A mis-sized source is refused outright rather than read past.
        let before = buf.clone();
        {
            let mut canvas = Canvas::new(&mut buf, 4, 4).unwrap();
            canvas.blit_opaque(&[0u8; 3], 4, 4, 0, 0);
        }
        assert_eq!(buf, before);
    }

    #[test]
    fn stroke_stays_inside_and_paints_each_edge_pixel_once() {
        // Drawn inside the rectangle (the layout reserved exactly this box)
        // and with no double-blended corner, which a translucent stroke
        // would otherwise show as four darker squares.
        let mut buf = buffer(6, 6);
        let mut canvas = Canvas::new(&mut buf, 6, 6).unwrap();
        canvas.stroke_rect(
            Rect {
                x: 1,
                y: 1,
                w: 4,
                h: 4,
            },
            [0xff, 0xff, 0xff, 0x80],
            1,
        );
        let edge = px(&buf, 6, 1, 1);
        assert_eq!(edge, px(&buf, 6, 4, 4), "corners blend exactly once");
        assert_eq!(edge, px(&buf, 6, 3, 1), "as does every other edge pixel");
        assert_eq!(px(&buf, 6, 2, 2), [0, 0, 0, 0xff], "interior untouched");
        assert_eq!(px(&buf, 6, 0, 0), [0, 0, 0, 0xff], "outside untouched");
    }

    #[test]
    fn darkening_is_uniform_monotonic_and_keeps_the_canvas_opaque() {
        let mut buf = buffer(3, 3);
        for px in buf.chunks_exact_mut(BYTES_PER_PIXEL) {
            px[..3].copy_from_slice(&[0xc8, 0x64, 0x20]);
        }
        let mut canvas = Canvas::new(&mut buf, 3, 3).unwrap();
        canvas.darken(0x80);
        for pixel in buf.chunks_exact(BYTES_PER_PIXEL) {
            assert!(pixel[0] < 0xc8 && pixel[1] < 0x64 && pixel[2] < 0x20);
            assert_eq!(pixel[3], 0xff, "the scrim must not make the view alpha");
        }
        // Uniform: every pixel of a uniform canvas darkens identically.
        assert_eq!(px(&buf, 3, 0, 0), px(&buf, 3, 2, 2));
    }

    #[test]
    fn rect_contains_is_half_open_on_both_axes() {
        // The P1.7.2 hit test: a click on the right or bottom edge belongs
        // to the *next* box, so adjacent buttons cannot both claim a pixel.
        let r = Rect {
            x: 10,
            y: 20,
            w: 4,
            h: 6,
        };
        assert!(r.contains(10, 20) && r.contains(13, 25));
        assert!(!r.contains(14, 25) && !r.contains(13, 26));
        assert!(!r.contains(9, 20) && !r.contains(10, 19));
    }
}
