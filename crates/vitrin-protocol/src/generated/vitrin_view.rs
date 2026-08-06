// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_view`, version 1.
//!
//! observation facet (poll-model capture)

pub const INTERFACE_NAME: &str = "vitrin_view";
pub const INTERFACE_VERSION: u32 = 1;

/// Every request on this interface exercises the grant verb `observe`.
pub const VERB: &str = "observe";

pub mod requests {

    /// Request `capture_frame` (opcode 0) on `vitrin_view`.
    ///
    /// request one frame of the realm view
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CaptureFrame {}

    impl CaptureFrame {
        pub const OPCODE: u8 = 0;
        pub const HAS_FD: bool = false;
        /// First protocol version at which this message is defined (`message/@since`);
        /// this opcode is not defined on a connection whose negotiated version is
        /// lower, where using it is fatal `invalid_opcode`.
        pub const SINCE: u32 = 1;

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
            Ok((header.object_id, CaptureFrame {}))
        }
    }
}

pub mod events {

    /// Event `frame_ready` (opcode 0) on `vitrin_view`.
    ///
    /// one captured frame
    #[derive(Debug)]
    pub struct FrameReady {
        /// fresh memfd holding the frame; ownership transfers to the receiver (not present in the byte buffer; carried out-of-band via SCM_RIGHTS)
        pub fd: std::os::fd::OwnedFd,
        /// pixel format (DRM fourcc value)
        pub format: crate::generated::vitrin_view::Format,
        /// frame width in pixels
        pub width: u32,
        /// frame height in pixels
        pub height: u32,
        /// row stride in bytes; equals width * 4 in version 1
        pub stride: u32,
        /// frame flags; always 0 in version 1
        pub flags: crate::generated::vitrin_view::FrameFlags,
    }

    impl FrameReady {
        pub const OPCODE: u8 = 0;
        pub const HAS_FD: bool = true;
        /// First protocol version at which this message is defined (`message/@since`);
        /// this opcode is not defined on a connection whose negotiated version is
        /// lower, where using it is fatal `invalid_opcode`.
        pub const SINCE: u32 = 1;

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
            crate::wire::write_uint(&mut out, self.format.to_wire());
            crate::wire::write_uint(&mut out, self.width);
            crate::wire::write_uint(&mut out, self.height);
            crate::wire::write_uint(&mut out, self.stride);
            crate::wire::write_uint(&mut out, self.flags.bits());
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
            let format = crate::generated::vitrin_view::Format::from_wire(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let width = crate::wire::read_uint(bytes, &mut pos)?;
            let height = crate::wire::read_uint(bytes, &mut pos)?;
            let stride = crate::wire::read_uint(bytes, &mut pos)?;
            let flags = crate::generated::vitrin_view::FrameFlags::from_bits(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                FrameReady {
                    fd,
                    format,
                    width,
                    height,
                    stride,
                    flags,
                },
            ))
        }
    }
}

/// Enum `format` on `vitrin_view`.
///
/// pixel formats (DRM fourcc values)
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Format {
    /// 32-bit xRGB, DRM_FORMAT_XRGB8888
    Xrgb8888 = 0x34325258,
    /// 32-bit ARGB, DRM_FORMAT_ARGB8888
    Argb8888 = 0x34325241,
}

impl Format {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Format] = &[Format::Xrgb8888, Format::Argb8888];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0x34325258 => Ok(Format::Xrgb8888),
            0x34325241 => Ok(Format::Argb8888),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_view",
                enum_name: "format",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Enum `frame_flags` on `vitrin_view` (bitfield).
///
/// frame flags (reserved in version 1)
///
/// Bitfield: any combination of the defined entries' bits is a legal wire
/// value; a bit outside their union is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FrameFlags(u32);

impl FrameFlags {
    /// rows are bottom-up (reserved; never set in version 1)
    pub const Y_INVERT: FrameFlags = FrameFlags(1);
    /// fd is a dmabuf, not a memfd (reserved; never set in version 1)
    pub const DMABUF: FrameFlags = FrameFlags(2);

    /// Union of every defined entry's bits; a wire value with any other
    /// bit set is invalid.
    pub const VALID_MASK: u32 = 1 | 2;

    /// Decode a wire value, rejecting any bit outside `VALID_MASK`.
    pub fn from_bits(value: u32) -> Result<Self, crate::error::DecodeError> {
        if value & !Self::VALID_MASK != 0 {
            Err(crate::error::DecodeError::InvalidBitfieldValue {
                interface: "vitrin_view",
                enum_name: "frame_flags",
                value,
            })
        } else {
            Ok(FrameFlags(value))
        }
    }

    /// The raw wire bitmask.
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Whether every bit set in `other` is also set in `self`.
    pub fn contains(self, other: FrameFlags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for FrameFlags {
    type Output = FrameFlags;

    fn bitor(self, rhs: FrameFlags) -> FrameFlags {
        FrameFlags(self.0 | rhs.0)
    }
}
