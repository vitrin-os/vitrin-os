// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_shim_surface`, version 1.
//!
//! shim-to-core buffer path

pub const INTERFACE_NAME: &str = "vitrin_shim_surface";
pub const INTERFACE_VERSION: u32 = 1;

pub mod requests {

    /// Request `attach` (opcode 0) on `vitrin_shim_surface`.
    ///
    /// stage a buffer as pending
    #[derive(Debug)]
    pub struct Attach {
        /// shim-chosen cookie identifying this buffer
        pub buffer_id: u32,
        /// memfd (kind shm) or dmabuf (kind dmabuf) (not present in the byte buffer; carried out-of-band via SCM_RIGHTS)
        pub fd: std::os::fd::OwnedFd,
        /// what the fd is
        pub kind: crate::generated::vitrin_shim_surface::Kind,
        /// pixel format (DRM fourcc value)
        pub format: crate::generated::vitrin_view::Format,
        /// buffer width in pixels
        pub width: u32,
        /// buffer height in pixels
        pub height: u32,
        /// row stride in bytes
        pub stride: u32,
    }

    impl Attach {
        pub const OPCODE: u8 = 0;
        pub const HAS_FD: bool = true;

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
            crate::wire::write_uint(&mut out, self.buffer_id);
            crate::wire::write_uint(&mut out, self.kind.to_wire());
            crate::wire::write_uint(&mut out, self.format.to_wire());
            crate::wire::write_uint(&mut out, self.width);
            crate::wire::write_uint(&mut out, self.height);
            crate::wire::write_uint(&mut out, self.stride);
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
            let buffer_id = crate::wire::read_uint(bytes, &mut pos)?;
            let fd = fd.expect("fd presence already validated above");
            let kind = crate::generated::vitrin_shim_surface::Kind::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let format = crate::generated::vitrin_view::Format::from_wire(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let width = crate::wire::read_uint(bytes, &mut pos)?;
            let height = crate::wire::read_uint(bytes, &mut pos)?;
            let stride = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Attach {
                    buffer_id,
                    fd,
                    kind,
                    format,
                    width,
                    height,
                    stride,
                },
            ))
        }
    }

    /// Request `damage` (opcode 1) on `vitrin_shim_surface`.
    ///
    /// add a pending damage rectangle
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Damage {
        /// rectangle x in buffer coordinates
        pub x: i32,
        /// rectangle y in buffer coordinates
        pub y: i32,
        /// rectangle width
        pub width: i32,
        /// rectangle height
        pub height: i32,
    }

    impl Damage {
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
            crate::wire::write_int(&mut out, self.x);
            crate::wire::write_int(&mut out, self.y);
            crate::wire::write_int(&mut out, self.width);
            crate::wire::write_int(&mut out, self.height);
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
            let x = crate::wire::read_int(bytes, &mut pos)?;
            let y = crate::wire::read_int(bytes, &mut pos)?;
            let width = crate::wire::read_int(bytes, &mut pos)?;
            let height = crate::wire::read_int(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                Damage {
                    x,
                    y,
                    width,
                    height,
                },
            ))
        }
    }

    /// Request `commit` (opcode 2) on `vitrin_shim_surface`.
    ///
    /// atomically apply pending state
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Commit {}

    impl Commit {
        pub const OPCODE: u8 = 2;
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
            Ok((header.object_id, Commit {}))
        }
    }
}

pub mod events {

    /// Event `frame_done` (opcode 0) on `vitrin_shim_surface`.
    ///
    /// frame pacing callback
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FrameDone {
        /// presentation time in milliseconds, monotonic domain
        pub time_ms: u32,
    }

    impl FrameDone {
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
            crate::wire::write_uint(&mut out, self.time_ms);
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
            let time_ms = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, FrameDone { time_ms }))
        }
    }

    /// Event `buffer_done` (opcode 1) on `vitrin_shim_surface`.
    ///
    /// buffer ownership returns (exactly once per attach)
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BufferDone {
        /// the cookie given in attach
        pub buffer_id: u32,
        /// disposition of that attach
        pub status: crate::generated::vitrin_shim_surface::BufferStatus,
    }

    impl BufferDone {
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
            crate::wire::write_uint(&mut out, self.buffer_id);
            crate::wire::write_uint(&mut out, self.status.to_wire());
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
            let buffer_id = crate::wire::read_uint(bytes, &mut pos)?;
            let status = crate::generated::vitrin_shim_surface::BufferStatus::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, BufferDone { buffer_id, status }))
        }
    }
}

/// Enum `kind` on `vitrin_shim_surface`.
///
/// attached fd kinds
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Kind {
    /// memfd; the core copies pixels in at commit
    Shm = 0,
    /// dmabuf; the core imports it zero-copy, single-plane, linear modifier implied
    Dmabuf = 1,
}

impl Kind {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Kind] = &[Kind::Shm, Kind::Dmabuf];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Kind::Shm),
            1 => Ok(Kind::Dmabuf),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_shim_surface",
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

/// Enum `buffer_status` on `vitrin_shim_surface`.
///
/// attach dispositions
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum BufferStatus {
    /// the buffer was used (or superseded) and may be reused
    Released = 0,
    /// dmabuf import failed; fall back to shm; buffer unused and released
    ImportFailed = 1,
    /// format not usable by the renderer; fall back to shm; buffer unused and released
    FormatUnsupported = 2,
    /// buffer exceeds the renderer's limits; fall back to shm; buffer unused and released
    TooLarge = 3,
}

impl BufferStatus {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [BufferStatus] = &[
        BufferStatus::Released,
        BufferStatus::ImportFailed,
        BufferStatus::FormatUnsupported,
        BufferStatus::TooLarge,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(BufferStatus::Released),
            1 => Ok(BufferStatus::ImportFailed),
            2 => Ok(BufferStatus::FormatUnsupported),
            3 => Ok(BufferStatus::TooLarge),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_shim_surface",
                enum_name: "buffer_status",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}
