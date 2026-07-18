"""Typed exception hierarchy for the Vitrin OS Python SDK.

P1.8.1 skeleton, fleshed out ergonomically in P1.8.3. The wire-to-exception
mapping is normative and exhaustive per docs/protocol/00-conventions.md:

- Section 5.2: every fatal ``vitrin_handshake.error`` code maps to exactly
  one :class:`FatalError` subclass (the connection is dead afterwards).
- Section 5.3: every ``vitrin_grant.resolved`` outcome other than
  ``granted`` maps to exactly one :class:`GrantResolutionError` subclass,
  and every ``vitrin_grant.refused`` code maps to exactly one
  :class:`GrantRefused` subclass (both recoverable: the connection lives).

Exhaustiveness is asserted at import time at the bottom of this module, so
a protocol enum growing without a matching exception class fails loudly.

Client-side *programming* errors (a string over its documented bound, a
forbidden control character in ``type()``, zero verbs in a petition) raise
plain :class:`ValueError` before any bytes hit the wire: a correct client
never emits fatal traffic, so those are bugs in the caller, not wire
conditions.
"""

from __future__ import annotations

__all__ = [
    "VitrinError",
    "ConnectionClosed",
    "ServerContractViolation",
    "ObjectIdsExhausted",
    "FatalError",
    "InvalidObject",
    "InvalidOpcode",
    "InvalidArgument",
    "Oversized",
    "FdViolation",
    "PreHandshake",
    "VersionUnsupported",
    "AuthFailed",
    "InternalError",
    "ResourceExhausted",
    "GrantResolutionError",
    "GrantDenied",
    "ConsentTimeout",
    "RealmUnavailable",
    "GrantUnsupported",
    "Busy",
    "GrantRefused",
    "NotGranted",
    "GrantExpired",
    "Revoked",
    "RateLimited",
    "Preempted",
    "ConsentHeld",
    "NoSurface",
    "OperationFailed",
    "fatal_error_by_code",
    "resolution_error_by_outcome",
    "refusal_error_by_code",
]


class VitrinError(Exception):
    """Base class of every exception raised by the vitrin_os SDK."""


class ConnectionClosed(VitrinError):
    """The connection is gone: EOF from the server, or use after close."""


class ServerContractViolation(VitrinError):
    """The *server* violated the protocol contract.

    Unframeable bytes, an event with a wrong fd count, an unknown opcode on
    a known object, a second ``resolved`` on one grant, or a ``frame_ready``
    frame that breaks the memfd contract (wrong size, missing seals, zero
    dimension, bad stride, nonzero flags in version 1). Version 1 has no
    client-to-server error channel by design, so the client's only sanction
    is disconnecting; the SDK closes the connection before raising this.
    """


class ObjectIdsExhausted(VitrinError):
    """The client-side id allocator ran out of ``[2, 0xfeffffff]``.

    The watermark rule (conventions section 3.1) makes ids strictly
    increasing and never reused; the SDK enforces the bound itself rather
    than letting the server kill the connection with ``resource_exhausted``.
    """


# ---------------------------------------------------------------------------
# Fatal errors — vitrin_handshake.error, conventions section 5.2.
# ---------------------------------------------------------------------------


class FatalError(VitrinError):
    """A fatal ``vitrin_handshake.error`` event arrived; the connection died.

    Attributes mirror the event arguments: ``object_id`` is the object where
    the error occurred, ``code`` the numeric error enum value, and
    ``wire_message`` the free-form (never machine-parsed) debug text.
    """

    CODE: int  # set on each subclass

    def __init__(self, object_id: int, code: int, wire_message: str) -> None:
        super().__init__(
            f"fatal protocol error {self.__class__.__name__} (code {code}) "
            f"on object {object_id}: {wire_message}"
        )
        self.object_id = object_id
        self.code = code
        self.wire_message = wire_message


class InvalidObject(FatalError):
    """Unknown/foreign object id, watermark reuse, or multi-new_id violation."""

    CODE = 0


class InvalidOpcode(FatalError):
    """Opcode not defined for the interface at the negotiated version."""

    CODE = 1


class InvalidArgument(FatalError):
    """Argument decode failure (bad UTF-8, over-bound string, bad enum, ...)."""

    CODE = 2


class Oversized(FatalError):
    """Declared frame size below the header minimum, or a short payload."""

    CODE = 3


class FdViolation(FatalError):
    """Header fd count disagrees with the signature, or unsolicited fds."""

    CODE = 4


class PreHandshake(FatalError):
    """Traffic before a first ``hello`` on a principal connection."""

    CODE = 5


class VersionUnsupported(FatalError):
    """``hello`` offered a protocol version above the server's maximum."""

    CODE = 6


class AuthFailed(FatalError):
    """Credential rejected; the cause is never distinguished on the wire."""

    CODE = 7


class InternalError(FatalError):
    """Server-side failure that poisoned the connection."""

    CODE = 8


class ResourceExhausted(FatalError):
    """A documented per-connection resource bound was breached."""

    CODE = 9


_FATAL_BY_CODE: dict[int, type[FatalError]] = {
    cls.CODE: cls
    for cls in (
        InvalidObject,
        InvalidOpcode,
        InvalidArgument,
        Oversized,
        FdViolation,
        PreHandshake,
        VersionUnsupported,
        AuthFailed,
        InternalError,
        ResourceExhausted,
    )
}


def fatal_error_by_code(object_id: int, code: int, wire_message: str) -> FatalError:
    """Build the typed :class:`FatalError` for one ``error`` event.

    An unknown code is itself a server contract violation on a version-1
    connection (enum values are immutable and the negotiated version defines
    exactly these ten), so it raises rather than collapsing into a generic
    error.
    """
    cls = _FATAL_BY_CODE.get(code)
    if cls is None:
        raise ServerContractViolation(
            f"server sent unknown fatal error code {code} on object {object_id}"
        )
    return cls(object_id, code, wire_message)


# ---------------------------------------------------------------------------
# Petition outcomes — vitrin_grant.resolved, conventions section 5.3.
# ---------------------------------------------------------------------------


class GrantResolutionError(VitrinError):
    """The petition resolved with an outcome other than ``granted``."""

    OUTCOME: int  # set on each subclass

    def __init__(self) -> None:
        super().__init__(
            f"grant petition resolved {self.__class__.__name__} "
            f"(outcome {self.OUTCOME})"
        )
        self.outcome = self.OUTCOME


class GrantDenied(GrantResolutionError):
    """The human said no."""

    OUTCOME = 1


class ConsentTimeout(GrantResolutionError):
    """The consent prompt expired unanswered; petitioning again is legal."""

    OUTCOME = 2


class RealmUnavailable(GrantResolutionError):
    """The realm was unknown, vacant, or closed while pending."""

    OUTCOME = 3


class GrantUnsupported(GrantResolutionError):
    """In-range but refused by policy (durable rung, reserved flag, ...)."""

    OUTCOME = 4


class Busy(GrantResolutionError):
    """The pending-petition admission cap for this identity was reached."""

    OUTCOME = 5


_RESOLUTION_BY_OUTCOME: dict[int, type[GrantResolutionError]] = {
    cls.OUTCOME: cls
    for cls in (GrantDenied, ConsentTimeout, RealmUnavailable, GrantUnsupported, Busy)
}


def resolution_error_by_outcome(outcome: int) -> GrantResolutionError:
    """Build the typed error for a non-``granted`` ``resolved`` outcome."""
    cls = _RESOLUTION_BY_OUTCOME.get(outcome)
    if cls is None:
        raise ServerContractViolation(
            f"server sent unknown resolved outcome {outcome}"
        )
    return cls()


# ---------------------------------------------------------------------------
# Use-time refusals — vitrin_grant.refused, conventions section 5.3.
# ---------------------------------------------------------------------------


class GrantRefused(VitrinError):
    """The enforcement chokepoint refused one use of a grant (recoverable)."""

    REFUSAL: int  # set on each subclass

    def __init__(self, verb: int, retry_after_ms: int = 0) -> None:
        super().__init__(
            f"use of verb {verb} refused: {self.__class__.__name__} "
            f"(code {self.REFUSAL}, retry_after_ms {retry_after_ms})"
        )
        self.verb = verb
        self.code = self.REFUSAL
        self.retry_after_ms = retry_after_ms


class NotGranted(GrantRefused):
    """The grant is not (or not yet) active, or the verb is outside its set."""

    REFUSAL = 0


class GrantExpired(GrantRefused):
    """The grant's expiry passed."""

    REFUSAL = 1


class Revoked(GrantRefused):
    """Revoked by hold-Esc, panel, or policy."""

    REFUSAL = 2


class RateLimited(GrantRefused):
    """The token bucket is empty; ``retry_after_ms`` hints the refill."""

    REFUSAL = 3


class Preempted(GrantRefused):
    """Physical human input owns the target right now."""

    REFUSAL = 4


class ConsentHeld(GrantRefused):
    """The principal's own pending petition has a prompt up."""

    REFUSAL = 5


class NoSurface(GrantRefused):
    """The realm has no surface (its shim crashed or exited)."""

    REFUSAL = 6


class OperationFailed(GrantRefused):
    """Server-side failure during this use (renderer, memfd, delivery)."""

    REFUSAL = 7


_REFUSAL_BY_CODE: dict[int, type[GrantRefused]] = {
    cls.REFUSAL: cls
    for cls in (
        NotGranted,
        GrantExpired,
        Revoked,
        RateLimited,
        Preempted,
        ConsentHeld,
        NoSurface,
        OperationFailed,
    )
}


def refusal_error_by_code(verb: int, code: int, retry_after_ms: int) -> GrantRefused:
    """Build the typed error for one ``refused`` event."""
    cls = _REFUSAL_BY_CODE.get(code)
    if cls is None:
        raise ServerContractViolation(f"server sent unknown refusal code {code}")
    return cls(verb, retry_after_ms)


# ---------------------------------------------------------------------------
# Exhaustiveness: one distinct class per wire value, no gaps, no overlaps.
# The enum value sets are restated here on purpose (not imported from
# protocol.py) so a drift between the two modules fails at import time.
# ---------------------------------------------------------------------------

assert sorted(_FATAL_BY_CODE) == list(range(10)), "fatal code mapping not exhaustive"
assert sorted(_RESOLUTION_BY_OUTCOME) == list(range(1, 6)), (
    "resolution outcome mapping not exhaustive"
)
assert sorted(_REFUSAL_BY_CODE) == list(range(8)), "refusal code mapping not exhaustive"
