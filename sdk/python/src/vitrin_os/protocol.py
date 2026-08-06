# SPDX-License-Identifier: Apache-2.0
"""Protocol constants and enums, transcribed by hand from protocol/vitrin-v0.xml.

This module is deliberately *not* generated from the IDL: the Python SDK is
an independent second implementation of the wire format (decision D8), so
these values are written down by a human reading the XML and pinned by the
golden-vector tests. If the IDL and this file disagree, the IDL wins — fix
this file.
"""

from __future__ import annotations

import enum

# protocol/@version — the wire version integer offered in hello. The "v0"
# in the document family's name is the spec generation; the wire integer
# starts at 1 (the schema forbids 0) and is 2 today. Version 2 appends the
# realm_launch verb, vitrin_grant.get_launcher and the vitrin_launcher
# interface; every version-1 signature is byte-identical at version 2, so
# this SDK offering 2 changes nothing about the messages it already sends.
PROTOCOL_VERSION = 2

# --- object-id ranges (conventions section 3) ------------------------------

NULL_OBJECT_ID = 0
BOOTSTRAP_OBJECT_ID = 1  # vitrin_handshake on principal connections
CLIENT_ID_MIN = 2
CLIENT_ID_MAX = 0xFEFFFFFF  # [0xff000000, 0xffffffff] is server-reserved

# --- per-argument string bounds in bytes (conventions section 2.3) ---------

MAX_IDENTITY_BYTES = 2048
MAX_CREDENTIAL_TYPE_BYTES = 32
MAX_CREDENTIAL_BYTES = 32768
MAX_ERROR_MESSAGE_BYTES = 1024
MAX_REALM_NAME_BYTES = 64
MAX_RESOURCE_BYTES = 256
MAX_TEXT_BYTES = 4096


class ErrorCode(enum.IntEnum):
    """vitrin_handshake.error — the ten connection-global fatal codes."""

    INVALID_OBJECT = 0
    INVALID_OPCODE = 1
    INVALID_ARGUMENT = 2
    OVERSIZED = 3
    FD_VIOLATION = 4
    PRE_HANDSHAKE = 5
    VERSION_UNSUPPORTED = 6
    AUTH_FAILED = 7
    INTERNAL = 8
    RESOURCE_EXHAUSTED = 9


class Verb(enum.IntFlag):
    """vitrin_grant.verb — the grantable verb bitfield.

    The last four are *defined but unserved*: the IDL defines them ahead of
    the core serving them and refuses them "unsupported". That distinction
    is the whole reason they are here. An out-of-range bit is fatal
    "invalid_argument" and kills the connection, so an SDK that omitted them
    would turn a recoverable refusal into a dead socket for any client that
    petitioned one.

    The 64/128/256 gap is deliberate and must not be filled by guesswork:
    those bits are allocated repo-wide to verbs the IDL does not define yet,
    so they are still *out of range* and fatal. `realm_launch` took 512 for
    exactly that reason.
    """

    OBSERVE = 1
    ACTUATE_POINTER = 2
    ACTUATE_TEXT = 4
    OBSERVE_CURSOR = 8  # resolves "unsupported" in version 1
    LAYOUT_ARRANGE = 16  # resolves "unsupported" in version 1
    LAYOUT_FOCUS = 32  # resolves "unsupported" in version 1
    REALM_LAUNCH = 512  # resolves "unsupported" until a deployment serves it


# Every bit the IDL's verb enum defines. Petitioning a defined-but-unserved
# bit is legal and answered "unsupported"; a bit outside this mask is fatal.
VERB_MASK = int(
    Verb.OBSERVE
    | Verb.ACTUATE_POINTER
    | Verb.ACTUATE_TEXT
    | Verb.OBSERVE_CURSOR
    | Verb.LAYOUT_ARRANGE
    | Verb.LAYOUT_FOCUS
    | Verb.REALM_LAUNCH
)

# The verbs a deployment actually serves. Named for version 1 because that
# is the version whose served set this is, and the set did not widen at
# version 2 — `realm_launch` arrived defined-but-unserved, like the three
# before it. The rest are refused "unsupported", and a petition mixing
# served and unserved verbs is refused whole — the core never silently
# narrows a verb set.
VERBS_SERVED_IN_VERSION_1 = int(Verb.OBSERVE | Verb.ACTUATE_POINTER | Verb.ACTUATE_TEXT)

# The SDK-level dotted names are these bits (per the IDL's verb enum text,
# which fixes the spelling: the first underscore of the wire name becomes a
# dot, so a second implementation has no name to invent).
VERB_BY_DOTTED_NAME: dict[str, Verb] = {
    "observe": Verb.OBSERVE,
    "actuate.pointer": Verb.ACTUATE_POINTER,
    "actuate.text": Verb.ACTUATE_TEXT,
    "observe.cursor": Verb.OBSERVE_CURSOR,
    "layout.arrange": Verb.LAYOUT_ARRANGE,
    "layout.focus": Verb.LAYOUT_FOCUS,
    "realm.launch": Verb.REALM_LAUNCH,
}


class Persistence(enum.IntEnum):
    """vitrin_grant.persistence — the consent persistence ladder."""

    ONCE = 0
    WHILE_RUNNING = 1
    UNTIL_REVOKED = 2  # resolves "unsupported" in version 1
    ALWAYS = 3  # resolves "unsupported" in version 1


class Outcome(enum.IntEnum):
    """vitrin_grant.outcome — petition outcomes."""

    GRANTED = 0
    DENIED = 1
    TIMED_OUT = 2
    UNAVAILABLE = 3
    UNSUPPORTED = 4
    BUSY = 5


class Refusal(enum.IntEnum):
    """vitrin_grant.refusal — use-time refusal codes.

    ``CAPACITY`` arrived with wire version 2 and is reachable only through
    ``realm_launch``. It is here even though no deployment serves that verb
    yet, for the same reason the unserved verb bits are: a code the SDK does
    not know decodes to ``ServerContractViolation``, which is an accusation
    that the *server* broke the contract — the wrong answer for a refusal
    the IDL defines.
    """

    NOT_GRANTED = 0
    EXPIRED = 1
    REVOKED = 2
    RATE_LIMITED = 3
    PREEMPTED = 4
    CONSENT_HELD = 5
    NO_SURFACE = 6
    INTERNAL = 7
    CAPACITY = 8


class ConsentState(enum.IntEnum):
    """vitrin_consent.consent_state — prompt visibility states."""

    QUEUED = 0
    SHOWN = 1
    CLOSED = 2


class Format(enum.IntEnum):
    """vitrin_view.format — pixel formats (DRM fourcc values)."""

    XRGB8888 = 0x34325258  # 'XR24'
    ARGB8888 = 0x34325241  # 'AR24'


class FrameFlags(enum.IntFlag):
    """vitrin_view.frame_flags — reserved in version 1, always 0."""

    Y_INVERT = 1
    DMABUF = 2


class ButtonState(enum.IntEnum):
    """vitrin_actuator_pointer.button_state."""

    RELEASED = 0
    PRESSED = 1


class Axis(enum.IntEnum):
    """vitrin_actuator_pointer.axis."""

    VERTICAL = 0
    HORIZONTAL = 1


# Linux evdev button codes for convenience (the wire carries the raw code).
BTN_LEFT = 0x110
BTN_RIGHT = 0x111
BTN_MIDDLE = 0x112

# memfd seal bits (linux/fcntl.h) — the frame_ready memfd contract requires
# all four. Python's fcntl module exposes F_SEAL_* on Linux, but the values
# are restated here so the contract check cannot silently weaken on a build
# whose fcntl lacks a constant.
F_SEAL_SEAL = 0x0001
F_SEAL_SHRINK = 0x0002
F_SEAL_GROW = 0x0004
F_SEAL_WRITE = 0x0008
REQUIRED_FRAME_SEALS = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE
