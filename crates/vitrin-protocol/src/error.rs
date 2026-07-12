//! Hand-written decode-error type.
//!
//! `DecodeError` is this crate's own Rust-side `Result` error type for turning
//! untrusted bytes (plus an out-of-band fd, where applicable) into a message
//! struct. It is deliberately **not** the same thing as the `vitrin_handshake`
//! `error` wire enum generated from `protocol/vitrin-v0.xml` in
//! `generated::vitrin_handshake::Error` -- that is wire *data* one interface
//! happens to define (the fatal-error codes a live connection sends to a
//! peer); this is a Rust-side decode-failure type that never crosses the wire
//! itself. See [`DecodeError::to_wire_error`] for the (optional, lossy)
//! bridge between the two.

use std::fmt;

/// Why decoding a message from a byte buffer (and, for the one fd-bearing
/// message on an interface, an out-of-band fd) failed.
///
/// One variant per meaningfully distinct decode failure, per
/// `docs/protocol/00-conventions.md` 2.2-2.4 and 5.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer ran out of bytes before the next fixed-size field (or the
    /// 8-byte frame header itself) could be read.
    Truncated { needed: usize, available: usize },

    /// A `string` argument's declared byte range was not valid UTF-8.
    InvalidUtf8,

    /// A `string` argument's bytes contained an embedded NUL. Forbidden on the
    /// wire even though the length is explicit and no NUL terminator is ever
    /// written -- see `docs/protocol/00-conventions.md` 2.2.
    EmbeddedNul,

    /// A `string` argument's declared length exceeded the maximum byte length
    /// documented for that argument (the `(max N bytes)` token in the IDL).
    StringTooLong { max: u32, actual: u32 },

    /// A plain (non-bitfield) enum argument's wire value did not equal any of
    /// the enum's defined entries (whole-value membership check).
    InvalidEnumValue {
        interface: &'static str,
        enum_name: &'static str,
        value: u32,
    },

    /// A `bitfield="true"` enum argument's wire value set a bit outside the
    /// union of the enum's defined entry bits.
    InvalidBitfieldValue {
        interface: &'static str,
        enum_name: &'static str,
        value: u32,
    },

    /// The presence or absence of an out-of-band fd disagreed with the
    /// message's signature (a message declaring one `fd` argument was decoded
    /// with no fd supplied, or vice versa). Models the wire's `fd_violation`:
    /// header `fd_count` disagreeing with the signature, or an unsolicited fd.
    FdCountMismatch { expected: u8, actual: u8 },

    /// Bytes remained in the buffer after every argument the message
    /// signature defines was decoded.
    TrailingBytes { consumed: usize, total: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated { needed, available } => write!(
                f,
                "truncated buffer: needed at least {needed} bytes, had {available}"
            ),
            DecodeError::InvalidUtf8 => write!(f, "string argument was not valid UTF-8"),
            DecodeError::EmbeddedNul => write!(f, "string argument contained an embedded NUL byte"),
            DecodeError::StringTooLong { max, actual } => write!(
                f,
                "string argument of {actual} bytes exceeds its documented maximum of {max} bytes"
            ),
            DecodeError::InvalidEnumValue {
                interface,
                enum_name,
                value,
            } => write!(
                f,
                "value {value} is not a defined entry of enum {interface}.{enum_name}"
            ),
            DecodeError::InvalidBitfieldValue {
                interface,
                enum_name,
                value,
            } => write!(
                f,
                "value 0x{value:x} sets bits outside the defined union of bitfield {interface}.{enum_name}"
            ),
            DecodeError::FdCountMismatch { expected, actual } => write!(
                f,
                "fd count mismatch: message signature expects {expected}, got {actual}"
            ),
            DecodeError::TrailingBytes { consumed, total } => write!(
                f,
                "{} trailing byte(s) after decoding a complete message ({consumed} of {total} consumed)",
                total - consumed
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

impl DecodeError {
    /// Best-effort bridge to the `vitrin_handshake.error` wire enum, per the
    /// mapping in `docs/protocol/00-conventions.md` 5.2. This is a Rust-side
    /// convenience for a caller that wants to turn a decode failure straight
    /// into the fatal wire error a principal connection would send back; it
    /// is lossy (several `DecodeError` variants collapse onto
    /// `invalid_argument`) and is not itself part of the wire codec.
    pub fn to_wire_error(&self) -> crate::generated::vitrin_handshake::Error {
        use crate::generated::vitrin_handshake::Error as WireError;
        match self {
            // "frame size above 65535 bytes, or a truncated payload"
            DecodeError::Truncated { .. } => WireError::Oversized,
            // "argument decode failure: bad UTF-8, embedded NUL, string over
            // its bound, out-of-range enum value, ..., malformed padding"
            DecodeError::InvalidUtf8
            | DecodeError::EmbeddedNul
            | DecodeError::StringTooLong { .. }
            | DecodeError::InvalidEnumValue { .. }
            | DecodeError::InvalidBitfieldValue { .. } => WireError::InvalidArgument,
            // "header fd_count disagrees with the signature, or unsolicited
            // fds attached"
            DecodeError::FdCountMismatch { .. } => WireError::FdViolation,
            // Not named verbatim in the fatal-error table; treated as a
            // malformed-argument-layout condition.
            DecodeError::TrailingBytes { .. } => WireError::InvalidArgument,
        }
    }
}
