// SPDX-License-Identifier: MPL-2.0
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
//! **One [`Scene`] is one realm's view, not the session's** (WS-E.1.3,
//! issue #209). A session holding N realms holds N scenes, in
//! [`RealmScenes`], with exactly one of them bound to the output; a capture
//! composes *the granted realm's* scene, never the output's. Everything
//! below is about a single realm's composition and is unchanged by that —
//! `Scene` never knew which realm it belonged to and still does not. See
//! [`realms`] for the binding, the hidden-realm contract, and why the
//! selection had to become structural.
//!
//! Composition is pure CPU byte assembly, deterministic and renderer-free:
//! a pure function of the committed surface content and the view size.
//! That keeps the pixel goldens exact (integer math only, no filtering, no
//! renderer state) and keeps this module trivially testable.
//!
//! # The surface-content seam (fed by the shim server, P1.3.4 / #21)
//!
//! [`Scene::commit`] / [`Scene::clear_surface`] are the seam the shim-facing
//! protocol server ([`crate::shim`]) drives: on a shim's `commit`, it copies
//! the attached shm/memfd buffer out of the client's fd (buffer path v0 =
//! copy-in, plan D3) and commits the bytes here, then the embedder triggers
//! a backend redraw; on surface loss (shim death) it clears. Deliberately
//! the smallest honest interface: plain bytes in, no protocol, no fds, no
//! Wayland objects. At runtime the seam goes live when the realm spawn
//! manager provides the shim connection (P1.5.2).
//!
//! # Damage tracking (issue #117)
//!
//! [`Scene::commit_with_damage`] additionally accepts the bounding box of
//! the wire `damage` rectangles a commit named, and [`Scene::take_damage_view`]
//! hands a presentation backend the accumulated damage since its last take,
//! mapped into view coordinates through the same [`layout::place`]
//! placement [`Scene::compose`] itself uses. This is what lets the nested
//! backend upload only the changed region of its window texture instead of
//! the whole view on every commit — the shim already tracks and forwards
//! real damage (`shim/src/upstream.c`); before this it was latched by
//! [`crate::shim`] and then discarded.
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
pub(crate) mod realms;

use std::error::Error;
use std::fmt;

pub(crate) use realms::RealmScenes;

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
/// not exactly `width * height * 4`. The shim-facing server
/// ([`crate::shim`], P1.3.4) maps this to the `invalid_buffer` log-and-close
/// condition at its commit site; composition itself never sees an
/// inconsistent buffer. Constructed only through [`from_rgba`].
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
    /// not a commit), as is any length mismatch (128-bit math, so even
    /// `u32::MAX * u32::MAX * 4` cannot wrap the check — these dimensions
    /// are wire-derived and attacker-controlled: the shim server
    /// ([`crate::shim`]) feeds them off the wire).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, ContentSizeMismatch> {
        let expected = u128::from(width) * u128::from(height) * BYTES_PER_PIXEL as u128;
        if width == 0 || height == 0 || rgba.len() as u128 != expected {
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

/// One damage rectangle, verbatim off the wire (buffer-local coordinates;
/// clamping is composition's business, not this type's — out-of-bounds is a
/// non-error per the IDL). Shared between the shim protocol server
/// ([`crate::shim`], which folds a commit's rectangles into one here) and
/// this module (which maps the fold into view space for a damage-limited
/// GPU upload, issue #117).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DamageRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl DamageRect {
    /// Fold `other` into `self` as a bounding box (saturating: damage is a
    /// hint, over-approximation is always legal).
    pub fn union(&mut self, other: DamageRect) {
        let x2 = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let y2 = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        self.x = self.x.min(other.x);
        self.y = self.y.min(other.y);
        self.width = x2.saturating_sub(self.x);
        self.height = y2.saturating_sub(self.y);
    }
}

/// What has changed in the scene since the last [`Scene::take_damage_view`],
/// in the committed surface's own coordinate space.
///
/// Three states rather than a plain `Option<DamageRect>` because "nothing
/// pending" and "pending, but not boundable" are different facts a caller
/// must not conflate: the first means a redraw can be skipped outright (no
/// caller of this module currently does, but the distinction is what makes
/// that safe to add later), the second means any redraw must treat the
/// *entire* view as changed. Collapsing them would either upload garbage
/// (treating an unbounded change as "nothing") or waste GPU bandwidth on an
/// idle scene (treating "nothing" as "everything").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingDamage {
    /// Nothing has changed since the last take.
    Clean,
    /// Exactly this surface-local rectangle changed; nothing else did.
    Rect(DamageRect),
    /// Something changed that cannot be soundly bounded to a rectangle (a
    /// brand-new surface, a resize, or a commit that named no damage at
    /// all): the next take must report "everything".
    Unbounded,
}

impl PendingDamage {
    /// Fold in one more commit's damage hint: `None` means the commit named
    /// no boundable rectangle (a hostile or merely lazy shim) and must be
    /// treated as unbounded, same as any other unbounded cause.
    fn fold(self, hint: Option<DamageRect>) -> PendingDamage {
        match (self, hint) {
            (PendingDamage::Unbounded, _) | (_, None) => PendingDamage::Unbounded,
            (PendingDamage::Clean, Some(r)) => PendingDamage::Rect(r),
            (PendingDamage::Rect(mut acc), Some(r)) => {
                acc.union(r);
                PendingDamage::Rect(acc)
            }
        }
    }
}

/// **One realm's** scene: at most one client surface in the MVP (single
/// maximized, plan P1.3.3). The realm object (P1.5.1) hangs off this; the
/// consent overlay (P1.7.1) later composites *above* the view this scene
/// produces.
///
/// One per realm since WS-E.1.3 (issue #209) — see [`RealmScenes`], which
/// owns the map and the output binding. "At most one client surface" is a
/// statement about **this realm**, not about the session: it is the MVP's
/// single-maximized model per app, and it is what `scene::layout` says must
/// never grow.
pub(crate) struct Scene {
    surface: Option<SurfaceContent>,
    /// Bumped on every content change; presentation caches (the nested
    /// backend's texture) key on it to know when to re-upload.
    generation: u64,
    /// Damage accumulated since the last [`Self::take_damage_view`] — the
    /// bookkeeping [`Self::take_damage_view`] drains and
    /// [`Self::commit`]/[`Self::clear_surface`] feed.
    pending_damage: PendingDamage,
}

impl Scene {
    /// An empty scene: composes the deterministic test-pattern background.
    pub fn new() -> Self {
        Self {
            surface: None,
            generation: 0,
            pending_damage: PendingDamage::Clean,
        }
    }

    /// Commit new surface content with no damage hint — every change is
    /// presumed to cover the whole surface. A thin wrapper over
    /// [`Self::commit_with_damage`] for the many call sites (mostly tests)
    /// that have no damage rectangle to offer; the shim protocol server
    /// ([`crate::shim`], P1.3.4), which does, calls the full form directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn commit(&mut self, content: SurfaceContent) {
        self.commit_with_damage(content, None);
    }

    /// Commit new surface content — the seam the shim protocol server
    /// ([`crate::shim`], P1.3.4) feeds after its shm copy-in — together with
    /// the bounding box of the wire `damage` rectangles that produced it, in
    /// the *surface's own* coordinates (buffer-local, exactly as the shim
    /// sent them). `None` means the commit named no boundable damage — a
    /// legal but coarse shim, or none of the callers that do not track
    /// damage at all — and is treated exactly like a change this scene
    /// cannot bound to a rectangle. The caller is responsible for
    /// triggering a redraw.
    ///
    /// A geometry change (the new content's size differs from what was
    /// committed before, or there was no prior surface at all) always
    /// widens the pending damage to "everything": the *placement* of the
    /// surface within the view moves too ([`layout::place`] is a function of
    /// both sizes), so a rectangle bounded in the old geometry could name
    /// the wrong pixels in the new one.
    pub fn commit_with_damage(&mut self, content: SurfaceContent, damage: Option<DamageRect>) {
        let same_geometry = self
            .surface
            .as_ref()
            .is_some_and(|prev| prev.width == content.width && prev.height == content.height);
        self.surface = Some(content);
        self.generation = self.generation.wrapping_add(1);
        self.pending_damage = if same_geometry {
            self.pending_damage.fold(damage)
        } else {
            PendingDamage::Unbounded
        };
    }

    /// Drop the committed surface (shim death — the shim server's
    /// connection-teardown path, P1.3.4/P1.5.3): the view falls back to
    /// the deterministic background, never a stale frame.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear_surface(&mut self) {
        self.surface = None;
        self.generation = self.generation.wrapping_add(1);
        // The whole view changes (background replaces whatever was there),
        // which no surface-local rectangle from before this call could name.
        self.pending_damage = PendingDamage::Unbounded;
    }

    /// Take (and reset) the damage accumulated since the last call, mapped
    /// into `view`-sized view coordinates through the same [`layout::place`]
    /// placement [`Self::compose`] uses — so a caller's idea of "what
    /// changed on screen" can never drift from what composition itself would
    /// draw differently.
    ///
    /// `None` means "the caller must treat the whole view as changed": no
    /// surface is committed, or the pending damage is
    /// [`PendingDamage::Unbounded`]. `Some(rect)` is exact — including the
    /// degenerate all-zero rectangle when the accumulated surface-local
    /// damage clips away entirely outside the view (e.g. it landed in a
    /// center-cropped margin), which tells the caller correctly that
    /// nothing *visible* changed and no GPU upload at all is owed.
    ///
    /// Exists for the nested backend's damage-limited texture upload (issue
    /// #117): the shim already tracks and forwards real damage
    /// (`shim/src/upstream.c`), and the core already latches it
    /// ([`crate::shim`]'s `handle_damage`/`handle_commit`) — this is the
    /// seam that stops it being discarded at the shim→core→GPU boundary.
    pub fn take_damage_view(&mut self, view: (u32, u32)) -> Option<DamageRect> {
        let pending = std::mem::replace(&mut self.pending_damage, PendingDamage::Clean);
        let (local, surface) = match (pending, &self.surface) {
            (PendingDamage::Rect(r), Some(s)) => (r, s),
            _ => return None,
        };
        let placement = layout::place(view, (surface.width, surface.height));
        let x1 = (placement.x + i64::from(local.x)).clamp(0, i64::from(view.0));
        let y1 = (placement.y + i64::from(local.y)).clamp(0, i64::from(view.1));
        let x2 = (placement.x + i64::from(local.x) + i64::from(local.width.max(0)))
            .clamp(0, i64::from(view.0));
        let y2 = (placement.y + i64::from(local.y) + i64::from(local.height.max(0)))
            .clamp(0, i64::from(view.1));
        Some(DamageRect {
            x: x1 as i32,
            y: y1 as i32,
            width: (x2 - x1).max(0) as i32,
            height: (y2 - y1).max(0) as i32,
        })
    }

    /// The content generation: changes exactly when composed output may
    /// have changed (for a fixed view size).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Size of the committed client surface, if any — the geometry the
    /// input router (P1.3.7, [`crate::input`]) maps pointer coordinates
    /// through. Router and composition use the same deterministic
    /// [`layout::place`], so they can never disagree about where the
    /// surface sits in the view.
    pub fn surface_size(&self) -> Option<(u32, u32)> {
        self.surface.as_ref().map(|s| (s.width, s.height))
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

        // **The matte is painted only where the client will not cover it.**
        //
        // Byte-for-byte what a full-view fill followed by the copy below
        // produces — the destination rectangle is written unconditionally on
        // the next loop — and in the maximized case, which is every session
        // this core currently runs, it is the difference between touching
        // every pixel twice and touching it once. That mattered enough to fix
        // when the bare-metal backend's first run on hardware composed a
        // 2560x1600 view twice per dirty round: at 4.1 M pixels a whole-view
        // 4-byte-at-a-time fill is ~40% of this function and 100% of it is
        // thrown away.
        //
        // `letterbox_rows` fills whole rows above and below the client;
        // `letterbox_cols` fills the two side bars beside it. Kept as two
        // named steps rather than one clever loop because the failure mode of
        // getting it wrong is a strip of uninitialised black along an edge
        // that no assertion about the client's own pixels would notice —
        // `compose_matches_a_whole_view_fill` compares against the naive
        // implementation over a matrix of geometries for exactly that reason.
        let row_bytes = vw * BYTES_PER_PIXEL;
        let fill_rows = |out: &mut [u8], from: usize, to: usize| {
            for px in out[from * row_bytes..to * row_bytes].chunks_exact_mut(BYTES_PER_PIXEL) {
                px.copy_from_slice(&LETTERBOX_RGBA);
            }
        };
        fill_rows(&mut out, 0, dst_y);
        fill_rows(&mut out, dst_y + copy_h, vh);
        for row in dst_y..dst_y + copy_h {
            let base = row * row_bytes;
            for px in out[base..base + dst_x * BYTES_PER_PIXEL].chunks_exact_mut(BYTES_PER_PIXEL) {
                px.copy_from_slice(&LETTERBOX_RGBA);
            }
            for px in out[base + (dst_x + copy_w) * BYTES_PER_PIXEL..base + row_bytes]
                .chunks_exact_mut(BYTES_PER_PIXEL)
            {
                px.copy_from_slice(&LETTERBOX_RGBA);
            }
        }

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
    fn wider_but_shorter_surface_letterboxes_x_and_crops_y() {
        // The mixed case: surface taller than the view but narrower than it
        // is wide relative to the surface — here view 100x20, surface 40x50,
        // so the destination x-offset (30) and the source y-offset (15) are
        // simultaneously nonzero on different axes. Pins the clip math in
        // compose against regression.
        let (vw, vh) = (100, 20);
        let (sw, sh) = (40, 50);
        let pixels = client_pixels(sw, sh);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(pixels.clone(), sw, sh).unwrap());
        let out = scene.compose(vw, vh);

        // Horizontally centered at 1:1; vertically the surface's central
        // vh-tall band fills the whole view height.
        let ox = (vw - sw) / 2; // 30
        let cy = (sh - vh) / 2; // 15
        for y in 0..vh {
            for x in 0..sw {
                assert_eq!(px(&out, vw, ox + x, y), px(&pixels, sw, x, cy + y));
            }
        }
        // Matte in the side bars, immediately outside the placed rectangle
        // and in all four view corners.
        for y in [0, vh - 1] {
            for x in [0, ox - 1, ox + sw, vw - 1] {
                assert_eq!(px(&out, vw, x, y), LETTERBOX_RGBA);
            }
        }
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

    /// **The matte painted only where the client will not cover it is
    /// byte-for-byte the whole-view fill it replaced** (WS-E.3.5).
    ///
    /// The optimisation is worth having — the bare-metal backend composes a
    /// 2560x1600 view twice per dirty round, and a whole-view 4-byte-at-a-time
    /// fill is ~40% of this function and entirely thrown away in the maximized
    /// case — and it is exactly the kind of change whose failure mode nothing
    /// else would catch: a strip of uninitialised **black** along one edge,
    /// which no assertion about the client's own pixels would notice and which
    /// looks like a rendering artifact rather than a bug.
    ///
    /// So this compares against the naive implementation directly, over a
    /// matrix of geometries chosen to hit every letterbox shape: client
    /// smaller than the view in one axis, in both, exactly equal, larger
    /// (clipped), and degenerate.
    #[test]
    fn compose_matches_a_whole_view_fill() {
        /// The implementation this replaced: fill every pixel, then blit.
        fn naive(scene: &Scene, width: u32, height: u32) -> Vec<u8> {
            let composed = scene.compose(width, height);
            let Some(surface) = &scene.surface else {
                return composed;
            };
            let (vw, vh) = (width as usize, height as usize);
            let mut out = vec![0u8; vw * vh * BYTES_PER_PIXEL];
            for px in out.chunks_exact_mut(BYTES_PER_PIXEL) {
                px.copy_from_slice(&LETTERBOX_RGBA);
            }
            if out.is_empty() {
                return out;
            }
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
                for px in dst.chunks_exact_mut(BYTES_PER_PIXEL) {
                    px[3] = 0xff;
                }
            }
            out
        }

        for (cw, ch) in [
            (1u32, 1u32),
            (33, 17),
            (64, 8),
            (8, 64),
            (64, 64),
            (200, 90),
        ] {
            for (vw, vh) in [(64u32, 64u32), (101, 59), (17, 33), (1, 1), (0, 0), (5, 0)] {
                let mut scene = Scene::new();
                scene.commit(SurfaceContent::from_rgba(client_pixels(cw, ch), cw, ch).unwrap());
                assert_eq!(
                    scene.compose(vw, vh),
                    naive(&scene, vw, vh),
                    "client {cw}x{ch} in view {vw}x{vh}: the matte must be painted everywhere \
                     the client does not cover, or an edge of the view is left black"
                );
                // ...and the letterbox really is present in at least one of
                // these, or the comparison above is between two identical
                // no-ops.
                if (vw > cw || vh > ch) && vw > 0 && vh > 0 {
                    assert!(
                        scene
                            .compose(vw, vh)
                            .chunks_exact(BYTES_PER_PIXEL)
                            .any(|px| px == LETTERBOX_RGBA),
                        "client {cw}x{ch} in view {vw}x{vh} must letterbox"
                    );
                }
            }
        }
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

    // ------------------------------------------------------------------
    // Damage tracking (issue #117): the view-space bounding box
    // `take_damage_view` hands the nested backend for its damage-limited
    // GPU upload.
    // ------------------------------------------------------------------

    #[test]
    fn a_fresh_or_resized_surface_reports_unbounded_damage() {
        let mut scene = Scene::new();
        // No prior surface at all.
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(8, 8), 8, 8).unwrap(),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            }),
        );
        assert_eq!(
            scene.take_damage_view((64, 64)),
            None,
            "the very first commit must report unbounded damage regardless of the hint"
        );

        // A same-generation resize likewise cannot be bounded: the surface's
        // placement in the view moved.
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(8, 8), 8, 8).unwrap(),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            }),
        );
        scene.take_damage_view((64, 64)); // drain, so the next take is clean
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(16, 8), 16, 8).unwrap(),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 16,
                height: 8,
            }),
        );
        assert_eq!(
            scene.take_damage_view((64, 64)),
            None,
            "a geometry change must report unbounded damage"
        );
    }

    #[test]
    fn a_commit_naming_no_damage_is_unbounded() {
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(8, 8), 8, 8).unwrap());
        scene.take_damage_view((64, 64)); // drain the initial unbounded state
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(8, 8), 8, 8).unwrap(),
            None,
        );
        assert_eq!(
            scene.take_damage_view((64, 64)),
            None,
            "a commit with no boundable hint must be treated as unbounded, not skipped"
        );
    }

    #[test]
    fn a_bounded_hint_maps_through_the_same_placement_composition_uses() {
        let (vw, vh) = (100, 80);
        let (sw, sh) = (40, 20);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(sw, sh), sw, sh).unwrap());
        scene.take_damage_view((vw, vh)); // drain the initial unbounded state

        // A 10x6 rectangle at (2, 3) in the surface's own coordinates.
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(sw, sh), sw, sh).unwrap(),
            Some(DamageRect {
                x: 2,
                y: 3,
                width: 10,
                height: 6,
            }),
        );
        let placement = layout::place((vw, vh), (sw, sh));
        assert_eq!(
            scene.take_damage_view((vw, vh)),
            Some(DamageRect {
                x: placement.x as i32 + 2,
                y: placement.y as i32 + 3,
                width: 10,
                height: 6,
            }),
            "the surface-local rectangle must land exactly where compose() would place it"
        );
        // Taken: a further take with nothing new pending is unbounded
        // (nothing has changed, but there is also nothing bounded to hand
        // back — see PendingDamage::Clean).
        assert_eq!(scene.take_damage_view((vw, vh)), None);
    }

    #[test]
    fn damage_outside_the_view_clips_to_a_degenerate_rect() {
        // A surface larger than the view, center-cropped (module docs):
        // damage entirely inside the cropped-away margin clips to a
        // zero-area rectangle rather than reporting unbounded — nothing
        // visible changed, and the caller can skip the GPU upload outright.
        let (vw, vh) = (40, 30);
        let (sw, sh) = (60, 50);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(sw, sh), sw, sh).unwrap());
        scene.take_damage_view((vw, vh));

        let placement = layout::place((vw, vh), (sw, sh));
        assert!(placement.x < 0, "the fixture must actually crop");
        // Damage confined to the cropped-away left margin (x in [0, -placement.x)).
        let margin = (-placement.x) as i32;
        assert!(margin > 0);
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(sw, sh), sw, sh).unwrap(),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: margin,
                height: 1,
            }),
        );
        let rect = scene.take_damage_view((vw, vh)).expect("bounded");
        assert_eq!((rect.width, rect.height), (0, 0));
    }

    #[test]
    fn clearing_the_surface_is_unbounded_damage() {
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(8, 8), 8, 8).unwrap());
        scene.take_damage_view((64, 64));
        scene.clear_surface();
        assert_eq!(scene.take_damage_view((64, 64)), None);
    }

    #[test]
    fn multiple_commits_accumulate_a_bounding_box_before_the_next_take() {
        let (vw, vh) = (64, 64);
        let mut scene = Scene::new();
        scene.commit(SurfaceContent::from_rgba(client_pixels(64, 64), 64, 64).unwrap());
        scene.take_damage_view((vw, vh));

        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(64, 64), 64, 64).unwrap(),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            }),
        );
        scene.commit_with_damage(
            SurfaceContent::from_rgba(client_pixels(64, 64), 64, 64).unwrap(),
            Some(DamageRect {
                x: 20,
                y: 20,
                width: 4,
                height: 4,
            }),
        );
        assert_eq!(
            scene.take_damage_view((vw, vh)),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 24,
                height: 24,
            }),
            "two commits coalesced by one take must union, exactly as a coalesced dispatch \
             round presents them"
        );
    }

    #[test]
    fn mis_sized_or_degenerate_content_is_rejected() {
        // Length mismatch in both directions.
        assert!(SurfaceContent::from_rgba(vec![0; 4 * 3], 2, 2).is_err());
        assert!(SurfaceContent::from_rgba(vec![0; 4 * 5], 2, 2).is_err());
        // Zero dimensions are a clear_surface, not a commit.
        assert!(SurfaceContent::from_rgba(Vec::new(), 0, 4).is_err());
        assert!(SurfaceContent::from_rgba(Vec::new(), 4, 0).is_err());
        // Huge dimensions whose byte count would wrap 64-bit math
        // (2^31 * 2^31 * 4 = 2^64) must be rejected, not wrap to an
        // expected length of 0 — these are wire-derived once P1.3.4 lands.
        assert!(SurfaceContent::from_rgba(Vec::new(), 0x8000_0000, 0x8000_0000).is_err());
        assert!(SurfaceContent::from_rgba(Vec::new(), u32::MAX, u32::MAX).is_err());
        // The well-formed case constructs.
        assert!(SurfaceContent::from_rgba(vec![0; 4 * 4], 2, 2).is_ok());
    }
}
