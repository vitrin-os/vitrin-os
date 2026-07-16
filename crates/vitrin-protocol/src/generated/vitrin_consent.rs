// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_consent`, version 1.
//!
//! consent prompt visibility (events only)

pub const INTERFACE_NAME: &str = "vitrin_consent";
pub const INTERFACE_VERSION: u32 = 1;

pub mod events {

    /// Event `state` (opcode 0) on `vitrin_consent`.
    ///
    /// prompt lifecycle transition
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct State {
        /// the new prompt state
        pub state: crate::generated::vitrin_consent::ConsentState,
    }

    impl State {
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
            crate::wire::write_uint(&mut out, self.state.to_wire());
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
            let state = crate::generated::vitrin_consent::ConsentState::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, State { state }))
        }
    }
}

/// Enum `consent_state` on `vitrin_consent`.
///
/// prompt visibility states
///
/// Plain enum: a wire value MUST exactly equal one defined entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ConsentState {
    /// waiting behind another prompt or a policy decision
    Queued = 0,
    /// visible; physical input is grabbed by the prompt
    Shown = 1,
    /// gone; the decision arrives on the grant
    Closed = 2,
}

impl ConsentState {
    /// Every defined entry, in document order. Lets generic code (property
    /// tests, a future C backend) enumerate valid values without hardcoding
    /// them, so an appended entry can never be silently missed.
    pub const ALL: &'static [ConsentState] = &[
        ConsentState::Queued,
        ConsentState::Shown,
        ConsentState::Closed,
    ];

    /// Decode a wire value, by whole-value membership in the defined entries.
    pub fn from_wire(value: u32) -> Result<Self, crate::error::DecodeError> {
        match value {
            0 => Ok(ConsentState::Queued),
            1 => Ok(ConsentState::Shown),
            2 => Ok(ConsentState::Closed),
            _ => Err(crate::error::DecodeError::InvalidEnumValue {
                interface: "vitrin_consent",
                enum_name: "consent_state",
                value,
            }),
        }
    }

    /// The wire value for this entry.
    pub fn to_wire(self) -> u32 {
        self as u32
    }
}
