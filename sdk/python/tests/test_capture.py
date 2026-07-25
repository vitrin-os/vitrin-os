# SPDX-License-Identifier: Apache-2.0
"""Capture ergonomics (P1.8.2): Frame.raw / to_png against the cross-pinned golden.

The golden file
---------------

``golden/test_pattern_64x40.xrgb`` is the raw xrgb8888 wire conversion of
the core's synthetic test pattern at 64x40 —
``rgba_to_xrgb8888(test_pattern::render(64, 40))``. It is pinned on the
Rust side by the ``crates/vitrin-core/src/capture.rs`` test
``sdk_capture_golden_file_pins_the_wire_bytes`` (Rust CI recomputes the
expectation from the generator and asserts the committed file matches) and
consumed here: the mock core serves those bytes as a real sealed memfd, and
``observe()`` must surface them verbatim through ``.raw`` and decodably
through ``.to_png()``. The two implementations can therefore only drift
loudly, and no image codec ever enters the core (plan risk R7).
Regenerate — deliberately only — with::

    VITRIN_REGEN_GOLDEN=1 cargo test -p vitrin-core sdk_capture_golden

Pillow's role
-------------

Pillow is a *test-only* independent decoder, never an SDK dependency: the
stdlib mini-decoder below verifies the exact container shape our encoder
emits (filter-0 scanlines, one zlib stream, CRC-checked chunks), and the
Pillow test — skipped when Pillow is absent — proves a real decoder agrees,
so an encoder/decoder-mirrored bug cannot hide. ``to_png()`` itself behaves
identically with and without Pillow installed (it never imports it).
"""

from __future__ import annotations

import os
import struct
import zlib
from pathlib import Path

import pytest

import flows
from mock_server import make_sealed_memfd
from vitrin_os import Format, ServerContractViolation, connect
from vitrin_os.client import Frame
from vitrin_os.messages import encode_capture_frame, encode_sync
from vitrin_os.png import encode_png, xrgb8888_to_rgb

GOLDEN_PATH = Path(__file__).parent / "golden" / "test_pattern_64x40.xrgb"
GOLDEN = GOLDEN_PATH.read_bytes()
W, H = 64, 40
STRIDE = W * 4  # the version-1 wire pin: stride == width * 4 exactly

# Per-row padding for the stride != width*4 cases (0xEE so a padding byte
# leaking into pixels is visible in a hex diff).
PAD = 16


def _padded_golden() -> bytes:
    """The golden rows re-laid with PAD trailing bytes per row."""
    return b"".join(
        GOLDEN[r * STRIDE : (r + 1) * STRIDE] + b"\xee" * PAD for r in range(H)
    )


def _rgb_of(xrgb: bytes) -> bytes:
    """Independent restatement of the BGRX→RGB swizzle (never the SDK's own
    converter), so the golden checks cannot inherit an SDK swizzle bug."""
    out = bytearray()
    for i in range(0, len(xrgb), 4):
        out += bytes((xrgb[i + 2], xrgb[i + 1], xrgb[i]))
    return bytes(out)


def _decode_png_rgb(data: bytes) -> tuple[int, int, bytes]:
    """Decode the exact PNG shape ``vitrin_os.png`` emits (8-bit truecolor,
    filter-0 scanlines, single deflate stream), CRC-checking every chunk.

    Deliberately a decoder for *our encoder's declared output shape*, not a
    general PNG reader: an unexpected filter byte or color type is a test
    failure, not something to handle. This keeps the no-Pillow leg able to
    prove full round-trips; Pillow (below) is the independent general
    decoder that keeps this mini-decoder honest.
    """
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "PNG signature"
    pos = 8
    width = height = None
    idat = b""
    seen_end = False
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        ctype = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        (crc,) = struct.unpack(">I", data[pos + 8 + length : pos + 12 + length])
        assert crc == zlib.crc32(ctype + body), f"CRC of {ctype!r}"
        if ctype == b"IHDR":
            width, height, depth, color, comp, filt, interlace = struct.unpack(
                ">IIBBBBB", body
            )
            assert (depth, color, comp, filt, interlace) == (8, 2, 0, 0, 0)
        elif ctype == b"IDAT":
            idat += body
        elif ctype == b"IEND":
            assert body == b""
            seen_end = True
        pos += 12 + length
    assert seen_end and width and height
    scanlines = zlib.decompress(idat)
    row = width * 3 + 1
    assert len(scanlines) == row * height
    rows = []
    for r in range(height):
        line = scanlines[r * row : (r + 1) * row]
        assert line[0] == 0, "our encoder always writes filter 0"
        rows.append(line[1:])
    return width, height, b"".join(rows)


def _open_fd_count() -> int:
    """Open fds in this process via /proc/self/fd. The listing includes the
    directory fd itself, identically in every call, so equality holds."""
    return len(os.listdir("/proc/self/fd"))


def _connect(server):
    return connect(
        server.path,
        identity=flows.IDENTITY,
        credential_type=flows.CREDENTIAL_TYPE,
        credential=flows.CREDENTIAL,
        timeout=5.0,
    )


def _observe_steps(content: bytes, *, width: int, height: int, stride: int) -> list[tuple]:
    """One capture answered with a fresh sealed memfd holding ``content``."""
    return [
        ("expect", encode_capture_frame(flows.VIEW_ID)),
        (
            "send_memfd",
            flows.frame_ready_frame(width=width, height=height, stride=stride),
            content,
        ),
    ]


# --- the golden file itself -------------------------------------------------


def test_golden_file_shape_and_orientation() -> None:
    """Tripwire on the committed file: exact size, X channel 0xFF throughout
    (the IDL pins the padding byte), and the four *distinct* corner markers
    in wire byte order (B,G,R,X) — red TL, green TR, blue BL, white BR — so
    a mangled checkout or a row-order regeneration bug fails here with a
    readable message rather than as a 10-KiB pixel diff."""
    assert len(GOLDEN) == W * H * 4
    assert all(GOLDEN[i] == 0xFF for i in range(3, len(GOLDEN), 4))

    def px(x: int, y: int) -> bytes:
        return GOLDEN[(y * W + x) * 4 : (y * W + x) * 4 + 4]

    assert px(0, 0) == b"\x00\x00\xff\xff"  # red, as B,G,R,X
    assert px(W - 1, 0) == b"\x00\xff\x00\xff"  # green
    assert px(0, H - 1) == b"\xff\x00\x00\xff"  # blue
    assert px(W - 1, H - 1) == b"\xff\xff\xff\xff"  # white


# --- observe() -> .raw ------------------------------------------------------


def test_observe_raw_is_the_golden_wire_buffer(server) -> None:
    """M1.1: ``.raw`` is the memfd bytes verbatim (the digest domain), with
    width/height/stride/format/size correct for a non-trivial frame."""
    server.run(
        [*flows.granted_steps(), *_observe_steps(GOLDEN, width=W, height=H, stride=STRIDE)]
    )
    conn = _connect(server)
    frame = conn.request_grant().await_consent().observe()
    assert (frame.width, frame.height, frame.stride) == (W, H, STRIDE)
    assert frame.format == Format.XRGB8888
    assert frame.size == len(frame.raw) == STRIDE * H
    assert frame.raw == GOLDEN
    conn.close()


# --- observe() -> .to_png() -------------------------------------------------


def test_to_png_round_trips_the_golden_without_pillow(server, tmp_path) -> None:
    """The M1.1 gate, dependency-free: observe() → to_png() → decoded
    pixels equal the committed pattern. Runs on a Pillow-less install —
    to_png's no-Pillow behavior IS its only behavior (pure stdlib)."""
    server.run(
        [*flows.granted_steps(), *_observe_steps(GOLDEN, width=W, height=H, stride=STRIDE)]
    )
    conn = _connect(server)
    frame = conn.request_grant().await_consent().observe()
    path = tmp_path / "frame.png"
    frame.to_png(path)
    width, height, rgb = _decode_png_rgb(path.read_bytes())
    assert (width, height) == (W, H)
    assert rgb == _rgb_of(GOLDEN)
    conn.close()


def test_to_png_decodes_with_pillow(server, tmp_path) -> None:
    """The independent-decoder proof (test-only Pillow, skipped when absent):
    a real PNG implementation reads back exactly the golden's RGB pixels,
    so the stdlib round-trip above cannot hide a mirrored encoder bug."""
    image = pytest.importorskip(
        "PIL.Image", reason="Pillow is the test-only independent PNG decoder"
    )
    server.run(
        [*flows.granted_steps(), *_observe_steps(GOLDEN, width=W, height=H, stride=STRIDE)]
    )
    conn = _connect(server)
    frame = conn.request_grant().await_consent().observe()
    path = tmp_path / "frame.png"
    frame.to_png(path)
    with image.open(path) as img:
        assert img.size == (W, H)
        assert img.mode == "RGB"
        assert img.tobytes() == _rgb_of(GOLDEN)
    conn.close()


# --- stride != width * 4 ----------------------------------------------------


def test_frame_addressing_is_stride_generic_over_a_padded_memfd(tmp_path) -> None:
    """stride != width*4: the Frame layer addresses rows through stride.

    Version 1 pins ``stride == width * 4`` *on the wire* (IDL frame_ready:
    "stride equals width * 4 exactly"; anything else is a server contract
    violation — the next test). The frame_ready signature nonetheless
    carries stride as its own argument precisely so a later version can
    relax it, and the Frame layer is written against the generic addressing
    rule ("row r begins at byte offset r * stride", ``width * 4`` payload
    bytes per row). Proven here through the same close-after-copy read path
    ``observe()`` uses, over a real sealed memfd with 16 padding bytes per
    row."""
    padded = _padded_golden()
    stride = STRIDE + PAD
    fd = make_sealed_memfd(padded)
    frame = Frame._from_fd(fd, format=Format.XRGB8888, width=W, height=H, stride=stride)
    with pytest.raises(OSError):
        os.fstat(fd)  # close-after-copy: the fd died inside _from_fd
    assert (frame.width, frame.height, frame.stride) == (W, H, stride)
    assert frame.size == len(frame.raw) == stride * H
    assert frame.raw == padded  # raw is the whole buffer, padding included
    path = tmp_path / "padded.png"
    frame.to_png(path)
    width, height, rgb = _decode_png_rgb(path.read_bytes())
    assert (width, height) == (W, H)
    assert rgb == _rgb_of(GOLDEN)  # the padding never reaches the pixels


def test_padded_stride_on_the_wire_is_a_contract_violation(server) -> None:
    """Serving that same padded frame *through the wire* violates the
    version-1 contract: the IDL pins stride == width * 4 exactly, so the
    SDK must sanction it rather than quietly widening the contract. If a
    later protocol version relaxes the pin, this is the test that changes
    — the Frame layer above is already generic."""
    server.run(
        [
            *flows.granted_steps(),
            *_observe_steps(_padded_golden(), width=W, height=H, stride=STRIDE + PAD),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(ServerContractViolation, match="stride"):
        grant.observe()
    assert conn.closed  # a server violation, never attributed to the grant


# --- fd hygiene -------------------------------------------------------------


def test_repeated_observe_holds_fd_count_flat(server) -> None:
    """The no-leak acceptance: over a capture loop the process fd count is
    flat — asserted with every captured Frame still alive, which is the
    property the close-after-copy lifecycle buys (a Frame never owns an
    fd, so flatness cannot depend on close() discipline or GC timing).

    Each iteration syncs after its capture: the mock server executes steps
    strictly in order, so once done(k) is back the server-side memfd copy
    for capture k is provably closed and the count is race-free. The
    memfds themselves are created just-in-time by the send_memfd step, so
    script construction pre-opens nothing."""
    rounds = 8
    steps: list[tuple] = [*flows.granted_steps()]
    for cookie in range(1, rounds + 1):
        steps += [
            *_observe_steps(GOLDEN, width=W, height=H, stride=STRIDE),
            ("expect", encode_sync(cookie)),
            ("send", flows.done_frame(cookie)),
        ]
    # Hold the server side open past the last done: without this the script
    # ends and the server thread closes its accepted socket concurrently
    # with the final count, making the measurement racy.
    steps.append(("linger",))
    server.run(steps)
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    baseline = _open_fd_count()
    frames = []
    for _ in range(rounds):
        frames.append(grant.observe())
        conn.sync()
        assert _open_fd_count() == baseline, "fd count must stay flat mid-loop"
    assert all(frame.raw == GOLDEN for frame in frames)
    assert _open_fd_count() == baseline
    conn.close()


# --- the encoder in isolation ----------------------------------------------


def test_xrgb_to_rgb_swizzle_vector() -> None:
    """Byte-order pin, cross-referenced with capture.rs
    ``rgba_to_xrgb8888_swizzles_and_pins_padding``: the same two pixels the
    Rust test converts *into* wire form come back out as their RGB
    channels here."""
    wire = bytes([0x33, 0x22, 0x11, 0xFF, 0xCC, 0xBB, 0xAA, 0xFF])
    assert xrgb8888_to_rgb(wire, width=2, height=1, stride=8) == bytes(
        [0x11, 0x22, 0x33, 0xAA, 0xBB, 0xCC]
    )


def test_encode_png_minimal_vector() -> None:
    """A 2x2 frame encodes to a spec-valid PNG whose decoded pixels are the
    swizzled input — pinpoints encoder bugs without any wire machinery —
    and encoding is deterministic (one code path, pinned zlib level)."""
    wire = bytes([0x33, 0x22, 0x11, 0xFF] * 2 + [0xCC, 0xBB, 0xAA, 0xFF] * 2)
    data = encode_png(wire, width=2, height=2, stride=8)
    width, height, rgb = _decode_png_rgb(data)
    assert (width, height) == (2, 2)
    assert rgb == bytes([0x11, 0x22, 0x33] * 2 + [0xAA, 0xBB, 0xCC] * 2)
    assert data == encode_png(wire, width=2, height=2, stride=8)


# --- Frame the value object -------------------------------------------------


def test_frame_constructor_rejects_inconsistent_geometry() -> None:
    ok = b"\x00" * 16
    Frame(ok, format=Format.XRGB8888, width=2, height=2, stride=8)  # sane
    with pytest.raises(ValueError, match="dimensions"):
        Frame(b"", format=Format.XRGB8888, width=0, height=1, stride=0)
    with pytest.raises(ValueError, match="stride"):
        Frame(ok, format=Format.XRGB8888, width=4, height=1, stride=8)
    with pytest.raises(ValueError, match="bytes"):
        Frame(ok[:-1], format=Format.XRGB8888, width=2, height=2, stride=8)


def test_to_png_rejects_non_xrgb_formats(tmp_path) -> None:
    """v1 capture only ever announces xrgb8888 (verified at receipt); the
    direct-construction door states its limit as a typed ValueError."""
    frame = Frame(b"\x00" * 16, format=Format.ARGB8888, width=2, height=2, stride=8)
    with pytest.raises(ValueError, match="xrgb8888"):
        frame.to_png(tmp_path / "x.png")
