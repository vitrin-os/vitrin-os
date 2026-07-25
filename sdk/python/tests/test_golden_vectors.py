# SPDX-License-Identifier: Apache-2.0
"""The shared golden-bytes corpus, asserted against the Python codec.

These frames are byte-for-byte identical to
crates/vitrin-protocol/tests/golden.rs and shim/tests/test_golden_frames.c:
every implementation must match the *written-down* bytes, never another
implementation's encoder output, so a symmetric encode/decode bug (mirrored
endianness, reordered fields, renumbered opcodes) cannot hide.
"""

from __future__ import annotations

import vectors
from vitrin_os import messages
from vitrin_os.messages import FrameReadyEvent, decode_event
from vitrin_os.wire import HEADER_SIZE, MessageDecoder, fixed_to_float, unpack_header


def test_vector_builder_matches_corpus() -> None:
    """The test-side struct-level builder reproduces the corpus bytes."""
    assert vectors.frame(1, 1, vectors.u32(42)) == vectors.GOLDEN_SYNC
    assert (
        vectors.frame(7, 0, vectors.u32(2) + vectors.string("abc"))
        == vectors.GOLDEN_GET_REALM
    )


def test_golden_sync_uint() -> None:
    assert messages.encode_sync(42) == vectors.GOLDEN_SYNC


def test_golden_get_realm_new_id_and_string_padding() -> None:
    assert (
        messages.encode_get_realm(7, realm_id=2, name="abc")
        == vectors.GOLDEN_GET_REALM
    )


def test_golden_pointer_move_negative_int() -> None:
    # -1 as two's-complement little-endian i32 pins signedness + endianness.
    assert messages.encode_move(3, x=-1, y=2) == vectors.GOLDEN_MOVE


def test_golden_seat_motion_fixed_point() -> None:
    # The shim class is not spoken by this SDK; the vector pins the generic
    # header layout and the signed 24.8 fixed-point interpretation.
    object_id, size, opcode, fd_count = unpack_header(vectors.GOLDEN_SEAT_MOTION)
    assert (object_id, size, opcode, fd_count) == (9, 20, 0, 0)
    dec = MessageDecoder(vectors.GOLDEN_SEAT_MOTION[HEADER_SIZE:])
    assert fixed_to_float(dec.fixed_bits()) == 1.5
    assert fixed_to_float(dec.fixed_bits()) == -1.0
    assert dec.uint() == 0  # origin physical
    dec.finish()


def test_golden_frame_ready_fd_header_and_fourcc_enum() -> None:
    # fd_count=1 in the header, fd bytes NEVER in the body; format is the
    # 'XR24' DRM fourcc (0x34325258) little-endian.
    object_id, size, opcode, fd_count = unpack_header(vectors.GOLDEN_FRAME_READY)
    assert (object_id, size, opcode, fd_count) == (5, 28, 0, 1)
    event = decode_event(
        "vitrin_view", opcode, vectors.GOLDEN_FRAME_READY[HEADER_SIZE:], fd=99
    )
    assert isinstance(event, FrameReadyEvent)
    assert event.fd == 99
    assert event.format == 0x34325258
    assert (event.width, event.height, event.stride) == (1, 2, 4)
    assert event.flags == 0
