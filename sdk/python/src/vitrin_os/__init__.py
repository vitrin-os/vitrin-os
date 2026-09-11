# SPDX-License-Identifier: Apache-2.0
"""vitrin_os — the pure-Python agent SDK for the Vitrin OS wire protocol.

A deliberately independent second implementation of the wire format
(spec v0, wire version 2): pure Python >= 3.11, stdlib only, no threads,
no async runtime, no C extension. See sdk/python/README.md.
"""

from .client import Connection, Frame, Grant, Realm, connect
from .errors import (
    AtCapacity,
    AuthFailed,
    Busy,
    ConnectionClosed,
    ConsentHeld,
    ConsentTimeout,
    FatalError,
    FdViolation,
    GrantDenied,
    GrantExpired,
    GrantRefused,
    GrantResolutionError,
    GrantUnsupported,
    InternalError,
    InvalidArgument,
    InvalidObject,
    InvalidOpcode,
    LayoutHeld,
    NoSurface,
    NotGranted,
    ObjectIdsExhausted,
    OperationFailed,
    Oversized,
    Preempted,
    PreHandshake,
    RateLimited,
    RealmUnavailable,
    ResourceExhausted,
    Revoked,
    ServerContractViolation,
    VersionUnsupported,
    VitrinError,
)
# The two powerbox terminals are the only *event* types re-exported here, and
# they are here because they are the RETURN TYPES of `Connection.request_file`
# and `request_dir`: a caller has to tell the two apart to use either, and
# reaching into `vitrin_os.messages` to name the answer it was just handed
# would be an odd thing to require. Every other event stays where it is —
# nothing else in this API returns one.
from .messages import DesignatedEvent, PowerboxRefusedEvent
from .protocol import (
    BTN_LEFT,
    BTN_MIDDLE,
    BTN_RIGHT,
    PROTOCOL_VERSION,
    Axis,
    ButtonState,
    ConsentState,
    DesignationKind,
    DesignationMode,
    ErrorCode,
    Format,
    FrameFlags,
    LayoutMode,
    Outcome,
    Persistence,
    PowerboxRefusal,
    Refusal,
    Verb,
)

__version__ = "0.1.0a0"

__all__ = [
    "connect",
    "Connection",
    "Realm",
    "Grant",
    "Frame",
    # errors
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
    "LayoutHeld",
    "GrantRefused",
    "NotGranted",
    "GrantExpired",
    "Revoked",
    "RateLimited",
    "Preempted",
    "ConsentHeld",
    "NoSurface",
    "OperationFailed",
    "AtCapacity",
    # designation terminals (the answers request_file / request_dir return)
    "DesignatedEvent",
    "PowerboxRefusedEvent",
    # protocol constants
    "PROTOCOL_VERSION",
    "Verb",
    "DesignationKind",
    "DesignationMode",
    "PowerboxRefusal",
    "Persistence",
    "Outcome",
    "Refusal",
    "ConsentState",
    "ErrorCode",
    "Format",
    "FrameFlags",
    "ButtonState",
    "Axis",
    "LayoutMode",
    "BTN_LEFT",
    "BTN_RIGHT",
    "BTN_MIDDLE",
]
