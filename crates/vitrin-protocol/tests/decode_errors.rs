// SPDX-License-Identifier: Apache-2.0
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
    // `vitrin_grant.verb` is a bitfield with VALID_MASK
    // 1|2|4|8|16|32|64|128|512
    // = 767. Whether a defined bit is *served* is a property of a deployment
    // and is settled at petition admission (`unsupported`), deliberately not
    // a decode error, so the codec must accept every defined bit whatever
    // any core does with it. That is unchanged by WS-E.1.4 serving
    // `layout_arrange` (16) and `layout_focus` (32), and by WS-E.1.1 serving
    // `realm_launch` (512), in the reference core: this file is the codec's,
    // and the codec never knew which bits were served. Bit 8
    // (`observe_cursor`) remains defined-and-unserved there (D-017), and
    // P2.6.5's bit 64 (`designate_file`) and P2.7.2's bit 128 (`egress`)
    // join it there -- unserved by
    // *every* deployment until the picker and its consent copy exist
    // (P2.6.6/P2.6.8) and until the out-of-core mediating proxy exists
    // (P2.7.3), and in range here regardless, which is the whole point of
    // defining a bit before serving it.
    //
    // **Re-pinned twice**, 575 -> 639 for E2.6 and 639 -> 767 for E2.7, per
    // the repo-wide registry in
    // `docs/plan/02-phase-2-semantic-epochs.md` §5, which re-pins the mask
    // once per epic rather than once per task. 256 is the ONE bit left that
    // is not free space: it is allocated (to `publish_tree`) but not yet
    // defined in the IDL, so today it is still out of range and
    // fatal. That is exactly why `realm_launch` took 512 rather than the next
    // unused-looking bit, and why `designate_file` and `egress` took 64 and
    // 128 from the same registry rather than the next ones after 512.
    assert_eq!(gen::vitrin_grant::Verb::VALID_MASK, 767);
    // The reserved bits are DERIVED from the mask rather than listed. They
    // were a hand-maintained literal through two allocations, and the list
    // shrank to one element when `designate_file` and `egress` merged -- at
    // which point a literal list is also a clippy `single_element_loop`. The
    // equality below is the tripwire that keeps this non-vacuous: it goes red
    // when a reserved bit is allocated, which is exactly when a human must
    // re-read the paragraph above.
    let reserved: Vec<u32> = (0..10)
        .map(|shift| 1u32 << shift)
        .filter(|bit| gen::vitrin_grant::Verb::VALID_MASK & bit == 0)
        .collect();
    assert_eq!(
        reserved,
        [256],
        "the set of allocated-but-undefined verb bits below 512 moved; \
         re-read the paragraph above before re-pinning it"
    );
    for bit in reserved {
        let err = gen::vitrin_grant::Verb::from_bits(bit).unwrap_err();
        assert_eq!(
            err,
            DecodeError::InvalidBitfieldValue {
                interface: "vitrin_grant",
                enum_name: "verb",
                value: bit,
            }
        );
    }
    // every subset of the defined bits, including all of them together, is
    // legal -- enumerated as (low eight bits) x (bit 512 present or not)
    // rather than a flat `0..=767` range, which would sweep through the
    // reserved bit above.
    for low in 0..=255u32 {
        for high in [0u32, 512] {
            gen::vitrin_grant::Verb::from_bits(low | high)
                .expect("every subset of defined bits is valid");
        }
    }

    // ... and through the generated decode: `resolved` carries `verbs` as its
    // second argument, after a valid `outcome`.
    let bytes = craft_frame(gen::vitrin_grant::events::Resolved::OPCODE, 0, |out| {
        vitrin_protocol::wire::write_uint(out, gen::vitrin_grant::Outcome::ALL[0].to_wire());
        // 256 rather than 64 or 128: it is the ONLY reserved bit left, its
        // owner (`publish_tree`, P2.4.1) is the furthest off, so this case
        // stays a decode failure for longest. 64 stopped being one when
        // `designate_file` landed and 128 when `egress` did, which is what
        // moved this line.
        vitrin_protocol::wire::write_uint(out, 256); // invalid verbs bit
        vitrin_protocol::wire::write_uint(out, gen::vitrin_grant::Persistence::ALL[0].to_wire());
        vitrin_protocol::wire::write_uint(out, 0); // expiry_ms
    });
    let err = gen::vitrin_grant::events::Resolved::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::InvalidBitfieldValue {
            interface: "vitrin_grant",
            enum_name: "verb",
            value: 256,
        }
    );
}

/// `vitrin_powerbox.designated` declares exactly one fd, so a frame whose
/// header disagrees dies fatal `fd_violation` -- P2.6.5's own case of the
/// invariant `00-conventions.md` §2.4 makes framing-level rather than
/// signature-level.
///
/// **This test and the one below it carry issue #189's acceptance criterion
/// 5, which asked for the case in `protocol/test-mutations.sh`.** That script
/// mutates the IDL and asserts the RELAX NG schema rejects it; `fd_count` is
/// a header byte the dialect cannot express, so no mutation there could prove
/// anything about it. The relocation is D-045 in
/// `docs/plan/20-decision-log.md`, including what it does NOT cover: these
/// are codec unit tests, not a hostile peer driven through a live `vitrind`.
///
/// Both directions are covered, because the cheap check (`fd.is_some()` vs
/// `HAS_FD`) passes one of them: a header claiming **one** fd for a message
/// that carries one, decoded with none supplied, and a header claiming
/// **none** for the same message while the fd really is supplied.
#[test]
fn designated_fd_count_mismatch_is_rejected_in_both_directions() {
    let (reader, writer) = std::io::pipe().unwrap();
    let value = gen::vitrin_powerbox::events::Designated {
        fd: std::os::fd::OwnedFd::from(reader),
        designation_id: 7,
        kind: gen::vitrin_powerbox::Kind::File,
        mode: gen::vitrin_powerbox::Mode::Read,
        name: "notes.txt".to_string(),
    };
    let mut bytes = value.encode(3);
    assert_eq!(bytes[7], 1, "fd_count byte for a one-fd message must be 1");

    // (a) the header is honest, but no fd accompanies the frame.
    let err = gen::vitrin_powerbox::events::Designated::decode(&bytes, None).unwrap_err();
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 1,
            actual: 0,
        }
    );
    assert_eq!(
        err.to_wire_error(),
        gen::vitrin_handshake::Error::FdViolation,
        "an fd_count mismatch is the fatal wire code fd_violation"
    );

    // (b) the header lies low: it declares 0 fds while the fd is really
    // there. `fd.is_some() == HAS_FD` alone would wave this through.
    bytes[7] = 0;
    let err =
        gen::vitrin_powerbox::events::Designated::decode(&bytes, Some(writer.into())).unwrap_err();
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 1,
            actual: 0,
        }
    );
    assert_eq!(
        err.to_wire_error(),
        gen::vitrin_handshake::Error::FdViolation
    );
}

/// The shim-side half of the same designation carries exactly one fd too,
/// and an unsolicited fd on a message that declares none is the other arm of
/// `fd_violation`. `request_dir` is the zero-argument, zero-fd request that
/// makes the arm testable on this epic's own surface.
#[test]
fn request_dir_rejects_an_unsolicited_fd() {
    let bytes = gen::vitrin_powerbox::requests::RequestDir {}.encode(3);
    assert_eq!(bytes[7], 0, "fd_count byte for a zero-fd message must be 0");
    let fd = std::io::pipe().unwrap().0;
    let err = gen::vitrin_powerbox::requests::RequestDir::decode(&bytes, Some(fd.into()))
        .expect_err("an unsolicited fd must be fatal");
    assert_eq!(
        err,
        DecodeError::FdCountMismatch {
            expected: 0,
            actual: 1,
        }
    );
    assert_eq!(
        err.to_wire_error(),
        gen::vitrin_handshake::Error::FdViolation
    );
}

#[test]
fn every_verb_is_classified_as_facet_bearing_or_not() {
    // A count that documents itself. `docs/protocol/04-vitrin_grant.md`
    // states the facet-bearing count twice, four hundred lines apart, and the
    // two sentences disagreed -- one said six and the other five -- until
    // issue #196's review caught it. Both are the same number, and neither
    // should be a sentence a human has to remember to update, so the
    // invariant lives here instead: `|facet| + |facetless|` partitions the
    // verb bitfield, and the size of each side is a tripwire below.
    //
    // **This comment used to quote those two sentences, and no longer does.**
    // It quoted "Six" and "six" while the page -- shipped by this same branch
    // -- said seven, and the page says eight since the powerbox facet merged
    // in. The first quotation had been accurate and went stale the
    // moment the egress facet moved the count. The second never matched
    // anything: the page's other site read "Every other verb does", so "the
    // other six do" was a paraphrase wearing quotation marks and was wrong
    // before the count ever moved. A quotation of a file this test does not
    // open is unholdable by construction -- nothing can go red when the
    // quoted text changes -- which is the same defect class the count itself
    // is here to close.
    //
    // The alternative -- have this test read the page and assert on its text
    // -- is deliberately NOT taken, for two reasons. Reading
    // `docs/protocol/` from a `vitrin-protocol` test would couple the wire
    // codec's negative-path suite to prose layout, so a reword that changed
    // no fact would go red. And that job is already done, by an instrument
    // built for it: `cargo xtask verb-sets --check` registers
    // `docs/protocol/04-vitrin_grant.md` as a `facet-verbs` carrier, reads
    // the marker beside that first sentence (`| count: eight`), derives the
    // set from the IDL, and fails with the path in it when the two disagree.
    // A second reader here would be a second definition of one check --
    // exactly what `crates/xtask/src/verb_sets.rs` exists to stop.
    //
    // So this test states the count, and the page's copy of it is held by
    // the tool that reads the page.
    //
    // **Nothing here is hand-listed any more, and that is the point of this
    // revision.** The first cut held `FACET_VERBS` as an array of six
    // per-interface `VERB` constants and the facetless pair as a literal.
    // Both halves were transcriptions: adding a *seventh* `interface/@verb`
    // to the IDL -- which is exactly what the egress facet did, in this same
    // issue's second half -- left this test green while the two doc sentences
    // it exists to hold became false. Issue #196's round-2 review levered that
    // by adding `verb="egress"` to `vitrin_consent` and watching the test
    // pass. The seventh then landed for real, and the derived form is what
    // made the two literals below go red so a human re-read the prose. An
    // EIGHTH landed at the same time on another branch -- `vitrin_powerbox`,
    // P2.6.5's `designate_file` facet -- and the derived form is why merging
    // the two cost one literal rather than a rewrite. Both
    // sides come out of the generator:
    //
    //   * `gen::FACET_VERBS` is emitted from `interface/@verb`, so a ninth
    //     facet moves its length;
    //   * `Verb::ENTRIES` is emitted from the verb bitfield's entries, so the
    //     facetless remainder is a set difference rather than a literal.
    let facet: Vec<&str> = gen::FACET_VERBS.to_vec();
    let facetless: Vec<&str> = gen::vitrin_grant::Verb::ENTRIES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !facet.contains(name))
        .collect();

    // Every `interface/@verb` names a verb the bitfield actually defines --
    // the direction the RELAX NG schema's closed `@verb` value set cannot
    // check, since it knows the names and not the bits.
    for verb in &facet {
        assert!(
            gen::vitrin_grant::Verb::ENTRIES
                .iter()
                .any(|(name, _)| name == verb),
            "`interface/@verb=\"{verb}\"` names no entry of `vitrin_grant.verb`"
        );
    }

    // The two literals below are TRIPWIRES, in this repo's established sense:
    // their whole job is to go red on the next verb or facet so that a human
    // re-reads the prose that states them. Neither is load-bearing for the
    // partition itself, which is derived above.
    assert_eq!(
        facet.len(),
        8,
        "the IDL no longer declares eight `interface/@verb` facets ({facet:?}); \
         docs/protocol/04-vitrin_grant.md states this count twice, and \
         `cargo xtask verb-sets --check` holds every surface that enumerates it"
    );
    // vitrin-verb-set: facetless-verbs = observe_cursor
    assert_eq!(
        facetless,
        ["observe_cursor"],
        "the facetless verb set moved. It is enumerated in prose on six \
         surfaces; `cargo xtask verb-sets --check` names them and fails on \
         each one that still lists the old set"
    );
    // A facet is not service. `egress` left the facetless set at P2.7.2's
    // second half and is still refused `unsupported` by every deployment,
    // because the proxy behind the facet does not exist. The unserved set is
    // pinned in `crates/vitrin-core/src/consent/render.rs`, not here: this
    // file is `vitrin-protocol`'s, and which verbs a deployment serves is not
    // a property of the wire.

    // ...and the partition really is a partition of the wire bitfield.
    assert_eq!(
        facet.len() + facetless.len(),
        gen::vitrin_grant::Verb::VALID_MASK.count_ones() as usize,
        "a verb bit is defined that is on neither side of the \
         facet-bearing/facetless line"
    );
    // Non-vacuity: the per-interface `VERB` constants and the generated
    // `FACET_VERBS` are two renderings of one attribute, and this test would
    // still pass if the generator emitted an empty slice for either.
    assert!(facet.contains(&gen::vitrin_view::VERB));
    assert!(facet.contains(&gen::vitrin_launcher::VERB));
    assert!(facet.contains(&gen::vitrin_powerbox::VERB));
    assert!(facet.contains(&gen::vitrin_egress::VERB));
    assert!(!facetless.is_empty());
}

#[test]
fn invalid_utf8_is_rejected_through_a_generated_message() {
    // get_realm's `name`: a 2-byte string that is not valid UTF-8.
    let bytes = craft_frame(
        gen::vitrin_principal::requests::GetRealm::OPCODE,
        0,
        |out| {
            vitrin_protocol::wire::write_uint(out, 2); // realm new_id
            vitrin_protocol::wire::write_uint(out, 2); // string length
            out.extend_from_slice(&[0xff, 0xfe, 0, 0]); // bad UTF-8 + padding
        },
    );
    let err = gen::vitrin_principal::requests::GetRealm::decode(&bytes, None).unwrap_err();
    assert_eq!(err, DecodeError::InvalidUtf8);
}

#[test]
fn embedded_nul_is_rejected_through_a_generated_message() {
    let bytes = craft_frame(
        gen::vitrin_principal::requests::GetRealm::OPCODE,
        0,
        |out| {
            vitrin_protocol::wire::write_uint(out, 2); // realm new_id
            vitrin_protocol::wire::write_uint(out, 3); // string length
            out.extend_from_slice(b"a\0b\0"); // embedded NUL + 1 padding byte
        },
    );
    let err = gen::vitrin_principal::requests::GetRealm::decode(&bytes, None).unwrap_err();
    assert_eq!(err, DecodeError::EmbeddedNul);
}

#[test]
fn malformed_padding_is_rejected_through_a_generated_message() {
    // A 1-byte string needs 3 padding bytes; conventions 2.2 makes a nonzero
    // one fatal invalid_argument, and accepting it would break the canonical
    // one-value-one-encoding property.
    let bytes = craft_frame(
        gen::vitrin_principal::requests::GetRealm::OPCODE,
        0,
        |out| {
            vitrin_protocol::wire::write_uint(out, 2); // realm new_id
            vitrin_protocol::wire::write_uint(out, 1); // string length
            out.extend_from_slice(&[b'a', 0xff, 0, 0]); // nonzero padding byte
        },
    );
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
    assert_eq!(
        DecodeError::NullObject.to_wire_error(),
        WireError::InvalidObject
    );
}
