// SPDX-License-Identifier: MPL-2.0
//! Presentation backends for the trusted core.
//!
//! Three backends exist. The nested [`winit`] backend runs the core as a
//! client of the host compositor, presenting one host window (P1.3.1). The
//! [`headless`] backend drives a fixed-size virtual output composited in
//! software, its framebuffer retained in memory for capture (P1.3.2). The
//! [`drm`] backend owns the display controller outright — mode setting, a GBM
//! swapchain, libinput and libseat — and is compiled only under the
//! non-default `drm-backend` feature (WS-E.3.2, issue #218). `main` selects
//! between them with `--nested` / `--headless` / `--drm`. All three present
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
//! Every backend reaches that one function on the **CPU compositing path**:
//! the nested backend through [`compose_human_visible`] (compose + overlay in
//! one step), the headless backend by calling it directly with the view it
//! already composed for its capture image, and the bare-metal backend through
//! the *same* `winit::window_pixels` the nested one uses, so the two paths
//! that face a human cannot disagree about what he is shown. Stated because
//! the previous arrangement had headless open-coding the same two steps, which
//! meant "both backends present the same output" rested on an equality
//! assertion in a single test rather than on there being one implementation —
//! and a doc comment claiming the latter.
//!
//! The **human's own cursor** is the one thing drawn on this side that is not
//! drawn by every backend, and the asymmetry is a fact about who owns the
//! display rather than a difference in policy: nested draws none because the
//! host desktop already does, headless has no pointer device at all, and
//! [`drm`] draws it because otherwise nobody would (D-029; the IDL sentence
//! that said no human cursor is ever composited was made nested-conditional
//! for this). It joins at the same output stage as the agent sprite, so it
//! reaches a capture exactly as often: never.
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
//!
//! **That third path arrived, and the arrangement held.** The bare-metal
//! backend (WS-E.3.2) composites into a scanout buffer through both of the
//! same two entry points — `winit::window_pixels` on the CPU and
//! [`crate::dmabuf::present_human_visible`] zero-copy — so it inherits the
//! band from the existing draw lists rather than painting a third. The one
//! thing it explicitly does *not* do is a **hardware cursor plane**: that is
//! composited by the display controller, outside any draw list this core
//! owns, and there would be no way to put the band into it.

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
/// The idle blank (WS-E.4.3, issue #223): the session's activity clock, the
/// state machine that turns idleness into a dark panel, and the opaque cover
/// that rides the output stage below while it happens.
///
/// Here rather than beside [`crate::lock`] because a blank is **output** state:
/// the cover is composited by [`human_visible_from_view`] and the display power
/// state is set by the one backend that owns a display controller. Living under
/// `crate::backend` is also what lets [`blank::BlankSurface`]'s constructor and
/// composite be `pub(in crate::backend)`, so no module outside this one can mint
/// a cover, hold a cover, or draw a cover.
#[cfg_attr(
    not(feature = "drm-backend"),
    allow(
        dead_code,
        reason = "the only production driver of the blank is the bare-metal backend's \
                  `service_screen`: it is the only backend that owns a display controller, and \
                  the only one whose hook stack carries the lock gate that writes the activity \
                  clock. A default build compiles the whole state machine, the cover and the \
                  resume detector, and exercises every one of them from this crate's own \
                  tests -- which is where CI's coverage of this lives, since no runner has a \
                  DRM device"
    )
)]
pub(crate) mod blank;
/// The bare-metal DRM/KMS backend (WS-E.3.2, issue #218) — **the third
/// presentation path**, and the one this module's docs below warned would
/// arrive: it presents through [`compose_human_visible`] on the CPU and
/// through [`crate::dmabuf::present_human_visible`] zero-copy, so it inherits
/// the trusted band from the same two draw lists rather than painting a third.
///
/// Behind a non-default cargo feature because two of the `-sys` crates it
/// pulls panic the build when their pkg-config file is absent; see the
/// `drm-backend` block in `crates/vitrin-core/Cargo.toml`.
#[cfg(feature = "drm-backend")]
pub mod drm;
pub mod headless;
pub mod winit;

use crate::consent::ConsentSurface;
use crate::lock::LockSurface;
use crate::scene::Scene;
use crate::status::StatusStrip;
use blank::BlankSurface;

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
///
/// `blank` is the **idle cover** (WS-E.4.3, [`blank`]). It joins between the
/// lock cover and the trusted band, and the position is argued in
/// [`blank::BlankSurface::composite_over`]: it is the outermost *opaque* cover
/// and must hide a lock card as well as an app, but nothing may sit on the band,
/// so a blanked frame is black with this session's secret colour still lit along
/// its top edge. It is a required parameter rather than an `Option` so a fourth
/// backend cannot forget it — the only way to have no cover is to hand over a
/// surface that is deliberately down.
// **Eight parameters, and the allow is a decision.** Every one of them is a
// surface the human-visible output stage composites, in the order it composites
// them, and that parallel is load-bearing: `backend::winit::TextureKey::current`
// takes the same run in the same order precisely so "every input to this
// function appears in the cache key" can be checked by lining the two argument
// lists up. Bundling them into a struct would hide exactly that.
#[allow(clippy::too_many_arguments)]
pub(crate) fn human_visible_from_view(
    mut view: Vec<u8>,
    consent: &mut ConsentSurface,
    lock: &mut LockSurface,
    blank: &BlankSurface,
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
    //
    // **That argument is about INPUT, and the record is a separate question the
    // sentence above used to be silent on.** `service_consent_round` has no lock
    // awareness at any line, so a petition arriving while the session is locked
    // *is* raised: it writes `consent_transition{shown}`, sets `prompt_shown`
    // (so the chokepoint refuses that principal `consent_held`) and composites a
    // card under this cover. That is deliberate and it is not the same defect
    // D-030(4) closes for a seat pause: a locked session still owns its panel,
    // so the card really is on the human's screen the moment they unlock, the
    // guard is restarted for them (`ConsentGrab::restart_guard`), and they can
    // answer it. A paused session's card reaches no panel at all and never will
    // — which is why that one is refused a raise and this one is not.
    lock.composite_over(&mut view, width, height);
    // ...then the IDLE BLANK (WS-E.4.3, issue #223), the outermost opaque cover
    // and the last thing before the band. After the lock because a screen going
    // dark must hide the lock card too -- a lock screen legible on a panel the
    // human believes is off would be the same disclosure the cover exists to
    // prevent, one surface up. **Before `composite_trust_band`, deliberately**:
    // the band's whole value is having exactly one correct appearance and
    // nothing may sit on it, core-drawn or not, so a blank that overdrew it
    // would be the first exception ever granted.
    //
    // The consequence is a feature rather than a compromise. In the success case
    // the panel is powered down and nobody sees the frame at all; in the failure
    // case -- a display controller that refuses `clear()` -- the human is looking
    // at a black screen carrying this session's secret colour, which is exactly
    // the signal that distinguishes "vitrind blanked" from "a confined app
    // painted itself black". `the_blank_cover_has_exactly_one_chokepoint` holds
    // this call in this position.
    blank.composite_over(&mut view, width, height);
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_human_visible(
    scene: &Scene,
    consent: &mut ConsentSurface,
    lock: &mut LockSurface,
    blank: &BlankSurface,
    status: &mut StatusStrip,
    width: u32,
    height: u32,
    attention: bool,
) -> Vec<u8> {
    human_visible_from_view(
        scene.compose(width, height),
        consent,
        lock,
        blank,
        status,
        width,
        height,
        attention,
    )
}

#[cfg(test)]
mod tests {
    /// **The blank cover has exactly one chokepoint**, and it is the shared
    /// output stage — the `the_sprite_has_exactly_one_chokepoint` discipline
    /// (WS-E.4.2) applied to WS-E.4.3's cover.
    ///
    /// The compiler already holds the half it can:
    /// [`super::blank::BlankSurface::new`] and
    /// [`super::blank::BlankSurface::composite_over`] are `pub(in
    /// crate::backend)`, so no module outside `crate::backend` can mint, hold or
    /// draw a cover. What the compiler cannot hold is the half this test is
    /// for — that the *one* call inside `crate::backend` is
    /// [`super::human_visible_from_view`]'s, in the one position where the
    /// trusted band is still painted afterwards.
    ///
    /// Three assertions, and each names a different way the property dies:
    ///
    /// * a **second** call site is a second decision about whether the human's
    ///   picture is covered, and the N+1st presentation path is the one that
    ///   forgets;
    /// * the one call being **outside** `human_visible_from_view` puts the cover
    ///   on a path some backend does not take, which is the "third presentation
    ///   path" `backend/mod.rs`'s own docs were written about;
    /// * the one call landing **after** `composite_trust_band` overdraws the
    ///   band, which is the one thing nothing in this core may ever do.
    #[test]
    fn the_blank_cover_has_exactly_one_chokepoint() {
        fn rust_sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("the crate source tree is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    rust_sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("a readable source file");
                    // Truncated at the test module, exactly as the sprite's scan
                    // is: otherwise the tests that exercise the cover would
                    // count as production call sites.
                    let production = text
                        .split_once("\n#[cfg(test)]\nmod tests {")
                        .map(|(before, _)| before.to_string())
                        .unwrap_or(text);
                    out.push((path, production));
                }
            }
        }
        let mut sources = Vec::new();
        rust_sources(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut sources,
        );
        assert!(
            sources.iter().any(|(p, _)| p.ends_with("backend/mod.rs")),
            "the scan must cover this file, or it proves nothing"
        );

        // **Scan for the METHOD, never for the receiver's name.** An earlier
        // version of this test counted the literal `blank.composite_over(`, and
        // a review walked straight through it by binding the same surface to a
        // differently-named local — which would have been a second, untested
        // decision about whether the human's screen is covered, in a test whose
        // whole purpose is to forbid one. The assertion is over the exact
        // expected set of `.composite_over(` receivers under `backend/`, so a
        // new call site fails here whatever it calls its variable.
        let mut calls: Vec<(String, String)> = Vec::new();
        for (path, text) in &sources {
            if !path.components().any(|c| c.as_os_str() == "backend") {
                continue;
            }
            for (at, _) in text.match_indices(".composite_over(") {
                let recv: String = text[..at]
                    .chars()
                    .rev()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<Vec<char>>()
                    .into_iter()
                    .rev()
                    .collect();
                let file = path
                    .file_name()
                    .expect("a source file has a name")
                    .to_string_lossy()
                    .into_owned();
                calls.push((file, recv));
            }
        }
        calls.sort();
        let mut expected = vec![
            ("mod.rs".to_string(), "blank".to_string()),
            ("mod.rs".to_string(), "lock".to_string()),
        ];
        expected.sort();
        let covers: Vec<(String, String)> = calls
            .iter()
            .filter(|(_, recv)| recv == "blank" || recv == "lock")
            .cloned()
            .collect();
        assert_eq!(
            covers, expected,
            "every full-screen cover must be composited from exactly ONE place -- the shared \
             human-visible output stage in backend/mod.rs. A second call is a second decision \
             about whether the human's screen is covered, and the path that forgets is the one \
             nobody tested. This scan matches the METHOD, so renaming the receiver does not \
             evade it. All `.composite_over(` calls under backend/: {calls:?}"
        );

        let source = include_str!("mod.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("this file ends with its own test module")
            .0;
        let stage = source
            .split("pub(crate) fn human_visible_from_view(")
            .nth(1)
            .expect("the shared output stage exists");
        let cover = stage
            .find("blank.composite_over(")
            .expect("the one call must be inside `human_visible_from_view`");
        let lock = stage
            .find("lock.composite_over(")
            .expect("the lock cover is composited at this stage");
        let band = stage
            .find("consent.composite_trust_band(")
            .expect("the trusted band is painted at this stage");
        assert!(
            lock < cover && cover < band,
            "the blank cover must sit AFTER the lock cover and BEFORE the trusted band: after \
             the lock because a dark screen must hide the lock card too, and before the band \
             because nothing -- core-drawn or not -- may sit on the one strip the human reads \
             this session's colour from"
        );
    }
}
