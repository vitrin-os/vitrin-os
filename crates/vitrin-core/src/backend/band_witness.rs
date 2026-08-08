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
//! [`BandReport::realm`] joins it under the same rule and passes: a realm id
//! is configuration (`realm.toml`), fixed before the indicator is minted, and
//! constant across two runs that differ only in the colour.
//!
//! # Which realm a report is about (WS-E.1.3, issue #209)
//!
//! **The realm bound to the output, and it says so.** A session may hold up
//! to [`crate::realm::MAX_REALMS`] realms, each with its own scene and its own
//! capture, but the human-visible framebuffer shows exactly one of them —
//! and every field below is a statement about *that* one:
//!
//! - [`BandReport::tracks_view`] compares the human-visible output against
//!   **the bound realm's** view, the two buffers
//!   [`super::headless::HeadlessOutput::present`] composites from. Against any
//!   other realm it would read `false` forever and mean nothing.
//! - [`BandReport::probe_changes`] and [`BandReport::probe_fnv`] digest the
//!   bound realm's rows below the band.
//! - [`BandReport::band_changes`] and [`BandReport::band_uniform`] are about
//!   the output's band rows, which belong to no realm at all — they are the
//!   core's own overdraw.
//!
//! Before this the assertion was about "the realm view" with one realm to
//! mean, so it was true by having nothing to be ambiguous about.
//! `tests/integration/test_real_trust_band.py`'s `band_changes == 0` would
//! otherwise become a zero over an undefined comparison — a number that
//! cannot be wrong because it is not about anything.
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
/// module docs for why that is the rule rather than "no pixels" — and for
/// **which realm** the pixel-derived fields are about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BandReport<'a> {
    /// The realm bound to the output at the latest composite, or `None`
    /// before any realm has attached.
    ///
    /// Not decoration: [`Self::tracks_view`], [`Self::probe_changes`] and
    /// [`Self::probe_fnv`] are statements about *this* realm's view, and a
    /// reader that assumed another realm would be comparing a number against
    /// the wrong picture. Secret-independent (it comes from `realm.toml`), so
    /// it does not weaken the rule the module docs set.
    pub realm: Option<&'a str>,
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
    /// At the latest composite: the human-visible output below **the band and
    /// the status strip** is byte-identical to the realm view under the same
    /// rows.
    ///
    /// True whenever no prompt is up (the prompt's scrim and trusted ring are
    /// exactly what makes it false, legitimately). Its job is to refuse the
    /// vacuous reading of `band_changes == 0`: a frozen or erased output
    /// framebuffer would hold its band rows constant too, and would fail here.
    ///
    /// **The strip's rows are excluded rather than ignored** (WS-E.2.3, issue
    /// #215). A session with `--status` on overdraws [`Self::strip_h`] rows of
    /// client content by design, so comparing them would make this read `false`
    /// forever and turn the field into a constant that reports nothing. The
    /// rows are not dropped from the witness — [`Self::strip_changes`] is what
    /// they moved to — and with `--status` off `strip_h` is `0` and this
    /// comparison is byte-for-byte what it was before the strip existed.
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
    /// The status strip's effective height in rows (WS-E.2.3): the session's
    /// `--status-height`, clamped to the view, and **`0` when `--status` is
    /// off**. Read from [`crate::status::StatusStrip::height`] rather than
    /// restated, so this witness cannot disagree with the compositor about
    /// which rows the strip owns.
    pub strip_h: u32,
    /// Composites (after the first) at which the human-visible output's
    /// **strip** rows changed, byte for byte.
    ///
    /// The counterpart to [`Self::band_changes`], and the reason the pair is
    /// worth having: `band_changes == 0` is the property, and a reading in
    /// which *both* counters are zero is a reading in which nothing was
    /// measured. With `--status` on and a clock ticking, this is expected to be
    /// non-zero while `band_changes` stays exactly `0`; with `--status` off it
    /// is `0` because there are no strip rows.
    ///
    /// Secret-independent, exactly like every other field: the strip is drawn
    /// from a snapshot of the clock, the battery and a `realm.toml` id, none of
    /// which is a function of the indicator's colour.
    pub strip_changes: u64,
    /// FNV-1a-64 over the **realm view's** probe strip at the latest
    /// composite. Client-owned bytes, so the harness can recompute it from its
    /// own `--capture-dump` read and check that this witness was evaluated on
    /// the frame the harness is looking at.
    pub probe_fnv: u64,
}

impl std::fmt::Display for BandReport<'_> {
    /// The channel's wire form: thirteen space-separated ASCII fields, no
    /// payload, no descriptor. Rendered here rather than at the call site so
    /// the one place the report becomes bytes is the one place to audit.
    ///
    /// The bound realm's id is eleventh, after the digest, so the ten fields
    /// that predate WS-E.1.3 keep their positions; `-` when no realm is
    /// bound. WS-E.2.3's two strip fields are appended **after** it for the
    /// same reason it was appended after the digest: every position an existing
    /// reader indexes stays where it was, and a harness that has not been taught
    /// about the strip reads the same numbers it read before.
    ///
    /// **Why it cannot turn this line into a payload**, stated as the two
    /// separate facts it actually rests on:
    ///
    /// - *Provenance.* The id comes from `realm.toml`, which the operator
    ///   writes. No peer of this channel, and no confined client, contributes
    ///   a byte of it — which is the property the module docs' rule is about.
    ///   Every other field is a core-owned counter, flag, geometry value or
    ///   digest, so the whole line is core-authored.
    /// - *Length.* A realm id is at most **64 bytes** over `[A-Za-z0-9._-]`
    ///   and never `.` or `..` (`crate::realm::validate_realm`, which defers
    ///   to `vitrin_ipc::paths::shim_runtime_dir_in` so the rule has one
    ///   definition). It is **not** the `[a-z0-9-]` this comment used to
    ///   claim: uppercase, `.` and `_` are all legal, and the real bound is
    ///   64 rather than the 7 bytes `realm-0` happens to occupy.
    ///
    /// Sixty-five bytes (the id plus its separator) is more than half the
    /// channel's 128-byte budget, so `the_wire_form_is_eleven_scalar_fields`
    /// measures the line at the **loader's** maximum rather than at a
    /// fixture's, and records what is left over for the counters.
    ///
    /// The counters are `u64` and are the one thing on this line without a
    /// short bound. At their type maximum the line would not fit `MAX_LINE`
    /// with *or* without a realm id (137 bytes for the first ten fields
    /// alone), and that is stated rather than papered over: what makes the
    /// bound hold is that they count this session's composites, and the test
    /// pins the headroom that leaves.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {} {} {} {} {} {} {:016x} {} {} {}",
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
            self.realm.unwrap_or("-"),
            self.strip_h,
            self.strip_changes,
        )
    }
}

/// Accumulates [`BandReport`] over a session's composites.
///
/// Fed from [`super::headless::HeadlessOutput::present`] with the two buffers
/// that composite already has in hand — **the bound realm's** view and the
/// human-visible output — so it measures the bytes that are actually
/// presented rather than a second composition that could agree with them
/// today and drift tomorrow.
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
    /// The previous composite's status-strip rows, from the human-visible
    /// output. Retained rather than digested for the reason `previous_band` is
    /// — an exact comparison cannot miss a change — but with none of the same
    /// caution behind it: these bytes are the strip's own raster, which holds no
    /// secret and is a function of the clock, the battery and a `realm.toml`
    /// id.
    previous_strip: Option<Vec<u8>>,
    composites: u64,
    band_changes: u64,
    probe_changes: u64,
    strip_changes: u64,
    refusals: u64,
    tracks_view: bool,
    band_uniform: bool,
    band_h: u32,
    strip_h: u32,
    view_w: u32,
    view_h: u32,
    probe_fnv: u64,
    /// The realm bound to the output at the latest composite. Owned rather
    /// than borrowed because the witness outlives any one composite; handed
    /// out as a `&str` in [`BandReport::realm`].
    realm: Option<crate::grants::RealmId>,
}

impl BandWitness {
    pub(crate) fn new() -> Self {
        Self {
            realm: None,
            previous_band: None,
            previous_probe: None,
            previous_strip: None,
            composites: 0,
            band_changes: 0,
            probe_changes: 0,
            strip_changes: 0,
            refusals: 0,
            // Before the first composite there is nothing to be true about.
            // False rather than true so a report read before any frame cannot
            // be mistaken for a passing one.
            tracks_view: false,
            band_uniform: false,
            band_h: 0,
            strip_h: 0,
            view_w: 0,
            view_h: 0,
            probe_fnv: 0,
        }
    }

    /// Evaluate one composite.
    ///
    /// `view` is **the bound realm's** [`crate::scene::Scene::compose`]
    /// output and `output` is [`super::human_visible_from_view`]'s, both
    /// tightly packed RGBA8888 of `width * height * 4` bytes; `realm` names
    /// the realm `view` belongs to, so the report can say what it is about
    /// (module docs). A pair that is not that size is **counted and skipped**
    /// rather than panicking or silently ignored: this runs inside the
    /// compositor, where taking the session down over a witness would be
    /// absurd, and a silent skip would let `band_changes == 0` mean "nothing
    /// was measured".
    ///
    /// The realm is recorded **before** the size check, deliberately: a
    /// refused composite is still a composite of some realm, and a report
    /// whose counters said "refusals: 1" while naming no realm would hide
    /// which one.
    ///
    /// `strip_h` is the status strip's height in rows for this composite
    /// ([`crate::status::StatusStrip::height`], `0` when `--status` is off). It
    /// is a parameter rather than a constant because the strip's height is the
    /// operator's, and it is *this* value rather than a re-derivation so the
    /// witness and the compositor cannot disagree about which rows the strip
    /// owns.
    pub(crate) fn observe(
        &mut self,
        view: &[u8],
        output: &[u8],
        width: u32,
        height: u32,
        strip_h: u32,
        realm: Option<&crate::grants::RealmId>,
    ) {
        if self.realm.as_ref() != realm {
            self.realm = realm.cloned();
        }
        let expected = width as usize * height as usize * 4;
        if expected == 0 || view.len() != expected || output.len() != expected {
            self.refusals += 1;
            return;
        }
        let band_h = TRUST_BAND_HEIGHT.min(height);
        let band_bytes = width as usize * band_h as usize * 4;
        // The strip's rows, immediately below the band and clamped to what is
        // left of the view.
        let strip_h = strip_h.min(height - band_h);
        let strip_end = band_bytes + width as usize * strip_h as usize * 4;
        // The probe strip: the same number of rows again, immediately below
        // the band, clamped to the view. Those rows are where a client would
        // paint to make its counterfeit band look like it continues, so they
        // are the most interesting client-owned pixels to correlate on.
        let probe_end = (band_bytes * 2).min(expected);

        self.composites += 1;
        self.band_h = band_h;
        self.strip_h = strip_h;
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

        // The strip's own rows, on the human-visible output. Measured whether
        // or not a strip is up: with `--status` off this is an empty slice, the
        // comparison is trivially equal, and the counter stays 0 — which is the
        // honest reading of "there is no strip", not a silent skip.
        let strip = &output[band_bytes..strip_end];
        if let Some(previous) = self.previous_strip.as_deref() {
            if previous != strip {
                self.strip_changes += 1;
            }
        }
        match self.previous_strip.as_mut() {
            Some(buffer) => {
                buffer.clear();
                buffer.extend_from_slice(strip);
            }
            None => self.previous_strip = Some(strip.to_vec()),
        }

        self.tracks_view = output[strip_end..] == view[strip_end..];
        self.band_uniform = band_bytes > 0
            && band[3] == 0xff
            && band.chunks_exact(4).all(|pixel| pixel == &band[..4]);
    }

    pub(crate) fn report(&self) -> BandReport<'_> {
        BandReport {
            realm: self.realm.as_ref().map(|realm| realm.as_str()),
            composites: self.composites,
            band_changes: self.band_changes,
            probe_changes: self.probe_changes,
            strip_changes: self.strip_changes,
            tracks_view: self.tracks_view,
            band_uniform: self.band_uniform,
            refusals: self.refusals,
            band_h: self.band_h,
            strip_h: self.strip_h,
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
        crate::backend::human_visible_from_view(
            view.to_vec(),
            &mut surface,
            &mut no_lock(),
            &mut no_status(),
            W,
            H,
            false,
        )
    }

    /// **The attention marker actually reaches the human's output** (WS-E.1.7,
    /// issue #232) — driven through the real shared composite, not a
    /// hand-rolled marker, for exactly the reason `human_visible` above gives:
    /// a test that painted its own would pass over a deleted
    /// `composite_attention_marker`.
    ///
    /// It had no reader at all when it landed: deleting the composite call left
    /// the whole suite green, so "the human sees something" — decision 8, the
    /// answer to *"what tells them what they just confirmed?"* — rested on
    /// nothing. Two directions, both load-bearing. Drawing it is what makes the
    /// gesture visible; **erasing** it is what stops the marker outliving the
    /// window and telling the human a defence is lifted when it has closed.
    #[test]
    fn the_attention_marker_reaches_the_human_output_and_leaves_the_band_alone() {
        let view = client_view([0x20, 0x20, 0x20]);
        let mut closed_surface = ConsentSurface::new(TrustedIndicator::for_test());
        let closed = crate::backend::human_visible_from_view(
            view.clone(),
            &mut closed_surface,
            &mut no_lock(),
            &mut no_status(),
            W,
            H,
            false,
        );
        let mut open_surface = ConsentSurface::new(TrustedIndicator::for_test());
        let open = crate::backend::human_visible_from_view(
            view.clone(),
            &mut open_surface,
            &mut no_lock(),
            &mut no_status(),
            W,
            H,
            true,
        );

        assert_ne!(
            closed, open,
            "an open attention window must change the human-visible output: the marker is \
             the only thing telling the human a defence is currently lifted"
        );

        // ...and it must not have got there by touching the trusted band. The
        // band's whole value is that its secret colour has exactly ONE correct
        // appearance; a band that sometimes carries a marker is a band whose
        // correct appearance is fuzzier, which is #215's rule for the clock and
        // battery applied here for the same reason.
        let band_bytes = (W as usize) * (crate::consent::TRUST_BAND_HEIGHT as usize) * 4;
        assert_eq!(
            closed[..band_bytes],
            open[..band_bytes],
            "the marker must be drawn BESIDE the trusted band, never in it"
        );
        assert_ne!(
            closed[band_bytes..],
            open[band_bytes..],
            "...which means the change has to be somewhere below the band -- if this fails \
             with the assertion above passing, the marker was drawn nowhere at all"
        );
    }

    /// The realm every fixture below binds to the output, so a report names
    /// one — the same well-known id `realm.toml` always carries.
    fn bound() -> crate::grants::RealmId {
        crate::grants::RealmId::new(crate::realm::WELL_KNOWN_REALM_ID)
    }

    fn run(indicator: TrustedIndicator, views: &[Vec<u8>]) -> BandReport<'static> {
        let mut witness = BandWitness::new();
        let realm = bound();
        for view in views {
            let output = human_visible(view, indicator);
            witness.observe(view, &output, W, H, 0, Some(&realm));
        }
        // The report borrows the witness, which dies here; the tests only
        // compare it, and every field but `realm` is `Copy` scalars — so the
        // realm is re-stated as a `'static` literal rather than kept
        // borrowed. Checked against the witness's own answer first, so this
        // convenience cannot mask a witness that named the wrong realm.
        let report = witness.report();
        assert_eq!(report.realm, Some(crate::realm::WELL_KNOWN_REALM_ID));
        BandReport {
            realm: Some(crate::realm::WELL_KNOWN_REALM_ID),
            ..report
        }
    }

    /// A view the fixture card — and, more to the point, the trusted ring
    /// around it — fits inside, so a composite taken with a prompt up really
    /// carries the second painting of the session secret. `W`x`H` above is
    /// deliberately tiny and would clip the ring away entirely.
    const CARD_W: u32 = 640;
    const CARD_H: u32 = 480;

    /// A lock surface with nothing raised. The witness's whole subject is the
    /// trusted band, and a raised lock would cover the rows below it — a
    /// different property, tested in `crate::lock`.
    /// A status strip that is off: `--status` is opt-in, so this is what every
    /// composite in this suite runs with unless it is testing the strip.
    fn no_status() -> crate::status::StatusStrip {
        crate::status::StatusStrip::new(crate::status::StatusConfig::off())
    }

    fn no_lock() -> crate::lock::LockSurface {
        crate::lock::LockSurface::new(TrustedIndicator::for_test())
    }

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
            &mut no_lock(),
            &mut no_status(),
            CARD_W,
            CARD_H,
            false,
        )
    }

    /// Three composites through the real overlay step, across a prompt's whole
    /// life: none up, one up, lowered again — with the client repainting its
    /// entire surface between each, exactly as `click-target` does. This is the
    /// sequence the real-app gate's session goes through before it reads the
    /// witness, so it is the sequence the secret-independence rule has to hold
    /// over.
    fn run_across_a_prompt(indicator: TrustedIndicator) -> BandReport<'static> {
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
            let output = crate::backend::human_visible_from_view(
                view.clone(),
                &mut surface,
                &mut no_lock(),
                &mut no_status(),
                CARD_W,
                CARD_H,
                false,
            );
            witness.observe(view, &output, CARD_W, CARD_H, 0, Some(&bound()));
        }
        let report = witness.report();
        assert_eq!(report.realm, Some(crate::realm::WELL_KNOWN_REALM_ID));
        BandReport {
            realm: Some(crate::realm::WELL_KNOWN_REALM_ID),
            ..report
        }
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
            witness.observe(&view, &view, W, H, 0, Some(&bound()));
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
            witness.observe(&view, &output, W, H, 0, Some(&bound()));
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
            witness.observe(&client_view(rgb), &frozen, W, H, 0, Some(&bound()));
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
        witness.observe(&[], &[], W, H, 0, Some(&bound()));
        witness.observe(&client_view([0; 3]), &[], W, H, 0, Some(&bound()));
        witness.observe(
            &client_view([0; 3]),
            &client_view([0; 3]),
            0,
            0,
            0,
            Some(&bound()),
        );
        let report = witness.report();
        assert_eq!(report.refusals, 3);
        assert_eq!(report.composites, 0);
        assert!(!report.tracks_view);
        assert!(!report.band_uniform);
    }

    /// **The property, over a thousand composites with a ticking clock**
    /// (WS-E.2.3, issue #215's central acceptance criterion).
    ///
    /// The band's rows are invariant while the strip's rows are not. Both
    /// halves matter: `band_changes == 0` on its own is satisfied by a witness
    /// that measured nothing, and `strip_changes > 0` is what says the clock
    /// really was moving in the frames the zero was counted over.
    ///
    /// It fails if somebody later moves the clock into the band, which is the
    /// only reason it exists.
    #[test]
    fn a_thousand_ticking_composites_move_the_strip_and_never_the_band() {
        use crate::status::{StatusConfig, StatusStrip, DEFAULT_HEIGHT};

        // A view taller than band + strip + something, so `tracks_view` has
        // rows left to be about: with a 24-row fixture the strip would cover
        // every row below the band and the comparison would be vacuously true.
        // Wide enough for the strip to actually draw something. At 64px there
        // is no room for a clock beside the attention marker's lane, so the
        // renderer correctly drops every field and the strip never changes —
        // which would make the "it was really repainting" assertion below fail
        // for a reason that has nothing to do with the witness.
        const SW: u32 = 320;
        const SH: u32 = 120;

        let indicator = TrustedIndicator::for_test();
        let mut strip = StatusStrip::new(StatusConfig {
            enabled: true,
            ..StatusConfig::default()
        });
        let mut witness = BandWitness::new();
        let realm = bound();
        let mono = std::time::Instant::now();
        let view = flat(SW, SH, [0x20, 0x21, 0x22]);
        for tick in 0..1000u64 {
            // One minute per composite, so every one of them moves the clock.
            strip.refresh(
                std::time::SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(1_786_244_643 + tick * 60),
                mono,
                Some(&realm),
            );
            let mut surface = ConsentSurface::new(indicator);
            let output = crate::backend::human_visible_from_view(
                view.clone(),
                &mut surface,
                &mut no_lock(),
                &mut strip,
                SW,
                SH,
                false,
            );
            witness.observe(&view, &output, SW, SH, strip.height(), Some(&realm));
        }
        let report = witness.report();
        assert_eq!(report.composites, 1000);
        assert_eq!(report.refusals, 0);
        assert_eq!(
            report.band_changes, 0,
            "the band's rows must be invariant under a ticking status strip"
        );
        assert!(
            report.strip_changes > 900,
            "the strip must actually have been repainting, or the zero above counts nothing: \
             {} changes in 1000 composites",
            report.strip_changes
        );
        assert_eq!(report.strip_h, DEFAULT_HEIGHT);
        assert!(report.band_uniform);
        // ...and the strip's rows are excluded from `tracks_view` rather than
        // dropped: below band + strip the output is still the realm view.
        assert!(
            report.tracks_view,
            "with the strip's own rows excluded, the output must still track the realm view"
        );
    }

    /// [`a_report_does_not_depend_on_the_bands_colour`]'s sibling for the strip
    /// (WS-E.2.3): the new counter must not become an oracle for the indicator.
    ///
    /// Two runs identical in every way but the session secret, with a strip up
    /// and a clock moving, must produce **byte-identical** reports. The strip is
    /// drawn from a snapshot of the clock, the battery and a `realm.toml` id, so
    /// this should hold trivially — which is exactly why it is checked rather
    /// than argued: the same was true of "do the band's rows equal the client's"
    /// right up until someone noticed it was a brute-force oracle.
    #[test]
    fn a_strip_report_does_not_depend_on_the_bands_colour() {
        use crate::status::{StatusConfig, StatusStrip};

        let run_with = |indicator: TrustedIndicator| {
            let mut strip = StatusStrip::new(StatusConfig {
                enabled: true,
                ..StatusConfig::default()
            });
            let mut witness = BandWitness::new();
            let realm = bound();
            let mono = std::time::Instant::now();
            for tick in 0..8u64 {
                let view = flat(320, 120, [tick as u8, 0x21, 0x22]);
                strip.refresh(
                    std::time::SystemTime::UNIX_EPOCH
                        + std::time::Duration::from_secs(1_786_244_643 + tick * 60),
                    mono,
                    Some(&realm),
                );
                let mut surface = ConsentSurface::new(indicator);
                let output = crate::backend::human_visible_from_view(
                    view.clone(),
                    &mut surface,
                    &mut no_lock(),
                    &mut strip,
                    320,
                    120,
                    false,
                );
                witness.observe(&view, &output, 320, 120, strip.height(), Some(&realm));
            }
            witness.report().to_string()
        };
        let dim = run_with(TrustedIndicator::from_rgb(0x40, 0x41, 0x42));
        let bright = run_with(TrustedIndicator::from_rgb(0xfe, 0xfd, 0xfc));
        // Non-vacuity: the run really did count a moving strip, so the equality
        // below is over a report with something in it.
        let strip_changes: u64 = dim
            .split(' ')
            .next_back()
            .expect("the strip counter is last")
            .parse()
            .expect("a number");
        assert!(
            strip_changes > 0,
            "this run counted no strip change, so the equality proves nothing: {dim}"
        );
        assert_eq!(dim, bright, "a report must not move with the secret");
    }

    /// A report read before any composite must not look like a passing one.
    #[test]
    fn a_witness_that_has_seen_nothing_reports_nothing_passing() {
        let witness = BandWitness::new();
        let report = witness.report();
        assert_eq!(report.composites, 0);
        assert_eq!(report.realm, None, "no realm has been bound");
        assert!(!report.tracks_view);
        assert!(!report.band_uniform);
        // A zero digest rather than the FNV offset basis: nothing was hashed,
        // and a report that opened with the digest of the empty string would be
        // indistinguishable from one taken over an empty probe strip.
        assert_eq!(
            report.to_string(),
            "0 0 0 0 0 0 0 0 0 0000000000000000 - 0 0"
        );
    }

    /// The wire form is thirteen fields of bounded ASCII, so the reply cannot
    /// become a pixel channel by accident: `MAX_LINE` is 128 bytes and a
    /// 640x480 band is 20 480. The eleventh is the bound realm's id, itself
    /// length-bounded by the loader; the twelfth and thirteenth are WS-E.2.3's
    /// strip height and strip-changed counter, appended after it so every
    /// position an existing reader indexes stays where it was.
    ///
    /// **Measured at the loader's maximum, not at `realm-0`'s length.** The
    /// bound that matters is 64 bytes over `[A-Za-z0-9._-]`, and this test
    /// used to render a 7-byte fixture and conclude the line fits — which
    /// checked 7 of the 65 bytes the field can actually contribute out of a
    /// 128-byte budget. See [`BandReport`]'s `Display` for the two facts the
    /// no-payload claim really rests on.
    #[test]
    fn the_wire_form_is_thirteen_scalar_fields() {
        let report = run(TrustedIndicator::for_test(), &[client_view([1, 2, 3])]);
        let line = report.to_string();
        assert_eq!(line.split(' ').count(), 13, "{line}");
        assert!(
            line.contains(&format!(" {} ", crate::realm::WELL_KNOWN_REALM_ID)),
            "the report must name the realm it is about: {line}"
        );
        assert!(
            line.ends_with(" 0 0"),
            "a session with `--status` off reports a zero-height strip that never changed: \
             {line}"
        );
        assert!(line.is_ascii());
        assert!(
            line.len() + "band ".len() < crate::consent::injector::MAX_LINE,
            "the band reply must fit the channel's line bound: {line}"
        );
    }

    /// **The reply still fits the channel at the longest realm id the loader
    /// accepts**, and here is exactly how much room that leaves.
    ///
    /// The id is checked against the real validator rather than assumed legal,
    /// so a future tightening or loosening of the rule moves this test rather
    /// than silently invalidating the arithmetic below.
    #[test]
    fn the_band_reply_fits_the_channel_at_the_longest_legal_realm_id() {
        const MAX_ID: usize = 64;
        // Every character class the rule allows, padded to the exact maximum:
        // uppercase, digits, `.`, `_` and `-` are all legal, which the old
        // `[a-z0-9-]` claim would have ruled out.
        let longest = format!("Aa0._-{}", "z".repeat(MAX_ID - 6));
        assert_eq!(longest.len(), MAX_ID);
        assert!(
            vitrin_ipc::paths::shim_runtime_dir_in(std::path::Path::new("/"), &longest).is_ok(),
            "fixture check: {longest} must be a legal realm id, or this measures nothing"
        );

        let mut witness = BandWitness::new();
        let realm = crate::grants::RealmId::new(&longest);
        let view = client_view([1, 2, 3]);
        witness.observe(
            &view,
            &human_visible(&view, TrustedIndicator::for_test()),
            W,
            H,
            0,
            Some(&realm),
        );
        let line = witness.report().to_string();
        assert!(line.is_ascii());
        assert_eq!(line.split(' ').count(), 13, "{line}");
        assert!(
            line.contains(&format!(" {longest} ")),
            "the report must name the realm it is about: {line}"
        );

        let budget = crate::consent::injector::MAX_LINE - "band ".len();
        assert!(
            line.len() < budget,
            "the band reply must fit the channel's line bound at the longest legal realm id: \
             {} of {budget} bytes -- {line}",
            line.len()
        );

        // What is left over, named rather than implied. The realm field eats
        // 65 bytes of the budget; the five `u64` counters and the four `u32`
        // geometry fields share what remains, and they are the only fields
        // without a short bound. This is a real constraint, not a formality:
        // at their type maximum the first ten fields alone are 137 bytes and
        // would overflow the line with no realm id at all. What keeps the
        // bound true is that they count *this session's* composites -- and
        // nothing a peer of this channel, or a confined client, can inflate.
        //
        // **Sixteen, and the number is measured rather than modelled.** The
        // old justification budgeted "~7 digits of `composites` and as many of
        // `probe_changes`" -- two counters -- and WS-E.2.3 added a third
        // (`strip_changes`) without revisiting it, so the stated model no
        // longer described the reply it guards. Raising the threshold to match
        // the model turns this assertion red on the real longest-legal reply,
        // which is the honest way of finding out that the model, not the
        // threshold, was the stale half: the counters share the residue rather
        // than each claiming a private seven digits, because a session cannot
        // composite 10^7 frames and also probe 10^7 times and also repaint the
        // strip 10^7 times within one run.
        let headroom = budget - line.len();
        assert!(
            headroom >= 16,
            "only {headroom} bytes are left for the three counters at the longest realm id; \
             they share this residue rather than each claiming it, so a reply that fails \
             here means a FOURTH field was added, not that a counter grew"
        );
    }
}
