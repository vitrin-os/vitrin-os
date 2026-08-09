// SPDX-License-Identifier: MPL-2.0
//! The per-session trusted indicator (issue #85): a secret colour every
//! genuine consent prompt is framed in, so a human can tell a real prompt
//! from a replica a confined app painted into its own surface.
//!
//! # Why a secret, and why it cannot be a copy-change
//!
//! PRD line 294 grounds unspoofability in the Qubes/Nitpicker trusted-labeling
//! principle: it works because the GUI server draws a label the confined
//! domain provably cannot reproduce. The consent card's own pixels give no
//! such guarantee — they sit inside a rectangle a maximized client surface
//! fills edge to edge, so an app can rasterize the identical card, commit it,
//! and [`crate::scene::Scene::compose`] presents it byte-for-byte. What the
//! app cannot do is paint *this session's colour*, because it never observes
//! it:
//!
//! - **Established before any client can connect** (`run_session`, at the same
//!   startup point the listener binds), so no client — not even one holding an
//!   observe grant — was ever running when the secret was minted.
//!   **And minted exactly once, for the life of the process.** That is the
//!   property, not an omission: the indicator defends against a replica by
//!   being *stable*, and a colour that changed whenever the session was
//!   interrupted would leave the human unable to tell a legitimate change from
//!   a forgery — the check would collapse into "the colour is different now,
//!   which is either normal or an attack". So the secret is **never rotated**,
//!   on any event: not on a VT switch, not on a resume, not on revocation
//!   (WS-E.3.3, D-030(3); `the_session_colour_is_minted_once_and_nothing_re_mints_it`
//!   holds it to one call site).
//!
//!   **The cost of that, stated rather than implied.** One compromise of the
//!   colour lasts the whole session, and there is no rotation path to reach
//!   for. On bare metal that stops being theoretical: the panel is physically
//!   in the room, so a camera pointed at it reads the band, and the
//!   photographer can then paint a byte-identical band and a byte-identical
//!   frame. What survives is the *second* line of defence — a replica gets no
//!   input grab ([`super::grab`]) — so a forged card still cannot mint a
//!   grant; the harm is deception, not direct authority. And the mitigation is
//!   ending the session, which on bare metal means leaving vitrind's VT and
//!   killing the process: **the dead-man chord is not it.** That chord revokes
//!   every grant and denies every petition, which is the right instrument for
//!   "stop everything" and does nothing at all for a compromised trust colour,
//!   because the process — and its colour — keep running.
//! - **Drawn only on the human-visible output path**
//!   ([`crate::backend::human_visible_from_view`]) — the per-prompt frame in
//!   [`super::ConsentSurface::composite_over`] and the always-present band in
//!   [`super::ConsentSurface::composite_trust_band`] — never in
//!   [`crate::scene::Scene::compose`]. The capture service reads the bare
//!   realm view, so the colour cannot leak through `vitrin_view.frame_ready`
//!   — the same structural argument the overlay itself already relies on.
//!   **And, since WS-E.2.4 (issue #216), the human's own screenshot key reads
//!   that same bare realm view for exactly this reason** — the one place in the
//!   core that writes pixels to a *file* a same-uid app can open. See
//!   [`crate::screenshot`] for the two softer designs that were examined and
//!   refused (cropping the band, which leaves the per-prompt frame; and
//!   redacting by colour, which hands the app an oracle it can binary-search
//!   the secret with). The cost — a screenshot that cannot show a consent
//!   prompt — is published rather than paid quietly.
//!
//! # How the human learns it — and why not the log
//!
//! The secret is **never written to any descriptor or file** — including the
//! files WS-E.2.4's screenshot key creates, which is why that key reads the
//! realm view. That is not
//! fastidiousness: the confined realm runs as the core's own uid (the
//! `SO_PEERCRED` same-user policy), so anything the core writes to stdout or
//! stderr is reachable by the app — directly if it inherits the fd, or through
//! `/proc/<core-pid>/fd` if it does not — and a file the recorder wrote would
//! be readable the same way. A secret in any of those places is a secret the
//! forger can read, and an indicator the forger can read is no indicator.
//!
//! So the human learns the colour the one way the app cannot follow: off the
//! **display vitrind owns**. [`super::ConsentSurface::composite_trust_band`]
//! paints a reserved band along the top of the human-visible output on *every*
//! frame — prompt or not — composited after the client's surface, so client
//! content in that strip is always overdrawn by the genuine colour (issue
//! #85's "reserved strip ... the scene compositor refuses to let client
//! content reach"). The human reads the band, then checks a prompt's frame
//! against it: a dialog framed in any other colour, or none, is forged —
//! whoever drew it.
//!
//! **"The display vitrind owns" means three different things now, and the
//! third one can be taken away.**
//!
//! - **Nested**: the app is a client of *this* core, not of the host, so it
//!   cannot read back the host window. The host compositor above us can, and
//!   always could; `docs/book/src/limits.md` publishes that the nested lock
//!   covers a window rather than a session, and the band inherits the same
//!   boundary.
//! - **Headless**: there is no human to teach, and the overlay is absent from
//!   the agent's capture regardless.
//! - **Bare metal (WS-E.3.2/#218)**: the phrase becomes literally true and
//!   stronger — this core holds DRM master on the connector, and the pixels
//!   live in its own GBM buffers, which no other DRM client may read. But for
//!   the first time the display can be **taken away**: `Ctrl-Alt-F<n>` hands
//!   the panel to another VT, and nothing in this project inhibits it
//!   (D-030(1) — a display server that traps the human on its own VT is one
//!   they cannot escape when it wedges, which contradicts the dead-man
//!   switch's whole posture).
//!
//! So the band asserts exactly this and no more: **everything above the line
//! on *this* screen was drawn by the `vitrind` process you started.** It
//! asserts nothing about any other VT, and while the seat holds the devices
//! this core cannot see that screen, cannot draw on it, and cannot tell the
//! human afterwards what was on it. What *is* checkable on return is the
//! colour's continuity, which is why (1)'s mint-once rule is load-bearing
//! rather than incidental: **the same colour means the same core; a different
//! colour means the core you left is not the core you came back to.**
//!
//! # What may sit near the band, and what may never sit in it
//!
//! Three core-drawn surfaces now occupy the rows immediately **below** the
//! band, and none of them is ever composited into rows `[0,
//! TRUST_BAND_HEIGHT)`: [`crate::attention`]'s marker (WS-E.1.7),
//! [`crate::status`]'s clock/battery/realm strip (WS-E.2.3), and, transiently,
//! [`crate::lock`]'s cover — which stops *below* the band for the same reason.
//! The rule is one sentence and it has no exceptions: **the band has exactly
//! one correct appearance, so nothing at all is drawn on it.** A band that
//! sometimes carried a glyph would be a band whose correct appearance is a
//! judgement call, and the human's check against a forged prompt is only as
//! sharp as that appearance is unambiguous.
//!
//! It is enforced structurally rather than by review. The strip's raster is the
//! strip's own height and is blitted at `y = TRUST_BAND_HEIGHT`, so no
//! coordinate expressible in its renderer lands in the band; the marker's rect
//! starts at the same row; the cover's fill is followed by
//! [`super::ConsentSurface::composite_trust_band`], which overdraws it. And it
//! is measured: [`crate::backend::band_witness`]'s `band_changes` is `0` in a
//! correct session, over composites its sibling `strip_changes` proves were
//! really repainting.
//!
//! The strip below is **not** covered by this argument, and the difference is
//! published rather than blurred: an app can paint a convincing fake strip one
//! row lower than the real one. The band is the anchor; the rule taught to the
//! human is "trusted content is everything above the coloured line".
//!
//! **On bare metal that rule needs a second clause, because a second forgery
//! surface appears that has no row coordinate at all**: another VT, running
//! another compositor, painting an entire fake session — band included, for
//! anyone who has photographed the real one. A spatial rule cannot exclude it.
//! So the rule taught to the human becomes *"trusted content is everything
//! above the coloured line, on the VT this core is driving"* — and this core
//! cannot tell the human which VT they are on, which is exactly what makes
//! honest scoping (D-030(1)) the only available answer rather than the
//! comfortable one.

/// A per-session secret colour. Opaque RGBA; `Copy` so it rides in the
/// [`crate::session::RuntimeSeed`] and into both backends' consent surfaces
/// without a clone, and small enough that copying it is free.
///
/// `Debug` is hand-written to redact the value, not derived: a derived `{:?}`
/// would print the secret, and a single `tracing::debug!(?indicator)` — or a
/// derived `Debug` on any carrier that reached a log — would write it to
/// stderr, which the same-uid app reads via `/proc`. Making the type refuse to
/// render its own value is what keeps "the secret never becomes text" true by
/// construction rather than by the current absence of such a call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrustedIndicator {
    /// Alpha is always `0xFF`: the frame is an opaque band, never a tint an
    /// app could approximate by guessing a blend.
    rgba: [u8; 4],
}

impl std::fmt::Debug for TrustedIndicator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted on purpose — see the type's docs. The same move
        // `recorder::ObservationDigest` and the credential types make.
        f.write_str("TrustedIndicator(<redacted>)")
    }
}

impl TrustedIndicator {
    /// Mint a fresh secret from the kernel CSPRNG (`getrandom(2)`).
    ///
    /// Each channel is scaled into `[64, 255]`, so the colour is always
    /// visible against the scrim (a near-black secret the human could not see
    /// would defeat the check) while still leaving roughly 22 bits an app
    /// would have to guess *blind* — it never sees the value to copy it. The
    /// scale (`64 + b*192/256`) is used rather than `64 + b % 192` on purpose:
    /// a modulo reduction would make the low third of each channel twice as
    /// likely, shrinking the real guess space; the scale keeps the draw
    /// near-uniform over the visible sub-range.
    ///
    /// **Fails closed.** If entropy is unavailable the core refuses to start
    /// rather than mint a guessable indicator: a predictable trust colour is
    /// worse than none, because it would train a human to trust a frame an app
    /// can reproduce.
    pub fn generate() -> std::io::Result<Self> {
        let mut raw = [0u8; 3];
        fill_random(&mut raw)?;
        let scale = |b: u8| 64 + (u16::from(b) * 192 / 256) as u8;
        Ok(Self {
            rgba: [scale(raw[0]), scale(raw[1]), scale(raw[2]), 0xff],
        })
    }

    /// An explicit indicator from an RGB triple. Alpha is forced opaque. The
    /// explicit-colour constructor: used by tests that need a known colour to
    /// assert the frame and band by.
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            rgba: [r, g, b, 0xff],
        }
    }

    /// The opaque colour the trusted frame and band are painted in. The secret
    /// is only ever exposed as *pixels on vitrind's own display* — there is
    /// deliberately no accessor that renders it as text (a hex string is
    /// exactly the kind of value that ends up in a log the forger can read).
    pub fn color(&self) -> [u8; 4] {
        self.rgba
    }

    /// A fixed, vivid indicator for tests: deterministic (goldens and pixel
    /// assertions stay stable) and a colour nothing else in the prompt paints
    /// — the card is greys and blues, the scrim darkens, the background is the
    /// test pattern — so a test that finds these bytes found the frame and
    /// nothing else.
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self::from_rgb(0xFF, 0x00, 0xAA)
    }
}

/// Fill `buf` from `getrandom(2)` via libc — the core's raw-syscall crate
/// (see `Cargo.toml`). Retries only `EINTR`; any other failure (or a short
/// read the loop cannot complete) propagates so the caller fails closed rather
/// than shipping a half-random secret. No flags: the kernel pool is long since
/// initialized this late in a session's life, so the call does not block.
///
/// `pub(crate)` for one further caller and deliberately not more: the
/// `consent-injector` build's `super::injector::PromptToken::mint` (issue
/// #138). Sharing this rather than opening a second entropy path keeps the
/// core to **one** call-site shape for randomness — this module is the one
/// that has to be right about it — and the visibility is still narrow in
/// practice, because `crate::consent::indicator` is a private module and so
/// unnameable outside the consent tree.
pub(crate) fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        // SAFETY: `getrandom` writes at most `buf.len() - filled` bytes into
        // the tail of `buf`, which is valid for that many writes; the pointer
        // and length describe exactly that region.
        let n = unsafe {
            libc::getrandom(
                buf[filled..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - filled,
                0,
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "getrandom returned no bytes",
            ));
        }
        filled += n as usize;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_renders_the_secret() {
        // The secret must not become text — a derived Debug would have printed
        // the channel bytes.
        let ind = TrustedIndicator::from_rgb(0xAB, 0xCD, 0xEF);
        let shown = format!("{ind:?}");
        assert_eq!(shown, "TrustedIndicator(<redacted>)");
        for byte in ["171", "205", "239", "ab", "cd", "ef", "AB", "CD", "EF"] {
            assert!(
                !shown.contains(byte),
                "the value leaked through Debug: {shown}"
            );
        }
    }

    /// **The session colour is minted exactly once per process, and nothing —
    /// least of all a VT switch — re-mints it** (WS-E.3.3, D-030(3)).
    ///
    /// Issue #219's acceptance criterion (c) asked for "`TrustedIndicator`
    /// compares equal across pause→resume". That assertion is **vacuous** and
    /// is deliberately not what this test is: the value is `Copy`, both
    /// surfaces hold their own copy, and nothing on either seat-event arm
    /// touches it — so an equality check passes against an empty handler, and
    /// would go on passing against a handler that did the wrong thing
    /// everywhere else. D-030(3) records the substitution.
    ///
    /// What actually holds the property is structural and is checked here: one
    /// call site in the whole crate, in `run_session`, before the backend
    /// begins accepting anyone. Add `self.view.indicator =
    /// TrustedIndicator::generate()?` to the bare-metal `ActivateSession` arm —
    /// which is exactly the "refresh the secret when the session is
    /// interrupted" hygiene reflex D-030(3) exists to refuse — and this goes
    /// red. Nothing else in the workspace would.
    ///
    /// Why rotation is the wrong instinct, in one line: the indicator defends
    /// against a replica by being *stable*, so a colour that changed after
    /// every VT switch would leave the human unable to tell a legitimate change
    /// from a forgery, and the check collapses.
    #[test]
    fn the_session_colour_is_minted_once_and_nothing_re_mints_it() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sites: Vec<String> = Vec::new();
        let mut checked = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the crate's src/ is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("a readable source file");
                checked += 1;
                // Production source only: every file's trailing test module is
                // cut off, or this test's own siblings (and every fixture that
                // mints a throwaway colour) would count as call sites.
                let production = text
                    .split("\n#[cfg(test)]\nmod tests {")
                    .next()
                    .and_then(|t| t.split("\n#[cfg(test)]\npub(crate) mod tests {").next())
                    .unwrap_or(&text);
                // The trailing paren is what separates a call from the doc
                // links this module and `band_witness` legitimately carry.
                for _ in 0..production.matches("TrustedIndicator::generate(").count() {
                    sites.push(path.display().to_string());
                }
            }
        }
        assert!(checked > 20, "the scan must have read the crate: {checked}");
        assert_eq!(
            sites.len(),
            1,
            "the session colour must be minted in exactly one place; found {sites:?}"
        );
        assert!(
            sites[0].ends_with("main.rs"),
            "the mint must stay in `run_session`, before any client can connect; found {}",
            sites[0]
        );
    }

    #[test]
    fn a_generated_indicator_is_opaque_and_visible() {
        // Every channel lands in [64, 255]: never near-black (invisible on the
        // scrim) and always fully opaque (a band, not a tint).
        for _ in 0..256 {
            let ind = TrustedIndicator::generate().expect("entropy is available");
            let c = ind.color();
            assert_eq!(c[3], 0xff, "the frame is opaque");
            for channel in &c[..3] {
                assert!(*channel >= 64, "a visible floor against the scrim");
            }
        }
    }
}
