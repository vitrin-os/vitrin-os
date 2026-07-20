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
//! [`human_visible_from_view`] is that fork, and it is the reason the consent
//! overlay (P1.7.1) can never reach a capture:
//!
//! ```text
//!   Scene::compose ─┬─► retained realm view ──► capture_frame ──► agent
//!                   └─► human_visible_from_view ──► the human's display
//!                          + ConsentSurface
//! ```
//!
//! Everything an agent may observe comes from [`Scene::compose`] directly;
//! everything a human sees comes from here. The overlay is applied only on
//! this side, so `docs/protocol/05-vitrin_consent.md`'s "it never appears in
//! captured frames" holds by construction rather than by a check (the full
//! argument is in [`crate::consent`]'s module docs).
//!
//! Both backends reach that one function: the nested backend through
//! [`compose_human_visible`] (compose + overlay in one step) and the headless
//! backend by calling it directly with the view it already composed for its
//! capture image. Stated because the previous arrangement had headless
//! open-coding the same two steps, which meant "both backends present the same
//! output" rested on an equality assertion in a single test rather than on
//! there being one implementation — and a doc comment claiming the latter.

pub mod headless;
pub mod winit;

use crate::consent::ConsentSurface;
use crate::scene::Scene;

/// Apply the consent overlay to an **already-composed** realm view, yielding
/// human-visible output.
///
/// This is *the* overlay-application step — the one place in the core where
/// prompt pixels join view pixels — and both backends reach it, which is what
/// makes "nested and headless cannot drift in what a human sees" a property of
/// the code rather than of an assertion in one test.
///
/// It takes composed bytes rather than a [`Scene`] because the headless
/// backend needs the realm view *by itself* as well (it retains that image
/// separately for capture), and composing twice would be two chances for the
/// capture path and the human's display to disagree about the realm view. So
/// headless composes once and calls this with the result; nested has no such
/// second consumer and calls [`compose_human_visible`], which is this function
/// with the compose in front of it.
///
/// `view` must be `width * height * 4` bytes of RGBA8888 — the layout
/// [`Scene::compose`] returns. With no prompt up it is returned unchanged,
/// byte for byte.
pub(crate) fn human_visible_from_view(
    mut view: Vec<u8>,
    consent: &mut ConsentSurface,
    width: u32,
    height: u32,
) -> Vec<u8> {
    consent.composite_over(&mut view, width, height);
    view
}

/// Compose one frame of **human-visible** output: the realm view with the
/// consent prompt, if any, on top.
///
/// [`Scene::compose`] followed by [`human_visible_from_view`], for callers
/// that do not separately need the bare realm view. It returns tightly packed
/// RGBA8888, rows top-down, every pixel opaque — the same layout and the same
/// contract as [`Scene::compose`], because with no prompt up it *is*
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
    human_visible_from_view(scene.compose(width, height), consent, width, height)
}
