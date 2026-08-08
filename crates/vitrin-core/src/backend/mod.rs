// SPDX-License-Identifier: MPL-2.0
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
//! argument is in [`crate::consent`]'s module docs). The **lock screen**
//! (WS-E.2.2, [`crate::lock`]) is applied at the same step and inherits the
//! identical property: an agent holding `observe` keeps receiving the realm
//! view across a lock and never sees the lock itself — which is the mechanical
//! half of the decision D-025 records, and is published in
//! `docs/book/src/limits.md` rather than left to be discovered. The trusted indicator
//! (issue #85) — the always-present band and the per-prompt frame — lives on
//! this side too, and is invisible to a capture for exactly the same reason.
//! So does the **agent cursor sprite** (D-019, [`crate::cursor`]), which is
//! why the IDL's ordering invariant 4 — no agent principal's cursor in another
//! principal's captured frame — is a rule with something to be true of rather
//! than a vacuous one. It is applied a step past this function (in [`winit`]'s
//! `window_pixels` and in headless's own composite), for the same
//! reason the dead-man hold indicator is: it is nested-side display state
//! headless draws only on request. What excludes it from a capture is
//! unchanged — it is downstream of [`Scene::compose`], full stop.
//!
//!
//! Both backends reach that one function on the **CPU compositing path**:
//! the nested backend through [`compose_human_visible`] (compose + overlay in
//! one step) and the headless backend by calling it directly with the view it
//! already composed for its capture image. Stated because the previous
//! arrangement had headless open-coding the same two steps, which meant "both
//! backends present the same output" rested on an equality assertion in a
//! single test rather than on there being one implementation — and a doc
//! comment claiming the latter.
//!
//! # There is a second human-visible path, and it does not come through here
//!
//! The nested backend's zero-copy dmabuf branch (P1.3.5) presents the
//! client's imported texture straight to the window with no CPU composite at
//! all, so it reaches neither [`Scene::compose`] nor this function. That is
//! not a hole in the capture argument above — a capture is *still* only ever
//! `Scene::compose` on the CPU, on both backends, so nothing this branch
//! draws can reach an agent — but it **is** a second place the trusted band
//! has to be painted, and the first cut of it was not painted there at all:
//! every dmabuf-presented frame consisted purely of pixels the confined
//! client owns, free to carry a counterfeit band with nothing genuine above
//! it (issue #85's whole threat).
//!
//! The band is therefore inside
//! [`crate::dmabuf::human_visible_frame`]'s draw list rather than applied by
//! whoever calls the GPU presenter, so the invariant survives a third
//! presentation path being added by someone who never read this paragraph.
//! `no_presentation_path_can_drop_the_trusted_band` (in [`winit`]'s tests)
//! holds the two paths against each other, including that they paint the one
//! session secret and not two.

/// The trusted-band witness (issue #139): the *negative* half of issue #85,
/// measured on the headless backend's own composites.
///
/// Present in a `cargo test` build as well as a `consent-injector` feature
/// build, and only those two — the same posture as
/// [`crate::consent::injector`]'s parser, for the same reason. Everything in
/// it is pure arithmetic over two buffers and it confers nothing; what is
/// gated on the feature alone is the *wiring* (the field on
/// [`headless::HeadlessOutput`], the call in its composite, and the `band`
/// reply), so a plain build computes nothing and answers nothing.
#[cfg(any(test, feature = "consent-injector"))]
pub(crate) mod band_witness;
pub mod headless;
pub mod winit;

use crate::consent::ConsentSurface;
use crate::lock::LockSurface;
use crate::scene::Scene;
use crate::status::StatusStrip;

/// Apply the consent overlay to an **already-composed** realm view, yielding
/// human-visible output.
///
/// This is *the* overlay-application step — the one place in the core where
/// prompt pixels join view pixels — and every CPU-composited frame on both
/// backends reaches it, which is what makes "nested and headless cannot drift
/// in what a human sees" a property of the code rather than of an assertion
/// in one test. A consent prompt or a dead-man hold forces the nested backend
/// onto the CPU path, so it is also the only step a *prompt* can be applied
/// in; the trusted band, which is on every frame prompt or not, has a second
/// home on the zero-copy path (see this module's docs and
/// [`crate::dmabuf::human_visible_frame`]).
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
/// [`Scene::compose`] returns. With no prompt up the realm view is unchanged
/// *except* for the trusted band along the top edge (issue #85), which is
/// present on every human-visible frame; a prompt adds the scrim, the framed
/// card, and nothing the band does not already assert.
///
/// `attention` is whether the human's attention window is open right now
/// ([`crate::attention::AttentionSignal::is_open`]). It draws a fixed-geometry
/// marker immediately *below* the reserved band — never in it, because the
/// band's whole value is having exactly one correct appearance. Passing it
/// through this function rather than a backend's own composite is what makes
/// "the marker can never reach a capture" the same structural fact the consent
/// card's exclusion is.
///
/// `status` is the session's status strip (WS-E.2.3, [`crate::status`]), on the
/// same terms and for the same reason: a clock is a timing oracle and a battery
/// level is a session fact an agent does not otherwise have, so both are drawn
/// on *this* side of the fork and can no more reach `frame_ready` than the
/// consent card can. With `--status` off it is a single branch and this
/// function's output is byte-identical to what it was before the strip existed.
pub(crate) fn human_visible_from_view(
    mut view: Vec<u8>,
    consent: &mut ConsentSurface,
    lock: &mut LockSurface,
    status: &mut StatusStrip,
    width: u32,
    height: u32,
    attention: bool,
) -> Vec<u8> {
    // **The status strip goes first** (WS-E.2.3, issue #215), and its first
    // arrangement had it last — over the consent card — which was wrong in the
    // one way that matters. The card is centred and the strip occupies rows
    // `[8, 8+H)`; on any output short enough for the card to reach those rows,
    // a strip drawn afterwards overdraws the card's **trusted ring**, which is
    // the human's only anti-forgery check on a prompt. That reconciliation
    // enumerated the lock cover, the attention marker, the band and the
    // dead-man indicator, and never mentioned the card at all.
    //
    // So the order is: strip, then everything whose integrity the human is
    // asked to trust. The cost of putting it here rather than after the lock is
    // real and is accepted: an opaque lock cover now hides the clock, so
    // "the strip is always there" gains the exception "except behind a lock".
    // A visible clock on a lock screen is a convenience; an unforgeable ring
    // around a consent card is the thing this project is for.
    status.composite_over(&mut view, width, height);
    // Prompt (scrim + frame + card) next; then the trusted band on top, so
    // client content — and even the scrim — never sits over the one strip the
    // human reads the session colour from.
    consent.composite_over(&mut view, width, height);
    // ...then the lock screen (WS-E.2.2, issue #214), which is an OPAQUE cover
    // rather than a scrim and therefore hides everything drawn so far,
    // deliberately including a consent card. A prompt raised while the human is
    // away is also *inert* while the lock is up — `LockGate` is outermost, so
    // the consent grab's judgement never runs — so hiding it is not concealing
    // an answerable decision; it resolves `timed_out`, which is refusal. Before
    // the band, because the band's whole value is having exactly one correct
    // appearance and nothing may sit on it, core-drawn or not.
    lock.composite_over(&mut view, width, height);
    consent.composite_trust_band(&mut view, width, height);
    // ...and, while the human's attention window is open, a marker **beside**
    // the band and never inside it (WS-E.1.7, issue #232). After the band so
    // the band's rows are already final, and here rather than in
    // `Scene::compose` so an agent cannot observe the human's attention presses
    // through `frame_ready` — the same structural fork the overlay and the band
    // already rest on. The dead-man hold indicator is still composited after
    // everything, so nothing here can hide a hold in progress.
    if attention {
        crate::attention::composite_attention_marker(&mut view, width, height);
    }
    view
}

/// Compose one frame of **human-visible** output: the realm view with the
/// consent prompt, if any, on top.
///
/// [`Scene::compose`] followed by [`human_visible_from_view`], for callers
/// that do not separately need the bare realm view. It returns tightly packed
/// RGBA8888, rows top-down, every pixel opaque — the same layout and the same
/// contract as [`Scene::compose`], differing from it by the trusted band along
/// the top edge (always) and the framed prompt (when one is up).
///
/// **Not what capture serves.** [`crate::capture::render_frame`] is fed the
/// realm view, never this. The headless backend keeps the two in separate
/// retained images to make that impossible to confuse; see
/// [`headless::HeadlessState`].
pub(crate) fn compose_human_visible(
    scene: &Scene,
    consent: &mut ConsentSurface,
    lock: &mut LockSurface,
    status: &mut StatusStrip,
    width: u32,
    height: u32,
    attention: bool,
) -> Vec<u8> {
    human_visible_from_view(
        scene.compose(width, height),
        consent,
        lock,
        status,
        width,
        height,
        attention,
    )
}
