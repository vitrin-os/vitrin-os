// SPDX-License-Identifier: Apache-2.0
//! Intermediate representation of the Vitrin wire protocol.
//!
//! This module is the losslessly-parsed, structured form of `protocol/vitrin-v0.xml`
//! that every codegen backend (Rust today, a future C backend) consumes. Backends
//! must never re-parse the XML themselves -- everything a backend needs (opcodes,
//! resolved enum references, string bounds, fd positions, ...) is already resolved
//! here by `parse.rs`.
//!
//! Field order in every `Vec` here is document order. That is load-bearing, not
//! cosmetic: opcode assignment IS document order (the Nth request/event, 0-indexed,
//! independently per kind), and re-running the scanner must be byte-for-byte
//! idempotent, which requires stable iteration order everywhere.

/// The whole protocol document.
#[derive(Debug, Clone)]
pub struct Protocol {
    /// The `protocol/@name` attribute (e.g. "vitrin").
    pub name: String,
    /// The `protocol/@version` attribute: the single wire version integer
    /// (also the first argument of `vitrin_handshake.hello`).
    pub version: u32,
    /// Interfaces in document order.
    pub interfaces: Vec<Interface>,
}

impl Protocol {
    /// Find an interface by name. Used to resolve dotted `enum="iface.name"`
    /// references and cross-interface arg types.
    pub fn interface(&self, name: &str) -> Option<&Interface> {
        self.interfaces.iter().find(|i| i.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    /// The interface's own `version` attribute (per-interface growth counter;
    /// distinct from the protocol-level version integer).
    pub version: u32,
    /// `interface/@verb`, present only on `vitrin_view`, `vitrin_actuator_pointer`,
    /// and `vitrin_actuator_text`: every request on the interface exercises this
    /// grant verb. Carried through for a future capability-enforcement chokepoint
    /// in a different crate; no enforcement logic here.
    pub verb: Option<String>,
    pub summary: String,
    /// Requests in document order; opcode == index into this `Vec`.
    pub requests: Vec<Message>,
    /// Events in document order; opcode == index into this `Vec`.
    ///
    /// May be empty (no special-casing needed): `vitrin_realm`, `vitrin_grant`,
    /// and `vitrin_actuator_pointer`/`vitrin_actuator_text` etc. each have zero
    /// requests or zero events; `vitrin_shim_seat` defines zero requests and only
    /// events, every one of which ends with an `origin` argument.
    pub events: Vec<Message>,
    /// Enums defined on this interface, document order.
    pub enums: Vec<EnumDef>,
}

impl Interface {
    pub fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.name == name)
    }
}

/// One request or event. `Interface::requests` and `Interface::events` each carry
/// their own `Message` list; opcodes are independent 0-based sequences per list.
#[derive(Debug, Clone)]
pub struct Message {
    pub name: String,
    /// 0-indexed position within its own kind (requests or events) in document
    /// order. Requests and events are numbered independently from 0.
    pub opcode: u8,
    /// The first protocol version at which this message is defined
    /// (`message/@since`), defaulting to 1 when the attribute is absent.
    ///
    /// This is the protocol *document* version, not the interface's own
    /// `version` counter: `docs/protocol/00-conventions.md` Appendix A names
    /// every growth seam as a `since="2"` message on an interface whose own
    /// version attribute tracks that interface's growth separately.
    ///
    /// A connection speaks exactly its negotiated version for its whole life,
    /// so an opcode whose `since` is above that version is not defined on it
    /// and is fatal `invalid_opcode`. Both backends emit this so a dispatcher
    /// can make that check from the generated table rather than a hand-kept
    /// list.
    pub since: u32,
    pub summary: String,
    /// Arguments in document order (also the wire order).
    pub args: Vec<Arg>,
}

impl Message {
    /// True if any argument is fd-typed. At most one fd argument exists on any
    /// single message in this IDL (a framing invariant, not merely incidental).
    pub fn has_fd(&self) -> bool {
        self.args.iter().any(|a| matches!(a.ty, ArgType::Fd))
    }
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: String,
    pub summary: String,
    pub ty: ArgType,
    /// `allow-null="true"` in the XML. Schema-legal only on `string`/`object`
    /// arguments (never on `new_id`, which always names a fresh, non-null id).
    pub allow_null: bool,
}

/// The closed set of seven wire argument types.
#[derive(Debug, Clone)]
pub enum ArgType {
    /// Signed 32-bit, little-endian. `enum_ref`, if present, names the enum
    /// this value is drawn from (already resolved to a concrete interface).
    /// No `int` argument in v0.xml carries an `enum` reference (only `uint`
    /// ones do), but the schema permits it on either scalar type, so this
    /// isn't hardcoded away.
    Int { enum_ref: Option<EnumRef> },
    /// Unsigned 32-bit, little-endian. `enum_ref`, if present, names the enum
    /// this value is drawn from (already resolved to a concrete interface).
    Uint { enum_ref: Option<EnumRef> },
    /// Signed 24.8 fixed-point, wire-encoded as a raw `i32`.
    Fixed,
    /// `u32` byte length + UTF-8 bytes (no NUL terminator), zero-padded to 4
    /// bytes. `max_bytes` is extracted from the `(max N bytes)` token in the
    /// arg's `summary` attribute -- every string arg in this IDL has one; a
    /// missing token is a scanner error (see `parse.rs`), never a silent default.
    String { max_bytes: u32 },
    /// `u32` object id; legal as `0` (null) only when `allow_null` is set.
    /// No argument in v0.xml has this type, but the codec must support it
    /// generically since the schema allows it and a future version may use it.
    Object { interface: String },
    /// `u32` id the sender allocates for a new object of the named interface.
    /// Never nullable (the schema disallows `allow-null` on `new_id`).
    NewId { interface: String },
    /// Not present in the byte buffer at all: carried out-of-band via
    /// `SCM_RIGHTS`, matched to the signature positionally. The codec neither
    /// reads nor writes fd bytes into the frame body.
    Fd,
}

/// A resolved reference to an enum, from an arg's `enum="..."` attribute.
///
/// A bare name (`enum="origin"`) resolves to an enum defined on the arg's own
/// interface; a dotted name (`enum="vitrin_grant.verb"`) resolves to an enum
/// defined on the named interface. This struct is always the *resolved* form --
/// `interface` is concrete either way, never "the referencing arg's interface"
/// left implicit for a consumer to re-derive.
#[derive(Debug, Clone)]
pub struct EnumRef {
    pub interface: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    /// `bitfield="true"`: valid wire values are any combination of the defined
    /// entries' bits (validated as a bitmask -- bits outside the union of
    /// defined values are rejected). Absent/false: the wire value MUST exactly
    /// equal one defined entry (whole-value membership).
    pub bitfield: bool,
    pub summary: String,
    /// Entries in document order. Values are decimal or 0x-hex in the XML;
    /// already parsed to `u32` here (not necessarily small/sequential --
    /// `vitrin_view.format`'s entries are DRM fourcc codes).
    pub entries: Vec<EnumEntry>,
}

#[derive(Debug, Clone)]
pub struct EnumEntry {
    pub name: String,
    pub value: u32,
    pub summary: String,
}
