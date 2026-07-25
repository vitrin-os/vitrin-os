# SPDX-License-Identifier: Apache-2.0
"""Protocol test vectors, independent of the SDK's own codec.

Two layers:

1. GOLDEN_* — the shared cross-language golden-bytes corpus, byte for byte
   identical to crates/vitrin-protocol/tests/golden.rs and
   shim/tests/test_golden_frames.c. Each implementation must match these
   written-down bytes, never another implementation's encoder output, so a
   symmetric codec bug cannot hide. When editing one copy, update all
   three.

2. Frame builders — a minimal struct-level encoder used by the mock-server
   scripts. Deliberately implemented here with raw ``struct.pack`` (not
   with vitrin_os.wire) so the mocked server's bytes are an independent
   restatement of the wire format.
"""

from __future__ import annotations

import struct


def frame(object_id: int, opcode: int, payload: bytes = b"", *, fd_count: int = 0) -> bytes:
    """Build one wire frame: 8-byte header + payload, size = whole frame."""
    return struct.pack("<IHBB", object_id, 8 + len(payload), opcode, fd_count) + payload


def u32(value: int) -> bytes:
    return struct.pack("<I", value)


def i32(value: int) -> bytes:
    return struct.pack("<i", value)


def string(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return u32(len(encoded)) + encoded + b"\x00" * ((-len(encoded)) % 4)


# ---------------------------------------------------------------------------
# The shared golden corpus (see golden.rs / test_golden_frames.c).
# ---------------------------------------------------------------------------

# vitrin_handshake.sync{cookie: 42} on object 1.
GOLDEN_SYNC = bytes([1, 0, 0, 0, 12, 0, 1, 0, 42, 0, 0, 0])

# vitrin_principal.get_realm{realm: 2, name: "abc"} on object 7
# (string: length 3, bytes, one zero padding byte).
GOLDEN_GET_REALM = bytes(
    [7, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, ord("a"), ord("b"), ord("c"), 0]
)

# vitrin_actuator_pointer.move{x: -1, y: 2} on object 3
# (-1 as two's-complement little-endian i32 pins signedness + endianness).
GOLDEN_MOVE = bytes([3, 0, 0, 0, 16, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 2, 0, 0, 0])

# vitrin_shim_seat.motion{x: 1.5, y: -1.0, origin: physical} on object 9
# (1.5 in 24.8 fixed point is 384 = 0x180; -1.0 is -256 = 0xffffff00 LE).
# The shim class is not spoken by this SDK; the vector pins the generic
# frame codec and the 24.8 fixed-point interpretation.
GOLDEN_SEAT_MOTION = bytes(
    [9, 0, 0, 0, 20, 0, 0, 0, 0x80, 1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]
)

# vitrin_view.frame_ready{format: xrgb8888, width: 1, height: 2, stride: 4,
# flags: 0} on object 5 — fd_count = 1 in the header, fd bytes NEVER in the
# body; format is the 'XR24' DRM fourcc (0x34325258) little-endian.
GOLDEN_FRAME_READY = bytes(
    [5, 0, 0, 0, 28, 0, 0, 1]
    + [0x58, 0x52, 0x32, 0x34]  # 'XR24' fourcc, little-endian
    + [1, 0, 0, 0]  # width
    + [2, 0, 0, 0]  # height
    + [4, 0, 0, 0]  # stride
    + [0, 0, 0, 0]  # flags (none)
)
