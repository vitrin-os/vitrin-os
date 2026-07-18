"""SCM_RIGHTS fd receipt: a real sealed memfd arrives usable, CLOEXEC set."""

from __future__ import annotations

import fcntl
import os
import struct

import pytest

import flows
from vitrin_os import NoSurface, ServerContractViolation, connect
from vitrin_os.messages import encode_capture_frame
from vitrin_os.protocol import REQUIRED_FRAME_SEALS

WIDTH, HEIGHT = 2, 2
STRIDE = WIDTH * 4
# xrgb8888 little-endian: bytes [B, G, R, X] with the X channel 0xFF exactly.
PIXELS = bytes.fromhex("aa bb cc ff" * (WIDTH * HEIGHT))


def _make_memfd(data: bytes, *, seal: bool = True, truncate_to: int | None = None) -> int:
    fd = os.memfd_create("vitrin-test-frame", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    os.write(fd, data)
    if truncate_to is not None:
        os.ftruncate(fd, truncate_to)
    if seal:
        fcntl.fcntl(fd, fcntl.F_ADD_SEALS, REQUIRED_FRAME_SEALS)
    return fd


def _connect(server):
    return connect(
        server.path,
        identity=flows.IDENTITY,
        credential_type=flows.CREDENTIAL_TYPE,
        credential=flows.CREDENTIAL,
        timeout=5.0,
    )


def _observe_script(fd: int) -> list[tuple]:
    return [
        *flows.granted_steps(),
        ("expect", encode_capture_frame(flows.VIEW_ID)),
        (
            "send_fd",
            flows.frame_ready_frame(width=WIDTH, height=HEIGHT, stride=STRIDE),
            fd,
        ),
    ]


def test_observe_receives_usable_sealed_memfd_with_cloexec(server) -> None:
    server.run(_observe_script(_make_memfd(PIXELS)))
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with grant.observe() as frame:
        assert (frame.width, frame.height, frame.stride) == (WIDTH, HEIGHT, STRIDE)
        assert frame.size == STRIDE * HEIGHT
        # The memfd is usable: the bytes the server wrote come back.
        assert frame.read_bytes() == PIXELS
        # CLOEXEC is set atomically at receipt (MSG_CMSG_CLOEXEC).
        assert fcntl.fcntl(frame.fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
        # The seals are really there (client-provable immutability).
        seals = fcntl.fcntl(frame.fd, fcntl.F_GET_SEALS)
        assert seals & REQUIRED_FRAME_SEALS == REQUIRED_FRAME_SEALS
    assert frame.closed
    conn.close()


def test_unsealed_memfd_is_a_server_contract_violation(server) -> None:
    server.run(_observe_script(_make_memfd(PIXELS, seal=False)))
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(ServerContractViolation, match="seals"):
        grant.observe()
    assert conn.closed  # never attributed to the grant; connection sanctioned


def test_wrong_memfd_size_is_a_server_contract_violation(server) -> None:
    # Sized stride*height + 1: violates the exact-size rule.
    fd = _make_memfd(PIXELS + b"\x00", seal=True)
    server.run(_observe_script(fd))
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(ServerContractViolation, match="size"):
        grant.observe()
    assert conn.closed


def test_unsolicited_fd_is_a_server_contract_violation(server) -> None:
    """An fd attached to a frame declaring fd_count == 0 is the client-side
    equivalent of the fd_violation condition (conventions section 2.4): it
    must never sit queued to mispair with a later fd-declaring frame."""
    fd = os.open(os.devnull, os.O_RDONLY)
    server.run(
        [
            ("expect", flows.hello_frame()),
            ("send_fd", flows.bound_frame(), fd),  # bound declares no fd
        ]
    )
    with pytest.raises(ServerContractViolation, match="unsolicited"):
        _connect(server)


def test_fd_flood_is_bounded(server) -> None:
    """Unclaimed fds cannot accumulate without bound (fd-bomb confinement)."""
    # A header declaring a 4096-byte frame that never completes, dribbled
    # one fd-bearing byte at a time: fds queue up, none is ever claimed.
    steps: list[tuple] = [
        ("expect", flows.hello_frame()),
        ("send", struct.pack("<IHBB", flows.PRINCIPAL_ID, 4096, 0, 0)),
    ]
    for _ in range(17):
        steps.append(("send_fd", b"x", os.open(os.devnull, os.O_RDONLY)))
    server.run(steps)
    with pytest.raises(ServerContractViolation, match="unclaimed"):
        _connect(server)


def test_refused_capture_is_typed_not_a_frame(server) -> None:
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_capture_frame(flows.VIEW_ID)),
            ("send", flows.refused_frame(verb=1, code=6)),  # no_surface
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(NoSurface) as excinfo:
        grant.observe()
    assert excinfo.value.verb == 1
    assert not conn.closed  # recoverable
    conn.close()
