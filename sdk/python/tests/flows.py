# SPDX-License-Identifier: Apache-2.0
"""Shared script fragments for the mocked-server flow tests.

Object ids follow from the SDK's watermark allocator (strictly increasing
from 2): principal=2, realm=3, then the five co-minted petition ids 4..8 in
argument order (grant, consent, view, pointer, text) — asserting these
exact bytes is what pins the allocator's watermark behavior on the wire.
"""

from __future__ import annotations

from vectors import frame, string, u32

from vitrin_os import protocol

IDENTITY = "vitrin://local/agent/demo"
# Deliberately different from the claimed identity: bound carries the
# verifier-canonical string, never an echo, and tests assert the canonical
# one wins.
CANONICAL_IDENTITY = "vitrin://local/agent/demo-canonical"
CREDENTIAL_TYPE = "static-token"
CREDENTIAL = "demo-secret"

PRINCIPAL_ID = 2
REALM_ID = 3
GRANT_ID = 4
CONSENT_ID = 5
VIEW_ID = 6
POINTER_ID = 7
TEXT_ID = 8

ALL_VERBS = 7  # observe | actuate_pointer | actuate_text
# The two layout facets are minted on demand, after the five co-minted ids,
# so they take 9 and 10 in the order the client first asks for them.
LAYOUT_FOCUS_ID = 9
LAYOUT_ARRANGE_ID = 10
#: The launch facet, likewise minted on demand. Tests that mint only this
#: one get 9, because ids are allocated in the order a client first asks.
LAUNCHER_ID = 9


def hello_frame(version: int = protocol.PROTOCOL_VERSION) -> bytes:
    """The bytes the SDK's own ``hello`` produces.

    The version defaults to the SDK's ``PROTOCOL_VERSION`` rather than a
    literal: this fixture asserts what the client sends, and the client
    sends its maximum. Pass an explicit integer where the *point* of the
    test is a specific offered version.
    """
    return frame(
        1,
        0,
        u32(version)
        + u32(PRINCIPAL_ID)
        + string(IDENTITY)
        + string(CREDENTIAL_TYPE)
        + string(CREDENTIAL),
    )


def bound_frame(identity: str = CANONICAL_IDENTITY) -> bytes:
    return frame(PRINCIPAL_ID, 0, string(identity))


def error_frame(object_id: int, code: int, message: str) -> bytes:
    return frame(1, 0, u32(object_id) + u32(code) + string(message))


def done_frame(cookie: int) -> bytes:
    return frame(1, 1, u32(cookie))


def get_realm_frame(name: str = "realm-0") -> bytes:
    return frame(PRINCIPAL_ID, 0, u32(REALM_ID) + string(name))


def request_grant_frame(
    *,
    resource: str | None = None,
    verbs: int = ALL_VERBS,
    expiry_ms: int = 0,
    max_event_rate: int = 0,
    persistence: int = 0,
    flags: int = 0,
    ids: tuple[int, int, int, int, int] = (GRANT_ID, CONSENT_ID, VIEW_ID, POINTER_ID, TEXT_ID),
) -> bytes:
    payload = b"".join(u32(i) for i in ids)
    payload += string(resource or "")  # null and empty encode identically
    payload += u32(verbs) + u32(expiry_ms) + u32(max_event_rate)
    payload += u32(persistence) + u32(flags)
    return frame(REALM_ID, 0, payload)


def resolved_frame(
    *, outcome: int, verbs: int = 0, persistence: int = 0, expiry_ms: int = 0, grant_id: int = GRANT_ID
) -> bytes:
    return frame(grant_id, 0, u32(outcome) + u32(verbs) + u32(persistence) + u32(expiry_ms))


def refused_frame(*, verb: int, code: int, retry_after_ms: int = 0) -> bytes:
    return frame(GRANT_ID, 1, u32(verb) + u32(code) + u32(retry_after_ms))


def consent_state_frame(state: int) -> bytes:
    return frame(CONSENT_ID, 0, u32(state))


def frame_ready_frame(*, format: int = 0x34325258, width: int, height: int, stride: int, flags: int = 0) -> bytes:
    return frame(
        VIEW_ID,
        0,
        u32(format) + u32(width) + u32(height) + u32(stride) + u32(flags),
        fd_count=1,
    )


def capture_frame_frame() -> bytes:
    return frame(VIEW_ID, 0)


def get_launcher_frame(facet_id: int = LAUNCHER_ID) -> bytes:
    """`vitrin_grant.get_launcher` — request opcode 0, since version 2."""
    return frame(GRANT_ID, 0, u32(facet_id))


def launch_frame(facet_id: int = LAUNCHER_ID) -> bytes:
    """`vitrin_launcher.launch` — no arguments, and that is the point."""
    return frame(facet_id, 0)


def launched_frame(realm: str, *, facet_id: int = LAUNCHER_ID) -> bytes:
    """`vitrin_launcher.launched` — the terminal of one launch."""
    return frame(facet_id, 0, string(realm))


def get_layout_focus_frame(facet_id: int = LAYOUT_FOCUS_ID) -> bytes:
    """`vitrin_grant.get_layout_focus` — request opcode 1, since version 2."""
    return frame(GRANT_ID, 1, u32(facet_id))


def get_layout_arrange_frame(facet_id: int = LAYOUT_ARRANGE_ID) -> bytes:
    """`vitrin_grant.get_layout_arrange` — request opcode 2, since version 2."""
    return frame(GRANT_ID, 2, u32(facet_id))


def focus_frame(facet_id: int = LAYOUT_FOCUS_ID) -> bytes:
    """`vitrin_layout_focus.focus` — no arguments, deliberately."""
    return frame(facet_id, 0)


def set_fullscreen_frame(*, mode: int, facet_id: int = LAYOUT_ARRANGE_ID) -> bytes:
    return frame(facet_id, 0, u32(mode))


def sync_frame(cookie: int) -> bytes:
    return frame(1, 1, u32(cookie))


# Script fragments -----------------------------------------------------------


def handshake_steps() -> list[tuple]:
    return [("expect", hello_frame()), ("send", bound_frame())]


def granted_steps(*, effective_verbs: int = ALL_VERBS) -> list[tuple]:
    """Handshake + realm-0 petition, auto-approved with no consent events."""
    return [
        *handshake_steps(),
        ("expect", get_realm_frame()),
        ("expect", request_grant_frame()),
        ("send", resolved_frame(outcome=0, verbs=effective_verbs)),
    ]
