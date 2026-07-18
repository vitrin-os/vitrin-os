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

from . import messages, protocol
from .errors import (
    ConnectionClosed,
    ObjectIdsExhausted,
    ServerContractViolation,
    fatal_error_by_code,
    refusal_error_by_code,
    resolution_error_by_outcome,
)
from .messages import (
    BoundEvent,
    ConsentStateEvent,
    DoneEvent,
    ErrorEvent,
    Event,
    FrameReadyEvent,
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
    if bits & ~int(Verb.OBSERVE | Verb.ACTUATE_POINTER | Verb.ACTUATE_TEXT):
        # An out-of-range verb bit is fatal invalid_argument server-side.
        raise ValueError(f"verb bits {bits:#x} outside the version-1 bitfield")
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
    """One captured frame, backed by a sealed memfd the client now owns."""

    def __init__(self, fd: int, format: Format, width: int, height: int, stride: int) -> None:
        self.fd = fd
        self.format = format
        self.width = width
        self.height = height
        self.stride = stride
        self.size = stride * height
        self._closed = False

    def read_bytes(self) -> bytes:
        """Read the whole frame (row r begins at byte offset r * stride)."""
        if self._closed:
            raise ValueError("frame is closed")
        return os.pread(self.fd, self.size, 0)

    @property
    def closed(self) -> bool:
        return self._closed

    def close(self) -> None:
        """Close the memfd (fd ownership transferred to us; we must close)."""
        if not self._closed:
            self._closed = True
            os.close(self.fd)

    def __enter__(self) -> "Frame":
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()

    def __del__(self) -> None:  # last-resort leak guard
        if not getattr(self, "_closed", True):
            self.close()


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

    # -- observation --------------------------------------------------------

    def observe(self) -> Frame:
        """Capture one frame (reply-bearing: one request, one terminal).

        Sends ``capture_frame`` and blocks until its terminal event:
        ``frame_ready`` (returned as a verified :class:`Frame`) or
        ``refused(observe, ...)`` (raised as the typed exception).
        """
        view = self._view
        self._conn._send(messages.encode_capture_frame(view.id))
        self._conn._run_until(
            lambda: view._frames or self._first_refusal(Verb.OBSERVE) is not None
        )
        refusal = self._first_refusal(Verb.OBSERVE)
        if not view._frames and refusal is not None:
            self._refusals.remove(refusal)
            raise refusal_error_by_code(
                refusal.verb, refusal.code, refusal.retry_after_ms
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
            exc = refusal_error_by_code(first.verb, first.code, first.retry_after_ms)
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
        """Verify the frame_ready memfd contract before exposing the frame.

        The receiver SHOULD verify size and seals so immutability is
        client-provable; a violating frame is a *server* protocol violation
        (never attributed to the grant): discard, close the fd, and close
        the connection (the IDL permits closing; this SDK always does).
        All arithmetic is exact — Python integers are unbounded, so the
        no-32-bit-wraparound requirement holds trivially.
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
        return Frame(fd, Format(event.format), event.width, event.height, event.stride)

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
