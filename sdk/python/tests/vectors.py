# SPDX-License-Identifier: Apache-2.0
"""Protocol test vectors, independent of the SDK's own codec.

Three layers:

1. GOLDEN_* under "the shared golden corpus" — the cross-language
   golden-bytes corpus, byte for byte identical to
   crates/vitrin-protocol/tests/golden.rs and
   shim/tests/test_golden_frames.c. Each implementation must match these
   written-down bytes, never another implementation's encoder output, so a
   symmetric codec bug cannot hide. When editing one copy, update all
   three.

2. GOLDEN_* under "the version-2 layout and launcher corpus" — written down
   the same way, from the IDL, and asserted the same way, but **Python-side
   only**: the shim speaks the shim class and never these four principal
   interfaces, and the Rust side's coverage of them is the decoder table in
   fuzz/ plus the mock-free gates. Stated rather than left for a reader to
   infer from which file a constant appears in, because "shared corpus" is a
   claim about other implementations and it would be false of these.

3. Frame builders — a minimal struct-level encoder used by the mock-server
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

# vitrin_principal.attention{} on object 2 — a bare 8-byte header and no
# payload at all. `attention` (WS-E.1.7) carries no arguments forever, and it
# is event opcode 1, appended after `bound`: the vector pins the emptiness and
# the opcode together, because a reorder would decode as a truncated `bound`.
GOLDEN_ATTENTION = bytes([2, 0, 0, 0, 8, 0, 1, 0])

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


# ---------------------------------------------------------------------------
# The version-2 layout and launcher corpus (Python-side; see the module
# docstring's layer 2 for why this is not claimed to be shared).
#
# Every byte below is read off protocol/vitrin-v0.xml -- request ordinals in
# declaration order, argument order as declared -- never copied out of
# vitrin_os.messages. That is the whole point: the SDK is a second
# independent implementation (decision D8), and a vector transcribed from the
# transcription it is supposed to check would agree with it by construction.
#
# The object ids are chosen so the four structural mints THIS SDK ENCODES and
# their uses form one coherent trace: grant 4 mints launcher 11,
# layout_focus 12, layout_arrange 13 and powerbox 14, and the use-vectors are
# addressed to those ids. vitrin_grant carries one further mint --
# `get_egress` (opcode 4, P2.7.2) -- with no vector here, because messages.py
# has no codec for it to check; see that module's docstring for why.
# ---------------------------------------------------------------------------

# vitrin_grant.get_launcher{launcher: 11} on grant object 4 -- request 0.
GOLDEN_GET_LAUNCHER = bytes([4, 0, 0, 0, 12, 0, 0, 0, 11, 0, 0, 0])

# vitrin_grant.get_layout_focus{layout_focus: 12} on grant object 4 -- request
# 1. The mints differ ONLY in the opcode byte and the minted id, which is why
# each one here is pinned: a reordering of the requests in the IDL would leave
# each frame individually well-formed and silently mint the wrong facet.
#
# FOUR OF FIVE. `get_powerbox` (request 3) joined them when the SDK gained a
# powerbox client, which is what the previous note here said would have to
# happen first: these vectors are asserted against an SDK encoder, and a vector
# with no encoder to compare against would pin the transcription rather than
# the implementation. `get_egress` (request 4) is the one still outside the
# reordering guard, for exactly that reason -- no `encode_get_egress` exists,
# because nothing serves `egress`.
GOLDEN_GET_LAYOUT_FOCUS = bytes([4, 0, 0, 0, 12, 0, 1, 0, 12, 0, 0, 0])

# vitrin_grant.get_layout_arrange{layout_arrange: 13} on grant object 4 --
# request 2.
GOLDEN_GET_LAYOUT_ARRANGE = bytes([4, 0, 0, 0, 12, 0, 2, 0, 13, 0, 0, 0])

# vitrin_launcher.launch{} on object 11 -- request 0, no arguments, so the
# whole frame is the 8-byte header. `launch` takes no arguments and cannot:
# which program runs is fixed by the realm template the grant addresses, in
# front of the human, never by an argument here.
GOLDEN_LAUNCH = bytes([11, 0, 0, 0, 8, 0, 0, 0])

# vitrin_layout_focus.focus{} on object 12 -- request 0, also argument-free.
# Byte-identical to GOLDEN_LAUNCH but for the object id, and that is the
# property worth pinning: the discriminator between "start a process" and
# "move the output" is the OBJECT, not the opcode. Both are compared as BYTES
# and header-unpacked, and that is all -- they are client-to-server requests
# and the SDK has no request decoder (`decode_event` covers events only), so
# there is no interface-keyed table for them to go through. `GOLDEN_LAUNCHED`
# is the one vector here that really is decoded.
GOLDEN_FOCUS = bytes([12, 0, 0, 0, 8, 0, 0, 0])

# vitrin_layout_arrange.set_fullscreen{mode: fullscreen} on object 13 --
# request 0. mode is the vitrin_layout_arrange.mode enum: windowed = 0,
# fullscreen = 1.
GOLDEN_SET_FULLSCREEN = bytes([13, 0, 0, 0, 12, 0, 0, 0, 1, 0, 0, 0])

# vitrin_launcher.launched{realm: "kiosk.1"} on object 11 -- event 0. The one
# reply-bearing message of the three facets: 7 bytes of UTF-8, so a 4-byte
# length, the bytes, and one zero padding byte to the next 4-byte boundary.
GOLDEN_LAUNCHED = bytes(
    [11, 0, 0, 0, 20, 0, 0, 0]
    + [7, 0, 0, 0]  # string length in BYTES, not code points
    + [0x6B, 0x69, 0x6F, 0x73, 0x6B, 0x2E, 0x31]  # "kiosk.1"
    + [0]  # padding to the 4-byte boundary; never counted in the length
)


# ---------------------------------------------------------------------------
# The version-2 powerbox corpus (P2.6.6 / #190), on the same terms as the
# section above: written down from protocol/vitrin-v0.xml, never copied out of
# vitrin_os.messages, and Python-side only -- the shim speaks the shim class
# and `vitrin_shim_session.designation` is a DIFFERENT message from
# `vitrin_powerbox.designated`, so there is no second copy of these bytes to
# keep in step.
#
# The four ask/answer vectors are addressed to powerbox object 14, which grant
# 4 mints just below.
# ---------------------------------------------------------------------------

# vitrin_grant.get_powerbox{powerbox: 14} on grant object 4 -- request 3.
GOLDEN_GET_POWERBOX = bytes([4, 0, 0, 0, 12, 0, 3, 0, 14, 0, 0, 0])

# vitrin_powerbox.request_file{mode: read_write} on object 14 -- request 0.
# mode is the vitrin_powerbox.mode enum: read = 0, read_write = 1. The frame
# carries the mode and NOTHING else -- no filename, no filter, no starting
# directory -- and that absence is the interface rather than an omission from
# this vector: an argument naming a file would defeat a powerbox, and a
# signature is immutable forever, so this frame can never grow one.
GOLDEN_REQUEST_FILE = bytes([14, 0, 0, 0, 12, 0, 0, 0, 1, 0, 0, 0])

# vitrin_powerbox.request_dir{} on object 14 -- request 1, no arguments, so
# the whole frame is the 8-byte header. Byte-identical to GOLDEN_REQUEST_FILE
# in every field but the opcode and the vanished payload: `request_dir` takes
# no mode because a subtree picker has one chrome.
GOLDEN_REQUEST_DIR = bytes([14, 0, 0, 0, 8, 0, 1, 0])

# vitrin_powerbox.designated{designation_id: 7, kind: file, mode: read_write,
# name: "notes.txt"} on object 14 -- event 0, and the header declares ONE fd.
# The descriptor rides SCM_RIGHTS: its bytes are never in the body, exactly as
# in GOLDEN_FRAME_READY, and the fd_count byte is the whole of the pairing.
#
# `kind` and `mode` are adjacent uints and are given DIFFERENT values on
# purpose: they are two enums in a row, so a decoder that read them in the
# wrong order would be invisible to a vector where both happened to be zero.
# (That the answer's mode may be NARROWER than the ask is a property of the
# facet rather than of this frame, and is asserted end-to-end in
# test_powerbox.py, where a `write=True` ask is answered `read`.)
GOLDEN_DESIGNATED = bytes(
    [14, 0, 0, 0, 36, 0, 0, 1]
    + [7, 0, 0, 0]  # designation_id -- opaque, and the realm's copy carries it
    + [0, 0, 0, 0]  # kind: file
    + [1, 0, 0, 0]  # mode: read_write -- the EFFECTIVE access the human gave
    + [9, 0, 0, 0]  # string length in BYTES
    + [0x6E, 0x6F, 0x74, 0x65, 0x73, 0x2E, 0x74, 0x78, 0x74]  # "notes.txt"
    + [0, 0, 0]  # padding to the 4-byte boundary; never counted in the length
)

# vitrin_powerbox.refused{code: busy} on object 14 -- event 1. `busy` (2)
# rather than `cancelled` (0) so the payload is not all zeros: a decoder that
# ignored the body entirely would still match a zero-valued code.
GOLDEN_POWERBOX_REFUSED = bytes([14, 0, 0, 0, 12, 0, 1, 0, 2, 0, 0, 0])
