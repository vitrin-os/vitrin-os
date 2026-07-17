// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_handshake`, version 1.
//!
//! principal connection bootstrap

pub const INTERFACE_NAME: &str = "vitrin_handshake";
pub const INTERFACE_VERSION: u32 = 1;

pub mod requests {

    /// Request `hello` (opcode 0) on `vitrin_handshake`.
    ///
    /// authenticate and bind a principal
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Hello {
        /// protocol version; version 1 requires exact match
        pub version: u32,
        /// principal object bound on success (new_id: vitrin_principal)
        pub principal: u32,
        /// claimed identity URI, e.g. vitrin://local/agent/demo (max 2048 bytes)
        pub identity: String,
        /// credential scheme discriminator (max 32 bytes)
        pub credential_type: String,
        /// opaque scheme-defined credential bytes (max 32768 bytes)
        pub credential: String,
    }

    impl Hello {
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
            crate::wire::write_uint(&mut out, self.version);
            crate::wire::write_uint(&mut out, self.principal);
            crate::wire::write_string(&mut out, &self.identity, 2048);
            crate::wire::write_string(&mut out, &self.credential_type, 32);
            crate::wire::write_string(&mut out, &self.credential, 32768);
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
            let version = crate::wire::read_uint(bytes, &mut pos)?;
            let principal = crate::wire::read_uint(bytes, &mut pos)?;
            let identity = crate::wire::read_string(bytes, &mut pos, 2048)?;
            let credential_type = crate::wire::read_string(bytes, &mut pos, 32)?;
            let credential = crate::wire::read_string(bytes, &mut pos, 32768)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Hello {
                    version,
                    principal,
                    identity,
                    credential_type,
                    credential,
                },
            ))
        }
    }

    /// Request `sync` (opcode 1) on `vitrin_handshake`.
    ///
    /// roundtrip barrier
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Sync {
        /// client-chosen value echoed by done
        pub cookie: u32,
    }

    impl Sync {
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
            crate::wire::write_uint(&mut out, self.cookie);
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
            let cookie = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, Sync { cookie }))
        }
    }
}

pub mod events {

    /// Event `error` (opcode 0) on `vitrin_handshake`.
    ///
    /// fatal protocol error; the connection closes
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Error {
        /// id of the object where the error occurred
        pub object_id: u32,
        /// error code, namespaced by the cited object's interface
        pub code: crate::generated::vitrin_handshake::Error,
        /// free-form debug description, never parsed (max 1024 bytes)
        pub message: String,
    }

    impl Error {
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
            crate::wire::write_uint(&mut out, self.object_id);
            crate::wire::write_uint(&mut out, self.code.to_wire());
            crate::wire::write_string(&mut out, &self.message, 1024);
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
            let object_id = crate::wire::read_uint(bytes, &mut pos)?;
            let code = crate::generated::vitrin_handshake::Error::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let message = crate::wire::read_string(bytes, &mut pos, 1024)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Error {
                    object_id,
                    code,
                    message,
                },
            ))
        }
    }

    /// Event `done` (opcode 1) on `vitrin_handshake`.
    ///
    /// barrier reply
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Done {
        /// the cookie passed to sync
        pub cookie: u32,
    }

    impl Done {
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
            crate::wire::write_uint(&mut out, self.cookie);
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
            let cookie = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, Done { cookie }))
        }
    }
}

/// Enum `error` on `vitrin_handshake`.
///
/// connection-global fatal error codes
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Error {
    /// unknown or foreign object id, id reuse at or below the watermark, reserved-range id, or multi-new_id rule violation
    InvalidObject = 0,
    /// opcode not defined for the interface at the negotiated version, including other-class opcodes and a second hello (hello's opcode is defined only in the CONNECTED state)
    InvalidOpcode = 1,
    /// argument decode failure: bad UTF-8, embedded NUL, string over its bound, out-of-range enum value, forbidden control character, zero verbs, malformed padding
    InvalidArgument = 2,
    /// declared frame size below the 8-byte header minimum, or a payload shorter than the size declares; the 65535-byte ceiling binds senders (a u16 cannot express more)
    Oversized = 3,
    /// fd count in the header disagrees with the message signature, or unsolicited fds attached
    FdViolation = 4,
    /// traffic before a first hello on a principal connection
    PreHandshake = 5,
    /// hello carried a protocol version the server does not implement; downgrade is refusal
    VersionUnsupported = 6,
    /// credential rejected: unknown identity, bad token, verifier failure, or SO_PEERCRED mismatch; the cause is never distinguished on the wire - uniform code, fixed message text, detail in the server log only
    AuthFailed = 7,
    /// server-side failure that poisoned the connection
    Internal = 8,
    /// a documented per-connection resource bound was breached: the petition-rate ceiling, the live-object cap, or object-id exhaustion; denial-of-service confinement, not a semantic judgement
    ResourceExhausted = 9,
}

impl Error {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Error] = &[
        Error::InvalidObject,
        Error::InvalidOpcode,
        Error::InvalidArgument,
        Error::Oversized,
        Error::FdViolation,
        Error::PreHandshake,
        Error::VersionUnsupported,
        Error::AuthFailed,
        Error::Internal,
        Error::ResourceExhausted,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Error::InvalidObject),
            1 => Ok(Error::InvalidOpcode),
            2 => Ok(Error::InvalidArgument),
            3 => Ok(Error::Oversized),
            4 => Ok(Error::FdViolation),
            5 => Ok(Error::PreHandshake),
            6 => Ok(Error::VersionUnsupported),
            7 => Ok(Error::AuthFailed),
            8 => Ok(Error::Internal),
            9 => Ok(Error::ResourceExhausted),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_handshake",
                enum_name: "error",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}
