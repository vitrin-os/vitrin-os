"""Issue #46's second half: a hostile peer against a live `vitrind`.

`fuzz/` (this repo's cargo-fuzz workspace) proves the other half in-process:
feed `vitrin-protocol` and `vitrin-ipc` arbitrary bytes and arbitrary fd
counts and assert no panic, no UB, and a round-trip property on whatever the
decoder accepts. That is a *memory-safety* claim, and the zero-I/O crate
split (`docs/PRD.md` Doc 2 §2) is exactly what makes it fuzzable without a
socket.

This module is the *policy* claim `fuzz/` structurally cannot make: given a
real Unix socket and a real forked realm, does the shipped binary actually
kill a misbehaving connection, leave the core and every other connection
undamaged, and keep composing while it does it (P1.2.3's "never block the
compositor loop")? Every test here sends bytes a conformant SDK would never
construct -- that's what `RawPeer` below is for, in place of the ordinary
`harness.Core.connect()` -- and then proves two things, not one: the
hostile connection died, *and* the core is still alive and still serving.

Two disjoint outcomes are asserted on, by design, matching where each
violation is caught:

- **Protocol-level** violations (decoded far enough to name an object and a
  reason: `pre_handshake`, `auth_failed`, `version_unsupported`, a second
  `hello`, a truncated `hello` payload) get a best-effort
  `vitrin_handshake.error` event before the close (`principal.rs`'s
  `handle_message` funnel) -- asserted via `_expect_fatal_error`.
- **Transport-level** violations (a header lying about its own minimum
  size, a declared fd count the protocol cannot express, an fd flood) are
  caught in `vitrin-ipc`, *below* the layer that could ever construct a
  message to attach an error to -- the peer just gets a bare close, and
  that absence is itself the thing under test (`_expect_bare_close`).
"""

from __future__ import annotations

import array
import os
import socket
import time
import unittest

from harness import (
    DEMO_IDENTITY,
    DEMO_TOKEN,
    IntegrationTest,
    capture_when_ready,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors, messages, protocol, wire  # noqa: E402  (needs PYTHONPATH)

#: `vitrin_handshake` is bound at this fixed object id on every principal
#: connection (protocol/vitrin-v0.xml; `crates/vitrin-core/src/principal.rs`'s
#: `HANDSHAKE_ID`); the SDK's client-facing name for the same constant.
HANDSHAKE_ID = protocol.BOOTSTRAP_OBJECT_ID
#: `vitrin_handshake.error` and `vitrin_principal.bound` are each opcode 0 on
#: their own interface -- coincidence of document order, not a shared code.
ERROR_OPCODE = 0
BOUND_OPCODE = 0


class RawPeer:
    """A bare `AF_UNIX` socket to a core, with no framing discipline of its
    own -- what a hostile client is, by definition.

    Deliberately not `vitrin_os.Transport`: that class is the *conformant*
    peer every other integration test wants, and it has no route to attach
    `SCM_RIGHTS` ancillary data (version 1's principal side never sends an
    fd, so nothing in the SDK needs one) -- exactly the capability the
    fd-spam tests below require. Kept local to this module rather than
    promoted to `harness.py` so "the well-behaved client" and "the raw
    socket a hostile one would use" can never be confused for each other.
    """

    def __init__(self, path: os.PathLike[str] | str) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5.0)
        self.sock.connect(os.fspath(path))
        self._buf = b""

    def send(self, payload: bytes, fds: list[int] | None = None) -> None:
        if fds:
            anc = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", fds).tobytes())]
            self.sock.sendmsg([payload], anc)
        else:
            self.sock.sendall(payload)

    def recv_frame(self, timeout: float = 5.0) -> tuple[int, int, bytes] | None:
        """One `(object_id, opcode, payload)` frame, or `None` on close.

        A close before a complete header, or mid-frame, both collapse to
        `None`: either way, the peer was never handed the frame it might
        have named. Raises `socket.timeout` (a real exception, not `None`)
        if neither a full frame nor a close arrives in time -- the "still
        alive but stuck" case the slow-loris test tells apart from both.
        """
        self.sock.settimeout(timeout)
        while len(self._buf) < wire.HEADER_SIZE:
            chunk = self.sock.recv(4096)
            if not chunk:
                return None
            self._buf += chunk
        object_id, size, opcode, fd_count = wire.unpack_header(self._buf)
        assert fd_count == 0, (
            f"a {ERROR_OPCODE}/{BOUND_OPCODE} handshake-side event never carries an fd, "
            f"got fd_count={fd_count}"
        )
        while len(self._buf) < size:
            chunk = self.sock.recv(4096)
            if not chunk:
                return None
            self._buf += chunk
        payload = self._buf[wire.HEADER_SIZE : size]
        self._buf = self._buf[size:]
        return object_id, opcode, payload

    def expect_bare_close(self, timeout: float = 5.0) -> None:
        """No frame, no event: just gone. See the module docstring for why
        this is the *correct* outcome for a transport-level violation, not
        a missing courtesy.
        """
        frame = self.recv_frame(timeout)
        if frame is not None:
            raise AssertionError(f"expected a bare close, got a frame instead: {frame!r}")

    def close(self) -> None:
        self.sock.close()

    def __enter__(self) -> "RawPeer":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()


def _raw_frame(object_id: int, opcode: int, payload: bytes = b"", *, fd_count: int = 0) -> bytes:
    """A frame whose header HONESTLY declares its own size.

    Every hostile property under test here lives in `object_id`, `opcode`,
    `payload` or `fd_count` -- never in a size field that lies about the
    bytes that follow it (that lie is its own dedicated test, built by hand
    in `OversizedLengths`, precisely so it stays the one place a dishonest
    size field appears).
    """
    return wire.pack_header(object_id, wire.HEADER_SIZE + len(payload), opcode, fd_count) + payload


def _send_hello(
    peer: RawPeer,
    *,
    principal_id: int = 2,
    identity: str = DEMO_IDENTITY,
    credential_type: str = "static-token",
    credential: str = DEMO_TOKEN,
    version: int = protocol.PROTOCOL_VERSION,
) -> None:
    peer.send(
        messages.encode_hello(
            version=version,
            principal_id=principal_id,
            identity=identity,
            credential_type=credential_type,
            credential=credential,
        )
    )


def _hello_and_expect_bound(peer: RawPeer, *, principal_id: int = 2) -> str:
    """Send a byte-perfect `hello` and return the `bound` identity.

    Used by the tests below `hello` itself is not the thing under attack
    (a second `hello`, say) to get a real connection to the CONNECTED state
    first, with the ordinary SDK-equivalent bytes.
    """
    _send_hello(peer, principal_id=principal_id)
    frame = peer.recv_frame()
    assert frame is not None, "expected vitrin_principal.bound, got a close"
    object_id, opcode, payload = frame
    assert object_id == principal_id, (
        f"bound must name the principal id hello minted ({principal_id}), got {object_id}"
    )
    assert opcode == BOUND_OPCODE, f"vitrin_principal.bound is opcode {BOUND_OPCODE}, got {opcode}"
    dec = wire.MessageDecoder(payload)
    identity = dec.string(max_bytes=2048)
    dec.finish()
    return identity


def _expect_fatal_error(peer: RawPeer, expected_cls: type) -> "errors.FatalError":
    """Read one frame, assert it is a `vitrin_handshake.error` of
    `expected_cls`, then assert the bare close that follows every fatal
    error (conventions: the connection dies right after).
    """
    frame = peer.recv_frame()
    assert frame is not None, f"expected a {expected_cls.__name__} error event, got a close"
    object_id, opcode, payload = frame
    assert object_id == HANDSHAKE_ID, (
        f"vitrin_handshake.error always arrives on object {HANDSHAKE_ID}, got {object_id}"
    )
    assert opcode == ERROR_OPCODE, f"vitrin_handshake.error is opcode {ERROR_OPCODE}, got {opcode}"
    dec = wire.MessageDecoder(payload)
    cited_object = dec.uint()
    code = dec.uint()
    message = dec.string(max_bytes=1024)
    dec.finish()
    exc = errors.fatal_error_by_code(cited_object, code, message)
    assert isinstance(exc, expected_cls), f"expected {expected_cls.__name__}, got {exc!r}"
    peer.expect_bare_close()
    return exc


def _assert_core_survives(core) -> None:
    """The other half of every acceptance criterion in this module: not
    just "the hostile connection died" but "the core is undamaged" --
    proven by actually serving a fresh, legitimate connection afterward,
    not merely by the absence of a crash.
    """
    assert core.proc.poll() is None, "the core process must not crash on a hostile peer"
    conn = core.connect()
    try:
        capture_when_ready(whole_realm_grant(conn))
    finally:
        conn.close()


class PreHandshakeGarbage(IntegrationTest):
    """AC: "pre-handshake garbage: connection killed, core alive"."""

    def test_non_hello_first_frame_is_refused(self):
        core = self.core()
        with RawPeer(core.socket) as peer:
            # object_id=99, opcode=7: neither is defined anywhere. `dispatch`
            # reads the connection's phase before it ever consults the object
            # table, so this never gets far enough to be `invalid_object` or
            # `invalid_opcode` -- ANY traffic here is `pre_handshake`, whatever
            # it names.
            peer.send(_raw_frame(99, 7))
            exc = _expect_fatal_error(peer, errors.PreHandshake)
            self.assertEqual(exc.object_id, 99, "pre_handshake cites the object the garbage named")
        _assert_core_survives(core)

    def test_a_byte_perfect_request_for_the_wrong_phase_is_also_refused(self):
        """Even a real, correctly-encoded `vitrin_principal.get_realm` is
        `pre_handshake` before `hello` -- there is no message shape that
        skips authentication by arriving early for a *later* phase.
        """
        core = self.core()
        with RawPeer(core.socket) as peer:
            # object 5 is not HANDSHAKE_ID, so this is refused before the
            # payload is even decoded as anything -- get_realm's opcode (0)
            # deliberately collides with hello's, and the object id is what
            # tells them apart in `dispatch`.
            peer.send(messages.encode_get_realm(5, realm_id=2, name="realm-0"))
            exc = _expect_fatal_error(peer, errors.PreHandshake)
            self.assertEqual(exc.object_id, 5)
        _assert_core_survives(core)


class CredentialAndVersion(IntegrationTest):
    """The rest of the handshake state machine a hostile `hello` can hit --
    not named in #46's three bullets verbatim, but the same funnel, and
    cheap regression coverage for `principal.rs` at the wire.
    """

    def test_bad_credential_is_refused_uniformly(self):
        core = self.core()
        with RawPeer(core.socket) as peer:
            _send_hello(peer, credential=DEMO_TOKEN[:-1] + ("b" if DEMO_TOKEN[-1] != "b" else "c"))
            exc = _expect_fatal_error(peer, errors.AuthFailed)
            # Cause-uniform on the wire (conventions 5.2): the message text
            # must never leak *why* -- unknown identity and bad token alike.
            self.assertEqual(exc.wire_message, "authentication refused")
            self.assertEqual(exc.object_id, HANDSHAKE_ID)
        _assert_core_survives(core)

    def test_unsupported_version_is_refused(self):
        core = self.core()
        with RawPeer(core.socket) as peer:
            _send_hello(peer, version=protocol.PROTOCOL_VERSION + 41)
            exc = _expect_fatal_error(peer, errors.VersionUnsupported)
            # Downgrade is refusal, not negotiation: no supported-version
            # hint anywhere in the message text.
            self.assertNotIn(str(protocol.PROTOCOL_VERSION), exc.wire_message)
        _assert_core_survives(core)

    def test_second_hello_is_refused(self):
        core = self.core()
        with RawPeer(core.socket) as peer:
            identity = _hello_and_expect_bound(peer, principal_id=2)
            self.assertEqual(identity, DEMO_IDENTITY)
            _send_hello(peer, principal_id=3)  # legal exactly once
            exc = _expect_fatal_error(peer, errors.InvalidOpcode)
            self.assertEqual(exc.object_id, HANDSHAKE_ID)
        _assert_core_survives(core)


class OversizedLengths(IntegrationTest):
    """AC: "oversized lengths: connection killed, core alive".

    Split across the two layers that each own one half of "oversized":
    `vitrin-protocol` (a frame that is internally too short for the message
    its opcode claims) and `vitrin-ipc` (a header whose own size field is
    below the 8-byte minimum, before any message is even in play). A u16
    `size` field cannot express a length *above* 65535 (`MAX_MESSAGE_SIZE`
    is defined as exactly that ceiling) -- the whole point of picking it --
    so there is no wire encoding of "declared length too large" to craft;
    the two tests below are the length lies the wire format actually admits.
    """

    def test_truncated_hello_payload_is_refused_as_oversized(self):
        """A `hello` frame whose header size is internally consistent (the
        transport hands the protocol decoder exactly as many bytes as it
        declares) but too short to hold every field `hello` requires.
        Caught in `vitrin-protocol`, so it *is* a message the state machine
        can name -- hence a real `error` event, unlike the transport-level
        case below.
        """
        core = self.core()
        with RawPeer(core.socket) as peer:
            # version (u32) only; hello also needs principal (new_id) and
            # three length-prefixed strings. Genuinely truncated, not just
            # short-valued.
            payload = (1).to_bytes(4, "little")
            peer.send(_raw_frame(HANDSHAKE_ID, messages.OP_HELLO, payload))
            exc = _expect_fatal_error(peer, errors.Oversized)
            self.assertEqual(exc.object_id, HANDSHAKE_ID)
        _assert_core_survives(core)

    def test_undersized_size_field_is_dropped_at_the_transport_layer(self):
        """The header's OWN `size` claims fewer than the 8 bytes a header
        always occupies -- caught in `vitrin-ipc`'s frame reassembly,
        strictly below the layer that could name a message to attach an
        `error` event to. Sent as the connection's first bytes: this check
        runs before the handshake phase is even consulted.
        """
        core = self.core()
        with RawPeer(core.socket) as peer:
            peer.send(wire.pack_header(HANDSHAKE_ID, 4, 0, 0))  # size=4 < HEADER_SIZE=8
            peer.expect_bare_close()
        _assert_core_survives(core)


class FdSpam(IntegrationTest):
    """AC: "fd spam: connection killed, core alive".

    Both variants are transport-level (`vitrin-ipc`), like the undersized
    header above: an fd-passing violation is judged from the header byte
    and the kernel's ancillary-data delivery alone, with no message
    decoded yet to name in an `error` event.
    """

    def test_header_declaring_two_fds_is_dropped(self):
        """Protocol v0 allows at most one fd per message (`fd_count` is 0 or
        1, the P1.1.2 codegen constraint); a header claiming 2 is refused on
        the header byte alone, before any fds are even matched.
        """
        core = self.core()
        with RawPeer(core.socket) as peer:
            peer.send(_raw_frame(HANDSHAKE_ID, 0, fd_count=2))
            peer.expect_bare_close()
        _assert_core_survives(core)

    def test_fd_flood_exceeds_the_unclaimed_cap_and_is_dropped(self):
        """More fds arrive than any frame has claimed, well past
        `MAX_UNCLAIMED_FDS` (8) -- the fd-bomb `vitrin-ipc`'s own docs name
        directly. Sent as a single `sendmsg` alongside one padding byte
        (some `AF_UNIX` stacks are fussy about a zero-length regular
        payload riding with `SCM_RIGHTS`); the payload byte is far short of
        a complete header, so no frame is ever in play to claim any of them.
        """
        core = self.core()
        devnull_fds = [os.open("/dev/null", os.O_RDONLY) for _ in range(16)]
        try:
            with RawPeer(core.socket) as peer:
                peer.send(b"\x00", fds=devnull_fds)
                peer.expect_bare_close()
        finally:
            for fd in devnull_fds:
                try:
                    os.close(fd)
                except OSError:
                    pass
        _assert_core_survives(core)


class SlowLorisFramingDoesNotStallOtherClients(IntegrationTest):
    """AC: "...frame timing unaffected" -- the one criterion the other
    classes above cannot exercise, because they all resolve (with an error
    or a bare close) in one round trip. A slow-loris peer resolves in NO
    round trips: it opens a connection and a frame, then simply never
    finishes sending it. Per P1.2.3 the core must go on servicing every
    OTHER connection regardless -- proven the same way `test_runtime_wiring
    .EndToEnd` proves an animating realm is live: two captures a fraction of
    a second apart must differ.
    """

    def test_a_stalled_partial_frame_does_not_stall_a_live_agent(self):
        core = self.core()
        conn = core.connect()
        grant = whole_realm_grant(conn)
        first = capture_when_ready(grant)

        with RawPeer(core.socket) as hostile:
            # The first 4 of `hello`'s ~20+ required bytes: enough for the
            # transport to know a frame is coming, never enough to complete
            # one. This socket is simply abandoned mid-frame for the rest of
            # the test -- exactly what a stalled/hostile peer looks like on
            # the wire, and never a valid EOF the transport could resolve
            # early.
            complete_hello = messages.encode_hello(
                version=protocol.PROTOCOL_VERSION,
                principal_id=2,
                identity=DEMO_IDENTITY,
                credential_type="static-token",
                credential=DEMO_TOKEN,
            )
            hostile.send(complete_hello[:4])

            # Long enough for the mock shim (which animates independently of
            # any observer, see harness.ANIMATE_FRAMES) to have painted a
            # different frame; short enough not to make this suite slow.
            time.sleep(0.6)
            second = grant.observe()
            self.assertNotEqual(
                first.raw,
                second.raw,
                "a live connection must keep receiving fresh frames while an "
                "unrelated peer sits with an incomplete frame in flight -- "
                "the compositor loop must never block on it",
            )
            self.assertIsNone(
                core.proc.poll(), "the core must stay up throughout the stall"
            )

        conn.close()


if __name__ == "__main__":
    unittest.main()
