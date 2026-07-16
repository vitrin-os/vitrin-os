//! Targeted negative-path tests for `DecodeError`, one per variant not
//! already exercised by `wire`'s own unit tests (`Truncated`, `InvalidUtf8`,
//! `EmbeddedNul`, `StringTooLong` are covered there at the primitive level).
//! The round-trip property test only ever constructs *valid* messages, so it
//! never drives these failure paths; this file closes that gap by decoding
//! through actual generated message types rather than the raw `wire`
//! primitives.

use vitrin_protocol::generated as gen;
use vitrin_protocol::DecodeError;

/// Build a frame by hand: header (with the given opcode/fd_count), then
/// `payload` written by the closure, then the size field patched to the real
/// total. Used by tests that need a byte-consistent frame with one hostile
/// argument inside it.
fn craft_frame(opcode: u8, fd_count: u8, payload: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut bytes = Vec::new();
    vitrin_protocol::wire::FrameHeader {
        object_id: 1,
        size: 0,
        opcode,
        fd_count,
    }
    .encode_with_placeholder_size(&mut bytes);
    payload(&mut bytes);
    vitrin_protocol::wire::patch_size(&mut bytes);
    bytes
}

#[test]
fn invalid_enum_value_is_rejected() {
    // `consent_state` is a plain enum with defined entries 0, 1, 2 -- checked
    // both at the helper level and through the actual generated decode, so a
    // codegen regression that stopped routing enum args through `from_wire`
    // could not pass on the helper test alone.
    let err = gen::vitrin_consent::ConsentState::from_wire(99).unwrap_err();
    assert_eq!(
        err,
        DecodeError::InvalidEnumValue {
            interface: "vitrin_consent",
            enum_name: "consent_state",
            value: 99,
        }
    );

    let bytes = craft_frame(gen::vitrin_consent::events::State::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, 99);
    });
    let err = gen::vitrin_consent::events::State::decode(&bytes, None).unwrap_err();
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

    // ... and through the generated decode: `resolved` carries `verbs` as its
    // second argument, after a valid `outcome`.
    let bytes = craft_frame(gen::vitrin_grant::events::Resolved::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, gen::vitrin_grant::Outcome::ALL[0].to_wire());
        vitrin_protocol::wire::write_uint(out, 8); // invalid verbs bit
        vitrin_protocol::wire::write_uint(out, gen::vitrin_grant::Persistence::ALL[0].to_wire());
        vitrin_protocol::wire::write_uint(out, 0); // expiry_ms
    });
    let err = gen::vitrin_grant::events::Resolved::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::InvalidBitfieldValue {
            interface: "vitrin_grant",
            enum_name: "verb",
            value: 8,
        }
    );
}

#[test]
fn invalid_utf8_is_rejected_through_a_generated_message() {
    // get_realm's `name`: a 2-byte string that is not valid UTF-8.
    let bytes = craft_frame(gen::vitrin_principal::requests::GetRealm::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, 2); // realm new_id
        vitrin_protocol::wire::write_uint(out, 2); // string length
        out.extend_from_slice(&[0xff, 0xfe, 0, 0]); // bad UTF-8 + padding
    });
    let err = gen::vitrin_principal::requests::GetRealm::decode(&bytes, None).unwrap_err();
    assert_eq!(err, DecodeError::InvalidUtf8);
}

#[test]
fn embedded_nul_is_rejected_through_a_generated_message() {
    let bytes = craft_frame(gen::vitrin_principal::requests::GetRealm::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, 2); // realm new_id
        vitrin_protocol::wire::write_uint(out, 3); // string length
        out.extend_from_slice(b"a\0b\0"); // embedded NUL + 1 padding byte
    });
    let err = gen::vitrin_principal::requests::GetRealm::decode(&bytes, None).unwrap_err();
    assert_eq!(err, DecodeError::EmbeddedNul);
}

#[test]
fn malformed_padding_is_rejected_through_a_generated_message() {
    // A 1-byte string needs 3 padding bytes; conventions 2.2 makes a nonzero
    // one fatal invalid_argument, and accepting it would break the canonical
    // one-value-one-encoding property.
    let bytes = craft_frame(gen::vitrin_principal::requests::GetRealm::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, 2); // realm new_id
        vitrin_protocol::wire::write_uint(out, 1); // string length
        out.extend_from_slice(&[b'a', 0xff, 0, 0]); // nonzero padding byte
    });
    let err = gen::vitrin_principal::requests::GetRealm::decode(&bytes, None).unwrap_err();
    assert_eq!(err, DecodeError::MalformedPadding);
}

#[test]
fn header_size_field_lying_is_rejected() {
    // Forge a valid Sync frame's size field (wire offset 4..6) from 12 to 8:
    // the header now lies about the delivered byte count, which conventions
    // 2.1 makes fatal `oversized`. Before the fix, generated decode never
    // compared the size field to anything and this frame decoded Ok.
    let value = gen::vitrin_handshake::requests::Sync { cookie: 42 };
    let mut bytes = value.encode(1);
    assert_eq!(bytes[4], 12, "sanity: sync frame is 12 bytes");
    bytes[4] = 8;
    let err = gen::vitrin_handshake::requests::Sync::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::SizeMismatch {
            declared: 8,
            actual: 12,
        }
    );
}

#[test]
fn header_opcode_byte_lying_is_rejected() {
    // Forge a valid Sync frame's opcode byte (wire offset 6) from 1 to 0
    // (`hello`'s opcode). Sync and Done have identical payload shapes, so
    // without this check a mis-routing dispatcher decodes the wrong message
    // silently instead of getting an error.
    let value = gen::vitrin_handshake::requests::Sync { cookie: 42 };
    let mut bytes = value.encode(1);
    assert_eq!(bytes[6], 1, "sanity: sync's opcode is 1");
    bytes[6] = 0;
    let err = gen::vitrin_handshake::requests::Sync::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::OpcodeMismatch {
            expected: 1,
            actual: 0,
        }
    );
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
    // The size field must be *consistent* with the padded buffer (else the
    // SizeMismatch check fires first, which has its own test above): craft a
    // frame whose header honestly declares 16 bytes but whose payload holds
    // 4 bytes more than sync's one argument.
    let bytes = craft_frame(gen::vitrin_handshake::requests::Sync::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, 42); // cookie
        out.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // junk
    });
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
    // A frame whose size field consistently declares 11 bytes (so the
    // SizeMismatch check passes) but whose cookie argument is one byte short.
    // A transport-truncated frame with an untouched size field surfaces as
    // SizeMismatch instead -- both map to the wire's fatal `oversized`.
    let value = gen::vitrin_handshake::requests::Sync { cookie: 42 };
    let bytes = value.encode(1);
    let mut short = bytes[..bytes.len() - 1].to_vec();
    short[4] = short.len() as u8;
    let err = gen::vitrin_handshake::requests::Sync::decode(&short, None).unwrap_err();
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
        DecodeError::SizeMismatch {
            declared: 8,
            actual: 12
        }
        .to_wire_error(),
        WireError::Oversized
    );
    assert_eq!(
        DecodeError::InvalidUtf8.to_wire_error(),
        WireError::InvalidArgument
    );
    assert_eq!(
        DecodeError::MalformedPadding.to_wire_error(),
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
    assert_eq!(
        DecodeError::OpcodeMismatch {
            expected: 1,
            actual: 0
        }
        .to_wire_error(),
        WireError::InvalidOpcode
    );
    assert_eq!(DecodeError::NullObject.to_wire_error(), WireError::InvalidObject);
}
