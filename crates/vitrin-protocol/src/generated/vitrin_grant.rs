// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_grant`, version 2.
//!
//! capability handle

pub const INTERFACE_NAME: &str = "vitrin_grant";
pub const INTERFACE_VERSION: u32 = 2;

pub mod requests {

    /// Request `get_launcher` (opcode 0) on `vitrin_grant`.
    ///
    /// mint the launch facet for this grant
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GetLauncher {
        /// the launch facet, born inert (confers nothing until this grant is granted with realm_launch) (new_id: vitrin_launcher)
        pub launcher: u32,
    }

    impl GetLauncher {
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
            crate::wire::write_uint(&mut out, self.launcher);
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
            let launcher = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, GetLauncher { launcher }))
        }
    }

    /// Request `get_layout_focus` (opcode 1) on `vitrin_grant`.
    ///
    /// mint the focus facet for this grant
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GetLayoutFocus {
        /// the focus facet, born inert (confers nothing until this grant is granted with layout_focus) (new_id: vitrin_layout_focus)
        pub layout_focus: u32,
    }

    impl GetLayoutFocus {
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
            crate::wire::write_uint(&mut out, self.layout_focus);
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
            let layout_focus = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, GetLayoutFocus { layout_focus }))
        }
    }

    /// Request `get_layout_arrange` (opcode 2) on `vitrin_grant`.
    ///
    /// mint the arrangement facet for this grant
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GetLayoutArrange {
        /// the arrangement facet, born inert (confers nothing until this grant is granted with layout_arrange) (new_id: vitrin_layout_arrange)
        pub layout_arrange: u32,
    }

    impl GetLayoutArrange {
        pub const OPCODE: u8 = 2;
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
            crate::wire::write_uint(&mut out, self.layout_arrange);
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
            let layout_arrange = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, GetLayoutArrange { layout_arrange }))
        }
    }

    /// Request `get_powerbox` (opcode 3) on `vitrin_grant`.
    ///
    /// mint the powerbox facet for this grant
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GetPowerbox {
        /// the powerbox facet, born inert (confers nothing until this grant is granted with designate_file) (new_id: vitrin_powerbox)
        pub powerbox: u32,
    }

    impl GetPowerbox {
        pub const OPCODE: u8 = 3;
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
            crate::wire::write_uint(&mut out, self.powerbox);
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
            let powerbox = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, GetPowerbox { powerbox }))
        }
    }

    /// Request `get_egress` (opcode 4) on `vitrin_grant`.
    ///
    /// mint the egress facet for this grant
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GetEgress {
        /// the egress facet, born inert (confers nothing until this grant is granted with egress, which no deployment does yet) (new_id: vitrin_egress)
        pub egress: u32,
    }

    impl GetEgress {
        pub const OPCODE: u8 = 4;
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
            crate::wire::write_uint(&mut out, self.egress);
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
            let egress = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((header.object_id, GetEgress { egress }))
        }
    }
}

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
    /// capture frames that include the human principal's cursor - reading the human's attention, hence a verb and not a display preference; meaningful only alongside observe, and a petition naming it without observe resolves unsupported; another agent principal's cursor is not purchasable by this or any verb; refused unsupported in version 1
    pub const OBSERVE_CURSOR: Verb = Verb(8);
    /// arrange the granted realm's view, subject to the ordering invariants no grant can purchase; exercised through vitrin_layout_arrange, which defines set_fullscreen and no other request - place, resize, raise and stacking are absent rather than refused, because a scene showing one unstacked realm cannot honour them; at most one holder per output, counting a live grant that carries this verb AND a petition still pending for it, so a second petition while either exists resolves layout_held
    pub const LAYOUT_ARRANGE: Verb = Verb(16);
    /// bind the output to a view of the granted realm and direct input there - one act, because routing keys to a realm the human cannot see is focus theft in its sharpest form; exercised through vitrin_layout_focus; separate from layout_arrange because focus theft is at once the sharpest attack and the most legitimate need, so it must be attenuable alone
    pub const LAYOUT_FOCUS: Verb = Verb(32);
    /// designate one file or one directory subtree to the granted realm, through the vitrin_powerbox facet; the human picks in a core-drawn picker and what crosses the wire is a file descriptor, never a path, so this is authority to ASK for a designation rather than authority over any named file; a delivered fd is kernel authority the core cannot recall, so revocation stops future designations and kills the grant row while the payload keeps every fd already handed over until its realm dies - PRD P2's revocation is immediate and transitive is FALSE for designations already made; refused unsupported in version 1, which cannot mint the facet at all, and by every deployment until the picker (P2.6.6) and its consent copy (P2.6.8) exist
    pub const DESIGNATE_FILE: Verb = Verb(64);
    /// open one outbound connection to the single host:port named by this grant's net: resource selector, through an out-of-core mediating proxy that asks the enforcement chokepoint per connection and holds no grant of its own; exercised through the vitrin_egress facet, which is a separate interface of its own rather than a request on the filesystem powerbox, since interface/@verb is one value per interface; the selector's grammar is wildcard-free, so a blanket egress grant is inexpressible rather than refused, and one selector covers exactly itself - though not every spelling of one endpoint is one selector, since the host is compared byte-exactly and kept as presented; SPECIFIED BUT NOT IMPLEMENTED ANYWHERE YET: a DNS name is to resolve only in the proxy and the addresses it resolved to at grant time are to be pinned into the grant row, so that a connection to an unpinned address is refused not_granted even under a name-scoped grant - no proxy, no resolver and no pinned column with a value exist today; the dotted SDK name is egress unchanged, the wire name carrying no underscore to replace; refused unsupported in version 1 and by every deployment at version 2 - the facet exists now, so what is missing is the proxy behind it rather than a request to ask through
    pub const EGRESS: Verb = Verb(128);
    /// launch the realm template this grant addresses into a new realm instance, through the vitrin_launcher facet; the template names the program and no command ever crosses the wire, so this is authority over an operator-written template rather than over an arbitrary command; bit 256 is allocated to a verb not yet defined here and was skipped rather than reused, as 64 was until designate_file landed on it and 128 was until egress did; refused unsupported in version 1, which cannot mint the facet at all, and by any deployment that does not serve it
    pub const REALM_LAUNCH: Verb = Verb(512);

    /// Union of every defined entry's bits; a wire value with any other
    /// bit set is invalid.
    pub const VALID_MASK: u32 = 1 | 2 | 4 | 8 | 16 | 32 | 64 | 128 | 512;

    /// Every defined entry as `(wire name, bit value)`, in IDL document
    /// order. `VALID_MASK` is the union of the values here; this constant
    /// adds the *names*, so a partition of the bitfield (served vs.
    /// unserved, facet-bearing vs. facetless) can be derived by name
    /// instead of transcribed into a list a human has to remember to
    /// update.
    pub const ENTRIES: &'static [(&'static str, u32)] = &[
        ("observe", 1),
        ("actuate_pointer", 2),
        ("actuate_text", 4),
        ("observe_cursor", 8),
        ("layout_arrange", 16),
        ("layout_focus", 32),
        ("designate_file", 64),
        ("egress", 128),
        ("realm_launch", 512),
    ];

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
    /// the pending-petition admission cap for this verified identity (across all of its connections) was reached
    Busy = 5,
    /// layout_arrange is already spoken for on this output, and there is at most one holder per output; the holder may be a live grant that carries the verb OR a petition still pending for it, because two petitions racing through a human's two approvals would otherwise mint two holders - so a petition that is only waiting really does hold the slot. A distinct entry rather than a reuse of busy, whose meaning is the consent-fatigue valve, and answered at admission rather than at use because contention is about who HOLDS the authority rather than about one use of it - it never reaches a prompt, so it costs the human nothing. Retrying once the holder's grant expires, is revoked, or its connection ends - or once the pending petition resolves to anything other than granted - is legal, and this outcome is the ONLY thing the core says about arbitration: choosing between two would-be holders is window-management policy and belongs outside the core
    LayoutHeld = 6,
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
        Outcome::LayoutHeld,
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
            6 => Ok(Outcome::LayoutHeld),
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
    /// the deployment is at its realm capacity, so no new realm can be created; a policy answer rather than a server-side failure, which is why it is not internal - retrying is legal once a realm exits, and retry_after_ms is 0 because the core cannot know when that will be. NOTE, a deliberate exception: every other code answers from the asking principal's OWN grant, but this one answers from deployment-wide state, so a principal holding one launch grant can poll launch and watch the answer flip - observing that SOME other principal created or exited a realm. That is a low-bandwidth cross-principal side channel, inherent to answering the question at all, and it is named here rather than left to be discovered; a deployment that cannot afford it must not serve realm_launch, because no attenuation of a launch grant removes it
    Capacity = 8,
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
        Refusal::Capacity,
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
            8 => Ok(Refusal::Capacity),
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
