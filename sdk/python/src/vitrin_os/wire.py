# SPDX-License-Identifier: Apache-2.0
"""Framing primitives: the 8-byte header and the seven argument types.

Hand-implemented from docs/protocol/00-conventions.md section 2 (and the
IDL's FRAMING description). Everything multi-byte is little-endian.

Header layout (8 bytes)::

    object_id (u32) | size (u16, whole frame incl. header) | opcode (u8) | fd_count (u8)

The seven argument types: int (s32), uint (u32), fixed (signed 24.8),
string (u32 byte length + UTF-8 + zero padding to 4), object (u32),
new_id (u32), fd (never in the body; SCM_RIGHTS, matched positionally).

Encoder misuse (a string over its documented bound, a frame over the u16
ceiling) raises :class:`ValueError` — those are caller bugs a correct
client must never put on the wire. Decoder failures raise
:class:`~vitrin_os.errors.ServerContractViolation` — on a client, inbound
bytes come from the server, and version 1 has no client-to-server error
channel, so the only sanction is disconnecting.
"""

from __future__ import annotations

import struct

from .errors import ServerContractViolation

HEADER = struct.Struct("<IHBB")
HEADER_SIZE = 8
MAX_FRAME_SIZE = 0xFFFF  # binds senders: a u16 cannot express more

_U32 = struct.Struct("<I")
_I32 = struct.Struct("<i")

FIXED_ONE = 256  # signed 24.8 fixed point: 24 integer bits, 8 fraction bits


def fixed_to_float(bits: int) -> float:
    """Interpret a signed 24.8 fixed-point wire value as a float."""
    return bits / FIXED_ONE


def float_to_fixed(value: float) -> int:
    """Encode a float as signed 24.8 fixed-point (truncating toward zero)."""
    return int(value * FIXED_ONE)


def pack_header(object_id: int, size: int, opcode: int, fd_count: int) -> bytes:
    return HEADER.pack(object_id, size, opcode, fd_count)


def unpack_header(buf: bytes | bytearray | memoryview) -> tuple[int, int, int, int]:
    """Return (object_id, size, opcode, fd_count) from the first 8 bytes."""
    return HEADER.unpack_from(buf)


def string_wire_length(encoded_len: int) -> int:
    """Bytes a string of ``encoded_len`` UTF-8 bytes occupies on the wire."""
    return 4 + (encoded_len + 3) // 4 * 4


class MessageEncoder:
    """Builds one frame's argument payload, then finishes it with a header."""

    def __init__(self) -> None:
        self._parts: list[bytes] = []
        self._size = 0

    def _append(self, part: bytes) -> None:
        self._parts.append(part)
        self._size += len(part)

    def put_uint(self, value: int) -> "MessageEncoder":
        if not 0 <= value <= 0xFFFFFFFF:
            raise ValueError(f"uint out of range: {value}")
        self._append(_U32.pack(value))
        return self

    def put_int(self, value: int) -> "MessageEncoder":
        if not -0x80000000 <= value <= 0x7FFFFFFF:
            raise ValueError(f"int out of range: {value}")
        self._append(_I32.pack(value))
        return self

    # object and new_id are u32 ids on the wire.
    put_object = put_uint
    put_new_id = put_uint

    def put_fixed_bits(self, bits: int) -> "MessageEncoder":
        return self.put_int(bits)

    def put_string(
        self,
        value: str | None,
        *,
        max_bytes: int,
        allow_null: bool = False,
    ) -> "MessageEncoder":
        """Encode a string argument.

        A null string is legal only where the IDL marks allow-null, and is
        encoded identically to the empty string (length 0) — the two are
        equivalent on the wire.
        """
        if value is None:
            if not allow_null:
                raise ValueError("null string where allow-null is not declared")
            value = ""
        encoded = value.encode("utf-8")
        if b"\x00" in encoded:
            raise ValueError("embedded NUL forbidden in wire strings")
        if len(encoded) > max_bytes:
            raise ValueError(
                f"string of {len(encoded)} bytes exceeds its documented "
                f"bound of {max_bytes} bytes"
            )
        padding = (-len(encoded)) % 4
        self._append(_U32.pack(len(encoded)))
        self._append(encoded)
        if padding:
            self._append(b"\x00" * padding)
        return self

    def finish(self, object_id: int, opcode: int, *, fd_count: int = 0) -> bytes:
        """Prepend the header and return the complete frame bytes."""
        size = HEADER_SIZE + self._size
        if size > MAX_FRAME_SIZE:
            # The 65535-byte ceiling binds senders (conventions section 2.1).
            raise ValueError(f"frame of {size} bytes exceeds the u16 size ceiling")
        return pack_header(object_id, size, opcode, fd_count) + b"".join(self._parts)


class MessageDecoder:
    """Reads one frame's argument payload (the bytes after the header)."""

    def __init__(self, payload: bytes) -> None:
        self._payload = payload
        self._offset = 0

    def _take(self, count: int) -> bytes:
        end = self._offset + count
        if end > len(self._payload):
            raise ServerContractViolation(
                "event payload shorter than its signature requires"
            )
        chunk = self._payload[self._offset : end]
        self._offset = end
        return chunk

    def uint(self) -> int:
        return _U32.unpack(self._take(4))[0]

    def int(self) -> int:
        return _I32.unpack(self._take(4))[0]

    def fixed_bits(self) -> int:
        return _I32.unpack(self._take(4))[0]

    def string(self, *, max_bytes: int) -> str:
        length = self.uint()
        if length > max_bytes:
            raise ServerContractViolation(
                f"string of {length} bytes exceeds its documented bound "
                f"of {max_bytes} bytes"
            )
        raw = self._take(length)
        padding = self._take((-length) % 4)
        if padding.strip(b"\x00"):
            raise ServerContractViolation("nonzero string padding bytes")
        if b"\x00" in raw:
            raise ServerContractViolation("embedded NUL in wire string")
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise ServerContractViolation(f"invalid UTF-8 in wire string: {exc}") from exc

    def finish(self) -> None:
        """Assert the whole payload was consumed (no trailing bytes)."""
        if self._offset != len(self._payload):
            raise ServerContractViolation(
                f"{len(self._payload) - self._offset} trailing bytes after "
                "the last argument of an event"
            )
