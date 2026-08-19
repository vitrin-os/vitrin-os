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
# starts at 1 (the schema forbids 0) and is 2 today. Version 2 appends, in
# full: the realm_launch verb; the three structural mints on vitrin_grant
# (get_launcher, get_layout_focus, get_layout_arrange); the three interfaces
# they mint (vitrin_launcher, vitrin_layout_focus, vitrin_layout_arrange)
# with their requests launch/launched, focus and set_fullscreen; the
# vitrin_layout_arrange.mode enum; the capacity refusal code; and the
# layout_held outcome. Every version-1 signature is byte-identical at
# version 2, so this SDK offering 2 changes nothing about the messages it
# already sends — it only widens what it can send and must decode.
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

    Every bit here is *defined on the wire*. Whether a given bit is **served**
    is a property of the deployment, not of the protocol: the IDL defines a
    verb ahead of anyone serving it, and a deployment that does not serve one
    refuses the petition "unsupported". That distinction is the whole reason
    the unserved bits are here. An out-of-range bit is fatal
    "invalid_argument" and kills the connection, so an SDK that omitted them
    would turn a recoverable refusal into a dead socket for any client that
    petitioned one. Read "unsupported" as "not here, not now", never as "not
    in this protocol".

    The 64/256 gap is deliberate and must not be filled by guesswork: those
    bits are allocated repo-wide to verbs the IDL does not define yet, so they
    are still *out of range* and fatal. `realm_launch` took 512 for exactly
    that reason, and `egress` took 128 from the same registry rather than the
    next bit after 512.
    """

    OBSERVE = 1
    ACTUATE_POINTER = 2
    ACTUATE_TEXT = 4
    OBSERVE_CURSOR = 8  # refused "unsupported" by every deployment at version 2
    LAYOUT_ARRANGE = 16  # served by the reference core since WS-E.1.4
    LAYOUT_FOCUS = 32  # served by the reference core since WS-E.1.4
    EGRESS = 128  # refused "unsupported" by every deployment at version 2
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
    | Verb.EGRESS
    | Verb.REALM_LAUNCH
)

# The verbs a deployment actually serves, as the IDL's own entry summaries
# state it: an entry carrying "refused unsupported in version 1" is unserved,
# and `tests/test_verb_parity.py` re-derives this constant from the IDL so the
# two cannot drift. Kept under its version-1 name because the *wire* did not
# change when the set widened — serving a verb is a deployment property, not a
# version property, and renaming this would imply otherwise.
#
# `layout_arrange` and `layout_focus` joined the set at WS-E.1.4: each gained a
# facet interface (`vitrin_layout_focus`, `vitrin_layout_arrange`), an
# enforcement arm and consent-prompt copy. Still out: `observe_cursor`
# (per-principal cursor delivery is M2's) and `realm_launch` — the latter for a
# reason that is about the VERSION rather than about any deployment, and that
# is why it stays out even though the reference core serves the verb since
# WS-E.1.1 (issue #207): a version-1 connection cannot mint `vitrin_launcher`
# at all (`get_launcher` is `since="2"`), so there is nothing for a version-1
# grant carrying the bit to be exercised through. The IDL says exactly that,
# and this constant is derived from the IDL's own summaries.
#
# `egress` (128, P2.7.2 / issue #196) is also out, and its IDL summary carries
# the same marker phrase for a reason that goes further than the version: the
# facet a connection would be asked for through is not in the IDL at all yet
# (P2.6.5), and the mediating proxy that would carry it is P2.7.3's. So the
# bit is refused by **every** deployment at version 2, not merely by a
# version-1 connection — which is why this constant does not move when it is
# eventually served on some deployments and not others.
# Those are refused "unsupported", and a petition mixing served and unserved
# verbs is refused whole — the core never silently narrows a verb set.
VERBS_SERVED_IN_VERSION_1 = int(
    Verb.OBSERVE
    | Verb.ACTUATE_POINTER
    | Verb.ACTUATE_TEXT
    | Verb.LAYOUT_ARRANGE
    | Verb.LAYOUT_FOCUS
)

# The SDK-level dotted names are these bits (per the IDL's verb enum text,
# which fixes the spelling: the first underscore of the wire name becomes a
# dot, so a second implementation has no name to invent). A wire name carrying
# no underscore has nothing to replace, so its dotted name is the wire name
# unchanged — `egress` is the first, and the IDL states that case rather than
# leaving it to be derived.
VERB_BY_DOTTED_NAME: dict[str, Verb] = {
    "observe": Verb.OBSERVE,
    "actuate.pointer": Verb.ACTUATE_POINTER,
    "actuate.text": Verb.ACTUATE_TEXT,
    "observe.cursor": Verb.OBSERVE_CURSOR,
    "layout.arrange": Verb.LAYOUT_ARRANGE,
    "layout.focus": Verb.LAYOUT_FOCUS,
    "egress": Verb.EGRESS,
    "realm.launch": Verb.REALM_LAUNCH,
}


class Persistence(enum.IntEnum):
    """vitrin_grant.persistence — the consent persistence ladder."""

    ONCE = 0
    WHILE_RUNNING = 1
    UNTIL_REVOKED = 2  # resolves "unsupported" in version 1
    ALWAYS = 3  # resolves "unsupported" in version 1


class Outcome(enum.IntEnum):
    """vitrin_grant.outcome — petition outcomes.

    ``LAYOUT_HELD`` arrived with wire version 2. It is here for exactly the
    reason ``Refusal.CAPACITY`` is: an outcome the SDK does not know decodes
    to ``ServerContractViolation``, which closes the connection — turning the
    recoverable admission answer the IDL's staging argument exists to
    guarantee into the connection death it exists to prevent.
    """

    GRANTED = 0
    DENIED = 1
    TIMED_OUT = 2
    UNAVAILABLE = 3
    UNSUPPORTED = 4
    BUSY = 5
    LAYOUT_HELD = 6


class Refusal(enum.IntEnum):
    """vitrin_grant.refusal — use-time refusal codes.

    ``CAPACITY`` arrived with wire version 2 and is reachable only through
    ``realm_launch``, which the reference core serves since WS-E.1.1. Whether
    any *particular* deployment serves it is still a deployment property, and
    every code the IDL defines is listed here regardless: a code the SDK does
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


class LayoutMode(enum.IntEnum):
    """vitrin_layout_arrange.mode — the two arrangements the scene can express.

    A closed pair rather than a boolean, so a scene able to express a third
    arrangement appends an entry here instead of needing a second request.
    An out-of-range value is fatal ``invalid_argument`` server-side, which is
    why :func:`vitrin_os.messages.encode_set_fullscreen` refuses one locally.
    """

    WINDOWED = 0
    FULLSCREEN = 1


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
