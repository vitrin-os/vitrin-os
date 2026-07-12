//! Targeted negative-path tests for `DecodeError`, one per variant not
//! already exercised by `wire`'s own unit tests (`Truncated`, `InvalidUtf8`,
//! `EmbeddedNul`, `StringTooLong` are covered there at the primitive level).
//! The round-trip property test only ever constructs *valid* messages, so it
//! never drives these failure paths; this file closes that gap by decoding
//! through actual generated message types rather than the raw `wire`
//! primitives.

use vitrin_protocol::generated as gen;
use vitrin_protocol::DecodeError;

#[test]
fn invalid_enum_value_is_rejected() {
    // `consent_state` is a plain enum with defined entries 0, 1, 2.
    let err = gen::vitrin_consent::ConsentState::from_wire(99).unwrap_err();
    assert_eq!(
        err,
        DecodeError::InvalidEnumValue {
            interface: "vitrin_consent",
            enum_name: "consent_state",
            value: 99,
        }
    );
}

#[test]
fn invalid_bitfield_value_is_rejected() {
    // `vitrin_grant.verb` is a bitfield with VALID_MASK 1|2|4 = 7; bit 8 is
    // reserved for a future verb and out of range today.
    assert_eq!(gen::vitrin_grant::Verb::VALID_MASK, 7);
    let err = gen::vitrin_grant::Verb::from_bits(8).unwrap_err();
    assert_eq!(
        err,
        DecodeError::InvalidBitfieldValue {
            interface: "vitrin_grant",
            enum_name: "verb",
            value: 8,
        }
    );
    // every subset of the defined bits, including all of them together, is legal
    for v in 0..=7u32 {
        gen::vitrin_grant::Verb::from_bits(v).expect("every subset of defined bits is valid");
    }
}

#[test]
fn fd_count_mismatch_missing_fd_is_rejected() {
    // vitrin_view.frame_ready::HAS_FD is true; decoding without a supplied
    // fd must fail, not silently proceed.
    let value = gen::vitrin_view::events::FrameReady {
        fd: std::io::pipe().unwrap().0.into(),
        format: gen::vitrin_view::Format::Xrgb8888,
        width: 1,
        height: 1,
        stride: 4,
        flags: gen::vitrin_view::FrameFlags::default(),
    };
    let bytes = value.encode(1);
    let err = gen::vitrin_view::events::FrameReady::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 1,
            actual: 0
        }
    );
}

#[test]
fn fd_count_mismatch_unsolicited_fd_is_rejected() {
    // vitrin_handshake.sync::HAS_FD is false; an unsolicited fd must be
    // rejected rather than silently dropped.
    let value = gen::vitrin_handshake::requests::Sync { cookie: 7 };
    let bytes = value.encode(1);
    let (reader, _writer) = std::io::pipe().unwrap();
    let err =
        gen::vitrin_handshake::requests::Sync::decode(&bytes, Some(reader.into())).unwrap_err();
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 0,
            actual: 1
        }
    );
}

#[test]
fn fd_count_header_byte_lying_high_is_rejected() {
    // vitrin_handshake.sync::HAS_FD is false. Forge a byte-identical valid
    // frame except for the header's own fd_count byte (wire offset 7),
    // flipped from 0 to 1 by hand -- no out-of-band fd is supplied, so the
    // fd.is_some() vs HAS_FD check alone would pass (false == false).
    // docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as fatal
    // when *either* the header's fd_count disagrees with the signature *or*
    // fds are attached to a message declaring none; this hits the first
    // disjunct, which the out-of-band check alone cannot see.
    let value = gen::vitrin_handshake::requests::Sync { cookie: 42 };
    let mut bytes = value.encode(1);
    assert_eq!(bytes[7], 0, "fd_count byte for a zero-fd message must be 0");
    bytes[7] = 1;
    let err = gen::vitrin_handshake::requests::Sync::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 0,
            actual: 1
        }
    );
}

#[test]
fn fd_count_header_byte_lying_low_is_rejected() {
    // vitrin_view.frame_ready::HAS_FD is true. Forge a byte-identical valid
    // frame except for the header's own fd_count byte, flipped from 1 to 0
    // by hand, while still supplying the real out-of-band fd -- so the
    // fd.is_some() vs HAS_FD check alone would pass (true == true) and
    // cannot see this tamper. Before the fix, no generated decode ever
    // re-read this byte after the header was parsed, so this exact frame
    // decoded successfully despite the header explicitly declaring 0 fds
    // for a message whose signature requires one.
    let (fd_for_encode, fd_for_decode) = std::io::pipe().unwrap();
    let value = gen::vitrin_view::events::FrameReady {
        fd: fd_for_encode.into(),
        format: gen::vitrin_view::Format::Xrgb8888,
        width: 1,
        height: 1,
        stride: 4,
        flags: gen::vitrin_view::FrameFlags::default(),
    };
    let mut bytes = value.encode(1);
    assert_eq!(bytes[7], 1, "fd_count byte for a one-fd message must be 1");
    bytes[7] = 0;
    let err = gen::vitrin_view::events::FrameReady::decode(&bytes, Some(fd_for_decode.into()))
        .unwrap_err();
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 1,
            actual: 0
        }
    );
}

#[test]
fn trailing_bytes_after_a_complete_message_are_rejected() {
    let value = gen::vitrin_handshake::requests::Sync { cookie: 42 };
    let mut bytes = value.encode(1);
    bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let err = gen::vitrin_handshake::requests::Sync::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::TrailingBytes {
            consumed: bytes.len() - 4,
            total: bytes.len(),
        }
    );
}

#[test]
fn truncated_buffer_is_rejected() {
    let value = gen::vitrin_handshake::requests::Sync { cookie: 42 };
    let bytes = value.encode(1);
    // Drop the last byte of the cookie argument.
    let short = &bytes[..bytes.len() - 1];
    let err = gen::vitrin_handshake::requests::Sync::decode(short, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::Truncated {
            needed: bytes.len(),
            available: short.len(),
        }
    );
}

#[test]
fn truncated_header_is_rejected() {
    let err = gen::vitrin_handshake::requests::Sync::decode(&[1, 2, 3], None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::Truncated {
            needed: 8,
            available: 3,
        }
    );
}

#[test]
fn string_over_bound_is_rejected_through_a_generated_message() {
    // vitrin_principal.get_realm's `name` bound is 64 bytes; hand-craft a
    // frame claiming a 65-byte string to hit the bound check through the
    // generated decode path (wire::tests already covers the primitive).
    let mut bytes = Vec::new();
    vitrin_protocol::wire::FrameHeader {
        object_id: 1,
        size: 0,
        opcode: gen::vitrin_principal::requests::GetRealm::OPCODE,
        fd_count: 0,
    }
    .encode_with_placeholder_size(&mut bytes);
    vitrin_protocol::wire::write_uint(&mut bytes, 2); // realm new_id
    vitrin_protocol::wire::write_uint(&mut bytes, 65); // claimed length: over the 64-byte bound
    bytes.extend(std::iter::repeat_n(b'a', 68)); // 65 padded to 68
    vitrin_protocol::wire::patch_size(&mut bytes);

    let err = gen::vitrin_principal::requests::GetRealm::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::StringTooLong {
            max: 64,
            actual: 65
        }
    );
}

#[test]
fn decode_error_bridges_to_the_wire_error_enum() {
    use gen::vitrin_handshake::Error as WireError;
    assert_eq!(
        DecodeError::Truncated {
            needed: 8,
            available: 0
        }
        .to_wire_error(),
        WireError::Oversized
    );
    assert_eq!(
        DecodeError::InvalidUtf8.to_wire_error(),
        WireError::InvalidArgument
    );
    assert_eq!(
        DecodeError::FdCountMismatch {
            expected: 1,
            actual: 0
        }
        .to_wire_error(),
        WireError::FdViolation
    );
}
