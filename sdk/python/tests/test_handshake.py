"""Handshake tests against the scripted mock core (P1.1.3 semantics)."""

from __future__ import annotations

import pytest

import flows
from vitrin_os import (
    AuthFailed,
    ConnectionClosed,
    PreHandshake,
    VersionUnsupported,
    connect,
)
from vitrin_os.client import Connection
from vitrin_os.messages import encode_sync
from vitrin_os.transport import Transport


def _connect(server, **kwargs):
    return connect(
        server.path,
        identity=flows.IDENTITY,
        credential_type=flows.CREDENTIAL_TYPE,
        credential=flows.CREDENTIAL,
        timeout=5.0,
        **kwargs,
    )


def test_successful_handshake_binds_canonical_identity(server) -> None:
    server.run(flows.handshake_steps())
    conn = _connect(server)
    # bound carries the verifier-canonical identity, never an echo of the
    # claimed string.
    assert conn.identity == flows.CANONICAL_IDENTITY
    assert conn.identity != flows.IDENTITY
    assert conn.bound
    conn.close()


def test_version_mismatch_is_typed_and_fatal(server) -> None:
    server.run(
        [
            ("expect", flows.hello_frame(version=2)),
            ("send", flows.error_frame(1, 6, "unsupported protocol version")),
            ("close",),
        ]
    )
    with pytest.raises(VersionUnsupported) as excinfo:
        _connect(server, version=2)
    assert excinfo.value.code == 6
    assert excinfo.value.object_id == 1


def test_auth_failed_is_typed_uniform_and_fatal(server) -> None:
    fixed_phrase = "authentication failed"
    server.run(
        [
            ("expect", flows.hello_frame()),
            ("send", flows.error_frame(1, 7, fixed_phrase)),
            ("close",),
        ]
    )
    with pytest.raises(AuthFailed) as excinfo:
        _connect(server)
    assert excinfo.value.code == 7
    # The wire message is the fixed cause-free phrase; the claimed identity
    # is never echoed (identity-probing resistance).
    assert excinfo.value.wire_message == fixed_phrase
    assert flows.IDENTITY not in excinfo.value.wire_message


def test_pre_handshake_traffic_is_typed_and_fatal(server) -> None:
    """Traffic before a first hello dies fatal pre_handshake (typed)."""
    sync = encode_sync(7)
    server.run(
        [
            ("expect", sync),
            ("send", flows.error_frame(1, 5, "traffic before hello")),
            ("close",),
        ]
    )
    transport = Transport.connect_unix(server.path, timeout=5.0)
    conn = Connection(transport)
    conn._send(sync)  # a misbehaving client the SDK's public API forbids
    with pytest.raises(PreHandshake) as excinfo:
        conn._dispatch_one()
    assert excinfo.value.code == 5
    assert conn.closed  # the connection died with the fatal error


def test_hello_is_legal_exactly_once_client_side(server) -> None:
    server.run(flows.handshake_steps())
    conn = _connect(server)
    with pytest.raises(RuntimeError, match="exactly once"):
        conn.hello(identity=flows.IDENTITY)
    conn.close()


def test_eof_during_handshake_raises_connection_closed(server) -> None:
    server.run([("expect", flows.hello_frame()), ("close",)])
    with pytest.raises(ConnectionClosed):
        _connect(server)


def test_connect_timeout_governs_handshake_only(server) -> None:
    """The connect timeout is cleared once bound: steady-state blocking
    calls (await_consent waits on an unbounded human delay) never trip it."""
    server.run(flows.handshake_steps())
    conn = _connect(server)  # timeout=5.0 during connect + handshake
    assert conn._transport._sock.gettimeout() is None
    conn.close()


def test_handshake_timeout_is_typed_not_raw(server) -> None:
    """A server stalling mid-handshake surfaces a typed VitrinError, never
    a raw TimeoutError outside the hierarchy."""
    server.run([("expect", flows.hello_frame()), ("linger",)])
    with pytest.raises(ConnectionClosed, match="timed out"):
        connect(
            server.path,
            identity=flows.IDENTITY,
            credential_type=flows.CREDENTIAL_TYPE,
            credential=flows.CREDENTIAL,
            timeout=0.2,
        )
