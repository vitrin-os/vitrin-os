"""Grant petition, sync barrier, and actuation flows against the mock core."""

from __future__ import annotations

import pytest

import flows
import vectors
from vitrin_os import (
    ConsentState,
    GrantDenied,
    Persistence,
    RateLimited,
    Revoked,
    Verb,
    connect,
)
from vitrin_os.messages import encode_button, encode_move, encode_sync, encode_type


def _connect(server):
    return connect(
        server.path,
        identity=flows.IDENTITY,
        credential_type=flows.CREDENTIAL_TYPE,
        credential=flows.CREDENTIAL,
        timeout=5.0,
    )


def test_request_grant_granted_flow(server) -> None:
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame()),
            ("send", flows.consent_state_frame(1)),  # shown
            (
                "send",
                flows.resolved_frame(outcome=0, verbs=flows.ALL_VERBS, persistence=0),
            ),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant()
    assert not grant.resolved
    grant.await_consent()
    assert grant.effective_verbs == Verb.OBSERVE | Verb.ACTUATE_POINTER | Verb.ACTUATE_TEXT
    assert grant.effective_persistence == Persistence.ONCE
    assert grant.consent_state == ConsentState.SHOWN
    conn.close()


def test_request_grant_denied_raises_typed(server) -> None:
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame()),
            ("send", flows.resolved_frame(outcome=1)),  # denied
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant()
    with pytest.raises(GrantDenied) as excinfo:
        grant.await_consent()
    assert excinfo.value.outcome == 1
    # A denial is an answer, not a protocol violation: the connection lives.
    assert not conn.closed
    conn.close()


def test_sync_done_barrier_round_trip(server) -> None:
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", encode_sync(1)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    conn.sync()  # returns only once done(cookie) arrived
    conn.close()


def test_actuation_refusal_surfaces_via_barrier(server) -> None:
    """The actuate_and_flush idiom: refusal discovery costs one round trip."""
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_move(flows.POINTER_ID, x=10, y=20)),
            ("expect", encode_sync(1)),
            ("send", flows.refused_frame(verb=2, code=3, retry_after_ms=1500)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(RateLimited) as excinfo:
        grant.pointer.move(10, 20)
    assert excinfo.value.retry_after_ms == 1500
    assert excinfo.value.verb == 2
    assert not conn.closed  # recoverable: the connection lives
    conn.close()


def test_text_type_revoked_is_typed(server) -> None:
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_type(flows.TEXT_ID, text="hi")),
            ("expect", encode_sync(1)),
            ("send", flows.refused_frame(verb=4, code=2)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(Revoked):
        grant.text.type("hi")
    conn.close()


def test_click_is_move_press_release_one_barrier(server) -> None:
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_move(flows.POINTER_ID, x=5, y=6)),
            ("expect", encode_button(flows.POINTER_ID, button=0x110, state=1)),
            ("expect", encode_button(flows.POINTER_ID, button=0x110, state=0)),
            ("expect", encode_sync(1)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    grant.pointer.click(5, 6)
    conn.close()


def test_events_to_unknown_objects_are_tolerated(server) -> None:
    """Clients MUST tolerate and discard events referencing unknown/dead ids."""
    stray = vectors.frame(1000, 0, vectors.u32(0))
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", encode_sync(1)),
            ("send", stray),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    conn.sync()  # the stray event is discarded, the barrier completes
    conn.close()


def test_second_petition_ids_keep_increasing(server) -> None:
    """The watermark never rewinds: petition two allocates ids 9..13."""
    server.run(
        [
            *flows.granted_steps(),
            ("expect", flows.request_grant_frame(ids=(9, 10, 11, 12, 13))),
            ("send", flows.resolved_frame(outcome=1, grant_id=9)),
        ]
    )
    conn = _connect(server)
    conn.request_grant().await_consent()
    second = conn.request_grant()  # realm handle is cached: no second get_realm
    with pytest.raises(GrantDenied):
        second.await_consent()
    conn.close()
