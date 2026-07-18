//! Scene composition v0 (P1.3.3): the realm's client surface composited
//! into the **realm view** — the framebuffer that both presentation paths
//! present and that capture serves.
//!
//! In the MVP object model, one view = the realm's composited framebuffer
//! (PRD Doc 2 §3.1 "View", §9 "one virtual framebuffer per realm"). This
//! module is where that becomes real: [`Scene::compose`] is the **single
//! composition implementation** shared by both backends. The headless
//! backend blits its output 1:1 into the retained virtual-output
//! framebuffer (the exact bytes `capture_frame` reads back, P1.3.6); the
//! nested backend uploads the same output as its full-window texture. The
//! two paths therefore present identical composed content by construction
//! — there is no per-backend scene walk to drift.
//!
//! Composition is pure CPU byte assembly, deterministic and renderer-free:
//! a pure function of the committed surface content and the view size.
//! That keeps the pixel goldens exact (integer math only, no filtering, no
//! renderer state) and keeps this module trivially testable.
//!
//! # The surface-content seam (fed by P1.3.4, #21)
//!
//! [`Scene::commit`] / [`Scene::clear_surface`] are the seam the shim-facing
//! protocol server will drive: on a shim's `commit`, it copies the attached
//! shm/memfd buffer out of the client's fd (buffer path v0 = copy-in, plan
//! D3) and commits the bytes here, then triggers a backend redraw; on
//! surface loss (shim crash, P1.5.3) it clears. Until that server lands,
//! only tests drive the seam — deliberately the smallest honest interface:
//! plain bytes in, no protocol, no fds, no Wayland objects.
//!
//! # No surface committed → the deterministic test pattern
//!
//! An empty scene composes [`test_pattern::render`] exactly, byte for byte.
//! This keeps the P1.3.2/P1.3.6 capture goldens exact and gives nested mode
//! a recognizable "core is up, no client yet" picture. It is a *documented
//! deterministic background*, not decoration.
//!
//! # Mismatched-size surface → letterbox at 1:1, never scale (the P1.3.3
//! decision)
//!
//! A committed buffer whose size differs from the view is painted
//! **unscaled and centered** over a solid matte ([`LETTERBOX_RGBA`]),
//! center-cropped if larger than the view. Rationale, in order:
//!
//! 1. **No resampling in the TCB.** A capture must show the client's
//!    actual pixels: agents reason over exact bytes and the goldens assert
//!    them. Scaling means choosing a filter, and a filter choice is
//!    presentation policy — exactly what PRD Doc 2 §2 keeps out of the
//!    core (see [`layout`]).
//! 2. **Mismatch is transient by design.** Single-maximized means the shim
//!    is told the view size and commits matching buffers; a mismatch only
//!    occurs mid-resize or from a misbehaving shim. A dumb, obviously
//!    correct fallback beats a pretty one.
//! 3. **It is the simplest honest thing**: a row-wise copy.
//!
//! Client content is composited as **opaque** (alpha forced to `0xFF`):
//! the wire capture format is xrgb8888 with X pinned `0xFF`, and blending
//! against the matte would make composed bytes depend on blend math
//! instead of being the client's bytes.

pub(crate) mod layout;

use std::error::Error;
use std::fmt;

use crate::test_pattern;

/// Bytes per pixel of the composed view: tightly packed RGBA8888, rows
/// top-down — the same layout [`test_pattern::render`] produces and the
/// backends import (DRM fourcc `ABGR8888` on little-endian).
pub(crate) const BYTES_PER_PIXEL: usize = test_pattern::BYTES_PER_PIXEL;

/// The letterbox matte (RGBA): a dark neutral, deliberately matching the
/// nested backend's clear color (0.06, 0.06, 0.08) so the bars read as
/// "background", never as client content. Solid and opaque — deterministic
/// bytes for the goldens.
pub(crate) const LETTERBOX_RGBA: [u8; 4] = [0x0f, 0x0f, 0x14, 0xff];

/// A committed client buffer: tightly packed RGBA8888, rows top-down,
/// exactly `width * height * 4` bytes — validated at construction so a
/// mis-sized commit can never reach composition.
#[derive(Debug, Clone)]
pub(crate) struct SurfaceContent {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// A rejected [`SurfaceContent`]: zero dimensions or a byte length that is
/// not exactly `width * height * 4`. The shim-facing server (P1.3.4) maps
/// this to a protocol error at the commit site; composition itself never
/// sees an inconsistent buffer. Constructed only through [`from_rgba`]
/// (test-driven until P1.3.4 lands, like the rest of the seam).
///
/// [`from_rgba`]: SurfaceContent::from_rgba
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ContentSizeMismatch {
    pub width: u32,
    pub height: u32,
    pub got_len: usize,
}

impl fmt::Display for ContentSizeMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "surface content must have positive dimensions and exactly \
             width * height * 4 bytes: got {}x{} with {} bytes",
            self.width, self.height, self.got_len
        )
    }
}

impl Error for ContentSizeMismatch {}

impl SurfaceContent {
    /// Wrap client bytes as committable surface content. Zero dimensions
    /// are rejected (a surface with no pixels is [`Scene::clear_surface`],
    /// not a commit), as is any length mismatch (64-bit math, so a huge
    /// `width * height` cannot wrap the check). Test-driven until P1.3.4
    /// lands, like the rest of the seam.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, ContentSizeMismatch> {
        let expected = u64::from(width) * u64::from(height) * BYTES_PER_PIXEL as u64;
        if width == 0 || height == 0 || rgba.len() as u64 != expected {
            return Err(ContentSizeMismatch {
                width,
                height,
                got_len: rgba.len(),
            });
        }
        Ok(Self {
            rgba,
            width,
            height,
        })
    }
}

/// The realm's scene: at most one client surface in the MVP (single
/// maximized, plan P1.3.3). The realm object (P1.5.1) hangs off this; the
/// consent overlay (P1.7.1) later composites *above* the view this scene
/// produces.
pub(crate) struct Scene {
    surface: Option<SurfaceContent>,
    /// Bumped on every content change; presentation caches (the nested
    /// backend's texture) key on it to know when to re-upload.
    generation: u64,
}

impl Scene {
    /// An empty scene: composes the deterministic test-pattern background.
    pub fn new() -> Self {
        Self {
            surface: None,
            generation: 0,
        }
    }

    /// Commit new surface content — the seam P1.3.4's shim protocol server
    /// feeds after its shm copy-in. The caller is responsible for
    /// triggering a redraw. Test-driven until that server lands.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn commit(&mut self, content: SurfaceContent) {
        self.surface = Some(content);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Drop the committed surface (shim crash / surface destroyed, P1.5.3):
    /// the view falls back to the deterministic background. Test-driven
    /// until the shim server lands.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear_surface(&mut self) {
        self.surface = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// The content generation: changes exactly when composed output may
    /// have changed (for a fixed view size).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Compose the realm view at `width x height`: tightly packed RGBA8888,
    /// rows top-down, every pixel opaque. Pure and deterministic — same
    /// scene + same size = same bytes. Zero-sized views yield an empty
    /// buffer.
    ///
    /// This is the one composition implementation both backends present
    /// (module docs); everything below is byte assembly, no renderer.
    pub fn compose(&self, width: u32, height: u32) -> Vec<u8> {
        let Some(surface) = &self.surface else {
            // Empty scene: the documented deterministic background, byte
            // for byte — the P1.3.2/P1.3.6 goldens assert exactly this.
            return test_pattern::render(width, height);
        };

        let vw = width as usize;
        let vh = height as usize;
        let mut out = vec![0u8; vw * vh * BYTES_PER_PIXEL];
        for px in out.chunks_exact_mut(BYTES_PER_PIXEL) {
            px.copy_from_slice(&LETTERBOX_RGBA);
        }
        if out.is_empty() {
            return out;
        }

        // Single-maximized placement (the quarantined policy module), then
        // clip the placed rectangle to the view. With both the view and the
        // surface non-degenerate the overlap is always at least 1x1.
        let placement = layout::place((width, height), (surface.width, surface.height));
        let dst_x = placement.x.max(0) as usize;
        let dst_y = placement.y.max(0) as usize;
        let src_x = (-placement.x).max(0) as usize;
        let src_y = (-placement.y).max(0) as usize;
        let sw = surface.width as usize;
        let sh = surface.height as usize;
        let copy_w = (vw - dst_x).min(sw - src_x);
        let copy_h = (vh - dst_y).min(sh - src_y);

        for row in 0..copy_h {
            let src_off = ((src_y + row) * sw + src_x) * BYTES_PER_PIXEL;
            let dst_off = ((dst_y + row) * vw + dst_x) * BYTES_PER_PIXEL;
            let dst = &mut out[dst_off..dst_off + copy_w * BYTES_PER_PIXEL];
            dst.copy_from_slice(&surface.rgba[src_off..src_off + copy_w * BYTES_PER_PIXEL]);
            // Opaque composition (module docs): the client's alpha byte is
            // never presented or captured.
            for px in dst.chunks_exact_mut(BYTES_PER_PIXEL) {
                px[3] = 0xff;
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A deterministic synthetic client buffer, visibly distinct from the
    /// test pattern and from the matte: pure integer function of (x, y).
    pub(crate) fn client_pixels(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&[
                    (x % 251) as u8,
                    (y % 241) as u8,
                    ((x ^ y) % 239) as u8,
                    0xff,
                ]);
            }
        }
        out
    }

    /// The RGBA quadruple at `(x, y)` of a tightly packed `width`-wide
    /// buffer.
    fn px(buf: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let off = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
        buf[off..off + BYTES_PER_PIXEL].try_into().unwrap()
    }

    #[test]
    fn empty_scene_composes_the_test_pattern() {
        // The documented deterministic background: byte-exact, so the
        // pre-existing headless/capture goldens keep holding.
        assert_eq!(
            Scene::new().compose(1280, 800),
            test_pattern::render(1280, 800)
        );
        assert!(Scene::new().compose(0, 600).is_empty());
    }

    #[test]
    fn exact_size_surface_is_the_view_byte_for_byte() {
        // The steady state of single-maximized: the composed view IS the
        // client buffer — no matte visible, no pixel moved.
        let (w, h) = (64, 48);
        let pixels = client_pixels(w, h);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(pixels.clone(), w, h).unwrap());
        assert_eq!(scene.compose(w, h), pixels);
    }

    #[test]
    fn smaller_surface_is_letterboxed_centered_unscaled() {
        let (vw, vh) = (100, 80);
        let (sw, sh) = (40, 20);
        let pixels = client_pixels(sw, sh);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(pixels.clone(), sw, sh).unwrap());
        let out = scene.compose(vw, vh);

        // Matte in all four view corners.
        for (x, y) in [(0, 0), (vw - 1, 0), (0, vh - 1), (vw - 1, vh - 1)] {
            assert_eq!(px(&out, vw, x, y), LETTERBOX_RGBA);
        }
        // The client bytes sit centered at 1:1: every surface pixel is
        // exactly where the placement puts it, unresampled.
        let (ox, oy) = ((vw - sw) / 2, (vh - sh) / 2);
        for y in 0..sh {
            for x in 0..sw {
                assert_eq!(px(&out, vw, ox + x, oy + y), px(&pixels, sw, x, y));
            }
        }
        // Matte immediately outside the placed rectangle.
        assert_eq!(px(&out, vw, ox - 1, oy), LETTERBOX_RGBA);
        assert_eq!(px(&out, vw, ox + sw, oy), LETTERBOX_RGBA);
        assert_eq!(px(&out, vw, ox, oy - 1), LETTERBOX_RGBA);
        assert_eq!(px(&out, vw, ox, oy + sh), LETTERBOX_RGBA);
    }

    #[test]
    fn larger_surface_is_center_cropped_unscaled() {
        let (vw, vh) = (40, 30);
        let (sw, sh) = (60, 50);
        let pixels = client_pixels(sw, sh);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(pixels.clone(), sw, sh).unwrap());
        let out = scene.compose(vw, vh);

        // The view shows the surface's central vw x vh window, 1:1.
        let (cx, cy) = ((sw - vw) / 2, (sh - vh) / 2);
        for y in 0..vh {
            for x in 0..vw {
                assert_eq!(px(&out, vw, x, y), px(&pixels, sw, cx + x, cy + y));
            }
        }
    }

    #[test]
    fn client_alpha_is_forced_opaque() {
        // A translucent client buffer composites as opaque bytes: only the
        // alpha byte changes, the color bytes are the client's own.
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(vec![0x10, 0x20, 0x30, 0x00], 1, 1).unwrap());
        assert_eq!(scene.compose(1, 1), [0x10, 0x20, 0x30, 0xff]);
    }

    #[test]
    fn compose_is_deterministic() {
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(33, 17), 33, 17).unwrap());
        assert_eq!(scene.compose(101, 59), scene.compose(101, 59));
    }

    #[test]
    fn clear_surface_restores_the_background() {
        let mut scene = Scene::new();
        let g0 = scene.generation();
        scene.commit(SurfaceContent::from_rgba(client_pixels(8, 8), 8, 8).unwrap());
        let g1 = scene.generation();
        assert_ne!(g0, g1, "commit must bump the generation");
        scene.clear_surface();
        assert_ne!(g1, scene.generation(), "clear must bump the generation");
        assert_eq!(scene.compose(64, 64), test_pattern::render(64, 64));
    }

    #[test]
    fn mis_sized_or_degenerate_content_is_rejected() {
        // Length mismatch in both directions.
        assert!(SurfaceContent::from_rgba(vec![0; 4 * 3], 2, 2).is_err());
        assert!(SurfaceContent::from_rgba(vec![0; 4 * 5], 2, 2).is_err());
        // Zero dimensions are a clear_surface, not a commit.
        assert!(SurfaceContent::from_rgba(Vec::new(), 0, 4).is_err());
        assert!(SurfaceContent::from_rgba(Vec::new(), 4, 0).is_err());
        // The well-formed case constructs.
        assert!(SurfaceContent::from_rgba(vec![0; 4 * 4], 2, 2).is_ok());
    }
}
