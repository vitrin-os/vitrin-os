# SPDX-License-Identifier: Apache-2.0
"""Per-message codecs for the principal-facing half of the protocol.

Opcodes are implicit document order in protocol/vitrin-v0.xml, requests and
events numbered separately from 0 (conventions section 7.4). The client
encodes requests and decodes events; the shim-facing interfaces
(vitrin_shim_*) are the other connection class and are deliberately absent.

Request opcodes (document order):
    vitrin_handshake:        hello=0, sync=1
    vitrin_principal:        get_realm=0
    vitrin_realm:            request_grant=0
    vitrin_grant:            get_launcher=0, get_layout_focus=1,
                             get_layout_arrange=2   (all since=2)
    vitrin_view:             capture_frame=0
    vitrin_actuator_pointer: move=0, button=1, scroll=2
    vitrin_actuator_text:    type=0
    vitrin_launcher:         launch=0               (since=2)
    vitrin_layout_focus:     focus=0                (since=2)
    vitrin_layout_arrange:   set_fullscreen=0       (since=2)

Event opcodes (document order):
    vitrin_handshake:        error=0, done=1
    vitrin_principal:        bound=0, attention=1        (attention since=2)
    vitrin_grant:            resolved=0, refused=1
    vitrin_consent:          state=0
    vitrin_view:             frame_ready=0 (fd_count=1)
    vitrin_launcher:         launched=0                  (since=2)
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable

from . import protocol
from .errors import ServerContractViolation
from .wire import MessageDecoder, MessageEncoder

# ---------------------------------------------------------------------------
# Request encoders (client -> server). Each returns complete frame bytes.
# ---------------------------------------------------------------------------

OP_HELLO = 0
OP_SYNC = 1
OP_GET_REALM = 0
OP_REQUEST_GRANT = 0
OP_CAPTURE_FRAME = 0
OP_MOVE = 0
OP_BUTTON = 1
OP_SCROLL = 2
OP_TYPE = 0
# vitrin_grant's three since="2" structural mints, in document order.
OP_GET_LAUNCHER = 0
OP_GET_LAYOUT_FOCUS = 1
OP_GET_LAYOUT_ARRANGE = 2
OP_LAUNCH = 0
OP_FOCUS = 0
OP_SET_FULLSCREEN = 0


def encode_hello(
    *,
    version: int,
    principal_id: int,
    identity: str,
    credential_type: str,
    credential: str,
) -> bytes:
    enc = MessageEncoder()
    enc.put_uint(version)
    enc.put_new_id(principal_id)
    enc.put_string(identity, max_bytes=protocol.MAX_IDENTITY_BYTES)
    enc.put_string(credential_type, max_bytes=protocol.MAX_CREDENTIAL_TYPE_BYTES)
    enc.put_string(credential, max_bytes=protocol.MAX_CREDENTIAL_BYTES)
    return enc.finish(protocol.BOOTSTRAP_OBJECT_ID, OP_HELLO)


def encode_sync(cookie: int) -> bytes:
    return MessageEncoder().put_uint(cookie).finish(protocol.BOOTSTRAP_OBJECT_ID, OP_SYNC)


def encode_get_realm(principal_oid: int, *, realm_id: int, name: str) -> bytes:
    enc = MessageEncoder()
    enc.put_new_id(realm_id)
    enc.put_string(name, max_bytes=protocol.MAX_REALM_NAME_BYTES)
    return enc.finish(principal_oid, OP_GET_REALM)


def encode_request_grant(
    realm_oid: int,
    *,
    grant_id: int,
    consent_id: int,
    view_id: int,
    pointer_id: int,
    text_id: int,
    resource: str | None,
    verbs: int,
    expiry_ms: int,
    max_event_rate: int,
    persistence: int,
    flags: int,
) -> bytes:
    if verbs == 0:
        # Zero verbs in a petition is fatal invalid_argument server-side; a
        # correct client never sends it (conventions section 5.2).
        raise ValueError("a petition's verb set MUST be non-zero")
    enc = MessageEncoder()
    enc.put_new_id(grant_id)
    enc.put_new_id(consent_id)
    enc.put_new_id(view_id)
    enc.put_new_id(pointer_id)
    enc.put_new_id(text_id)
    enc.put_string(resource, max_bytes=protocol.MAX_RESOURCE_BYTES, allow_null=True)
    enc.put_uint(verbs)
    enc.put_uint(expiry_ms)
    enc.put_uint(max_event_rate)
    enc.put_uint(persistence)
    enc.put_uint(flags)
    return enc.finish(realm_oid, OP_REQUEST_GRANT)


def encode_capture_frame(view_oid: int) -> bytes:
    # The request deliberately carries no arguments.
    return MessageEncoder().finish(view_oid, OP_CAPTURE_FRAME)


def encode_get_launcher(grant_oid: int, *, facet_id: int) -> bytes:
    """`vitrin_grant.get_launcher` — a since=2 structural mint.

    Neither reply-bearing nor refusable, like its two layout siblings: it
    allocates the launch facet and nothing else. A grant that does not hold
    `realm.launch` still mints fine and refuses on first use, because
    refusing the mint would turn it into an oracle for what the grant holds.
    """
    return MessageEncoder().put_new_id(facet_id).finish(grant_oid, OP_GET_LAUNCHER)


def encode_launch(facet_oid: int) -> bytes:
    """`vitrin_launcher.launch` — no arguments, and that is the security
    property rather than an economy.

    The realm the grant was petitioned over names a **template**, and the
    template names the program. A command string off the wire would hand the
    choice of what the trusted core executes to any principal holding one
    grant. Selecting *which* program to run is done by petitioning over a
    different realm, at consent time, in front of the human.
    """
    return MessageEncoder().finish(facet_oid, OP_LAUNCH)


def encode_get_layout_focus(grant_oid: int, *, facet_id: int) -> bytes:
    """`vitrin_grant.get_layout_focus` — a since=2 structural mint.

    Neither reply-bearing nor refusable: it allocates the facet and nothing
    else, so there is no terminal to wait for. A grant that does not hold
    `layout.focus` still mints fine and refuses on first use.
    """
    return MessageEncoder().put_new_id(facet_id).finish(grant_oid, OP_GET_LAYOUT_FOCUS)


def encode_get_layout_arrange(grant_oid: int, *, facet_id: int) -> bytes:
    """`vitrin_grant.get_layout_arrange` — the sibling mint.

    A separate request rather than one `get_layout`, because a facet
    interface declares exactly one grant verb and the two layout verbs stay
    independently attenuable.
    """
    return MessageEncoder().put_new_id(facet_id).finish(grant_oid, OP_GET_LAYOUT_ARRANGE)


def encode_focus(facet_oid: int) -> bytes:
    """`vitrin_layout_focus.focus` — no arguments, deliberately.

    The realm the grant was petitioned over is the realm this focuses, so a
    holder can only ever move the output to a realm the human saw on the
    consent prompt.
    """
    return MessageEncoder().finish(facet_oid, OP_FOCUS)


def encode_set_fullscreen(facet_oid: int, *, mode: int) -> bytes:
    """`vitrin_layout_arrange.set_fullscreen`.

    `mode` is :class:`~vitrin_os.protocol.LayoutMode`: 0 windowed, 1
    fullscreen. An out-of-range value is fatal `invalid_argument` server-side
    (plain enums decode by whole-value membership), so it is rejected here
    rather than sent — a client-side bug must not cost the caller its
    connection.
    """
    if mode not in tuple(protocol.LayoutMode):
        raise ValueError(f"set_fullscreen mode {mode} is outside vitrin_layout_arrange.mode")
    return MessageEncoder().put_uint(int(mode)).finish(facet_oid, OP_SET_FULLSCREEN)


def encode_move(pointer_oid: int, *, x: int, y: int) -> bytes:
    return MessageEncoder().put_int(x).put_int(y).finish(pointer_oid, OP_MOVE)


def encode_button(pointer_oid: int, *, button: int, state: int) -> bytes:
    return MessageEncoder().put_uint(button).put_uint(state).finish(pointer_oid, OP_BUTTON)


def encode_scroll(pointer_oid: int, *, axis: int, value120: int) -> bytes:
    return MessageEncoder().put_uint(axis).put_int(value120).finish(pointer_oid, OP_SCROLL)


def _validate_type_text(text: str) -> None:
    """Enforce the vitrin_actuator_text.type control-character rule.

    Newline (U+000A) and tab (U+0009) are legal (rendered as Return / Tab
    by the delivery path); all other C0 and C1 control characters are fatal
    invalid_argument server-side, so a correct client never emits them.
    """
    for ch in text:
        code = ord(ch)
        if (code < 0x20 and ch not in ("\n", "\t")) or code == 0x7F or 0x80 <= code <= 0x9F:
            raise ValueError(
                f"control character U+{code:04X} is forbidden in type() "
                "(only newline and tab are legal)"
            )


def encode_type(text_oid: int, *, text: str) -> bytes:
    _validate_type_text(text)
    enc = MessageEncoder()
    enc.put_string(text, max_bytes=protocol.MAX_TEXT_BYTES)
    return enc.finish(text_oid, OP_TYPE)


# ---------------------------------------------------------------------------
# Event decoders (server -> client).
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ErrorEvent:
    """vitrin_handshake.error — fatal; the connection closes after it."""

    object_id: int
    code: int
    message: str


@dataclass(frozen=True)
class DoneEvent:
    """vitrin_handshake.done — the sync barrier reply."""

    cookie: int


@dataclass(frozen=True)
class BoundEvent:
    """vitrin_principal.bound — handshake succeeded."""

    identity: str


@dataclass(frozen=True)
class AttentionEvent:
    """vitrin_principal.attention — the human pressed the compositor's own
    attention key (since version 2).

    Carries no arguments and confers **nothing**. It says the human made a
    statement about their own input state ("my hand is off this app"), which
    for a short server-chosen window stops the server refusing this
    principal's ``layout.focus``/``layout.arrange`` uses ``preempted``. It is
    not a confirmation, not a consent decision, and delegates no authority:
    everything the client may do afterwards it could already do.

    Only principals holding a live grant carrying a layout verb receive it.
    Receiving it is not a promise the window is yours — any recipient may use
    it and the first admitted use consumes it — so the honest response is to
    send an already-staged request immediately and surface the ``Preempted``
    refusal if you lost the race.
    """


@dataclass(frozen=True)
class ResolvedEvent:
    """vitrin_grant.resolved — the petition's terminal outcome."""

    outcome: int
    verbs: int
    persistence: int
    expiry_ms: int


@dataclass(frozen=True)
class RefusedEvent:
    """vitrin_grant.refused — one refused use of the grant."""

    verb: int
    code: int
    retry_after_ms: int


@dataclass(frozen=True)
class LaunchedEvent:
    """vitrin_launcher.launched — the realm one `launch` created.

    ``realm`` is minted by the core, unique for the life of the session, and
    usable as :meth:`Connection.get_realm`'s ``name``. **Treat it as
    opaque**: its internal shape is the server's business, not something a
    client may parse or predict.

    Launching confers nothing over what was launched — observing or
    actuating the new realm is a separate petition, seen by the human
    separately.
    """

    realm: str


@dataclass(frozen=True)
class ConsentStateEvent:
    """vitrin_consent.state — prompt lifecycle transition."""

    state: int


@dataclass(frozen=True)
class FrameReadyEvent:
    """vitrin_view.frame_ready — one captured frame; owns the received fd."""

    fd: int
    format: int
    width: int
    height: int
    stride: int
    flags: int


Event = (
    ErrorEvent
    | DoneEvent
    | BoundEvent
    | AttentionEvent
    | ResolvedEvent
    | RefusedEvent
    | ConsentStateEvent
    | FrameReadyEvent
    | LaunchedEvent
)


def _decode_error(dec: MessageDecoder, fd: int | None) -> ErrorEvent:
    return ErrorEvent(
        object_id=dec.uint(),
        code=dec.uint(),
        message=dec.string(max_bytes=protocol.MAX_ERROR_MESSAGE_BYTES),
    )


def _decode_done(dec: MessageDecoder, fd: int | None) -> DoneEvent:
    return DoneEvent(cookie=dec.uint())


def _decode_bound(dec: MessageDecoder, fd: int | None) -> BoundEvent:
    return BoundEvent(identity=dec.string(max_bytes=protocol.MAX_IDENTITY_BYTES))


def _decode_attention(dec: MessageDecoder, fd: int | None) -> AttentionEvent:
    return AttentionEvent()


def _decode_resolved(dec: MessageDecoder, fd: int | None) -> ResolvedEvent:
    return ResolvedEvent(
        outcome=dec.uint(),
        verbs=dec.uint(),
        persistence=dec.uint(),
        expiry_ms=dec.uint(),
    )


def _decode_refused(dec: MessageDecoder, fd: int | None) -> RefusedEvent:
    return RefusedEvent(verb=dec.uint(), code=dec.uint(), retry_after_ms=dec.uint())


def _decode_launched(dec: MessageDecoder, fd: int | None) -> LaunchedEvent:
    return LaunchedEvent(realm=dec.string(max_bytes=protocol.MAX_REALM_NAME_BYTES))


def _decode_consent_state(dec: MessageDecoder, fd: int | None) -> ConsentStateEvent:
    return ConsentStateEvent(state=dec.uint())


def _decode_frame_ready(dec: MessageDecoder, fd: int | None) -> FrameReadyEvent:
    assert fd is not None  # guaranteed by the fd_expected check in decode_event
    return FrameReadyEvent(
        fd=fd,
        format=dec.uint(),
        width=dec.uint(),
        height=dec.uint(),
        stride=dec.uint(),
        flags=dec.uint(),
    )


# interface name -> opcode -> (expects_fd, decoder)
_EVENT_DECODERS: dict[
    str, dict[int, tuple[bool, Callable[[MessageDecoder, int | None], Event]]]
] = {
    "vitrin_handshake": {0: (False, _decode_error), 1: (False, _decode_done)},
    "vitrin_principal": {
        0: (False, _decode_bound),
        1: (False, _decode_attention),
    },
    "vitrin_grant": {0: (False, _decode_resolved), 1: (False, _decode_refused)},
    "vitrin_consent": {0: (False, _decode_consent_state)},
    "vitrin_view": {0: (True, _decode_frame_ready)},
    "vitrin_launcher": {0: (False, _decode_launched)},
    # vitrin_realm, vitrin_actuator_pointer, and vitrin_actuator_text carry
    # no events in version 1.
    "vitrin_realm": {},
    "vitrin_actuator_pointer": {},
    "vitrin_actuator_text": {},
    # vitrin_layout_focus and vitrin_layout_arrange are deliberately
    # event-free: neither reports what it did (protocol/vitrin-v0.xml).
    "vitrin_layout_focus": {},
    "vitrin_layout_arrange": {},
}


def decode_event(interface: str, opcode: int, payload: bytes, fd: int | None) -> Event:
    """Decode one inbound event frame's payload into a typed event.

    Raises :class:`ServerContractViolation` on an opcode the interface does
    not define at version 1, an fd count that disagrees with the signature,
    or an argument decode failure. The caller owns closing ``fd`` on error.
    """
    table = _EVENT_DECODERS.get(interface)
    if table is None:
        raise ServerContractViolation(f"no event table for interface {interface!r}")
    entry = table.get(opcode)
    if entry is None:
        raise ServerContractViolation(
            f"server sent undefined event opcode {opcode} on {interface}"
        )
    expects_fd, decoder = entry
    if expects_fd != (fd is not None):
        raise ServerContractViolation(
            f"fd count disagrees with the signature of {interface} "
            f"event opcode {opcode}"
        )
    dec = MessageDecoder(payload)
    event = decoder(dec, fd)
    dec.finish()
    return event
