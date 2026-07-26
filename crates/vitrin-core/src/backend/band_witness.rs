// SPDX-License-Identifier: MPL-2.0
//! The trusted-band witness (issue #139): the *negative* half of issue #85's
//! unspoofability property, stated in numbers a harness may hold without ever
//! holding the session secret.
//!
//! # The property, and the half that can be gated
//!
//! [`ConsentSurface::composite_trust_band`] paints this session's secret
//! colour over the top [`TRUST_BAND_HEIGHT`] rows of every human-visible
//! frame, after the client's surface and after the prompt, and never on the
//! capture path. Two claims follow, and only one of them is CI-able:
//!
//! - **The negative half.** A confined app's own rendering can never reach
//!   those rows on the human-visible output, and never reaches the capture
//!   path at all. This needs no secret to check: it is an *invariance*
//!   statement, and this module measures it.
//! - **The positive half.** A human at a real screen, having learned the
//!   colour off the band, can tell a genuine prompt's frame from a forgery.
//!   That needs an eye and a display; there is no automatable form of it, and
//!   pretending otherwise is how a gate comes to be cited for a property it
//!   never checked. What exists today is one clause of `shim/docs/firefox.md`
//!   §9 ("visible in the trusted indicator band"), not a written procedure for
//!   checking a forgery against it; `docs/plan/01-phase-1-mvp.md` §5 records
//!   that gap under "the fourth direction" rather than leaving it implied.
//!
//! # The rule this module holds to, which is stricter than "export no pixels"
//!
//! [`super::super::consent::indicator`] already forbids the secret reaching
//! any descriptor or file, because the confined realm runs as the core's own
//! uid and can read `/proc/<core>/fd`. That rule alone is **not enough** for a
//! witness, and the near-miss is worth writing down because it looks safe:
//!
//! > A tempting field to export is "the band's rows equal the realm view's
//! > rows beneath them". No pixel leaves, no digest of the secret leaves — one
//! > boolean. It is an **oracle**: a confined app paints candidate colour `C`
//! > over its whole surface and reads the bit, which is exactly `S == C`.
//! > [`TrustedIndicator::generate`] scales each channel into `[64, 255]`, so
//! > `S` has at most `192³ ≈ 7.1 M` values; the app recovers the session
//! > secret by search.
//!
//! So the rule here is: **every field this module exports must be a constant
//! function of the run, independent of the *value* of the indicator.** Then a
//! same-uid attacker who steals the whole channel learns nothing, and the
//! question "is this channel a leak" has an answer that does not depend on how
//! hard the search is. [`a_report_does_not_depend_on_the_bands_colour`] checks
//! it mechanically: two runs identical in every way but the band's colour must
//! produce byte-identical reports. It is checked twice, over both overlay
//! states the real-app gate samples — prompt-free composites, where the band is
//! the frame's only indicator-coloured pixels, and
//! [`a_report_does_not_depend_on_the_bands_colour_with_a_card_up`], where the
//! composite also carries the scrim, the opaque card and the *other* painting
//! of the same secret, the trusted ring.
//!
//! The one field derived from real pixels, [`BandReport::probe_fnv`], is a
//! digest of the **realm view** — the composition an observe grant may capture
//! anyway — and of rows *below* the band. The indicator is never in the realm
//! view (that is the whole capture argument in [`super`]), so that digest is
//! client-owned bytes the harness already holds.
//!
//! # Why not a commitment scheme
//!
//! Issue #139 sketches one: the core publishes a hash or derived witness the
//! harness checks against pixels it cannot itself produce. Rejected, on three
//! independent grounds, any one of which is fatal:
//!
//! 1. **The secret is too small to commit to.** `H(S)` over a 22.8-bit space
//!    is invertible by exhaustive search in well under a second, on the same
//!    machine, by the same-uid app that reads it out of `/proc`. A commitment
//!    to a 22.8-bit secret is the secret with extra steps — the same reasoning
//!    [`super::super::consent::indicator`] uses to refuse writing the colour to
//!    a log.
//! 2. **Salting it makes it uncheckable.** `H(S ‖ nonce)` with a retained
//!    nonce cannot be verified by a harness that never holds a band pixel, and
//!    publishing the nonce collapses it to (1). A harness that *did* hold band
//!    pixels would have the secret in its address space and one `write(2)`
//!    from a file, which is the arrangement the whole design refuses.
//! 3. **Replay.** Even a scheme immune to (1) and (2) publishes the witness on
//!    a channel a same-uid observer can reach, and the human's check is
//!    supposed to require *seeing the screen*. A witness the app can read is a
//!    witness the app can quote.
//!
//! The invariance measured here needs none of that, because it never mentions
//! the colour at all.
//!
//! # What a reading of this witness does not establish
//!
//! - **Nothing about the colour.** A build whose [`TrustedIndicator`] were a
//!   hard-coded constant, or minted after the listener bound, would satisfy
//!   every field below. That is `indicator.rs`'s own tests and `run_session`'s
//!   ordering, and it stays there.
//! - **Nothing about the nested backend.** This is fed from the headless
//!   backend's CPU composite. The nested backend's two presentation paths are
//!   held against each other by `winit.rs`'s
//!   `no_presentation_path_can_drop_the_trusted_band`.
//! - **Nothing a human would call "unforgeable".** See the split at the top.
//!
//! [`ConsentSurface::composite_trust_band`]: crate::consent::ConsentSurface::composite_trust_band
//! [`TRUST_BAND_HEIGHT`]: crate::consent::TRUST_BAND_HEIGHT
//! [`TrustedIndicator`]: crate::consent::TrustedIndicator
//! [`TrustedIndicator::generate`]: crate::consent::TrustedIndicator
//! [`a_report_does_not_depend_on_the_bands_colour`]: tests::a_report_does_not_depend_on_the_bands_colour
//! [`a_report_does_not_depend_on_the_bands_colour_with_a_card_up`]: tests::a_report_does_not_depend_on_the_bands_colour_with_a_card_up

use crate::consent::TRUST_BAND_HEIGHT;

/// FNV-1a-64 offset basis and prime. Chosen over a cryptographic digest for
/// one reason: the harness that checks [`BandReport::probe_fnv`] is the
/// dependency-free stdlib-Python integration suite (D8), which has to be able
/// to recompute it over the same bytes at a sane speed, and it must not
/// acquire a dependency to do so. Nothing here is a security property — the
/// digest covers client-owned pixels the agent may capture anyway — so
/// collision resistance is not what is being bought; agreement between two
/// independent readers of the same bytes is.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// One reading of the witness. Every field is secret-independent; see the
/// module docs for why that is the rule rather than "no pixels".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BandReport {
    /// Composites this witness evaluated, over the life of the session.
    pub composites: u64,
    /// Composites (after the first) at which the human-visible output's band
    /// rows differed, byte for byte, from the previous composite's.
    ///
    /// **Zero in a correct session, always.** This is the property: the band's
    /// rows are invariant under everything the client does.
    pub band_changes: u64,
    /// Composites (after the first) at which the **realm view's** probe strip
    /// — the [`TRUST_BAND_HEIGHT`] rows immediately *below* the band — changed.
    ///
    /// The counterweight to `band_changes`. A witness that never looked would
    /// report zero for both, and "zero changes" would then be evidence of
    /// nothing; this says how many times the client really did repaint the
    /// pixels nearest the band while the band itself did not move.
    pub probe_changes: u64,
    /// At the latest composite: the human-visible output below the band is
    /// byte-identical to the realm view below the band.
    ///
    /// True whenever no prompt is up (the prompt's scrim and trusted ring are
    /// exactly what makes it false, legitimately). Its job is to refuse the
    /// vacuous reading of `band_changes == 0`: a frozen or erased output
    /// framebuffer would hold its band rows constant too, and would fail here.
    pub tracks_view: bool,
    /// At the latest composite: every pixel of the band's rows is the same
    /// fully opaque colour.
    ///
    /// Catches the partial overdraw and the alpha-blend — a band composited
    /// *under* client content, or blended with it, picks up the client's
    /// variation and stops being uniform.
    pub band_uniform: bool,
    /// Composites the witness could not evaluate because the buffers it was
    /// handed were not `width * height * 4`. **Zero in a correct session**; a
    /// non-zero count means the numbers above are about fewer frames than the
    /// session actually composited, which is exactly the sort of quiet gap a
    /// gate must not be allowed to read as success.
    pub refusals: u64,
    /// The band's effective height in rows (`TRUST_BAND_HEIGHT`, clamped to
    /// the view).
    pub band_h: u32,
    /// The view's dimensions at the latest composite.
    pub view_w: u32,
    pub view_h: u32,
    /// FNV-1a-64 over the **realm view's** probe strip at the latest
    /// composite. Client-owned bytes, so the harness can recompute it from its
    /// own `--capture-dump` read and check that this witness was evaluated on
    /// the frame the harness is looking at.
    pub probe_fnv: u64,
}

impl std::fmt::Display for BandReport {
    /// The channel's wire form: ten space-separated ASCII fields, no payload,
    /// no descriptor. Rendered here rather than at the call site so the one
    /// place the report becomes bytes is the one place to audit.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {} {} {} {} {} {} {:016x}",
            self.composites,
            self.band_changes,
            self.probe_changes,
            u8::from(self.tracks_view),
            u8::from(self.band_uniform),
            self.refusals,
            self.band_h,
            self.view_w,
            self.view_h,
            self.probe_fnv,
        )
    }
}

/// Accumulates [`BandReport`] over a session's composites.
///
/// Fed from [`super::headless::HeadlessOutput::present`] with the two buffers
/// that composite already has in hand — the realm view and the human-visible
/// output — so it measures the bytes that are actually presented rather than a
/// second composition that could agree with them today and drift tomorrow.
pub(crate) struct BandWitness {
    /// The previous composite's band rows.
    ///
    /// **The one place in this module that holds indicator-coloured bytes**,
    /// and it is retained rather than digested on purpose: an exact comparison
    /// cannot miss a change, and a digest of the band would be a 22.8-bit
    /// commitment sitting in a struct one careless `Debug` away from a log
    /// (module docs). It is compared and overwritten; nothing reads it out.
    /// `BandWitness` deliberately derives no `Debug` for the same reason
    /// `TrustedIndicator` hand-writes a redacting one.
    previous_band: Option<Vec<u8>>,
    /// The previous composite's probe-strip digest — realm view, so
    /// indicator-free by construction.
    previous_probe: Option<u64>,
    composites: u64,
    band_changes: u64,
    probe_changes: u64,
    refusals: u64,
    tracks_view: bool,
    band_uniform: bool,
    band_h: u32,
    view_w: u32,
    view_h: u32,
    probe_fnv: u64,
}

impl BandWitness {
    pub(crate) fn new() -> Self {
        Self {
            previous_band: None,
            previous_probe: None,
            composites: 0,
            band_changes: 0,
            probe_changes: 0,
            refusals: 0,
            // Before the first composite there is nothing to be true about.
            // False rather than true so a report read before any frame cannot
            // be mistaken for a passing one.
            tracks_view: false,
            band_uniform: false,
            band_h: 0,
            view_w: 0,
            view_h: 0,
            probe_fnv: 0,
        }
    }

    /// Evaluate one composite.
    ///
    /// `view` is [`crate::scene::Scene::compose`]'s output and `output` is
    /// [`super::human_visible_from_view`]'s, both tightly packed RGBA8888 of
    /// `width * height * 4` bytes. A pair that is not that size is **counted
    /// and skipped** rather than panicking or silently ignored: this runs
    /// inside the compositor, where taking the session down over a witness
    /// would be absurd, and a silent skip would let `band_changes == 0` mean
    /// "nothing was measured".
    pub(crate) fn observe(&mut self, view: &[u8], output: &[u8], width: u32, height: u32) {
        let expected = width as usize * height as usize * 4;
        if expected == 0 || view.len() != expected || output.len() != expected {
            self.refusals += 1;
            return;
        }
        let band_h = TRUST_BAND_HEIGHT.min(height);
        let band_bytes = width as usize * band_h as usize * 4;
        // The probe strip: the same number of rows again, immediately below
        // the band, clamped to the view. Those rows are where a client would
        // paint to make its counterfeit band look like it continues, so they
        // are the most interesting client-owned pixels to correlate on.
        let probe_end = (band_bytes * 2).min(expected);

        self.composites += 1;
        self.band_h = band_h;
        self.view_w = width;
        self.view_h = height;

        let band = &output[..band_bytes];
        if let Some(previous) = self.previous_band.as_deref() {
            if previous != band {
                self.band_changes += 1;
            }
        }
        match self.previous_band.as_mut() {
            Some(buffer) => {
                buffer.clear();
                buffer.extend_from_slice(band);
            }
            None => self.previous_band = Some(band.to_vec()),
        }

        self.probe_fnv = fnv1a64(&view[band_bytes..probe_end]);
        if let Some(previous) = self.previous_probe {
            if previous != self.probe_fnv {
                self.probe_changes += 1;
            }
        }
        self.previous_probe = Some(self.probe_fnv);

        self.tracks_view = output[band_bytes..] == view[band_bytes..];
        self.band_uniform = band_bytes > 0
            && band[3] == 0xff
            && band.chunks_exact(4).all(|pixel| pixel == &band[..4]);
    }

    pub(crate) fn report(&self) -> BandReport {
        BandReport {
            composites: self.composites,
            band_changes: self.band_changes,
            probe_changes: self.probe_changes,
            tracks_view: self.tracks_view,
            band_uniform: self.band_uniform,
            refusals: self.refusals,
            band_h: self.band_h,
            view_w: self.view_w,
            view_h: self.view_h,
            probe_fnv: self.probe_fnv,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{ConsentSurface, TrustedIndicator};

    const W: u32 = 40;
    const H: u32 = 24;

    /// A realm view filled with one colour — a confined app painting its whole
    /// surface, which is the strongest counterfeit available to something that
    /// cannot see the colour it would have to match.
    fn client_view(rgb: [u8; 3]) -> Vec<u8> {
        [rgb[0], rgb[1], rgb[2], 0xff]
            .repeat(W as usize * H as usize)
            .to_vec()
    }

    /// The real human-visible composite: the shared overlay step both backends
    /// call, with a surface carrying `indicator`. Deliberately not a hand-rolled
    /// band — a test that painted its own would pass over a `composite_trust_band`
    /// that had been deleted.
    fn human_visible(view: &[u8], indicator: TrustedIndicator) -> Vec<u8> {
        let mut surface = ConsentSurface::new(indicator);
        crate::backend::human_visible_from_view(view.to_vec(), &mut surface, W, H)
    }

    fn run(indicator: TrustedIndicator, views: &[Vec<u8>]) -> BandReport {
        let mut witness = BandWitness::new();
        for view in views {
            let output = human_visible(view, indicator);
            witness.observe(view, &output, W, H);
        }
        witness.report()
    }

    /// A view the fixture card — and, more to the point, the trusted ring
    /// around it — fits inside, so a composite taken with a prompt up really
    /// carries the second painting of the session secret. `W`x`H` above is
    /// deliberately tiny and would clip the ring away entirely.
    const CARD_W: u32 = 640;
    const CARD_H: u32 = 480;

    fn flat(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        [rgb[0], rgb[1], rgb[2], 0xff].repeat(width as usize * height as usize)
    }

    /// One human-visible composite with the fixture prompt up, at a size the
    /// card fits. Used to prove the trusted ring is actually in the frames
    /// [`run_across_a_prompt`] samples.
    fn prompt_up_composite(indicator: TrustedIndicator) -> Vec<u8> {
        let mut surface = ConsentSurface::new(indicator);
        surface.show_for_test(crate::consent::tests::prompt_fixture());
        crate::backend::human_visible_from_view(
            flat(CARD_W, CARD_H, [0x00, 0x00, 0x00]),
            &mut surface,
            CARD_W,
            CARD_H,
        )
    }

    /// Three composites through the real overlay step, across a prompt's whole
    /// life: none up, one up, lowered again — with the client repainting its
    /// entire surface between each, exactly as `click-target` does. This is the
    /// sequence the real-app gate's session goes through before it reads the
    /// witness, so it is the sequence the secret-independence rule has to hold
    /// over.
    fn run_across_a_prompt(indicator: TrustedIndicator) -> BandReport {
        let mut surface = ConsentSurface::new(indicator);
        let mut witness = BandWitness::new();
        let views = [
            flat(CARD_W, CARD_H, [0x00, 0x00, 0x00]),
            flat(CARD_W, CARD_H, [0xff, 0x00, 0x00]),
            flat(CARD_W, CARD_H, [0x00, 0xff, 0x00]),
        ];
        for (index, view) in views.iter().enumerate() {
            match index {
                1 => surface.show_for_test(crate::consent::tests::prompt_fixture()),
                2 => surface.dismiss_for_test(),
                _ => {}
            }
            let output =
                crate::backend::human_visible_from_view(view.clone(), &mut surface, CARD_W, CARD_H);
            witness.observe(view, &output, CARD_W, CARD_H);
        }
        witness.report()
    }

    /// **The leak argument, mechanically.** Two sessions identical in every way
    /// but the value of the session secret must produce byte-identical reports.
    ///
    /// This is what makes "the harness cannot learn the colour, and neither can
    /// a same-uid app that steals the channel" a checked fact rather than a
    /// reading of the code. It is strictly stronger than "no pixels are
    /// exported": the near-miss the module docs describe — exporting
    /// `band == view` as one boolean — passes "no pixels" and fails here,
    /// because that bit is `S == C` and moves with `S`.
    #[test]
    fn a_report_does_not_depend_on_the_bands_colour() {
        let views = vec![
            client_view([0x00, 0x00, 0x00]),
            client_view([0xff, 0x00, 0x00]),
            client_view([0x11, 0x22, 0x33]),
        ];
        let first = run(TrustedIndicator::from_rgb(0x40, 0x41, 0x42), &views);
        let second = run(TrustedIndicator::from_rgb(0xfe, 0xfd, 0xfc), &views);
        assert_eq!(first, second, "a report must not move with the secret");
        assert_eq!(first.to_string(), second.to_string());
    }

    /// **The leak argument again, over the overlay states the gate actually
    /// samples.**
    ///
    /// [`a_report_does_not_depend_on_the_bands_colour`] drives composites with
    /// no prompt up, so the band is the only indicator-coloured thing in the
    /// frame. That is narrower than the real-app gate's session:
    /// `tests/integration/test_real_trust_band.py` runs under
    /// `--consent=interactive`, answers a real petition, and only then reads
    /// the witness — so composites it counts include ones carrying the scrim,
    /// the opaque card, and the **trusted ring**, which is the *other* painting
    /// of the same secret and lands well inside the view at this size.
    ///
    /// A report that moved with the colour only when a card was up would be
    /// exactly as much of an oracle as one that moved always, and the narrower
    /// test would not see it. So the rule is checked again here rather than
    /// argued from the prompt-free case.
    #[test]
    fn a_report_does_not_depend_on_the_bands_colour_with_a_card_up() {
        let dim = TrustedIndicator::from_rgb(0x40, 0x41, 0x42);
        let bright = TrustedIndicator::from_rgb(0xfe, 0xfd, 0xfc);

        // The guard against a vacuous version of this test, stated as the
        // property rather than as the geometry that produces it: BELOW the
        // band's rows, two prompt-up composites of the same view under two
        // different secrets must differ. Above the band they always differ, so
        // comparing whole frames would prove nothing. If this fails, the ring
        // is not in these frames — the card no longer fits, or the ring clipped
        // away — and the equality asserted next would hold because there was
        // nothing here to leak.
        let band_bytes = CARD_W as usize * TRUST_BAND_HEIGHT as usize * 4;
        let dim_frame = prompt_up_composite(dim);
        let bright_frame = prompt_up_composite(bright);
        assert_ne!(
            dim_frame[band_bytes..],
            bright_frame[band_bytes..],
            "below the band, two prompt-up composites must differ by the trusted \
             ring's colour; if they do not, this run samples no indicator pixel \
             the prompt-free run does not, and checks nothing new"
        );

        let first = run_across_a_prompt(dim);
        let second = run_across_a_prompt(bright);
        assert_eq!(
            first, second,
            "a report must not move with the secret, prompt up or not"
        );
        assert_eq!(first.to_string(), second.to_string());
        // The ordinary fields, so a report that matched only because BOTH runs
        // had collapsed to nothing cannot pass. That a card was really up is
        // the `assert_ne!` above, not these; `tracks_view` being true here says
        // only that the last composite is after the dismissal, which is where
        // the sequence leaves it.
        assert_eq!(first.composites, 3);
        assert_eq!(first.band_changes, 0, "client content reached the band");
        assert!(
            first.tracks_view,
            "the prompt was lowered before the report"
        );
        assert_eq!(first.refusals, 0);
    }

    /// The property, over the exact scenario the real-app gate drives: a
    /// client repaints its entire surface — the band's rows included — in a
    /// different colour, twice, and the human-visible band rows never move.
    #[test]
    fn a_whole_surface_repaint_never_reaches_the_band() {
        let report = run(
            TrustedIndicator::for_test(),
            &[
                client_view([0x00, 0x00, 0x00]),
                client_view([0xff, 0x00, 0x00]),
                client_view([0x00, 0xff, 0x00]),
            ],
        );
        assert_eq!(report.composites, 3);
        assert_eq!(report.band_changes, 0, "client content reached the band");
        assert_eq!(
            report.probe_changes, 2,
            "the client's own rows below the band must be seen changing, or \
             `band_changes == 0` is evidence of nothing"
        );
        assert!(report.tracks_view);
        assert!(report.band_uniform);
        assert_eq!(report.refusals, 0);
        assert_eq!(report.band_h, TRUST_BAND_HEIGHT);
    }

    /// **The gate's discriminating power, pinned in-process** (plan §5 D12
    /// item 4: where a criterion is a computed metric, pin the metric against
    /// one input it must accept and one from the class it claims to reject).
    ///
    /// The rejected class is exactly the regression the property exists for:
    /// the band not composited, so the human-visible output *is* the client's
    /// frame in those rows. Modelled by handing the witness the realm view as
    /// the human-visible output — which is what `human_visible_from_view`
    /// would return if `composite_trust_band` were a no-op.
    #[test]
    fn a_band_that_did_not_overdraw_the_client_is_counted_as_a_change() {
        let mut witness = BandWitness::new();
        for view in [
            client_view([0x00, 0x00, 0x00]),
            client_view([0xff, 0x00, 0x00]),
        ] {
            witness.observe(&view, &view, W, H);
        }
        let report = witness.report();
        assert_eq!(
            report.band_changes, 1,
            "a client repaint that reached the band must be counted"
        );
        // ...and it is still `tracks_view`, which is why that field can never
        // stand in for the property: an unpainted band tracks the view
        // perfectly.
        assert!(report.tracks_view);
    }

    /// A band blended with client content rather than overdrawing it keeps
    /// changing *and* stops being uniform, so either field catches it. Pinned
    /// because a 50% blend is the plausible way someone "softens" the band.
    #[test]
    fn a_blended_band_is_neither_stable_nor_uniform() {
        let mut witness = BandWitness::new();
        let indicator = TrustedIndicator::for_test();
        for rgb in [[0x00, 0x00, 0x00], [0xff, 0xff, 0xff]] {
            let view = client_view(rgb);
            let mut output = view.clone();
            let band_bytes = W as usize * TRUST_BAND_HEIGHT as usize * 4;
            // Half the band left as the client's pixels: a partial overdraw,
            // which uniformity is the check for.
            let colour = indicator.color();
            for pixel in output[..band_bytes / 2].chunks_exact_mut(4) {
                pixel.copy_from_slice(&colour);
            }
            witness.observe(&view, &output, W, H);
        }
        let report = witness.report();
        assert_eq!(report.band_changes, 1);
        assert!(
            !report.band_uniform,
            "a partly-overdrawn band is not uniform"
        );
    }

    /// A human-visible output that stopped tracking the realm view — the
    /// frozen or erased framebuffer — holds its band rows constant and so
    /// passes the property vacuously. `tracks_view` is what refuses it.
    #[test]
    fn an_erased_human_visible_frame_is_refused_by_tracks_view() {
        let mut witness = BandWitness::new();
        let indicator = TrustedIndicator::for_test();
        let frozen = human_visible(&client_view([0x00, 0x00, 0x00]), indicator);
        for rgb in [[0x00, 0x00, 0x00], [0xff, 0x00, 0x00]] {
            witness.observe(&client_view(rgb), &frozen, W, H);
        }
        let report = witness.report();
        assert_eq!(report.band_changes, 0, "a frozen output never changes");
        assert!(
            !report.tracks_view,
            "a frozen human-visible frame must not read as a live one"
        );
        assert_eq!(report.probe_changes, 1, "the client did repaint");
    }

    /// Buffers that are not `width * height * 4` are counted, never silently
    /// dropped: a witness that quietly measured nothing would report the
    /// passing values for every field it does report.
    #[test]
    fn a_mismatched_buffer_is_counted_as_a_refusal() {
        let mut witness = BandWitness::new();
        witness.observe(&[], &[], W, H);
        witness.observe(&client_view([0; 3]), &[], W, H);
        witness.observe(&client_view([0; 3]), &client_view([0; 3]), 0, 0);
        let report = witness.report();
        assert_eq!(report.refusals, 3);
        assert_eq!(report.composites, 0);
        assert!(!report.tracks_view);
        assert!(!report.band_uniform);
    }

    /// A report read before any composite must not look like a passing one.
    #[test]
    fn a_witness_that_has_seen_nothing_reports_nothing_passing() {
        let report = BandWitness::new().report();
        assert_eq!(report.composites, 0);
        assert!(!report.tracks_view);
        assert!(!report.band_uniform);
        // A zero digest rather than the FNV offset basis: nothing was hashed,
        // and a report that opened with the digest of the empty string would be
        // indistinguishable from one taken over an empty probe strip.
        assert_eq!(report.to_string(), "0 0 0 0 0 0 0 0 0 0000000000000000");
    }

    /// The wire form is ten fields of bounded ASCII, so the reply cannot become
    /// a pixel channel by accident: `MAX_LINE` is 128 bytes and a 640x480
    /// band is 20 480.
    #[test]
    fn the_wire_form_is_ten_scalar_fields() {
        let report = run(TrustedIndicator::for_test(), &[client_view([1, 2, 3])]);
        let line = report.to_string();
        assert_eq!(line.split(' ').count(), 10, "{line}");
        assert!(line.is_ascii());
        assert!(
            line.len() + "band ".len() < crate::consent::injector::MAX_LINE,
            "the band reply must fit the channel's line bound: {line}"
        );
    }
}
