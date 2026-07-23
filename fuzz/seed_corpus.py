#!/usr/bin/env python3
"""Regenerate the checked-in seed corpus for both fuzz targets.

Per the P1.9.3 (#46) acceptance criterion "fuzz corpus + regression inputs
checked in", `fuzz/corpus/<target>/` holds a small, hand-curated set of
byte-for-byte seed inputs -- not the sprawling, opaque corpus libFuzzer
itself grows during a run (which is real but not meant for review). Each
seed here is named for the specific wire condition it exercises, so a
diff to this script is reviewable the same way a diff to a hand-written
unit test is.

Run it (from the `fuzz/` directory or anywhere; paths are relative to this
file) to regenerate every seed file from scratch:

    python3 fuzz/seed_corpus.py

Deliberately dependency-free stdlib Python (same rule
`tests/integration/README.md` states for the SDK): this script never needs
a Rust toolchain, matching the repo's Python tooling elsewhere. The frame
layout mirrors `sdk/python/src/vitrin_os/wire.py` exactly (same header
struct, same string encoding) -- both are hand-transcriptions of
`docs/protocol/00-conventions.md` section 2, not code shared with the fuzz
targets, which is the point: an independent encoding of the same spec is
what proves the seed inputs are "real" wire bytes, not an artifact of
whatever the decoder itself would produce.
"""

from __future__ import annotations

import pathlib
import struct

HEADER = struct.Struct("<IHBB")  # object_id, size, opcode, fd_count
ROOT = pathlib.Path(__file__).resolve().parent


def u32(v: int) -> bytes:
    return struct.pack("<I", v)


def pad(n: int) -> bytes:
    return b"\x00" * ((-n) % 4)


def wire_string(s: str) -> bytes:
    b = s.encode("utf-8")
    return u32(len(b)) + b + pad(len(b))


def frame(object_id: int, opcode: int, payload: bytes, *, size_override: int | None = None, fd_count: int = 0) -> bytes:
    size = size_override if size_override is not None else 8 + len(payload)
    return HEADER.pack(object_id, size, opcode, fd_count) + payload


# ---------------------------------------------------------------------------
# protocol_decode: data[0] = decoder selector (mod 29), data[1] = fd flag
# (bit 0), data[2:] = frame bytes handed to decode() verbatim. Selector
# indices below MUST match fuzz_targets/protocol_decode.rs's DECODERS
# table order exactly -- see that file's `decoder_table!` invocation.
# ---------------------------------------------------------------------------

SEL_HELLO = 0
SEL_FRAME_READY = 11
SEL_ATTACH = 19


def protocol_decode_seeds() -> dict[str, bytes]:
    hello_payload = (
        u32(1)  # version
        + u32(2)  # principal (new_id)
        + wire_string("vitrin://local/agent/demo")
        + wire_string("bearer")
        + wire_string("a" * 64)
    )
    hello_frame = frame(1, 0, hello_payload)

    return {
        # A syntactically valid `hello` -- the happy path every mutation
        # starts from; also the target's own round-trip check (encode the
        # decoded value, decode it again, compare) gets a guaranteed first
        # exercise instead of relying on the fuzzer stumbling onto one.
        "valid_hello": bytes([SEL_HELLO, 0]) + hello_frame,
        # The one size violation a u16 field can express (conventions
        # 2.1): declared size below the 8-byte header minimum. Mirrors
        # `crates/vitrin-protocol/src/wire.rs`'s own truncated-buffer
        # tests, transcribed as a fuzz seed rather than a `#[test]`.
        "truncated_below_header": bytes([SEL_HELLO, 0, 0x01, 0x02, 0x03]),
        # A header whose declared `size` disagrees with the byte count
        # actually handed to `decode` -- `DecodeError::SizeMismatch`.
        "size_field_mismatch": bytes([SEL_HELLO, 0]) + frame(1, 0, hello_payload, size_override=0xFFFF),
        # `hello` declares no fd (`HAS_FD = false`); asking `decode` for an
        # fd anyway must hit `FdCountMismatch`, never desync the parse.
        "fd_count_mismatch_none_expected": bytes([SEL_HELLO, 1]) + hello_frame,
        # The inverse: `frame_ready`/`attach` (`HAS_FD = true`) decoded
        # with NO fd attached -- the other half of `FdCountMismatch`.
        "fd_count_mismatch_fd_expected": bytes([SEL_FRAME_READY, 0])
        + frame(1, 1, u32(0) + u32(0) + u32(0) + u32(0) + u32(0)),
        # `attach`, correctly supplied an fd -- the one seed that can only
        # succeed with `want_fd = true`, so the harness's fd-bearing
        # round-trip path (a *fresh* fd for the re-decode half) is seeded
        # too, not left to chance.
        "attach_with_fd": bytes([SEL_ATTACH, 1])
        + frame(
            1,
            0,
            u32(7)  # buffer_id
            + u32(0)  # kind = Shm (vitrin_shim_surface.Kind::Shm = 0)
            + u32(0x34325258)  # format = Xrgb8888 (vitrin_view.Format::Xrgb8888)
            + u32(4)  # width
            + u32(4)  # height
            + u32(16),  # stride
        ),
        # An embedded NUL inside a string argument's declared byte range --
        # `DecodeError::EmbeddedNul`, one of the "argument decode failure"
        # family `wire.rs::read_string` enforces.
        "embedded_nul_in_string": bytes([SEL_HELLO, 0])
        + frame(
            1,
            0,
            u32(1) + u32(2) + u32(1) + b"\x00" + pad(1) + wire_string("bearer") + wire_string("x"),
        ),
    }


# ---------------------------------------------------------------------------
# ipc_framing: data[0] = fds attached to the first sendmsg (mod 4),
# data[1] = fds attached to the second (mod 4), data[2] = split point
# (mod len(rest)+1), data[3:] = the raw bytes sent across the two
# sendmsg calls. See fuzz_targets/ipc_framing.rs's doc comment.
# ---------------------------------------------------------------------------


def ipc_framing_seeds() -> dict[str, bytes]:
    plain_frame = frame(1, 0, b"ping")  # fd_count = 0, no ancillary data

    return {
        # A complete, fd-less frame delivered in one `sendmsg` -- the
        # happy path `recv_message` must resolve to `Ok(Some(_))` then
        # `Ok(None)` on the follow-up EOF.
        "valid_frame_one_write": bytes([0, 0, 0]) + plain_frame,
        # The same frame split mid-header across the two `sendmsg` calls
        # this target always issues -- exercises the partial-header
        # reassembly path (`recv_buf.len() < HEADER_LEN`).
        "valid_frame_split_mid_header": bytes([0, 0, 4]) + plain_frame,
        # `size` below the 8-byte minimum -- `PeerViolation::UndersizedSizeField`,
        # the transport-level "oversized" fatal condition
        # (`crates/vitrin-ipc/tests/backpressure.rs`'s own regression,
        # reachable here as a seed instead of only a hand-written test).
        "undersized_size_field": bytes([0, 0, 0]) + HEADER.pack(1, 4, 0, 0),
        # A frame declaring `fd_count = 0` with an fd attached anyway --
        # `PeerViolation::UnsolicitedFd` (conventions 2.4's "fds attached
        # to a message that declares none").
        "unsolicited_fd": bytes([1, 0, 0]) + plain_frame,
        # A frame declaring one fd, sent with none attached -- the frame
        # completes on bytes alone, so this hits `PeerViolation::MissingFd`.
        "missing_fd": bytes([0, 0, 0]) + frame(1, 0, b"ping", fd_count=1),
        # A frame header with no bytes to follow it at all: the stream
        # ends mid-frame -- `TransportError::Eof`.
        "truncated_after_header": bytes([0, 0, 8]) + HEADER.pack(1, 32, 0, 0),
        # `fd_count` above the one-fd-per-message ceiling --
        # `PeerViolation::FdCountExceeded`.
        "fd_count_exceeded": bytes([0, 0, 0]) + frame(1, 0, b"", fd_count=2),
    }


def write_seeds(target: str, seeds: dict[str, bytes]) -> None:
    out_dir = ROOT / "corpus" / target
    out_dir.mkdir(parents=True, exist_ok=True)
    for name, data in seeds.items():
        (out_dir / name).write_bytes(data)
    print(f"wrote {len(seeds)} seed(s) to {out_dir}")


def main() -> None:
    write_seeds("protocol_decode", protocol_decode_seeds())
    write_seeds("ipc_framing", ipc_framing_seeds())


if __name__ == "__main__":
    main()
