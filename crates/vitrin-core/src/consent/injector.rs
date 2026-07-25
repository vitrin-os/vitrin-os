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
/// = 43 bytes; 128 leaves room for the vocabulary to grow without leaving
/// room for a peer to make the core allocate.
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
            // SAFETY: validated immediately above as an open SOCK_STREAM
            // socket, and nothing else in this process owns it — it was
            // inherited across `fork`/`exec` from the harness and named on
            // the command line, so no other `OwnedFd` in the core wraps it.
            // From here the `Injector` is its sole owner and closes it once.
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

        /// Read whatever the peer has written and return the complete
        /// requests in it, oldest first.
        ///
        /// `Err(())` means the channel is finished — EOF, or a protocol
        /// violation (an over-long line, a non-UTF-8 byte) — and the caller
        /// must drop the source. A malformed *request* is not a violation:
        /// it comes back as `None` in the vector so the caller can answer
        /// `decided-ack malformed` and keep the channel.
        #[allow(clippy::result_unit_err)]
        pub(crate) fn poll_requests(&mut self) -> Result<Vec<Option<Request>>, ()> {
            let mut out = Vec::new();
            loop {
                let mut buf = [0u8; 256];
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
                if self.pending.len() > MAX_LINE {
                    // Either an over-long line or a peer pipelining faster
                    // than the core drains; both are answered by draining
                    // what is complete first, and only then judging.
                    if !self.pending.contains(&b'\n') {
                        tracing::warn!(
                            buffered = self.pending.len(),
                            "consent-injector: peer sent more than MAX_LINE bytes with no \
                             newline; closing the channel"
                        );
                        return Err(());
                    }
                }
            }
            while let Some(nl) = self.pending.iter().position(|b| *b == b'\n') {
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
