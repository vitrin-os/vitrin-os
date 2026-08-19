// SPDX-License-Identifier: Apache-2.0
//! Round-trip property test: `encode(decode(bytes)) == bytes` for every
//! message type across every interface in `protocol/vitrin-v0.xml`.
//!
//! Per `docs/protocol/00-conventions.md` 8.3, the scanner-level round-trip
//! property is about the *canonical* encoding (fixed argument order, explicit
//! enum values, deterministic zero padding, length-prefixed strings, no
//! optional wire fields). Reading "bytes" as that canonical encoding, this
//! test: generates an arbitrary but valid value for a message type, encodes
//! it, decodes those bytes back, re-encodes the decoded value, and asserts
//! the two encoded byte buffers are identical.
//!
//! Coverage is driven by one generic checker (`assert_roundtrip`) plus one
//! per-message arbitrary-value strategy, rather than 32 hand-duplicated test
//! bodies: the round-trip *logic* (encode -> decode -> re-encode -> compare)
//! appears exactly once, so it cannot silently drift per-message as the IDL
//! grows. Every message needs its own strategy regardless (its fields
//! differ), but a forgotten message shows up as a missing line in the
//! declarative `roundtrip_test!` table below, not as a missing hand-rolled
//! test function nobody wrote.
//!
//! The two fd-bearing messages (`vitrin_view.frame_ready`,
//! `vitrin_shim_surface.attach` -- found by grep for an `fd`-typed arg) get
//! their own hand-written `proptest!` blocks instead of the table, since they
//! need a real disposable fd pair from `std::io::pipe()` rather than a pure
//! value strategy.

use std::os::fd::OwnedFd;

use proptest::prelude::*;
use proptest::proptest;

use vitrin_protocol::generated as gen;
use vitrin_protocol::DecodeError;
use vitrin_protocol::Fixed;

// ---------------------------------------------------------------------------
// Type aliases: one per message struct, so strategy/test code below reads
// without three-segment module paths. A few generated names (`Sync`, `Type`,
// `Move`) shadow common std/prelude identifiers if used bare; aliasing to
// something unambiguous avoids any confusion for a reader (Rust itself has no
// trouble with the shadowing -- these are ordinary type-position names).
// ---------------------------------------------------------------------------

type Hello = gen::vitrin_handshake::requests::Hello;
type HandshakeSync = gen::vitrin_handshake::requests::Sync;
type HandshakeError = gen::vitrin_handshake::events::Error;
type HandshakeDone = gen::vitrin_handshake::events::Done;

type GetRealm = gen::vitrin_principal::requests::GetRealm;
type Bound = gen::vitrin_principal::events::Bound;
type Attention = gen::vitrin_principal::events::Attention;

type RequestGrant = gen::vitrin_realm::requests::RequestGrant;

type GetLauncher = gen::vitrin_grant::requests::GetLauncher;
type Resolved = gen::vitrin_grant::events::Resolved;
type Refused = gen::vitrin_grant::events::Refused;

type Launch = gen::vitrin_launcher::requests::Launch;
type Launched = gen::vitrin_launcher::events::Launched;

type GetLayoutFocus = gen::vitrin_grant::requests::GetLayoutFocus;
type GetLayoutArrange = gen::vitrin_grant::requests::GetLayoutArrange;
type LayoutFocusFocus = gen::vitrin_layout_focus::requests::Focus;
type SetFullscreen = gen::vitrin_layout_arrange::requests::SetFullscreen;

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

// ---------------------------------------------------------------------------
// Shared field-level strategies.
// ---------------------------------------------------------------------------

fn any_i32() -> impl Strategy<Value = i32> {
    any::<i32>()
}

fn any_u32() -> impl Strategy<Value = u32> {
    any::<u32>()
}

fn any_fixed() -> impl Strategy<Value = Fixed> {
    any::<i32>().prop_map(Fixed::from_bits)
}

/// A `char` that is never NUL (forbidden in a wire string), spanning 1-, 2-,
/// 3-, and 4-byte UTF-8 widths so `bounded_string` exercises all of them.
/// `char` has no blanket `RangeInclusive` `Strategy` impl in proptest, hence
/// `proptest::char::range` rather than bare `'a'..='z'` literals.
fn safe_char() -> impl Strategy<Value = char> {
    prop_oneof![
        10 => proptest::char::range(' ', '~'),
        2 => proptest::char::range('\u{a1}', '\u{7ff}'),
        2 => proptest::char::range('\u{800}', '\u{d7ff}'),
        2 => proptest::char::range('\u{e000}', '\u{ffff}'),
        2 => proptest::char::range('\u{10000}', '\u{10ffff}'),
    ]
}

/// A `String` whose encoded UTF-8 byte length never exceeds `max_bytes` (the
/// arg's documented wire bound) -- always valid UTF-8, never an embedded NUL,
/// by construction from `safe_char`. Built greedily (append a char unless it
/// would overflow the budget) so there is no rejection-sampling, and
/// generation length is capped at 200 chars even when `max_bytes` is much
/// larger (e.g. `hello.credential`'s 32768): the codec's encode/decode logic
/// does not change shape with string magnitude, so this trades away nothing
/// the property cares about in exchange for fast test runs.
fn bounded_string(max_bytes: u32) -> impl Strategy<Value = String> {
    let cap = max_bytes.min(200) as usize;
    proptest::collection::vec(safe_char(), 0..=cap).prop_map(move |chars| {
        let mut s = String::new();
        for c in chars {
            if s.len() + c.len_utf8() > max_bytes as usize {
                break;
            }
            s.push(c);
        }
        s
    })
}

/// Uniformly pick one of a plain enum's defined entries (whole-value
/// membership) from its generated `ALL` table. Never hand-enumerates
/// variants, so an appended entry is automatically covered.
fn plain_enum<T: Copy + std::fmt::Debug>(all: &'static [T]) -> impl Strategy<Value = T> {
    proptest::sample::select(all)
}

/// Any legal bitfield value: an arbitrary `u32` masked down to the enum's
/// `VALID_MASK`, so every subset of defined bits (including all or none) is
/// reachable and no out-of-mask bit ever is generated.
fn bitfield_mask<T: std::fmt::Debug>(
    valid_mask: u32,
    from_bits: impl Fn(u32) -> Result<T, DecodeError>,
) -> impl Strategy<Value = T> {
    any::<u32>()
        .prop_map(move |v| from_bits(v & valid_mask).expect("masked to VALID_MASK, must be valid"))
}

// ---------------------------------------------------------------------------
// The `Message` trait unifies every generated message struct's identical
// `encode(&self, u32) -> Vec<u8>` / `decode(&[u8], Option<OwnedFd>) -> ...`
// shape behind one generic function, so `assert_roundtrip` is written once.
// ---------------------------------------------------------------------------

trait Message: std::fmt::Debug + Sized {
    fn encode_msg(&self, object_id: u32) -> Vec<u8>;
    fn decode_msg(bytes: &[u8], fd: Option<OwnedFd>) -> Result<(u32, Self), DecodeError>;
}

macro_rules! impl_message {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Message for $ty {
                fn encode_msg(&self, object_id: u32) -> Vec<u8> {
                    self.encode(object_id)
                }
                fn decode_msg(bytes: &[u8], fd: Option<OwnedFd>) -> Result<(u32, Self), DecodeError> {
                    Self::decode(bytes, fd)
                }
            }
        )*
        /// How many message types the table above wires into the round-trip
        /// harness -- asserted against the generated `MESSAGE_COUNT` below so
        /// a message added to the IDL cannot ship silently untested.
        const ROUNDTRIP_COVERED_MESSAGES: usize = [$(stringify!($ty)),*].len();
    };
}

impl_message!(
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
    FrameReady,
    PointerMove,
    PointerButton,
    PointerScroll,
    TypeText,
    CreateSurface,
    GetSeat,
    Configure,
    SessionSelection,
    RequestSelection,
    OfferSelection,
    SessionPointerConstraint,
    SessionPointerConstraintState,
    SessionIdleInhibit,
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
    GetLayoutFocus,
    GetLayoutArrange,
    LayoutFocusFocus,
    SetFullscreen,
);

/// Exhaustiveness gate: the `impl_message!` table must cover every message
/// the IDL defines (the generated `MESSAGE_COUNT` is emitted for exactly this
/// assertion). This catches the "IDL grew, test table didn't" failure mode --
/// a new message makes this test fail until it is added to the table (and,
/// since the `impl Message` exists only to feed a strategy/test line, in
/// practice to the `roundtrip_test!` table too).
#[test]
fn every_idl_message_is_in_the_roundtrip_table() {
    assert_eq!(
        ROUNDTRIP_COVERED_MESSAGES,
        gen::MESSAGE_COUNT,
        "a message defined in protocol/vitrin-v0.xml is missing from (or extra in) \
         roundtrip.rs's impl_message! table"
    );
}

/// The property itself: encode the generated value, decode those bytes back,
/// re-encode the decoded value, and assert the two encoded buffers are
/// byte-for-byte identical. `object_id` round-trips too since it lives in the
/// frame header rather than any argument.
fn assert_roundtrip<T: Message>(value: T, object_id: u32, fd_for_decode: Option<OwnedFd>) {
    let bytes1 = value.encode_msg(object_id);
    let (decoded_object_id, decoded) = T::decode_msg(&bytes1, fd_for_decode).unwrap_or_else(|e| {
        panic!("decode of our own valid encoding failed: {e} (value={value:?}, bytes={bytes1:?})")
    });
    assert_eq!(
        decoded_object_id, object_id,
        "object_id must round-trip unchanged"
    );
    let bytes2 = decoded.encode_msg(object_id);
    assert_eq!(
        bytes1, bytes2,
        "encode(decode(bytes)) != bytes for {value:?}"
    );
}

// ---------------------------------------------------------------------------
// Per-message arbitrary-value strategies. Field order mirrors each generated
// struct's declaration; construction uses field-init shorthand so a field
// naming mismatch is a compile error, not a silent mix-up.
// ---------------------------------------------------------------------------

fn hello() -> impl Strategy<Value = Hello> {
    (
        any_u32(),
        any_u32(),
        bounded_string(2048),
        bounded_string(32),
        bounded_string(32768),
    )
        .prop_map(
            |(version, principal, identity, credential_type, credential)| Hello {
                version,
                principal,
                identity,
                credential_type,
                credential,
            },
        )
}

fn handshake_sync() -> impl Strategy<Value = HandshakeSync> {
    any_u32().prop_map(|cookie| HandshakeSync { cookie })
}

fn handshake_error() -> impl Strategy<Value = HandshakeError> {
    (
        any_u32(),
        plain_enum(gen::vitrin_handshake::Error::ALL),
        bounded_string(1024),
    )
        .prop_map(|(object_id, code, message)| HandshakeError {
            object_id,
            code,
            message,
        })
}

fn handshake_done() -> impl Strategy<Value = HandshakeDone> {
    any_u32().prop_map(|cookie| HandshakeDone { cookie })
}

fn get_realm() -> impl Strategy<Value = GetRealm> {
    (any_u32(), bounded_string(64)).prop_map(|(realm, name)| GetRealm { realm, name })
}

fn bound() -> impl Strategy<Value = Bound> {
    bounded_string(2048).prop_map(|identity| Bound { identity })
}

/// `vitrin_principal.attention` carries no arguments, forever (IDL): the
/// window's length is a server-side security parameter, not something a client
/// may build a timer off. Same shape as `vitrin_shim_surface.commit`.
fn attention() -> impl Strategy<Value = Attention> {
    Just(Attention {})
}

fn request_grant() -> impl Strategy<Value = RequestGrant> {
    (
        any_u32(),
        any_u32(),
        any_u32(),
        any_u32(),
        any_u32(),
        bounded_string(256),
        bitfield_mask(
            gen::vitrin_grant::Verb::VALID_MASK,
            gen::vitrin_grant::Verb::from_bits,
        ),
        any_u32(),
        any_u32(),
        plain_enum(gen::vitrin_grant::Persistence::ALL),
        any_u32(),
    )
        .prop_map(
            |(
                grant,
                consent,
                view,
                pointer,
                text,
                resource,
                verbs,
                expiry_ms,
                max_event_rate,
                persistence,
                flags,
            )| {
                RequestGrant {
                    grant,
                    consent,
                    view,
                    pointer,
                    text,
                    resource,
                    verbs,
                    expiry_ms,
                    max_event_rate,
                    persistence,
                    flags,
                }
            },
        )
}

fn get_launcher() -> impl Strategy<Value = GetLauncher> {
    any_u32().prop_map(|launcher| GetLauncher { launcher })
}

fn resolved() -> impl Strategy<Value = Resolved> {
    (
        plain_enum(gen::vitrin_grant::Outcome::ALL),
        bitfield_mask(
            gen::vitrin_grant::Verb::VALID_MASK,
            gen::vitrin_grant::Verb::from_bits,
        ),
        plain_enum(gen::vitrin_grant::Persistence::ALL),
        any_u32(),
    )
        .prop_map(|(outcome, verbs, persistence, expiry_ms)| Resolved {
            outcome,
            verbs,
            persistence,
            expiry_ms,
        })
}

fn refused() -> impl Strategy<Value = Refused> {
    (
        bitfield_mask(
            gen::vitrin_grant::Verb::VALID_MASK,
            gen::vitrin_grant::Verb::from_bits,
        ),
        plain_enum(gen::vitrin_grant::Refusal::ALL),
        any_u32(),
    )
        .prop_map(|(verb, code, retry_after_ms)| Refused {
            verb,
            code,
            retry_after_ms,
        })
}

fn consent_state_event() -> impl Strategy<Value = ConsentStateEvent> {
    plain_enum(gen::vitrin_consent::ConsentState::ALL).prop_map(|state| ConsentStateEvent { state })
}

fn capture_frame() -> impl Strategy<Value = CaptureFrame> {
    Just(CaptureFrame {})
}

/// Non-fd fields of `frame_ready`, in field order; the fd itself is spliced
/// in by the dedicated `proptest!` block below.
fn frame_ready_fields() -> impl Strategy<
    Value = (
        gen::vitrin_view::Format,
        u32,
        u32,
        u32,
        gen::vitrin_view::FrameFlags,
    ),
> {
    (
        plain_enum(gen::vitrin_view::Format::ALL),
        any_u32(),
        any_u32(),
        any_u32(),
        bitfield_mask(
            gen::vitrin_view::FrameFlags::VALID_MASK,
            gen::vitrin_view::FrameFlags::from_bits,
        ),
    )
}

fn pointer_move() -> impl Strategy<Value = PointerMove> {
    (any_i32(), any_i32()).prop_map(|(x, y)| PointerMove { x, y })
}

fn pointer_button() -> impl Strategy<Value = PointerButton> {
    (
        any_u32(),
        plain_enum(gen::vitrin_actuator_pointer::ButtonState::ALL),
    )
        .prop_map(|(button, state)| PointerButton { button, state })
}

fn pointer_scroll() -> impl Strategy<Value = PointerScroll> {
    (
        plain_enum(gen::vitrin_actuator_pointer::Axis::ALL),
        any_i32(),
    )
        .prop_map(|(axis, value120)| PointerScroll { axis, value120 })
}

fn type_text() -> impl Strategy<Value = TypeText> {
    bounded_string(4096).prop_map(|text| TypeText { text })
}

fn create_surface() -> impl Strategy<Value = CreateSurface> {
    any_u32().prop_map(|surface| CreateSurface { surface })
}

fn get_seat() -> impl Strategy<Value = GetSeat> {
    any_u32().prop_map(|seat| GetSeat { seat })
}

fn configure() -> impl Strategy<Value = Configure> {
    (bounded_string(64), any_u32(), any_u32()).prop_map(|(realm, width, height)| Configure {
        realm,
        width,
        height,
    })
}

/// The cross-realm clipboard's three messages (WS-E.2.1, issue #213).
///
/// `mime` and `data` are exercised at their **declared** bounds rather than at
/// the one value the core serves: the round trip is about the wire, and the
/// wire's bound is what a hostile peer will push against.
fn session_selection() -> impl Strategy<Value = SessionSelection> {
    (
        any_u32(),
        plain_enum(gen::vitrin_shim_session::SelectionStatus::ALL),
        bounded_string(32),
        bounded_string(61440),
    )
        .prop_map(|(serial, status, mime, data)| SessionSelection {
            serial,
            status,
            mime,
            data,
        })
}

fn request_selection() -> impl Strategy<Value = RequestSelection> {
    any_u32().prop_map(|serial| RequestSelection { serial })
}

fn offer_selection() -> impl Strategy<Value = OfferSelection> {
    (bounded_string(32), bounded_string(61440))
        .prop_map(|(mime, data)| OfferSelection { mime, data })
}

/// A nullable `object` argument. `None` is the null object id, which is `0`
/// on the wire and legal only where `allow-null` is declared (conventions
/// section 3); `Some(0)` is deliberately never generated, because `0` *is*
/// the null id and is not a value a conformant sender can mean by `Some`.
/// The first such argument in the protocol is `pointer_constraint.surface`.
fn nullable_object() -> impl Strategy<Value = Option<u32>> {
    prop_oneof![
        1 => Just(None),
        4 => (1u32..=u32::MAX).prop_map(Some),
    ]
}

/// The pointer-constraint pair (WS-E.4.2, issue #222).
///
/// `surface` is exercised both null and non-null: `kind = none` is the
/// withdrawal, and the IDL requires `surface` to be null in exactly that
/// arm — but that is a *semantic* rule the core enforces, not a codec one,
/// so the round trip deliberately generates every combination the wire
/// admits rather than only the well-formed ones.
fn session_pointer_constraint() -> impl Strategy<Value = SessionPointerConstraint> {
    (
        any_u32(),
        nullable_object(),
        plain_enum(gen::vitrin_shim_session::PointerConstraintKind::ALL),
        plain_enum(gen::vitrin_shim_session::PointerConstraintLifetime::ALL),
        any_i32(),
        any_i32(),
        any_u32(),
        any_u32(),
    )
        .prop_map(|(serial, surface, kind, lifetime, x, y, width, height)| {
            SessionPointerConstraint {
                serial,
                surface,
                kind,
                lifetime,
                x,
                y,
                width,
                height,
            }
        })
}

/// The idle inhibit (D-042, issue #306).
///
/// `surface` is exercised both null and non-null in both `state` arms, for
/// `session_pointer_constraint`'s reason: the IDL requires a null `surface`
/// when `state` is `released`, but that is a *semantic* rule the core enforces
/// and not a codec one, so the round trip generates every combination the wire
/// admits rather than only the well-formed ones.
fn session_idle_inhibit() -> impl Strategy<Value = SessionIdleInhibit> {
    (
        nullable_object(),
        plain_enum(gen::vitrin_shim_session::IdleInhibitState::ALL),
    )
        .prop_map(|(surface, state)| SessionIdleInhibit { surface, state })
}

fn session_pointer_constraint_state() -> impl Strategy<Value = SessionPointerConstraintState> {
    (
        any_u32(),
        plain_enum(gen::vitrin_shim_session::PointerConstraintStatus::ALL),
    )
        .prop_map(|(serial, state)| SessionPointerConstraintState { serial, state })
}

/// Non-fd fields of `attach`, in field order; the fd itself is spliced in by
/// the dedicated `proptest!` block below.
type AttachFields = (
    u32,
    gen::vitrin_shim_surface::Kind,
    gen::vitrin_view::Format,
    u32,
    u32,
    u32,
);

fn attach_fields() -> impl Strategy<Value = AttachFields> {
    (
        any_u32(),
        plain_enum(gen::vitrin_shim_surface::Kind::ALL),
        plain_enum(gen::vitrin_view::Format::ALL),
        any_u32(),
        any_u32(),
        any_u32(),
    )
}

fn damage() -> impl Strategy<Value = Damage> {
    (any_i32(), any_i32(), any_i32(), any_i32()).prop_map(|(x, y, width, height)| Damage {
        x,
        y,
        width,
        height,
    })
}

fn commit() -> impl Strategy<Value = Commit> {
    Just(Commit {})
}

fn frame_done() -> impl Strategy<Value = FrameDone> {
    any_u32().prop_map(|time_ms| FrameDone { time_ms })
}

fn buffer_done() -> impl Strategy<Value = BufferDone> {
    (
        any_u32(),
        plain_enum(gen::vitrin_shim_surface::BufferStatus::ALL),
    )
        .prop_map(|(buffer_id, status)| BufferDone { buffer_id, status })
}

fn seat_motion() -> impl Strategy<Value = SeatMotion> {
    (
        any_fixed(),
        any_fixed(),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(x, y, origin)| SeatMotion { x, y, origin })
}

fn seat_button() -> impl Strategy<Value = SeatButton> {
    (
        any_u32(),
        plain_enum(gen::vitrin_actuator_pointer::ButtonState::ALL),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(button, state, origin)| SeatButton {
            button,
            state,
            origin,
        })
}

fn seat_scroll() -> impl Strategy<Value = SeatScroll> {
    (
        plain_enum(gen::vitrin_actuator_pointer::Axis::ALL),
        any_i32(),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(axis, value120, origin)| SeatScroll {
            axis,
            value120,
            origin,
        })
}

fn seat_key() -> impl Strategy<Value = SeatKey> {
    (
        any_u32(),
        plain_enum(gen::vitrin_shim_seat::KeyState::ALL),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(keysym, state, origin)| SeatKey {
            keysym,
            state,
            origin,
        })
}

fn seat_text() -> impl Strategy<Value = SeatText> {
    (
        bounded_string(4096),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(text, origin)| SeatText { text, origin })
}

fn seat_relative_motion() -> impl Strategy<Value = SeatRelativeMotion> {
    (
        any_fixed(),
        any_fixed(),
        any_fixed(),
        any_fixed(),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(
            |(dx, dy, dx_unaccel, dy_unaccel, origin)| SeatRelativeMotion {
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
                origin,
            },
        )
}

fn seat_gesture_begin() -> impl Strategy<Value = SeatGestureBegin> {
    (
        plain_enum(gen::vitrin_shim_seat::GestureKind::ALL),
        any_u32(),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(kind, fingers, origin)| SeatGestureBegin {
            kind,
            fingers,
            origin,
        })
}

fn seat_gesture_swipe_update() -> impl Strategy<Value = SeatGestureSwipeUpdate> {
    (
        any_fixed(),
        any_fixed(),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(dx, dy, origin)| SeatGestureSwipeUpdate { dx, dy, origin })
}

fn seat_gesture_pinch_update() -> impl Strategy<Value = SeatGesturePinchUpdate> {
    (
        any_fixed(),
        any_fixed(),
        any_fixed(),
        any_fixed(),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(dx, dy, scale, rotation, origin)| SeatGesturePinchUpdate {
            dx,
            dy,
            scale,
            rotation,
            origin,
        })
}

fn seat_gesture_end() -> impl Strategy<Value = SeatGestureEnd> {
    (
        plain_enum(gen::vitrin_shim_seat::GestureKind::ALL),
        plain_enum(gen::vitrin_shim_seat::GestureState::ALL),
        plain_enum(gen::vitrin_shim_seat::Origin::ALL),
    )
        .prop_map(|(kind, state, origin)| SeatGestureEnd {
            kind,
            state,
            origin,
        })
}

fn launch() -> impl Strategy<Value = Launch> {
    Just(Launch {})
}

fn launched() -> impl Strategy<Value = Launched> {
    bounded_string(64).prop_map(|realm| Launched { realm })
}

fn get_layout_focus() -> impl Strategy<Value = GetLayoutFocus> {
    any_u32().prop_map(|layout_focus| GetLayoutFocus { layout_focus })
}

fn get_layout_arrange() -> impl Strategy<Value = GetLayoutArrange> {
    any_u32().prop_map(|layout_arrange| GetLayoutArrange { layout_arrange })
}

fn layout_focus_focus() -> impl Strategy<Value = LayoutFocusFocus> {
    Just(LayoutFocusFocus {})
}

fn set_fullscreen() -> impl Strategy<Value = SetFullscreen> {
    plain_enum(gen::vitrin_layout_arrange::Mode::ALL).prop_map(|mode| SetFullscreen { mode })
}

// ---------------------------------------------------------------------------
// One `#[test]` per message, generated from the table below. Each expands to
// exactly: generate an object_id and a value, run `assert_roundtrip`. Adding
// a message the IDL grows is a one-line addition here, not a new hand-rolled
// test body.
// ---------------------------------------------------------------------------

macro_rules! roundtrip_test {
    ($test_name:ident, $strategy:expr) => {
        proptest! {
            #[test]
            fn $test_name(object_id in any::<u32>(), v in $strategy) {
                assert_roundtrip(v, object_id, None);
            }
        }
    };
}

roundtrip_test!(roundtrip_vitrin_handshake_hello, hello());
roundtrip_test!(roundtrip_vitrin_handshake_sync, handshake_sync());
roundtrip_test!(roundtrip_vitrin_handshake_error, handshake_error());
roundtrip_test!(roundtrip_vitrin_handshake_done, handshake_done());

roundtrip_test!(roundtrip_vitrin_principal_get_realm, get_realm());
roundtrip_test!(roundtrip_vitrin_principal_bound, bound());
roundtrip_test!(roundtrip_vitrin_principal_attention, attention());

roundtrip_test!(roundtrip_vitrin_realm_request_grant, request_grant());

roundtrip_test!(roundtrip_vitrin_grant_get_launcher, get_launcher());
roundtrip_test!(roundtrip_vitrin_grant_resolved, resolved());
roundtrip_test!(roundtrip_vitrin_grant_refused, refused());

roundtrip_test!(roundtrip_vitrin_consent_state, consent_state_event());

roundtrip_test!(roundtrip_vitrin_view_capture_frame, capture_frame());
// vitrin_view.frame_ready: fd-bearing, see dedicated block below.

roundtrip_test!(roundtrip_vitrin_actuator_pointer_move, pointer_move());
roundtrip_test!(roundtrip_vitrin_actuator_pointer_button, pointer_button());
roundtrip_test!(roundtrip_vitrin_actuator_pointer_scroll, pointer_scroll());

roundtrip_test!(roundtrip_vitrin_actuator_text_type, type_text());

roundtrip_test!(
    roundtrip_vitrin_shim_session_create_surface,
    create_surface()
);
roundtrip_test!(roundtrip_vitrin_shim_session_get_seat, get_seat());
roundtrip_test!(roundtrip_vitrin_shim_session_configure, configure());
roundtrip_test!(roundtrip_vitrin_shim_session_selection, session_selection());
roundtrip_test!(
    roundtrip_vitrin_shim_session_request_selection,
    request_selection()
);
roundtrip_test!(
    roundtrip_vitrin_shim_session_offer_selection,
    offer_selection()
);
roundtrip_test!(
    roundtrip_vitrin_shim_session_pointer_constraint,
    session_pointer_constraint()
);
roundtrip_test!(
    roundtrip_vitrin_shim_session_pointer_constraint_state,
    session_pointer_constraint_state()
);
roundtrip_test!(
    roundtrip_vitrin_shim_session_idle_inhibit,
    session_idle_inhibit()
);

// vitrin_shim_surface.attach: fd-bearing, see dedicated block below.
roundtrip_test!(roundtrip_vitrin_shim_surface_damage, damage());
roundtrip_test!(roundtrip_vitrin_shim_surface_commit, commit());
roundtrip_test!(roundtrip_vitrin_shim_surface_frame_done, frame_done());
roundtrip_test!(roundtrip_vitrin_shim_surface_buffer_done, buffer_done());

roundtrip_test!(roundtrip_vitrin_shim_seat_motion, seat_motion());
roundtrip_test!(roundtrip_vitrin_shim_seat_button, seat_button());
roundtrip_test!(roundtrip_vitrin_shim_seat_scroll, seat_scroll());
roundtrip_test!(roundtrip_vitrin_shim_seat_key, seat_key());
roundtrip_test!(roundtrip_vitrin_shim_seat_text, seat_text());
roundtrip_test!(
    roundtrip_vitrin_shim_seat_relative_motion,
    seat_relative_motion()
);
roundtrip_test!(
    roundtrip_vitrin_shim_seat_gesture_begin,
    seat_gesture_begin()
);
roundtrip_test!(
    roundtrip_vitrin_shim_seat_gesture_swipe_update,
    seat_gesture_swipe_update()
);
roundtrip_test!(
    roundtrip_vitrin_shim_seat_gesture_pinch_update,
    seat_gesture_pinch_update()
);
roundtrip_test!(roundtrip_vitrin_shim_seat_gesture_end, seat_gesture_end());

roundtrip_test!(roundtrip_vitrin_launcher_launch, launch());
roundtrip_test!(roundtrip_vitrin_launcher_launched, launched());
roundtrip_test!(roundtrip_vitrin_grant_get_layout_focus, get_layout_focus());
roundtrip_test!(
    roundtrip_vitrin_grant_get_layout_arrange,
    get_layout_arrange()
);
roundtrip_test!(roundtrip_vitrin_layout_focus_focus, layout_focus_focus());
roundtrip_test!(
    roundtrip_vitrin_layout_arrange_set_fullscreen,
    set_fullscreen()
);

// ---------------------------------------------------------------------------
// The two fd-bearing messages in v0.xml (grep for an `fd`-typed arg):
// `vitrin_view.frame_ready` and `vitrin_shim_surface.attach`. Each gets a
// real, disposable fd pair from `std::io::pipe()` (stable since Rust 1.87;
// this workspace pins its toolchain in `rust-toolchain.toml` and declares
// `rust-version` in the workspace `Cargo.toml`) rather than skipping fd
// coverage: one end is moved into the value that gets encoded, the other is
// handed to `decode` as the out-of-band fd, exactly mirroring how a real
// transport would deliver bytes and an SCM_RIGHTS fd side by side. `encode`
// never inspects the fd's value (fd bytes never enter the buffer), so which
// end plays which role does not affect the round-trip byte comparison --
// this also incidentally confirms that indifference.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn roundtrip_vitrin_view_frame_ready(
        object_id in any::<u32>(),
        (format, width, height, stride, flags) in frame_ready_fields(),
    ) {
        let (reader, writer) = std::io::pipe().expect("creating a disposable pipe for fd round-trip coverage");
        let value = FrameReady {
            fd: OwnedFd::from(reader),
            format,
            width,
            height,
            stride,
            flags,
        };
        assert_roundtrip(value, object_id, Some(OwnedFd::from(writer)));
    }
}

proptest! {
    #[test]
    fn roundtrip_vitrin_shim_surface_attach(
        object_id in any::<u32>(),
        (buffer_id, kind, format, width, height, stride) in attach_fields(),
    ) {
        let (reader, writer) = std::io::pipe().expect("creating a disposable pipe for fd round-trip coverage");
        let value = Attach {
            buffer_id,
            fd: OwnedFd::from(reader),
            kind,
            format,
            width,
            height,
            stride,
        };
        assert_roundtrip(value, object_id, Some(OwnedFd::from(writer)));
    }
}
