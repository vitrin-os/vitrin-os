// SPDX-License-Identifier: Apache-2.0
//! C codegen backend: IR -> `shim/include/vitrin-protocol.h`.
//!
//! Emits a single self-contained, header-only C11 header (every function is
//! `static inline`; there is no companion `.c` file) with marshal helpers for
//! every message in `protocol/vitrin-v0.xml`, for the wlroots shim (a
//! separate tracked task) to consume. Starts with [`BANNER`] (source XML +
//! regeneration command, no timestamp -- idempotency depends on it) and a
//! longer top-of-file rationale comment ([`TOP_COMMENT`]) covering the two
//! deliberate asymmetries with the generated Rust side: the borrowed
//! `vitrin_string_t` view (never a NUL-terminated `char *`) and fd arguments
//! never touching the byte buffer.
//!
//! ## Two-phase structure (why this file is not one section per interface)
//!
//! The generated Rust side nests everything -- requests, events, enums --
//! under one `pub mod <interface>`, in document order, because Rust items
//! resolve by name regardless of textual order. C has no such luxury: a
//! `struct` member of enum type requires that enum's type to be *complete*
//! (fully defined, not just forward-declared) at the point of use, and
//! `vitrin_realm.request_grant` (interface `vitrin_realm`, document position
//! 3) references two enums defined on `vitrin_grant` (document position 4) --
//! a genuine forward reference in document order. A single interleaved pass
//! emitting each interface's enums-then-messages in turn would put
//! `vitrin_realm`'s message struct before `vitrin_grant`'s enum definitions
//! it needs, which does not compile.
//!
//! So this backend emits in two phases instead: **every** enum, across
//! **every** interface (Section 1), before **any** message struct (Section
//! 2). Document order is still preserved *within* each phase (interfaces in
//! document order; within an interface, enums/requests/events in document
//! order) -- only the phase split is new structure, not a reordering of any
//! one kind of item relative to its siblings. Message structs never
//! reference each other (no message-typed argument exists in this wire
//! format), so Section 2 has no analogous ordering hazard.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::casing::to_screaming_snake;
use crate::gen_util::{doc_text, format_u32_literal, Buf};
use crate::ir::{Arg, ArgType, EnumDef, EnumRef, Interface, Message, Protocol};

/// Banner every generated file starts with. Deliberately free of timestamps
/// or any other per-run-varying content: idempotency (running the generator
/// twice produces byte-identical output) depends on it. Kept textually
/// identical to `rust_gen::BANNER`'s content (differs only in comment
/// syntax, which is the same `//` in both languages) so a reader sees the
/// same provenance notice regardless of which generated file they're in --
/// including the SPDX tag, which is `Apache-2.0` for the same reason on both
/// sides (D-005 / issue #133: a mechanical transliteration of the Apache-2.0
/// `protocol/vitrin-v0.xml` stays Apache-2.0, so the one generated header
/// under `shim/` is carved out of the shim's MPL-2.0 sources by name and any
/// third-party C client can `#include` it without touching copyleft code).
const BANNER: &str = "\
// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen
";

/// The header's top-of-file rationale comment: what this file is, and the
/// two deliberate asymmetries with the generated Rust side (string
/// representation, fd handling) that the issue asked to be stated briefly
/// here.
const TOP_COMMENT: &str = "\
// vitrin-protocol.h -- Vitrin OS wire protocol: C structs and marshal
// helpers, for the wlroots shim (shim/src/*.c, a separate tracked task) to
// consume.
//
// Self-contained and header-only: every function below is `static inline`
// and there is no companion .c file, so this header may be `#include`d from
// multiple translation units with no link errors. It performs no I/O and no
// allocation -- every `*_encode` writes into a caller-supplied buffer and
// every `*_decode` reads from one, full stop.
//
// STRING ARGUMENTS are represented by `vitrin_string_t`: a borrowed,
// non-owning view (a `uint32_t` byte length plus a `const uint8_t *`
// pointer) -- never a NUL-terminated `char *`. This is a deliberate
// asymmetry with the generated Rust side, which owns `String`/`Vec<u8>` for
// ergonomics: the wire format itself forbids relying on a NUL terminator
// (the byte length is authoritative; a well-formed peer never embeds a NUL,
// but nothing about the bytes on the wire stops a hostile one from trying)
// and allows arbitrary content up to that declared length. A
// NUL-terminated representation would either lie about the true length or
// require an allocation to hold a defensive copy -- a length+pointer view
// needs neither: on decode it borrows directly into the caller-supplied
// input buffer, and on encode the caller borrows it from wherever their own
// bytes already live. Ownership and lifetime of the pointed-to bytes are
// always the caller's problem, never this header's.
//
// FD ARGUMENTS are never part of the byte buffer on either side, matching
// the wire format (`fd` is transferred out-of-band via `SCM_RIGHTS`,
// matched to the signature positionally, and is not in the frame body at
// all). A message with an `fd` argument carries a plain `int` field for it;
// `*_encode` never touches that field (the caller must send the fd
// out-of-band alongside the returned bytes) and `*_decode` takes the
// received fd as an explicit `int fd` parameter (`-1` if none) on *every*
// message, not only fd-bearing ones, so a signature/fd-presence mismatch is
// always checkable -- mirroring the generated Rust side's uniform
// `Option<OwnedFd>` decode parameter. Real `SCM_RIGHTS` socket handling
// belongs to the future shim code, out of scope here.
";

/// Which of a message's two independently-numbered opcode spaces it's in.
/// Threaded through every C symbol as a `req`/`evt` infix (see
/// [`msg_c_base`]) so a request and event can never collide in C's single
/// flat namespace even if they shared a name -- e.g. `vitrin_handshake`
/// defines both an `error` *event* and an `error` *enum*; the event becomes
/// `vitrin_handshake_evt_error_t` (enums never carry a kind infix), so it
/// cannot collide with the enum's `vitrin_handshake_error_t` either.
#[derive(Clone, Copy)]
enum CMsgKind {
    Request,
    Event,
}

impl CMsgKind {
    fn infix(self) -> &'static str {
        match self {
            CMsgKind::Request => "req",
            CMsgKind::Event => "evt",
        }
    }

    fn label(self) -> &'static str {
        match self {
            CMsgKind::Request => "Request",
            CMsgKind::Event => "Event",
        }
    }
}

/// Generate `shim/include/vitrin-protocol.h` (or wherever `out_path` points)
/// from `protocol`, creating parent directories as needed.
pub fn generate(protocol: &Protocol, out_path: &Path) -> Result<()> {
    let contents = header_contents(protocol)?;
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    fs::write(out_path, contents).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(())
}

fn header_contents(protocol: &Protocol) -> Result<String> {
    let mut buf = Buf::default();
    buf.line(BANNER);
    buf.line(TOP_COMMENT);
    buf.line("#ifndef VITRIN_PROTOCOL_H");
    buf.line("#define VITRIN_PROTOCOL_H");
    buf.blank();
    buf.line("#ifdef __cplusplus");
    buf.line("extern \"C\" {");
    buf.line("#endif");
    buf.blank();
    buf.line("#include <stdbool.h>");
    buf.line("#include <stddef.h>");
    buf.line("#include <stdint.h>");
    buf.line("#include <string.h>");
    buf.blank();

    gen_generic_support(&mut buf);

    buf.blank();
    buf.line(format!(
        "/* The `{}` protocol's single wire version integer (`protocol/@version`); */",
        protocol.name
    ));
    buf.line("/* also the first argument of vitrin_handshake's hello request, whose */");
    buf.line("/* accepted value becomes the connection's negotiated version. A server */");
    buf.line("/* implements every version up to its maximum and refuses anything above */");
    buf.line("/* it -- downgrade is refusal, not negotiation. */");
    buf.line(format!(
        "#define VITRIN_PROTOCOL_VERSION {}u",
        protocol.version
    ));

    buf.blank();
    let message_count: usize = protocol
        .interfaces
        .iter()
        .map(|i| i.requests.len() + i.events.len())
        .sum();
    buf.line("/* Total number of messages (requests + events) across every interface. */");
    buf.line("/* Exists so exhaustiveness can be *asserted* rather than assumed: a C */");
    buf.line("/* translation unit enumerating every message (shim/tests/ */");
    buf.line("/* test_header_compiles.c) checks its own list length against this with */");
    buf.line("/* _Static_assert, so a message added to the IDL cannot ship without a */");
    buf.line("/* compile-time proof that its marshal functions type-check. */");
    buf.line(format!("#define VITRIN_MESSAGE_COUNT {message_count}"));

    buf.blank();
    let enum_count: usize = protocol.interfaces.iter().map(|i| i.enums.len()).sum();
    buf.line("/* Total number of enums (plain and bitfield) across every interface. */");
    buf.line("/* The same gate VITRIN_MESSAGE_COUNT gives the message list, for the */");
    buf.line("/* enum list beside it. It exists because that list had NO gate and went */");
    buf.line("/* stale repeatedly: shim/tests/test_header_compiles.c was silently short */");
    buf.line("/* four enums (vitrin_shim_session's three pointer-constraint enums and */");
    buf.line("/* its idle_inhibit_state) when P2.6.5 came to append to it, having */");
    buf.line("/* already recorded two earlier misses in its own comment. An */");
    buf.line("/* untype-checked validity predicate is exactly the class of check that */");
    buf.line("/* stops checking while still compiling green. */");
    buf.line(format!("#define VITRIN_ENUM_COUNT {enum_count}"));

    buf.blank();
    buf.line("/* ==================================================================== */");
    buf.line("/* Section 1: per-interface metadata and enums.                          */");
    buf.line("/*                                                                        */");
    buf.line("/* Every enum, across every interface, is emitted here -- before any      */");
    buf.line("/* message struct in Section 2 -- because a struct member of enum type    */");
    buf.line("/* requires a complete type at the point of use, and vitrin_realm (an     */");
    buf.line("/* earlier interface) references enums defined on vitrin_grant (a later   */");
    buf.line("/* one). See this module's doc comment for the full rationale. Document   */");
    buf.line("/* order is preserved *within* this phase (interface order, then each     */");
    buf.line("/* interface's own enum order).                                          */");
    buf.line("/* ==================================================================== */");
    for iface in &protocol.interfaces {
        gen_phase1_interface(&mut buf, iface)?;
    }

    buf.blank();
    buf.line("/* ==================================================================== */");
    buf.line("/* Section 2: message structs and marshal functions, in document order   */");
    buf.line("/* (interfaces in document order; within an interface, requests then     */");
    buf.line("/* events, each in document order -- opcode assignment IS document       */");
    buf.line("/* order, independently per request/event list).                        */");
    buf.line("/* ==================================================================== */");
    for iface in &protocol.interfaces {
        gen_phase2_interface(&mut buf, protocol, iface)?;
    }

    buf.blank();
    buf.line("#ifdef __cplusplus");
    buf.line("} /* extern \"C\" */");
    buf.line("#endif");
    buf.blank();
    buf.line("#endif /* VITRIN_PROTOCOL_H */");

    Ok(buf.0)
}

/// The fixed boilerplate that never varies with the IDL: the fixed-point and
/// string-view types, the frame header type + marshal functions, the decode
/// status enum, and the raw little-endian read/write primitives every
/// per-message function is built from.
fn gen_generic_support(buf: &mut Buf) {
    buf.line("/* ---- fixed-point: vitrin_fixed_t is signed 24.8, wire-encoded as a raw */");
    buf.line("/* int32_t (docs/protocol/00-conventions.md 2.2). Used only by            */");
    buf.line("/* vitrin_shim_seat's motion event in v0.                                */");
    buf.line("typedef int32_t vitrin_fixed_t;");
    buf.blank();
    buf.line("static inline double vitrin_fixed_to_double(vitrin_fixed_t f) {");
    buf.line("    return (double)f / 256.0;");
    buf.line("}");
    buf.blank();
    buf.line("/* Rounds half away from zero (matching the generated Rust side's");
    buf.line("   Fixed::from_f64, which uses f64::round()) without pulling in <math.h>");
    buf.line("   -- this header has no dependency beyond the four includes above.");
    buf.line("");
    buf.line("   Out-of-range input clamps to INT32_MIN/INT32_MAX and NaN maps to 0,");
    buf.line("   matching the Rust side's saturating `as i32` cast exactly. The clamp is");
    buf.line("   not optional in C: casting an out-of-range double straight to int32_t");
    buf.line("   is undefined behavior (confirmed by -fsanitize=float-cast-overflow),");
    buf.line("   not saturation. 2147483648.0 and -2147483649.0 are both exactly");
    buf.line("   representable as doubles, so the comparisons below are exact. */");
    buf.line("static inline vitrin_fixed_t vitrin_fixed_from_double(double v) {");
    buf.line("    double scaled = v * 256.0;");
    buf.line("    double rounded = scaled >= 0.0 ? scaled + 0.5 : scaled - 0.5;");
    buf.line("    if (rounded != rounded) { /* NaN (also caught v = NaN: NaN * 256 is NaN) */");
    buf.line("        return 0;");
    buf.line("    }");
    buf.line("    if (rounded >= 2147483648.0) {");
    buf.line("        return INT32_MAX;");
    buf.line("    }");
    buf.line("    if (rounded <= -2147483649.0) {");
    buf.line("        return INT32_MIN;");
    buf.line("    }");
    buf.line("    return (vitrin_fixed_t)rounded;");
    buf.line("}");
    buf.blank();

    buf.line("/* ---- borrowed string view -- see the rationale at the top of this file. */");
    buf.line("typedef struct {");
    buf.line("    uint32_t len;        /* byte length; excludes wire padding */");
    buf.line(
        "    const uint8_t *data; /* borrowed; valid only as long as the buffer it points into */",
    );
    buf.line("} vitrin_string_t;");
    buf.blank();

    buf.line("/* ---- frame header: 8 bytes, little-endian throughout. object_id (u32), */");
    buf.line("/* size (u16, the whole frame including this header), opcode (u8),        */");
    buf.line("/* fd_count (u8, always 0 or 1 in v0). */");
    buf.line("#define VITRIN_HEADER_LEN ((size_t)8)");
    buf.blank();
    buf.line("typedef struct {");
    buf.line("    uint32_t object_id;");
    buf.line("    uint16_t size;");
    buf.line("    uint8_t opcode;");
    buf.line("    uint8_t fd_count;");
    buf.line("} vitrin_frame_header_t;");
    buf.blank();

    buf.line("/* Sentinel returned by a `*_encode` function when out_capacity is too");
    buf.line("   small to hold the encoded frame, or the frame would exceed the wire");
    buf.line("   format's 65535-byte limit. */");
    buf.line("#define VITRIN_ENCODE_ERR_OVERFLOW ((int32_t)-1)");
    buf.blank();
    buf.line("/* Sentinel returned by a `*_encode` function when a string argument's");
    buf.line("   `len` exceeds that argument's documented `(max N bytes)` bound. The");
    buf.line("   frame is never written: without this check the encoder could emit a");
    buf.line("   well-formed but spec-non-conformant frame that a conforming decoder");
    buf.line("   rejects, wasting a round trip -- mirroring the Rust side, which treats");
    buf.line("   an over-bound string on encode as a caller bug (there it panics; C has");
    buf.line("   no panic, so it is an error return). */");
    buf.line("#define VITRIN_ENCODE_ERR_STRING_TOO_LONG ((int32_t)-2)");
    buf.blank();

    buf.line("/* Returned by every `*_decode` function (and the raw helpers below).");
    buf.line("   VITRIN_DECODE_OK (0) is success; every other value is a distinct");
    buf.line("   failure, deliberately mirroring vitrin_protocol::DecodeError's variants");
    buf.line("   on the Rust side (crates/vitrin-protocol/src/error.rs) -- except");
    buf.line("   InvalidUtf8 and EmbeddedNul: vitrin_string_t is a borrowed length+");
    buf.line("   pointer byte view with no NUL-terminator or Unicode invariant to");
    buf.line("   protect (unlike Rust's owned String), so UTF-8 validity and");
    buf.line("   embedded-NUL rejection are deliberately left as a Rust-side (or");
    buf.line("   higher-layer) concern. This header enforces the checks that matter for");
    buf.line("   wire-format correctness and buffer safety in a language with no");
    buf.line("   owned-string type: truncation, the declared max-byte bound, enum/");
    buf.line("   bitfield membership, fd-count/signature agreement, and no trailing");
    buf.line("   bytes. */");
    buf.line("typedef enum {");
    buf.line("    VITRIN_DECODE_OK = 0,");
    buf.line("    VITRIN_DECODE_ERR_TRUNCATED = -1,");
    buf.line("    VITRIN_DECODE_ERR_STRING_TOO_LONG = -2,");
    buf.line("    VITRIN_DECODE_ERR_INVALID_ENUM = -3,");
    buf.line("    VITRIN_DECODE_ERR_INVALID_BITFIELD = -4,");
    buf.line("    VITRIN_DECODE_ERR_FD_MISMATCH = -5,");
    buf.line("    VITRIN_DECODE_ERR_TRAILING_BYTES = -6,");
    buf.line("    /* a string argument's zero padding contained a nonzero byte (fatal");
    buf.line("       invalid_argument per docs/protocol/00-conventions.md 2.2) */");
    buf.line("    VITRIN_DECODE_ERR_MALFORMED_PADDING = -7,");
    buf.line("    /* the header's size field disagrees with the delivered byte count */");
    buf.line("    VITRIN_DECODE_ERR_SIZE_MISMATCH = -8,");
    buf.line("    /* the header's opcode byte is not this message's opcode (dispatcher");
    buf.line("       mis-route; defense-in-depth, like the fd_count checks) */");
    buf.line("    VITRIN_DECODE_ERR_OPCODE_MISMATCH = -9,");
    buf.line("    /* object id 0 (null) for an argument not marked allow-null (no v0");
    buf.line("       message has a plain object argument; kept for spec completeness) */");
    buf.line("    VITRIN_DECODE_ERR_NULL_OBJECT = -10,");
    buf.line("} vitrin_decode_status_t;");
    buf.blank();
    buf.line("/* Human-readable name for a vitrin_decode_status_t, for logging. Returns");
    buf.line("   a static string literal; never NULL. */");
    buf.line("static inline const char *vitrin_decode_status_string(vitrin_decode_status_t s) {");
    buf.line("    switch (s) {");
    buf.line("        case VITRIN_DECODE_OK: return \"ok\";");
    buf.line("        case VITRIN_DECODE_ERR_TRUNCATED: return \"truncated\";");
    buf.line("        case VITRIN_DECODE_ERR_STRING_TOO_LONG: return \"string_too_long\";");
    buf.line("        case VITRIN_DECODE_ERR_INVALID_ENUM: return \"invalid_enum\";");
    buf.line("        case VITRIN_DECODE_ERR_INVALID_BITFIELD: return \"invalid_bitfield\";");
    buf.line("        case VITRIN_DECODE_ERR_FD_MISMATCH: return \"fd_mismatch\";");
    buf.line("        case VITRIN_DECODE_ERR_TRAILING_BYTES: return \"trailing_bytes\";");
    buf.line("        case VITRIN_DECODE_ERR_MALFORMED_PADDING: return \"malformed_padding\";");
    buf.line("        case VITRIN_DECODE_ERR_SIZE_MISMATCH: return \"size_mismatch\";");
    buf.line("        case VITRIN_DECODE_ERR_OPCODE_MISMATCH: return \"opcode_mismatch\";");
    buf.line("        case VITRIN_DECODE_ERR_NULL_OBJECT: return \"null_object\";");
    buf.line("        default: return \"unknown\";");
    buf.line("    }");
    buf.line("}");
    buf.blank();

    buf.line("/* ---- raw little-endian primitives -----------------------------------");
    buf.line("   Internal to this header (used by the per-message functions in Section");
    buf.line("   2 below); prefer those over calling these directly. Write helpers are");
    buf.line("   infallible: every `*_encode` has already checked out_capacity against a");
    buf.line("   precomputed total size before writing a single byte. Read helpers are");
    buf.line("   bounds-checked against in_len and return a vitrin_decode_status_t,");
    buf.line("   mirroring wire.rs's read_uint/read_string on the Rust side (a single");
    buf.line("   checked u32 reader covers both int and uint fields here, exactly as");
    buf.line("   wire.rs's own read_int is read_uint(...).map(|v| v as i32) --");
    buf.line("   little-endian bytes are identical either way; only the cast at the");
    buf.line("   call site differs). ---- */");
    buf.blank();
    buf.line("static inline void vitrin_raw_write_u32(uint8_t *out, uint32_t v) {");
    buf.line("    out[0] = (uint8_t)(v & 0xffu);");
    buf.line("    out[1] = (uint8_t)((v >> 8) & 0xffu);");
    buf.line("    out[2] = (uint8_t)((v >> 16) & 0xffu);");
    buf.line("    out[3] = (uint8_t)((v >> 24) & 0xffu);");
    buf.line("}");
    buf.blank();
    buf.line("static inline vitrin_decode_status_t vitrin_raw_read_u32(");
    buf.line("    const uint8_t *in, size_t in_len, size_t *pos, uint32_t *out) {");
    buf.line("    if (*pos + 4u > in_len) {");
    buf.line("        return VITRIN_DECODE_ERR_TRUNCATED;");
    buf.line("    }");
    buf.line("    *out = (uint32_t)in[*pos]");
    buf.line("         | ((uint32_t)in[*pos + 1u] << 8)");
    buf.line("         | ((uint32_t)in[*pos + 2u] << 16)");
    buf.line("         | ((uint32_t)in[*pos + 3u] << 24);");
    buf.line("    *pos += 4u;");
    buf.line("    return VITRIN_DECODE_OK;");
    buf.line("}");
    buf.blank();
    buf.line("static inline size_t vitrin_raw_pad_len(uint32_t len) {");
    buf.line("    return (size_t)((4u - (len % 4u)) % 4u);");
    buf.line("}");
    buf.blank();
    buf.line("/* Wire size of one string argument: 4-byte length prefix + bytes + zero");
    buf.line("   padding to the next 4-byte boundary. Computed in uint64_t, NOT size_t:");
    buf.line("   on a 32-bit target a byte_len near UINT32_MAX would wrap 32-bit size_t");
    buf.line("   arithmetic to a tiny value, and every *_encode's total-frame-size guard");
    buf.line("   below would then pass a frame whose memcpy runs ~4 GiB past the output");
    buf.line("   buffer. 64-bit arithmetic cannot wrap here (max term is well under");
    buf.line("   2^33), so the guard stays sound on every target. */");
    buf.line("static inline uint64_t vitrin_raw_string_wire_len(uint32_t byte_len) {");
    buf.line(
        "    return (uint64_t)4 + (uint64_t)byte_len + (uint64_t)vitrin_raw_pad_len(byte_len);",
    );
    buf.line("}");
    buf.blank();
    buf.line("/* Writes a string argument: u32 byte length, the bytes themselves (no NUL");
    buf.line("   terminator), zero-padded to the next 4-byte boundary -- the length");
    buf.line("   prefix counts only the bytes, never the padding. Returns the total");
    buf.line("   number of bytes written (vitrin_raw_string_wire_len(s.len)). */");
    buf.line("static inline size_t vitrin_raw_write_string(uint8_t *out, vitrin_string_t s) {");
    buf.line("    size_t pad = vitrin_raw_pad_len(s.len);");
    buf.line("    vitrin_raw_write_u32(out, s.len);");
    buf.line("    if (s.len > 0u) {");
    buf.line("        memcpy(out + 4, s.data, s.len);");
    buf.line("    }");
    buf.line("    if (pad > 0u) {");
    buf.line("        memset(out + 4 + (size_t)s.len, 0, pad);");
    buf.line("    }");
    buf.line("    return (size_t)4 + (size_t)s.len + pad;");
    buf.line("}");
    buf.blank();
    buf.line("/* Reads a string argument, enforcing max_bytes (the arg's documented");
    buf.line("   `(max N bytes)` bound), buffer bounds, and all-zero padding (malformed");
    buf.line("   padding is fatal invalid_argument per conventions 2.2; accepting");
    buf.line("   arbitrary padding bytes would also open a covert channel). *out borrows");
    buf.line("   directly into `in` (out->data = in + <offset>); it is valid only as");
    buf.line("   long as `in` is. Does not validate UTF-8 or reject embedded NUL bytes");
    buf.line("   -- see the rationale on vitrin_decode_status_t above. */");
    buf.line("static inline vitrin_decode_status_t vitrin_raw_read_string(");
    buf.line("    const uint8_t *in, size_t in_len, size_t *pos, uint32_t max_bytes,");
    buf.line("    vitrin_string_t *out) {");
    buf.line("    uint32_t len;");
    buf.line("    size_t pad;");
    buf.line("    vitrin_decode_status_t st = vitrin_raw_read_u32(in, in_len, pos, &len);");
    buf.line("    if (st != VITRIN_DECODE_OK) {");
    buf.line("        return st;");
    buf.line("    }");
    buf.line("    if (len > max_bytes) {");
    buf.line("        return VITRIN_DECODE_ERR_STRING_TOO_LONG;");
    buf.line("    }");
    buf.line("    if (*pos + (size_t)len > in_len) {");
    buf.line("        return VITRIN_DECODE_ERR_TRUNCATED;");
    buf.line("    }");
    buf.line("    out->len = len;");
    buf.line("    out->data = in + *pos;");
    buf.line("    *pos += (size_t)len;");
    buf.line("    pad = vitrin_raw_pad_len(len);");
    buf.line("    if (*pos + pad > in_len) {");
    buf.line("        return VITRIN_DECODE_ERR_TRUNCATED;");
    buf.line("    }");
    buf.line("    for (size_t i = 0; i < pad; i++) {");
    buf.line("        if (in[*pos + i] != 0u) {");
    buf.line("            return VITRIN_DECODE_ERR_MALFORMED_PADDING;");
    buf.line("        }");
    buf.line("    }");
    buf.line("    *pos += pad;");
    buf.line("    return VITRIN_DECODE_OK;");
    buf.line("}");
    buf.blank();

    buf.line("/* ---- frame header marshal ---- */");
    buf.blank();
    buf.line("static inline void vitrin_frame_header_encode(const vitrin_frame_header_t *hdr, uint8_t *out) {");
    buf.line("    out[0] = (uint8_t)(hdr->object_id & 0xffu);");
    buf.line("    out[1] = (uint8_t)((hdr->object_id >> 8) & 0xffu);");
    buf.line("    out[2] = (uint8_t)((hdr->object_id >> 16) & 0xffu);");
    buf.line("    out[3] = (uint8_t)((hdr->object_id >> 24) & 0xffu);");
    buf.line("    out[4] = (uint8_t)(hdr->size & 0xffu);");
    buf.line("    out[5] = (uint8_t)((hdr->size >> 8) & 0xffu);");
    buf.line("    out[6] = hdr->opcode;");
    buf.line("    out[7] = hdr->fd_count;");
    buf.line("}");
    buf.blank();
    buf.line("static inline vitrin_decode_status_t vitrin_frame_header_decode(");
    buf.line("    const uint8_t *in, size_t in_len, vitrin_frame_header_t *out) {");
    buf.line("    if (in_len < VITRIN_HEADER_LEN) {");
    buf.line("        return VITRIN_DECODE_ERR_TRUNCATED;");
    buf.line("    }");
    buf.line("    out->object_id = (uint32_t)in[0]");
    buf.line("                   | ((uint32_t)in[1] << 8)");
    buf.line("                   | ((uint32_t)in[2] << 16)");
    buf.line("                   | ((uint32_t)in[3] << 24);");
    buf.line("    out->size = (uint16_t)((uint32_t)in[4] | ((uint32_t)in[5] << 8));");
    buf.line("    out->opcode = in[6];");
    buf.line("    out->fd_count = in[7];");
    buf.line("    return VITRIN_DECODE_OK;");
    buf.line("}");
}

// ---------------------------------------------------------------------------
// Naming helpers. Interface/message/arg/enum/entry names are already
// snake_case in the IDL (Wayland-style) and are used as-is for C identifiers
// (fields, function names); only *macro* names need SCREAMING_SNAKE.
// ---------------------------------------------------------------------------

/// Lowercase base for an enum's generated names, e.g. `vitrin_view_format`.
fn enum_c_base(iface_name: &str, enum_name: &str) -> String {
    format!("{iface_name}_{enum_name}")
}

fn enum_c_type_name(iface_name: &str, enum_name: &str) -> String {
    format!("{}_t", enum_c_base(iface_name, enum_name))
}

fn enum_c_validity_fn(iface_name: &str, enum_name: &str) -> String {
    format!("{}_is_valid", enum_c_base(iface_name, enum_name))
}

fn enum_c_macro_prefix(iface_name: &str, enum_name: &str) -> String {
    to_screaming_snake(&enum_c_base(iface_name, enum_name))
}

/// Lowercase base for a message's generated names, e.g.
/// `vitrin_view_evt_frame_ready`. The `req`/`evt` infix (see [`CMsgKind`])
/// guarantees this can never collide with a bare enum's base name, and a
/// request can never collide with an event of the same name.
fn msg_c_base(iface_name: &str, kind: CMsgKind, msg_name: &str) -> String {
    format!("{iface_name}_{}_{msg_name}", kind.infix())
}

fn msg_c_macro_prefix(iface_name: &str, kind: CMsgKind, msg_name: &str) -> String {
    to_screaming_snake(&msg_c_base(iface_name, kind, msg_name))
}

fn is_bitfield(protocol: &Protocol, r: &EnumRef) -> bool {
    protocol
        .interface(&r.interface)
        .and_then(|i| i.enum_def(&r.name))
        .map(|e| e.bitfield)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Section 1: interface metadata + enums.
// ---------------------------------------------------------------------------

fn gen_phase1_interface(buf: &mut Buf, iface: &Interface) -> Result<()> {
    buf.blank();
    buf.line(format!(
        "/* ==== {} (version {}) ==== */",
        iface.name, iface.version
    ));
    if !iface.summary.is_empty() {
        buf.line(format!("/* {} */", doc_text(&iface.summary)));
    }
    buf.blank();
    gen_interface_consts(buf, iface);
    for enum_def in &iface.enums {
        buf.blank();
        gen_c_enum(buf, iface, enum_def)?;
    }
    Ok(())
}

fn gen_interface_consts(buf: &mut Buf, iface: &Interface) {
    let upper = to_screaming_snake(&iface.name);
    buf.line(format!("#define {upper}_INTERFACE_NAME \"{}\"", iface.name));
    buf.line(format!(
        "#define {upper}_INTERFACE_VERSION {}u",
        iface.version
    ));
    if let Some(verb) = &iface.verb {
        buf.line(format!(
            "/* Every request on this interface exercises the grant verb `{verb}`. */"
        ));
        buf.line(format!("#define {upper}_VERB \"{verb}\""));
    }
}

fn gen_c_enum(buf: &mut Buf, iface: &Interface, enum_def: &EnumDef) -> Result<()> {
    if enum_def.bitfield {
        gen_bitfield_enum(buf, iface, enum_def);
        Ok(())
    } else {
        gen_plain_enum(buf, iface, enum_def)
    }
}

fn gen_plain_enum(buf: &mut Buf, iface: &Interface, enum_def: &EnumDef) -> Result<()> {
    // ISO C requires an enumeration constant's value to be representable as
    // `int`; every entry in v0.xml (including vitrin_view.format's DRM
    // fourcc codes, the largest values in the IDL) fits easily, but a future
    // entry might not -- fail loudly at codegen time rather than silently
    // emit a header that some compiler accepts as a GNU extension and
    // another rejects.
    for entry in &enum_def.entries {
        if entry.value > 0x7fff_ffff {
            bail!(
                "enum '{}.{}' entry '{}' has value {} (0x{:x}), which does not fit in a C \
                 `int` and therefore cannot be a plain C enum constant (ISO C requires \
                 enumeration constants to be representable as `int`); the C backend does not \
                 support this -- a bitfield-style value would need `bitfield=\"true\"` instead",
                iface.name,
                enum_def.name,
                entry.name,
                entry.value,
                entry.value
            );
        }
    }

    let base = enum_c_base(&iface.name, &enum_def.name);
    let type_name = enum_c_type_name(&iface.name, &enum_def.name);
    let macro_prefix = enum_c_macro_prefix(&iface.name, &enum_def.name);

    buf.line(format!("/* Enum `{}` on `{}`.", enum_def.name, iface.name));
    if !enum_def.summary.is_empty() {
        buf.line(" *");
        buf.line(format!(" * {}", doc_text(&enum_def.summary)));
    }
    buf.line(" *");
    buf.line(" * Plain enum: a wire value MUST exactly equal one defined entry. */");
    buf.line("typedef enum {");
    for entry in &enum_def.entries {
        let name = format!("{macro_prefix}_{}", to_screaming_snake(&entry.name));
        if !entry.summary.is_empty() {
            buf.line(format!("    /* {} */", doc_text(&entry.summary)));
        }
        buf.line(format!("    {name} = {},", format_u32_literal(entry.value)));
    }
    buf.line(format!("}} {type_name};"));
    buf.blank();
    buf.line(format!(
        "/* Whole-value membership check for `{type_name}` (decode a wire value by"
    ));
    buf.line("   whether it equals one of the defined entries above). */");
    buf.line(format!("static inline bool {base}_is_valid(uint32_t v) {{"));
    buf.line("    switch (v) {");
    for entry in &enum_def.entries {
        let name = format!("{macro_prefix}_{}", to_screaming_snake(&entry.name));
        buf.line(format!("        case {name}:"));
    }
    buf.line("            return true;");
    buf.line("        default:");
    buf.line("            return false;");
    buf.line("    }");
    buf.line("}");
    Ok(())
}

fn gen_bitfield_enum(buf: &mut Buf, iface: &Interface, enum_def: &EnumDef) {
    let base = enum_c_base(&iface.name, &enum_def.name);
    let type_name = enum_c_type_name(&iface.name, &enum_def.name);
    let macro_prefix = enum_c_macro_prefix(&iface.name, &enum_def.name);

    buf.line(format!(
        "/* Enum `{}` on `{}` (bitfield).",
        enum_def.name, iface.name
    ));
    if !enum_def.summary.is_empty() {
        buf.line(" *");
        buf.line(format!(" * {}", doc_text(&enum_def.summary)));
    }
    buf.line(" *");
    buf.line(" * Bitfield: any combination of the defined entries' bits is a legal wire");
    buf.line(" * value; a bit outside their union is invalid. Represented as a plain");
    buf.line(" * uint32_t typedef (not a C enum, which ISO C would require to fit in an");
    buf.line(" * `int` and which offers no bitwise-OR-of-two-enumerators guarantee");
    buf.line(" * anyway) with one #define per bit, matching how the bits actually get");
    buf.line(" * combined by callers. */");
    buf.line(format!("typedef uint32_t {type_name};"));
    buf.blank();
    for entry in &enum_def.entries {
        if !entry.summary.is_empty() {
            buf.line(format!("/* {} */", doc_text(&entry.summary)));
        }
        buf.line(format!(
            "#define {macro_prefix}_{} (({type_name}){})",
            to_screaming_snake(&entry.name),
            format_u32_literal(entry.value)
        ));
    }
    let mask_expr = enum_def
        .entries
        .iter()
        .map(|e| format_u32_literal(e.value))
        .collect::<Vec<_>>()
        .join(" | ");
    buf.line("/* Union of every defined entry's bits; a wire value with any other bit");
    buf.line("   set is invalid. */");
    buf.line(format!(
        "#define {macro_prefix}_VALID_MASK (({type_name})({mask_expr}))"
    ));
    buf.blank();
    buf.line(format!(
        "/* Bitmask validity check for `{type_name}`: rejects any bit outside"
    ));
    buf.line(format!("   {macro_prefix}_VALID_MASK. */"));
    buf.line(format!("static inline bool {base}_is_valid(uint32_t v) {{"));
    buf.line(format!(
        "    return (v & ~((uint32_t){macro_prefix}_VALID_MASK)) == 0u;"
    ));
    buf.line("}");
}

// ---------------------------------------------------------------------------
// Section 2: message structs + marshal functions.
// ---------------------------------------------------------------------------

fn gen_phase2_interface(buf: &mut Buf, protocol: &Protocol, iface: &Interface) -> Result<()> {
    if iface.requests.is_empty() && iface.events.is_empty() {
        return Ok(());
    }
    buf.blank();
    buf.line(format!("/* ==== {} messages ==== */", iface.name));
    for msg in &iface.requests {
        buf.blank();
        gen_c_message(buf, protocol, iface, msg, CMsgKind::Request);
    }
    for msg in &iface.events {
        buf.blank();
        gen_c_message(buf, protocol, iface, msg, CMsgKind::Event);
    }
    Ok(())
}

fn gen_c_message(
    buf: &mut Buf,
    protocol: &Protocol,
    iface: &Interface,
    msg: &Message,
    kind: CMsgKind,
) {
    let base = msg_c_base(&iface.name, kind, &msg.name);
    let type_name = format!("{base}_t");
    let macro_prefix = msg_c_macro_prefix(&iface.name, kind, &msg.name);
    let has_fd = msg.has_fd();

    buf.line(format!(
        "/* {} `{}` (opcode {}) on `{}`.",
        kind.label(),
        msg.name,
        msg.opcode,
        iface.name
    ));
    if !msg.summary.is_empty() {
        buf.line(" *");
        buf.line(format!(" * {}", doc_text(&msg.summary)));
    }
    buf.line(" */");
    buf.line("typedef struct {");
    if msg.args.is_empty() {
        buf.line("    /* no arguments -- a truly empty struct is not portable standard C */");
        buf.line("    char reserved;");
    } else {
        for arg in &msg.args {
            gen_c_field(buf, arg);
        }
    }
    buf.line(format!("}} {type_name};"));
    buf.blank();

    buf.line(format!(
        "#define {macro_prefix}_OPCODE ((uint8_t){})",
        msg.opcode
    ));
    buf.line(format!(
        "#define {macro_prefix}_HAS_FD {}",
        i32::from(has_fd)
    ));
    buf.line("/* First protocol version at which this message is defined (`message/@since`); */");
    buf.line("/* this opcode is not defined on a connection whose negotiated version is    */");
    buf.line("/* lower, where using it is fatal `invalid_opcode`.                          */");
    buf.line(format!("#define {macro_prefix}_SINCE {}u", msg.since));
    buf.blank();

    gen_c_encode(buf, protocol, msg, &type_name, &macro_prefix, &base);
    buf.blank();
    gen_c_decode(buf, protocol, msg, &type_name, &macro_prefix, &base);
}

fn gen_c_field(buf: &mut Buf, arg: &Arg) {
    let ty = c_field_type(arg);
    let mut doc = doc_text(&arg.summary);
    match &arg.ty {
        ArgType::NewId { interface } => {
            doc = if doc.is_empty() {
                format!("(new_id: {interface})")
            } else {
                format!("{doc} (new_id: {interface})")
            };
        }
        ArgType::Object { interface } => {
            let note = if arg.allow_null {
                format!("(object: {interface}; 0 = null)")
            } else {
                format!("(object: {interface})")
            };
            doc = if doc.is_empty() {
                note
            } else {
                format!("{doc} {note}")
            };
        }
        ArgType::Fd => {
            let note = "not present in the byte buffer; carried out-of-band via SCM_RIGHTS";
            doc = if doc.is_empty() {
                note.to_string()
            } else {
                format!("{doc} ({note})")
            };
        }
        _ => {}
    }
    if !doc.is_empty() {
        buf.line(format!("    /* {doc} */"));
    }
    buf.line(format!("    {ty} {};", arg.name));
}

/// The C field type for one argument. `object`/`new_id` args are always
/// plain `uint32_t` regardless of `allow_null`: unlike Rust's `Option<u32>`,
/// C has no wrapper type here, and none is needed -- the wire format already
/// treats object id `0` as the null sentinel (`docs/protocol/00-conventions.md`
/// section 3), so a plain `uint32_t` with "0 = null" as a documented
/// convention (see `gen_c_field` above) is the natural, wire-native
/// representation, not a simplification that loses anything.
fn c_field_type(arg: &Arg) -> String {
    match &arg.ty {
        ArgType::Int { enum_ref: None } => "int32_t".to_string(),
        ArgType::Int { enum_ref: Some(r) } | ArgType::Uint { enum_ref: Some(r) } => {
            enum_c_type_name(&r.interface, &r.name)
        }
        ArgType::Uint { enum_ref: None } => "uint32_t".to_string(),
        ArgType::Fixed => "vitrin_fixed_t".to_string(),
        ArgType::String { .. } => "vitrin_string_t".to_string(),
        ArgType::Object { .. } => "uint32_t".to_string(),
        ArgType::NewId { .. } => "uint32_t".to_string(),
        ArgType::Fd => "int".to_string(),
    }
}

/// One argument's contribution to a message's precomputed encode size
/// (`None` for `fd`, which is never in the byte buffer).
fn arg_size_term(arg: &Arg) -> Option<String> {
    match &arg.ty {
        ArgType::Fd => None,
        ArgType::String { .. } => {
            Some(format!("vitrin_raw_string_wire_len(msg->{}.len)", arg.name))
        }
        _ => Some("4".to_string()),
    }
}

fn gen_c_encode(
    buf: &mut Buf,
    protocol: &Protocol,
    msg: &Message,
    type_name: &str,
    macro_prefix: &str,
    base: &str,
) {
    buf.line("/* Encodes into a complete frame (header + argument payload). Returns the");
    buf.line("   number of bytes written (fits in an int32_t: the wire format's own u16");
    buf.line("   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if");
    buf.line("   out_capacity is too small or the frame would exceed 65535 bytes, or");
    buf.line("   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own");
    buf.line("   documented `(max N bytes)` bound. Nothing is written to `out` on either");
    buf.line("   error. Any fd argument is never written here -- send it out-of-band via");
    buf.line("   SCM_RIGHTS alongside these bytes. */");
    buf.line(format!(
        "static inline int32_t {base}_encode(const {type_name} *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {{"
    ));

    // Per-argument bound checks come first, before any size arithmetic or
    // writes: an over-bound string is a distinct caller error, not an
    // out_capacity problem, and reporting it precisely mirrors the Rust
    // side's per-argument assertion in wire::write_string.
    for arg in &msg.args {
        if let ArgType::String { max_bytes } = &arg.ty {
            buf.line(format!("    if (msg->{}.len > {max_bytes}u) {{", arg.name));
            buf.line("        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;");
            buf.line("    }");
        }
    }

    let mut size_expr = String::from("(uint64_t)VITRIN_HEADER_LEN");
    for arg in &msg.args {
        if let Some(term) = arg_size_term(arg) {
            size_expr.push_str(" + ");
            size_expr.push_str(&term);
        }
    }
    // uint64_t on purpose -- see vitrin_raw_string_wire_len's comment: 32-bit
    // size_t arithmetic could wrap on a hostile string length and defeat this
    // guard entirely.
    buf.line(format!("    uint64_t size = {size_expr};"));
    buf.line("    if (size > 0xffffu || size > (uint64_t)out_capacity) {");
    buf.line("        return VITRIN_ENCODE_ERR_OVERFLOW;");
    buf.line("    }");
    buf.line("    vitrin_frame_header_t hdr;");
    buf.line("    hdr.object_id = object_id;");
    buf.line("    hdr.size = (uint16_t)size;");
    buf.line(format!("    hdr.opcode = {macro_prefix}_OPCODE;"));
    buf.line(format!(
        "    hdr.fd_count = (uint8_t){macro_prefix}_HAS_FD;"
    ));
    buf.line("    vitrin_frame_header_encode(&hdr, out);");

    let readable_args: Vec<&Arg> = msg
        .args
        .iter()
        .filter(|a| !matches!(a.ty, ArgType::Fd))
        .collect();
    if readable_args.is_empty() {
        buf.line("    (void)msg;");
    } else {
        buf.line("    size_t pos = VITRIN_HEADER_LEN;");
        for arg in &msg.args {
            gen_encode_arg(buf, protocol, arg);
        }
    }
    buf.line("    return (int32_t)size;");
    buf.line("}");
}

fn gen_encode_arg(buf: &mut Buf, protocol: &Protocol, arg: &Arg) {
    let field = &arg.name;
    match &arg.ty {
        ArgType::Int { enum_ref: None } => {
            buf.line(format!(
                "    vitrin_raw_write_u32(out + pos, (uint32_t)msg->{field});"
            ));
            buf.line("    pos += 4u;");
        }
        ArgType::Uint { enum_ref: None } | ArgType::Object { .. } | ArgType::NewId { .. } => {
            buf.line(format!(
                "    vitrin_raw_write_u32(out + pos, msg->{field});"
            ));
            buf.line("    pos += 4u;");
        }
        ArgType::Int { enum_ref: Some(r) } | ArgType::Uint { enum_ref: Some(r) } => {
            if is_bitfield(protocol, r) {
                buf.line(format!(
                    "    vitrin_raw_write_u32(out + pos, msg->{field});"
                ));
            } else {
                buf.line(format!(
                    "    vitrin_raw_write_u32(out + pos, (uint32_t)msg->{field});"
                ));
            }
            buf.line("    pos += 4u;");
        }
        ArgType::Fixed => {
            buf.line(format!(
                "    vitrin_raw_write_u32(out + pos, (uint32_t)msg->{field});"
            ));
            buf.line("    pos += 4u;");
        }
        ArgType::String { .. } => {
            buf.line(format!(
                "    pos += vitrin_raw_write_string(out + pos, msg->{field});"
            ));
        }
        ArgType::Fd => {
            buf.line(format!(
                "    /* {field}: fd argument, never written to the byte buffer */"
            ));
        }
    }
}

fn gen_c_decode(
    buf: &mut Buf,
    protocol: &Protocol,
    msg: &Message,
    type_name: &str,
    macro_prefix: &str,
    base: &str,
) {
    buf.line("/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.");
    buf.line("   already delimited by a transport layer using the header's own size field,");
    buf.line("   out of scope here) plus, iff HAS_FD below, the fd received alongside it");
    buf.line("   out-of-band (fd = -1 if none). On success writes the frame's object_id to");
    buf.line("   *out_object_id and the decoded message to *out and returns");
    buf.line("   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and");
    buf.line("   leaves *out_object_id and *out unspecified.");
    buf.line("");
    buf.line("   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two");
    buf.line("   independent disjuncts, both checked here: the header's own fd_count byte");
    buf.line("   disagreeing with this message's signature, and the out-of-band fd");
    buf.line("   parameter disagreeing with it. A hostile or buggy peer can make either");
    buf.line("   one lie without the other, so neither check substitutes for the other.");
    buf.line("");
    buf.line("   The header's opcode and size fields are validated in the same");
    buf.line("   defense-in-depth spirit: the dispatcher already selected this message by");
    buf.line("   opcode and delimited the frame by size, but a dispatcher bug (or a");
    buf.line("   header whose size field lies about the delivered byte count, fatal");
    buf.line("   `oversized` per conventions 2.1) must surface as an error here, not as a");
    buf.line("   silently mis-decoded message. */");
    buf.line(format!(
        "static inline vitrin_decode_status_t {base}_decode("
    ));
    buf.line("    const uint8_t *in, size_t in_len, int fd,");
    buf.line(format!("    uint32_t *out_object_id, {type_name} *out) {{"));
    buf.line("    int fd_present = (fd >= 0) ? 1 : 0;");
    buf.line(format!("    if (fd_present != {macro_prefix}_HAS_FD) {{"));
    buf.line("        return VITRIN_DECODE_ERR_FD_MISMATCH;");
    buf.line("    }");
    buf.line("    vitrin_frame_header_t hdr;");
    buf.line("    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);");
    buf.line("    if (hdr_st != VITRIN_DECODE_OK) {");
    buf.line("        return hdr_st;");
    buf.line("    }");
    buf.line(format!("    if (hdr.opcode != {macro_prefix}_OPCODE) {{"));
    buf.line("        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;");
    buf.line("    }");
    buf.line("    if ((size_t)hdr.size != in_len) {");
    buf.line("        return VITRIN_DECODE_ERR_SIZE_MISMATCH;");
    buf.line("    }");
    buf.line(format!(
        "    if (hdr.fd_count != (uint8_t){macro_prefix}_HAS_FD) {{"
    ));
    buf.line("        return VITRIN_DECODE_ERR_FD_MISMATCH;");
    buf.line("    }");
    buf.line("    size_t pos = VITRIN_HEADER_LEN;");
    if msg.args.is_empty() {
        buf.line("    out->reserved = 0;");
    } else {
        for arg in &msg.args {
            gen_decode_arg(buf, protocol, arg);
        }
    }
    buf.line("    if (pos != in_len) {");
    buf.line("        return VITRIN_DECODE_ERR_TRAILING_BYTES;");
    buf.line("    }");
    buf.line("    *out_object_id = hdr.object_id;");
    buf.line("    return VITRIN_DECODE_OK;");
    buf.line("}");
}

fn gen_decode_arg(buf: &mut Buf, protocol: &Protocol, arg: &Arg) {
    let field = &arg.name;
    match &arg.ty {
        ArgType::Int { enum_ref: None } => {
            buf.line(format!("    uint32_t {field}_raw;"));
            buf.line(format!(
                "    vitrin_decode_status_t st_{field} = vitrin_raw_read_u32(in, in_len, &pos, &{field}_raw);"
            ));
            buf.line(format!(
                "    if (st_{field} != VITRIN_DECODE_OK) {{ return st_{field}; }}"
            ));
            buf.line(format!("    out->{field} = (int32_t){field}_raw;"));
        }
        ArgType::Uint { enum_ref: None } | ArgType::NewId { .. } => {
            buf.line(format!(
                "    vitrin_decode_status_t st_{field} = vitrin_raw_read_u32(in, in_len, &pos, &out->{field});"
            ));
            buf.line(format!(
                "    if (st_{field} != VITRIN_DECODE_OK) {{ return st_{field}; }}"
            ));
        }
        ArgType::Object { .. } => {
            buf.line(format!(
                "    vitrin_decode_status_t st_{field} = vitrin_raw_read_u32(in, in_len, &pos, &out->{field});"
            ));
            buf.line(format!(
                "    if (st_{field} != VITRIN_DECODE_OK) {{ return st_{field}; }}"
            ));
            if !arg.allow_null {
                // Object id 0 is the null object, legal only under allow-null
                // (conventions section 3).
                buf.line(format!(
                    "    if (out->{field} == 0u) {{ return VITRIN_DECODE_ERR_NULL_OBJECT; }}"
                ));
            }
        }
        ArgType::Int { enum_ref: Some(r) } | ArgType::Uint { enum_ref: Some(r) } => {
            let ty = enum_c_type_name(&r.interface, &r.name);
            let validity_fn = enum_c_validity_fn(&r.interface, &r.name);
            let err = if is_bitfield(protocol, r) {
                "VITRIN_DECODE_ERR_INVALID_BITFIELD"
            } else {
                "VITRIN_DECODE_ERR_INVALID_ENUM"
            };
            buf.line(format!("    uint32_t {field}_raw;"));
            buf.line(format!(
                "    vitrin_decode_status_t st_{field} = vitrin_raw_read_u32(in, in_len, &pos, &{field}_raw);"
            ));
            buf.line(format!(
                "    if (st_{field} != VITRIN_DECODE_OK) {{ return st_{field}; }}"
            ));
            buf.line(format!(
                "    if (!{validity_fn}({field}_raw)) {{ return {err}; }}"
            ));
            buf.line(format!("    out->{field} = ({ty}){field}_raw;"));
        }
        ArgType::Fixed => {
            buf.line(format!("    uint32_t {field}_raw;"));
            buf.line(format!(
                "    vitrin_decode_status_t st_{field} = vitrin_raw_read_u32(in, in_len, &pos, &{field}_raw);"
            ));
            buf.line(format!(
                "    if (st_{field} != VITRIN_DECODE_OK) {{ return st_{field}; }}"
            ));
            buf.line(format!("    out->{field} = (vitrin_fixed_t){field}_raw;"));
        }
        ArgType::String { max_bytes } => {
            buf.line(format!(
                "    vitrin_decode_status_t st_{field} = vitrin_raw_read_string(in, in_len, &pos, {max_bytes}u, &out->{field});"
            ));
            buf.line(format!(
                "    if (st_{field} != VITRIN_DECODE_OK) {{ return st_{field}; }}"
            ));
        }
        ArgType::Fd => {
            buf.line(format!("    out->{field} = fd;"));
        }
    }
}
