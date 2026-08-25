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
//!
//!   **Since issue #304 that half is stronger, and this module had to be
//!   taught the difference.** The app is now *configured* at
//!   [`crate::view::ViewGeometry::usable`] — the output minus the reserved
//!   rows — so it is not merely overdrawn in the band's rows, it has no way
//!   to address them: [`crate::scene::Scene::compose`] fills them with
//!   [`crate::scene::LETTERBOX_RGBA`] and places the client's buffer below.
//!   The old statement ("it painted there and the band covered it anyway") is
//!   a consequence of the new one, not the other way round.
//!
//!   That strengthening cost the counters their self-sufficiency, which is
//!   the trap worth naming: with the client unable to reach those rows of the
//!   view, **nothing the client does can move them**, so a build whose
//!   [`ConsentSurface::composite_trust_band`] were made a no-op would leave
//!   the output's band rows holding the matte — uniform, fully opaque, and
//!   unmoved by any repaint — and `band_changes == 0` and
//!   `band_uniform == true` are then satisfied by a session with no trusted
//!   band on the screen at all.
//!
//!   Both directions of `band_changes` are degenerate now, and it is worth
//!   being exact about which: a **zero** is guaranteed by the inset rather
//!   than earned by the band; a **rise**, on a bandless build, comes from the
//!   *core's* own composition showing through — the empty-scene background
//!   before a client attaches, a consent scrim — never from the app. Measured
//!   on the shipped binary: with the composite deleted,
//!   `tests/integration/test_real_trust_band.py`'s first reading was
//!   `band_changes=3, band_uniform=1, tracks_view=1, view_reserved=1`, and
//!   the `3` was already `1` at four composites before any petition. A
//!   criterion that only fires when something unrelated moves is not the
//!   criterion.
//!
//!   Inheriting a stronger property while a gate criterion quietly stops
//!   discriminating is how a check comes to stop checking. So the
//!   strengthening is *asserted* rather than assumed, by a field for each
//!   half: [`BandReport::view_reserved`] (the client cannot reach those rows)
//!   and [`BandReport::band_over_matte`] (the band still drew over them
//!   last). `a_no_op_band_over_a_reserved_view_survives_both_old_counters` is
//!   the in-process pin — it feeds client repaints and nothing else, so it
//!   isolates the client's contribution the real session cannot — and it
//!   asserts the old counters passing so the reason the fields exist cannot
//!   be read as decoration.
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
//! [`BandReport::band_over_matte`] is the nearest thing to that shape this
//! module exports, and the distinction is the whole reason it is safe: it
//! compares the band against [`crate::scene::LETTERBOX_RGBA`], a constant in
//! this crate's own source, **not** against bytes the client chose. The
//! client picked `C` in the oracle above; it cannot pick the matte. And the
//! floor makes the answer a constant rather than a bit: every channel of a
//! mintable indicator is in `[64, 255]` and the matte's are `0x0f, 0x0f,
//! 0x14`, so the field is `true` for every legal session. A same-uid attacker
//! reading it learns what the source already told them.
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
//! - [`BandReport::band_changes`], [`BandReport::band_uniform`] and
//!   [`BandReport::band_over_matte`] are about the output's band rows, which
//!   belong to no realm at all — they are the core's own overdraw.
//! - [`BandReport::view_reserved`] is about **the bound realm's** view again:
//!   the rows that realm's app was configured out of.
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
    /// **Zero in a correct session, always.** This was the property: the
    /// band's rows are invariant under everything the client does.
    ///
    /// **It stopped being a statement about the client when the realm view was
    /// inset** (issue #304). The client no longer owns those rows of the view,
    /// so nothing it does can raise this counter — the zero is guaranteed by
    /// the inset rather than earned by the band, and a build whose band never
    /// composited at all reports it too. What can still raise it is the core's
    /// own composition changing beneath an absent band. So the field is kept
    /// as the statement "the core holds these rows constant", which is worth
    /// having, and [`Self::band_over_matte`] is what says the constant is a
    /// drawn band. The pair is what a reader must take together; neither alone
    /// is the property any more.
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
    /// At the latest composite: every pixel of the **realm view's** reserved
    /// rows — `band_h + strip_h`, the rows
    /// [`crate::view::ViewGeometry::reserved_top`] keeps — is exactly
    /// [`crate::scene::LETTERBOX_RGBA`], the core's own matte.
    ///
    /// **The structural half of the property** (issue #304). Before the realm
    /// view was inset, the app was configured at the output's full height, so
    /// the client's own pixels really were in those rows of its view and the
    /// band's whole job was to cover them. Now the app is configured at
    /// [`crate::view::ViewGeometry::usable`] and
    /// [`crate::scene::Scene::compose`] fills the reserved rows with the matte
    /// before it blits anything, so a confined client has no way to address
    /// them at all — which is strictly stronger than "it is covered", and this
    /// is where that strength is asserted rather than assumed.
    ///
    /// It fails, loudly, on the regression that would silently restore the old
    /// weaker world: a `configure` that went back to carrying the output's
    /// full height, or a placement that stopped translating by
    /// `reserved_top()`, puts client bytes back in these rows and reads
    /// `false` here.
    ///
    /// Secret-independent, like every other field: the comparison is against a
    /// compile-time core constant, and the indicator is not one of its inputs.
    pub view_reserved: bool,
    /// At the latest composite: **no** pixel of the human-visible output's
    /// band rows is [`crate::scene::LETTERBOX_RGBA`] — something was painted
    /// over the matte those rows arrive carrying.
    ///
    /// **The temporal half of the property**, and the reason it had to be
    /// added with the inset. [`Self::band_changes`] used to carry it by
    /// itself: with the client owning those rows of the view, a
    /// `composite_trust_band` made a no-op let the client's repaint through
    /// and the counter rose. With [`Self::view_reserved`] true those rows are
    /// a core-owned constant that no client can move, so a band that never
    /// drew leaves them constant *and* uniform — `band_changes == 0` and
    /// `band_uniform == true` are both satisfied by a build with no band at
    /// all. **This is the only field a no-op band fails**, and it was measured
    /// that way against the shipped binary rather than argued: see the module
    /// docs for the reading that sabotage produced.
    ///
    /// **Why this is not the brute-force oracle the module docs reject.** That
    /// oracle was *"the band's rows equal the realm view's rows beneath
    /// them"*, and it leaked because the client chose the right-hand side: the
    /// bit was exactly `S == C` for a candidate colour `C` the app painted.
    /// Here the right-hand side is `LETTERBOX_RGBA`, a constant in this
    /// crate's source that no client and no peer of this channel contributes a
    /// byte to. Its value is `[0x0f, 0x0f, 0x14, 0xff]` and
    /// [`TrustedIndicator::generate`] scales every channel into `[64, 255]`,
    /// so a mintable indicator can never equal it: this field is `true` for
    /// **every** legal session and carries zero bits about which one. That is
    /// the same floor `tests/integration/test_real_trust_band.py` already
    /// leans on, used in the same safe direction.
    pub band_over_matte: bool,
}

impl std::fmt::Display for BandReport<'_> {
    /// The channel's wire form: fifteen space-separated ASCII fields, no
    /// payload, no descriptor. Rendered here rather than at the call site so
    /// the one place the report becomes bytes is the one place to audit.
    ///
    /// The bound realm's id is eleventh, after the digest, so the ten fields
    /// that predate WS-E.1.3 keep their positions; `-` when no realm is
    /// bound. WS-E.2.3's two strip fields are appended **after** it for the
    /// same reason it was appended after the digest: every position an existing
    /// reader indexes stays where it was, and a harness that has not been taught
    /// about the strip reads the same numbers it read before. #304's
    /// [`BandReport::view_reserved`] and [`BandReport::band_over_matte`] are
    /// appended after those, on the same rule.
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
    /// channel's 128-byte budget, so
    /// [`the_band_reply_fits_the_channel_at_the_longest_legal_realm_id`]
    /// measures the line at the **loader's** maximum rather than at a
    /// fixture's, and records what is left over for the counters.
    ///
    /// The counters are `u64` and are the one thing on this line without a
    /// short bound. At their type maximum the line would not fit `MAX_LINE`
    /// with *or* without a realm id (137 bytes for the first ten fields
    /// alone), and that is stated rather than papered over: what makes the
    /// bound hold is that they count this session's composites, and the test
    /// derives the ceiling that leaves and asserts a reply at it still fits.
    ///
    /// [`the_band_reply_fits_the_channel_at_the_longest_legal_realm_id`]: tests::the_band_reply_fits_the_channel_at_the_longest_legal_realm_id
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {} {} {} {} {} {} {:016x} {} {} {} {} {}",
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
            u8::from(self.view_reserved),
            u8::from(self.band_over_matte),
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
    view_reserved: bool,
    band_over_matte: bool,
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
            view_reserved: false,
            band_over_matte: false,
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
        // #304's two halves. `view[..strip_end]` is exactly the rows
        // `ViewGeometry::reserved_top` keeps — the band's plus the strip's —
        // so this is "the client cannot address them", read off the same
        // buffer the capture path serves. `band` is the human-visible
        // output's band rows, so the second is "and something was drawn over
        // the matte they arrive carrying". Neither mentions the indicator.
        self.view_reserved = strip_end > 0
            && view[..strip_end]
                .chunks_exact(4)
                .all(|pixel| pixel == crate::scene::LETTERBOX_RGBA);
        self.band_over_matte = band_bytes > 0
            && band
                .chunks_exact(4)
                .all(|pixel| pixel != crate::scene::LETTERBOX_RGBA);
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
            view_reserved: self.view_reserved,
            band_over_matte: self.band_over_matte,
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

    /// A realm view filled with one colour, **band rows included** — the
    /// pre-#304 shape, when an app was configured at the output's full height
    /// and really could paint the rows the band would cover.
    ///
    /// **No longer what `Scene::compose` produces**, and kept deliberately:
    /// the counters' arithmetic is still specified over "the client's bytes
    /// moved in the band's rows", and this is the only fixture that can put
    /// them there. Every test whose subject is the *shipping* view shape uses
    /// [`composed_view`] instead, and the two are contrasted by
    /// [`the_witness_tells_a_reserved_view_from_a_client_painted_one`].
    fn client_view(rgb: [u8; 3]) -> Vec<u8> {
        [rgb[0], rgb[1], rgb[2], 0xff]
            .repeat(W as usize * H as usize)
            .to_vec()
    }

    /// A realm view of the shape [`crate::scene::Scene::compose`] actually
    /// produces since issue #304: the reserved rows are the core's matte, and
    /// the client's colour starts at
    /// [`crate::view::ViewGeometry::reserved_top`].
    ///
    /// `strip_h` rows of strip on top of the band's, so the same fixture
    /// serves a `--status`-off session (`0`) and a `--status`-on one.
    fn composed_view(rgb: [u8; 3], strip_h: u32) -> Vec<u8> {
        let reserved = (TRUST_BAND_HEIGHT + strip_h).min(H);
        let mut view = crate::scene::LETTERBOX_RGBA.repeat((W * reserved) as usize);
        view.extend_from_slice(
            &[rgb[0], rgb[1], rgb[2], 0xff].repeat((W * (H - reserved)) as usize),
        );
        assert_eq!(
            view.len(),
            (W * H * 4) as usize,
            "fixture must be a whole frame"
        );
        view
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
            &no_blank(),
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
            &no_blank(),
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
            &no_blank(),
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

    fn no_blank() -> crate::backend::blank::BlankSurface {
        crate::backend::blank::BlankSurface::for_test()
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
            &no_blank(),
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
                &no_blank(),
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

    /// **The blank cover rides the SAME output stage the band witness measures,
    /// and the band survives it** (WS-E.4.3, issue #223).
    ///
    /// This is the acceptance criterion #223 states in as many words, and it is
    /// written to fail if a blank is ever routed around
    /// [`crate::backend::human_visible_from_view`]: every byte asserted below
    /// comes out of that one call, so a cover applied in a backend's own
    /// composite instead — the "third presentation path" this module's
    /// neighbours were written about — leaves this test looking at an
    /// uncovered frame.
    ///
    /// Three things are checked, and each is a different failure:
    ///
    /// * **Not one pixel of the realm view survives.** The witness's
    ///   `tracks_view` reads `true` on the lit frame (the control, without which
    ///   the second reading proves nothing) and `false` on the covered one, and
    ///   the raw comparison below says why: the rows below the band are the
    ///   cover's colour and not the client's.
    /// * **The band is untouched**, byte for byte, across the transition —
    ///   `band_changes == 0`, which is the property, and the cover is the first
    ///   opaque full-view fill ever composited *under* it. A cover drawn after
    ///   `composite_trust_band` would black the band out and turn this red.
    /// * **The band still carries this session's own colour.** Not merely
    ///   "unchanged": a frame that is black everywhere including the band would
    ///   satisfy `band_changes == 0` on its own if the previous frame were black
    ///   too, and it is the *lit* band on a blanked frame that distinguishes
    ///   "vitrind blanked" from "a confined app painted itself black".
    #[test]
    fn the_blank_cover_rides_the_output_stage_and_leaves_the_band_lit() {
        let indicator = TrustedIndicator::from_rgb(0x40, 0x41, 0x42);
        let view = client_view([0x11, 0x22, 0x33]);
        let mut surface = ConsentSurface::new(indicator);
        let mut blank = no_blank();
        let mut witness = BandWitness::new();

        // The control: lit. Without it "no realm pixel survives" is a claim
        // about a witness that might never have seen one.
        let lit = crate::backend::human_visible_from_view(
            view.clone(),
            &mut surface,
            &mut no_lock(),
            &blank,
            &mut no_status(),
            W,
            H,
            false,
        );
        witness.observe(&view, &lit, W, H, 0, Some(&bound()));
        assert!(
            witness.report().tracks_view,
            "the lit frame must track the realm view, or the covered reading below is \
             measuring nothing"
        );

        // ...and now the cover.
        blank.set_covering(true);
        let covered = crate::backend::human_visible_from_view(
            view.clone(),
            &mut surface,
            &mut no_lock(),
            &blank,
            &mut no_status(),
            W,
            H,
            false,
        );
        witness.observe(&view, &covered, W, H, 0, Some(&bound()));
        let report = witness.report();

        assert!(
            !report.tracks_view,
            "a covered frame must NOT track the realm view: the whole point of the cover is \
             that the human's last screenful is not what is sitting in the scanout buffer \
             when the panel goes dark"
        );
        assert_eq!(
            report.band_changes, 0,
            "the trusted band must be byte-identical across a blank. The cover is the first \
             opaque full-view fill ever composited UNDER it, and a cover drawn after \
             `composite_trust_band` instead would black out the one strip the human reads \
             this session's colour from"
        );
        assert!(report.band_uniform, "and it must still be one flat colour");

        let band_bytes = (W as usize) * (crate::consent::TRUST_BAND_HEIGHT.min(H) as usize) * 4;
        assert!(
            covered[..band_bytes]
                .chunks_exact(4)
                .all(|px| px == indicator.color()),
            "the band on a blanked frame must still carry THIS SESSION'S colour -- that is \
             what tells a human looking at a black screen that vitrind blanked it rather than \
             a confined app painting itself black"
        );
        assert!(
            covered[band_bytes..]
                .chunks_exact(4)
                .all(|px| px == [0x00, 0x00, 0x00, 0xff]),
            "and every row below the band must be the cover, with no client pixel left: \
             {:?}",
            &covered[band_bytes..band_bytes + 16]
        );
        assert_ne!(
            &covered[band_bytes..band_bytes + 4],
            &view[band_bytes..band_bytes + 4],
            "...which the control makes meaningful: the client's own colour really was there"
        );
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

    /// **The two halves of the post-#304 property, over the view shape
    /// `Scene::compose` actually emits**: the client cannot address the
    /// reserved rows, and the band is drawn over the matte they carry.
    ///
    /// This is the real-app gate's scenario translated to the shipping
    /// geometry — a confined app repainting everything it owns, twice — and
    /// it is the positive control for
    /// [`a_no_op_band_over_a_reserved_view_survives_both_old_counters`]: same
    /// views, real band, both new fields true.
    #[test]
    fn a_reserved_view_keeps_the_client_out_of_the_bands_rows_entirely() {
        let mut witness = BandWitness::new();
        let indicator = TrustedIndicator::for_test();
        for rgb in [[0x00, 0x00, 0x00], [0xff, 0x00, 0x00], [0x00, 0xff, 0x00]] {
            let view = composed_view(rgb, 0);
            let output = human_visible(&view, indicator);
            witness.observe(&view, &output, W, H, 0, Some(&bound()));
        }
        let report = witness.report();
        assert_eq!(report.composites, 3);
        assert_eq!(report.band_changes, 0, "client content reached the band");
        assert!(
            report.view_reserved,
            "the realm view's reserved rows must be the core's matte: the app is configured \
             at `usable()`, so nothing of its own can be in them"
        );
        assert!(
            report.band_over_matte,
            "the band must still be drawn over that matte -- `view_reserved` alone would be \
             satisfied by a build with no band at all"
        );
        assert!(report.band_uniform);
        assert_eq!(
            report.probe_changes, 2,
            "the client's own rows -- now the FIRST rows it has -- must be seen changing"
        );
        assert!(report.tracks_view);
        assert_eq!(report.refusals, 0);
    }

    /// **Why [`BandReport::band_over_matte`] had to be added** (issue #304),
    /// and it is written as the demonstration rather than asserted in a
    /// comment: the exact sabotage `test_real_trust_band.py` names first — a
    /// `composite_trust_band` made a no-op — **passes both counters that used
    /// to catch it** once the realm view is inset.
    ///
    /// The two `assert!`s on the old fields are the point of the test and must
    /// not be softened into `let _ =`: if a later change makes `band_changes`
    /// discriminate here again, this test goes red and says the new fields'
    /// justification has moved.
    #[test]
    fn a_no_op_band_over_a_reserved_view_survives_both_old_counters() {
        let mut witness = BandWitness::new();
        for rgb in [[0x00, 0x00, 0x00], [0xff, 0x00, 0x00], [0x00, 0xff, 0x00]] {
            // The output IS the view: what `human_visible_from_view` would
            // return with the band's composite deleted.
            let view = composed_view(rgb, 0);
            witness.observe(&view, &view, W, H, 0, Some(&bound()));
        }
        let report = witness.report();
        assert_eq!(
            report.band_changes, 0,
            "PRE-#304 this sabotage raised this counter, because the client owned these rows \
             of its own view; it no longer can, so the matte sits there unchanged"
        );
        assert!(
            report.band_uniform,
            "...and the matte is one fully opaque colour, so uniformity passes too"
        );
        assert!(
            report.tracks_view,
            "...and an unpainted band tracks the view perfectly, as it always did"
        );
        assert!(
            report.view_reserved,
            "the view is the shipping shape: this sabotage is about the band, not the inset"
        );
        assert!(
            !report.band_over_matte,
            "THE FIELD THIS TEST EXISTS FOR: with every older criterion passing, only \
             `band_over_matte` distinguishes a session with a trusted band from one without"
        );
    }

    /// The structural half is a real reading of the pixels, not a constant:
    /// the same witness reports `view_reserved` false on a view whose reserved
    /// rows carry client bytes.
    ///
    /// That is the shape a reverted inset produces — a `configure` carrying the
    /// output's full height, or a placement that stopped translating by
    /// `reserved_top()` — so this is the regression `view_reserved` is for.
    #[test]
    fn the_witness_tells_a_reserved_view_from_a_client_painted_one() {
        let indicator = TrustedIndicator::for_test();
        for (view, want, what) in [
            (
                composed_view([0x11, 0x22, 0x33], 0),
                true,
                "the shipping shape",
            ),
            (client_view([0x11, 0x22, 0x33]), false, "the pre-#304 shape"),
        ] {
            let mut witness = BandWitness::new();
            let output = human_visible(&view, indicator);
            witness.observe(&view, &output, W, H, 0, Some(&bound()));
            assert_eq!(
                witness.report().view_reserved,
                want,
                "view_reserved must read {want} over {what}"
            );
            // The band is drawn either way: this field is about the view, and
            // a reader must not be able to confuse the two halves.
            assert!(witness.report().band_over_matte);
        }
    }

    /// With `--status` on, the reserved rows are the band's **and** the
    /// strip's — `ViewGeometry::reserved_top()` — and `view_reserved` is about
    /// all of them.
    ///
    /// Written because the tempting implementation checks only the band's
    /// rows, which would leave a client able to paint the strip's rows of its
    /// own view while this still read `true`.
    #[test]
    fn view_reserved_covers_the_strips_rows_too_when_a_strip_is_up() {
        const STRIP_H: u32 = 6;
        let indicator = TrustedIndicator::for_test();

        let good = composed_view([0x11, 0x22, 0x33], STRIP_H);
        let mut witness = BandWitness::new();
        witness.observe(
            &good,
            &human_visible(&good, indicator),
            W,
            H,
            STRIP_H,
            Some(&bound()),
        );
        assert!(witness.report().view_reserved);
        assert_eq!(witness.report().strip_h, STRIP_H);

        // The band's rows reserved, the strip's rows client-painted: exactly
        // what a band-only check would wave through.
        let band_only = composed_view([0x11, 0x22, 0x33], 0);
        let mut witness = BandWitness::new();
        witness.observe(
            &band_only,
            &human_visible(&band_only, indicator),
            W,
            H,
            STRIP_H,
            Some(&bound()),
        );
        assert!(
            !witness.report().view_reserved,
            "a view whose strip rows carry client bytes is not a reserved view"
        );
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
    ///
    /// **The view it feeds is the pre-#304 shape and this test says so**, so
    /// that what it pins is not mistaken for the shipping case: it pins the
    /// *arithmetic* — client bytes that move in the band's rows are counted —
    /// over a frame `Scene::compose` no longer emits. The shipping case is
    /// [`a_no_op_band_over_a_reserved_view_survives_both_old_counters`], where
    /// this same sabotage passes `band_changes` and is caught elsewhere.
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
                &no_blank(),
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
                    &no_blank(),
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
        assert!(!report.view_reserved);
        assert!(!report.band_over_matte);
        assert_eq!(
            report.to_string(),
            "0 0 0 0 0 0 0 0 0 0000000000000000 - 0 0 0 0"
        );
    }

    /// The wire form is fifteen fields of bounded ASCII, so the reply cannot
    /// become a pixel channel by accident: `MAX_LINE` is 128 bytes and a
    /// 640x480 band is 20 480. The eleventh is the bound realm's id, itself
    /// length-bounded by the loader; the twelfth and thirteenth are WS-E.2.3's
    /// strip height and strip-changed counter, appended after it so every
    /// position an existing reader indexes stays where it was; the fourteenth
    /// and fifteenth are #304's `view_reserved` and `band_over_matte`, two
    /// single-digit booleans appended on the same rule.
    ///
    /// **The count is in the name and the name must not go stale.** This test
    /// was `..._is_thirteen_scalar_fields` and #304's two fields made that a
    /// number the reply no longer had: a test whose name states a count is a
    /// published claim, and renaming it is part of moving the wire form rather
    /// than an afterthought. The fields are read **by index** below rather
    /// than off the end of the line, because the previous shape asserted the
    /// strip pair with `ends_with(" 0 0")` — an assertion that stops being
    /// about the strip the moment anything is appended after it, and would
    /// have passed here by reading #304's two booleans instead.
    #[test]
    fn the_wire_form_is_fifteen_scalar_fields() {
        // The SHIPPING view shape, so the two #304 fields carry the values a
        // correct session reports rather than the pre-inset fixture's.
        let report = run(TrustedIndicator::for_test(), &[composed_view([1, 2, 3], 0)]);
        let line = report.to_string();
        let fields: Vec<&str> = line.split(' ').collect();
        assert_eq!(fields.len(), 15, "{line}");
        assert_eq!(
            fields[10],
            crate::realm::WELL_KNOWN_REALM_ID,
            "the report must name the realm it is about, in the eleventh field: {line}"
        );
        assert_eq!(
            (fields[11], fields[12]),
            ("0", "0"),
            "a session with `--status` off reports a zero-height strip that never changed, \
             in the twelfth and thirteenth fields: {line}"
        );
        assert_eq!(
            (fields[13], fields[14]),
            ("1", "1"),
            "#304's two fields are the fourteenth and fifteenth: the client cannot reach the \
             reserved rows of its own view, and the band was drawn over the matte they carry: \
             {line}"
        );
        assert!(line.is_ascii());
        assert!(
            line.len() + "band ".len() < crate::consent::injector::MAX_LINE,
            "the band reply must fit the channel's line bound: {line}"
        );
    }

    /// **The reply still fits the channel at the longest realm id the loader
    /// accepts**, and here is exactly how much room that leaves — re-derived
    /// for #304's two extra fields rather than left standing at the old
    /// number.
    ///
    /// The id is checked against the real validator rather than assumed legal,
    /// so a future tightening or loosening of the rule moves this test rather
    /// than silently invalidating the arithmetic below.
    ///
    /// **What changed, and why the answer is a digit budget rather than a
    /// headroom.** The previous shape measured one minimal fixture and
    /// asserted `headroom >= 16` — a number about *that* reply, not about any
    /// reply the channel carries, so it said nothing about a session whose
    /// counters had actually run. #304 appended `view_reserved` and
    /// `band_over_matte`, two single-digit booleans and their separators, and
    /// the reply grew by exactly four bytes. Widening `MAX_LINE` to absorb
    /// them was refused: the bound is re-derived instead, and stated as the
    /// thing that actually constrains the line — how many decimal digits the
    /// nine variable-width numeric fields may share.
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
        let view = composed_view([1, 2, 3], 0);
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
        assert_eq!(line.split(' ').count(), 15, "{line}");
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

        // **The bound, derived rather than eyeballed.** Everything on the line
        // except the nine numeric fields is fixed-width once the realm id is
        // at its maximum: 14 separators, four single-digit booleans, a 16-digit
        // hex digest and 64 bytes of realm id. Whatever the budget has left
        // over is the total number of decimal digits those nine may share --
        // `composites`, `band_changes`, `probe_changes`, `refusals`,
        // `strip_changes`, `band_h`, `view_w`, `view_h` and `strip_h`. That is
        // the real constraint: at their `u64` maximum the counters alone would
        // overflow the line with no realm id at all, and what keeps the bound
        // true is that they count *this session's* composites -- nothing a peer
        // of this channel, or a confined client, can inflate.
        const SEPARATORS: usize = 14;
        const BOOLEANS: usize = 4;
        const DIGEST: usize = 16;
        let fixed = SEPARATORS + BOOLEANS + DIGEST + MAX_ID;
        // `< budget`, not `<=`, so the ceiling is the largest total that still
        // satisfies the assertion above.
        let digit_budget = budget - fixed - 1;
        assert_eq!(
            digit_budget, 24,
            "the nine variable-width numeric fields share {digit_budget} digits at the longest \
             legal realm id. This number is the wire form's real bound and it MOVED with #304: \
             it was 28 before `view_reserved` and `band_over_matte` were appended (two booleans \
             and two separators), and a change that moves it again is a change to what the \
             channel can carry, not a formality"
        );

        // **What that budget buys, asserted on a rendered reply rather than
        // modelled in a comment** -- the half the old `headroom >= 16` never
        // did. This is the geometry every session on this channel actually
        // runs (`--headless`, `--status` off, `tests/integration`'s 640x480)
        // with its counters at a million composites, which is about five hours
        // at 60 Hz.
        let ceiling = BandReport {
            realm: Some(&longest),
            composites: 1_000_000,
            band_changes: 0,
            probe_changes: 999_999,
            strip_changes: 0,
            tracks_view: true,
            band_uniform: true,
            view_reserved: true,
            band_over_matte: true,
            refusals: 0,
            band_h: TRUST_BAND_HEIGHT,
            strip_h: 0,
            view_w: 640,
            view_h: 480,
            probe_fnv: u64::MAX,
        };
        let at_ceiling = ceiling.to_string();
        assert!(
            at_ceiling.len() < budget,
            "a million-composite session at this channel's own geometry must still fit at the \
             longest legal realm id: {} of {budget} bytes -- {at_ceiling}",
            at_ceiling.len()
        );

        // ...and the other direction, because "it fits" without a boundary is
        // the kind of claim that quietly stops being true. The line does NOT
        // fit forever: pinning where it stops is what keeps the sentence above
        // from being read as universal.
        //
        // A 4K output with a 20-row strip spends 11 of the 24 digits on
        // geometry alone, leaving 13 for five counters -- so the same session
        // overflows somewhere around a thousand composites, which is seconds.
        // **#304 did not create this cliff; it moved it four bytes closer**,
        // and no session this channel exists in can reach it: the channel is
        // gated on the `consent-injector` feature plus `--headless`, and the
        // one suite that opens it runs 640x480 under the realm id `realm-0`.
        // Recorded here rather than fixed, because fixing it means either
        // widening `MAX_LINE` -- which is the channel bending to suit the
        // reply -- or shortening the reply, and neither belongs in #304.
        let past_ceiling = BandReport {
            view_w: 3840,
            view_h: 2160,
            strip_h: crate::status::MAX_HEIGHT,
            ..ceiling
        };
        assert!(
            past_ceiling.to_string().len() >= budget,
            "the boundary this test pins has moved: a 4K, `--status`-on session at a million \
             composites now fits at the longest legal realm id, where it did not before. That \
             is a real improvement, but it means the sentence above about where the reply \
             stops fitting is stale -- re-derive it rather than deleting this assertion"
        );
    }
}
