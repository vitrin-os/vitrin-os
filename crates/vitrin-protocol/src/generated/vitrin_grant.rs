// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_grant`, version 1.
//!
//! capability handle

pub const INTERFACE_NAME: &str = "vitrin_grant";
pub const INTERFACE_VERSION: u32 = 1;

pub mod events {

    /// Event `resolved` (opcode 0) on `vitrin_grant`.
    ///
    /// the petition's terminal outcome
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Resolved {
        /// how the petition resolved
        pub outcome: crate::generated::vitrin_grant::Outcome,
        /// effective verb set (0 unless granted)
        pub verbs: crate::generated::vitrin_grant::Verb,
        /// effective persistence rung (once unless granted)
        pub persistence: crate::generated::vitrin_grant::Persistence,
        /// effective lifetime in milliseconds; 0 = bounded by the rung
        pub expiry_ms: u32,
    }

    impl Resolved {
        pub const OPCODE: u8 = 0;
        pub const HAS_FD: bool = false;

        /// Encode into a complete frame (header + argument payload). The fd
        /// argument, if this message has one, is not written here -- send it
        /// out-of-band via `SCM_RIGHTS` alongside these bytes.
        pub fn encode(&self, object_id: u32) -> Vec<u8> {
            let mut out = Vec::new();
            crate::wire::FrameHeader {
                object_id,
                size: 0,
                opcode: Self::OPCODE,
                fd_count: Self::HAS_FD as u8,
            }
            .encode_with_placeholder_size(&mut out);
            crate::wire::write_uint(&mut out, self.outcome.to_wire());
            crate::wire::write_uint(&mut out, self.verbs.bits());
            crate::wire::write_uint(&mut out, self.persistence.to_wire());
            crate::wire::write_uint(&mut out, self.expiry_ms);
            crate::wire::patch_size(&mut out);
            out
        }

        /// Decode a complete frame (header + argument payload) plus, iff
        /// `Self::HAS_FD`, the fd received alongside it out-of-band. Returns the
        /// frame's `object_id` (routing data the caller's dispatcher needs)
        /// alongside the decoded message.
        ///
        /// `docs/protocol/00-conventions.md` 2.4/5.2 define `fd_violation` as two
        /// independent disjuncts, both checked here: the header's own `fd_count`
        /// byte disagreeing with this message's signature, and the out-of-band
        /// `fd` parameter disagreeing with it. A hostile or buggy peer can make
        /// either one lie without the other, so neither check substitutes for
        /// the other.
        ///
        /// The header's `opcode` and `size` fields are validated in the same
        /// defense-in-depth spirit: the dispatcher already selected this message
        /// type by opcode and delimited the frame by size, but a dispatcher bug
        /// (or a header whose size field lies about the delivered byte count,
        /// fatal `oversized` per conventions 2.1) must surface as an error here,
        /// not as a silently mis-decoded message.
        pub fn decode(
            bytes: &[u8],
            fd: Option<std::os::fd::OwnedFd>,
        ) -> Result<(u32, Self), crate::error::DecodeError> {
            if fd.is_some() != Self::HAS_FD {
                return Err(crate::error::DecodeError::FdCountMismatch {
                    expected: Self::HAS_FD as u8,
                    actual: fd.is_some() as u8,
                });
            }
            let header = crate::wire::FrameHeader::decode(bytes)?;
            if header.opcode != Self::OPCODE {
                return Err(crate::error::DecodeError::OpcodeMismatch {
                    expected: Self::OPCODE,
                    actual: header.opcode,
                });
            }
            if header.size as usize != bytes.len() {
                return Err(crate::error::DecodeError::SizeMismatch {
                    declared: header.size,
                    actual: bytes.len(),
                });
            }
            if header.fd_count != Self::HAS_FD as u8 {
                return Err(crate::error::DecodeError::FdCountMismatch {
                    expected: Self::HAS_FD as u8,
                    actual: header.fd_count,
                });
            }
            #[allow(unused_mut)]
            let mut pos = crate::wire::HEADER_LEN;
            let outcome = crate::generated::vitrin_grant::Outcome::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let verbs = crate::generated::vitrin_grant::Verb::from_bits(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let persistence = crate::generated::vitrin_grant::Persistence::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let expiry_ms = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Resolved {
                    outcome,
                    verbs,
                    persistence,
                    expiry_ms,
                },
            ))
        }
    }

    /// Event `refused` (opcode 1) on `vitrin_grant`.
    ///
    /// the chokepoint refused one use of this grant
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Refused {
        /// the verb whose use was refused
        pub verb: crate::generated::vitrin_grant::Verb,
        /// why the use was refused
        pub code: crate::generated::vitrin_grant::Refusal,
        /// refill hint in milliseconds; nonzero only for rate_limited
        pub retry_after_ms: u32,
    }

    impl Refused {
        pub const OPCODE: u8 = 1;
        pub const HAS_FD: bool = false;

        /// Encode into a complete frame (header + argument payload). The fd
        /// argument, if this message has one, is not written here -- send it
        /// out-of-band via `SCM_RIGHTS` alongside these bytes.
        pub fn encode(&self, object_id: u32) -> Vec<u8> {
            let mut out = Vec::new();
            crate::wire::FrameHeader {
                object_id,
                size: 0,
                opcode: Self::OPCODE,
                fd_count: Self::HAS_FD as u8,
            }
            .encode_with_placeholder_size(&mut out);
            crate::wire::write_uint(&mut out, self.verb.bits());
            crate::wire::write_uint(&mut out, self.code.to_wire());
            crate::wire::write_uint(&mut out, self.retry_after_ms);
            crate::wire::patch_size(&mut out);
            out
        }

        /// Decode a complete frame (header + argument payload) plus, iff
        /// `Self::HAS_FD`, the fd received alongside it out-of-band. Returns the
        /// frame's `object_id` (routing data the caller's dispatcher needs)
        /// alongside the decoded message.
        ///
        /// `docs/protocol/00-conventions.md` 2.4/5.2 define `fd_violation` as two
        /// independent disjuncts, both checked here: the header's own `fd_count`
        /// byte disagreeing with this message's signature, and the out-of-band
        /// `fd` parameter disagreeing with it. A hostile or buggy peer can make
        /// either one lie without the other, so neither check substitutes for
        /// the other.
        ///
        /// The header's `opcode` and `size` fields are validated in the same
        /// defense-in-depth spirit: the dispatcher already selected this message
        /// type by opcode and delimited the frame by size, but a dispatcher bug
        /// (or a header whose size field lies about the delivered byte count,
        /// fatal `oversized` per conventions 2.1) must surface as an error here,
        /// not as a silently mis-decoded message.
        pub fn decode(
            bytes: &[u8],
            fd: Option<std::os::fd::OwnedFd>,
        ) -> Result<(u32, Self), crate::error::DecodeError> {
            if fd.is_some() != Self::HAS_FD {
                return Err(crate::error::DecodeError::FdCountMismatch {
                    expected: Self::HAS_FD as u8,
                    actual: fd.is_some() as u8,
                });
            }
            let header = crate::wire::FrameHeader::decode(bytes)?;
            if header.opcode != Self::OPCODE {
                return Err(crate::error::DecodeError::OpcodeMismatch {
                    expected: Self::OPCODE,
                    actual: header.opcode,
                });
            }
            if header.size as usize != bytes.len() {
                return Err(crate::error::DecodeError::SizeMismatch {
                    declared: header.size,
                    actual: bytes.len(),
                });
            }
            if header.fd_count != Self::HAS_FD as u8 {
                return Err(crate::error::DecodeError::FdCountMismatch {
                    expected: Self::HAS_FD as u8,
                    actual: header.fd_count,
                });
            }
            #[allow(unused_mut)]
            let mut pos = crate::wire::HEADER_LEN;
            let verb = crate::generated::vitrin_grant::Verb::from_bits(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let code = crate::generated::vitrin_grant::Refusal::from_wire(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let retry_after_ms = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Refused {
                    verb,
                    code,
                    retry_after_ms,
                },
            ))
        }
    }
}

/// Enum `verb` on `vitrin_grant` (bitfield).
///
/// grantable verbs
///
/// Bitfield: any combination of the defined entries' bits is a legal wire
/// value; a bit outside their union is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Verb(u32);

impl Verb {
    /// capture frames of the granted resource
    pub const OBSERVE: Verb = Verb(1);
    /// inject pointer motion, buttons, and scroll
    pub const ACTUATE_POINTER: Verb = Verb(2);
    /// inject Unicode text
    pub const ACTUATE_TEXT: Verb = Verb(4);

    /// Union of every defined entry's bits; a wire value with any other
    /// bit set is invalid.
    pub const VALID_MASK: u32 = 1 | 2 | 4;

    /// Decode a wire value, rejecting any bit outside `VALID_MASK`.
    pub fn from_bits(value: u32) -> Result<Self, crate::error::DecodeError> {
        if value & !Self::VALID_MASK != 0 {
            Err(crate::error::DecodeError::InvalidBitfieldValue {
                interface: "vitrin_grant",
                enum_name: "verb",
                value,
            })
        } else {
            Ok(Verb(value))
        }
    }

    /// The raw wire bitmask.
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Whether every bit set in `other` is also set in `self`.
    pub fn contains(self, other: Verb) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Verb {
    type Output = Verb;

    fn bitor(self, rhs: Verb) -> Verb {
        Verb(self.0 | rhs.0)
    }
}

/// Enum `persistence` on `vitrin_grant`.
///
/// the consent persistence ladder
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Persistence {
    /// single-use authority
    Once = 0,
    /// lives while the requesting principal's connection lives
    WhileRunning = 1,
    /// durable until explicitly revoked (requires verified provenance; refused in version 1)
    UntilRevoked = 2,
    /// durable and auto-reissued (requires verified provenance; refused in version 1)
    Always = 3,
}

impl Persistence {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Persistence] = &[
        Persistence::Once,
        Persistence::WhileRunning,
        Persistence::UntilRevoked,
        Persistence::Always,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Persistence::Once),
            1 => Ok(Persistence::WhileRunning),
            2 => Ok(Persistence::UntilRevoked),
            3 => Ok(Persistence::Always),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_grant",
                enum_name: "persistence",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Enum `outcome` on `vitrin_grant`.
///
/// petition outcomes
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Outcome {
    /// authority active; the event carries the effective verbs, rung, and expiry
    Granted = 0,
    /// the human said no
    Denied = 1,
    /// the consent prompt expired unanswered; petitioning again later is legal
    TimedOut = 2,
    /// the realm was unknown, vacant, or closed while the petition was pending
    Unavailable = 3,
    /// in-range but refused by policy: durable rung without provenance, reserved flag set, unserved resource prefix, or a defined verb this deployment or resource does not serve (an out-of-range verb bit is instead fatal invalid_argument)
    Unsupported = 4,
    /// the per-principal pending-petition cap was reached
    Busy = 5,
}

impl Outcome {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Outcome] = &[
        Outcome::Granted,
        Outcome::Denied,
        Outcome::TimedOut,
        Outcome::Unavailable,
        Outcome::Unsupported,
        Outcome::Busy,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Outcome::Granted),
            1 => Ok(Outcome::Denied),
            2 => Ok(Outcome::TimedOut),
            3 => Ok(Outcome::Unavailable),
            4 => Ok(Outcome::Unsupported),
            5 => Ok(Outcome::Busy),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_grant",
                enum_name: "outcome",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Enum `refusal` on `vitrin_grant`.
///
/// use-time refusal codes
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Refusal {
    /// the grant is not (or not yet) active, or the verb is outside its effective set: use while pending, through an ungranted facet, or after any non-granted resolution (denied, timed_out, unavailable, unsupported, busy)
    NotGranted = 0,
    /// the grant's expiry passed; checked on use and by a proactive timer
    Expired = 1,
    /// revoked by hold-Esc, panel, or policy; effective on the very next request
    Revoked = 2,
    /// the token bucket is empty; retry_after_ms hints the refill
    RateLimited = 3,
    /// physical human input owns the target right now
    Preempted = 4,
    /// the principal's own pending petition has a prompt up; that principal's actuation is refused (never delivered to the app) until the prompt closes; other principals' grants are unaffected
    ConsentHeld = 5,
    /// the realm has no surface (its shim crashed or exited); never a stale frame
    NoSurface = 6,
    /// server-side failure during this use (renderer, memfd, delivery)
    Internal = 7,
}

impl Refusal {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Refusal] = &[
        Refusal::NotGranted,
        Refusal::Expired,
        Refusal::Revoked,
        Refusal::RateLimited,
        Refusal::Preempted,
        Refusal::ConsentHeld,
        Refusal::NoSurface,
        Refusal::Internal,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Refusal::NotGranted),
            1 => Ok(Refusal::Expired),
            2 => Ok(Refusal::Revoked),
            3 => Ok(Refusal::RateLimited),
            4 => Ok(Refusal::Preempted),
            5 => Ok(Refusal::ConsentHeld),
            6 => Ok(Refusal::NoSurface),
            7 => Ok(Refusal::Internal),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_grant",
                enum_name: "refusal",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}
