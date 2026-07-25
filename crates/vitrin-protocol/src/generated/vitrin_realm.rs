// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

//! Interface `vitrin_realm`, version 1.
//!
//! realm address

pub const INTERFACE_NAME: &str = "vitrin_realm";
pub const INTERFACE_VERSION: u32 = 1;

pub mod requests {

    /// Request `request_grant` (opcode 0) on `vitrin_realm`.
    ///
    /// petition for authority over this realm
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RequestGrant {
        /// the grant handle, born pending (new_id: vitrin_grant)
        pub grant: u32,
        /// prompt-visibility observer for this petition (new_id: vitrin_consent)
        pub consent: u32,
        /// observation facet (inert until granted with observe) (new_id: vitrin_view)
        pub view: u32,
        /// pointer facet (inert until granted with actuate_pointer) (new_id: vitrin_actuator_pointer)
        pub pointer: u32,
        /// text facet (inert until granted with actuate_text) (new_id: vitrin_actuator_text)
        pub text: u32,
        /// resource selector within the realm; null or empty = whole realm (max 256 bytes)
        pub resource: String,
        /// requested verb set; MUST be non-zero
        pub verbs: crate::generated::vitrin_grant::Verb,
        /// requested lifetime in milliseconds; 0 = bounded by the persistence rung
        pub expiry_ms: u32,
        /// requested ceiling in events per second for observation and actuation; 0 = server default, never unlimited
        pub max_event_rate: u32,
        /// requested persistence rung
        pub persistence: crate::generated::vitrin_grant::Persistence,
        /// boolean constraint bits; MUST be 0 in version 1 (bit 0 reserved: one_shot)
        pub flags: u32,
    }

    impl RequestGrant {
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
            crate::wire::write_uint(&mut out, self.grant);
            crate::wire::write_uint(&mut out, self.consent);
            crate::wire::write_uint(&mut out, self.view);
            crate::wire::write_uint(&mut out, self.pointer);
            crate::wire::write_uint(&mut out, self.text);
            crate::wire::write_string(&mut out, &self.resource, 256);
            crate::wire::write_uint(&mut out, self.verbs.bits());
            crate::wire::write_uint(&mut out, self.expiry_ms);
            crate::wire::write_uint(&mut out, self.max_event_rate);
            crate::wire::write_uint(&mut out, self.persistence.to_wire());
            crate::wire::write_uint(&mut out, self.flags);
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
            let grant = crate::wire::read_uint(bytes, &mut pos)?;
            let consent = crate::wire::read_uint(bytes, &mut pos)?;
            let view = crate::wire::read_uint(bytes, &mut pos)?;
            let pointer = crate::wire::read_uint(bytes, &mut pos)?;
            let text = crate::wire::read_uint(bytes, &mut pos)?;
            let resource = crate::wire::read_string(bytes, &mut pos, 256)?;
            let verbs = crate::generated::vitrin_grant::Verb::from_bits(crate::wire::read_uint(
                bytes, &mut pos,
            )?)?;
            let expiry_ms = crate::wire::read_uint(bytes, &mut pos)?;
            let max_event_rate = crate::wire::read_uint(bytes, &mut pos)?;
            let persistence = crate::generated::vitrin_grant::Persistence::from_wire(
                crate::wire::read_uint(bytes, &mut pos)?,
            )?;
            let flags = crate::wire::read_uint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(crate::error::DecodeError::TrailingBytes {
                    consumed: pos,
                    total: bytes.len(),
                });
            }
            Ok((
                header.object_id,
                RequestGrant {
                    grant,
                    consent,
                    view,
                    pointer,
                    text,
                    resource,
                    verbs,
                    expiry_ms,
                    max_event_rate,
                    persistence,
                    flags,
                },
            ))
        }
    }
}
