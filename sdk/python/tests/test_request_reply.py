# SPDX-License-Identifier: Apache-2.0
"""Grant petition, sync barrier, and actuation flows against the mock core."""

from __future__ import annotations

import pytest

import flows
import vectors
from vitrin_os import (
    ConsentState,
    GrantDenied,
    LayoutHeld,
    Persistence,
    RateLimited,
    Revoked,
    ServerContractViolation,
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


def test_undefined_consent_state_is_contract_violation(server) -> None:
    """An enum value outside the negotiated version's set is a server
    contract violation (typed, connection closed), never a raw ValueError."""
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame()),
            ("send", flows.consent_state_frame(9)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant()
    with pytest.raises(ServerContractViolation, match="consent state"):
        grant.await_consent()
    assert conn.closed


def test_undefined_resolved_persistence_is_contract_violation(server) -> None:
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame()),
            ("send", flows.resolved_frame(outcome=0, verbs=flows.ALL_VERBS, persistence=9)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant()
    with pytest.raises(ServerContractViolation, match="undefined enum"):
        grant.await_consent()
    assert conn.closed


def test_undefined_refusal_code_is_contract_violation(server) -> None:
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_move(flows.POINTER_ID, x=1, y=2)),
            ("expect", encode_sync(1)),
            ("send", flows.refused_frame(verb=2, code=9)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(ServerContractViolation, match="refusal code"):
        grant.pointer.move(1, 2)
    assert conn.closed


def test_refused_barrier_consumes_done_no_cookie_leak(server) -> None:
    """A refused barrier still reads its done: no cookie state leaks, and
    the next barrier round-trips cleanly."""
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_move(flows.POINTER_ID, x=1, y=2)),
            ("expect", encode_sync(1)),
            ("send", flows.refused_frame(verb=2, code=2)),
            ("send", flows.done_frame(1)),
            ("expect", encode_type(flows.TEXT_ID, text="ok")),
            ("expect", encode_sync(2)),
            ("send", flows.done_frame(2)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    with pytest.raises(Revoked):
        grant.pointer.move(1, 2)
    grant.text.type("ok")  # the next barrier completes against fresh state
    assert conn._done_cookies == set()
    conn.close()


def test_flush_false_batch_surfaces_refusals_via_sync(server) -> None:
    """flush=False actuations + sync(grant): the barrier drains the whole
    refusal window — the oldest raises, the rest attach as notes."""
    server.run(
        [
            *flows.granted_steps(),
            ("expect", encode_move(flows.POINTER_ID, x=1, y=2)),
            ("expect", encode_type(flows.TEXT_ID, text="hi")),
            ("expect", encode_sync(1)),
            ("send", flows.refused_frame(verb=2, code=2)),
            ("send", flows.refused_frame(verb=4, code=2)),  # coalescing: one per (verb, code)
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant().await_consent()
    grant.pointer.move(1, 2, flush=False)
    grant.text.type("hi", flush=False)
    with pytest.raises(Revoked) as excinfo:
        conn.sync(grant)
    assert excinfo.value.verb == 2
    assert any("verb 4" in note for note in excinfo.value.__notes__)
    assert not grant._refusals  # the window is fully drained
    assert not conn.closed
    conn.close()


def test_encode_failure_burns_no_ids_and_registers_no_proxies(server) -> None:
    """A client-side encode ValueError must not advance the never-reusable
    watermark or leave dead proxies: the next petition still uses ids 4..8
    (asserted byte-exactly by the mock script)."""
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame()),
            ("send", flows.resolved_frame(outcome=0, verbs=flows.ALL_VERBS)),
        ]
    )
    conn = _connect(server)
    realm = conn.get_realm()
    objects_before = dict(conn._objects)
    with pytest.raises(ValueError, match="bound"):
        realm.request_grant(resource="x" * 257)  # over the 256-byte bound
    assert conn._objects == objects_before  # no dead proxies registered
    conn.request_grant().await_consent()
    conn.close()


def test_layout_held_outcome_decodes_and_keeps_the_connection(server) -> None:
    """Outcome 6 is a *recoverable admission answer*, not a dead socket.

    `layout_held` says another live grant already holds `layout_arrange` for
    this output. It is the whole reason the IDL stages an outcome ahead of a
    client knowing it: an SDK that did not carry it would decode 6 into
    `ServerContractViolation`, which **closes the connection** — turning the
    recoverable answer into the connection death the staging exists to
    prevent. So both halves are asserted: the typed exception, and the live
    socket after it.
    """
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame(verbs=int(Verb.LAYOUT_ARRANGE))),
            ("send", flows.resolved_frame(outcome=6)),
            # The connection is still usable afterwards, proved by a real
            # round trip rather than by `not conn.closed` alone.
            ("expect", encode_sync(1)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant(verbs=Verb.LAYOUT_ARRANGE)
    with pytest.raises(LayoutHeld) as excinfo:
        grant.await_consent()
    assert excinfo.value.outcome == 6
    assert not isinstance(excinfo.value, ServerContractViolation)
    assert not conn.closed
    conn.sync()
    conn.close()


def test_layout_requests_mint_their_facets_and_reach_the_wire(server) -> None:
    """`focus` and `set_fullscreen` are encoded exactly as the IDL defines them.

    Both facets are minted lazily and separately — `request_grant`'s five
    `new_id` arguments are frozen forever, so every facet added after version
    1 arrives as its own structural mint on the grant — and each mint is
    sent exactly once however many times the request is used.
    """
    layout_verbs = int(Verb.LAYOUT_ARRANGE | Verb.LAYOUT_FOCUS)
    server.run(
        [
            *flows.handshake_steps(),
            ("expect", flows.get_realm_frame()),
            ("expect", flows.request_grant_frame(verbs=layout_verbs)),
            ("send", flows.resolved_frame(outcome=0, verbs=layout_verbs)),
            ("expect", flows.get_layout_focus_frame()),
            ("expect", flows.focus_frame()),
            ("expect", flows.get_layout_arrange_frame()),
            ("expect", flows.set_fullscreen_frame(mode=1)),
            ("expect", flows.set_fullscreen_frame(mode=0)),
            # No second mint: the facet is remembered.
            ("expect", flows.focus_frame()),
            ("expect", encode_sync(1)),
            ("send", flows.done_frame(1)),
        ]
    )
    conn = _connect(server)
    grant = conn.request_grant(
        verbs=Verb.LAYOUT_ARRANGE | Verb.LAYOUT_FOCUS
    ).await_consent()
    grant.focus()
    grant.set_fullscreen(True)
    grant.set_fullscreen(False)
    grant.focus()
    conn.sync()
    conn.close()


def test_set_fullscreen_rejects_a_mode_outside_the_enum() -> None:
    """A bad `mode` is caught client-side, before it costs the connection.

    `vitrin_layout_arrange.mode` is a plain enum, so an out-of-range value is
    fatal `invalid_argument` server-side. A client-side bug must not be paid
    for with a dead socket, so the encoder refuses first.
    """
    from vitrin_os.messages import encode_set_fullscreen

    for mode in (2, -1, 255):
        with pytest.raises(ValueError, match="vitrin_layout_arrange.mode"):
            encode_set_fullscreen(flows.LAYOUT_ARRANGE_ID, mode=mode)


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
