// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_powerbox`, version 1.
//!
//! powerbox facet

pub const INTERFACE_NAME: &str = "vitrin_powerbox";
pub const INTERFACE_VERSION: u32 = 1;

/// Every request on this interface exercises the grant verb `designate_file`.
pub const VERB: &str = "designate_file";

pub mod requests {

    /// Request `request_file` (opcode 0) on `vitrin_powerbox`.
    ///
    /// ask the human to designate one file
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RequestFile {
        /// the access this ask is for; the human may narrow it, and designated.mode carries what was actually approved
        pub mode: crate::generated::vitrin_powerbox::Mode,
    }

    impl RequestFile {
        pub const OPCODE: u8 = 0;
        pub const HAS_FD: bool = false;
        /// First protocol version at which this message is defined (`message/@since`);
        /// this opcode is not defined on a connection whose negotiated version is
        /// lower, where using it is fatal `invalid_opcode`.
        pub const SINCE: u32 = 2;

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
            crate::wire::write_uint(&mut out, self.mode.to_wire());
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
            let mode = crate::generated::vitrin_powerbox::Mode::from_wire(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, RequestFile { mode }))
        }
    }

    /// Request `request_dir` (opcode 1) on `vitrin_powerbox`.
    ///
    /// ask the human to designate one directory subtree
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RequestDir {}

    impl RequestDir {
        pub const OPCODE: u8 = 1;
        pub const HAS_FD: bool = false;
        /// First protocol version at which this message is defined (`message/@since`);
        /// this opcode is not defined on a connection whose negotiated version is
        /// lower, where using it is fatal `invalid_opcode`.
        pub const SINCE: u32 = 2;

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
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, RequestDir {}))
        }
    }
}

pub mod events {

    /// Event `designated` (opcode 0) on `vitrin_powerbox`.
    ///
    /// the descriptor the human designated
    #[derive(Debug)]
    pub struct Designated {
        /// the designated file or directory descriptor; ownership transfers to the receiver, which MUST close it (not present in the byte buffer; carried out-of-band via SCM_RIGHTS)
        pub fd: std::os::fd::OwnedFd,
        /// the core's opaque id for this designation, matching the journal record and the realm's designation event
        pub designation_id: u32,
        /// whether the descriptor is a file or a directory subtree
        pub kind: crate::generated::vitrin_powerbox::Kind,
        /// the EFFECTIVE access the human approved, which may be narrower than the ask
        pub mode: crate::generated::vitrin_powerbox::Mode,
        /// basename of what the human chose, for display only - never a path (max 255 bytes)
        pub name: String,
    }

    impl Designated {
        pub const OPCODE: u8 = 0;
        pub const HAS_FD: bool = true;
        /// First protocol version at which this message is defined (`message/@since`);
        /// this opcode is not defined on a connection whose negotiated version is
        /// lower, where using it is fatal `invalid_opcode`.
        pub const SINCE: u32 = 2;

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
            crate::wire::write_uint(&mut out, self.designation_id);
            crate::wire::write_uint(&mut out, self.kind.to_wire());
            crate::wire::write_uint(&mut out, self.mode.to_wire());
            crate::wire::write_string(&mut out, &self.name, 255);
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
            let fd = fd.expect("fd presence already validated above");
            let designation_id = crate::wire::read_uint(bytes, &mut pos)?;
            let kind = crate::generated::vitrin_powerbox::Kind::from_wire(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let mode = crate::generated::vitrin_powerbox::Mode::from_wire(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let name = crate::wire::read_string(bytes, &mut pos, 255)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Designated {
                    fd,
                    designation_id,
                    kind,
                    mode,
                    name,
                },
            ))
        }
    }

    /// Event `refused` (opcode 1) on `vitrin_powerbox`.
    ///
    /// the picker was raised and produced no descriptor
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Refused {
        /// why the ask produced no descriptor
        pub code: crate::generated::vitrin_powerbox::Refusal,
    }

    impl Refused {
        pub const OPCODE: u8 = 1;
        pub const HAS_FD: bool = false;
        /// First protocol version at which this message is defined (`message/@since`);
        /// this opcode is not defined on a connection whose negotiated version is
        /// lower, where using it is fatal `invalid_opcode`.
        pub const SINCE: u32 = 2;

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
            crate::wire::write_uint(&mut out, self.code.to_wire());
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
            let code = crate::generated::vitrin_powerbox::Refusal::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, Refused { code }))
        }
    }
}

/// Enum `mode` on `vitrin_powerbox`.
///
/// the access a designation carries
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Mode {
    /// the descriptor is opened for reading
    Read = 0,
    /// the descriptor is opened for reading and writing, so the holder may change or truncate what it names
    ReadWrite = 1,
}

impl Mode {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Mode] = &[Mode::Read, Mode::ReadWrite];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Mode::Read),
            1 => Ok(Mode::ReadWrite),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_powerbox",
                enum_name: "mode",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Enum `kind` on `vitrin_powerbox`.
///
/// what a designated descriptor names
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Kind {
    /// a single file
    File = 0,
    /// a directory, designating the whole subtree beneath it as one descriptor
    Directory = 1,
}

impl Kind {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Kind] = &[Kind::File, Kind::Directory];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Kind::File),
            1 => Ok(Kind::Directory),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_powerbox",
                enum_name: "kind",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Enum `refusal` on `vitrin_powerbox`.
///
/// why a raised picker produced no descriptor
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Refusal {
    /// the human dismissed the picker without choosing; the ordinary answer, and asking again later is legal
    Cancelled = 0,
    /// the picker was raised and expired unanswered, on the deployment's own deadline; distinct from cancelled because nobody decided anything
    TimedOut = 1,
    /// a picker for this principal is already up; at most one at a time, because two stacked in front of one human is the consent-fatigue shape the busy petition outcome already names
    Busy = 2,
    /// the human chose, and the core would not designate it: the entry could not be resolved without following a symlink or losing a race between the confirmation and the open, so the core refuses rather than delivering a descriptor that may not name what the human saw; says nothing about whether the entry exists
    Unresolvable = 3,
}

impl Refusal {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Refusal] = &[
        Refusal::Cancelled,
        Refusal::TimedOut,
        Refusal::Busy,
        Refusal::Unresolvable,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Refusal::Cancelled),
            1 => Ok(Refusal::TimedOut),
            2 => Ok(Refusal::Busy),
            3 => Ok(Refusal::Unresolvable),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_powerbox",
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
