// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: every `vitrin-protocol` message decoder, fed arbitrary
//! bytes and an arbitrary fd presence.
//!
//! `vitrin-protocol` is pure data plus encode/decode -- **no I/O, no
//! sockets** (crate docs) -- which is exactly what makes it fuzzable
//! in-process: no socketpair, no `vitrind`, no syscalls on the hot path.
//! This is the P1.9.3 (#46) acceptance criterion "cargo-fuzz targets cover
//! decode of arbitrary bytes and arbitrary fd counts; no panics/UB".
//!
//! # What this proves
//!
//! `decode(bytes, fd)` on **every** message type defined in
//! `protocol/vitrin-v0.xml` -- `DECODERS.len()` is asserted against
//! `vitrin_protocol::generated::MESSAGE_COUNT` below, so an IDL message
//! added without a matching decoder entry here fails the assertion the
//! first time the fuzzer runs, not silently. Deliberately no literal
//! count in this prose: the assertion is the gate, and a number written
//! here would be one more thing to forget to bump (it was, at 32, when
//! `vitrin_layout_focus`/`vitrin_layout_arrange` took the total to 36).
//! For **any** byte slice and **any** fd presence, every decoder must:
//!
//! - never panic and never invoke undefined behaviour (libFuzzer's own
//!   contract: a panic or an ASan/UBSan report aborts the process, which
//!   cargo-fuzz reports as a crashing input);
//! - return a typed [`vitrin_protocol::DecodeError`] on anything that is
//!   not a well-formed frame for the selected message -- this is the
//!   "decode must return errors, never panic, on arbitrary bytes" promise
//!   `wire.rs::read_string`'s doc comment makes explicit for the
//!   `checked_add` cursor arithmetic; this target is what actually
//!   exercises that promise against a real fuzzer instead of only the
//!   hand-picked edge cases in `decode_errors.rs`;
//! - round-trip stably on the rare input the fuzzer mutates into an
//!   *actually well-formed* frame: re-encoding a successful decode and
//!   decoding those bytes again must succeed and reproduce the same value
//!   (the same `encode(decode(bytes)) == bytes` property
//!   `tests/roundtrip.rs` checks from the generator side, checked here from
//!   the "found by mutation" side instead of a `proptest` strategy).
//!
//! # Input layout
//!
//! `data[0]` selects which message decoder to call (mod
//! `DECODERS.len()`); `data[1]`'s low bit selects whether an fd
//! accompanies the call (an open `/dev/null`, close-on-exec, freshly
//! opened per call since `decode` takes ownership of an `OwnedFd`); the
//! rest of `data` is handed to `decode` verbatim as the candidate frame
//! bytes -- header included, exactly as a peer's bytes would arrive.
//! Deliberately not gating on `header.opcode`/`fd_count` before dispatch:
//! calling every decoder with the same raw bytes is what fuzzes the
//! `OpcodeMismatch`/`FdCountMismatch` guards themselves, not just the
//! per-argument decode logic behind them.
#![no_main]

use std::fmt::Debug;
use std::fs::File;
use std::os::fd::OwnedFd;

use libfuzzer_sys::fuzz_target;

use vitrin_protocol::generated as gen;
use vitrin_protocol::DecodeError;

type Hello = gen::vitrin_handshake::requests::Hello;
type HandshakeSync = gen::vitrin_handshake::requests::Sync;
type HandshakeError = gen::vitrin_handshake::events::Error;
type HandshakeDone = gen::vitrin_handshake::events::Done;
type GetRealm = gen::vitrin_principal::requests::GetRealm;
type Bound = gen::vitrin_principal::events::Bound;
type Attention = gen::vitrin_principal::events::Attention;
type RequestGrant = gen::vitrin_realm::requests::RequestGrant;
type GetLauncher = gen::vitrin_grant::requests::GetLauncher;
type GetLayoutFocus = gen::vitrin_grant::requests::GetLayoutFocus;
type GetLayoutArrange = gen::vitrin_grant::requests::GetLayoutArrange;
type GetEgress = gen::vitrin_grant::requests::GetEgress;
type Resolved = gen::vitrin_grant::events::Resolved;
type Refused = gen::vitrin_grant::events::Refused;
type Launch = gen::vitrin_launcher::requests::Launch;
type Launched = gen::vitrin_launcher::events::Launched;
type LayoutFocus = gen::vitrin_layout_focus::requests::Focus;
type LayoutSetFullscreen = gen::vitrin_layout_arrange::requests::SetFullscreen;
type RequestConnect = gen::vitrin_egress::requests::RequestConnect;
type Connected = gen::vitrin_egress::events::Connected;
type ConnectFailed = gen::vitrin_egress::events::ConnectFailed;
type ConsentStateEvent = gen::vitrin_consent::events::State;
type CaptureFrame = gen::vitrin_view::requests::CaptureFrame;
type FrameReady = gen::vitrin_view::events::FrameReady;
type PointerMove = gen::vitrin_actuator_pointer::requests::Move;
type PointerButton = gen::vitrin_actuator_pointer::requests::Button;
type PointerScroll = gen::vitrin_actuator_pointer::requests::Scroll;
type TypeText = gen::vitrin_actuator_text::requests::Type;
type CreateSurface = gen::vitrin_shim_session::requests::CreateSurface;
type GetSeat = gen::vitrin_shim_session::requests::GetSeat;
type Configure = gen::vitrin_shim_session::events::Configure;
type SessionSelection = gen::vitrin_shim_session::requests::Selection;
type RequestSelection = gen::vitrin_shim_session::events::RequestSelection;
type OfferSelection = gen::vitrin_shim_session::events::OfferSelection;
type SessionPointerConstraint = gen::vitrin_shim_session::requests::PointerConstraint;
type SessionPointerConstraintState = gen::vitrin_shim_session::events::PointerConstraintState;
type SessionIdleInhibit = gen::vitrin_shim_session::requests::IdleInhibit;
type Attach = gen::vitrin_shim_surface::requests::Attach;
type Damage = gen::vitrin_shim_surface::requests::Damage;
type Commit = gen::vitrin_shim_surface::requests::Commit;
type FrameDone = gen::vitrin_shim_surface::events::FrameDone;
type BufferDone = gen::vitrin_shim_surface::events::BufferDone;
type SeatMotion = gen::vitrin_shim_seat::events::Motion;
type SeatButton = gen::vitrin_shim_seat::events::Button;
type SeatScroll = gen::vitrin_shim_seat::events::Scroll;
type SeatKey = gen::vitrin_shim_seat::events::Key;
type SeatText = gen::vitrin_shim_seat::events::Text;
type SeatRelativeMotion = gen::vitrin_shim_seat::events::RelativeMotion;
type SeatGestureBegin = gen::vitrin_shim_seat::events::GestureBegin;
type SeatGestureSwipeUpdate = gen::vitrin_shim_seat::events::GestureSwipeUpdate;
type SeatGesturePinchUpdate = gen::vitrin_shim_seat::events::GesturePinchUpdate;
type SeatGestureEnd = gen::vitrin_shim_seat::events::GestureEnd;

/// One fresh, close-on-exec fd -- `decode` takes ownership of it, so every
/// call site that wants to hand one over needs its own.
fn devnull_fd() -> OwnedFd {
    File::open("/dev/null")
        .expect("/dev/null must be openable in the fuzz sandbox")
        .into()
}

/// Try one message type's `decode`, discarding the value but keeping the
/// panic-freedom and round-trip checks in one place so the decoder table
/// below is pure data, not one copy of this logic per type. For the fd-less
/// majority of message types the round-trip is a full equality check
/// ([`DecodeMsg::decoded_eq`]); the three fd-bearing types
/// (`vitrin_view.frame_ready`, `vitrin_shim_surface.attach`,
/// `vitrin_egress.connected`) only derive
/// `Debug` in the generated code (an `OwnedFd` field has no `PartialEq`),
/// so their impl of [`DecodeMsg::decoded_eq`] is vacuously `true` -- the
/// panic-freedom and "must decode again" checks below still run for them
/// in full, only the field-level equality is skipped.
fn try_decode<T>(bytes: &[u8], fd: Option<OwnedFd>)
where
    T: Debug + DecodeMsg,
{
    let decoded = match T::decode_msg(bytes, fd) {
        Ok((_object_id, value)) => value,
        Err(_) => return,
    };
    // The fuzzer found bytes (plus fd presence) that decode successfully.
    // Round-trip it: encode the decoded value, decode those fresh bytes,
    // and require the two decoded values to agree. A mismatch here is a
    // genuine decode/encode asymmetry bug, exactly like a `roundtrip.rs`
    // regression -- but discovered by mutation instead of `proptest`, so
    // the crashing input becomes a permanent corpus regression the next
    // run replays for free.
    let reencoded = decoded.encode_msg(0xF00D);
    let (_, redecoded) = T::decode_msg(&reencoded, decoded.fd_for_reencode())
        .expect("re-encoding a value this decoder just accepted must decode again");
    assert!(
        decoded.decoded_eq(&redecoded),
        "decode(encode(decode(bytes))) != decode(bytes): {decoded:?} vs {redecoded:?}"
    );
}

/// Bridges every generated message struct's identical
/// `encode(&self, u32) -> Vec<u8>` / `decode(&[u8], Option<OwnedFd>) -> ...`
/// shape (same pattern `tests/roundtrip.rs` uses) plus two extra hooks:
/// [`fd_for_reencode`], because the three fd-bearing message types need a
/// *fresh* fd supplied for their re-decode half of the round-trip check
/// (their own `fd` field was already consumed by the first `decode`)
/// while every fd-less type needs `None`; and [`decoded_eq`], because
/// those same three types have no `PartialEq` (see `try_decode`'s doc
/// comment) so the round-trip comparison needs a per-type answer instead
/// of a blanket trait bound.
trait DecodeMsg: Sized {
    fn decode_msg(bytes: &[u8], fd: Option<OwnedFd>) -> Result<(u32, Self), DecodeError>;
    fn encode_msg(&self, object_id: u32) -> Vec<u8>;
    fn fd_for_reencode(&self) -> Option<OwnedFd>;
    fn decoded_eq(&self, other: &Self) -> bool;
}

macro_rules! impl_decode_msg_no_fd {
    ($($ty:ty),* $(,)?) => {
        $(
            impl DecodeMsg for $ty {
                fn decode_msg(bytes: &[u8], fd: Option<OwnedFd>) -> Result<(u32, Self), DecodeError> {
                    Self::decode(bytes, fd)
                }
                fn encode_msg(&self, object_id: u32) -> Vec<u8> {
                    self.encode(object_id)
                }
                fn fd_for_reencode(&self) -> Option<OwnedFd> {
                    None
                }
                fn decoded_eq(&self, other: &Self) -> bool {
                    self == other
                }
            }
        )*
    };
}

macro_rules! impl_decode_msg_with_fd {
    ($($ty:ty),* $(,)?) => {
        $(
            impl DecodeMsg for $ty {
                fn decode_msg(bytes: &[u8], fd: Option<OwnedFd>) -> Result<(u32, Self), DecodeError> {
                    Self::decode(bytes, fd)
                }
                fn encode_msg(&self, object_id: u32) -> Vec<u8> {
                    self.encode(object_id)
                }
                fn fd_for_reencode(&self) -> Option<OwnedFd> {
                    Some(devnull_fd())
                }
                fn decoded_eq(&self, _other: &Self) -> bool {
                    // No `PartialEq` (an `OwnedFd` field has none, and a
                    // fresh `/dev/null` fd's raw number is incidental, not
                    // a property to assert on). `try_decode` already
                    // required the re-decode to succeed at all; that is
                    // this type's round-trip proof.
                    true
                }
            }
        )*
    };
}

impl_decode_msg_no_fd!(
    Hello,
    HandshakeSync,
    HandshakeError,
    HandshakeDone,
    GetRealm,
    Bound,
    Attention,
    RequestGrant,
    GetLauncher,
    Resolved,
    Refused,
    ConsentStateEvent,
    CaptureFrame,
    PointerMove,
    PointerButton,
    PointerScroll,
    TypeText,
    CreateSurface,
    GetSeat,
    Configure,
    Damage,
    Commit,
    FrameDone,
    BufferDone,
    SeatMotion,
    SeatButton,
    SeatScroll,
    SeatKey,
    SeatText,
    SeatRelativeMotion,
    SeatGestureBegin,
    SeatGestureSwipeUpdate,
    SeatGesturePinchUpdate,
    SeatGestureEnd,
    Launch,
    Launched,
    GetLayoutFocus,
    GetLayoutArrange,
    GetEgress,
    LayoutFocus,
    LayoutSetFullscreen,
    RequestConnect,
    ConnectFailed,
    SessionSelection,
    RequestSelection,
    OfferSelection,
    SessionPointerConstraint,
    SessionPointerConstraintState,
    SessionIdleInhibit,
);
// The three fd-bearing messages in v0.xml (grep for an `fd`-typed arg),
// matching tests/roundtrip.rs's dedicated-block split.
impl_decode_msg_with_fd!(FrameReady, Attach, Connected);

/// One panic-free-and-round-trips-if-it-decodes wrapper per message type,
/// unified behind this shape so the fuzz target body is a single indexed
/// call, not a match with one arm per message.
type Decoder = fn(&[u8], Option<OwnedFd>);

macro_rules! decoder_table {
    ($($ty:ty),* $(,)?) => {
        &[$(
            (|bytes: &[u8], fd: Option<OwnedFd>| try_decode::<$ty>(bytes, fd)) as Decoder,
        )*]
    };
}

/// The decoder table, in **IDL declaration order, which is load-bearing** --
/// interfaces top to bottom, each interface's requests before its events, which
/// is also how the scanner assigns the implicit per-interface opcodes.
/// `fuzz/seed_corpus.py` derives its selector indices from the IDL on exactly
/// that assumption, so a table written in any other order silently points every
/// checked-in seed at a decoder it does not claim.
///
/// It really had drifted. `vitrin_principal.attention` and the two layout mints
/// were appended to the END of this table instead of inserted where the IDL puts
/// them, which moved every later index and left the `attach_with_fd` seed
/// feeding an fd to `get_seat` -- an immediate `FdCountMismatch`, so the one
/// seed that exercises the fd path exercised nothing. Nothing caught it because
/// `fuzz/` is its own cargo workspace and no CI job ran `cargo test` inside it.
/// WS-E.2.1 reorders the table and adds that job step in the same commit.
///
/// **That reorder was incomplete, and P2.7.2's second half finished it.**
/// `vitrin_shim_session`'s nine messages sat here as `create_surface`,
/// `get_seat`, `selection`, `configure`, `request_selection`, `offer_selection`,
/// `pointer_constraint`, `pointer_constraint_state`, `idle_inhibit` -- requests
/// and events interleaved, where the IDL puts all five requests before all four
/// events. Indices 24 through 29 therefore named a different message here than
/// `fuzz/seed_corpus.py`'s IDL-derived table named, and nothing was red: the two
/// tables are only ever compared *at the indices a checked-in seed selects*, and
/// no seed selects one of those six. It was a trap armed for the next seed, not
/// a live defect, and it is fixed here rather than left for that seed's author
/// to debug.
///
/// Nothing may be written between the macro's parentheses but bare type names:
/// `seed_corpus_reachability.rs` parses this invocation out of the source.
static DECODERS: &[Decoder] = decoder_table!(
    Hello,
    HandshakeSync,
    HandshakeError,
    HandshakeDone,
    GetRealm,
    Bound,
    Attention,
    RequestGrant,
    GetLauncher,
    GetLayoutFocus,
    GetLayoutArrange,
    GetEgress,
    Resolved,
    Refused,
    ConsentStateEvent,
    CaptureFrame,
    FrameReady,
    PointerMove,
    PointerButton,
    PointerScroll,
    TypeText,
    CreateSurface,
    GetSeat,
    SessionSelection,
    SessionPointerConstraint,
    SessionIdleInhibit,
    Configure,
    RequestSelection,
    OfferSelection,
    SessionPointerConstraintState,
    Attach,
    Damage,
    Commit,
    FrameDone,
    BufferDone,
    SeatMotion,
    SeatButton,
    SeatScroll,
    SeatKey,
    SeatText,
    SeatRelativeMotion,
    SeatGestureBegin,
    SeatGestureSwipeUpdate,
    SeatGesturePinchUpdate,
    SeatGestureEnd,
    Launch,
    Launched,
    LayoutFocus,
    LayoutSetFullscreen,
    RequestConnect,
    Connected,
    ConnectFailed,
);

fuzz_target!(|data: &[u8]| {
    assert_eq!(
        DECODERS.len(),
        gen::MESSAGE_COUNT,
        "the fuzz decoder table must cover every message protocol/vitrin-v0.xml defines"
    );
    if data.len() < 2 {
        return;
    }
    let selector = data[0] as usize % DECODERS.len();
    let want_fd = data[1] & 1 == 1;
    let frame = &data[2..];
    let fd = if want_fd { Some(devnull_fd()) } else { None };
    DECODERS[selector](frame, fd);
});
