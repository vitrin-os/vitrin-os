// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_shim_seat`, version 1.
//!
//! input delivery to the shim (events only, origin-tagged)

pub const INTERFACE_NAME: &str = "vitrin_shim_seat";
pub const INTERFACE_VERSION: u32 = 1;

pub mod events {

    /// Event `motion` (opcode 0) on `vitrin_shim_seat`.
    ///
    /// pointer moved
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Motion {
        /// realm-view x
        pub x: crate::fixed::Fixed,
        /// realm-view y
        pub y: crate::fixed::Fixed,
        /// who caused this event
        pub origin: crate::generated::vitrin_shim_seat::Origin,
    }

    impl Motion {
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
            crate::wire::write_int(&mut out, self.x.to_bits());
            crate::wire::write_int(&mut out, self.y.to_bits());
            crate::wire::write_uint(&mut out, self.origin.to_wire());
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
            let x = crate::fixed::Fixed::from_bits(crate::wire::read_int(bytes, &mut pos)?);
            let y = crate::fixed::Fixed::from_bits(crate::wire::read_int(bytes, &mut pos)?);
            let origin = crate::generated::vitrin_shim_seat::Origin::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, Motion { x, y, origin }))
        }
    }

    /// Event `button` (opcode 1) on `vitrin_shim_seat`.
    ///
    /// pointer button
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Button {
        /// Linux evdev button code
        pub button: u32,
        /// pressed or released
        pub state: crate::generated::vitrin_actuator_pointer::ButtonState,
        /// who caused this event
        pub origin: crate::generated::vitrin_shim_seat::Origin,
    }

    impl Button {
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
            crate::wire::write_uint(&mut out, self.button);
            crate::wire::write_uint(&mut out, self.state.to_wire());
            crate::wire::write_uint(&mut out, self.origin.to_wire());
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
            let button = crate::wire::read_uint(bytes, &mut pos)?;
            let state = crate::generated::vitrin_actuator_pointer::ButtonState::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let origin = crate::generated::vitrin_shim_seat::Origin::from_wire(
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
                Button {
                    button,
                    state,
                    origin,
                },
            ))
        }
    }

    /// Event `scroll` (opcode 2) on `vitrin_shim_seat`.
    ///
    /// scroll
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Scroll {
        /// scroll axis
        pub axis: crate::generated::vitrin_actuator_pointer::Axis,
        /// scroll amount; one notch = +-120
        pub value120: i32,
        /// who caused this event
        pub origin: crate::generated::vitrin_shim_seat::Origin,
    }

    impl Scroll {
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
            crate::wire::write_uint(&mut out, self.axis.to_wire());
            crate::wire::write_int(&mut out, self.value120);
            crate::wire::write_uint(&mut out, self.origin.to_wire());
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
            let axis = crate::generated::vitrin_actuator_pointer::Axis::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let value120 = crate::wire::read_int(bytes, &mut pos)?;
            let origin = crate::generated::vitrin_shim_seat::Origin::from_wire(
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
                Scroll {
                    axis,
                    value120,
                    origin,
                },
            ))
        }
    }

    /// Event `key` (opcode 3) on `vitrin_shim_seat`.
    ///
    /// key press or release (keysym)
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Key {
        /// xkbcommon keysym, modifier-resolved
        pub keysym: u32,
        /// pressed or released
        pub state: crate::generated::vitrin_shim_seat::KeyState,
        /// who caused this event
        pub origin: crate::generated::vitrin_shim_seat::Origin,
    }

    impl Key {
        pub const OPCODE: u8 = 3;
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
            crate::wire::write_uint(&mut out, self.keysym);
            crate::wire::write_uint(&mut out, self.state.to_wire());
            crate::wire::write_uint(&mut out, self.origin.to_wire());
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
            let keysym = crate::wire::read_uint(bytes, &mut pos)?;
            let state = crate::generated::vitrin_shim_seat::KeyState::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let origin = crate::generated::vitrin_shim_seat::Origin::from_wire(
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
                Key {
                    keysym,
                    state,
                    origin,
                },
            ))
        }
    }

    /// Event `text` (opcode 4) on `vitrin_shim_seat`.
    ///
    /// deliver a Unicode string
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Text {
        /// UTF-8 text to deliver (max 4096 bytes)
        pub text: String,
        /// who caused this event
        pub origin: crate::generated::vitrin_shim_seat::Origin,
    }

    impl Text {
        pub const OPCODE: u8 = 4;
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
            crate::wire::write_string(&mut out, &self.text, 4096);
            crate::wire::write_uint(&mut out, self.origin.to_wire());
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
            let text = crate::wire::read_string(bytes, &mut pos, 4096)?;
            let origin = crate::generated::vitrin_shim_seat::Origin::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, Text { text, origin }))
        }
    }
}

/// Enum `key_state` on `vitrin_shim_seat`.
///
/// key states
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum KeyState {
    /// key released
    Released = 0,
    /// key pressed
    Pressed = 1,
}

impl KeyState {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [KeyState] = &[KeyState::Released, KeyState::Pressed];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(KeyState::Released),
            1 => Ok(KeyState::Pressed),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_shim_seat",
                enum_name: "key_state",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}

/// Enum `origin` on `vitrin_shim_seat`.
///
/// input origin (physical versus emulated)
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Origin {
    /// produced by a physical human input device
    Physical = 0,
    /// produced by a principal's actuator
    Emulated = 1,
}

impl Origin {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [Origin] = &[Origin::Physical, Origin::Emulated];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(Origin::Physical),
            1 => Ok(Origin::Emulated),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_shim_seat",
                enum_name: "origin",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}
