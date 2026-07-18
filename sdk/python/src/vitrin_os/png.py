"""Minimal deterministic PNG encoding for captured frames (pure stdlib).

PNG encoding lives in the SDK, never in the core (plan risk R7: the TCB
dependency budget) — the wire carries raw xrgb8888 only, and this module is
where those bytes become a portable image.

The encoder is deliberately hand-rolled on ``zlib`` + ``struct`` instead of
delegating to Pillow (the decision issue #41 left open):

- Decision D8 pins the SDK to pure Python with zero runtime dependencies,
  so ``Frame.to_png()`` must work on every install — it cannot *require*
  an image library, and raising without one would leave the advertised API
  broken on the default install.
- An optional-import fork ("use Pillow when installed") would make
  ``to_png()`` output environment-dependent: Pillow picks per-row filters
  heuristically, so the same frame would produce different files on
  different machines. One fixed code path — filter 0 on every scanline,
  one zlib stream at a pinned level — keeps the output a pure function of
  the frame bytes for any given zlib build. The SDK therefore **never
  imports Pillow**; the test suite alone uses it, as an independent
  decoder for the round-trip proof.
- The golden gate compares *decoded pixels*, never compressed container
  bytes: zlib builds may compress differently across platforms, so no test
  pins the ``.png`` file bytes themselves.

The output is a minimal, spec-valid PNG: signature, IHDR (8-bit truecolor,
no interlace), one IDAT, IEND.
"""

from __future__ import annotations

import struct
import zlib

__all__ = ["encode_png", "xrgb8888_to_rgb"]

_SIGNATURE = b"\x89PNG\r\n\x1a\n"
# Pinned explicitly (never zlib's default alias) so identical frames yield
# identical files under any one zlib build.
_ZLIB_LEVEL = 6
# The PNG spec caps dimensions at 2^31 - 1 (4-byte fields, high bit clear).
_MAX_DIMENSION = 2**31 - 1


def xrgb8888_to_rgb(
    pixels: bytes | bytearray | memoryview, *, width: int, height: int, stride: int
) -> bytes:
    """Repack little-endian xrgb8888 rows into tightly packed RGB bytes.

    The wire pixel layout is bytes ``B, G, R, X`` per pixel (xrgb8888
    little-endian, X pinned ``0xFF``); row ``r`` begins at byte offset
    ``r * stride`` and carries ``width * 4`` payload bytes. Addressing is
    **stride-generic**: trailing per-row padding (``stride > width * 4`` —
    a later-version seam, never emitted in version 1) is skipped. This
    function is where the XRGB→RGB conversion responsibility lives — on
    the SDK presentation boundary — so ``Frame.raw`` stays wire-exact (the
    observation-digest domain) and the core never grows a converter.
    """
    if width <= 0 or height <= 0:
        raise ValueError("frame dimensions must be positive")
    row_bytes = width * 4
    if stride < row_bytes:
        raise ValueError(f"stride {stride} cannot hold {width} 4-byte pixels per row")
    view = memoryview(pixels).cast("B")
    if len(view) != stride * height:
        raise ValueError(
            f"buffer holds {len(view)} bytes, expected stride * height = {stride * height}"
        )
    if stride == row_bytes:
        tight = view
    else:
        tight = memoryview(
            b"".join(view[r * stride : r * stride + row_bytes] for r in range(height))
        )
    # Strided extraction runs in C; a per-pixel Python loop would not.
    rgb = bytearray(width * height * 3)
    rgb[0::3] = tight[2::4].tobytes()  # R: wire byte 2 of each pixel
    rgb[1::3] = tight[1::4].tobytes()  # G: wire byte 1
    rgb[2::3] = tight[0::4].tobytes()  # B: wire byte 0
    return bytes(rgb)


def _chunk(chunk_type: bytes, data: bytes) -> bytes:
    """One PNG chunk: length, type, data, CRC-32 over type + data."""
    return (
        struct.pack(">I", len(data))
        + chunk_type
        + data
        + struct.pack(">I", zlib.crc32(chunk_type + data))
    )


def encode_png(
    pixels: bytes | bytearray | memoryview, *, width: int, height: int, stride: int
) -> bytes:
    """Encode one xrgb8888 frame buffer as a complete PNG byte string.

    Deterministic by construction: filter byte 0 (None) on every scanline
    and a single zlib stream at the pinned level — compression quality is
    irrelevant next to reproducibility at screenshot sizes.
    """
    if width > _MAX_DIMENSION or height > _MAX_DIMENSION:
        raise ValueError("PNG dimensions are capped at 2**31 - 1")
    rgb = xrgb8888_to_rgb(pixels, width=width, height=height, stride=stride)
    row = width * 3
    scanlines = b"".join(b"\x00" + rgb[r * row : (r + 1) * row] for r in range(height))
    # bit depth 8, color type 2 (truecolor RGB), deflate, filter method 0,
    # no interlace.
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    idat = zlib.compress(scanlines, _ZLIB_LEVEL)
    return _SIGNATURE + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", idat) + _chunk(b"IEND", b"")
