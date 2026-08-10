#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
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

or, without writing anything, to check the seeds already on disk:

    python3 fuzz/seed_corpus.py --check

Deliberately dependency-free stdlib Python (same rule
`tests/integration/README.md` states for the SDK): this script never needs
a Rust toolchain, matching the repo's Python tooling elsewhere. The frame
layout mirrors `sdk/python/src/vitrin_os/wire.py` exactly (same header
struct, same string encoding) -- both are hand-transcriptions of
`docs/protocol/00-conventions.md` section 2, not code shared with the fuzz
targets, which is the point: an independent encoding of the same spec is
what proves the seed inputs are "real" wire bytes, not an artifact of
whatever the decoder itself would produce.

# A seed that claims a path must reach it

Every seed below is named for a wire condition, and that name is the only
thing a reviewer reads. Two seeds were found (2026-07-25) to have rotted
into names that no longer described their bytes -- `attach_with_fd`
declared `fd_count = 0` in a header whose message type requires an fd, so
it always failed at the decoder's `FdCountMismatch` gate instead of
reaching the successful-decode path it exists to seed; and
`unsolicited_fd` put its fd on a zero-length `sendmsg`, which Linux drops
on a `SOCK_STREAM` socket, so it degenerated into a plain valid frame and
never seeded `PeerViolation::UnsolicitedFd` at all. Neither was visible
from a `git diff`: a dead seed is silently a no-op.

Two checks now stop that class of rot from returning silently:

1. :func:`verify_seeds` below -- structural, and run unconditionally on
   every regeneration and by ``--check``. It cross-checks each seed's
   header bytes against `protocol/vitrin-v0.xml` itself (message order,
   opcode, whether the message carries an fd) and against the two fuzz
   targets' documented input layouts, and it requires every seed in a
   target to be byte-distinct from every other.
2. `fuzz/tests/seed_corpus_reachability.rs` -- authoritative, and run
   with `cargo test --manifest-path fuzz/Cargo.toml`. It feeds each seed
   file to the *real* decoder / the *real* `vitrin_ipc::Connection` over
   a real `socketpair` and asserts the exact outcome the seed's name
   claims. That test is the one that cannot be fooled by a model; the
   structural check here exists so the failure is legible without a Rust
   toolchain.
"""

from __future__ import annotations

import pathlib
import struct
import sys
import xml.etree.ElementTree as ET

HEADER = struct.Struct("<IHBB")  # object_id, size, opcode, fd_count
ROOT = pathlib.Path(__file__).resolve().parent
#: The IDL is the source of truth for message order, opcodes, and which
#: messages carry an fd (CLAUDE.md's protocol authoring rule). The checks
#: below read it rather than restating it, so an IDL change that renumbers
#: a message cannot leave a stale selector byte in a seed.
IDL = ROOT.parent / "protocol" / "vitrin-v0.xml"


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
# protocol_decode: data[0] = decoder selector (mod MESSAGE_COUNT), data[1] = fd flag
# (bit 0), data[2:] = frame bytes handed to decode() verbatim. Selector
# indices below MUST match fuzz_targets/protocol_decode.rs's DECODERS
# table order exactly -- see that file's `decoder_table!` invocation.
# ---------------------------------------------------------------------------

SEL_HELLO = 0
SEL_FRAME_READY = 15
SEL_ATTACH = 28


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
        #
        # `fd_count=1` is load-bearing, not decoration: `attach` has
        # `HAS_FD = true`, and the generated `decode` checks the *header's*
        # `fd_count` byte independently of the out-of-band fd
        # (conventions 2.4's two disjuncts). Built with the default
        # `fd_count=0` this seed died at that second gate on every run and
        # never reached the successful-decode path it exists to seed.
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
            fd_count=1,
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
        #
        # The split point is the whole frame length, NOT 0, and that is
        # load-bearing: the fd rides the *first* `sendmsg`, and Linux
        # discards `SCM_RIGHTS` on a zero-length `SOCK_STREAM` send
        # (`unix_stream_sendmsg`'s `while (sent < len)` never runs a single
        # iteration for `len == 0`, so neither the bytes nor the ancillary
        # payload leave). With the split at 0 this seed sent its fd on an
        # empty write, the kernel dropped it, and what arrived was a plain
        # valid frame -- the same stream `valid_frame_one_write` already
        # seeds. Sending the whole frame in that first, fd-bearing write
        # makes the fd's delivering `recvmsg` span end exactly at the
        # frame's last byte, which is what `recv_message` reads as "an fd
        # attached to a frame that declares none".
        "unsolicited_fd": bytes([1, 0, len(plain_frame)]) + plain_frame,
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


# ---------------------------------------------------------------------------
# The self-test: a seed that claims a path must reach it.
#
# Structural half only -- see this module's docstring. Anything that needs
# the real decoder's verdict lives in
# `fuzz/tests/seed_corpus_reachability.rs`; what is checkable from the bytes
# alone is checked here so a rotted seed is legible without a Rust toolchain.
# ---------------------------------------------------------------------------


class SeedRotError(AssertionError):
    """A seed's bytes no longer match the path its name claims."""


class Message:
    """One IDL message, as `fuzz_targets/protocol_decode.rs` indexes them."""

    __slots__ = ("interface", "kind", "name", "opcode", "has_fd")

    def __init__(self, interface: str, kind: str, name: str, opcode: int, has_fd: bool):
        self.interface = interface
        self.kind = kind
        self.name = name
        self.opcode = opcode
        self.has_fd = has_fd

    def __str__(self) -> str:
        return f"{self.interface}.{self.name}"


def idl_messages() -> list[Message]:
    """Every message in `protocol/vitrin-v0.xml`, in DECODERS-table order.

    The fuzz target selects a decoder by `data[0] % DECODERS.len()`, and its
    table is written in IDL declaration order -- interfaces top to bottom,
    each interface's requests before its events, which is also how the
    generator assigns the implicit per-interface opcodes
    (`crates/vitrin-scanner`). Deriving the table here instead of restating
    it means a renumbering in the IDL surfaces as a failing check rather
    than as a seed that quietly selects the wrong decoder.
    """
    root = ET.parse(IDL).getroot()
    out: list[Message] = []
    for iface in root.findall("interface"):
        iface_name = iface.get("name") or "?"
        for kind in ("request", "event"):
            for opcode, msg in enumerate(iface.findall(kind)):
                has_fd = any(a.get("type") == "fd" for a in msg.findall("arg"))
                out.append(
                    Message(iface_name, kind, msg.get("name") or "?", opcode, has_fd)
                )
    return out


#: What each `protocol_decode` seed claims, as `(selector, message, outcome)`.
#: `outcome` is the first gate the seed is meant to reach in the generated
#: `decode`'s fixed check order (fd presence, header, opcode, size, header
#: fd_count, then per-argument reads); `"decodes"` means it clears them all.
PROTOCOL_DECODE_CLAIMS: dict[str, tuple[int, str, str]] = {
    "valid_hello": (SEL_HELLO, "vitrin_handshake.hello", "decodes"),
    "truncated_below_header": (SEL_HELLO, "vitrin_handshake.hello", "truncated"),
    "size_field_mismatch": (SEL_HELLO, "vitrin_handshake.hello", "size_mismatch"),
    "fd_count_mismatch_none_expected": (
        SEL_HELLO,
        "vitrin_handshake.hello",
        "fd_count_mismatch",
    ),
    "fd_count_mismatch_fd_expected": (
        SEL_FRAME_READY,
        "vitrin_view.frame_ready",
        "fd_count_mismatch",
    ),
    "attach_with_fd": (SEL_ATTACH, "vitrin_shim_surface.attach", "decodes"),
    "embedded_nul_in_string": (SEL_HELLO, "vitrin_handshake.hello", "embedded_nul"),
}

#: What each `ipc_framing` seed claims. `fd_bearing_write` names which of the
#: target's two `sendmsg` calls must actually carry the fd for the claim to
#: mean anything (`None` when the seed attaches none); a write that carries
#: an fd but no bytes delivers neither, so this is exactly the check the
#: `unsolicited_fd` rot would have failed.
IPC_FRAMING_CLAIMS: dict[str, tuple[str, int | None]] = {
    "valid_frame_one_write": ("frame_then_eof", None),
    "valid_frame_split_mid_header": ("frame_then_eof", None),
    "undersized_size_field": ("undersized_size_field", None),
    "unsolicited_fd": ("unsolicited_fd", 0),
    "missing_fd": ("missing_fd", None),
    "truncated_after_header": ("eof", None),
    "fd_count_exceeded": ("fd_count_exceeded", None),
}


def _distinct(target: str, seeds: dict[str, bytes]) -> None:
    """No two seeds in a target may be byte-identical.

    A duplicate is a seed that costs review attention and buys no coverage;
    it is also the shape `unsolicited_fd` rotted into once the kernel had
    dropped its (undeliverable) fd.
    """
    seen: dict[bytes, str] = {}
    for name, data in seeds.items():
        if data in seen:
            raise SeedRotError(
                f"{target}: seed {name!r} is byte-identical to {seen[data]!r}; one of "
                "the two exercises nothing its sibling does not"
            )
        seen[data] = name


def verify_protocol_decode_seeds(seeds: dict[str, bytes]) -> None:
    messages = idl_messages()
    _distinct("protocol_decode", seeds)
    if set(seeds) != set(PROTOCOL_DECODE_CLAIMS):
        raise SeedRotError(
            "every protocol_decode seed must declare what it claims to reach; "
            f"unclaimed: {sorted(set(seeds) - set(PROTOCOL_DECODE_CLAIMS))}, "
            f"claimed but absent: {sorted(set(PROTOCOL_DECODE_CLAIMS) - set(seeds))}"
        )
    for name, data in seeds.items():
        selector, message_name, outcome = PROTOCOL_DECODE_CLAIMS[name]
        where = f"protocol_decode/{name}"
        if len(data) < 2:
            raise SeedRotError(f"{where}: below the target's 2-byte input floor")
        if data[0] % len(messages) != selector:
            raise SeedRotError(
                f"{where}: selector byte {data[0]} picks decoder "
                f"{data[0] % len(messages)}, not the claimed {selector}"
            )
        msg = messages[selector]
        if str(msg) != message_name:
            raise SeedRotError(
                f"{where}: selector {selector} is {msg} in protocol/vitrin-v0.xml, "
                f"not the claimed {message_name} -- the IDL moved under this seed"
            )
        want_fd = bool(data[1] & 1)
        body = data[2:]
        if outcome != "decodes":
            continue
        # A successful decode has to clear every gate. Each of these is a
        # real gate in the generated `decode`, in its own check order.
        if want_fd != msg.has_fd:
            raise SeedRotError(
                f"{where}: claims to decode, but hands the decoder "
                f"{'an' if want_fd else 'no'} out-of-band fd while {msg} has "
                f"HAS_FD = {str(msg.has_fd).lower()} -- FdCountMismatch, every run"
            )
        if len(body) < HEADER.size:
            raise SeedRotError(f"{where}: claims to decode, but has no full header")
        _object_id, size, opcode, fd_count = HEADER.unpack(body[: HEADER.size])
        if opcode != msg.opcode:
            raise SeedRotError(
                f"{where}: claims to decode, but the header's opcode {opcode} is not "
                f"{msg}'s {msg.opcode} -- OpcodeMismatch, every run"
            )
        if size != len(body):
            raise SeedRotError(
                f"{where}: claims to decode, but the header declares {size} bytes and "
                f"the frame is {len(body)} -- SizeMismatch, every run"
            )
        if fd_count != int(msg.has_fd):
            raise SeedRotError(
                f"{where}: claims to decode, but the header's fd_count byte is "
                f"{fd_count} and {msg} requires {int(msg.has_fd)} -- FdCountMismatch, "
                "every run. (The header byte is checked independently of the "
                "out-of-band fd: conventions 2.4's two disjuncts.)"
            )


def verify_ipc_framing_seeds(seeds: dict[str, bytes]) -> None:
    _distinct("ipc_framing", seeds)
    if set(seeds) != set(IPC_FRAMING_CLAIMS):
        raise SeedRotError(
            "every ipc_framing seed must declare what it claims to reach; "
            f"unclaimed: {sorted(set(seeds) - set(IPC_FRAMING_CLAIMS))}, "
            f"claimed but absent: {sorted(set(IPC_FRAMING_CLAIMS) - set(seeds))}"
        )
    for name, data in seeds.items():
        _outcome, fd_write = IPC_FRAMING_CLAIMS[name]
        where = f"ipc_framing/{name}"
        if len(data) < 3:
            raise SeedRotError(f"{where}: below the target's 3-byte input floor")
        fd_counts = (data[0] % 4, data[1] % 4)
        rest = data[3:]
        split = (data[2] % (len(rest) + 1)) if rest else 0
        writes = (rest[:split], rest[split:])
        if fd_write is None:
            if any(fd_counts):
                raise SeedRotError(
                    f"{where}: claims to attach no fd, but the layout bytes ask for "
                    f"{fd_counts} on the two sendmsg calls"
                )
            continue
        if fd_counts[fd_write] < 1:
            raise SeedRotError(
                f"{where}: claims an fd on sendmsg #{fd_write}, but its fd-count byte "
                f"resolves to {fd_counts[fd_write]}"
            )
        if not writes[fd_write]:
            raise SeedRotError(
                f"{where}: claims an fd on sendmsg #{fd_write}, but the split byte "
                f"{data[2]} leaves that write with zero data bytes. Linux discards "
                "SCM_RIGHTS on a zero-length SOCK_STREAM send, so the fd never "
                "arrives and this seed degenerates into a plain byte stream"
            )


def verify_seeds(all_seeds: dict[str, dict[str, bytes]]) -> int:
    """Structurally verify the seeds this script *generates*; return the count.

    Raises :class:`SeedRotError` on a seed whose bytes no longer reach the
    path its name claims. Deliberately silent on success: it verifies the
    generator's output, and in ``--check`` mode that is only half the
    question -- the other half is whether the bytes on disk still match. Only
    :func:`main` knows both answers, so only :func:`main` prints a verdict.
    (It used to print "verified N seed(s)" from here, unconditionally, which
    in ``--check`` mode put a reassuring line on stdout even when the on-disk
    corpus had been altered and the FAIL lines had gone to stderr.)
    """
    verify_protocol_decode_seeds(all_seeds["protocol_decode"])
    verify_ipc_framing_seeds(all_seeds["ipc_framing"])
    return sum(len(s) for s in all_seeds.values())


def write_seeds(target: str, seeds: dict[str, bytes]) -> None:
    out_dir = ROOT / "corpus" / target
    out_dir.mkdir(parents=True, exist_ok=True)
    for name, data in seeds.items():
        (out_dir / name).write_bytes(data)
    print(f"wrote {len(seeds)} seed(s) to {out_dir}")


def check_seeds(target: str, seeds: dict[str, bytes]) -> list[str]:
    """Diff the generator's output against `corpus/<target>/` on disk."""
    out_dir = ROOT / "corpus" / target
    problems = []
    for name, data in seeds.items():
        path = out_dir / name
        if not path.is_file():
            problems.append(f"{target}/{name}: missing on disk")
        elif path.read_bytes() != data:
            problems.append(
                f"{target}/{name}: on-disk bytes differ from this generator's output "
                "-- re-run `python3 fuzz/seed_corpus.py` and commit the result"
            )
    for path in sorted(out_dir.glob("*")):
        if path.is_file() and path.name not in seeds:
            # A crash-derived regression promoted by hand (fuzz/.gitignore's
            # note) is legitimate and unnamed here; only report it.
            print(f"note: {target}/{path.name} is not generated by this script")
    return problems


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    check_only = argv == ["--check"]
    if argv and not check_only:
        print("usage: seed_corpus.py [--check]", file=sys.stderr)
        return 2

    all_seeds = {
        "protocol_decode": protocol_decode_seeds(),
        "ipc_framing": ipc_framing_seeds(),
    }
    # Unconditional: a regeneration must never be able to write a dead seed.
    total = verify_seeds(all_seeds)

    if check_only:
        problems = [p for t, s in all_seeds.items() for p in check_seeds(t, s)]
        for problem in problems:
            print(f"FAIL: {problem}", file=sys.stderr)
        if problems:
            # On stdout too, and *instead of* any success line: a log that
            # keeps only stdout must not be able to read as a clean pass.
            print(
                f"FAILED: {len(problems)} seed(s) on disk do not match this "
                "generator (details on stderr). The structural verification this "
                "script just ran covered the bytes it would WRITE, not the bytes in "
                "`fuzz/corpus/` -- so it says nothing about the corpus the fuzzer "
                "actually reads."
            )
            return 1
        print(
            f"verified {total} seed(s): every seed reaches the path its name claims, "
            "and every corpus file on disk matches this generator byte for byte"
        )
        print(
            "  (structural half; run `cargo test --manifest-path fuzz/Cargo.toml` for "
            "the real-decoder proof)"
        )
        return 0
    for target, seeds in all_seeds.items():
        write_seeds(target, seeds)
    print(f"verified {total} seed(s): every seed reaches the path its name claims")
    print(
        "  (structural half; run `cargo test --manifest-path fuzz/Cargo.toml` for "
        "the real-decoder proof)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
