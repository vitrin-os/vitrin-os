// SPDX-License-Identifier: MPL-2.0
//! The physical-input test injector's channel (issue #212, WS-E.1.6): the
//! line vocabulary a harness one process boundary away uses to make
//! **physical-origin** seat input happen in a headless `vitrind`.
//!
//! Compiled only under the `physical-input-injector` cargo feature. A
//! deployment build cannot name any of it, and cannot be made to respond to
//! it: `--physical-input-fd` is refused at *parse* time, exactly as
//! `--consent-injector-fd` is on a build without that feature.
//!
//! # Why it exists at all
//!
//! WS-E.1.6's whole claim is about where physical input goes versus where an
//! agent's actuation goes, and CI cannot produce physical input:
//! [`super::SeatInput::physical`] is private to `crate::input` by design,
//! [`super::intake_physical`] is the nested backend's only caller, headless has
//! no input device, and headless is the only backend CI runs (D-019(4)). A
//! criterion asserting real hardware input in CI would never go green, so
//! issue #212 asks for this instead — and for the discipline the two existing
//! injectors already set.
//!
//! # The real entry point, never a second one
//!
//! Every request here becomes a host event handed to [`super::intake_physical`]
//! — the same function the nested backend's winit handler calls, with the same
//! synthetic-backend trait surface the unit tests drive it through
//! ([`super::synthetic`]) — except keys, which go to
//! [`super::physical_key`], the function `intake_physical`'s own `Keyboard` arm
//! calls and which the nested backend also calls directly (issue #118: winit
//! resolves `logical_key` itself, so the real nested keyboard path bypasses the
//! scancode-only arm). There is no third translation and no way to reach
//! `SeatInput` from here without going through one of those two.
//!
//! Keys are therefore limited to what [`super::invariant_keysym`] can resolve
//! from a scancode alone — Escape, Enter, Tab, arrows, modifiers, F-keys,
//! space — because a headless core has no host keymap to ask and the core is
//! forbidden to grow one. That is a real limit on what a gate here can type,
//! and it is the honest one: the alternative is inventing keysyms in the core,
//! which is exactly what `vitrin_shim_seat.key` carrying keysyms exists to
//! prevent.
//!
//! # What this widens
//!
//! `crate::input`'s module docs lean on `SeatInput::physical`'s privacy being
//! a **compile-time** guarantee: in a headless build no phantom physical-input
//! path can exist, structurally. In a `physical-input-injector` build that is
//! a *runtime* guarantee instead — the feature plus the flag plus possession of
//! an inherited descriptor. Said plainly there and in the feature's Cargo
//! block, because a weaker guarantee stated as the old one is the kind of
//! claim this repo's docs exist to avoid.
//!
//! # Why a socketpair, and why not the vitrin wire format
//!
//! Both answers are [`crate::consent::injector`]'s, which this module
//! deliberately mirrors rather than reasoning afresh: a *named* channel is
//! forgeable by the same-uid confined app, an inherited socketpair has no name
//! to connect to, and authentication is descriptor possession. The vocabulary
//! is bounded ASCII lines rather than framed messages, so the only surface an
//! untrusted peer controls is [`parse_request`], [`MAX_LINE`] bytes at a time.
//!
//! Unlike consent, a signal would not do here for a second reason beyond
//! `kill(2)`'s lack of a payload: an input event *is* a payload — a code, a
//! state, a coordinate — and there is nowhere to put it.
//!
//! # The vocabulary
//!
//! ```text
//!   harness -> core   motion <x> <y>                 (view pixels, f64)
//!                     button <evdev-code> <press|release>
//!                     scroll <vertical|horizontal> <v120>
//!                     key <evdev-scancode> <press|release>
//!                     attention                      (one whole chord tap)
//!                     clipboard <promote|offer>      (one whole modifier chord)
//!                     screenshot                     (one whole modifier chord)
//!
//!   core -> harness   vitrin-physical-input 1        (banner, once)
//!                     ack <injected-event-count>     (per accepted request)
//!                     err <reason>                   (per rejected request)
//! ```
//!
//! `attention` is the *configured* attention chord (WS-E.1.7,
//! [`crate::attention::AttentionChord`]), pressed and released — one line
//! because the chord is a tap and half a tap is not a gesture. It is **not** a
//! second path: it resolves to the chord's own evdev scancode and goes through
//! the same [`super::physical_key`] the `key` line does, twice, so `attention`
//! and `key 125 press` + `key 125 release` are the identical two calls. It
//! exists so a harness names the *gesture* rather than a scancode it had to
//! guess, and so a run started with `--attention-chord rsuper` is exercised
//! with `rsuper` without the harness knowing.
//!
//! `clipboard` is the same idea one gesture up (WS-E.2.1,
//! [`crate::clipboard`]): one line is the *whole* chord — modifiers down,
//! trigger down, trigger up, modifiers up — because half a chord is not a
//! gesture and a harness that sent only the press would leave the core
//! believing Shift is held. It is **not** a second path either: every event it
//! produces comes out of the same [`super::physical_key`] a `key` line uses, on
//! the modifiers' and the trigger's own scancodes, so `clipboard promote` and
//! the five `key` lines that spell it are the identical five calls. Which key
//! it chords is this run's `--clipboard-key`, not the peer's to choose.
//!
//! `screenshot` is the same idea again (WS-E.2.4, [`crate::screenshot`]): one
//! line is the whole configured chord, pressed through [`super::physical_key`]
//! on the modifiers' and the trigger's own scancodes, so `screenshot` and the
//! four `key` lines that spell `ctrl+print` are the identical four calls.
//!
//! It matters more here than for the two above, and the reason is issue #216's
//! own "CI cannot press a key" criterion: with this line CI *can*, so the
//! screenshot gate covers the **detection** half — a real chord on real
//! scancodes reaching the real hook stack — as well as the effect half, rather
//! than asserting the effect and leaving the key to a manual runbook.
//!
//! `ack` counts the [`super::SeatInput`]s the intake produced, not the
//! deliveries: whether an event reaches an app is the router's and the shim's
//! business, and a channel that reported delivery would be the core agreeing
//! with itself about the very thing the gate is measuring. A `scroll` whose
//! `v120` rounds to zero legitimately acks `0`.

use smithay::backend::input as host;
use smithay::backend::input::InputEvent;

use super::synthetic::{SyntheticButton, SyntheticHost, SyntheticMotion};
use super::SeatInput;

/// The longest line the core will accept from the peer, `\n` included.
///
/// The peer controls exactly one thing about this channel — the bytes it
/// writes — so the parse surface is bounded before it is interpreted. The
/// longest legal request is a `motion` with two long float literals; 128
/// leaves room for those and for the vocabulary to grow.
#[cfg_attr(not(feature = "physical-input-injector"), allow(dead_code))]
pub(crate) const MAX_LINE: usize = 128;

/// The banner the core writes as soon as it adopts the channel, so a harness
/// can tell an instrumented core from one that merely opened a socket.
#[cfg_attr(not(feature = "physical-input-injector"), allow(dead_code))]
pub(crate) const BANNER: &str = "vitrin-physical-input 1";

/// One well-formed request from the peer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Request {
    /// Absolute pointer position, in realm-view pixels.
    Motion { x: f64, y: f64 },
    /// A pointer button by Linux evdev code (`BTN_LEFT` = 0x110).
    Button { code: u32, pressed: bool },
    /// High-resolution scroll; one wheel notch = ±120.
    Scroll { horizontal: bool, value120: i32 },
    /// A key by Linux evdev **scancode** (`KEY_ESC` = 1), resolved to a
    /// keysym by the core's own layout-invariant table.
    Key { evdev: u32, pressed: bool },
    /// One whole tap of **this run's configured attention chord** (WS-E.1.7):
    /// press then release, through the same [`super::physical_key`] every
    /// other key goes through, on the scancode
    /// [`crate::attention::AttentionChord`] carries.
    ///
    /// A distinct request rather than two `key` lines because the chord is a
    /// *tap* — half of one is not a gesture, and a harness that sent only the
    /// press would leave the core believing the key is held. It carries no
    /// argument for the same reason the wire event does not: which key it is
    /// is the core's own configuration, not the peer's to choose.
    Attention,
    /// One whole chord of **this run's configured clipboard gesture**
    /// (WS-E.2.1): the modifiers down, the trigger down, the trigger up, the
    /// modifiers up, through the same [`super::physical_key`] every other key
    /// goes through.
    ///
    /// The direction is an argument where [`Request::Attention`]'s key is not,
    /// and the asymmetry is the point: *which* key the attention chord uses is
    /// the core's configuration, while *which of the two* clipboard gestures a
    /// harness means is genuinely the harness's to say — they are two different
    /// gestures, not two spellings of one.
    Clipboard { promote: bool },
    /// One whole chord of **this run's configured screenshot gesture**
    /// (WS-E.2.4). No argument: there is one screenshot chord, and which keys
    /// it is spelled with is this run's `--screenshot-chord`, not the peer's to
    /// choose.
    Screenshot,
}

/// Parse one line into a [`Request`], or `None` for anything else.
///
/// A pure function with no authority, so it is compiled for `cargo test` as
/// well as for the feature: its vocabulary is exactly the thing that rots if
/// only a Python test ever exercises it. Same split
/// [`crate::consent::injector`] makes for the same reason.
///
/// Whitespace-separated fields, no quoting, no escapes; a trailing field makes
/// the line a different message rather than a decorated one.
pub(crate) fn parse_request(line: &str) -> Option<Request> {
    let mut fields = line.split_ascii_whitespace();
    let request = match fields.next()? {
        "motion" => Request::Motion {
            x: parse_finite(fields.next()?)?,
            y: parse_finite(fields.next()?)?,
        },
        "button" => Request::Button {
            code: fields.next()?.parse().ok()?,
            pressed: parse_press(fields.next()?)?,
        },
        "scroll" => Request::Scroll {
            horizontal: match fields.next()? {
                "horizontal" => true,
                "vertical" => false,
                _ => return None,
            },
            value120: fields.next()?.parse().ok()?,
        },
        "key" => Request::Key {
            evdev: fields.next()?.parse().ok()?,
            pressed: parse_press(fields.next()?)?,
        },
        "attention" => Request::Attention,
        "screenshot" => Request::Screenshot,
        "clipboard" => Request::Clipboard {
            promote: match fields.next()? {
                "promote" => true,
                "offer" => false,
                _ => return None,
            },
        },
        _ => return None,
    };
    if fields.next().is_some() {
        return None;
    }
    Some(request)
}

/// `press`/`release` and nothing else. A bare boolean spelling (`1`/`0`) is
/// deliberately not accepted: a transposed digit in a harness is a silently
/// different gesture, where a misspelt word is an `err` line.
fn parse_press(field: &str) -> Option<bool> {
    match field {
        "press" => Some(true),
        "release" => Some(false),
        _ => None,
    }
}

/// A finite `f64` coordinate. NaN and the infinities are rejected at the
/// parse boundary rather than clamped downstream: the router's hit test and
/// the wire's 24.8 fixed-point both have defined behaviour for them and none
/// of it is a gesture anybody meant, and `crate::consent::grab` already
/// refuses non-finite view coordinates for the sharper version of the same
/// reason.
fn parse_finite(field: &str) -> Option<f64> {
    let value: f64 = field.parse().ok()?;
    value.is_finite().then_some(value)
}

/// Turn one request into the physical-tagged [`SeatInput`]s the core routes,
/// **through the production intake and nothing else**.
///
/// `view` is the realm-view size in pixels, passed to `intake_physical` as the
/// space absolute positions resolve into — the same argument the nested
/// backend passes its host window size. `attention` is **this run's configured
/// attention chord**, a parameter rather than a constant so the `attention`
/// line presses the key the operator actually chose — a hard-coded 125 would
/// be a second, silently-wrong path the moment anyone passed
/// `--attention-chord rsuper`.
///
/// Returns an empty vector where the real intake would also produce nothing (a
/// `scroll` rounding to zero, a `key` whose scancode is outside the
/// layout-invariant table), so an `ack 0` means "the core's own intake made
/// nothing of this", never "the channel dropped it".
pub(crate) fn intake(
    request: Request,
    view: (i32, i32),
    attention: crate::attention::AttentionChord,
    clipboard: crate::chord::Trigger,
    screenshot: crate::chord::ModChord,
) -> Vec<SeatInput> {
    match request {
        Request::Motion { x, y } => super::intake_physical::<SyntheticHost>(
            &InputEvent::PointerMotionAbsolute {
                event: SyntheticMotion { x, y },
            },
            view,
        ),
        Request::Button { code, pressed } => super::intake_physical::<SyntheticHost>(
            &InputEvent::PointerButton {
                event: SyntheticButton {
                    code,
                    state: if pressed {
                        host::ButtonState::Pressed
                    } else {
                        host::ButtonState::Released
                    },
                },
            },
            view,
        ),
        Request::Scroll {
            horizontal,
            value120,
        } => {
            // v120 straight through the discrete path, exactly as a real
            // wheel reports it -- `intake_physical` prefers `amount_v120` and
            // only falls back to the pixel conversion for continuous sources.
            let v120 = Some(f64::from(value120));
            super::intake_physical::<SyntheticHost>(
                &InputEvent::PointerAxis {
                    event: super::synthetic::SyntheticScroll {
                        v120: if horizontal {
                            (None, v120)
                        } else {
                            (v120, None)
                        },
                        pixels: (None, None),
                    },
                },
                view,
            )
        }
        // Keys go to `physical_key` with no host keysym, which is the exact
        // call `intake_physical`'s own `Keyboard` arm makes and the exact call
        // the nested backend makes for a key winit could not interpret. A
        // headless core has no host keymap to offer, so the layout-invariant
        // table is the whole of what can be typed here (module docs).
        Request::Key { evdev, pressed } => super::physical_key(
            evdev,
            None,
            if pressed {
                vitrin_protocol::generated::vitrin_shim_seat::KeyState::Pressed
            } else {
                vitrin_protocol::generated::vitrin_shim_seat::KeyState::Released
            },
        ),
        // The whole tap, through the **same** `physical_key` the `key` arm
        // uses, on the chord's own scancode: `attention` is a name for a
        // gesture, never a second way into the core.
        Request::Attention => {
            use vitrin_protocol::generated::vitrin_shim_seat::KeyState;
            let mut out = super::physical_key(attention.evdev(), None, KeyState::Pressed);
            out.extend(super::physical_key(
                attention.evdev(),
                None,
                KeyState::Released,
            ));
            out
        }
        // The whole chord, through the **same** `physical_key` every other key
        // goes through, on the configured trigger's own scancode (see
        // `chord_events`).
        Request::Clipboard { promote } => {
            let mut mods = Vec::new();
            if promote {
                mods.push(crate::chord::Modifier::Ctrl.evdev());
            }
            mods.push(crate::chord::Modifier::Shift.evdev());
            chord_events(&mods, clipboard.evdev())
        }
        // WS-E.2.4, and the chord's own scancodes rather than a list assembled
        // here: `ModChord::scancodes` is the single place a gesture's spelling
        // becomes keys, so `--screenshot-chord super+f5` is exercised as
        // `super+f5` without this channel knowing.
        Request::Screenshot => {
            let (mods, trigger) = screenshot.scancodes();
            chord_events(&mods, trigger)
        }
    }
}

/// Press `mods`, tap `trigger`, release `mods` in **reverse** order, all
/// through the same [`super::physical_key`] a bare `key` line uses.
///
/// The reverse release is not cosmetic: the core's matcher tracks press and
/// release per modifier, and a harness that released them in press order would
/// still be correct here but would stop resembling a human, which is the only
/// thing this channel is for.
fn chord_events(mods: &[u32], trigger: u32) -> Vec<SeatInput> {
    use vitrin_protocol::generated::vitrin_shim_seat::KeyState;
    let mut out = Vec::new();
    for evdev in mods {
        out.extend(super::physical_key(*evdev, None, KeyState::Pressed));
    }
    out.extend(super::physical_key(trigger, None, KeyState::Pressed));
    out.extend(super::physical_key(trigger, None, KeyState::Released));
    for evdev in mods.iter().rev() {
        out.extend(super::physical_key(*evdev, None, KeyState::Released));
    }
    out
}

/// The live channel: the adopted socketpair end and the reassembly buffer for
/// the peer's lines.
///
/// Under the feature **alone**, the `dead-man-injector` posture: everything
/// that carries authority — the fd adoption, the reads, the replies — exists
/// only in an instrumented build. Only the pure halves above ([`parse_request`],
/// [`intake`]) are also compiled for `cargo test`, because a vocabulary that
/// only a Python suite ever exercises is a vocabulary that rots.
#[cfg(feature = "physical-input-injector")]
pub(crate) use live::Injector;

#[cfg(feature = "physical-input-injector")]
mod live {
    use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

    use rustix::net::{RecvFlags, SendFlags, SocketType};

    use super::{parse_request, Request, BANNER, MAX_LINE};

    /// The lowest descriptor number the flag may name; `0`/`1`/`2` are the
    /// session's own stdio.
    const LOWEST_FD: RawFd = 3;

    /// The most requests one [`Injector::poll_requests`] call will return —
    /// [`MAX_LINE`]'s companion bound, on a batch rather than a line.
    /// Injecting is cheap, but a peer chooses how many it asks for and one
    /// readiness callback must not become an unbounded loop.
    const MAX_REQUESTS_PER_POLL: usize = 64;

    /// One `recv` at a time, so the reassembly buffer never exceeds
    /// `MAX_LINE + READ_CHUNK`.
    const READ_CHUNK: usize = 256;

    /// The `physical-input-injector` channel (module docs).
    pub(crate) struct Injector {
        /// Used only for its audited `SO_PEERCRED` capture and its `AsFd`.
        /// No vitrin frame is ever sent or received through it.
        conn: vitrin_ipc::Connection,
        /// Bytes read but not yet terminated by a `\n`.
        pending: Vec<u8>,
        /// Set once a write has failed; the channel is then write-dead and
        /// every later reply is dropped with a log line rather than retried
        /// into a stalled compositor loop.
        write_dead: bool,
    }

    /// Whether `fd` may be adopted as the injector channel.
    ///
    /// The rules, and the reason each is a *memory-safety* rule rather than
    /// taste, are [`crate::consent::injector`]'s `validate_injector_fd`'s in
    /// full: [`Injector::adopt`] takes **ownership** of the number, so the
    /// predicate must exclude every descriptor the core already owns — and
    /// the core's own `core.sock` listener is an `AF_UNIX`/`SOCK_STREAM`
    /// socket whose `SO_PEERCRED` reports this very process, so file type,
    /// `SO_TYPE` and the uid check all pass on it. "Connected, never
    /// listening" is what excludes it.
    ///
    /// Restated here rather than shared with that module because the two are
    /// gated on different features and the messages name different flags; the
    /// duplication is four `if`s and a test each, against a `pub(crate)`
    /// helper that would have to be compiled under the union of two feature
    /// sets to be reachable from both.
    pub(crate) fn validate_injector_fd(fd: BorrowedFd<'_>, number: RawFd) -> Result<(), String> {
        if number < LOWEST_FD {
            return Err(format!(
                "`--physical-input-fd {number}` names one of this process's own standard \
                 descriptors; the channel must be inherited on {LOWEST_FD} or above"
            ));
        }
        let stat = rustix::fs::fstat(fd)
            .map_err(|err| format!("`--physical-input-fd {number}` is not open: {err}"))?;
        let kind = rustix::fs::FileType::from_raw_mode(stat.st_mode);
        if kind != rustix::fs::FileType::Socket {
            return Err(format!(
                "`--physical-input-fd {number}` is a {kind:?}, not a socket; the channel is \
                 an inherited AF_UNIX SOCK_STREAM socketpair end"
            ));
        }
        match rustix::net::sockopt::socket_type(fd) {
            Ok(SocketType::STREAM) => {}
            Ok(other) => {
                return Err(format!(
                    "`--physical-input-fd {number}` is a {other:?} socket, not SOCK_STREAM"
                ))
            }
            Err(err) => {
                return Err(format!(
                    "`--physical-input-fd {number}`: cannot read SO_TYPE: {err}"
                ))
            }
        }
        match rustix::net::sockopt::socket_acceptconn(fd) {
            Ok(false) => {}
            Ok(true) => {
                return Err(format!(
                    "`--physical-input-fd {number}` is a LISTENING socket; the channel is the \
                     connected end of an inherited socketpair. A listener at this number is \
                     almost certainly this core's own `core.sock`, which the core already owns \
                     — adopting it would give one descriptor two owners"
                ))
            }
            Err(err) => {
                return Err(format!(
                    "`--physical-input-fd {number}`: cannot read SO_ACCEPTCONN: {err}"
                ))
            }
        }
        if let Err(err) = rustix::net::getpeername(fd) {
            return Err(format!(
                "`--physical-input-fd {number}` is not connected to a peer ({err}); the \
                 channel is the connected end of an inherited socketpair"
            ));
        }
        Ok(())
    }

    impl Injector {
        /// Adopt the descriptor `--physical-input-fd` named.
        ///
        /// Validated *before* adoption ([`validate_injector_fd`]), then set
        /// `FD_CLOEXEC` and `O_NONBLOCK` explicitly rather than trusting the
        /// parent to have done either: a blocking channel would let a peer
        /// stall the compositor loop, and an inheritable one would follow the
        /// realm across `execve` into the very app this core confines.
        pub(crate) fn adopt(number: RawFd) -> Result<Self, String> {
            // SAFETY: `borrow_raw` only asserts that `number` is a valid open
            // descriptor for the duration of the borrow, which the `fstat`
            // inside `validate_injector_fd` checks on the very next line (an
            // unopened number fails EBADF there rather than being used). The
            // borrow is dropped before the `OwnedFd` below takes ownership.
            let borrowed = unsafe { BorrowedFd::borrow_raw(number) };
            validate_injector_fd(borrowed, number)?;
            rustix::io::fcntl_setfd(borrowed, rustix::io::FdFlags::CLOEXEC)
                .map_err(|err| format!("`--physical-input-fd {number}`: FD_CLOEXEC: {err}"))?;
            let flags = rustix::fs::fcntl_getfl(borrowed)
                .map_err(|err| format!("`--physical-input-fd {number}`: F_GETFL: {err}"))?;
            rustix::fs::fcntl_setfl(borrowed, flags | rustix::fs::OFlags::NONBLOCK)
                .map_err(|err| format!("`--physical-input-fd {number}`: O_NONBLOCK: {err}"))?;
            // SAFETY: validated immediately above as an open, *connected*,
            // non-listening SOCK_STREAM socket, which is what excludes the one
            // descriptor the core itself owns at this point — its `core.sock`
            // listener (see `validate_injector_fd`). No other `OwnedFd` in the
            // core wraps a connected socket yet: the listener has accepted
            // nobody, because the event loop has not dispatched. So this
            // `OwnedFd` is the sole owner and closes the descriptor once.
            let owned = unsafe { OwnedFd::from_raw_fd(number) };
            let conn = vitrin_ipc::Connection::from_fd(owned).map_err(|err| {
                format!("`--physical-input-fd {number}`: cannot read SO_PEERCRED: {err}")
            })?;
            let peer = conn.peer_cred();
            // Secondary, exactly as for the consent channel: on a socketpair
            // `SO_PEERCRED` reports the creating process, so this excludes an
            // exotic cross-uid handover and nothing more. Authentication is
            // descriptor possession.
            let euid = rustix::process::geteuid().as_raw();
            if peer.uid != euid {
                return Err(format!(
                    "`--physical-input-fd {number}`: peer uid {} is not this core's uid {euid}; \
                     the physical-input injector channel is same-user only",
                    peer.uid
                ));
            }
            let mut injector = Self {
                conn,
                pending: Vec::new(),
                write_dead: false,
            };
            tracing::warn!(
                fd = number,
                peer_pid = ?peer.pid,
                peer_uid = peer.uid,
                "physical-input-injector: channel adopted; this build can mint \
                 origin=physical seat input on the say-so of whoever holds this descriptor \
                 (issue #212; this build path never ships)"
            );
            injector.send_line(BANNER);
            Ok(injector)
        }

        /// The channel's descriptor, for the loop's readiness source.
        pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
            self.conn.as_fd()
        }

        /// Drain whatever complete lines the peer has written, parsed.
        ///
        /// `Ok(batch)` where each element is `Some(request)` for a line this
        /// core understood and `None` for one it did not (the caller answers
        /// `err` and keeps serving — a typo is not a reason to tear the
        /// channel down). `Err(())` is EOF or a protocol violation: the caller
        /// removes the source.
        pub(crate) fn poll_requests(&mut self) -> Result<Vec<Option<Request>>, ()> {
            let mut out = Vec::new();
            loop {
                if out.len() >= MAX_REQUESTS_PER_POLL {
                    return Ok(out);
                }
                // Complete lines already buffered first, so the bound above
                // is a bound on the batch and the buffer both.
                while let Some(at) = self.pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = self.pending.drain(..=at).collect();
                    if line.len() > MAX_LINE {
                        tracing::warn!(
                            len = line.len(),
                            "physical-input-injector: over-long line; closing the channel"
                        );
                        return Err(());
                    }
                    match std::str::from_utf8(&line[..line.len() - 1]) {
                        Ok(text) => out.push(parse_request(text.trim_end_matches('\r'))),
                        // Not UTF-8: unparseable by construction, answered
                        // `err` like any other unknown line rather than
                        // killing a channel over one bad byte.
                        Err(_) => out.push(None),
                    }
                    if out.len() >= MAX_REQUESTS_PER_POLL {
                        return Ok(out);
                    }
                }
                let mut chunk = [0u8; READ_CHUNK];
                match rustix::net::recv(self.conn.as_fd(), &mut chunk, RecvFlags::empty()) {
                    Ok((0, _)) => {
                        // EOF only when nothing is buffered; a final line
                        // without a trailing newline is dropped, which is the
                        // same rule the consent channel applies.
                        return if out.is_empty() { Err(()) } else { Ok(out) };
                    }
                    Ok((n, _)) => {
                        if self.pending.len() + n > MAX_LINE + READ_CHUNK {
                            tracing::warn!(
                                "physical-input-injector: unterminated line past the bound; \
                                 closing the channel"
                            );
                            return Err(());
                        }
                        self.pending.extend_from_slice(&chunk[..n]);
                    }
                    // `AGAIN` and `WOULDBLOCK` are the same errno on Linux.
                    Err(rustix::io::Errno::AGAIN) => return Ok(out),
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(err) => {
                        tracing::warn!(%err, "physical-input-injector: channel read failed");
                        return Err(());
                    }
                }
            }
        }

        /// Answer one accepted request with the number of intake events it
        /// produced.
        pub(crate) fn ack(&mut self, produced: usize) {
            self.send_line(&format!("ack {produced}"));
        }

        /// Answer one rejected line. The reason is this core's own fixed
        /// string, never an echo of the peer's bytes.
        pub(crate) fn reject(&mut self, reason: &str) {
            self.send_line(&format!("err {reason}"));
        }

        /// One `\n`-terminated line, best effort. A failed write marks the
        /// channel write-dead rather than retrying into a stalled loop.
        fn send_line(&mut self, line: &str) {
            if self.write_dead {
                return;
            }
            let mut bytes = line.as_bytes().to_vec();
            bytes.push(b'\n');
            let mut sent = 0;
            while sent < bytes.len() {
                match rustix::net::send(self.conn.as_fd(), &bytes[sent..], SendFlags::NOSIGNAL) {
                    Ok(0) => break,
                    Ok(n) => sent += n,
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(err) => {
                        tracing::warn!(
                            %err,
                            "physical-input-injector: channel write failed; going quiet"
                        );
                        self.write_dead = true;
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::SeatInputKind;
    use super::*;
    use vitrin_protocol::generated::vitrin_actuator_pointer::{Axis, ButtonState};
    use vitrin_protocol::generated::vitrin_shim_seat::{KeyState, Origin};

    const VIEW: (i32, i32) = (640, 480);

    fn chord() -> crate::attention::AttentionChord {
        crate::attention::AttentionChord::parse(crate::attention::DEFAULT_CHORD)
            .expect("the default attention chord parses")
    }

    fn trigger() -> crate::chord::Trigger {
        crate::chord::Trigger::parse(crate::clipboard::DEFAULT_TRIGGER)
            .expect("the default clipboard trigger parses")
    }

    /// This build's default screenshot chord (WS-E.2.4).
    fn shot() -> crate::chord::ModChord {
        crate::chord::ModChord::parse(crate::screenshot::DEFAULT_SCREENSHOT_CHORD)
            .expect("the default screenshot chord parses")
    }

    #[test]
    fn the_vocabulary_is_exactly_seven_verbs_and_nothing_decorated() {
        assert_eq!(parse_request("attention"), Some(Request::Attention));
        // Argument-free: which key it is is the core's configuration, not the
        // peer's to choose, so a decorated line is a different message.
        assert_eq!(parse_request("attention super"), None);
        assert_eq!(
            parse_request("clipboard promote"),
            Some(Request::Clipboard { promote: true })
        );
        assert_eq!(
            parse_request("clipboard offer"),
            Some(Request::Clipboard { promote: false })
        );
        // The direction is required and closed: `clipboard` alone, or any other
        // word, is a different message rather than a defaulted one.
        assert_eq!(parse_request("clipboard"), None);
        assert_eq!(parse_request("clipboard paste"), None);
        assert_eq!(parse_request("clipboard promote extra"), None);
        // The seventh (WS-E.2.4). Asserted HERE and not only in the screenshot
        // module's own test: this is the test whose stated job is that the
        // vocabulary is CLOSED, so a verb it does not mention is a verb this
        // test does not actually pin.
        assert_eq!(parse_request("screenshot"), Some(Request::Screenshot));
        assert_eq!(parse_request("screenshot full"), None);
        assert_eq!(parse_request("screenshots"), None);
        assert_eq!(
            parse_request("motion 12.5 -3"),
            Some(Request::Motion { x: 12.5, y: -3.0 })
        );
        assert_eq!(
            parse_request("button 272 press"),
            Some(Request::Button {
                code: 272,
                pressed: true
            })
        );
        assert_eq!(
            parse_request("scroll horizontal -120"),
            Some(Request::Scroll {
                horizontal: true,
                value120: -120
            })
        );
        assert_eq!(
            parse_request("key 1 release"),
            Some(Request::Key {
                evdev: 1,
                pressed: false
            })
        );

        // A trailing field is a different message, not a decorated one --
        // otherwise `button 272 press extra` would be accepted as a click and
        // whatever the peer meant by `extra` silently discarded.
        assert_eq!(parse_request("button 272 press extra"), None);
        assert_eq!(parse_request("motion 1"), None);
        assert_eq!(parse_request("describe"), None);
        assert_eq!(parse_request(""), None);
        // A boolean spelling is not a press state (module docs).
        assert_eq!(parse_request("button 272 1"), None);
        assert_eq!(parse_request("scroll sideways 120"), None);
    }

    #[test]
    fn non_finite_coordinates_are_refused_at_the_parse_boundary() {
        // Not clamped downstream: NaN and the infinities have defined
        // behaviour in the hit test and in `Fixed::from_f64` and none of it is
        // a gesture anybody meant.
        for bad in ["motion NaN 0", "motion 0 inf", "motion -inf 0"] {
            assert_eq!(parse_request(bad), None, "{bad} must not parse");
        }
    }

    #[test]
    fn every_request_reaches_the_production_intake_and_is_tagged_physical() {
        // The whole point of the feature: what comes out is what
        // `intake_physical`/`physical_key` produce, tagged by the one
        // constructor that can tag it, and nothing here builds a `SeatInput`
        // of its own.
        let motion = intake(
            Request::Motion { x: 4.0, y: 9.0 },
            VIEW,
            chord(),
            trigger(),
            shot(),
        );
        assert_eq!(motion.len(), 1);
        assert_eq!(motion[0].origin(), Origin::Physical);
        assert_eq!(motion[0].kind(), &SeatInputKind::Motion { x: 4.0, y: 9.0 });

        let button = intake(
            Request::Button {
                code: 0x110,
                pressed: true,
            },
            VIEW,
            chord(),
            trigger(),
            shot(),
        );
        assert_eq!(
            button[0].kind(),
            &SeatInputKind::Button {
                button: 0x110,
                state: ButtonState::Pressed
            }
        );

        let scroll = intake(
            Request::Scroll {
                horizontal: false,
                value120: 120,
            },
            VIEW,
            chord(),
            trigger(),
            shot(),
        );
        assert_eq!(
            scroll[0].kind(),
            &SeatInputKind::Scroll {
                axis: Axis::Vertical,
                value120: 120
            }
        );

        // KEY_ESC through the core's own layout-invariant table, not a
        // keysym the channel chose.
        let key = intake(
            Request::Key {
                evdev: 1,
                pressed: true,
            },
            VIEW,
            chord(),
            trigger(),
            shot(),
        );
        assert_eq!(
            key[0].kind(),
            &SeatInputKind::Key {
                keysym: 0xff1b,
                state: KeyState::Pressed
            }
        );
    }

    #[test]
    fn the_attention_line_is_one_whole_tap_of_the_configured_chord() {
        // Two events, not one: the chord is a *tap*, and a harness that sent
        // only the press would leave the core believing the key is held.
        // Both come out of the same `physical_key` a `key` line uses, on the
        // chord's own scancode -- `attention` names a gesture, it is never a
        // second way into the core.
        let chord = chord();
        let tap = intake(Request::Attention, VIEW, chord, trigger(), shot());
        let by_hand: Vec<_> = intake(
            Request::Key {
                evdev: chord.evdev(),
                pressed: true,
            },
            VIEW,
            chord,
            trigger(),
            shot(),
        )
        .into_iter()
        .chain(intake(
            Request::Key {
                evdev: chord.evdev(),
                pressed: false,
            },
            VIEW,
            chord,
            trigger(),
            shot(),
        ))
        .collect();
        assert_eq!(tap.len(), 2);
        assert_eq!(
            tap.iter().map(|i| i.kind()).collect::<Vec<_>>(),
            by_hand.iter().map(|i| i.kind()).collect::<Vec<_>>(),
            "`attention` must be exactly the two `key` calls, never a third path"
        );
        for event in &tap {
            assert_eq!(event.origin(), Origin::Physical);
        }
        assert_eq!(
            tap[0].kind(),
            &SeatInputKind::Key {
                keysym: 0xffeb,
                state: KeyState::Pressed
            }
        );
    }

    #[test]
    fn the_screenshot_line_is_the_configured_chord_and_nothing_else() {
        // Four events for `ctrl+print`: Ctrl down, Print down, Print up, Ctrl
        // up -- the whole chord, because half a chord is not a gesture and a
        // harness that sent only the press would leave the core believing Ctrl
        // is held. Every one of them comes out of the same `physical_key` a
        // `key` line uses, so this is a *name* for a gesture and never a second
        // way into the core.
        let shot_chord = shot();
        let whole = intake(Request::Screenshot, VIEW, chord(), trigger(), shot_chord);
        let (mods, key) = shot_chord.scancodes();
        assert_eq!(mods, vec![29], "ctrl");
        assert_eq!(key, 99, "KEY_SYSRQ");
        let by_hand: Vec<_> = [(29, true), (99, true), (99, false), (29, false)]
            .into_iter()
            .flat_map(|(evdev, pressed)| {
                intake(
                    Request::Key { evdev, pressed },
                    VIEW,
                    chord(),
                    trigger(),
                    shot_chord,
                )
            })
            .collect();
        assert_eq!(whole.len(), 4);
        assert_eq!(
            whole.iter().map(|i| i.kind()).collect::<Vec<_>>(),
            by_hand.iter().map(|i| i.kind()).collect::<Vec<_>>(),
            "`screenshot` must be exactly the four `key` calls, never a third path"
        );
        for event in &whole {
            assert_eq!(event.origin(), Origin::Physical);
        }
        assert_eq!(
            whole[1].kind(),
            &SeatInputKind::Key {
                keysym: 0xff61,
                state: KeyState::Pressed
            },
            "the trigger really is XK_Print"
        );

        // It presses THIS RUN's chord, not a constant: a run configured with
        // `super+f5` is exercised with `super+f5` without the harness knowing.
        let other = crate::chord::ModChord::parse("super+f5").expect("in the vocabulary");
        let whole = intake(Request::Screenshot, VIEW, chord(), trigger(), other);
        assert_eq!(
            whole[1].kind(),
            &SeatInputKind::Key {
                keysym: 0xffc2,
                state: KeyState::Pressed
            },
            "F5, because that is what this run configured"
        );

        // The vocabulary is closed: `screenshot` takes no argument.
        assert_eq!(parse_request("screenshot"), Some(Request::Screenshot));
        assert_eq!(parse_request("screenshot full"), None);
        assert_eq!(parse_request("screenshots"), None);
    }

    #[test]
    fn intake_produces_nothing_where_the_real_intake_produces_nothing() {
        // A layout-DEPENDENT key: the core has no keymap and refuses to
        // invent one, so this is dropped at intake exactly as it would be from
        // a winit event with no resolved `logical_key`. `ack 0` is the honest
        // answer and the channel does not pretend otherwise.
        const KEY_A: u32 = 30;
        assert!(super::super::invariant_keysym(KEY_A).is_none());
        assert!(intake(
            Request::Key {
                evdev: KEY_A,
                pressed: true
            },
            VIEW,
            chord(),
            trigger(),
            shot(),
        )
        .is_empty());
        // A scroll that rounds to nothing is not a zero-valued event.
        assert!(intake(
            Request::Scroll {
                horizontal: false,
                value120: 0
            },
            VIEW,
            chord(),
            trigger(),
            shot(),
        )
        .is_empty());
    }
}
