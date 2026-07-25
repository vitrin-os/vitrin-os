// SPDX-License-Identifier: MPL-2.0
//! The out-of-process consent injector's channel (issue #138): the line
//! vocabulary a harness one process boundary away uses to *see* the prompt a
//! headless `vitrind` raised and to say which button the human pressed.
//!
//! Compiled only under `cfg(any(test, feature = "consent-injector"))`, and
//! the half that carries authority ([`Injector`], the fd adoption, the
//! sending) only under the feature. A deployment build cannot name any of it.
//!
//! # Why a socketpair and not a bound path plus a nonce
//!
//! The confined realm runs as the core's own uid (the `SO_PEERCRED`
//! same-user policy), which is the same argument [`super::indicator`] makes
//! about the trust colour: a same-uid app reads `/proc/<core>/cmdline` for a
//! socket path and reads a `0600` nonce file beside it. A *named* channel is
//! therefore forgeable by the very component the consent surface exists to
//! defend against. An inherited socketpair is not — `spawn`'s post-fork
//! `close_range(3, .., CLOSE_RANGE_CLOEXEC)` marks every inherited descriptor
//! close-on-exec, so neither the shim nor the app holds it after `execve`, and
//! there is no name in the filesystem to `connect()` to.
//!
//! **Authentication is descriptor possession.** The `SO_PEERCRED` uid check
//! is retained — it costs nothing, it is the repo's established policy, and
//! the peer's pid/uid are journalled — but it is *secondary*: on a socketpair
//! it reports the creating process, so it excludes an exotic cross-uid
//! handover and nothing more.
//!
//! Which is exactly why [`live::validate_injector_fd`] must be a check on the
//! descriptor's *shape* and not on its peer: the core's own `core.sock`
//! listener is an `AF_UNIX`/`SOCK_STREAM` socket whose `SO_PEERCRED` reports
//! this very process, so every credential test passes on it while adopting it
//! would give one live descriptor two owners. See that function's docs — the
//! rule is "connected, never listening", and it is a memory-safety rule.
//!
//! # Why not a signal
//!
//! `dead-man-injector` uses `SIGUSR1`, and the asymmetry is decisive rather
//! than stylistic: that signal **revokes** authority, so a spurious delivery
//! is fail-safe. A consent injection **grants** it, so a spurious delivery is
//! fail-dangerous. `kill(2)` authenticates nothing beyond same-uid — exactly
//! the confined app's uid — carries no payload (so the answer would have to
//! live in a second, separately unauthenticated file), cannot name a
//! petition, cannot tell the harness *when* a prompt went up, and a signal
//! handler inside a live event loop is not unit-testable. A socket fixes all
//! five, and it leaves `main::block_loop_signals`'s mask untouched.
//!
//! # Why not the vitrin wire format
//!
//! Consent decisions are deliberately **not protocol-expressible**
//! (`docs/protocol/05-vitrin_consent.md`). A header with an object id and an
//! opcode is a wire protocol whatever it is called, so this channel is
//! bounded ASCII lines instead: the only surface an untrusted peer controls
//! is [`parse_request`], `MAX_LINE` bytes at a time, and everything it can
//! say is in one `match`. [`vitrin_ipc::Connection`] is used solely for its
//! audited `SO_PEERCRED` capture and its `AsFd`; `send_message`/`recv_message`
//! are never called and no `FrameHeader` is ever involved.
//!
//! # The vocabulary
//!
//! ```text
//!   harness -> core   describe
//!                     decide <token> <allow-once|allow-while-running|deny>
//!
//!   core -> harness   vitrin-consent-injector 1            (banner, once)
//!                     raised <petition_id> <token>          (unsolicited edge)
//!                     lowered <petition_id>                 (unsolicited edge)
//!                     prompt <none|shown> <token|-> ...     (describe reply)
//!                     decided-ack <queued|no-prompt|unknown-token
//!                                 |no-such-button|malformed>
//! ```
//!
//! **The injector never reports an authority outcome.** It reports *edges*
//! (a card went up, a card came down) and *message acceptance* (`queued`
//! means "handed to the `ConsentGrab`"). Whether authority was conferred is
//! observed where it always is: the agent's `resolved` event on the wire and
//! the flight recorder. A gate must not assert the core's own conclusion
//! about itself.

use crate::consent::Choice;
use crate::grants::PersistenceRung;

/// The longest line the core will accept from the peer, `\n` included.
///
/// The peer controls exactly one thing about this channel — the bytes it
/// writes — so the parse surface is bounded before it is interpreted. The
/// longest legal request is `decide ` + 16 hex + ` ` + `allow-while-running`
/// = 43 bytes; 128 leaves room for the vocabulary to grow.
///
/// **This bounds one line, not a batch of them.** A peer writing many short
/// newline-terminated lines is bounded by the *other* constant,
/// `live::MAX_REQUESTS_PER_POLL`, which stops one readiness callback from
/// serving an unlimited number of them — and by `Injector::poll_requests`
/// draining complete lines before each `recv`, which is what makes the test
/// below "is this line too long" rather than "has the peer sent a lot".
/// Before those two, a burst of `describe\n` grew both the reassembly buffer
/// and the returned batch without limit while this comment claimed otherwise.
pub(crate) const MAX_LINE: usize = 128;

/// The banner the core writes as soon as it adopts the channel, so a harness
/// can tell an instrumented core from one that merely opened a socket.
#[cfg_attr(not(feature = "consent-injector"), allow(dead_code))]
pub(crate) const BANNER: &str = "vitrin-consent-injector 1";

/// A per-prompt capability naming the card currently on screen.
///
/// **The petition id is deliberately not a decidable name.** A fresh token is
/// minted when a prompt is raised, carried on the `raised` edge, and
/// invalidated the instant a decision is accepted. So a replayed `decide` is
/// `unknown-token`, and a decision can never land on a petition that advanced
/// underneath it. Three further layers stand behind it, all pre-existing:
/// `ArmedPrompt::decided`, [`ConsentGrab::lower`]'s decision-dropping, and
/// [`PetitionRegistry::resolve_human`]'s `NotPending`.
///
/// [`ConsentGrab::lower`]: crate::consent::grab::ConsentGrab::lower
/// [`PetitionRegistry::resolve_human`]: crate::petitions::PetitionRegistry::resolve_human
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PromptToken([u8; 8]);

impl std::fmt::Debug for PromptToken {
    /// Redacted, following [`super::indicator::TrustedIndicator`]'s reasoning
    /// one notch down: this value is not a session secret (the peer holding
    /// the descriptor is told it outright), but it *is* an authority-bearing
    /// name on an instrumented build, and the core's own log is readable by
    /// the same-uid confined app. It travels on the socket and nowhere else.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PromptToken(<redacted>)")
    }
}

impl PromptToken {
    /// Mint a fresh token from the kernel CSPRNG.
    ///
    /// Reuses [`super::indicator::fill_random`] rather than opening a second
    /// entropy path: one call-site shape for randomness in the core, and the
    /// same fail-closed posture — an error propagates and the prompt is
    /// raised without an injectable token rather than with a guessable one.
    #[cfg(feature = "consent-injector")]
    pub(crate) fn mint() -> std::io::Result<Self> {
        let mut raw = [0u8; 8];
        super::indicator::fill_random(&mut raw)?;
        Ok(Self(raw))
    }

    /// The token as 16 lowercase hex characters — its only textual form.
    pub(crate) fn to_hex(self) -> String {
        let mut out = String::with_capacity(16);
        for byte in self.0 {
            out.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble < 16"));
            out.push(char::from_digit((byte & 0x0f) as u32, 16).expect("nibble < 16"));
        }
        out
    }

    /// Parse exactly 16 **lowercase** hex characters, or `None`.
    ///
    /// Case-strict on purpose: the core emits one spelling, so accepting a
    /// second one would only widen what a peer can say without widening what
    /// the core can mean.
    pub(crate) fn parse_hex(raw: &str) -> Option<Self> {
        if raw.len() != 16 {
            return None;
        }
        let mut out = [0u8; 8];
        let bytes = raw.as_bytes();
        for (i, slot) in out.iter_mut().enumerate() {
            let hi = lower_hex_digit(bytes[2 * i])?;
            let lo = lower_hex_digit(bytes[2 * i + 1])?;
            *slot = (hi << 4) | lo;
        }
        Some(Self(out))
    }

    /// An explicit token, for tests that need two that differ.
    #[cfg(test)]
    pub(crate) fn from_bytes(raw: [u8; 8]) -> Self {
        Self(raw)
    }
}

fn lower_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Everything a peer may say. Nothing else is a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Request {
    /// Snapshot the consent surface: geometry, the live token, and — when a
    /// card is up — the card's own footprint of the human-visible
    /// framebuffer as a sealed memfd.
    Describe,
    /// Press a button on the prompt `token` names.
    Decide { token: PromptToken, choice: Choice },
}

/// What the core did with a `decide` line. Never an authority outcome (module
/// docs) — only whether the message was accepted for the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecideAck {
    /// Handed to the round's [`ConsentGrab`](crate::consent::grab::ConsentGrab).
    Queued,
    /// No card is up, so there is nothing to press.
    NoPrompt,
    /// The token names no live prompt: a replay, a stale token, or one whose
    /// petition timed out under it.
    UnknownToken,
    /// The named button is not one the raised prompt offers
    /// ([`PromptContent::choices`](crate::consent::PromptContent::choices)).
    NoSuchButton,
    /// The line was not a request at all.
    Malformed,
}

impl DecideAck {
    pub(crate) fn word(self) -> &'static str {
        match self {
            DecideAck::Queued => "queued",
            DecideAck::NoPrompt => "no-prompt",
            DecideAck::UnknownToken => "unknown-token",
            DecideAck::NoSuchButton => "no-such-button",
            DecideAck::Malformed => "malformed",
        }
    }
}

/// The word for a button, exhaustively matched over the
/// [`Choice`]/[`PersistenceRung`] product.
///
/// Exhaustive on purpose: a rung added to the ladder fails to **compile**
/// here, so a new button can never become silently un-nameable (or, worse,
/// silently share another rung's word) on this channel. [`parse_choice`] is
/// asserted to be this function's inverse over
/// [`PersistenceRung::ALL`](crate::grants::PersistenceRung::ALL) in the tests
/// below, so the two cannot drift.
pub(crate) fn choice_word(choice: Choice) -> &'static str {
    match choice {
        Choice::Allow(PersistenceRung::Once) => "allow-once",
        Choice::Allow(PersistenceRung::WhileRunning) => "allow-while-running",
        Choice::Deny => "deny",
    }
}

/// The inverse of [`choice_word`], or `None` for anything else.
fn parse_choice(word: &str) -> Option<Choice> {
    match word {
        "allow-once" => Some(Choice::Allow(PersistenceRung::Once)),
        "allow-while-running" => Some(Choice::Allow(PersistenceRung::WhileRunning)),
        "deny" => Some(Choice::Deny),
        _ => None,
    }
}

/// Parse one `\n`-stripped line into a [`Request`], or `None` for **anything
/// else at all**.
///
/// Deliberately strict and total, with no normalisation: no case folding, no
/// whitespace trimming, no tolerance for a trailing `\r` or an extra field.
/// Every relaxation is a second spelling of an authority-bearing message, and
/// the caller's answer to `None` is `decided-ack malformed` with **nothing
/// queued** — not a synthesised `Deny`. That distinction matters: a denial
/// nobody took would be a lie in the flight recorder, whereas the petition
/// timing out is equally fail-closed and true.
pub(crate) fn parse_request(line: &str) -> Option<Request> {
    if line.is_empty() || line.len() > MAX_LINE {
        return None;
    }
    // Printable ASCII only: no NUL, no control characters, no UTF-8 above
    // 0x7e. A peer cannot make the core interpret bytes it cannot spell.
    if !line.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return None;
    }
    let mut fields = line.split(' ');
    let request = match fields.next()? {
        "describe" => Request::Describe,
        "decide" => {
            let token = PromptToken::parse_hex(fields.next()?)?;
            let choice = parse_choice(fields.next()?)?;
            Request::Decide { token, choice }
        }
        _ => return None,
    };
    // A trailing field is a different message, not a decorated one.
    if fields.next().is_some() {
        return None;
    }
    Some(request)
}

/// The live channel: the adopted socketpair end, its peer credentials, and
/// the reassembly buffer for the peer's lines.
#[cfg(feature = "consent-injector")]
pub(crate) use live::Injector;

#[cfg(feature = "consent-injector")]
mod live {
    use std::io::IoSlice;
    use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

    use rustix::net::{
        RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketType,
    };

    use super::{parse_request, PromptToken, Request, BANNER, MAX_LINE};
    use crate::petitions::PetitionId;

    /// The lowest descriptor number the flag may name. `0`/`1`/`2` are the
    /// session's own stdio; adopting one of those would take the core's log
    /// away from it and turn every `tracing::warn!` into channel garbage.
    const LOWEST_FD: RawFd = 3;

    /// The most requests one [`Injector::poll_requests`] call will return.
    ///
    /// The companion bound to [`MAX_LINE`]: that one bounds a single line,
    /// this one bounds a *batch*. Both are needed, because answering a
    /// request is expensive (a recomposite, a readback, a `sendmsg` with a
    /// descriptor) and the peer chooses how many it asks for.
    ///
    /// 32 is far above anything the harness pipelines — the largest batch in
    /// `tests/integration/test_consent_injector.py` is two `decide` lines in
    /// one write, and that test's determinism depends on both landing in one
    /// callback — and far below a number that could wedge a dispatch.
    const MAX_REQUESTS_PER_POLL: usize = 32;

    /// One `recv` at a time. Named because it is the second half of the
    /// reassembly buffer's bound: a capped call leaves behind at most one
    /// unterminated line ([`MAX_LINE`]) plus one chunk's worth of complete
    /// lines it stopped short of, so `pending` never exceeds
    /// `MAX_LINE + READ_CHUNK` — asserted in the tests below.
    const READ_CHUNK: usize = 256;

    /// The `consent-injector` channel (module docs).
    pub(crate) struct Injector {
        /// Used only for its audited `SO_PEERCRED` capture and its `AsFd`.
        /// No frame is ever sent or received through it.
        conn: vitrin_ipc::Connection,
        /// Bytes read but not yet terminated by a `\n`.
        pending: Vec<u8>,
        /// The petition whose card is currently up, as this channel last
        /// heard. Cleared on the `lowered` edge.
        live: Option<PetitionId>,
        /// The token naming that card. Cleared *earlier* than `live` -- the
        /// instant a decision is accepted -- so a replay while the same card
        /// is still on screen is `unknown-token` (a spent name) rather than
        /// `no-prompt` (which would be a false statement about the screen).
        token: Option<PromptToken>,
        /// Set once a write has failed; the channel is then write-dead and
        /// every later notification is dropped with a log line rather than
        /// retried into a stalled compositor loop.
        write_dead: bool,
    }

    /// Whether `fd` may be adopted as the injector channel: a plain-predicate
    /// split from [`Injector::adopt`] so the rules are unit-testable without
    /// an `unsafe` adoption in the test.
    ///
    /// Fails closed on every ambiguity; the caller turns a failure into a
    /// **startup error**, never a warning.
    ///
    /// # Why "connected, never listening" is a memory-safety rule, not taste
    ///
    /// [`Injector::adopt`] takes **ownership** of the number this returns
    /// `Ok` for, so the predicate must exclude every descriptor the core
    /// already owns — otherwise the `OwnedFd` is a *second* owner and its
    /// `Drop` closes a descriptor the first owner still holds (best case an
    /// abort on a double close, worst case the number was recycled in between
    /// and an unrelated live descriptor — a client connection, the recorder,
    /// a memfd — is closed instead). File type plus `SO_TYPE` does not
    /// exclude that: the core's own `core.sock` **listener** is an
    /// `AF_UNIX`/`SOCK_STREAM` socket, bound by `bind_core_socket` before
    /// this runs, and `SO_PEERCRED` on it reports this very process, so the
    /// same-uid check in `adopt` passes as well. `--consent-injector-fd 4` on
    /// a core whose listener landed on 4 reproduced exactly that: adopted,
    /// served two clients, then died at shutdown with `IO Safety violation:
    /// owned file descriptor already closed`.
    ///
    /// The channel is by construction the **connected** end of a socketpair
    /// the harness made, so `SO_ACCEPTCONN == 0` plus a successful
    /// `getpeername` states that positively, and nothing the core owns at
    /// adoption time satisfies both: the listener is `SO_ACCEPTCONN == 1`,
    /// and no accepted client connection exists yet (the injector is adopted
    /// in `start_headless`, before the loop dispatches a single event).
    pub(crate) fn validate_injector_fd(fd: BorrowedFd<'_>, number: RawFd) -> Result<(), String> {
        if number < LOWEST_FD {
            return Err(format!(
                "`--consent-injector-fd {number}` names one of this process's own standard \
                 descriptors; the channel must be inherited on {LOWEST_FD} or above"
            ));
        }
        let stat = rustix::fs::fstat(fd)
            .map_err(|err| format!("`--consent-injector-fd {number}` is not open: {err}"))?;
        let kind = rustix::fs::FileType::from_raw_mode(stat.st_mode);
        if kind != rustix::fs::FileType::Socket {
            return Err(format!(
                "`--consent-injector-fd {number}` is a {kind:?}, not a socket; the channel is \
                 an inherited AF_UNIX SOCK_STREAM socketpair end"
            ));
        }
        match rustix::net::sockopt::socket_type(fd) {
            Ok(SocketType::STREAM) => {}
            Ok(other) => {
                return Err(format!(
                    "`--consent-injector-fd {number}` is a {other:?} socket, not SOCK_STREAM"
                ))
            }
            Err(err) => {
                return Err(format!(
                    "`--consent-injector-fd {number}`: cannot read SO_TYPE: {err}"
                ))
            }
        }
        match rustix::net::sockopt::socket_acceptconn(fd) {
            Ok(false) => {}
            Ok(true) => {
                return Err(format!(
                    "`--consent-injector-fd {number}` is a LISTENING socket; the channel is the \
                     connected end of an inherited socketpair. A listener at this number is \
                     almost certainly this core's own `core.sock`, which the core already owns \
                     — adopting it would give one descriptor two owners"
                ))
            }
            Err(err) => {
                return Err(format!(
                    "`--consent-injector-fd {number}`: cannot read SO_ACCEPTCONN: {err}"
                ))
            }
        }
        if let Err(err) = rustix::net::getpeername(fd) {
            return Err(format!(
                "`--consent-injector-fd {number}` is not connected to a peer ({err}); the \
                 channel is the connected end of an inherited socketpair"
            ));
        }
        Ok(())
    }

    impl Injector {
        /// Adopt the descriptor `--consent-injector-fd` named.
        ///
        /// Validated *before* adoption ([`validate_injector_fd`]), then set
        /// `FD_CLOEXEC` and `O_NONBLOCK` explicitly rather than trusting the
        /// parent to have done either: a blocking channel would let a peer
        /// stall the compositor loop, and an inheritable one would follow the
        /// realm across `execve` into the app this whole surface defends
        /// against.
        pub(crate) fn adopt(number: RawFd) -> Result<Self, String> {
            // SAFETY: `borrow_raw` only asserts that `number` is a valid open
            // descriptor for the duration of the borrow, which the `fstat`
            // inside `validate_injector_fd` checks on the very next line (an
            // unopened number fails EBADF there rather than being used). The
            // borrow is dropped before the `OwnedFd` below takes ownership.
            let borrowed = unsafe { BorrowedFd::borrow_raw(number) };
            validate_injector_fd(borrowed, number)?;
            rustix::io::fcntl_setfd(borrowed, rustix::io::FdFlags::CLOEXEC)
                .map_err(|err| format!("`--consent-injector-fd {number}`: FD_CLOEXEC: {err}"))?;
            let flags = rustix::fs::fcntl_getfl(borrowed)
                .map_err(|err| format!("`--consent-injector-fd {number}`: F_GETFL: {err}"))?;
            rustix::fs::fcntl_setfl(borrowed, flags | rustix::fs::OFlags::NONBLOCK)
                .map_err(|err| format!("`--consent-injector-fd {number}`: O_NONBLOCK: {err}"))?;
            // SAFETY: validated immediately above as an open, *connected*,
            // non-listening SOCK_STREAM socket. That is what excludes the one
            // descriptor the core itself owns at this point — its `core.sock`
            // listener, which is otherwise the same file type, the same
            // `SO_TYPE` and the same peer uid (see `validate_injector_fd`'s
            // docs). No other `OwnedFd` in the core wraps a connected socket
            // yet: the listener has accepted nobody, because the event loop
            // has not dispatched. So this `OwnedFd` is the sole owner from
            // here and closes the descriptor exactly once.
            let owned = unsafe { OwnedFd::from_raw_fd(number) };
            let conn = vitrin_ipc::Connection::from_fd(owned).map_err(|err| {
                format!("`--consent-injector-fd {number}`: cannot read SO_PEERCRED: {err}")
            })?;
            let peer = conn.peer_cred();
            // Secondary, and documented as such (module docs): on a
            // socketpair `SO_PEERCRED` reports the creating process, so this
            // excludes an exotic cross-uid handover and nothing more.
            // Authentication is descriptor possession.
            let euid = rustix::process::geteuid().as_raw();
            if peer.uid != euid {
                return Err(format!(
                    "`--consent-injector-fd {number}`: peer uid {} is not this core's uid {euid}; \
                     the consent injector channel is same-user only",
                    peer.uid
                ));
            }
            let mut injector = Self {
                conn,
                pending: Vec::new(),
                live: None,
                token: None,
                write_dead: false,
            };
            tracing::warn!(
                fd = number,
                peer_pid = ?peer.pid,
                peer_uid = peer.uid,
                "consent-injector: channel adopted; consent prompts on this session can be \
                 answered over it (issue #138; this build path never ships)"
            );
            injector.send_line(BANNER, None);
            Ok(injector)
        }

        /// The descriptor, for the calloop readiness source.
        pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
            self.conn.as_fd()
        }

        /// The peer's captured credentials, for the per-decision `warn` that
        /// attributes a synthetic decision to whoever made it.
        ///
        /// Read from the `SO_PEERCRED` [`vitrin_ipc::Connection`] captured at
        /// adoption, never re-derived: the log line must name the peer, not
        /// restate the core's own identity.
        pub(crate) fn peer_cred(&self) -> (Option<i32>, u32) {
            let cred = self.conn.peer_cred();
            (cred.pid, cred.uid)
        }

        /// The petition whose card is up, if any -- regardless of whether
        /// its token has been spent.
        pub(crate) fn armed_petition(&self) -> Option<PetitionId> {
            self.live
        }

        /// The unspent token naming the card that is up, if there is one.
        pub(crate) fn live_token(&self) -> Option<(PetitionId, PromptToken)> {
            Some((self.live?, self.token?))
        }

        /// Announce a card going up, minting the token that names it.
        ///
        /// A minting failure leaves the prompt up and un-injectable: the
        /// petition then times out, which is fail-closed. It does not fail
        /// the session — entropy starvation this late is not a reason to take
        /// a compositor down.
        pub(crate) fn note_raised(&mut self, petition: PetitionId) {
            let token = match PromptToken::mint() {
                Ok(token) => token,
                Err(err) => {
                    self.live = Some(petition);
                    self.token = None;
                    tracing::error!(
                        %petition, %err,
                        "consent-injector: cannot mint a prompt token; this prompt is not \
                         answerable over the channel and will time out"
                    );
                    return;
                }
            };
            self.live = Some(petition);
            self.token = Some(token);
            let line = format!("raised {petition} {}", token.to_hex());
            self.send_line(&line, None);
        }

        /// Announce a card coming down. The token dies with it.
        pub(crate) fn note_lowered(&mut self, petition: PetitionId) {
            self.live = None;
            self.token = None;
            let line = format!("lowered {petition}");
            self.send_line(&line, None);
        }

        /// Spend the live token, so a replayed `decide` is `unknown-token`.
        /// The card stays "up" on this channel until its `lowered` edge.
        pub(crate) fn spend_token(&mut self) {
            self.token = None;
        }

        /// Move **every** complete line out of `pending` onto `out`, leaving
        /// at most one unterminated line behind.
        ///
        /// Draining before each `recv` (rather than after the whole read
        /// loop) is what makes both bounds in [`Self::poll_requests`] real:
        /// the `MAX_LINE` test on `pending` is then a test on *a line*,
        /// which is what `MAX_LINE` documents itself as bounding, rather
        /// than on a batch that merely happens to contain newlines.
        ///
        /// It drains *fully* rather than stopping at the cap on purpose. A
        /// complete line left in `pending` would be a request nothing wakes
        /// the loop for: the readiness source is `Mode::Level` on the
        /// **socket**, so a buffer the core is holding is invisible to it,
        /// and a peer that stopped writing would leave that request unserved
        /// forever. Stopping the *reads* bounds the work; stopping the drain
        /// would strand it.
        fn take_complete_lines(&mut self, out: &mut Vec<Option<Request>>) -> Result<(), ()> {
            loop {
                let Some(nl) = self.pending.iter().position(|b| *b == b'\n') else {
                    return Ok(());
                };
                let line: Vec<u8> = self.pending.drain(..=nl).collect();
                let line = &line[..nl];
                if line.len() > MAX_LINE {
                    tracing::warn!("consent-injector: over-long line; closing the channel");
                    return Err(());
                }
                match std::str::from_utf8(line) {
                    Ok(text) => out.push(parse_request(text)),
                    Err(_) => {
                        tracing::warn!("consent-injector: non-UTF-8 line; closing the channel");
                        return Err(());
                    }
                }
            }
        }

        /// Read what the peer has written and return the complete requests in
        /// it, oldest first — a **bounded** batch, never "everything the peer
        /// can write".
        ///
        /// `Err(())` means the channel is finished — EOF, or a protocol
        /// violation (an over-long line, a non-UTF-8 byte) — and the caller
        /// must drop the source. A malformed *request* is not a violation:
        /// it comes back as `None` in the vector so the caller can answer
        /// `decided-ack malformed` and keep the channel.
        ///
        /// # Both allocations are bounded, per call
        ///
        /// The peer controls how many bytes it writes and how fast, and the
        /// caller (`HeadlessState::service_injector`) answers *each*
        /// `describe` with a full recomposite, a readback and a `sendmsg`
        /// carrying a descriptor. So "read until `EAGAIN`" would
        /// let a peer that pipelines faster than the core drains turn one
        /// readiness callback into an unbounded batch of those — the
        /// compositor's dispatch loop wedged for as long as the peer keeps
        /// writing, with in-flight descriptors piling up at the receiver.
        ///
        /// Hence: complete lines are drained *before* each `recv`, reading
        /// **stops** once [`MAX_REQUESTS_PER_POLL`] requests are in hand, and
        /// whatever is still in the socket stays there. The source is
        /// registered `calloop::Mode::Level`, so a socket left readable fires
        /// again on the next dispatch and the remainder is served then —
        /// bounded work per callback, no request dropped, no descriptor
        /// leaked.
        ///
        /// The exact bound on the returned vector is
        /// `MAX_REQUESTS_PER_POLL + READ_CHUNK`, not `MAX_REQUESTS_PER_POLL`:
        /// the cap is tested between reads, and the last `recv` before it
        /// trips may itself deliver up to [`READ_CHUNK`] complete lines (a
        /// chunk of bare `\n`s is `READ_CHUNK` empty ones). Both are
        /// constants, which is the whole point — the peer no longer chooses
        /// the number. `a_burst_of_requests_is_served_in_bounded_batches`
        /// pins it.
        ///
        /// And `pending` is bounded by `MAX_LINE` after every full drain, so
        /// a peer cannot make the core buffer without limit either — which is
        /// what `MAX_LINE`'s own doc comment claimed and, before the drain
        /// moved ahead of the reads, did not deliver.
        #[allow(clippy::result_unit_err)]
        pub(crate) fn poll_requests(&mut self) -> Result<Vec<Option<Request>>, ()> {
            let mut out = Vec::new();
            loop {
                self.take_complete_lines(&mut out)?;
                if out.len() >= MAX_REQUESTS_PER_POLL {
                    tracing::warn!(
                        served = out.len(),
                        "consent-injector: per-callback request cap reached; whatever is still \
                         in the socket is served on the next dispatch"
                    );
                    break;
                }
                // `pending` now holds at most one unterminated line, so this
                // bound really is `MAX_LINE`'s stated one: the longest thing
                // a peer may spell before a `\n`.
                if self.pending.len() > MAX_LINE {
                    tracing::warn!(
                        buffered = self.pending.len(),
                        "consent-injector: peer sent more than MAX_LINE bytes with no \
                         newline; closing the channel"
                    );
                    return Err(());
                }
                let mut buf = [0u8; READ_CHUNK];
                let read = match rustix::net::recv(
                    self.conn.as_fd(),
                    &mut buf[..],
                    RecvFlags::DONTWAIT,
                ) {
                    Ok((filled, _)) => filled,
                    Err(rustix::io::Errno::AGAIN) | Err(rustix::io::Errno::INTR) => break,
                    Err(err) => {
                        tracing::warn!(%err, "consent-injector: channel read failed; closing it");
                        return Err(());
                    }
                };
                if read == 0 {
                    tracing::warn!("consent-injector: peer closed the channel");
                    return Err(());
                }
                self.pending.extend_from_slice(&buf[..read]);
            }
            Ok(out)
        }

        /// Write one `\n`-terminated line, optionally with one descriptor
        /// riding `SCM_RIGHTS` on the same `sendmsg`.
        ///
        /// `MSG_DONTWAIT`: an `EAGAIN` drops the notification and logs. The
        /// harness then times out — fail-closed for the test — whereas a
        /// blocking write would stall the compositor loop, which P1.2.3's
        /// posture forbids outright.
        pub(crate) fn send_line(&mut self, line: &str, fd: Option<BorrowedFd<'_>>) {
            if self.write_dead {
                return;
            }
            let mut framed = String::with_capacity(line.len() + 1);
            framed.push_str(line);
            framed.push('\n');
            let iov = [IoSlice::new(framed.as_bytes())];
            let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut control = SendAncillaryBuffer::new(&mut space);
            let borrowed;
            if let Some(fd) = fd {
                borrowed = [fd];
                control.push(SendAncillaryMessage::ScmRights(&borrowed));
            }
            match rustix::net::sendmsg(
                self.conn.as_fd(),
                &iov,
                &mut control,
                SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
            ) {
                Ok(sent) if sent == framed.len() => {}
                Ok(sent) => {
                    self.write_dead = true;
                    tracing::error!(
                        sent,
                        want = framed.len(),
                        "consent-injector: short write; the channel is now write-dead"
                    );
                }
                Err(err) => {
                    self.write_dead = true;
                    tracing::error!(
                        %err,
                        "consent-injector: write failed; the channel is now write-dead and \
                         every pending petition will time out"
                    );
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The fd-validation predicate refuses every shape that is not an
        /// inherited `AF_UNIX`/`SOCK_STREAM` socketpair end at or above fd 3.
        ///
        /// Split out of `Injector::adopt` precisely so this can be asserted
        /// without an `unsafe` adoption in a test: `adopt`'s one `unsafe`
        /// block is preceded by exactly this call, so what is checked here is
        /// what stands between the flag and the descriptor.
        #[test]
        fn only_a_stream_socket_at_or_above_fd_three_may_be_adopted() {
            use rustix::net::{AddressFamily, SocketFlags, SocketType};

            let (a, _b) = rustix::net::socketpair(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::CLOEXEC,
                None,
            )
            .expect("a socketpair");
            // The real shape is accepted, at a plausible number.
            assert!(validate_injector_fd(a.as_fd(), 7).is_ok());

            // stdio numbers are refused before anything is read.
            for number in [0, 1, 2, -1] {
                let err = validate_injector_fd(a.as_fd(), number)
                    .expect_err("stdio and negative numbers are never the channel");
                assert!(err.contains("standard descriptors"), "{err}");
            }

            // A datagram socketpair is the right file type and the wrong
            // socket type -- the check that makes `SOCK_TYPE` load-bearing.
            let (dgram, _peer) = rustix::net::socketpair(
                AddressFamily::UNIX,
                SocketType::DGRAM,
                SocketFlags::CLOEXEC,
                None,
            )
            .expect("a datagram socketpair");
            let err =
                validate_injector_fd(dgram.as_fd(), 7).expect_err("SOCK_DGRAM is not this channel");
            assert!(err.contains("SOCK_STREAM"), "{err}");

            // A pipe is not a socket at all.
            let (read, _write) = rustix::pipe::pipe().expect("a pipe");
            let err = validate_injector_fd(read.as_fd(), 7).expect_err("a pipe is not a socket");
            assert!(err.contains("not a socket"), "{err}");
        }

        /// A **listening** `AF_UNIX`/`SOCK_STREAM` socket is refused, and an
        /// unconnected one with it.
        ///
        /// This is the descriptor shape the core already owns: `core.sock`,
        /// bound by `bind_core_socket` before the headless backend starts, is
        /// the right file type, the right `SO_TYPE`, and reports this very
        /// process from `SO_PEERCRED`. Adopting it made the `OwnedFd` in
        /// `Injector::adopt` a *second* owner of a live descriptor, which
        /// aborted the process at shutdown (`IO Safety violation: owned file
        /// descriptor already closed`) and, with a recycled number, would
        /// have closed an unrelated live descriptor instead.
        #[test]
        fn a_listening_or_unconnected_socket_is_never_the_channel() {
            use rustix::net::{AddressFamily, SocketFlags, SocketType};

            // Exactly how the core's own listener is shaped: an AF_UNIX
            // stream socket bound to a path and listening.
            let path = std::env::temp_dir().join(format!(
                "vitrin-injector-listener-{}-{:?}.sock",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_file(&path);
            let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
            let err = validate_injector_fd(listener.as_fd(), 4)
                .expect_err("the core's own listener must never be adoptable");
            assert!(err.contains("LISTENING"), "{err}");

            // ...and it really would have passed every earlier check.
            let stat = rustix::fs::fstat(listener.as_fd()).expect("fstat");
            assert_eq!(
                rustix::fs::FileType::from_raw_mode(stat.st_mode),
                rustix::fs::FileType::Socket
            );
            assert_eq!(
                rustix::net::sockopt::socket_type(listener.as_fd()).expect("SO_TYPE"),
                SocketType::STREAM
            );

            // A socket that was never connected and never bound: the right
            // type, no peer. `getpeername` is what refuses it.
            let lone = rustix::net::socket_with(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::CLOEXEC,
                None,
            )
            .expect("an unconnected socket");
            let err = validate_injector_fd(lone.as_fd(), 7)
                .expect_err("an unconnected socket is not the channel");
            assert!(err.contains("not connected"), "{err}");

            // The genuine article -- a connected socketpair end -- still
            // passes, so the two new rules refuse nothing the harness sends.
            let (a, _b) = rustix::net::socketpair(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::CLOEXEC,
                None,
            )
            .expect("a socketpair");
            validate_injector_fd(a.as_fd(), 7).expect("the real channel is still adoptable");

            drop(listener);
            let _ = std::fs::remove_file(&path);
        }

        /// A socketpair whose core end is wrapped in an [`Injector`], for the
        /// batching tests below. `send_all` writes until the socket buffer
        /// fills and reports how many bytes really landed.
        fn injector_pair() -> (Injector, OwnedFd) {
            use rustix::net::{AddressFamily, SocketFlags, SocketType};

            let (core_end, peer) = rustix::net::socketpair(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::CLOEXEC,
                None,
            )
            .expect("a socketpair");
            let conn = vitrin_ipc::Connection::from_fd(core_end).expect("SO_PEERCRED");
            let injector = Injector {
                conn,
                pending: Vec::new(),
                live: None,
                token: None,
                write_dead: false,
            };
            (injector, peer)
        }

        fn send_all(peer: &OwnedFd, bytes: &[u8]) -> usize {
            let mut written = 0;
            while written < bytes.len() {
                match rustix::net::send(
                    peer.as_fd(),
                    &bytes[written..],
                    rustix::net::SendFlags::DONTWAIT,
                ) {
                    Ok(sent) => written += sent,
                    // The socket buffer filled. What is in it is already far
                    // more than one call's worth, which is what these tests
                    // need.
                    Err(rustix::io::Errno::AGAIN) => break,
                    Err(err) => panic!("send: {err}"),
                }
            }
            written
        }

        /// A burst of newline-terminated requests is served in **bounded**
        /// batches, with nothing dropped and the reassembly buffer bounded
        /// throughout.
        ///
        /// Before the cap, `MAX_LINE` only fired when `pending` held no
        /// newline at all, so a peer writing `describe\n` faster than the
        /// core drained grew both `pending` and the returned vector without
        /// limit — and each returned request costs the caller a recomposite,
        /// a framebuffer readback and a `sendmsg` carrying a descriptor,
        /// inside one calloop callback. `describe` is exactly that expensive
        /// verb, so it is what this burst is made of.
        #[test]
        fn a_burst_of_requests_is_served_in_bounded_batches() {
            const LINE: &str = "describe\n";
            let (mut injector, peer) = injector_pair();
            let burst = LINE.repeat(64 * MAX_REQUESTS_PER_POLL);
            let written = send_all(&peer, burst.as_bytes());
            assert!(
                written > MAX_REQUESTS_PER_POLL * LINE.len() * 4,
                "the test needs several calls' worth in the socket; only {written} landed"
            );

            let ceiling = MAX_REQUESTS_PER_POLL + READ_CHUNK;
            let mut served = 0usize;
            let mut batches = 0usize;
            loop {
                let batch = injector.poll_requests().expect("a legal batch");
                assert!(
                    batch.len() <= ceiling,
                    "one callback served {} requests; the bound is {ceiling}",
                    batch.len()
                );
                assert!(
                    batch.iter().all(|r| *r == Some(Request::Describe)),
                    "every served line is the request the peer wrote"
                );
                assert!(
                    injector.pending.len() <= MAX_LINE,
                    "the reassembly buffer held {} bytes -- MAX_LINE must bound a LINE, not a \
                     batch that happens to contain newlines",
                    injector.pending.len()
                );
                if batch.is_empty() {
                    break;
                }
                served += batch.len();
                batches += 1;
            }
            assert_eq!(
                served,
                written / LINE.len(),
                "every line the peer wrote is served -- just not all in one callback"
            );
            assert!(
                batches > 1,
                "the burst must really have spanned several callbacks, or this test proves \
                 nothing about the cap"
            );
        }

        /// An over-long line with no newline still closes the channel — the
        /// fail-closed behaviour the drain-first restructure must not lose.
        #[test]
        fn an_unterminated_over_long_line_still_closes_the_channel() {
            let (mut injector, peer) = injector_pair();
            let junk = vec![b'x'; MAX_LINE * 8];
            assert!(send_all(&peer, &junk) > MAX_LINE);
            injector
                .poll_requests()
                .expect_err("an unterminated over-long line ends the channel");
        }

        /// The batch the harness really pipelines — two `decide` lines in one
        /// write — still lands in **one** callback.
        ///
        /// `tests/integration/test_consent_injector.py`'s spent-token case
        /// depends on that: the second line has to be judged against a prompt
        /// that is provably still up, which is only true if the core drains
        /// both before `post_dispatch` can lower the card. The cap must sit
        /// far above what the harness sends, and this pins that it does.
        #[test]
        fn the_harnesss_pipelined_pair_still_arrives_in_one_callback() {
            let (mut injector, peer) = injector_pair();
            let token = PromptToken::from_bytes([0xab; 8]).to_hex();
            let pair = format!("decide {token} deny\ndecide {token} allow-while-running\n");
            assert_eq!(send_all(&peer, pair.as_bytes()), pair.len());
            let batch = injector.poll_requests().expect("a legal batch");
            assert_eq!(
                batch.len(),
                2,
                "a pipelined pair must not be split across callbacks"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every representable button has a word on this channel, and
    /// [`parse_choice`] is exactly [`choice_word`]'s inverse.
    ///
    /// Driven off [`PersistenceRung::ALL`] rather than a hand-written list,
    /// so a rung added to the ladder is covered here the moment it exists —
    /// and `choice_word`'s exhaustive match makes forgetting one a compile
    /// error rather than a test failure.
    fn all_choices() -> Vec<Choice> {
        let mut choices: Vec<Choice> = PersistenceRung::ALL
            .iter()
            .copied()
            .map(Choice::Allow)
            .collect();
        choices.push(Choice::Deny);
        choices
    }

    #[test]
    fn every_button_round_trips_through_its_word() {
        for choice in all_choices() {
            let word = choice_word(choice);
            assert_eq!(
                parse_choice(word),
                Some(choice),
                "{word:?} must parse back to the button it names"
            );
        }
        // ...and no two buttons share a word.
        let mut words: Vec<&str> = all_choices().into_iter().map(choice_word).collect();
        words.sort_unstable();
        let before = words.len();
        words.dedup();
        assert_eq!(before, words.len(), "two buttons share one word: {words:?}");
    }

    #[test]
    fn the_two_requests_parse() {
        assert_eq!(parse_request("describe"), Some(Request::Describe));
        let token = PromptToken::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        assert_eq!(token.to_hex(), "0123456789abcdef");
        assert_eq!(
            parse_request("decide 0123456789abcdef allow-while-running"),
            Some(Request::Decide {
                token,
                choice: Choice::Allow(PersistenceRung::WhileRunning),
            })
        );
        assert_eq!(
            parse_request("decide 0123456789abcdef allow-once"),
            Some(Request::Decide {
                token,
                choice: Choice::Allow(PersistenceRung::Once),
            })
        );
        assert_eq!(
            parse_request("decide 0123456789abcdef deny"),
            Some(Request::Decide {
                token,
                choice: Choice::Deny,
            })
        );
    }

    /// **Nothing else is a request.** Every one of these must come back
    /// `None`, so the caller answers `decided-ack malformed` and queues
    /// nothing — the only way this channel could ever widen authority is by
    /// one of these lines quietly meaning "allow".
    #[test]
    fn the_injector_recognises_no_line_that_is_not_a_request() {
        let tok = "0123456789abcdef";
        let cases: Vec<String> = vec![
            String::new(),
            " ".into(),
            "describe ".into(),
            "describe now".into(),
            " describe".into(),
            "DESCRIBE".into(),
            "Describe".into(),
            "decide".into(),
            "decide ".into(),
            format!("decide {tok}"),
            format!("decide {tok} "),
            format!("decide {tok} yes"),
            format!("decide {tok} allow"),
            format!("decide {tok} ALLOW-ONCE"),
            format!("decide {tok} allow_once"),
            format!("decide {tok} allow once"),
            format!("decide {tok} allow-once extra"),
            format!("decide {tok} allow-onceX"),
            format!("decide {tok} allow-while-runnin"),
            format!("decide  {tok} deny"), // two spaces: an empty token field
            format!("decide {tok}  deny"), // two spaces: an empty button field
            format!("decide {} deny", &tok[..15]), // a short token
            format!("decide {tok}0 deny"), // a long token
            format!("decide {} deny", tok.to_uppercase()), // upper-case hex
            "decide 0123456789abcdeg deny".into(), // 'g' is not hex
            format!("decide {tok} deny\r"), // a CR is a byte, not decoration
            format!("decide {tok} deny\t"),
            format!("decide {tok}\tdeny"),
            format!("decide\t{tok} deny"),
            "allow".into(),
            "allow-while-running".into(),
            "yes".into(),
            "1".into(),
            "grant".into(),
            "approve".into(),
            format!("decide {tok} deny; decide {tok} allow-once"),
            format!("decide {tok} deny\0"),
            // Longer than MAX_LINE, even though its prefix is a real request.
            format!("decide {tok} deny{}", " ".repeat(MAX_LINE)),
        ];
        for raw in cases {
            assert_eq!(
                parse_request(&raw),
                None,
                "{raw:?} must not parse as a consent-injector request"
            );
        }
    }

    /// The longest legal request fits inside [`MAX_LINE`] with room to spare,
    /// so the bound is a defence against a hostile peer rather than a
    /// constraint on the vocabulary.
    #[test]
    fn the_longest_legal_request_fits_the_line_bound() {
        let longest = format!(
            "decide {} {}",
            "0".repeat(16),
            all_choices()
                .into_iter()
                .map(choice_word)
                .max_by_key(|w| w.len())
                .expect("the ladder is non-empty")
        );
        assert!(parse_request(&longest).is_some());
        assert!(
            longest.len() * 2 < MAX_LINE,
            "MAX_LINE ({MAX_LINE}) should leave the vocabulary room to grow; longest is {}",
            longest.len()
        );
    }

    #[test]
    fn a_token_round_trips_and_rejects_everything_else() {
        let token = PromptToken::from_bytes([0xff; 8]);
        assert_eq!(PromptToken::parse_hex(&token.to_hex()), Some(token));
        for raw in [
            "",
            "0",
            "0123456789abcde",
            "0123456789abcdef0",
            "  ",
            "0123456789ABCDEF",
        ] {
            assert_eq!(PromptToken::parse_hex(raw), None, "{raw:?} is not a token");
        }
    }

    #[test]
    fn a_token_never_renders_its_value() {
        let token = PromptToken::from_bytes([0xab; 8]);
        let shown = format!("{token:?}");
        assert_eq!(shown, "PromptToken(<redacted>)");
        assert!(
            !shown.contains("ab"),
            "the token leaked through Debug: {shown}"
        );
    }

    #[test]
    fn every_ack_has_a_distinct_word() {
        let acks = [
            DecideAck::Queued,
            DecideAck::NoPrompt,
            DecideAck::UnknownToken,
            DecideAck::NoSuchButton,
            DecideAck::Malformed,
        ];
        let mut words: Vec<&str> = acks.iter().copied().map(DecideAck::word).collect();
        words.sort_unstable();
        let before = words.len();
        words.dedup();
        assert_eq!(before, words.len(), "two acks share one word: {words:?}");
    }
}
