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
//! - **Drawn only on the human-visible output path**
//!   ([`crate::backend::human_visible_from_view`]) — the per-prompt frame in
//!   [`super::ConsentSurface::composite_over`] and the always-present band in
//!   [`super::ConsentSurface::composite_trust_band`] — never in
//!   [`crate::scene::Scene::compose`]. The capture service reads the bare
//!   realm view, so the colour cannot leak through `vitrin_view.frame_ready`
//!   — the same structural argument the overlay itself already relies on.
//!
//! # How the human learns it — and why not the log
//!
//! The secret is **never written to any descriptor or file.** That is not
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
//! content reach"). In nested mode the app is a client of *this* core, not of
//! the host, so it cannot read back the host window; in headless mode there is
//! no human to teach, and the overlay is absent from the agent's capture
//! regardless. The human reads the band, then checks a prompt's frame against
//! it: a dialog framed in any other colour, or none, is forged — whoever drew
//! it.

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
fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
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
