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
                             get_layout_arrange=2, get_powerbox=3,
                             get_egress=4           (all since=2)
    vitrin_view:             capture_frame=0
    vitrin_actuator_pointer: move=0, button=1, scroll=2
    vitrin_actuator_text:    type=0
    vitrin_launcher:         launch=0               (since=2)
    vitrin_layout_focus:     focus=0                (since=2)
    vitrin_layout_arrange:   set_fullscreen=0       (since=2)
    vitrin_powerbox:         request_file=0, request_dir=1
                                                    (both since=2)
    vitrin_egress:           request_connect=0      (since=2)

NOT ENCODED HERE, and named rather than left as a gap in the table above:
`vitrin_grant.get_egress` and everything on `vitrin_egress`. The mint opcode is
listed because it is principal-facing and this module's tables claim to be that
half of the IDL, but no codec below emits it: `egress` is outside every
deployment's served set — what is missing is the out-of-core mediating proxy a
connection would be made through (P2.7.3) — so an SDK call that minted the
facet could only ever reach a refusal. Adding the codecs belongs to that task,
alongside the mechanism that makes an answer possible. That facet's own
messages are left out of both tables entirely rather than listed and disowned,
because it is not reachable without its mint.

`vitrin_grant.get_powerbox` and `vitrin_powerbox` were in that paragraph too,
on the same argument, until P2.6.6 landed the core-drawn picker and the
reference core moved `designate_file` into its served set. The argument is
unchanged and it stopped applying: a deployment now exists that can answer an
ask with a descriptor, so an SDK that cannot make one is what would be missing.
What did NOT change is that `designate_file` is a **deployment property** — a
deployment with no picker still refuses it — which is why encoding these five
messages (the mint, the two asks, the two terminals) is not a claim that any
particular core serves the verb.

Event opcodes (document order):
    vitrin_handshake:        error=0, done=1
    vitrin_principal:        bound=0, attention=1        (attention since=2)
    vitrin_grant:            resolved=0, refused=1
    vitrin_consent:          state=0
    vitrin_view:             frame_ready=0 (fd_count=1)
    vitrin_launcher:         launched=0                  (since=2)
    vitrin_powerbox:         designated=0 (fd_count=1), refused=1
                                                        (both since=2)
    vitrin_egress:           connected=0 (fd_count=1), connect_failed=1
                                                        (both since=2)
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
# Four of vitrin_grant's five since="2" structural mints, in document order.
# The fifth is `get_egress` (opcode 4), listed in the table above and
# deliberately not encoded here -- see the note under it.
OP_GET_LAUNCHER = 0
OP_GET_LAYOUT_FOCUS = 1
OP_GET_LAYOUT_ARRANGE = 2
OP_GET_POWERBOX = 3
OP_LAUNCH = 0
OP_FOCUS = 0
OP_SET_FULLSCREEN = 0
OP_REQUEST_FILE = 0
OP_REQUEST_DIR = 1


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


def encode_get_powerbox(grant_oid: int, *, facet_id: int) -> bytes:
    """`vitrin_grant.get_powerbox` — the fourth since=2 structural mint.

    Neither reply-bearing nor refusable, like its three siblings: it allocates
    the powerbox facet and nothing else. A grant that does not hold
    `designate_file` still mints fine and refuses on first *use*, because
    refusing the mint would turn it into an oracle for what the grant holds.
    """
    return MessageEncoder().put_new_id(facet_id).finish(grant_oid, OP_GET_POWERBOX)


def encode_request_file(facet_oid: int, *, mode: int) -> bytes:
    """`vitrin_powerbox.request_file` — ask the human to designate one file.

    It names no file, and cannot. The whole security property of this facet is
    that the path never crosses the wire in either direction: the request
    raises the core-drawn picker, the human chooses in front of the trusted
    indicator, and what comes back is a descriptor. There is deliberately no
    filter, suggestion, hint or starting-directory argument either — each would
    be an agent-supplied string steering what the human sees in a window the
    human is meant to trust, and a signature is immutable forever.

    `mode` is :class:`~vitrin_os.protocol.DesignationMode`: 0 read, 1
    read_write. It selects the chrome the picker opens with (an open dialog, or
    one that also offers to create) and is a ceiling, never a promise — the
    human may narrow it, and `designated.mode` carries what was approved. An
    out-of-range value is fatal `invalid_argument` server-side (the enum
    argument is whole-value checked by the decoder), so it is rejected here
    rather than sent: a client-side bug must not cost the caller its
    connection.
    """
    if mode not in tuple(protocol.DesignationMode):
        raise ValueError(f"request_file mode {mode} is outside vitrin_powerbox.mode")
    return MessageEncoder().put_uint(int(mode)).finish(facet_oid, OP_REQUEST_FILE)


def encode_request_dir(facet_oid: int) -> bytes:
    """`vitrin_powerbox.request_dir` — ask for one directory subtree.

    No arguments at all, and the missing `mode` is argued rather than
    accidental: a subtree picker has one chrome, so a mode here would steer
    nothing and would put the widest ask this verb can make — read-write over a
    whole subtree — in the least visible place. The human's tick in the picker
    decides it and `designated.mode` carries the answer.

    A subtree arrives as ONE directory descriptor, never a batch: the receiver
    walks it with the kernel's own `openat`, which is what makes "subtree" a
    containment boundary the kernel enforces rather than a prefix match on
    strings this protocol never sees.
    """
    return MessageEncoder().finish(facet_oid, OP_REQUEST_DIR)


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
class DesignatedEvent:
    """vitrin_powerbox.designated — the descriptor the human designated.

    One of the two terminals of an admitted ask (the other is
    :class:`PowerboxRefusedEvent`), and the only message in this SDK that hands
    the caller a resource.

    **The descriptor is yours to close.** Ownership transfers on receipt: the
    core closes its own copy after sending, and nothing in this SDK will close
    ``fd`` for you — ``os.close(event.fd)`` when you are done with it. That is
    unlike every other value this SDK returns, and unlike :class:`Frame` in
    particular, which is materialized close-after-copy precisely so no caller
    owes anything. Here the descriptor *is* the payload, so it cannot be copied
    out and closed on your behalf.

    **``name`` is display-only and there is no path to be had.** It is the
    basename of what the human chose, carried so a client can say what it was
    given; it is not a path, it is not resolvable, and re-opening "the file
    called that" is not the same act as using this descriptor. The point of a
    powerbox is that you were handed the *thing* rather than a name to
    re-resolve — treating the name as a path reintroduces exactly the race the
    descriptor closed, in which the name comes to mean a different file between
    the human's confirmation and your open. (Withholding the path is not
    claimed as confidentiality: whoever holds the descriptor can read the path
    out of ``/proc/self/fd``. It is withheld so that no path is ever part of
    this interface's contract.)

    "Basename" is the *server's* contract and nothing here verifies it: the
    IDL requires the exact bytes of the human's choice, unchanged and
    unmarked, so this SDK passes them through and checks only the 255-byte
    bound. A name carrying a path separator, a newline or a control character
    would be a server contract violation this SDK does not detect — one more
    reason to render ``name`` and never resolve it.

    **Both copies share a file offset.** The same descriptor is delivered to
    the asking agent here and to the realm's shim as
    ``vitrin_shim_session.designation``; the core resolves the human's choice
    ONCE and sends that one descriptor twice, and ``SCM_RIGHTS`` installs in
    each receiver a descriptor referring to the **same open file description**
    — what ``dup(2)`` produces, not what a second ``open`` would. So a ``read``
    by either side advances the other's cursor, an ``lseek`` by either moves
    the other, and for a directory designation the shared position is the
    ``getdents`` cursor, so two receivers that both walk the subtree each see
    part of it and neither sees all of it. A receiver that must not be
    disturbed uses positional I/O (``os.pread``/``os.pwrite``, which never
    touch the shared offset) or, for a directory, opens a fresh description
    from the descriptor it holds (``os.open(".", O_RDONLY | O_DIRECTORY,
    dir_fd=fd)``), which stays inside the designated subtree. ``os.dup`` does
    **not** help — it makes another descriptor onto the very description being
    shared. This is inherent to resolving the human's choice once, not a defect
    to be fixed by opening twice: two opens are two resolutions, and between
    them the name could come to mean a different file.

    ``mode`` is the **effective** access the human approved, which may be
    narrower than the ask — a ``request_file(write=True)`` answered
    :attr:`~vitrin_os.protocol.DesignationMode.READ` is an approval, not a
    refusal. It describes what the core opened the descriptor with; it is not a
    promise about the file's permissions, which the kernel enforces and may
    change underneath any holder.

    ``designation_id`` is the core's own opaque id, unique for the life of the
    session, matching the journal record and the realm's copy of this
    designation. ``kind`` says whether the descriptor is a file or a directory
    subtree.

    This descriptor **outlives the grant**. Revocation and expiry stop future
    asks and kill the grant row; they do not close what has already been
    delivered, on either connection. A file descriptor that has crossed a
    socket is kernel authority the core cannot recall.
    """

    fd: int
    designation_id: int
    kind: int
    mode: int
    name: str


@dataclass(frozen=True)
class PowerboxRefusedEvent:
    """vitrin_powerbox.refused — an admitted ask that produced no descriptor.

    The other terminal of an ask the chokepoint ALLOWED: the human declined,
    the card expired, the core would not designate what they chose, or no card
    could be raised at all. It is **not an exception** in this SDK and is
    returned, not raised — a human declining to hand over a file is the system
    working, and an agent that treated it as an error would be treating the
    human as a fault.

    Not a second enforcement voice either. Authority questions are answered by
    ``vitrin_grant.refused``, from the one chokepoint, for every verb; every
    ``code`` here (:class:`~vitrin_os.protocol.PowerboxRefusal`) is compatible
    with a perfectly live grant, and asking again later is legal, bounded by
    the same rate ceiling as any other use.
    """

    code: int


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
    | DesignatedEvent
    | PowerboxRefusedEvent
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


def _decode_designated(dec: MessageDecoder, fd: int | None) -> DesignatedEvent:
    assert fd is not None  # guaranteed by the fd_expected check in decode_event
    return DesignatedEvent(
        fd=fd,
        designation_id=dec.uint(),
        kind=dec.uint(),
        mode=dec.uint(),
        name=dec.string(max_bytes=protocol.MAX_DESIGNATION_NAME_BYTES),
    )


def _decode_powerbox_refused(dec: MessageDecoder, fd: int | None) -> PowerboxRefusedEvent:
    return PowerboxRefusedEvent(code=dec.uint())


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
    # `designated` is the second fd-bearing event this SDK decodes, and the
    # only one whose fd is handed on to the caller rather than consumed.
    "vitrin_powerbox": {
        0: (True, _decode_designated),
        1: (False, _decode_powerbox_refused),
    },
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
