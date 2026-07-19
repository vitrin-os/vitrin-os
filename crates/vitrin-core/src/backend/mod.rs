//! Presentation backends for the trusted core.
//!
//! Two backends exist. The nested [`winit`] backend runs the core as a client
//! of the host compositor, presenting one host window (P1.3.1). The
//! [`headless`] backend drives a fixed-size virtual output composited in
//! software, its framebuffer retained in memory for capture (P1.3.2). `main`
//! selects between them with `--nested` / `--headless`. Both backends present
//! the same realm views; nothing outside this module may depend on which one
//! is running.
//!
//! # The output stage is where human-visible and agent-visible pixels fork
//!
//! [`compose_human_visible`] is that fork, and it is the reason the consent
//! overlay (P1.7.1) can never reach a capture:
//!
//! ```text
//!   Scene::compose ─┬─► retained realm view ──► capture_frame ──► agent
//!                   └─► compose_human_visible ──► the human's display
//!                          + ConsentSurface
//! ```
//!
//! Everything an agent may observe comes from [`Scene::compose`] directly;
//! everything a human sees comes from here. The overlay is applied only on
//! this side, so `docs/protocol/05-vitrin_consent.md`'s "it never appears in
//! captured frames" holds by construction rather than by a check (the full
//! argument is in [`crate::consent`]'s module docs).

pub mod headless;
pub mod winit;

use crate::consent::ConsentSurface;
use crate::scene::Scene;

/// Compose one frame of **human-visible** output: the realm view with the
/// consent prompt, if any, on top.
///
/// The single implementation both backends present, for the same reason
/// [`Scene::compose`] is the single realm-view implementation (P1.3.3): so
/// nested and headless cannot drift in what a human sees. It returns tightly
/// packed RGBA8888, rows top-down, every pixel opaque — the same layout and
/// the same contract as [`Scene::compose`], because with no prompt up it *is*
/// [`Scene::compose`], byte for byte.
///
/// **Not what capture serves.** [`crate::capture::render_frame`] is fed the
/// realm view, never this. The headless backend keeps the two in separate
/// retained images to make that impossible to confuse; see
/// [`headless::HeadlessState`].
pub(crate) fn compose_human_visible(
    scene: &Scene,
    consent: &mut ConsentSurface,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut pixels = scene.compose(width, height);
    consent.composite_over(&mut pixels, width, height);
    pixels
}
