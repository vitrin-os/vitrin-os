//! Golden-bytes (known-answer) tests: hardcoded wire frames asserted
//! byte-for-byte against `encode`, then decoded back and compared field-wise.
//!
//! These exist because the round-trip property (`encode(decode(bytes)) ==
//! bytes`) is *self-referential*: any bug that is symmetric across encode and
//! decode -- big-endian on both sides, a mirrored field reorder, renumbered
//! opcodes, a mirrored padding-rule change -- passes it with flying colors.
//! A handful of frames whose exact bytes are written down independently pins
//! the actual wire format, not the codec's agreement with itself.
//!
//! The same five frames, byte for byte, are asserted from the C side in
//! `shim/tests/test_golden_frames.c` -- so Rust and C cannot silently
//! disagree on the wire layout either: both must match *these* bytes, not
//! each other.
//!
//! Coverage: every argument type that appears in v0 (`uint`, `new_id`,
//! `string` + padding, `int`, `fixed`, enum-by-fourcc, bitfield, and the
//! fd-bearing header shape). Plain `object` args do not exist in v0.

use vitrin_protocol::generated as gen;
use vitrin_protocol::Fixed;

#[test]
fn golden_sync_uint() {
    // header: object_id=1 | size=12 | opcode=1 | fd_count=0, then cookie=42.
    let frame = gen::vitrin_handshake::requests::Sync { cookie: 42 }.encode(1);
    assert_eq!(frame, [1, 0, 0, 0, 12, 0, 1, 0, 42, 0, 0, 0]);

    let (object_id, decoded) = gen::vitrin_handshake::requests::Sync::decode(&frame, None).unwrap();
    assert_eq!(object_id, 1);
    assert_eq!(decoded.cookie, 42);
}

#[test]
fn golden_get_realm_new_id_and_string_padding() {
    // header: object_id=7 | size=20 | opcode=0 | fd_count=0,
    // realm new_id=2, then "abc": length 3, bytes, 1 zero padding byte.
    let frame = gen::vitrin_principal::requests::GetRealm {
        realm: 2,
        name: "abc".to_string(),
    }
    .encode(7);
    assert_eq!(
        frame,
        [7, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, b'a', b'b', b'c', 0]
    );

    let (object_id, decoded) =
        gen::vitrin_principal::requests::GetRealm::decode(&frame, None).unwrap();
    assert_eq!(object_id, 7);
    assert_eq!(decoded.realm, 2);
    assert_eq!(decoded.name, "abc");
}

#[test]
fn golden_pointer_move_negative_int() {
    // -1 as two's-complement little-endian i32 pins signedness + endianness.
    let frame = gen::vitrin_actuator_pointer::requests::Move { x: -1, y: 2 }.encode(3);
    assert_eq!(
        frame,
        [3, 0, 0, 0, 16, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 2, 0, 0, 0]
    );

    let (object_id, decoded) =
        gen::vitrin_actuator_pointer::requests::Move::decode(&frame, None).unwrap();
    assert_eq!(object_id, 3);
    assert_eq!((decoded.x, decoded.y), (-1, 2));
}

#[test]
fn golden_seat_motion_fixed_point() {
    // 1.5 in 24.8 fixed-point is 384 (0x180); -1.0 is -256 (0xffffff00 LE).
    // origin physical = 0.
    let frame = gen::vitrin_shim_seat::events::Motion {
        x: Fixed::from_f64(1.5),
        y: Fixed::from_bits(-256),
        origin: gen::vitrin_shim_seat::Origin::Physical,
    }
    .encode(9);
    assert_eq!(
        frame,
        [9, 0, 0, 0, 20, 0, 0, 0, 0x80, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0, 0, 0, 0]
    );

    let (object_id, decoded) = gen::vitrin_shim_seat::events::Motion::decode(&frame, None).unwrap();
    assert_eq!(object_id, 9);
    assert_eq!(decoded.x.to_bits(), 384);
    assert_eq!(decoded.y.to_bits(), -256);
    assert_eq!(decoded.origin, gen::vitrin_shim_seat::Origin::Physical);
}

#[test]
fn golden_frame_ready_fd_header_and_fourcc_enum() {
    // fd_count=1 in the header, fd bytes NEVER in the body; format is the
    // XR24 DRM fourcc (0x34325258) little-endian.
    let (reader, writer) = std::io::pipe().unwrap();
    let frame = gen::vitrin_view::events::FrameReady {
        fd: reader.into(),
        format: gen::vitrin_view::Format::Xrgb8888,
        width: 1,
        height: 2,
        stride: 4,
        flags: gen::vitrin_view::FrameFlags::default(),
    }
    .encode(5);
    assert_eq!(
        frame,
        [
            5, 0, 0, 0, 28, 0, 0, 1, // header, fd_count = 1
            0x58, 0x52, 0x32, 0x34, // 'XR24' fourcc, little-endian
            1, 0, 0, 0, // width
            2, 0, 0, 0, // height
            4, 0, 0, 0, // stride
            0, 0, 0, 0, // flags (none)
        ]
    );

    let (object_id, decoded) =
        gen::vitrin_view::events::FrameReady::decode(&frame, Some(writer.into())).unwrap();
    assert_eq!(object_id, 5);
    assert_eq!(decoded.format, gen::vitrin_view::Format::Xrgb8888);
    assert_eq!((decoded.width, decoded.height, decoded.stride), (1, 2, 4));
    assert_eq!(decoded.flags, gen::vitrin_view::FrameFlags::default());
}
