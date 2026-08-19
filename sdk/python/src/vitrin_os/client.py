# SPDX-License-Identifier: Apache-2.0
"""The blocking object API: connect, handshake, grants, facets, frames.

Threadless and blocking by design (conventions section 4: the single
ordered event stream is what makes "send, then read until the terminal
event" correct with no extra machinery). Every reply-bearing request blocks
until its exactly-one terminal event; fire-and-forget actuations are
followed by the sync/done barrier idiom (section 6.4) so failure discovery
costs one round trip and surfaces as a typed exception.

The public shape mirrors PRD Document 2 section 18:

    conn = connect(path, identity=..., credential_type=..., credential=...)
    grant = conn.request_grant(realm="realm-0", resource=None,
                               verbs=("observe", "actuate.pointer", "actuate.text"))
    grant.await_consent()
    frame = grant.observe()
    grant.pointer.click(x, y)
    grant.text.type("hello\\n")
"""

from __future__ import annotations

import fcntl
import os
from collections import deque
from typing import Callable, Iterable, NoReturn

from . import messages, png, protocol
from .errors import (
    ConnectionClosed,
    ObjectIdsExhausted,
    ServerContractViolation,
    fatal_error_by_code,
    refusal_error_by_code,
    resolution_error_by_outcome,
)
from .messages import (
    AttentionEvent,
    BoundEvent,
    ConsentStateEvent,
    DoneEvent,
    ErrorEvent,
    Event,
    FrameReadyEvent,
    LaunchedEvent,
    RefusedEvent,
    ResolvedEvent,
)
from .protocol import (
    BTN_LEFT,
    Axis,
    ButtonState,
    ConsentState,
    Format,
    Outcome,
    Persistence,
    Refusal,
    Verb,
)
from .transport import Transport

__all__ = ["connect", "Connection", "Realm", "Grant", "Frame"]


def _parse_verbs(verbs: int | Verb | Iterable[str | Verb]) -> int:
    """Accept a Verb bitmask or an iterable of SDK dotted names / Verb bits."""
    if isinstance(verbs, int):
        bits = int(verbs)
    else:
        bits = 0
        for verb in verbs:
            if isinstance(verb, str):
                try:
                    bits |= protocol.VERB_BY_DOTTED_NAME[verb]
                except KeyError:
                    raise ValueError(
                        f"unknown verb {verb!r}; expected one of "
                        f"{sorted(protocol.VERB_BY_DOTTED_NAME)}"
                    ) from None
            else:
                bits |= int(verb)
    if bits == 0:
        raise ValueError("a petition's verb set MUST be non-zero")
    if bits & ~protocol.VERB_MASK:
        # An out-of-range verb bit is fatal invalid_argument server-side.
        raise ValueError(f"verb bits {bits:#x} outside the IDL's verb bitfield")
    # Bits inside the mask a given deployment may not serve are NOT refused
    # here: the core answers them "unsupported" on the grant, which is
    # recoverable and is the answer the caller is entitled to see. Pre-empting
    # it locally would hide a deployment difference behind a client-side error.
    #
    # vitrin-verb-set: unserved-verbs = observe_cursor, egress
    #
    # Two of them -- observe.cursor and egress -- are refused by EVERY
    # deployment today (no cursor delivery; no facet at all), and realm.launch
    # and the layout.* pair by any deployment that declines them. The first
    # list is derived from the reference core by `cargo xtask verb-sets
    # --check`, so it cannot fall behind the way it did when `egress` landed;
    # the second is a deployment property and cannot be listed at all.
    return bits


class _Proxy:
    """Base of every wire object the client holds a handle to."""

    _interface: str

    def __init__(self, conn: "Connection", oid: int) -> None:
        self._conn = conn
        self.id = oid

    def _handle_event(self, event: Event) -> None:  # pragma: no cover - abstract
        raise NotImplementedError


class _HandshakeProxy(_Proxy):
    """Bootstrap object 1: the fatal-error channel and the sync barrier."""

    _interface = "vitrin_handshake"

    def _handle_event(self, event: Event) -> None:
        if isinstance(event, DoneEvent):
            self._conn._done_cookies.add(event.cookie)
        # ErrorEvent is intercepted in Connection._dispatch_one and never
        # reaches here.


class _PrincipalProxy(_Proxy):
    _interface = "vitrin_principal"

    def _handle_event(self, event: Event) -> None:
        if isinstance(event, BoundEvent):
            # bound carries the verifier-canonical identity, not an echo of
            # the claimed string.
            self._conn._bound_identity = event.identity
        elif isinstance(event, AttentionEvent):
            # The human pressed the compositor's attention key. It confers
            # nothing, so the SDK does nothing with it beyond counting it:
            # what a client should do is send the layout request it has
            # already staged and show the refusal if it lost the race. A
            # counter rather than a callback keeps this dependency-free and
            # keeps the SDK from implying the window is this client's.
            self._conn._attention_count += 1


class _ConsentProxy(_Proxy):
    _interface = "vitrin_consent"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant

    def _handle_event(self, event: Event) -> None:
        if isinstance(event, ConsentStateEvent):
            try:
                state = ConsentState(event.state)
            except ValueError:
                # A version-1 server never emits an undefined enum entry:
                # this is a server contract violation, never a ValueError
                # surfaced to the caller with the connection left open.
                self._conn._die_contract(
                    f"server sent undefined consent state {event.state}"
                )
            self._grant._consent_states.append(state)


class _ViewProxy(_Proxy):
    _interface = "vitrin_view"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant
        self._frames: deque[FrameReadyEvent] = deque()

    def _handle_event(self, event: Event) -> None:
        if isinstance(event, FrameReadyEvent):
            self._frames.append(event)


class PointerActuator(_Proxy):
    """The pointer facet: move, button, scroll (fire-and-forget + barrier)."""

    _interface = "vitrin_actuator_pointer"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant

    def _handle_event(self, event: Event) -> None:  # no events in version 1
        pass

    def move(self, x: int, y: int, *, flush: bool = True) -> None:
        self._conn._send(messages.encode_move(self.id, x=x, y=y))
        if flush:
            self._conn._barrier(self._grant)

    def button(self, button: int, state: ButtonState, *, flush: bool = True) -> None:
        self._conn._send(
            messages.encode_button(self.id, button=button, state=int(state))
        )
        if flush:
            self._conn._barrier(self._grant)

    def scroll(self, axis: Axis, value120: int, *, flush: bool = True) -> None:
        self._conn._send(
            messages.encode_scroll(self.id, axis=int(axis), value120=value120)
        )
        if flush:
            self._conn._barrier(self._grant)

    def click(self, x: int, y: int, button: int = BTN_LEFT) -> None:
        """Move + press + release, bounded by one sync barrier."""
        self.move(x, y, flush=False)
        self.button(button, ButtonState.PRESSED, flush=False)
        self.button(button, ButtonState.RELEASED, flush=False)
        self._conn._barrier(self._grant)


class TextActuator(_Proxy):
    """The text facet: deliver a Unicode string (fire-and-forget + barrier)."""

    _interface = "vitrin_actuator_text"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant

    def _handle_event(self, event: Event) -> None:  # no events in version 1
        pass

    def type(self, text: str, *, flush: bool = True) -> None:
        self._conn._send(messages.encode_type(self.id, text=text))
        if flush:
            self._conn._barrier(self._grant)


class Frame:
    """One captured frame: a plain immutable value object, fd-free.

    Memfd lifecycle (the P1.8.2 decision): **close-after-copy**.
    ``observe()`` verifies the frame_ready memfd contract, copies the whole
    buffer out, and closes the fd before this object exists — a ``Frame``
    never owns a file descriptor, so there is nothing to close, no
    context-manager obligation, and the no-fd-leak acceptance ("fd count
    flat over a capture loop") holds unconditionally rather than only for
    callers who remember to release frames. The alternative (an
    mmap-backed lazy view) would defer nothing under the poll model (D6):
    every consumer — ``raw``, ``to_png``, a digest — reads the whole
    fresh-per-capture buffer exactly once either way, while the mapping
    would add a lifetime to manage.

    ``raw`` is the wire buffer verbatim (``stride * height`` bytes of
    little-endian xrgb8888) — the IDL's observation-digest domain — so a
    digest over ``raw`` equals a digest over the memfd. Pixel addressing
    is stride-generic per the IDL ("pixels are addressed only through this
    event's arguments"): row ``r`` begins at byte offset ``r * stride``
    and carries ``width * 4`` payload bytes. Version 1 additionally pins
    ``stride == width * 4`` on the wire (enforced at receipt in
    :meth:`Connection._verify_frame`); the generic form here is the
    later-version seam. XRGB→RGB conversion deliberately never touches
    ``raw``: it happens only at the presentation boundary, inside
    :meth:`to_png` (see :mod:`vitrin_os.png`).
    """

    __slots__ = ("_raw", "format", "width", "height", "stride")

    def __init__(
        self, raw: bytes, *, format: Format, width: int, height: int, stride: int
    ) -> None:
        if width <= 0 or height <= 0:
            raise ValueError("frame dimensions must be positive")
        if stride < width * 4:
            raise ValueError(f"stride {stride} cannot hold {width} 4-byte pixels per row")
        if len(raw) != stride * height:
            raise ValueError(
                f"buffer holds {len(raw)} bytes, expected stride * height = {stride * height}"
            )
        self._raw = bytes(raw)
        self.format = format
        self.width = width
        self.height = height
        self.stride = stride

    @classmethod
    def _from_fd(
        cls, fd: int, *, format: Format, width: int, height: int, stride: int
    ) -> "Frame":
        """Materialize close-after-copy: read the whole buffer, close the fd.

        The fd is closed on every path — ownership transferred to us with
        ``frame_ready``, and nothing retains it past this call.
        """
        size = stride * height
        chunks: list[bytes] = []
        try:
            offset = 0
            while offset < size:
                chunk = os.pread(fd, size - offset, offset)
                if not chunk:
                    # Unreachable for a contract-verified memfd (exact fstat
                    # size, SHRINK seal); guards the direct-construction door.
                    raise OSError(f"frame buffer ended at byte {offset}, expected {size}")
                chunks.append(chunk)
                offset += len(chunk)
        finally:
            os.close(fd)
        return cls(b"".join(chunks), format=format, width=width, height=height, stride=stride)

    @property
    def raw(self) -> bytes:
        """The frame buffer exactly as the wire delivered it."""
        return self._raw

    @property
    def size(self) -> int:
        """Buffer size in bytes (``== stride * height == len(raw)``)."""
        return len(self._raw)

    def to_png(self, path: str | os.PathLike[str]) -> None:
        """Write the frame to ``path`` as a PNG.

        Pure stdlib and always available — the SDK never imports Pillow
        (see :mod:`vitrin_os.png` for the rationale and the determinism
        guarantees).
        """
        if self.format != Format.XRGB8888:
            raise ValueError(
                f"to_png supports xrgb8888 only, not {self.format!r} "
                "(the only format version-1 capture announces)"
            )
        data = png.encode_png(
            self._raw, width=self.width, height=self.height, stride=self.stride
        )
        with open(path, "wb") as out:
            out.write(data)

    def __repr__(self) -> str:
        return (
            f"<Frame {self.width}x{self.height} {self.format.name.lower()} "
            f"stride={self.stride}>"
        )


class _LauncherFacet(_Proxy):
    """`vitrin_launcher` — start the app the granted realm's template names.

    Reply-bearing, so this facet is the one that receives an event: exactly
    one :class:`~vitrin_os.messages.LaunchedEvent` per successful ``launch``,
    in request order. A refused launch arrives on the *grant* instead, as
    ``refused(realm.launch, …)``.
    """

    _interface = "vitrin_launcher"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant
        self._launched: deque[LaunchedEvent] = deque()

    def _handle_event(self, event: Event) -> None:
        if isinstance(event, LaunchedEvent):
            self._launched.append(event)


class _LayoutFocusFacet(_Proxy):
    """`vitrin_layout_focus` — bind the output to the granted realm.

    Events: none. This interface deliberately offers no read of which realm
    holds the output; a holder learns the effect of its own request through a
    capture it holds separate `observe` authority for.
    """

    _interface = "vitrin_layout_focus"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant


class _LayoutArrangeFacet(_Proxy):
    """`vitrin_layout_arrange` — fill the output, or keep the app's own size."""

    _interface = "vitrin_layout_arrange"

    def __init__(self, conn: "Connection", oid: int, grant: "Grant") -> None:
        super().__init__(conn, oid)
        self._grant = grant


class Grant(_Proxy):
    """A capability handle, born pending; resolved exactly once, ever."""

    _interface = "vitrin_grant"

    def __init__(self, conn: "Connection", oid: int) -> None:
        super().__init__(conn, oid)
        self._resolved: ResolvedEvent | None = None
        self._refusals: deque[RefusedEvent] = deque()
        self._consent_states: list[ConsentState] = []
        # Facets are attached by Realm.request_grant right after minting.
        self.consent: _ConsentProxy
        self._view: _ViewProxy
        self.pointer: PointerActuator
        self.text: TextActuator
        # The two layout facets are minted on demand rather than co-minted:
        # `request_grant`'s five new_id arguments are frozen forever, so
        # every facet added after version 1 arrives as a structural mint on
        # the grant. Minting is always legal, so this is lazy purely to keep
        # a petition's id footprint at five for clients that never arrange.
        self._layout_focus: _LayoutFocusFacet | None = None
        self._layout_arrange: _LayoutArrangeFacet | None = None
        self._launcher: _LauncherFacet | None = None

    def _handle_event(self, event: Event) -> None:
        if isinstance(event, ResolvedEvent):
            if self._resolved is not None:
                # resolved is sent exactly once per grant, ever.
                self._conn._die_contract("server sent resolved twice on one grant")
            try:
                # Undefined enum entries from a version-1 server are a
                # contract violation (validated here, at dispatch, so the
                # effective_* properties can convert without surprises).
                Outcome(event.outcome)
                Persistence(event.persistence)
            except ValueError as exc:
                self._conn._die_contract(
                    f"server sent resolved with an undefined enum value: {exc}"
                )
            self._resolved = event
        elif isinstance(event, RefusedEvent):
            try:
                Refusal(event.code)
            except ValueError:
                self._conn._die_contract(
                    f"server sent undefined refusal code {event.code}"
                )
            self._refusals.append(event)

    # -- petition lifecycle -------------------------------------------------

    def await_consent(self) -> "Grant":
        """Block until this petition's ``resolved`` event, then return self.

        Petition resolution is exempt from the sync barrier (it waits on an
        unbounded human consent delay), so this simply reads the event
        stream until this grant's terminal arrives. Raises the typed
        :class:`GrantResolutionError` subclass on any non-granted outcome.
        """
        self._conn._run_until(lambda: self._resolved is not None)
        assert self._resolved is not None
        if self._resolved.outcome != Outcome.GRANTED:
            raise resolution_error_by_outcome(self._resolved.outcome)
        return self

    @property
    def resolved(self) -> bool:
        return self._resolved is not None

    @property
    def effective_verbs(self) -> Verb:
        """The effective verb set the human chose (0 while pending/denied)."""
        return Verb(self._resolved.verbs if self._resolved else 0)

    @property
    def effective_persistence(self) -> Persistence:
        return Persistence(self._resolved.persistence if self._resolved else 0)

    @property
    def effective_expiry_ms(self) -> int:
        return self._resolved.expiry_ms if self._resolved else 0

    @property
    def consent_state(self) -> ConsentState | None:
        """The latest prompt-visibility state, if any was delivered."""
        return self._consent_states[-1] if self._consent_states else None

    # -- layout --------------------------------------------------------------

    def focus(self) -> None:
        """Show the granted realm and send the human's input there.

        Fire-and-forget: no terminal event, so this returns as soon as the
        request is on the wire. A refusal arrives on the grant and surfaces
        at the next :meth:`Connection.sync` or the next reply-bearing call —
        the sync-barrier discovery idiom, which is how every fire-and-forget
        refusal is discovered.

        Showing a realm and directing the human's own keyboard and pointer to
        it are **one act**: there is no verb set that separates them, because
        routing a human's keys to a realm they cannot see is focus theft in
        its sharpest form.
        """
        self._conn._send(messages.encode_focus(self._focus_facet().id))

    def set_fullscreen(self, fullscreen: bool) -> None:
        """Make the granted realm fill the output, or stop making it.

        Fire-and-forget, like :meth:`focus`.

        The two modes differ only in whether the realm's view size *tracks*
        the output's, so while the output and the realm are the same size
        they are indistinguishable and this changes nothing observable. That
        is the protocol's own statement, not this SDK's simplification.
        """
        facet = self._arrange_facet()
        mode = (
            protocol.LayoutMode.FULLSCREEN if fullscreen else protocol.LayoutMode.WINDOWED
        )
        self._conn._send(messages.encode_set_fullscreen(facet.id, mode=mode))

    # -- launch --------------------------------------------------------------

    def launch(self) -> str:
        """Start the granted realm's template, returning the new realm's id.

        Reply-bearing (one request, one terminal), exactly like
        :meth:`observe`: sends ``launch`` and blocks until ``launched``
        (returned as the new realm's id) or ``refused(realm.launch, …)``
        (raised as the typed exception — :class:`~vitrin_os.errors.AtCapacity`
        when the deployment is at its realm limit).

        **It takes no arguments, and cannot.** The realm this grant was
        petitioned over names a template; the template names the program.
        Choosing *which* program to run is done by petitioning over a
        different realm, in front of the human, never by an argument here.

        The returned id is minted by the server and is **opaque** — pass it
        straight to :meth:`Connection.get_realm`, do not parse or predict it.
        Launching confers nothing over what was launched: observing or
        actuating the new realm is a separate petition.
        """
        facet = self._launcher_facet()
        self._conn._send(messages.encode_launch(facet.id))
        self._conn._run_until(
            lambda: facet._launched or self._first_refusal(Verb.REALM_LAUNCH) is not None
        )
        refusal = self._first_refusal(Verb.REALM_LAUNCH)
        if not facet._launched and refusal is not None:
            self._refusals.remove(refusal)
            raise refusal_error_by_code(
                refusal.verb, refusal.code, refusal.retry_after_ms, grant_id=self.id
            )
        return facet._launched.popleft().realm

    def _launcher_facet(self) -> _LauncherFacet:
        if self._launcher is None:
            oid = self._conn._allocate_ids(1)[0]
            self._launcher = _LauncherFacet(self._conn, oid, self)
            self._conn._register(self._launcher)
            self._conn._send(messages.encode_get_launcher(self.id, facet_id=oid))
        return self._launcher

    def _focus_facet(self) -> _LayoutFocusFacet:
        if self._layout_focus is None:
            oid = self._conn._allocate_ids(1)[0]
            self._layout_focus = _LayoutFocusFacet(self._conn, oid, self)
            self._conn._register(self._layout_focus)
            self._conn._send(messages.encode_get_layout_focus(self.id, facet_id=oid))
        return self._layout_focus

    def _arrange_facet(self) -> _LayoutArrangeFacet:
        if self._layout_arrange is None:
            oid = self._conn._allocate_ids(1)[0]
            self._layout_arrange = _LayoutArrangeFacet(self._conn, oid, self)
            self._conn._register(self._layout_arrange)
            self._conn._send(messages.encode_get_layout_arrange(self.id, facet_id=oid))
        return self._layout_arrange

    # -- observation --------------------------------------------------------

    def observe(self) -> Frame:
        """Capture one frame (reply-bearing: one request, one terminal).

        Sends ``capture_frame`` and blocks until its terminal event:
        ``frame_ready`` (returned as a verified :class:`Frame`) or
        ``refused(observe, ...)`` (raised as the typed exception). The
        returned frame is a value object — the memfd was verified, copied,
        and closed before this returns, so callers hold no descriptor and
        owe no cleanup (poll model, D6: one fresh frame per call).
        """
        view = self._view
        self._conn._send(messages.encode_capture_frame(view.id))
        self._conn._run_until(
            lambda: view._frames or self._first_refusal(Verb.OBSERVE) is not None
        )
        refusal = self._first_refusal(Verb.OBSERVE)
        if not view._frames and refusal is not None:
            self._refusals.remove(refusal)
            # `self` is the grant the refused event was dispatched on, so
            # self.id is its id — the same payload the actuation path carries
            # (P1.8.3 criterion 3).
            raise refusal_error_by_code(
                refusal.verb, refusal.code, refusal.retry_after_ms, grant_id=self.id
            )
        return self._conn._verify_frame(view._frames.popleft())

    def _first_refusal(self, verb: Verb) -> RefusedEvent | None:
        for refusal in self._refusals:
            if refusal.verb == verb:
                return refusal
        return None


class Realm(_Proxy):
    """An authority-free realm address handle: it only lets you petition."""

    _interface = "vitrin_realm"

    def __init__(self, conn: "Connection", oid: int, name: str) -> None:
        super().__init__(conn, oid)
        self.name = name

    def _handle_event(self, event: Event) -> None:  # no events in version 1
        pass

    def request_grant(
        self,
        *,
        resource: str | None = None,
        verbs: int | Verb | Iterable[str | Verb] = (
            "observe",
            "actuate.pointer",
            "actuate.text",
        ),
        expiry_ms: int = 0,
        max_event_rate: int = 0,
        persistence: Persistence = Persistence.ONCE,
        flags: int = 0,
    ) -> Grant:
        """Petition for authority; mints grant + consent + three facets.

        The five new ids follow the multi-new_id rule by construction:
        allocated contiguously from the strictly-increasing watermark
        allocator, so they are distinct, increasing in argument order, and
        above the watermark.
        """
        conn = self._conn
        verb_bits = _parse_verbs(verbs)
        # Peek at the ids and encode *before* committing anything: an
        # encode-time ValueError (over-bound resource, out-of-u32-range
        # expiry/rate/flags) is a pure client-side bug and must not burn
        # never-reusable watermark ids nor leave dead proxies registered.
        grant_id, consent_id, view_id, pointer_id, text_id = conn._peek_ids(5)
        request = messages.encode_request_grant(
            self.id,
            grant_id=grant_id,
            consent_id=consent_id,
            view_id=view_id,
            pointer_id=pointer_id,
            text_id=text_id,
            resource=resource,
            verbs=verb_bits,
            expiry_ms=expiry_ms,
            max_event_rate=max_event_rate,
            persistence=int(persistence),
            flags=flags,
        )
        conn._commit_ids(5)
        grant = Grant(conn, grant_id)
        grant.consent = _ConsentProxy(conn, consent_id, grant)
        grant._view = _ViewProxy(conn, view_id, grant)
        grant.pointer = PointerActuator(conn, pointer_id, grant)
        grant.text = TextActuator(conn, text_id, grant)
        for proxy in (grant, grant.consent, grant._view, grant.pointer, grant.text):
            conn._register(proxy)
        conn._send(request)
        return grant


class Connection:
    """One principal connection: handshake state machine + event dispatch."""

    def __init__(self, transport: Transport) -> None:
        self._transport = transport
        self._next_id = protocol.CLIENT_ID_MIN
        self._objects: dict[int, _Proxy] = {}
        self._done_cookies: set[int] = set()
        self._next_cookie = 1
        self._bound_identity: str | None = None
        self._attention_count = 0
        self._hello_sent = False
        self._realms: dict[str, Realm] = {}
        self._register(_HandshakeProxy(self, protocol.BOOTSTRAP_OBJECT_ID))

    # -- id allocation (watermark rule, conventions section 3.1) -----------

    def _peek_ids(self, count: int) -> list[int]:
        """The next ``count`` ids *without* advancing the watermark.

        Callers that can still fail client-side after seeing the ids (e.g.
        encode-time validation) peek first and commit only once the frame
        that names the ids is actually going onto the wire, so a pure
        client-side error never burns never-reusable ids.
        """
        if self._next_id + count - 1 > protocol.CLIENT_ID_MAX:
            raise ObjectIdsExhausted(
                "client object ids exhausted (watermark reached 0xfeffffff)"
            )
        return list(range(self._next_id, self._next_id + count))

    def _commit_ids(self, count: int) -> None:
        """Advance the watermark over ids previously peeked."""
        self._next_id += count

    def _allocate_ids(self, count: int) -> list[int]:
        """Allocate ``count`` strictly-increasing ids; never reused.

        The SDK enforces the watermark itself rather than relying on the
        server to catch bugs: ids only ever move forward, and running past
        0xfeffffff raises :class:`ObjectIdsExhausted` locally.
        """
        ids = self._peek_ids(count)
        self._commit_ids(count)
        return ids

    def _allocate_id(self) -> int:
        return self._allocate_ids(1)[0]

    def _register(self, proxy: _Proxy) -> None:
        self._objects[proxy.id] = proxy

    # -- transport plumbing -------------------------------------------------

    def _send(self, frame: bytes) -> None:
        self._transport.send_frame(frame)

    def _die_contract(self, reason: str) -> NoReturn:
        self._transport.close()
        raise ServerContractViolation(reason)

    def _dispatch_one(self) -> None:
        """Read exactly one frame and route it.

        Events addressed to an object this connection does not know are
        tolerated and discarded (their fd closed) per the tolerate-events-
        to-dead-objects rule — safe forever because ids are never reused.
        A fatal ``vitrin_handshake.error`` closes the connection and raises
        its typed :class:`FatalError` subclass.
        """
        object_id, opcode, payload, fd = self._transport.recv_frame()
        proxy = self._objects.get(object_id)
        if proxy is None:
            if fd is not None:
                os.close(fd)
            return
        try:
            event = messages.decode_event(proxy._interface, opcode, payload, fd)
        except ServerContractViolation:
            if fd is not None:
                os.close(fd)
            self._transport.close()
            raise
        if isinstance(event, ErrorEvent):
            self._transport.close()
            raise fatal_error_by_code(event.object_id, event.code, event.message)
        proxy._handle_event(event)

    def _run_until(self, predicate: Callable[[], bool]) -> None:
        while not predicate():
            self._dispatch_one()

    def _barrier(self, grant: Grant | None = None) -> None:
        """The sync/done barrier idiom (conventions section 6.4).

        Sends ``sync`` and reads the ordered stream until its ``done`` —
        always consuming the ``done``, so a refused barrier leaks no cookie
        state. When ``grant`` is given, every refusal queued on that grant
        by the time the ``done`` arrives raises the typed exception: each
        one was necessarily caused by a request sent before the sync
        (possibly an earlier ``flush=False`` actuation or a refusal
        coalesced into this window, not only the immediately preceding
        call). The oldest refusal becomes the exception; the rest of the
        drained window is attached via ``add_note`` — still one round trip,
        since the ``done`` was already in flight behind the refusals.
        """
        cookie = self._next_cookie
        self._next_cookie = self._next_cookie + 1 & 0xFFFFFFFF
        self._send(messages.encode_sync(cookie))
        self._run_until(lambda: cookie in self._done_cookies)
        self._done_cookies.discard(cookie)
        if grant is not None and grant._refusals:
            first = grant._refusals.popleft()
            # The refused event is dispatched on the grant object, so grant.id
            # is the id it names — carried into the exception for debugging
            # (P1.8.3: "exceptions carry the protocol error payload").
            exc = refusal_error_by_code(
                first.verb, first.code, first.retry_after_ms, grant_id=grant.id
            )
            while grant._refusals:
                extra = grant._refusals.popleft()
                exc.add_note(
                    f"also refused in the same barrier window: verb {extra.verb}, "
                    f"code {extra.code}, retry_after_ms {extra.retry_after_ms}"
                )
            raise exc

    # -- handshake (P1.1.3 state machine, client side) ----------------------

    def hello(
        self,
        *,
        identity: str,
        credential_type: str = "static-token",
        credential: str = "",
        version: int = protocol.PROTOCOL_VERSION,
    ) -> "Connection":
        """Authenticate: send ``hello`` and block until ``bound`` or death.

        ``hello`` is legal exactly once per connection; the SDK enforces
        that client-side. Failure raises the typed fatal exception the
        server chose: :class:`VersionUnsupported` when the offered version
        is above the server's maximum, :class:`AuthFailed` on any
        credential rejection (deliberately cause-uniform on the wire).
        """
        if self._hello_sent:
            raise RuntimeError("hello is legal exactly once per connection")
        principal_id = self._allocate_id()
        self._register(_PrincipalProxy(self, principal_id))
        self._hello_sent = True
        self._send(
            messages.encode_hello(
                version=version,
                principal_id=principal_id,
                identity=identity,
                credential_type=credential_type,
                credential=credential,
            )
        )
        self._principal_id = principal_id
        self._run_until(lambda: self._bound_identity is not None)
        return self

    @property
    def identity(self) -> str:
        """The verifier-canonical principal identity (post-``bound``)."""
        if self._bound_identity is None:
            raise RuntimeError("connection is not bound yet")
        return self._bound_identity

    @property
    def bound(self) -> bool:
        return self._bound_identity is not None

    @property
    def attention_count(self) -> int:
        """How many ``vitrin_principal.attention`` events have arrived.

        The human pressed the compositor's own attention key. The event
        confers **nothing** — it says the human's hand is off the app they are
        in, which for a short window stops the server refusing this
        principal's layout requests ``preempted``. Only principals holding a
        live layout grant receive it at all.

        Exposed as a count rather than a callback on purpose: a callback would
        invite a client to *start* work on the press, and the window is not
        promised to any one recipient. Send the request you already staged and
        show the ``Preempted`` refusal if you lost the race.
        """
        return self._attention_count

    # -- steady-state API ---------------------------------------------------

    def _require_bound(self) -> None:
        if self._bound_identity is None:
            raise RuntimeError("connection is not bound (call hello first)")

    def get_realm(self, name: str = "realm-0") -> Realm:
        """Mint an address handle for a realm (structural: no reply).

        Handles are cached per name: minting is cheap but ids are never
        reused, so re-addressing the same realm reuses the handle.
        """
        self._require_bound()
        cached = self._realms.get(name)
        if cached is not None:
            return cached
        realm_id = self._allocate_id()
        realm = Realm(self, realm_id, name)
        self._register(realm)
        self._send(messages.encode_get_realm(self._principal_id, realm_id=realm_id, name=name))
        self._realms[name] = realm
        return realm

    def request_grant(
        self,
        *,
        realm: str = "realm-0",
        resource: str | None = None,
        verbs: int | Verb | Iterable[str | Verb] = (
            "observe",
            "actuate.pointer",
            "actuate.text",
        ),
        expiry_ms: int = 0,
        max_event_rate: int = 0,
        persistence: Persistence = Persistence.ONCE,
        flags: int = 0,
    ) -> Grant:
        """Convenience mirroring PRD section 18: address a realm and petition."""
        return self.get_realm(realm).request_grant(
            resource=resource,
            verbs=verbs,
            expiry_ms=expiry_ms,
            max_event_rate=max_event_rate,
            persistence=persistence,
            flags=flags,
        )

    def sync(self, grant: Grant | None = None) -> None:
        """A barrier round trip: returns once all prior requests are
        processed and their events delivered (petition resolution excepted).

        Pass ``grant`` to surface that grant's refusals as typed exceptions
        — the pattern for a batch of ``flush=False`` actuations bounded by
        one barrier. Without it, refusals stay queued on their grant and
        surface at that grant's next barrier.
        """
        self._require_bound()
        self._barrier(grant)

    # -- frame contract (vitrin_view.frame_ready memfd contract) ------------

    def _verify_frame(self, event: FrameReadyEvent) -> Frame:
        """Verify the frame_ready memfd contract, then materialize the frame.

        The receiver SHOULD verify size and seals so immutability is
        client-provable; a violating frame is a *server* protocol violation
        (never attributed to the grant): discard, close the fd, and close
        the connection (the IDL permits closing; this SDK always does).
        All arithmetic is exact — Python integers are unbounded, so the
        no-32-bit-wraparound requirement holds trivially. A verified frame
        is materialized close-after-copy (:meth:`Frame._from_fd`): the fd
        is read once and closed before ``observe()`` returns.
        """
        fd = event.fd
        try:
            if event.flags != 0:
                raise ServerContractViolation(
                    f"nonzero frame flags {event.flags:#x} in version 1"
                )
            if event.width == 0 or event.height == 0:
                raise ServerContractViolation("zero frame dimension")
            if event.format != Format.XRGB8888:
                raise ServerContractViolation(
                    f"unexpected capture format {event.format:#x} "
                    "(version 1 always announces xrgb8888)"
                )
            if event.stride != event.width * 4:
                raise ServerContractViolation(
                    f"stride {event.stride} != width * 4 ({event.width * 4})"
                )
            actual_size = os.fstat(fd).st_size
            expected_size = event.stride * event.height
            if actual_size != expected_size:
                raise ServerContractViolation(
                    f"memfd size {actual_size} != stride * height {expected_size}"
                )
            seals = fcntl.fcntl(fd, fcntl.F_GET_SEALS)
            required = protocol.REQUIRED_FRAME_SEALS
            if seals & required != required:
                raise ServerContractViolation(
                    f"memfd seals {seals:#x} lack the required "
                    f"SHRINK|GROW|WRITE|SEAL set {required:#x}"
                )
        except ServerContractViolation:
            os.close(fd)
            self._transport.close()
            raise
        return Frame._from_fd(
            fd,
            format=Format(event.format),
            width=event.width,
            height=event.height,
            stride=event.stride,
        )

    # -- lifecycle ----------------------------------------------------------

    @property
    def closed(self) -> bool:
        return self._transport.closed

    def close(self) -> None:
        self._transport.close()

    def __enter__(self) -> "Connection":
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()


def connect(
    path: str | os.PathLike[str],
    *,
    identity: str,
    credential_type: str = "static-token",
    credential: str = "",
    version: int = protocol.PROTOCOL_VERSION,
    timeout: float | None = None,
) -> Connection:
    """Connect to a core socket and complete the credential handshake.

    Returns a bound :class:`Connection`. Raises the typed fatal exception
    (:class:`AuthFailed`, :class:`VersionUnsupported`, ...) if the server
    refuses the handshake; the socket is closed in every failure path.
    """
    transport = Transport.connect_unix(path, timeout=timeout)
    conn = Connection(transport)
    try:
        conn.hello(
            identity=identity,
            credential_type=credential_type,
            credential=credential,
            version=version,
        )
    except BaseException:
        transport.close()
        raise
    # The timeout governs connect + handshake only. Steady-state blocking
    # calls must never trip it: await_consent waits on an unbounded human
    # consent delay, and a timeout escaping mid-sendall would leave the
    # framed stream indeterminate.
    transport.settimeout(None)
    return conn
