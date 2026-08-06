# SPDX-License-Identifier: Apache-2.0
"""Unit tests for the framing primitives and client-side validation."""

from __future__ import annotations

import pytest

from vitrin_os import errors, messages
from vitrin_os.wire import (
    HEADER_SIZE,
    MessageDecoder,
    MessageEncoder,
    fixed_to_float,
    float_to_fixed,
    pack_header,
    unpack_header,
)


def test_header_roundtrip() -> None:
    buf = pack_header(0xDEADBEEF, 0x1234, 0x56, 1)
    assert len(buf) == HEADER_SIZE
    assert unpack_header(buf) == (0xDEADBEEF, 0x1234, 0x56, 1)


@pytest.mark.parametrize(
    ("text", "padded_len"),
    [("", 4), ("a", 8), ("ab", 8), ("abc", 8), ("abcd", 8), ("abcde", 12)],
)
def test_string_padding(text: str, padded_len: int) -> None:
    frame = MessageEncoder().put_string(text, max_bytes=64).finish(1, 0)
    assert len(frame) == HEADER_SIZE + padded_len
    dec = MessageDecoder(frame[HEADER_SIZE:])
    assert dec.string(max_bytes=64) == text
    dec.finish()


def test_string_over_bound_raises_before_sending() -> None:
    with pytest.raises(ValueError, match="exceeds its documented bound"):
        MessageEncoder().put_string("x" * 65, max_bytes=64)


def test_embedded_nul_rejected_on_encode() -> None:
    with pytest.raises(ValueError, match="embedded NUL"):
        MessageEncoder().put_string("a\x00b", max_bytes=64)


def test_null_string_requires_allow_null() -> None:
    with pytest.raises(ValueError, match="allow-null"):
        MessageEncoder().put_string(None, max_bytes=64)
    # Null and empty encode identically (length 0).
    null = MessageEncoder().put_string(None, max_bytes=64, allow_null=True).finish(1, 0)
    empty = MessageEncoder().put_string("", max_bytes=64).finish(1, 0)
    assert null == empty


def test_decoder_rejects_trailing_bytes() -> None:
    dec = MessageDecoder(b"\x2a\x00\x00\x00\xff")
    dec.uint()
    with pytest.raises(errors.ServerContractViolation, match="trailing"):
        dec.finish()


def test_decoder_rejects_short_payload() -> None:
    with pytest.raises(errors.ServerContractViolation, match="shorter"):
        MessageDecoder(b"\x2a\x00").uint()


def test_decoder_rejects_bad_utf8_and_nonzero_padding() -> None:
    # length 1, invalid UTF-8 byte, 3 padding bytes
    with pytest.raises(errors.ServerContractViolation, match="UTF-8"):
        MessageDecoder(b"\x01\x00\x00\x00\xff\x00\x00\x00").string(max_bytes=64)
    # length 1, 'a', nonzero padding
    with pytest.raises(errors.ServerContractViolation, match="padding"):
        MessageDecoder(b"\x01\x00\x00\x00a\x00\x01\x00").string(max_bytes=64)


def test_fixed_point_conversions() -> None:
    assert fixed_to_float(384) == 1.5
    assert fixed_to_float(-256) == -1.0
    assert float_to_fixed(1.5) == 384
    assert float_to_fixed(-1.0) == -256


def test_frame_size_ceiling_binds_the_sender() -> None:
    enc = MessageEncoder()
    for _ in range(0x4000):  # 16384 uints = 65536 payload bytes; 65544 > 65535
        enc.put_uint(0)
    with pytest.raises(ValueError, match="ceiling"):
        enc.finish(1, 0)


def test_type_control_character_rule() -> None:
    # Newline and tab are legal; other C0/C1 controls and DEL are not.
    assert messages.encode_type(8, text="ok\n\tok")
    for bad in ("\x00", "\x1b", "\x7f", "\x85", "\x9f"):
        with pytest.raises(ValueError, match="control character"):
            messages.encode_type(8, text=f"a{bad}b")


def test_request_grant_rejects_zero_verbs() -> None:
    with pytest.raises(ValueError, match="non-zero"):
        messages.encode_request_grant(
            3,
            grant_id=4,
            consent_id=5,
            view_id=6,
            pointer_id=7,
            text_id=8,
            resource=None,
            verbs=0,
            expiry_ms=0,
            max_event_rate=0,
            persistence=0,
            flags=0,
        )


def test_id_allocator_watermark() -> None:
    """The SDK enforces the strictly-increasing, never-reused id rule itself."""
    from vitrin_os.client import Connection
    from vitrin_os.protocol import CLIENT_ID_MAX

    class _FakeTransport:
        closed = False

        def close(self) -> None:
            self.closed = True

    conn = Connection(_FakeTransport())  # type: ignore[arg-type]
    first = conn._allocate_ids(5)
    second = conn._allocate_ids(2)
    assert first == [2, 3, 4, 5, 6]
    assert second == [7, 8]

    conn._next_id = CLIENT_ID_MAX  # one id left
    assert conn._allocate_ids(1) == [CLIENT_ID_MAX]
    with pytest.raises(errors.ObjectIdsExhausted):
        conn._allocate_ids(1)


def test_typed_exception_mappings_are_exhaustive() -> None:
    """Every wire code maps to exactly one distinct exception class."""
    from vitrin_os.protocol import ErrorCode, Outcome, Refusal

    fatal_classes = {
        type(errors.fatal_error_by_code(1, code, "m")) for code in ErrorCode
    }
    assert len(fatal_classes) == len(ErrorCode) == 10

    outcome_classes = {
        type(errors.resolution_error_by_outcome(o))
        for o in Outcome
        if o != Outcome.GRANTED
    }
    assert len(outcome_classes) == len(Outcome) - 1 == 5

    refusal_classes = {
        type(errors.refusal_error_by_code(1, code, 0)) for code in Refusal
    }
    assert len(refusal_classes) == len(Refusal) == 9

    # Unknown codes are a server contract violation, never a silent collapse.
    with pytest.raises(errors.ServerContractViolation):
        errors.fatal_error_by_code(1, 10, "m")
    with pytest.raises(errors.ServerContractViolation):
        errors.resolution_error_by_outcome(6)
    with pytest.raises(errors.ServerContractViolation):
        errors.refusal_error_by_code(1, 9, 0)


def test_capacity_decodes_to_at_capacity_not_a_contract_violation() -> None:
    """Refusal code 8 is ``AtCapacity`` — a refusal, not an accusation.

    Four surfaces (the IDL entry, conventions §5.3's mapping table, prose
    page 04, and the launcher page) promise this class by name. Before it
    existed, code 8 fell through ``_REFUSAL_BY_CODE`` and raised
    ``ServerContractViolation`` — telling the caller the *server* broke the
    contract, for a code the server is documented to send.
    """
    from vitrin_os import AtCapacity
    from vitrin_os.protocol import Refusal

    err = errors.refusal_error_by_code(
        int(protocol_verb := 512), Refusal.CAPACITY, 0, grant_id=4
    )
    assert isinstance(err, AtCapacity)
    assert isinstance(err, errors.GrantRefused)
    assert err.code == Refusal.CAPACITY == 8
    assert err.verb == protocol_verb
    assert err.grant_id == 4
    # retry_after_ms is 0 by the IDL: the core cannot know when a realm
    # exits, so this is not a rate limit in disguise.
    assert err.retry_after_ms == 0
