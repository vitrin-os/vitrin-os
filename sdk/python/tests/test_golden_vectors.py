# SPDX-License-Identifier: Apache-2.0
"""The shared golden-bytes corpus, asserted against the Python codec.

These frames are byte-for-byte identical to
crates/vitrin-protocol/tests/golden.rs and shim/tests/test_golden_frames.c:
every implementation must match the *written-down* bytes, never another
implementation's encoder output, so a symmetric encode/decode bug (mirrored
endianness, reordered fields, renumbered opcodes) cannot hide.
"""

from __future__ import annotations

import pytest

import vectors
from vitrin_os import messages, protocol
from vitrin_os.messages import (
    AttentionEvent,
    FrameReadyEvent,
    LaunchedEvent,
    decode_event,
)
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


def test_golden_attention_is_a_bare_header_with_no_payload() -> None:
    # `vitrin_principal.attention` carries no arguments, forever, so the whole
    # frame is the 8-byte header. Decoding it through the SDK's own event table
    # is what proves the opcode is right: opcode 0 would have been `bound` and
    # would have raised on a missing string.
    object_id, size, opcode, fd_count = unpack_header(vectors.GOLDEN_ATTENTION)
    assert (object_id, size, opcode, fd_count) == (2, 8, 1, 0)
    event = decode_event(
        "vitrin_principal", opcode, vectors.GOLDEN_ATTENTION[HEADER_SIZE:], fd=None
    )
    assert isinstance(event, AttentionEvent)


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


# ---------------------------------------------------------------------------
# The version-2 layout and launcher facets (WS-E.1.5, #211).
#
# Seven messages, and between them every shape the three facets have: two
# structural mints that differ only in their opcode, a third that differs
# again, two argument-free requests that differ only in their object id, an
# enum-carrying request, and the one reply-bearing event. Encoders are
# asserted against the written-down bytes; the event is decoded through the
# SDK's own interface-keyed table, because a vector that only round-tripped
# through the encoder would pass with a reordered opcode table.
# ---------------------------------------------------------------------------


def test_golden_get_launcher_is_request_zero_on_the_grant() -> None:
    assert (
        messages.encode_get_launcher(4, facet_id=11) == vectors.GOLDEN_GET_LAUNCHER
    )


def test_golden_get_layout_focus_is_request_one_on_the_grant() -> None:
    assert (
        messages.encode_get_layout_focus(4, facet_id=12)
        == vectors.GOLDEN_GET_LAYOUT_FOCUS
    )


def test_golden_get_layout_arrange_is_request_two_on_the_grant() -> None:
    assert (
        messages.encode_get_layout_arrange(4, facet_id=13)
        == vectors.GOLDEN_GET_LAYOUT_ARRANGE
    )


def test_the_three_structural_mints_differ_only_in_opcode_and_minted_id() -> None:
    """The mints are one byte apart, so their ORDER is the whole contract.

    Stated as an assertion rather than a comment: if the IDL's request order
    ever changed, each frame would stay individually well-formed and the SDK
    would silently mint the wrong facet from the right grant — the one drift
    a per-message equality check would still pass, since each encoder would
    have been "fixed" to match its own new vector.
    """
    opcodes = [
        unpack_header(v)[2]
        for v in (
            vectors.GOLDEN_GET_LAUNCHER,
            vectors.GOLDEN_GET_LAYOUT_FOCUS,
            vectors.GOLDEN_GET_LAYOUT_ARRANGE,
        )
    ]
    assert opcodes == [0, 1, 2]


def test_golden_focus_and_launch_are_bare_headers_told_apart_by_object() -> None:
    """Both are argument-free, so only the object id distinguishes them.

    `launch` starts a process and `focus` moves the output; on the wire the
    two frames are identical but for four bytes. Pinning that here is what
    keeps "the object is the capability" a checked property of this codec
    rather than a sentence in a document.
    """
    assert messages.encode_launch(11) == vectors.GOLDEN_LAUNCH
    assert messages.encode_focus(12) == vectors.GOLDEN_FOCUS
    assert unpack_header(vectors.GOLDEN_LAUNCH) == (11, 8, 0, 0)
    assert unpack_header(vectors.GOLDEN_FOCUS) == (12, 8, 0, 0)
    assert vectors.GOLDEN_LAUNCH[4:] == vectors.GOLDEN_FOCUS[4:]


def test_golden_set_fullscreen_carries_the_mode_enum() -> None:
    assert (
        messages.encode_set_fullscreen(13, mode=protocol.LayoutMode.FULLSCREEN)
        == vectors.GOLDEN_SET_FULLSCREEN
    )
    # ...and windowed is the same frame with a zero, which is the whole of the
    # enum. An out-of-range value is fatal `invalid_argument` server-side, so
    # the encoder refuses it locally rather than killing the connection.
    windowed = bytearray(vectors.GOLDEN_SET_FULLSCREEN)
    windowed[8] = 0
    assert (
        messages.encode_set_fullscreen(13, mode=protocol.LayoutMode.WINDOWED)
        == bytes(windowed)
    )
    with pytest.raises(ValueError):
        messages.encode_set_fullscreen(13, mode=2)


def test_golden_launched_decodes_the_core_minted_realm_id() -> None:
    """The one reply-bearing message of the three facets.

    Decoded through `decode_event`'s interface-keyed table rather than by a
    hand-rolled read, so this fails if `launched` is not event 0 on
    `vitrin_launcher` — the same property `test_golden_attention_...` pins
    for `vitrin_principal`.
    """
    object_id, size, opcode, fd_count = unpack_header(vectors.GOLDEN_LAUNCHED)
    assert (object_id, size, opcode, fd_count) == (11, 20, 0, 0)
    event = decode_event(
        "vitrin_launcher", opcode, vectors.GOLDEN_LAUNCHED[HEADER_SIZE:], fd=None
    )
    assert isinstance(event, LaunchedEvent)
    # The id is opaque and server-minted: this SDK must carry it through
    # unparsed. `kiosk.1` has the `<template>.<n>` shape the core mints, and
    # nothing here may depend on that shape.
    assert event.realm == "kiosk.1"
