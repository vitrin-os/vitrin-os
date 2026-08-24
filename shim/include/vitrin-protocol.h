// SPDX-License-Identifier: Apache-2.0
//
// GENERATED FILE -- DO NOT EDIT BY HAND.
//
// Source: protocol/vitrin-v0.xml
// Regenerate with: cargo xtask codegen

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

#ifndef VITRIN_PROTOCOL_H
#define VITRIN_PROTOCOL_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

/* ---- fixed-point: vitrin_fixed_t is signed 24.8, wire-encoded as a raw */
/* int32_t (docs/protocol/00-conventions.md 2.2). Used only by            */
/* vitrin_shim_seat's motion event in v0.                                */
typedef int32_t vitrin_fixed_t;

static inline double vitrin_fixed_to_double(vitrin_fixed_t f) {
    return (double)f / 256.0;
}

/* Rounds half away from zero (matching the generated Rust side's
   Fixed::from_f64, which uses f64::round()) without pulling in <math.h>
   -- this header has no dependency beyond the four includes above.

   Out-of-range input clamps to INT32_MIN/INT32_MAX and NaN maps to 0,
   matching the Rust side's saturating `as i32` cast exactly. The clamp is
   not optional in C: casting an out-of-range double straight to int32_t
   is undefined behavior (confirmed by -fsanitize=float-cast-overflow),
   not saturation. 2147483648.0 and -2147483649.0 are both exactly
   representable as doubles, so the comparisons below are exact. */
static inline vitrin_fixed_t vitrin_fixed_from_double(double v) {
    double scaled = v * 256.0;
    double rounded = scaled >= 0.0 ? scaled + 0.5 : scaled - 0.5;
    if (rounded != rounded) { /* NaN (also caught v = NaN: NaN * 256 is NaN) */
        return 0;
    }
    if (rounded >= 2147483648.0) {
        return INT32_MAX;
    }
    if (rounded <= -2147483649.0) {
        return INT32_MIN;
    }
    return (vitrin_fixed_t)rounded;
}

/* ---- borrowed string view -- see the rationale at the top of this file. */
typedef struct {
    uint32_t len;        /* byte length; excludes wire padding */
    const uint8_t *data; /* borrowed; valid only as long as the buffer it points into */
} vitrin_string_t;

/* ---- frame header: 8 bytes, little-endian throughout. object_id (u32), */
/* size (u16, the whole frame including this header), opcode (u8),        */
/* fd_count (u8, always 0 or 1 in v0). */
#define VITRIN_HEADER_LEN ((size_t)8)

typedef struct {
    uint32_t object_id;
    uint16_t size;
    uint8_t opcode;
    uint8_t fd_count;
} vitrin_frame_header_t;

/* Sentinel returned by a `*_encode` function when out_capacity is too
   small to hold the encoded frame, or the frame would exceed the wire
   format's 65535-byte limit. */
#define VITRIN_ENCODE_ERR_OVERFLOW ((int32_t)-1)

/* Sentinel returned by a `*_encode` function when a string argument's
   `len` exceeds that argument's documented `(max N bytes)` bound. The
   frame is never written: without this check the encoder could emit a
   well-formed but spec-non-conformant frame that a conforming decoder
   rejects, wasting a round trip -- mirroring the Rust side, which treats
   an over-bound string on encode as a caller bug (there it panics; C has
   no panic, so it is an error return). */
#define VITRIN_ENCODE_ERR_STRING_TOO_LONG ((int32_t)-2)

/* Returned by every `*_decode` function (and the raw helpers below).
   VITRIN_DECODE_OK (0) is success; every other value is a distinct
   failure, deliberately mirroring vitrin_protocol::DecodeError's variants
   on the Rust side (crates/vitrin-protocol/src/error.rs) -- except
   InvalidUtf8 and EmbeddedNul: vitrin_string_t is a borrowed length+
   pointer byte view with no NUL-terminator or Unicode invariant to
   protect (unlike Rust's owned String), so UTF-8 validity and
   embedded-NUL rejection are deliberately left as a Rust-side (or
   higher-layer) concern. This header enforces the checks that matter for
   wire-format correctness and buffer safety in a language with no
   owned-string type: truncation, the declared max-byte bound, enum/
   bitfield membership, fd-count/signature agreement, and no trailing
   bytes. */
typedef enum {
    VITRIN_DECODE_OK = 0,
    VITRIN_DECODE_ERR_TRUNCATED = -1,
    VITRIN_DECODE_ERR_STRING_TOO_LONG = -2,
    VITRIN_DECODE_ERR_INVALID_ENUM = -3,
    VITRIN_DECODE_ERR_INVALID_BITFIELD = -4,
    VITRIN_DECODE_ERR_FD_MISMATCH = -5,
    VITRIN_DECODE_ERR_TRAILING_BYTES = -6,
    /* a string argument's zero padding contained a nonzero byte (fatal
       invalid_argument per docs/protocol/00-conventions.md 2.2) */
    VITRIN_DECODE_ERR_MALFORMED_PADDING = -7,
    /* the header's size field disagrees with the delivered byte count */
    VITRIN_DECODE_ERR_SIZE_MISMATCH = -8,
    /* the header's opcode byte is not this message's opcode (dispatcher
       mis-route; defense-in-depth, like the fd_count checks) */
    VITRIN_DECODE_ERR_OPCODE_MISMATCH = -9,
    /* object id 0 (null) for an argument not marked allow-null (no v0
       message has a plain object argument; kept for spec completeness) */
    VITRIN_DECODE_ERR_NULL_OBJECT = -10,
} vitrin_decode_status_t;

/* Human-readable name for a vitrin_decode_status_t, for logging. Returns
   a static string literal; never NULL. */
static inline const char *vitrin_decode_status_string(vitrin_decode_status_t s) {
    switch (s) {
        case VITRIN_DECODE_OK: return "ok";
        case VITRIN_DECODE_ERR_TRUNCATED: return "truncated";
        case VITRIN_DECODE_ERR_STRING_TOO_LONG: return "string_too_long";
        case VITRIN_DECODE_ERR_INVALID_ENUM: return "invalid_enum";
        case VITRIN_DECODE_ERR_INVALID_BITFIELD: return "invalid_bitfield";
        case VITRIN_DECODE_ERR_FD_MISMATCH: return "fd_mismatch";
        case VITRIN_DECODE_ERR_TRAILING_BYTES: return "trailing_bytes";
        case VITRIN_DECODE_ERR_MALFORMED_PADDING: return "malformed_padding";
        case VITRIN_DECODE_ERR_SIZE_MISMATCH: return "size_mismatch";
        case VITRIN_DECODE_ERR_OPCODE_MISMATCH: return "opcode_mismatch";
        case VITRIN_DECODE_ERR_NULL_OBJECT: return "null_object";
        default: return "unknown";
    }
}

/* ---- raw little-endian primitives -----------------------------------
   Internal to this header (used by the per-message functions in Section
   2 below); prefer those over calling these directly. Write helpers are
   infallible: every `*_encode` has already checked out_capacity against a
   precomputed total size before writing a single byte. Read helpers are
   bounds-checked against in_len and return a vitrin_decode_status_t,
   mirroring wire.rs's read_uint/read_string on the Rust side (a single
   checked u32 reader covers both int and uint fields here, exactly as
   wire.rs's own read_int is read_uint(...).map(|v| v as i32) --
   little-endian bytes are identical either way; only the cast at the
   call site differs). ---- */

static inline void vitrin_raw_write_u32(uint8_t *out, uint32_t v) {
    out[0] = (uint8_t)(v & 0xffu);
    out[1] = (uint8_t)((v >> 8) & 0xffu);
    out[2] = (uint8_t)((v >> 16) & 0xffu);
    out[3] = (uint8_t)((v >> 24) & 0xffu);
}

static inline vitrin_decode_status_t vitrin_raw_read_u32(
    const uint8_t *in, size_t in_len, size_t *pos, uint32_t *out) {
    if (*pos + 4u > in_len) {
        return VITRIN_DECODE_ERR_TRUNCATED;
    }
    *out = (uint32_t)in[*pos]
         | ((uint32_t)in[*pos + 1u] << 8)
         | ((uint32_t)in[*pos + 2u] << 16)
         | ((uint32_t)in[*pos + 3u] << 24);
    *pos += 4u;
    return VITRIN_DECODE_OK;
}

static inline size_t vitrin_raw_pad_len(uint32_t len) {
    return (size_t)((4u - (len % 4u)) % 4u);
}

/* Wire size of one string argument: 4-byte length prefix + bytes + zero
   padding to the next 4-byte boundary. Computed in uint64_t, NOT size_t:
   on a 32-bit target a byte_len near UINT32_MAX would wrap 32-bit size_t
   arithmetic to a tiny value, and every *_encode's total-frame-size guard
   below would then pass a frame whose memcpy runs ~4 GiB past the output
   buffer. 64-bit arithmetic cannot wrap here (max term is well under
   2^33), so the guard stays sound on every target. */
static inline uint64_t vitrin_raw_string_wire_len(uint32_t byte_len) {
    return (uint64_t)4 + (uint64_t)byte_len + (uint64_t)vitrin_raw_pad_len(byte_len);
}

/* Writes a string argument: u32 byte length, the bytes themselves (no NUL
   terminator), zero-padded to the next 4-byte boundary -- the length
   prefix counts only the bytes, never the padding. Returns the total
   number of bytes written (vitrin_raw_string_wire_len(s.len)). */
static inline size_t vitrin_raw_write_string(uint8_t *out, vitrin_string_t s) {
    size_t pad = vitrin_raw_pad_len(s.len);
    vitrin_raw_write_u32(out, s.len);
    if (s.len > 0u) {
        memcpy(out + 4, s.data, s.len);
    }
    if (pad > 0u) {
        memset(out + 4 + (size_t)s.len, 0, pad);
    }
    return (size_t)4 + (size_t)s.len + pad;
}

/* Reads a string argument, enforcing max_bytes (the arg's documented
   `(max N bytes)` bound), buffer bounds, and all-zero padding (malformed
   padding is fatal invalid_argument per conventions 2.2; accepting
   arbitrary padding bytes would also open a covert channel). *out borrows
   directly into `in` (out->data = in + <offset>); it is valid only as
   long as `in` is. Does not validate UTF-8 or reject embedded NUL bytes
   -- see the rationale on vitrin_decode_status_t above. */
static inline vitrin_decode_status_t vitrin_raw_read_string(
    const uint8_t *in, size_t in_len, size_t *pos, uint32_t max_bytes,
    vitrin_string_t *out) {
    uint32_t len;
    size_t pad;
    vitrin_decode_status_t st = vitrin_raw_read_u32(in, in_len, pos, &len);
    if (st != VITRIN_DECODE_OK) {
        return st;
    }
    if (len > max_bytes) {
        return VITRIN_DECODE_ERR_STRING_TOO_LONG;
    }
    if (*pos + (size_t)len > in_len) {
        return VITRIN_DECODE_ERR_TRUNCATED;
    }
    out->len = len;
    out->data = in + *pos;
    *pos += (size_t)len;
    pad = vitrin_raw_pad_len(len);
    if (*pos + pad > in_len) {
        return VITRIN_DECODE_ERR_TRUNCATED;
    }
    for (size_t i = 0; i < pad; i++) {
        if (in[*pos + i] != 0u) {
            return VITRIN_DECODE_ERR_MALFORMED_PADDING;
        }
    }
    *pos += pad;
    return VITRIN_DECODE_OK;
}

/* ---- frame header marshal ---- */

static inline void vitrin_frame_header_encode(const vitrin_frame_header_t *hdr, uint8_t *out) {
    out[0] = (uint8_t)(hdr->object_id & 0xffu);
    out[1] = (uint8_t)((hdr->object_id >> 8) & 0xffu);
    out[2] = (uint8_t)((hdr->object_id >> 16) & 0xffu);
    out[3] = (uint8_t)((hdr->object_id >> 24) & 0xffu);
    out[4] = (uint8_t)(hdr->size & 0xffu);
    out[5] = (uint8_t)((hdr->size >> 8) & 0xffu);
    out[6] = hdr->opcode;
    out[7] = hdr->fd_count;
}

static inline vitrin_decode_status_t vitrin_frame_header_decode(
    const uint8_t *in, size_t in_len, vitrin_frame_header_t *out) {
    if (in_len < VITRIN_HEADER_LEN) {
        return VITRIN_DECODE_ERR_TRUNCATED;
    }
    out->object_id = (uint32_t)in[0]
                   | ((uint32_t)in[1] << 8)
                   | ((uint32_t)in[2] << 16)
                   | ((uint32_t)in[3] << 24);
    out->size = (uint16_t)((uint32_t)in[4] | ((uint32_t)in[5] << 8));
    out->opcode = in[6];
    out->fd_count = in[7];
    return VITRIN_DECODE_OK;
}

/* The `vitrin` protocol's single wire version integer (`protocol/@version`); */
/* also the first argument of vitrin_handshake's hello request, whose */
/* accepted value becomes the connection's negotiated version. A server */
/* implements every version up to its maximum and refuses anything above */
/* it -- downgrade is refusal, not negotiation. */
#define VITRIN_PROTOCOL_VERSION 2u

/* Total number of messages (requests + events) across every interface. */
/* Exists so exhaustiveness can be *asserted* rather than assumed: a C */
/* translation unit enumerating every message (shim/tests/ */
/* test_header_compiles.c) checks its own list length against this with */
/* _Static_assert, so a message added to the IDL cannot ship without a */
/* compile-time proof that its marshal functions type-check. */
#define VITRIN_MESSAGE_COUNT 58

/* Total number of enums (plain and bitfield) across every interface. */
/* The same gate VITRIN_MESSAGE_COUNT gives the message list, for the */
/* enum list beside it. It exists because that list had NO gate and went */
/* stale repeatedly: shim/tests/test_header_compiles.c was silently short */
/* four enums (vitrin_shim_session's three pointer-constraint enums and */
/* its idle_inhibit_state) when P2.6.5 came to append to it, having */
/* already recorded two earlier misses in its own comment. An */
/* untype-checked validity predicate is exactly the class of check that */
/* stops checking while still compiling green. */
#define VITRIN_ENUM_COUNT 26

/* ==================================================================== */
/* Section 1: per-interface metadata and enums.                          */
/*                                                                        */
/* Every enum, across every interface, is emitted here -- before any      */
/* message struct in Section 2 -- because a struct member of enum type    */
/* requires a complete type at the point of use, and vitrin_realm (an     */
/* earlier interface) references enums defined on vitrin_grant (a later   */
/* one). See this module's doc comment for the full rationale. Document   */
/* order is preserved *within* this phase (interface order, then each     */
/* interface's own enum order).                                          */
/* ==================================================================== */

/* ==== vitrin_handshake (version 1) ==== */
/* principal connection bootstrap */

#define VITRIN_HANDSHAKE_INTERFACE_NAME "vitrin_handshake"
#define VITRIN_HANDSHAKE_INTERFACE_VERSION 1u

/* Enum `error` on `vitrin_handshake`.
 *
 * connection-global fatal error codes
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* unknown or foreign object id, id reuse at or below the watermark, reserved-range id, or multi-new_id rule violation */
    VITRIN_HANDSHAKE_ERROR_INVALID_OBJECT = 0,
    /* opcode not defined for the interface at the negotiated version, including other-class opcodes and a second hello (hello's opcode is defined only in the CONNECTED state) */
    VITRIN_HANDSHAKE_ERROR_INVALID_OPCODE = 1,
    /* argument decode failure: bad UTF-8, embedded NUL, string over its bound, out-of-range enum value, forbidden control character, zero verbs, malformed padding */
    VITRIN_HANDSHAKE_ERROR_INVALID_ARGUMENT = 2,
    /* declared frame size below the 8-byte header minimum, or a payload shorter than the size declares; the 65535-byte ceiling binds senders (a u16 cannot express more) */
    VITRIN_HANDSHAKE_ERROR_OVERSIZED = 3,
    /* fd count in the header disagrees with the message signature, or unsolicited fds attached */
    VITRIN_HANDSHAKE_ERROR_FD_VIOLATION = 4,
    /* traffic before a first hello on a principal connection */
    VITRIN_HANDSHAKE_ERROR_PRE_HANDSHAKE = 5,
    /* hello offered a protocol version the server does not implement - i.e. above its maximum, since additive growth means a server implements every version up to its maximum; downgrade is refusal, not negotiation */
    VITRIN_HANDSHAKE_ERROR_VERSION_UNSUPPORTED = 6,
    /* credential rejected: unknown identity, bad token, verifier failure, or SO_PEERCRED mismatch; the cause is never distinguished on the wire - uniform code, fixed message text, detail in the server log only */
    VITRIN_HANDSHAKE_ERROR_AUTH_FAILED = 7,
    /* server-side failure that poisoned the connection */
    VITRIN_HANDSHAKE_ERROR_INTERNAL = 8,
    /* a documented per-connection resource bound was breached: the petition-rate ceiling, the live-object cap, or object-id exhaustion; denial-of-service confinement, not a semantic judgement */
    VITRIN_HANDSHAKE_ERROR_RESOURCE_EXHAUSTED = 9,
} vitrin_handshake_error_t;

/* Whole-value membership check for `vitrin_handshake_error_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_handshake_error_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_HANDSHAKE_ERROR_INVALID_OBJECT:
        case VITRIN_HANDSHAKE_ERROR_INVALID_OPCODE:
        case VITRIN_HANDSHAKE_ERROR_INVALID_ARGUMENT:
        case VITRIN_HANDSHAKE_ERROR_OVERSIZED:
        case VITRIN_HANDSHAKE_ERROR_FD_VIOLATION:
        case VITRIN_HANDSHAKE_ERROR_PRE_HANDSHAKE:
        case VITRIN_HANDSHAKE_ERROR_VERSION_UNSUPPORTED:
        case VITRIN_HANDSHAKE_ERROR_AUTH_FAILED:
        case VITRIN_HANDSHAKE_ERROR_INTERNAL:
        case VITRIN_HANDSHAKE_ERROR_RESOURCE_EXHAUSTED:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_principal (version 2) ==== */
/* the authenticated principal */

#define VITRIN_PRINCIPAL_INTERFACE_NAME "vitrin_principal"
#define VITRIN_PRINCIPAL_INTERFACE_VERSION 2u

/* ==== vitrin_realm (version 1) ==== */
/* realm address */

#define VITRIN_REALM_INTERFACE_NAME "vitrin_realm"
#define VITRIN_REALM_INTERFACE_VERSION 1u

/* ==== vitrin_grant (version 2) ==== */
/* capability handle */

#define VITRIN_GRANT_INTERFACE_NAME "vitrin_grant"
#define VITRIN_GRANT_INTERFACE_VERSION 2u

/* Enum `verb` on `vitrin_grant` (bitfield).
 *
 * grantable verbs
 *
 * Bitfield: any combination of the defined entries' bits is a legal wire
 * value; a bit outside their union is invalid. Represented as a plain
 * uint32_t typedef (not a C enum, which ISO C would require to fit in an
 * `int` and which offers no bitwise-OR-of-two-enumerators guarantee
 * anyway) with one #define per bit, matching how the bits actually get
 * combined by callers. */
typedef uint32_t vitrin_grant_verb_t;

/* capture frames of the granted resource */
#define VITRIN_GRANT_VERB_OBSERVE ((vitrin_grant_verb_t)1)
/* inject pointer motion, buttons, and scroll */
#define VITRIN_GRANT_VERB_ACTUATE_POINTER ((vitrin_grant_verb_t)2)
/* inject Unicode text */
#define VITRIN_GRANT_VERB_ACTUATE_TEXT ((vitrin_grant_verb_t)4)
/* capture frames that include the human principal's cursor - reading the human's attention, hence a verb and not a display preference; meaningful only alongside observe, and a petition naming it without observe resolves unsupported; another agent principal's cursor is not purchasable by this or any verb; refused unsupported in version 1 */
#define VITRIN_GRANT_VERB_OBSERVE_CURSOR ((vitrin_grant_verb_t)8)
/* arrange the granted realm's view, subject to the ordering invariants no grant can purchase; exercised through vitrin_layout_arrange, which defines set_fullscreen and no other request - place, resize, raise and stacking are absent rather than refused, because a scene showing one unstacked realm cannot honour them; at most one holder per output, counting a live grant that carries this verb AND a petition still pending for it, so a second petition while either exists resolves layout_held */
#define VITRIN_GRANT_VERB_LAYOUT_ARRANGE ((vitrin_grant_verb_t)16)
/* bind the output to a view of the granted realm and direct input there - one act, because routing keys to a realm the human cannot see is focus theft in its sharpest form; exercised through vitrin_layout_focus; separate from layout_arrange because focus theft is at once the sharpest attack and the most legitimate need, so it must be attenuable alone */
#define VITRIN_GRANT_VERB_LAYOUT_FOCUS ((vitrin_grant_verb_t)32)
/* designate one file or one directory subtree to the granted realm, through the vitrin_powerbox facet; the human picks in a core-drawn picker and what crosses the wire is a file descriptor, never a path, so this is authority to ASK for a designation rather than authority over any named file; a delivered fd is kernel authority the core cannot recall, so revocation stops future designations and kills the grant row while the payload keeps every fd already handed over until its realm dies - PRD P2's revocation is immediate and transitive is FALSE for designations already made; refused unsupported in version 1, which cannot mint the facet at all, and by every deployment until the picker (P2.6.6) and its consent copy (P2.6.8) exist */
#define VITRIN_GRANT_VERB_DESIGNATE_FILE ((vitrin_grant_verb_t)64)
/* open one outbound connection to the single host:port named by this grant's net: resource selector, through an out-of-core mediating proxy that asks the enforcement chokepoint per connection and holds no grant of its own; exercised through the vitrin_egress facet, which is a separate interface of its own rather than a request on the filesystem powerbox, since interface/@verb is one value per interface; the selector's grammar is wildcard-free, so a blanket egress grant is inexpressible rather than refused, and one selector covers exactly itself - though not every spelling of one endpoint is one selector, since the host is compared byte-exactly and kept as presented; SPECIFIED BUT NOT IMPLEMENTED ANYWHERE YET: a DNS name is to resolve only in the proxy and the addresses it resolved to at grant time are to be pinned into the grant row, so that a connection to an unpinned address is refused not_granted even under a name-scoped grant - no proxy, no resolver and no pinned column with a value exist today; the dotted SDK name is egress unchanged, the wire name carrying no underscore to replace; refused unsupported in version 1 and by every deployment at version 2 - the facet exists now, so what is missing is the proxy behind it rather than a request to ask through */
#define VITRIN_GRANT_VERB_EGRESS ((vitrin_grant_verb_t)128)
/* launch the realm template this grant addresses into a new realm instance, through the vitrin_launcher facet; the template names the program and no command ever crosses the wire, so this is authority over an operator-written template rather than over an arbitrary command; bit 256 is allocated to a verb not yet defined here and was skipped rather than reused, as 64 was until designate_file landed on it and 128 was until egress did; refused unsupported in version 1, which cannot mint the facet at all, and by any deployment that does not serve it */
#define VITRIN_GRANT_VERB_REALM_LAUNCH ((vitrin_grant_verb_t)512)
/* Union of every defined entry's bits; a wire value with any other bit
   set is invalid. */
#define VITRIN_GRANT_VERB_VALID_MASK ((vitrin_grant_verb_t)(1 | 2 | 4 | 8 | 16 | 32 | 64 | 128 | 512))

/* Bitmask validity check for `vitrin_grant_verb_t`: rejects any bit outside
   VITRIN_GRANT_VERB_VALID_MASK. */
static inline bool vitrin_grant_verb_is_valid(uint32_t v) {
    return (v & ~((uint32_t)VITRIN_GRANT_VERB_VALID_MASK)) == 0u;
}

/* Enum `persistence` on `vitrin_grant`.
 *
 * the consent persistence ladder
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* single-use authority */
    VITRIN_GRANT_PERSISTENCE_ONCE = 0,
    /* lives while the requesting principal's connection lives */
    VITRIN_GRANT_PERSISTENCE_WHILE_RUNNING = 1,
    /* durable until explicitly revoked (requires verified provenance; refused in version 1) */
    VITRIN_GRANT_PERSISTENCE_UNTIL_REVOKED = 2,
    /* durable and auto-reissued (requires verified provenance; refused in version 1) */
    VITRIN_GRANT_PERSISTENCE_ALWAYS = 3,
} vitrin_grant_persistence_t;

/* Whole-value membership check for `vitrin_grant_persistence_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_grant_persistence_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_GRANT_PERSISTENCE_ONCE:
        case VITRIN_GRANT_PERSISTENCE_WHILE_RUNNING:
        case VITRIN_GRANT_PERSISTENCE_UNTIL_REVOKED:
        case VITRIN_GRANT_PERSISTENCE_ALWAYS:
            return true;
        default:
            return false;
    }
}

/* Enum `outcome` on `vitrin_grant`.
 *
 * petition outcomes
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* authority active; the event carries the effective verbs, rung, and expiry */
    VITRIN_GRANT_OUTCOME_GRANTED = 0,
    /* the human said no */
    VITRIN_GRANT_OUTCOME_DENIED = 1,
    /* the consent prompt expired unanswered; petitioning again later is legal */
    VITRIN_GRANT_OUTCOME_TIMED_OUT = 2,
    /* the realm was unknown, vacant, or closed while the petition was pending */
    VITRIN_GRANT_OUTCOME_UNAVAILABLE = 3,
    /* in-range but refused by policy: durable rung without provenance, reserved flag set, unserved resource prefix, or a defined verb this deployment or resource does not serve (an out-of-range verb bit is instead fatal invalid_argument) */
    VITRIN_GRANT_OUTCOME_UNSUPPORTED = 4,
    /* the pending-petition admission cap for this verified identity (across all of its connections) was reached */
    VITRIN_GRANT_OUTCOME_BUSY = 5,
    /* layout_arrange is already spoken for on this output, and there is at most one holder per output; the holder may be a live grant that carries the verb OR a petition still pending for it, because two petitions racing through a human's two approvals would otherwise mint two holders - so a petition that is only waiting really does hold the slot. A distinct entry rather than a reuse of busy, whose meaning is the consent-fatigue valve, and answered at admission rather than at use because contention is about who HOLDS the authority rather than about one use of it - it never reaches a prompt, so it costs the human nothing. Retrying once the holder's grant expires, is revoked, or its connection ends - or once the pending petition resolves to anything other than granted - is legal, and this outcome is the ONLY thing the core says about arbitration: choosing between two would-be holders is window-management policy and belongs outside the core */
    VITRIN_GRANT_OUTCOME_LAYOUT_HELD = 6,
} vitrin_grant_outcome_t;

/* Whole-value membership check for `vitrin_grant_outcome_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_grant_outcome_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_GRANT_OUTCOME_GRANTED:
        case VITRIN_GRANT_OUTCOME_DENIED:
        case VITRIN_GRANT_OUTCOME_TIMED_OUT:
        case VITRIN_GRANT_OUTCOME_UNAVAILABLE:
        case VITRIN_GRANT_OUTCOME_UNSUPPORTED:
        case VITRIN_GRANT_OUTCOME_BUSY:
        case VITRIN_GRANT_OUTCOME_LAYOUT_HELD:
            return true;
        default:
            return false;
    }
}

/* Enum `refusal` on `vitrin_grant`.
 *
 * use-time refusal codes
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* the grant is not (or not yet) active, or the verb is outside its effective set: use while pending, through an ungranted facet, or after any non-granted resolution (denied, timed_out, unavailable, unsupported, busy) */
    VITRIN_GRANT_REFUSAL_NOT_GRANTED = 0,
    /* the grant's expiry passed; checked on use and by a proactive timer */
    VITRIN_GRANT_REFUSAL_EXPIRED = 1,
    /* revoked by hold-Esc, panel, or policy; effective on the very next request */
    VITRIN_GRANT_REFUSAL_REVOKED = 2,
    /* the token bucket is empty; retry_after_ms hints the refill */
    VITRIN_GRANT_REFUSAL_RATE_LIMITED = 3,
    /* physical human input owns the target right now */
    VITRIN_GRANT_REFUSAL_PREEMPTED = 4,
    /* the principal's own pending petition has a prompt up; that principal's actuation is refused (never delivered to the app) until the prompt closes; other principals' grants are unaffected */
    VITRIN_GRANT_REFUSAL_CONSENT_HELD = 5,
    /* the realm has no surface (its shim crashed or exited); never a stale frame */
    VITRIN_GRANT_REFUSAL_NO_SURFACE = 6,
    /* server-side failure during this use (renderer, memfd, delivery) */
    VITRIN_GRANT_REFUSAL_INTERNAL = 7,
    /* the deployment is at its realm capacity, so no new realm can be created; a policy answer rather than a server-side failure, which is why it is not internal - retrying is legal once a realm exits, and retry_after_ms is 0 because the core cannot know when that will be. NOTE, a deliberate exception: every other code answers from the asking principal's OWN grant, but this one answers from deployment-wide state, so a principal holding one launch grant can poll launch and watch the answer flip - observing that SOME other principal created or exited a realm. That is a low-bandwidth cross-principal side channel, inherent to answering the question at all, and it is named here rather than left to be discovered; a deployment that cannot afford it must not serve realm_launch, because no attenuation of a launch grant removes it */
    VITRIN_GRANT_REFUSAL_CAPACITY = 8,
} vitrin_grant_refusal_t;

/* Whole-value membership check for `vitrin_grant_refusal_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_grant_refusal_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_GRANT_REFUSAL_NOT_GRANTED:
        case VITRIN_GRANT_REFUSAL_EXPIRED:
        case VITRIN_GRANT_REFUSAL_REVOKED:
        case VITRIN_GRANT_REFUSAL_RATE_LIMITED:
        case VITRIN_GRANT_REFUSAL_PREEMPTED:
        case VITRIN_GRANT_REFUSAL_CONSENT_HELD:
        case VITRIN_GRANT_REFUSAL_NO_SURFACE:
        case VITRIN_GRANT_REFUSAL_INTERNAL:
        case VITRIN_GRANT_REFUSAL_CAPACITY:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_consent (version 1) ==== */
/* consent prompt visibility (events only) */

#define VITRIN_CONSENT_INTERFACE_NAME "vitrin_consent"
#define VITRIN_CONSENT_INTERFACE_VERSION 1u

/* Enum `consent_state` on `vitrin_consent`.
 *
 * prompt visibility states
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* waiting behind another prompt or a policy decision */
    VITRIN_CONSENT_CONSENT_STATE_QUEUED = 0,
    /* visible; physical input is grabbed by the prompt */
    VITRIN_CONSENT_CONSENT_STATE_SHOWN = 1,
    /* gone; the decision arrives on the grant */
    VITRIN_CONSENT_CONSENT_STATE_CLOSED = 2,
} vitrin_consent_consent_state_t;

/* Whole-value membership check for `vitrin_consent_consent_state_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_consent_consent_state_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_CONSENT_CONSENT_STATE_QUEUED:
        case VITRIN_CONSENT_CONSENT_STATE_SHOWN:
        case VITRIN_CONSENT_CONSENT_STATE_CLOSED:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_view (version 1) ==== */
/* observation facet (poll-model capture) */

#define VITRIN_VIEW_INTERFACE_NAME "vitrin_view"
#define VITRIN_VIEW_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `observe`. */
#define VITRIN_VIEW_VERB "observe"

/* Enum `format` on `vitrin_view`.
 *
 * pixel formats (DRM fourcc values)
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* 32-bit xRGB, DRM_FORMAT_XRGB8888 */
    VITRIN_VIEW_FORMAT_XRGB8888 = 0x34325258,
    /* 32-bit ARGB, DRM_FORMAT_ARGB8888 */
    VITRIN_VIEW_FORMAT_ARGB8888 = 0x34325241,
} vitrin_view_format_t;

/* Whole-value membership check for `vitrin_view_format_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_view_format_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_VIEW_FORMAT_XRGB8888:
        case VITRIN_VIEW_FORMAT_ARGB8888:
            return true;
        default:
            return false;
    }
}

/* Enum `frame_flags` on `vitrin_view` (bitfield).
 *
 * frame flags (reserved in version 1)
 *
 * Bitfield: any combination of the defined entries' bits is a legal wire
 * value; a bit outside their union is invalid. Represented as a plain
 * uint32_t typedef (not a C enum, which ISO C would require to fit in an
 * `int` and which offers no bitwise-OR-of-two-enumerators guarantee
 * anyway) with one #define per bit, matching how the bits actually get
 * combined by callers. */
typedef uint32_t vitrin_view_frame_flags_t;

/* rows are bottom-up (reserved; never set in version 1) */
#define VITRIN_VIEW_FRAME_FLAGS_Y_INVERT ((vitrin_view_frame_flags_t)1)
/* fd is a dmabuf, not a memfd (reserved; never set in version 1) */
#define VITRIN_VIEW_FRAME_FLAGS_DMABUF ((vitrin_view_frame_flags_t)2)
/* Union of every defined entry's bits; a wire value with any other bit
   set is invalid. */
#define VITRIN_VIEW_FRAME_FLAGS_VALID_MASK ((vitrin_view_frame_flags_t)(1 | 2))

/* Bitmask validity check for `vitrin_view_frame_flags_t`: rejects any bit outside
   VITRIN_VIEW_FRAME_FLAGS_VALID_MASK. */
static inline bool vitrin_view_frame_flags_is_valid(uint32_t v) {
    return (v & ~((uint32_t)VITRIN_VIEW_FRAME_FLAGS_VALID_MASK)) == 0u;
}

/* ==== vitrin_actuator_pointer (version 1) ==== */
/* pointer actuation facet */

#define VITRIN_ACTUATOR_POINTER_INTERFACE_NAME "vitrin_actuator_pointer"
#define VITRIN_ACTUATOR_POINTER_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `actuate_pointer`. */
#define VITRIN_ACTUATOR_POINTER_VERB "actuate_pointer"

/* Enum `button_state` on `vitrin_actuator_pointer`.
 *
 * button states
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* button released */
    VITRIN_ACTUATOR_POINTER_BUTTON_STATE_RELEASED = 0,
    /* button pressed */
    VITRIN_ACTUATOR_POINTER_BUTTON_STATE_PRESSED = 1,
} vitrin_actuator_pointer_button_state_t;

/* Whole-value membership check for `vitrin_actuator_pointer_button_state_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_actuator_pointer_button_state_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_ACTUATOR_POINTER_BUTTON_STATE_RELEASED:
        case VITRIN_ACTUATOR_POINTER_BUTTON_STATE_PRESSED:
            return true;
        default:
            return false;
    }
}

/* Enum `axis` on `vitrin_actuator_pointer`.
 *
 * scroll axes
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* vertical scroll */
    VITRIN_ACTUATOR_POINTER_AXIS_VERTICAL = 0,
    /* horizontal scroll */
    VITRIN_ACTUATOR_POINTER_AXIS_HORIZONTAL = 1,
} vitrin_actuator_pointer_axis_t;

/* Whole-value membership check for `vitrin_actuator_pointer_axis_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_actuator_pointer_axis_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_ACTUATOR_POINTER_AXIS_VERTICAL:
        case VITRIN_ACTUATOR_POINTER_AXIS_HORIZONTAL:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_actuator_text (version 1) ==== */
/* text actuation facet */

#define VITRIN_ACTUATOR_TEXT_INTERFACE_NAME "vitrin_actuator_text"
#define VITRIN_ACTUATOR_TEXT_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `actuate_text`. */
#define VITRIN_ACTUATOR_TEXT_VERB "actuate_text"

/* ==== vitrin_shim_session (version 2) ==== */
/* shim connection bootstrap */

#define VITRIN_SHIM_SESSION_INTERFACE_NAME "vitrin_shim_session"
#define VITRIN_SHIM_SESSION_INTERFACE_VERSION 2u

/* Enum `selection_status` on `vitrin_shim_session`.
 *
 * why a selection answer carries no data
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* mime and data carry the app's selection */
    VITRIN_SHIM_SESSION_SELECTION_STATUS_OK = 0,
    /* the app has no selection at all */
    VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY = 1,
    /* the selection is not well-formed text/plain;charset=utf-8 */
    VITRIN_SHIM_SESSION_SELECTION_STATUS_WRONG_TYPE = 2,
    /* the selection exceeds data's byte bound */
    VITRIN_SHIM_SESSION_SELECTION_STATUS_TOO_LARGE = 3,
} vitrin_shim_session_selection_status_t;

/* Whole-value membership check for `vitrin_shim_session_selection_status_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_session_selection_status_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SESSION_SELECTION_STATUS_OK:
        case VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY:
        case VITRIN_SHIM_SESSION_SELECTION_STATUS_WRONG_TYPE:
        case VITRIN_SHIM_SESSION_SELECTION_STATUS_TOO_LARGE:
            return true;
        default:
            return false;
    }
}

/* Enum `pointer_constraint_kind` on `vitrin_shim_session`.
 *
 * what a pointer_constraint asks for, including nothing
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* withdraw this connection's constraint; surface MUST be null */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_NONE = 0,
    /* pin the pointer; movement reaches the app as relative_motion only */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_LOCK = 1,
    /* keep the pointer inside the region; absolute motion continues within it */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_CONFINE = 2,
} vitrin_shim_session_pointer_constraint_kind_t;

/* Whole-value membership check for `vitrin_shim_session_pointer_constraint_kind_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_session_pointer_constraint_kind_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_NONE:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_LOCK:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_KIND_CONFINE:
            return true;
        default:
            return false;
    }
}

/* Enum `pointer_constraint_lifetime` on `vitrin_shim_session`.
 *
 * whether a constraint survives its own deactivation
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* ends for good at its first deactivation */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_ONESHOT = 0,
    /* may deactivate and reactivate with no new ask */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_PERSISTENT = 1,
} vitrin_shim_session_pointer_constraint_lifetime_t;

/* Whole-value membership check for `vitrin_shim_session_pointer_constraint_lifetime_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_session_pointer_constraint_lifetime_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_ONESHOT:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_LIFETIME_PERSISTENT:
            return true;
        default:
            return false;
    }
}

/* Enum `pointer_constraint_status` on `vitrin_shim_session`.
 *
 * what the core did with a pointer_constraint, and what is in force
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* recorded but not in force; may become active later with no new ask */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_INACTIVE = 0,
    /* in force: absolute motion stops, relative_motion continues, the core hides its own cursor sprite */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_ACTIVE = 1,
    /* the record is gone: the shim withdrew it, or what it named went away */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_WITHDRAWN = 2,
    /* not recorded at all; the app's object stays inert and this serial is not re-asked */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_REFUSED = 3,
    /* a later ask on this connection replaced it; this serial gets nothing further */
    VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_SUPERSEDED = 4,
} vitrin_shim_session_pointer_constraint_status_t;

/* Whole-value membership check for `vitrin_shim_session_pointer_constraint_status_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_session_pointer_constraint_status_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_INACTIVE:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_ACTIVE:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_WITHDRAWN:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_REFUSED:
        case VITRIN_SHIM_SESSION_POINTER_CONSTRAINT_STATUS_SUPERSEDED:
            return true;
        default:
            return false;
    }
}

/* Enum `idle_inhibit_state` on `vitrin_shim_session`.
 *
 * whether a realm is holding an idle inhibit
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* this realm holds no inhibit; surface MUST be null */
    VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_RELEASED = 0,
    /* this realm asks that the screen not blank while its output is on the panel */
    VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_HELD = 1,
} vitrin_shim_session_idle_inhibit_state_t;

/* Whole-value membership check for `vitrin_shim_session_idle_inhibit_state_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_session_idle_inhibit_state_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_RELEASED:
        case VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_HELD:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_shim_surface (version 1) ==== */
/* shim-to-core buffer path */

#define VITRIN_SHIM_SURFACE_INTERFACE_NAME "vitrin_shim_surface"
#define VITRIN_SHIM_SURFACE_INTERFACE_VERSION 1u

/* Enum `kind` on `vitrin_shim_surface`.
 *
 * attached fd kinds
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* memfd; the core copies pixels in at commit */
    VITRIN_SHIM_SURFACE_KIND_SHM = 0,
    /* dmabuf; the core imports it zero-copy, single-plane, linear modifier implied */
    VITRIN_SHIM_SURFACE_KIND_DMABUF = 1,
} vitrin_shim_surface_kind_t;

/* Whole-value membership check for `vitrin_shim_surface_kind_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_surface_kind_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SURFACE_KIND_SHM:
        case VITRIN_SHIM_SURFACE_KIND_DMABUF:
            return true;
        default:
            return false;
    }
}

/* Enum `buffer_status` on `vitrin_shim_surface`.
 *
 * attach dispositions
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* the buffer was used (or superseded) and may be reused */
    VITRIN_SHIM_SURFACE_BUFFER_STATUS_RELEASED = 0,
    /* dmabuf import failed; fall back to shm; buffer unused and released */
    VITRIN_SHIM_SURFACE_BUFFER_STATUS_IMPORT_FAILED = 1,
    /* format not usable by the renderer; fall back to shm; buffer unused and released */
    VITRIN_SHIM_SURFACE_BUFFER_STATUS_FORMAT_UNSUPPORTED = 2,
    /* buffer exceeds the renderer's limits; fall back to shm; buffer unused and released */
    VITRIN_SHIM_SURFACE_BUFFER_STATUS_TOO_LARGE = 3,
} vitrin_shim_surface_buffer_status_t;

/* Whole-value membership check for `vitrin_shim_surface_buffer_status_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_surface_buffer_status_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SURFACE_BUFFER_STATUS_RELEASED:
        case VITRIN_SHIM_SURFACE_BUFFER_STATUS_IMPORT_FAILED:
        case VITRIN_SHIM_SURFACE_BUFFER_STATUS_FORMAT_UNSUPPORTED:
        case VITRIN_SHIM_SURFACE_BUFFER_STATUS_TOO_LARGE:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_shim_seat (version 2) ==== */
/* input delivery to the shim (events only, origin-tagged) */

#define VITRIN_SHIM_SEAT_INTERFACE_NAME "vitrin_shim_seat"
#define VITRIN_SHIM_SEAT_INTERFACE_VERSION 2u

/* Enum `key_state` on `vitrin_shim_seat`.
 *
 * key states
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* key released */
    VITRIN_SHIM_SEAT_KEY_STATE_RELEASED = 0,
    /* key pressed */
    VITRIN_SHIM_SEAT_KEY_STATE_PRESSED = 1,
} vitrin_shim_seat_key_state_t;

/* Whole-value membership check for `vitrin_shim_seat_key_state_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_seat_key_state_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SEAT_KEY_STATE_RELEASED:
        case VITRIN_SHIM_SEAT_KEY_STATE_PRESSED:
            return true;
        default:
            return false;
    }
}

/* Enum `origin` on `vitrin_shim_seat`.
 *
 * input origin (physical versus emulated)
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* produced by a physical human input device */
    VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL = 0,
    /* produced by a principal's actuator */
    VITRIN_SHIM_SEAT_ORIGIN_EMULATED = 1,
} vitrin_shim_seat_origin_t;

/* Whole-value membership check for `vitrin_shim_seat_origin_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_seat_origin_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL:
        case VITRIN_SHIM_SEAT_ORIGIN_EMULATED:
            return true;
        default:
            return false;
    }
}

/* Enum `gesture_kind` on `vitrin_shim_seat`.
 *
 * which gesture a shared begin or end names
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* multi-finger swipe; motion arrives as gesture_swipe_update */
    VITRIN_SHIM_SEAT_GESTURE_KIND_SWIPE = 0,
    /* pinch; motion arrives as gesture_pinch_update */
    VITRIN_SHIM_SEAT_GESTURE_KIND_PINCH = 1,
} vitrin_shim_seat_gesture_kind_t;

/* Whole-value membership check for `vitrin_shim_seat_gesture_kind_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_seat_gesture_kind_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SEAT_GESTURE_KIND_SWIPE:
        case VITRIN_SHIM_SEAT_GESTURE_KIND_PINCH:
            return true;
        default:
            return false;
    }
}

/* Enum `gesture_state` on `vitrin_shim_seat`.
 *
 * how a gesture ended
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* the human finished the gesture */
    VITRIN_SHIM_SEAT_GESTURE_STATE_COMPLETED = 0,
    /* the gesture did not finish; a preview should be undone */
    VITRIN_SHIM_SEAT_GESTURE_STATE_CANCELLED = 1,
} vitrin_shim_seat_gesture_state_t;

/* Whole-value membership check for `vitrin_shim_seat_gesture_state_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_shim_seat_gesture_state_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_SHIM_SEAT_GESTURE_STATE_COMPLETED:
        case VITRIN_SHIM_SEAT_GESTURE_STATE_CANCELLED:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_launcher (version 1) ==== */
/* realm-launch facet */

#define VITRIN_LAUNCHER_INTERFACE_NAME "vitrin_launcher"
#define VITRIN_LAUNCHER_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `realm_launch`. */
#define VITRIN_LAUNCHER_VERB "realm_launch"

/* ==== vitrin_layout_focus (version 1) ==== */
/* focus facet */

#define VITRIN_LAYOUT_FOCUS_INTERFACE_NAME "vitrin_layout_focus"
#define VITRIN_LAYOUT_FOCUS_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `layout_focus`. */
#define VITRIN_LAYOUT_FOCUS_VERB "layout_focus"

/* ==== vitrin_layout_arrange (version 1) ==== */
/* arrangement facet */

#define VITRIN_LAYOUT_ARRANGE_INTERFACE_NAME "vitrin_layout_arrange"
#define VITRIN_LAYOUT_ARRANGE_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `layout_arrange`. */
#define VITRIN_LAYOUT_ARRANGE_VERB "layout_arrange"

/* Enum `mode` on `vitrin_layout_arrange`.
 *
 * the two arrangements this scene can express
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* compose the realm's view at the size its own app last committed, letterboxed centered and unscaled inside the output */
    VITRIN_LAYOUT_ARRANGE_MODE_WINDOWED = 0,
    /* configure the realm's view to the output's size, so the app fills the output */
    VITRIN_LAYOUT_ARRANGE_MODE_FULLSCREEN = 1,
} vitrin_layout_arrange_mode_t;

/* Whole-value membership check for `vitrin_layout_arrange_mode_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_layout_arrange_mode_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_LAYOUT_ARRANGE_MODE_WINDOWED:
        case VITRIN_LAYOUT_ARRANGE_MODE_FULLSCREEN:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_powerbox (version 1) ==== */
/* powerbox facet */

#define VITRIN_POWERBOX_INTERFACE_NAME "vitrin_powerbox"
#define VITRIN_POWERBOX_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `designate_file`. */
#define VITRIN_POWERBOX_VERB "designate_file"

/* Enum `mode` on `vitrin_powerbox`.
 *
 * the access a designation carries
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* the descriptor is opened for reading */
    VITRIN_POWERBOX_MODE_READ = 0,
    /* the descriptor is opened for reading and writing, so the holder may change or truncate what it names */
    VITRIN_POWERBOX_MODE_READ_WRITE = 1,
} vitrin_powerbox_mode_t;

/* Whole-value membership check for `vitrin_powerbox_mode_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_powerbox_mode_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_POWERBOX_MODE_READ:
        case VITRIN_POWERBOX_MODE_READ_WRITE:
            return true;
        default:
            return false;
    }
}

/* Enum `kind` on `vitrin_powerbox`.
 *
 * what a designated descriptor names
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* a single file */
    VITRIN_POWERBOX_KIND_FILE = 0,
    /* a directory, designating the whole subtree beneath it as one descriptor */
    VITRIN_POWERBOX_KIND_DIRECTORY = 1,
} vitrin_powerbox_kind_t;

/* Whole-value membership check for `vitrin_powerbox_kind_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_powerbox_kind_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_POWERBOX_KIND_FILE:
        case VITRIN_POWERBOX_KIND_DIRECTORY:
            return true;
        default:
            return false;
    }
}

/* Enum `refusal` on `vitrin_powerbox`.
 *
 * why a raised picker produced no descriptor
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* the human dismissed the picker without choosing; the ordinary answer, and asking again later is legal */
    VITRIN_POWERBOX_REFUSAL_CANCELLED = 0,
    /* the picker was raised and expired unanswered, on the deployment's own deadline; distinct from cancelled because nobody decided anything */
    VITRIN_POWERBOX_REFUSAL_TIMED_OUT = 1,
    /* a picker for this principal is already up; at most one at a time, because two stacked in front of one human is the consent-fatigue shape the busy petition outcome already names */
    VITRIN_POWERBOX_REFUSAL_BUSY = 2,
    /* the human chose, and the core would not designate it: the entry could not be resolved without following a symlink or losing a race between the confirmation and the open, so the core refuses rather than delivering a descriptor that may not name what the human saw; says nothing about whether the entry exists */
    VITRIN_POWERBOX_REFUSAL_UNRESOLVABLE = 3,
} vitrin_powerbox_refusal_t;

/* Whole-value membership check for `vitrin_powerbox_refusal_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_powerbox_refusal_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_POWERBOX_REFUSAL_CANCELLED:
        case VITRIN_POWERBOX_REFUSAL_TIMED_OUT:
        case VITRIN_POWERBOX_REFUSAL_BUSY:
        case VITRIN_POWERBOX_REFUSAL_UNRESOLVABLE:
            return true;
        default:
            return false;
    }
}

/* ==== vitrin_egress (version 1) ==== */
/* egress facet */

#define VITRIN_EGRESS_INTERFACE_NAME "vitrin_egress"
#define VITRIN_EGRESS_INTERFACE_VERSION 1u
/* Every request on this interface exercises the grant verb `egress`. */
#define VITRIN_EGRESS_VERB "egress"

/* Enum `failure` on `vitrin_egress`.
 *
 * why an admitted connection did not complete
 *
 * Plain enum: a wire value MUST exactly equal one defined entry. */
typedef enum {
    /* the far end actively refused the connection (nothing listening on that port) */
    VITRIN_EGRESS_FAILURE_REFUSED = 0,
    /* no route to the host or the network; the packet had nowhere to go */
    VITRIN_EGRESS_FAILURE_UNREACHABLE = 1,
    /* the connection attempt exceeded the proxy's deadline with no answer either way */
    VITRIN_EGRESS_FAILURE_TIMED_OUT = 2,
    /* the selector named a DNS name and resolution - which happens only in the proxy, never inside the realm - did not yield an address */
    VITRIN_EGRESS_FAILURE_RESOLUTION_FAILED = 3,
} vitrin_egress_failure_t;

/* Whole-value membership check for `vitrin_egress_failure_t` (decode a wire value by
   whether it equals one of the defined entries above). */
static inline bool vitrin_egress_failure_is_valid(uint32_t v) {
    switch (v) {
        case VITRIN_EGRESS_FAILURE_REFUSED:
        case VITRIN_EGRESS_FAILURE_UNREACHABLE:
        case VITRIN_EGRESS_FAILURE_TIMED_OUT:
        case VITRIN_EGRESS_FAILURE_RESOLUTION_FAILED:
            return true;
        default:
            return false;
    }
}

/* ==================================================================== */
/* Section 2: message structs and marshal functions, in document order   */
/* (interfaces in document order; within an interface, requests then     */
/* events, each in document order -- opcode assignment IS document       */
/* order, independently per request/event list).                        */
/* ==================================================================== */

/* ==== vitrin_handshake messages ==== */

/* Request `hello` (opcode 0) on `vitrin_handshake`.
 *
 * authenticate and bind a principal
 */
typedef struct {
    /* protocol version the connection will speak (the negotiated version); a version the server does not implement (above its maximum) is fatal version_unsupported */
    uint32_t version;
    /* principal object bound on success (new_id: vitrin_principal) */
    uint32_t principal;
    /* claimed identity URI, e.g. vitrin://local/agent/demo (max 2048 bytes) */
    vitrin_string_t identity;
    /* credential scheme discriminator (max 32 bytes) */
    vitrin_string_t credential_type;
    /* opaque scheme-defined credential bytes (max 32768 bytes) */
    vitrin_string_t credential;
} vitrin_handshake_req_hello_t;

#define VITRIN_HANDSHAKE_REQ_HELLO_OPCODE ((uint8_t)0)
#define VITRIN_HANDSHAKE_REQ_HELLO_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_HANDSHAKE_REQ_HELLO_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_handshake_req_hello_encode(const vitrin_handshake_req_hello_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->identity.len > 2048u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    if (msg->credential_type.len > 32u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    if (msg->credential.len > 32768u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + vitrin_raw_string_wire_len(msg->identity.len) + vitrin_raw_string_wire_len(msg->credential_type.len) + vitrin_raw_string_wire_len(msg->credential.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_HANDSHAKE_REQ_HELLO_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_HANDSHAKE_REQ_HELLO_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->version);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->principal);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->identity);
    pos += vitrin_raw_write_string(out + pos, msg->credential_type);
    pos += vitrin_raw_write_string(out + pos, msg->credential);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_handshake_req_hello_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_handshake_req_hello_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_HANDSHAKE_REQ_HELLO_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_HANDSHAKE_REQ_HELLO_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_HANDSHAKE_REQ_HELLO_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_version = vitrin_raw_read_u32(in, in_len, &pos, &out->version);
    if (st_version != VITRIN_DECODE_OK) { return st_version; }
    vitrin_decode_status_t st_principal = vitrin_raw_read_u32(in, in_len, &pos, &out->principal);
    if (st_principal != VITRIN_DECODE_OK) { return st_principal; }
    vitrin_decode_status_t st_identity = vitrin_raw_read_string(in, in_len, &pos, 2048u, &out->identity);
    if (st_identity != VITRIN_DECODE_OK) { return st_identity; }
    vitrin_decode_status_t st_credential_type = vitrin_raw_read_string(in, in_len, &pos, 32u, &out->credential_type);
    if (st_credential_type != VITRIN_DECODE_OK) { return st_credential_type; }
    vitrin_decode_status_t st_credential = vitrin_raw_read_string(in, in_len, &pos, 32768u, &out->credential);
    if (st_credential != VITRIN_DECODE_OK) { return st_credential; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `sync` (opcode 1) on `vitrin_handshake`.
 *
 * roundtrip barrier
 */
typedef struct {
    /* client-chosen value echoed by done */
    uint32_t cookie;
} vitrin_handshake_req_sync_t;

#define VITRIN_HANDSHAKE_REQ_SYNC_OPCODE ((uint8_t)1)
#define VITRIN_HANDSHAKE_REQ_SYNC_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_HANDSHAKE_REQ_SYNC_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_handshake_req_sync_encode(const vitrin_handshake_req_sync_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_HANDSHAKE_REQ_SYNC_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_HANDSHAKE_REQ_SYNC_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->cookie);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_handshake_req_sync_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_handshake_req_sync_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_HANDSHAKE_REQ_SYNC_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_HANDSHAKE_REQ_SYNC_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_HANDSHAKE_REQ_SYNC_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_cookie = vitrin_raw_read_u32(in, in_len, &pos, &out->cookie);
    if (st_cookie != VITRIN_DECODE_OK) { return st_cookie; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `error` (opcode 0) on `vitrin_handshake`.
 *
 * fatal protocol error; the connection closes
 */
typedef struct {
    /* id of the object where the error occurred */
    uint32_t object_id;
    /* error code, namespaced by the cited object's interface */
    vitrin_handshake_error_t code;
    /* free-form debug description, never parsed (max 1024 bytes) */
    vitrin_string_t message;
} vitrin_handshake_evt_error_t;

#define VITRIN_HANDSHAKE_EVT_ERROR_OPCODE ((uint8_t)0)
#define VITRIN_HANDSHAKE_EVT_ERROR_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_HANDSHAKE_EVT_ERROR_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_handshake_evt_error_encode(const vitrin_handshake_evt_error_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->message.len > 1024u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + vitrin_raw_string_wire_len(msg->message.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_HANDSHAKE_EVT_ERROR_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_HANDSHAKE_EVT_ERROR_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->object_id);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->code);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->message);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_handshake_evt_error_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_handshake_evt_error_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_HANDSHAKE_EVT_ERROR_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_HANDSHAKE_EVT_ERROR_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_HANDSHAKE_EVT_ERROR_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_object_id = vitrin_raw_read_u32(in, in_len, &pos, &out->object_id);
    if (st_object_id != VITRIN_DECODE_OK) { return st_object_id; }
    uint32_t code_raw;
    vitrin_decode_status_t st_code = vitrin_raw_read_u32(in, in_len, &pos, &code_raw);
    if (st_code != VITRIN_DECODE_OK) { return st_code; }
    if (!vitrin_handshake_error_is_valid(code_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->code = (vitrin_handshake_error_t)code_raw;
    vitrin_decode_status_t st_message = vitrin_raw_read_string(in, in_len, &pos, 1024u, &out->message);
    if (st_message != VITRIN_DECODE_OK) { return st_message; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `done` (opcode 1) on `vitrin_handshake`.
 *
 * barrier reply
 */
typedef struct {
    /* the cookie passed to sync */
    uint32_t cookie;
} vitrin_handshake_evt_done_t;

#define VITRIN_HANDSHAKE_EVT_DONE_OPCODE ((uint8_t)1)
#define VITRIN_HANDSHAKE_EVT_DONE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_HANDSHAKE_EVT_DONE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_handshake_evt_done_encode(const vitrin_handshake_evt_done_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_HANDSHAKE_EVT_DONE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_HANDSHAKE_EVT_DONE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->cookie);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_handshake_evt_done_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_handshake_evt_done_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_HANDSHAKE_EVT_DONE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_HANDSHAKE_EVT_DONE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_HANDSHAKE_EVT_DONE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_cookie = vitrin_raw_read_u32(in, in_len, &pos, &out->cookie);
    if (st_cookie != VITRIN_DECODE_OK) { return st_cookie; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_principal messages ==== */

/* Request `get_realm` (opcode 0) on `vitrin_principal`.
 *
 * mint an address handle for a realm
 */
typedef struct {
    /* the new realm address handle (new_id: vitrin_realm) */
    uint32_t realm;
    /* realm name (max 64 bytes); "realm-0" is the well-known one */
    vitrin_string_t name;
} vitrin_principal_req_get_realm_t;

#define VITRIN_PRINCIPAL_REQ_GET_REALM_OPCODE ((uint8_t)0)
#define VITRIN_PRINCIPAL_REQ_GET_REALM_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_PRINCIPAL_REQ_GET_REALM_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_principal_req_get_realm_encode(const vitrin_principal_req_get_realm_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->name.len > 64u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + vitrin_raw_string_wire_len(msg->name.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_PRINCIPAL_REQ_GET_REALM_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_PRINCIPAL_REQ_GET_REALM_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->realm);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->name);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_principal_req_get_realm_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_principal_req_get_realm_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_PRINCIPAL_REQ_GET_REALM_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_PRINCIPAL_REQ_GET_REALM_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_PRINCIPAL_REQ_GET_REALM_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_realm = vitrin_raw_read_u32(in, in_len, &pos, &out->realm);
    if (st_realm != VITRIN_DECODE_OK) { return st_realm; }
    vitrin_decode_status_t st_name = vitrin_raw_read_string(in, in_len, &pos, 64u, &out->name);
    if (st_name != VITRIN_DECODE_OK) { return st_name; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `bound` (opcode 0) on `vitrin_principal`.
 *
 * handshake succeeded
 */
typedef struct {
    /* verifier-canonical principal identity (max 2048 bytes) */
    vitrin_string_t identity;
} vitrin_principal_evt_bound_t;

#define VITRIN_PRINCIPAL_EVT_BOUND_OPCODE ((uint8_t)0)
#define VITRIN_PRINCIPAL_EVT_BOUND_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_PRINCIPAL_EVT_BOUND_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_principal_evt_bound_encode(const vitrin_principal_evt_bound_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->identity.len > 2048u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->identity.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_PRINCIPAL_EVT_BOUND_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_PRINCIPAL_EVT_BOUND_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->identity);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_principal_evt_bound_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_principal_evt_bound_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_PRINCIPAL_EVT_BOUND_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_PRINCIPAL_EVT_BOUND_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_PRINCIPAL_EVT_BOUND_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_identity = vitrin_raw_read_string(in, in_len, &pos, 2048u, &out->identity);
    if (st_identity != VITRIN_DECODE_OK) { return st_identity; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `attention` (opcode 1) on `vitrin_principal`.
 *
 * the human asked for their attention to move
 */
typedef struct {
    /* no arguments -- a truly empty struct is not portable standard C */
    char reserved;
} vitrin_principal_evt_attention_t;

#define VITRIN_PRINCIPAL_EVT_ATTENTION_OPCODE ((uint8_t)1)
#define VITRIN_PRINCIPAL_EVT_ATTENTION_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_PRINCIPAL_EVT_ATTENTION_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_principal_evt_attention_encode(const vitrin_principal_evt_attention_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_PRINCIPAL_EVT_ATTENTION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_PRINCIPAL_EVT_ATTENTION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    (void)msg;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_principal_evt_attention_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_principal_evt_attention_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_PRINCIPAL_EVT_ATTENTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_PRINCIPAL_EVT_ATTENTION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_PRINCIPAL_EVT_ATTENTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->reserved = 0;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_realm messages ==== */

/* Request `request_grant` (opcode 0) on `vitrin_realm`.
 *
 * petition for authority over this realm
 */
typedef struct {
    /* the grant handle, born pending (new_id: vitrin_grant) */
    uint32_t grant;
    /* prompt-visibility observer for this petition (new_id: vitrin_consent) */
    uint32_t consent;
    /* observation facet (inert until granted with observe) (new_id: vitrin_view) */
    uint32_t view;
    /* pointer facet (inert until granted with actuate_pointer) (new_id: vitrin_actuator_pointer) */
    uint32_t pointer;
    /* text facet (inert until granted with actuate_text) (new_id: vitrin_actuator_text) */
    uint32_t text;
    /* resource selector within the realm; null or empty = whole realm (max 256 bytes) */
    vitrin_string_t resource;
    /* requested verb set; MUST be non-zero */
    vitrin_grant_verb_t verbs;
    /* requested lifetime in milliseconds; 0 = bounded by the persistence rung */
    uint32_t expiry_ms;
    /* requested ceiling in events per second for observation and actuation; 0 = server default, never unlimited */
    uint32_t max_event_rate;
    /* requested persistence rung */
    vitrin_grant_persistence_t persistence;
    /* boolean constraint bits; MUST be 0 in version 1 (bit 0 reserved: one_shot) */
    uint32_t flags;
} vitrin_realm_req_request_grant_t;

#define VITRIN_REALM_REQ_REQUEST_GRANT_OPCODE ((uint8_t)0)
#define VITRIN_REALM_REQ_REQUEST_GRANT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_REALM_REQ_REQUEST_GRANT_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_realm_req_request_grant_encode(const vitrin_realm_req_request_grant_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->resource.len > 256u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4 + 4 + vitrin_raw_string_wire_len(msg->resource.len) + 4 + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_REALM_REQ_REQUEST_GRANT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_REALM_REQ_REQUEST_GRANT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->grant);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->consent);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->view);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->pointer);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->text);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->resource);
    vitrin_raw_write_u32(out + pos, msg->verbs);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->expiry_ms);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->max_event_rate);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->persistence);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->flags);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_realm_req_request_grant_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_realm_req_request_grant_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_REALM_REQ_REQUEST_GRANT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_REALM_REQ_REQUEST_GRANT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_REALM_REQ_REQUEST_GRANT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_grant = vitrin_raw_read_u32(in, in_len, &pos, &out->grant);
    if (st_grant != VITRIN_DECODE_OK) { return st_grant; }
    vitrin_decode_status_t st_consent = vitrin_raw_read_u32(in, in_len, &pos, &out->consent);
    if (st_consent != VITRIN_DECODE_OK) { return st_consent; }
    vitrin_decode_status_t st_view = vitrin_raw_read_u32(in, in_len, &pos, &out->view);
    if (st_view != VITRIN_DECODE_OK) { return st_view; }
    vitrin_decode_status_t st_pointer = vitrin_raw_read_u32(in, in_len, &pos, &out->pointer);
    if (st_pointer != VITRIN_DECODE_OK) { return st_pointer; }
    vitrin_decode_status_t st_text = vitrin_raw_read_u32(in, in_len, &pos, &out->text);
    if (st_text != VITRIN_DECODE_OK) { return st_text; }
    vitrin_decode_status_t st_resource = vitrin_raw_read_string(in, in_len, &pos, 256u, &out->resource);
    if (st_resource != VITRIN_DECODE_OK) { return st_resource; }
    uint32_t verbs_raw;
    vitrin_decode_status_t st_verbs = vitrin_raw_read_u32(in, in_len, &pos, &verbs_raw);
    if (st_verbs != VITRIN_DECODE_OK) { return st_verbs; }
    if (!vitrin_grant_verb_is_valid(verbs_raw)) { return VITRIN_DECODE_ERR_INVALID_BITFIELD; }
    out->verbs = (vitrin_grant_verb_t)verbs_raw;
    vitrin_decode_status_t st_expiry_ms = vitrin_raw_read_u32(in, in_len, &pos, &out->expiry_ms);
    if (st_expiry_ms != VITRIN_DECODE_OK) { return st_expiry_ms; }
    vitrin_decode_status_t st_max_event_rate = vitrin_raw_read_u32(in, in_len, &pos, &out->max_event_rate);
    if (st_max_event_rate != VITRIN_DECODE_OK) { return st_max_event_rate; }
    uint32_t persistence_raw;
    vitrin_decode_status_t st_persistence = vitrin_raw_read_u32(in, in_len, &pos, &persistence_raw);
    if (st_persistence != VITRIN_DECODE_OK) { return st_persistence; }
    if (!vitrin_grant_persistence_is_valid(persistence_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->persistence = (vitrin_grant_persistence_t)persistence_raw;
    vitrin_decode_status_t st_flags = vitrin_raw_read_u32(in, in_len, &pos, &out->flags);
    if (st_flags != VITRIN_DECODE_OK) { return st_flags; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_grant messages ==== */

/* Request `get_launcher` (opcode 0) on `vitrin_grant`.
 *
 * mint the launch facet for this grant
 */
typedef struct {
    /* the launch facet, born inert (confers nothing until this grant is granted with realm_launch) (new_id: vitrin_launcher) */
    uint32_t launcher;
} vitrin_grant_req_get_launcher_t;

#define VITRIN_GRANT_REQ_GET_LAUNCHER_OPCODE ((uint8_t)0)
#define VITRIN_GRANT_REQ_GET_LAUNCHER_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_REQ_GET_LAUNCHER_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_req_get_launcher_encode(const vitrin_grant_req_get_launcher_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_REQ_GET_LAUNCHER_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_REQ_GET_LAUNCHER_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->launcher);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_req_get_launcher_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_req_get_launcher_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_REQ_GET_LAUNCHER_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_REQ_GET_LAUNCHER_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_REQ_GET_LAUNCHER_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_launcher = vitrin_raw_read_u32(in, in_len, &pos, &out->launcher);
    if (st_launcher != VITRIN_DECODE_OK) { return st_launcher; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `get_layout_focus` (opcode 1) on `vitrin_grant`.
 *
 * mint the focus facet for this grant
 */
typedef struct {
    /* the focus facet, born inert (confers nothing until this grant is granted with layout_focus) (new_id: vitrin_layout_focus) */
    uint32_t layout_focus;
} vitrin_grant_req_get_layout_focus_t;

#define VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_OPCODE ((uint8_t)1)
#define VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_req_get_layout_focus_encode(const vitrin_grant_req_get_layout_focus_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->layout_focus);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_req_get_layout_focus_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_req_get_layout_focus_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_REQ_GET_LAYOUT_FOCUS_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_layout_focus = vitrin_raw_read_u32(in, in_len, &pos, &out->layout_focus);
    if (st_layout_focus != VITRIN_DECODE_OK) { return st_layout_focus; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `get_layout_arrange` (opcode 2) on `vitrin_grant`.
 *
 * mint the arrangement facet for this grant
 */
typedef struct {
    /* the arrangement facet, born inert (confers nothing until this grant is granted with layout_arrange) (new_id: vitrin_layout_arrange) */
    uint32_t layout_arrange;
} vitrin_grant_req_get_layout_arrange_t;

#define VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_OPCODE ((uint8_t)2)
#define VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_req_get_layout_arrange_encode(const vitrin_grant_req_get_layout_arrange_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->layout_arrange);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_req_get_layout_arrange_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_req_get_layout_arrange_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_REQ_GET_LAYOUT_ARRANGE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_layout_arrange = vitrin_raw_read_u32(in, in_len, &pos, &out->layout_arrange);
    if (st_layout_arrange != VITRIN_DECODE_OK) { return st_layout_arrange; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `get_powerbox` (opcode 3) on `vitrin_grant`.
 *
 * mint the powerbox facet for this grant
 */
typedef struct {
    /* the powerbox facet, born inert (confers nothing until this grant is granted with designate_file) (new_id: vitrin_powerbox) */
    uint32_t powerbox;
} vitrin_grant_req_get_powerbox_t;

#define VITRIN_GRANT_REQ_GET_POWERBOX_OPCODE ((uint8_t)3)
#define VITRIN_GRANT_REQ_GET_POWERBOX_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_REQ_GET_POWERBOX_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_req_get_powerbox_encode(const vitrin_grant_req_get_powerbox_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_REQ_GET_POWERBOX_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_REQ_GET_POWERBOX_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->powerbox);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_req_get_powerbox_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_req_get_powerbox_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_REQ_GET_POWERBOX_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_REQ_GET_POWERBOX_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_REQ_GET_POWERBOX_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_powerbox = vitrin_raw_read_u32(in, in_len, &pos, &out->powerbox);
    if (st_powerbox != VITRIN_DECODE_OK) { return st_powerbox; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `get_egress` (opcode 4) on `vitrin_grant`.
 *
 * mint the egress facet for this grant
 */
typedef struct {
    /* the egress facet, born inert (confers nothing until this grant is granted with egress, which no deployment does yet) (new_id: vitrin_egress) */
    uint32_t egress;
} vitrin_grant_req_get_egress_t;

#define VITRIN_GRANT_REQ_GET_EGRESS_OPCODE ((uint8_t)4)
#define VITRIN_GRANT_REQ_GET_EGRESS_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_REQ_GET_EGRESS_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_req_get_egress_encode(const vitrin_grant_req_get_egress_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_REQ_GET_EGRESS_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_REQ_GET_EGRESS_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->egress);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_req_get_egress_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_req_get_egress_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_REQ_GET_EGRESS_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_REQ_GET_EGRESS_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_REQ_GET_EGRESS_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_egress = vitrin_raw_read_u32(in, in_len, &pos, &out->egress);
    if (st_egress != VITRIN_DECODE_OK) { return st_egress; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `resolved` (opcode 0) on `vitrin_grant`.
 *
 * the petition's terminal outcome
 */
typedef struct {
    /* how the petition resolved */
    vitrin_grant_outcome_t outcome;
    /* effective verb set (0 unless granted) */
    vitrin_grant_verb_t verbs;
    /* effective persistence rung (once unless granted) */
    vitrin_grant_persistence_t persistence;
    /* effective lifetime in milliseconds; 0 = bounded by the rung */
    uint32_t expiry_ms;
} vitrin_grant_evt_resolved_t;

#define VITRIN_GRANT_EVT_RESOLVED_OPCODE ((uint8_t)0)
#define VITRIN_GRANT_EVT_RESOLVED_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_EVT_RESOLVED_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_evt_resolved_encode(const vitrin_grant_evt_resolved_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_EVT_RESOLVED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_EVT_RESOLVED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->outcome);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->verbs);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->persistence);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->expiry_ms);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_evt_resolved_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_evt_resolved_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_EVT_RESOLVED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_EVT_RESOLVED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_EVT_RESOLVED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t outcome_raw;
    vitrin_decode_status_t st_outcome = vitrin_raw_read_u32(in, in_len, &pos, &outcome_raw);
    if (st_outcome != VITRIN_DECODE_OK) { return st_outcome; }
    if (!vitrin_grant_outcome_is_valid(outcome_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->outcome = (vitrin_grant_outcome_t)outcome_raw;
    uint32_t verbs_raw;
    vitrin_decode_status_t st_verbs = vitrin_raw_read_u32(in, in_len, &pos, &verbs_raw);
    if (st_verbs != VITRIN_DECODE_OK) { return st_verbs; }
    if (!vitrin_grant_verb_is_valid(verbs_raw)) { return VITRIN_DECODE_ERR_INVALID_BITFIELD; }
    out->verbs = (vitrin_grant_verb_t)verbs_raw;
    uint32_t persistence_raw;
    vitrin_decode_status_t st_persistence = vitrin_raw_read_u32(in, in_len, &pos, &persistence_raw);
    if (st_persistence != VITRIN_DECODE_OK) { return st_persistence; }
    if (!vitrin_grant_persistence_is_valid(persistence_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->persistence = (vitrin_grant_persistence_t)persistence_raw;
    vitrin_decode_status_t st_expiry_ms = vitrin_raw_read_u32(in, in_len, &pos, &out->expiry_ms);
    if (st_expiry_ms != VITRIN_DECODE_OK) { return st_expiry_ms; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `refused` (opcode 1) on `vitrin_grant`.
 *
 * the chokepoint refused one use of this grant
 */
typedef struct {
    /* the verb whose use was refused */
    vitrin_grant_verb_t verb;
    /* why the use was refused */
    vitrin_grant_refusal_t code;
    /* refill hint in milliseconds; nonzero only for rate_limited */
    uint32_t retry_after_ms;
} vitrin_grant_evt_refused_t;

#define VITRIN_GRANT_EVT_REFUSED_OPCODE ((uint8_t)1)
#define VITRIN_GRANT_EVT_REFUSED_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_GRANT_EVT_REFUSED_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_grant_evt_refused_encode(const vitrin_grant_evt_refused_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_GRANT_EVT_REFUSED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_GRANT_EVT_REFUSED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->verb);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->code);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->retry_after_ms);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_grant_evt_refused_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_grant_evt_refused_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_GRANT_EVT_REFUSED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_GRANT_EVT_REFUSED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_GRANT_EVT_REFUSED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t verb_raw;
    vitrin_decode_status_t st_verb = vitrin_raw_read_u32(in, in_len, &pos, &verb_raw);
    if (st_verb != VITRIN_DECODE_OK) { return st_verb; }
    if (!vitrin_grant_verb_is_valid(verb_raw)) { return VITRIN_DECODE_ERR_INVALID_BITFIELD; }
    out->verb = (vitrin_grant_verb_t)verb_raw;
    uint32_t code_raw;
    vitrin_decode_status_t st_code = vitrin_raw_read_u32(in, in_len, &pos, &code_raw);
    if (st_code != VITRIN_DECODE_OK) { return st_code; }
    if (!vitrin_grant_refusal_is_valid(code_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->code = (vitrin_grant_refusal_t)code_raw;
    vitrin_decode_status_t st_retry_after_ms = vitrin_raw_read_u32(in, in_len, &pos, &out->retry_after_ms);
    if (st_retry_after_ms != VITRIN_DECODE_OK) { return st_retry_after_ms; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_consent messages ==== */

/* Event `state` (opcode 0) on `vitrin_consent`.
 *
 * prompt lifecycle transition
 */
typedef struct {
    /* the new prompt state */
    vitrin_consent_consent_state_t state;
} vitrin_consent_evt_state_t;

#define VITRIN_CONSENT_EVT_STATE_OPCODE ((uint8_t)0)
#define VITRIN_CONSENT_EVT_STATE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_CONSENT_EVT_STATE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_consent_evt_state_encode(const vitrin_consent_evt_state_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_CONSENT_EVT_STATE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_CONSENT_EVT_STATE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_consent_evt_state_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_consent_evt_state_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_CONSENT_EVT_STATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_CONSENT_EVT_STATE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_CONSENT_EVT_STATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_consent_consent_state_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_consent_consent_state_t)state_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_view messages ==== */

/* Request `capture_frame` (opcode 0) on `vitrin_view`.
 *
 * request one frame of the realm view
 */
typedef struct {
    /* no arguments -- a truly empty struct is not portable standard C */
    char reserved;
} vitrin_view_req_capture_frame_t;

#define VITRIN_VIEW_REQ_CAPTURE_FRAME_OPCODE ((uint8_t)0)
#define VITRIN_VIEW_REQ_CAPTURE_FRAME_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_VIEW_REQ_CAPTURE_FRAME_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_view_req_capture_frame_encode(const vitrin_view_req_capture_frame_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_VIEW_REQ_CAPTURE_FRAME_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_VIEW_REQ_CAPTURE_FRAME_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    (void)msg;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_view_req_capture_frame_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_view_req_capture_frame_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_VIEW_REQ_CAPTURE_FRAME_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_VIEW_REQ_CAPTURE_FRAME_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_VIEW_REQ_CAPTURE_FRAME_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->reserved = 0;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `frame_ready` (opcode 0) on `vitrin_view`.
 *
 * one captured frame
 */
typedef struct {
    /* fresh memfd holding the frame; ownership transfers to the receiver (not present in the byte buffer; carried out-of-band via SCM_RIGHTS) */
    int fd;
    /* pixel format (DRM fourcc value) */
    vitrin_view_format_t format;
    /* frame width in pixels */
    uint32_t width;
    /* frame height in pixels */
    uint32_t height;
    /* row stride in bytes; equals width * 4 in version 1 */
    uint32_t stride;
    /* frame flags; always 0 in version 1 */
    vitrin_view_frame_flags_t flags;
} vitrin_view_evt_frame_ready_t;

#define VITRIN_VIEW_EVT_FRAME_READY_OPCODE ((uint8_t)0)
#define VITRIN_VIEW_EVT_FRAME_READY_HAS_FD 1
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_VIEW_EVT_FRAME_READY_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_view_evt_frame_ready_encode(const vitrin_view_evt_frame_ready_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_VIEW_EVT_FRAME_READY_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_VIEW_EVT_FRAME_READY_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    /* fd: fd argument, never written to the byte buffer */
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->format);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->width);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->height);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->stride);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->flags);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_view_evt_frame_ready_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_view_evt_frame_ready_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_VIEW_EVT_FRAME_READY_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_VIEW_EVT_FRAME_READY_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_VIEW_EVT_FRAME_READY_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->fd = fd;
    uint32_t format_raw;
    vitrin_decode_status_t st_format = vitrin_raw_read_u32(in, in_len, &pos, &format_raw);
    if (st_format != VITRIN_DECODE_OK) { return st_format; }
    if (!vitrin_view_format_is_valid(format_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->format = (vitrin_view_format_t)format_raw;
    vitrin_decode_status_t st_width = vitrin_raw_read_u32(in, in_len, &pos, &out->width);
    if (st_width != VITRIN_DECODE_OK) { return st_width; }
    vitrin_decode_status_t st_height = vitrin_raw_read_u32(in, in_len, &pos, &out->height);
    if (st_height != VITRIN_DECODE_OK) { return st_height; }
    vitrin_decode_status_t st_stride = vitrin_raw_read_u32(in, in_len, &pos, &out->stride);
    if (st_stride != VITRIN_DECODE_OK) { return st_stride; }
    uint32_t flags_raw;
    vitrin_decode_status_t st_flags = vitrin_raw_read_u32(in, in_len, &pos, &flags_raw);
    if (st_flags != VITRIN_DECODE_OK) { return st_flags; }
    if (!vitrin_view_frame_flags_is_valid(flags_raw)) { return VITRIN_DECODE_ERR_INVALID_BITFIELD; }
    out->flags = (vitrin_view_frame_flags_t)flags_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_actuator_pointer messages ==== */

/* Request `move` (opcode 0) on `vitrin_actuator_pointer`.
 *
 * move the pointer
 */
typedef struct {
    /* realm-view x in pixels */
    int32_t x;
    /* realm-view y in pixels */
    int32_t y;
} vitrin_actuator_pointer_req_move_t;

#define VITRIN_ACTUATOR_POINTER_REQ_MOVE_OPCODE ((uint8_t)0)
#define VITRIN_ACTUATOR_POINTER_REQ_MOVE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_ACTUATOR_POINTER_REQ_MOVE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_actuator_pointer_req_move_encode(const vitrin_actuator_pointer_req_move_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_ACTUATOR_POINTER_REQ_MOVE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_ACTUATOR_POINTER_REQ_MOVE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->x);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->y);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_actuator_pointer_req_move_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_actuator_pointer_req_move_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_ACTUATOR_POINTER_REQ_MOVE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_ACTUATOR_POINTER_REQ_MOVE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_ACTUATOR_POINTER_REQ_MOVE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t x_raw;
    vitrin_decode_status_t st_x = vitrin_raw_read_u32(in, in_len, &pos, &x_raw);
    if (st_x != VITRIN_DECODE_OK) { return st_x; }
    out->x = (int32_t)x_raw;
    uint32_t y_raw;
    vitrin_decode_status_t st_y = vitrin_raw_read_u32(in, in_len, &pos, &y_raw);
    if (st_y != VITRIN_DECODE_OK) { return st_y; }
    out->y = (int32_t)y_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `button` (opcode 1) on `vitrin_actuator_pointer`.
 *
 * press or release a pointer button
 */
typedef struct {
    /* Linux evdev button code */
    uint32_t button;
    /* pressed or released */
    vitrin_actuator_pointer_button_state_t state;
} vitrin_actuator_pointer_req_button_t;

#define VITRIN_ACTUATOR_POINTER_REQ_BUTTON_OPCODE ((uint8_t)1)
#define VITRIN_ACTUATOR_POINTER_REQ_BUTTON_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_ACTUATOR_POINTER_REQ_BUTTON_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_actuator_pointer_req_button_encode(const vitrin_actuator_pointer_req_button_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_ACTUATOR_POINTER_REQ_BUTTON_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_ACTUATOR_POINTER_REQ_BUTTON_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->button);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_actuator_pointer_req_button_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_actuator_pointer_req_button_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_ACTUATOR_POINTER_REQ_BUTTON_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_ACTUATOR_POINTER_REQ_BUTTON_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_ACTUATOR_POINTER_REQ_BUTTON_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_button = vitrin_raw_read_u32(in, in_len, &pos, &out->button);
    if (st_button != VITRIN_DECODE_OK) { return st_button; }
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_actuator_pointer_button_state_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_actuator_pointer_button_state_t)state_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `scroll` (opcode 2) on `vitrin_actuator_pointer`.
 *
 * scroll
 */
typedef struct {
    /* scroll axis */
    vitrin_actuator_pointer_axis_t axis;
    /* scroll amount; one notch = +-120 */
    int32_t value120;
} vitrin_actuator_pointer_req_scroll_t;

#define VITRIN_ACTUATOR_POINTER_REQ_SCROLL_OPCODE ((uint8_t)2)
#define VITRIN_ACTUATOR_POINTER_REQ_SCROLL_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_ACTUATOR_POINTER_REQ_SCROLL_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_actuator_pointer_req_scroll_encode(const vitrin_actuator_pointer_req_scroll_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_ACTUATOR_POINTER_REQ_SCROLL_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_ACTUATOR_POINTER_REQ_SCROLL_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->axis);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->value120);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_actuator_pointer_req_scroll_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_actuator_pointer_req_scroll_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_ACTUATOR_POINTER_REQ_SCROLL_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_ACTUATOR_POINTER_REQ_SCROLL_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_ACTUATOR_POINTER_REQ_SCROLL_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t axis_raw;
    vitrin_decode_status_t st_axis = vitrin_raw_read_u32(in, in_len, &pos, &axis_raw);
    if (st_axis != VITRIN_DECODE_OK) { return st_axis; }
    if (!vitrin_actuator_pointer_axis_is_valid(axis_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->axis = (vitrin_actuator_pointer_axis_t)axis_raw;
    uint32_t value120_raw;
    vitrin_decode_status_t st_value120 = vitrin_raw_read_u32(in, in_len, &pos, &value120_raw);
    if (st_value120 != VITRIN_DECODE_OK) { return st_value120; }
    out->value120 = (int32_t)value120_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_actuator_text messages ==== */

/* Request `type` (opcode 0) on `vitrin_actuator_text`.
 *
 * deliver a Unicode string
 */
typedef struct {
    /* UTF-8 text to deliver (max 4096 bytes) */
    vitrin_string_t text;
} vitrin_actuator_text_req_type_t;

#define VITRIN_ACTUATOR_TEXT_REQ_TYPE_OPCODE ((uint8_t)0)
#define VITRIN_ACTUATOR_TEXT_REQ_TYPE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_ACTUATOR_TEXT_REQ_TYPE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_actuator_text_req_type_encode(const vitrin_actuator_text_req_type_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->text.len > 4096u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->text.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_ACTUATOR_TEXT_REQ_TYPE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_ACTUATOR_TEXT_REQ_TYPE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->text);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_actuator_text_req_type_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_actuator_text_req_type_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_ACTUATOR_TEXT_REQ_TYPE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_ACTUATOR_TEXT_REQ_TYPE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_ACTUATOR_TEXT_REQ_TYPE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_text = vitrin_raw_read_string(in, in_len, &pos, 4096u, &out->text);
    if (st_text != VITRIN_DECODE_OK) { return st_text; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_shim_session messages ==== */

/* Request `create_surface` (opcode 0) on `vitrin_shim_session`.
 *
 * create a surface for the app's content
 */
typedef struct {
    /* the new surface (new_id: vitrin_shim_surface) */
    uint32_t surface;
} vitrin_shim_session_req_create_surface_t;

#define VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_OPCODE ((uint8_t)0)
#define VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_req_create_surface_encode(const vitrin_shim_session_req_create_surface_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->surface);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_req_create_surface_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_req_create_surface_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_surface = vitrin_raw_read_u32(in, in_len, &pos, &out->surface);
    if (st_surface != VITRIN_DECODE_OK) { return st_surface; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `get_seat` (opcode 1) on `vitrin_shim_session`.
 *
 * mint the session's input-delivery object
 */
typedef struct {
    /* the new seat (new_id: vitrin_shim_seat) */
    uint32_t seat;
} vitrin_shim_session_req_get_seat_t;

#define VITRIN_SHIM_SESSION_REQ_GET_SEAT_OPCODE ((uint8_t)1)
#define VITRIN_SHIM_SESSION_REQ_GET_SEAT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_REQ_GET_SEAT_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_req_get_seat_encode(const vitrin_shim_session_req_get_seat_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_REQ_GET_SEAT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_REQ_GET_SEAT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->seat);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_req_get_seat_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_req_get_seat_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_REQ_GET_SEAT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_REQ_GET_SEAT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_REQ_GET_SEAT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_seat = vitrin_raw_read_u32(in, in_len, &pos, &out->seat);
    if (st_seat != VITRIN_DECODE_OK) { return st_seat; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `selection` (opcode 2) on `vitrin_shim_session`.
 *
 * answer request_selection with the app's current selection
 */
typedef struct {
    /* the serial of the request_selection being answered */
    uint32_t serial;
    /* whether data follows, and why not */
    vitrin_shim_session_selection_status_t status;
    /* MIME type of data, empty unless status is ok (max 32 bytes) */
    vitrin_string_t mime;
    /* the selection as UTF-8, empty unless status is ok (max 61440 bytes) */
    vitrin_string_t data;
} vitrin_shim_session_req_selection_t;

#define VITRIN_SHIM_SESSION_REQ_SELECTION_OPCODE ((uint8_t)2)
#define VITRIN_SHIM_SESSION_REQ_SELECTION_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_REQ_SELECTION_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_req_selection_encode(const vitrin_shim_session_req_selection_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->mime.len > 32u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    if (msg->data.len > 61440u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + vitrin_raw_string_wire_len(msg->mime.len) + vitrin_raw_string_wire_len(msg->data.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_REQ_SELECTION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_REQ_SELECTION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->serial);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->status);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->mime);
    pos += vitrin_raw_write_string(out + pos, msg->data);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_req_selection_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_req_selection_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_REQ_SELECTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_REQ_SELECTION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_REQ_SELECTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_serial = vitrin_raw_read_u32(in, in_len, &pos, &out->serial);
    if (st_serial != VITRIN_DECODE_OK) { return st_serial; }
    uint32_t status_raw;
    vitrin_decode_status_t st_status = vitrin_raw_read_u32(in, in_len, &pos, &status_raw);
    if (st_status != VITRIN_DECODE_OK) { return st_status; }
    if (!vitrin_shim_session_selection_status_is_valid(status_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->status = (vitrin_shim_session_selection_status_t)status_raw;
    vitrin_decode_status_t st_mime = vitrin_raw_read_string(in, in_len, &pos, 32u, &out->mime);
    if (st_mime != VITRIN_DECODE_OK) { return st_mime; }
    vitrin_decode_status_t st_data = vitrin_raw_read_string(in, in_len, &pos, 61440u, &out->data);
    if (st_data != VITRIN_DECODE_OK) { return st_data; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `pointer_constraint` (opcode 3) on `vitrin_shim_session`.
 *
 * ask the core to lock or confine the pointer to a surface
 */
typedef struct {
    /* shim-minted; names the answer this ask expects */
    uint32_t serial;
    /* the surface the constraint applies to; MUST be null when kind is none (object: vitrin_shim_surface; 0 = null) */
    uint32_t surface;
    /* lock, confine, or none to withdraw */
    vitrin_shim_session_pointer_constraint_kind_t kind;
    /* oneshot or persistent; ignored when kind is none */
    vitrin_shim_session_pointer_constraint_lifetime_t lifetime;
    /* region origin x, surface-local pixels */
    int32_t x;
    /* region origin y, surface-local pixels */
    int32_t y;
    /* region width; zero with height zero means the whole surface */
    uint32_t width;
    /* region height; zero with width zero means the whole surface */
    uint32_t height;
} vitrin_shim_session_req_pointer_constraint_t;

#define VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_OPCODE ((uint8_t)3)
#define VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_req_pointer_constraint_encode(const vitrin_shim_session_req_pointer_constraint_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4 + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->serial);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->surface);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->kind);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->lifetime);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->x);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->y);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->width);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->height);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_req_pointer_constraint_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_req_pointer_constraint_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_REQ_POINTER_CONSTRAINT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_serial = vitrin_raw_read_u32(in, in_len, &pos, &out->serial);
    if (st_serial != VITRIN_DECODE_OK) { return st_serial; }
    vitrin_decode_status_t st_surface = vitrin_raw_read_u32(in, in_len, &pos, &out->surface);
    if (st_surface != VITRIN_DECODE_OK) { return st_surface; }
    uint32_t kind_raw;
    vitrin_decode_status_t st_kind = vitrin_raw_read_u32(in, in_len, &pos, &kind_raw);
    if (st_kind != VITRIN_DECODE_OK) { return st_kind; }
    if (!vitrin_shim_session_pointer_constraint_kind_is_valid(kind_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->kind = (vitrin_shim_session_pointer_constraint_kind_t)kind_raw;
    uint32_t lifetime_raw;
    vitrin_decode_status_t st_lifetime = vitrin_raw_read_u32(in, in_len, &pos, &lifetime_raw);
    if (st_lifetime != VITRIN_DECODE_OK) { return st_lifetime; }
    if (!vitrin_shim_session_pointer_constraint_lifetime_is_valid(lifetime_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->lifetime = (vitrin_shim_session_pointer_constraint_lifetime_t)lifetime_raw;
    uint32_t x_raw;
    vitrin_decode_status_t st_x = vitrin_raw_read_u32(in, in_len, &pos, &x_raw);
    if (st_x != VITRIN_DECODE_OK) { return st_x; }
    out->x = (int32_t)x_raw;
    uint32_t y_raw;
    vitrin_decode_status_t st_y = vitrin_raw_read_u32(in, in_len, &pos, &y_raw);
    if (st_y != VITRIN_DECODE_OK) { return st_y; }
    out->y = (int32_t)y_raw;
    vitrin_decode_status_t st_width = vitrin_raw_read_u32(in, in_len, &pos, &out->width);
    if (st_width != VITRIN_DECODE_OK) { return st_width; }
    vitrin_decode_status_t st_height = vitrin_raw_read_u32(in, in_len, &pos, &out->height);
    if (st_height != VITRIN_DECODE_OK) { return st_height; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `idle_inhibit` (opcode 4) on `vitrin_shim_session`.
 *
 * ask the core not to blank this realm's screen while it is being watched
 */
typedef struct {
    /* the surface whose content asks to stay visible; MUST be null when state is released (object: vitrin_shim_surface; 0 = null) */
    uint32_t surface;
    /* whether this realm is holding an idle inhibit */
    vitrin_shim_session_idle_inhibit_state_t state;
} vitrin_shim_session_req_idle_inhibit_t;

#define VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_OPCODE ((uint8_t)4)
#define VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_req_idle_inhibit_encode(const vitrin_shim_session_req_idle_inhibit_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->surface);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_req_idle_inhibit_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_req_idle_inhibit_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_surface = vitrin_raw_read_u32(in, in_len, &pos, &out->surface);
    if (st_surface != VITRIN_DECODE_OK) { return st_surface; }
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_shim_session_idle_inhibit_state_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_shim_session_idle_inhibit_state_t)state_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `configure` (opcode 0) on `vitrin_shim_session`.
 *
 * realm identity and view geometry
 */
typedef struct {
    /* realm identity assigned at fork (max 64 bytes) */
    vitrin_string_t realm;
    /* realm-view width in pixels */
    uint32_t width;
    /* realm-view height in pixels */
    uint32_t height;
} vitrin_shim_session_evt_configure_t;

#define VITRIN_SHIM_SESSION_EVT_CONFIGURE_OPCODE ((uint8_t)0)
#define VITRIN_SHIM_SESSION_EVT_CONFIGURE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_EVT_CONFIGURE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_evt_configure_encode(const vitrin_shim_session_evt_configure_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->realm.len > 64u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->realm.len) + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_EVT_CONFIGURE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_EVT_CONFIGURE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->realm);
    vitrin_raw_write_u32(out + pos, msg->width);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->height);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_evt_configure_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_evt_configure_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_EVT_CONFIGURE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_EVT_CONFIGURE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_EVT_CONFIGURE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_realm = vitrin_raw_read_string(in, in_len, &pos, 64u, &out->realm);
    if (st_realm != VITRIN_DECODE_OK) { return st_realm; }
    vitrin_decode_status_t st_width = vitrin_raw_read_u32(in, in_len, &pos, &out->width);
    if (st_width != VITRIN_DECODE_OK) { return st_width; }
    vitrin_decode_status_t st_height = vitrin_raw_read_u32(in, in_len, &pos, &out->height);
    if (st_height != VITRIN_DECODE_OK) { return st_height; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `request_selection` (opcode 1) on `vitrin_shim_session`.
 *
 * ask this realm for its current selection
 */
typedef struct {
    /* names the answer this request expects */
    uint32_t serial;
} vitrin_shim_session_evt_request_selection_t;

#define VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_OPCODE ((uint8_t)1)
#define VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_evt_request_selection_encode(const vitrin_shim_session_evt_request_selection_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->serial);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_evt_request_selection_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_evt_request_selection_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_EVT_REQUEST_SELECTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_serial = vitrin_raw_read_u32(in, in_len, &pos, &out->serial);
    if (st_serial != VITRIN_DECODE_OK) { return st_serial; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `offer_selection` (opcode 2) on `vitrin_shim_session`.
 *
 * offer the core-held clipboard to this realm
 */
typedef struct {
    /* MIME type of data (max 32 bytes) */
    vitrin_string_t mime;
    /* the clipboard contents as UTF-8 (max 61440 bytes) */
    vitrin_string_t data;
} vitrin_shim_session_evt_offer_selection_t;

#define VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_OPCODE ((uint8_t)2)
#define VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_evt_offer_selection_encode(const vitrin_shim_session_evt_offer_selection_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->mime.len > 32u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    if (msg->data.len > 61440u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->mime.len) + vitrin_raw_string_wire_len(msg->data.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->mime);
    pos += vitrin_raw_write_string(out + pos, msg->data);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_evt_offer_selection_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_evt_offer_selection_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_EVT_OFFER_SELECTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_mime = vitrin_raw_read_string(in, in_len, &pos, 32u, &out->mime);
    if (st_mime != VITRIN_DECODE_OK) { return st_mime; }
    vitrin_decode_status_t st_data = vitrin_raw_read_string(in, in_len, &pos, 61440u, &out->data);
    if (st_data != VITRIN_DECODE_OK) { return st_data; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `pointer_constraint_state` (opcode 3) on `vitrin_shim_session`.
 *
 * the core's verdict on a pointer_constraint, and its running state
 */
typedef struct {
    /* the serial of the pointer_constraint ask this concerns */
    uint32_t serial;
    /* what the core did with that ask, and what is in force now */
    vitrin_shim_session_pointer_constraint_status_t state;
} vitrin_shim_session_evt_pointer_constraint_state_t;

#define VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_OPCODE ((uint8_t)3)
#define VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_evt_pointer_constraint_state_encode(const vitrin_shim_session_evt_pointer_constraint_state_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->serial);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_evt_pointer_constraint_state_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_evt_pointer_constraint_state_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_EVT_POINTER_CONSTRAINT_STATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_serial = vitrin_raw_read_u32(in, in_len, &pos, &out->serial);
    if (st_serial != VITRIN_DECODE_OK) { return st_serial; }
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_shim_session_pointer_constraint_status_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_shim_session_pointer_constraint_status_t)state_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `designation` (opcode 4) on `vitrin_shim_session`.
 *
 * hand this realm one designated file descriptor
 */
typedef struct {
    /* the designated file or directory descriptor; ownership transfers to the receiver, which MUST close it (not present in the byte buffer; carried out-of-band via SCM_RIGHTS) */
    int fd;
    /* the core's opaque id for this designation, matching the journal record and the asking agent's designated event */
    uint32_t designation_id;
    /* whether the descriptor is a file or a directory subtree */
    vitrin_powerbox_kind_t kind;
    /* the EFFECTIVE access the human approved, which may be narrower than what was asked */
    vitrin_powerbox_mode_t mode;
    /* basename of what the human chose, for display only - never a path (max 255 bytes) */
    vitrin_string_t name;
} vitrin_shim_session_evt_designation_t;

#define VITRIN_SHIM_SESSION_EVT_DESIGNATION_OPCODE ((uint8_t)4)
#define VITRIN_SHIM_SESSION_EVT_DESIGNATION_HAS_FD 1
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SESSION_EVT_DESIGNATION_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_session_evt_designation_encode(const vitrin_shim_session_evt_designation_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->name.len > 255u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + vitrin_raw_string_wire_len(msg->name.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SESSION_EVT_DESIGNATION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SESSION_EVT_DESIGNATION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    /* fd: fd argument, never written to the byte buffer */
    vitrin_raw_write_u32(out + pos, msg->designation_id);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->kind);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->mode);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->name);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_session_evt_designation_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_session_evt_designation_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SESSION_EVT_DESIGNATION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SESSION_EVT_DESIGNATION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SESSION_EVT_DESIGNATION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->fd = fd;
    vitrin_decode_status_t st_designation_id = vitrin_raw_read_u32(in, in_len, &pos, &out->designation_id);
    if (st_designation_id != VITRIN_DECODE_OK) { return st_designation_id; }
    uint32_t kind_raw;
    vitrin_decode_status_t st_kind = vitrin_raw_read_u32(in, in_len, &pos, &kind_raw);
    if (st_kind != VITRIN_DECODE_OK) { return st_kind; }
    if (!vitrin_powerbox_kind_is_valid(kind_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->kind = (vitrin_powerbox_kind_t)kind_raw;
    uint32_t mode_raw;
    vitrin_decode_status_t st_mode = vitrin_raw_read_u32(in, in_len, &pos, &mode_raw);
    if (st_mode != VITRIN_DECODE_OK) { return st_mode; }
    if (!vitrin_powerbox_mode_is_valid(mode_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->mode = (vitrin_powerbox_mode_t)mode_raw;
    vitrin_decode_status_t st_name = vitrin_raw_read_string(in, in_len, &pos, 255u, &out->name);
    if (st_name != VITRIN_DECODE_OK) { return st_name; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_shim_surface messages ==== */

/* Request `attach` (opcode 0) on `vitrin_shim_surface`.
 *
 * stage a buffer as pending
 */
typedef struct {
    /* shim-chosen cookie identifying this buffer */
    uint32_t buffer_id;
    /* memfd (kind shm) or dmabuf (kind dmabuf) (not present in the byte buffer; carried out-of-band via SCM_RIGHTS) */
    int fd;
    /* what the fd is */
    vitrin_shim_surface_kind_t kind;
    /* pixel format (DRM fourcc value) */
    vitrin_view_format_t format;
    /* buffer width in pixels */
    uint32_t width;
    /* buffer height in pixels */
    uint32_t height;
    /* row stride in bytes */
    uint32_t stride;
} vitrin_shim_surface_req_attach_t;

#define VITRIN_SHIM_SURFACE_REQ_ATTACH_OPCODE ((uint8_t)0)
#define VITRIN_SHIM_SURFACE_REQ_ATTACH_HAS_FD 1
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SURFACE_REQ_ATTACH_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_surface_req_attach_encode(const vitrin_shim_surface_req_attach_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SURFACE_REQ_ATTACH_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SURFACE_REQ_ATTACH_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->buffer_id);
    pos += 4u;
    /* fd: fd argument, never written to the byte buffer */
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->kind);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->format);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->width);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->height);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->stride);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_surface_req_attach_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_surface_req_attach_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SURFACE_REQ_ATTACH_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SURFACE_REQ_ATTACH_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SURFACE_REQ_ATTACH_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_buffer_id = vitrin_raw_read_u32(in, in_len, &pos, &out->buffer_id);
    if (st_buffer_id != VITRIN_DECODE_OK) { return st_buffer_id; }
    out->fd = fd;
    uint32_t kind_raw;
    vitrin_decode_status_t st_kind = vitrin_raw_read_u32(in, in_len, &pos, &kind_raw);
    if (st_kind != VITRIN_DECODE_OK) { return st_kind; }
    if (!vitrin_shim_surface_kind_is_valid(kind_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->kind = (vitrin_shim_surface_kind_t)kind_raw;
    uint32_t format_raw;
    vitrin_decode_status_t st_format = vitrin_raw_read_u32(in, in_len, &pos, &format_raw);
    if (st_format != VITRIN_DECODE_OK) { return st_format; }
    if (!vitrin_view_format_is_valid(format_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->format = (vitrin_view_format_t)format_raw;
    vitrin_decode_status_t st_width = vitrin_raw_read_u32(in, in_len, &pos, &out->width);
    if (st_width != VITRIN_DECODE_OK) { return st_width; }
    vitrin_decode_status_t st_height = vitrin_raw_read_u32(in, in_len, &pos, &out->height);
    if (st_height != VITRIN_DECODE_OK) { return st_height; }
    vitrin_decode_status_t st_stride = vitrin_raw_read_u32(in, in_len, &pos, &out->stride);
    if (st_stride != VITRIN_DECODE_OK) { return st_stride; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `damage` (opcode 1) on `vitrin_shim_surface`.
 *
 * add a pending damage rectangle
 */
typedef struct {
    /* rectangle x in buffer coordinates */
    int32_t x;
    /* rectangle y in buffer coordinates */
    int32_t y;
    /* rectangle width */
    int32_t width;
    /* rectangle height */
    int32_t height;
} vitrin_shim_surface_req_damage_t;

#define VITRIN_SHIM_SURFACE_REQ_DAMAGE_OPCODE ((uint8_t)1)
#define VITRIN_SHIM_SURFACE_REQ_DAMAGE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SURFACE_REQ_DAMAGE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_surface_req_damage_encode(const vitrin_shim_surface_req_damage_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SURFACE_REQ_DAMAGE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SURFACE_REQ_DAMAGE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->x);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->y);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->width);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->height);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_surface_req_damage_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_surface_req_damage_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SURFACE_REQ_DAMAGE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SURFACE_REQ_DAMAGE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SURFACE_REQ_DAMAGE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t x_raw;
    vitrin_decode_status_t st_x = vitrin_raw_read_u32(in, in_len, &pos, &x_raw);
    if (st_x != VITRIN_DECODE_OK) { return st_x; }
    out->x = (int32_t)x_raw;
    uint32_t y_raw;
    vitrin_decode_status_t st_y = vitrin_raw_read_u32(in, in_len, &pos, &y_raw);
    if (st_y != VITRIN_DECODE_OK) { return st_y; }
    out->y = (int32_t)y_raw;
    uint32_t width_raw;
    vitrin_decode_status_t st_width = vitrin_raw_read_u32(in, in_len, &pos, &width_raw);
    if (st_width != VITRIN_DECODE_OK) { return st_width; }
    out->width = (int32_t)width_raw;
    uint32_t height_raw;
    vitrin_decode_status_t st_height = vitrin_raw_read_u32(in, in_len, &pos, &height_raw);
    if (st_height != VITRIN_DECODE_OK) { return st_height; }
    out->height = (int32_t)height_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `commit` (opcode 2) on `vitrin_shim_surface`.
 *
 * atomically apply pending state
 */
typedef struct {
    /* no arguments -- a truly empty struct is not portable standard C */
    char reserved;
} vitrin_shim_surface_req_commit_t;

#define VITRIN_SHIM_SURFACE_REQ_COMMIT_OPCODE ((uint8_t)2)
#define VITRIN_SHIM_SURFACE_REQ_COMMIT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SURFACE_REQ_COMMIT_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_surface_req_commit_encode(const vitrin_shim_surface_req_commit_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SURFACE_REQ_COMMIT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SURFACE_REQ_COMMIT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    (void)msg;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_surface_req_commit_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_surface_req_commit_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SURFACE_REQ_COMMIT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SURFACE_REQ_COMMIT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SURFACE_REQ_COMMIT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->reserved = 0;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `frame_done` (opcode 0) on `vitrin_shim_surface`.
 *
 * frame pacing callback
 */
typedef struct {
    /* presentation time in milliseconds, monotonic domain */
    uint32_t time_ms;
} vitrin_shim_surface_evt_frame_done_t;

#define VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_OPCODE ((uint8_t)0)
#define VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_surface_evt_frame_done_encode(const vitrin_shim_surface_evt_frame_done_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->time_ms);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_surface_evt_frame_done_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_surface_evt_frame_done_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_time_ms = vitrin_raw_read_u32(in, in_len, &pos, &out->time_ms);
    if (st_time_ms != VITRIN_DECODE_OK) { return st_time_ms; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `buffer_done` (opcode 1) on `vitrin_shim_surface`.
 *
 * buffer ownership returns (exactly once per attach)
 */
typedef struct {
    /* the cookie given in attach */
    uint32_t buffer_id;
    /* disposition of that attach */
    vitrin_shim_surface_buffer_status_t status;
} vitrin_shim_surface_evt_buffer_done_t;

#define VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_OPCODE ((uint8_t)1)
#define VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_surface_evt_buffer_done_encode(const vitrin_shim_surface_evt_buffer_done_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->buffer_id);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->status);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_surface_evt_buffer_done_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_surface_evt_buffer_done_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_buffer_id = vitrin_raw_read_u32(in, in_len, &pos, &out->buffer_id);
    if (st_buffer_id != VITRIN_DECODE_OK) { return st_buffer_id; }
    uint32_t status_raw;
    vitrin_decode_status_t st_status = vitrin_raw_read_u32(in, in_len, &pos, &status_raw);
    if (st_status != VITRIN_DECODE_OK) { return st_status; }
    if (!vitrin_shim_surface_buffer_status_is_valid(status_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->status = (vitrin_shim_surface_buffer_status_t)status_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_shim_seat messages ==== */

/* Event `motion` (opcode 0) on `vitrin_shim_seat`.
 *
 * pointer moved
 */
typedef struct {
    /* realm-view x */
    vitrin_fixed_t x;
    /* realm-view y */
    vitrin_fixed_t y;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_motion_t;

#define VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE ((uint8_t)0)
#define VITRIN_SHIM_SEAT_EVT_MOTION_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_MOTION_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_motion_encode(const vitrin_shim_seat_evt_motion_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_MOTION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->x);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->y);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_motion_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_motion_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_MOTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_MOTION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_MOTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t x_raw;
    vitrin_decode_status_t st_x = vitrin_raw_read_u32(in, in_len, &pos, &x_raw);
    if (st_x != VITRIN_DECODE_OK) { return st_x; }
    out->x = (vitrin_fixed_t)x_raw;
    uint32_t y_raw;
    vitrin_decode_status_t st_y = vitrin_raw_read_u32(in, in_len, &pos, &y_raw);
    if (st_y != VITRIN_DECODE_OK) { return st_y; }
    out->y = (vitrin_fixed_t)y_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `button` (opcode 1) on `vitrin_shim_seat`.
 *
 * pointer button
 */
typedef struct {
    /* Linux evdev button code */
    uint32_t button;
    /* pressed or released */
    vitrin_actuator_pointer_button_state_t state;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_button_t;

#define VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE ((uint8_t)1)
#define VITRIN_SHIM_SEAT_EVT_BUTTON_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_BUTTON_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_button_encode(const vitrin_shim_seat_evt_button_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_BUTTON_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->button);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_button_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_button_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_BUTTON_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_BUTTON_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_BUTTON_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_button = vitrin_raw_read_u32(in, in_len, &pos, &out->button);
    if (st_button != VITRIN_DECODE_OK) { return st_button; }
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_actuator_pointer_button_state_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_actuator_pointer_button_state_t)state_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `scroll` (opcode 2) on `vitrin_shim_seat`.
 *
 * scroll
 */
typedef struct {
    /* scroll axis */
    vitrin_actuator_pointer_axis_t axis;
    /* scroll amount; one notch = +-120 */
    int32_t value120;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_scroll_t;

#define VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE ((uint8_t)2)
#define VITRIN_SHIM_SEAT_EVT_SCROLL_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_SCROLL_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_scroll_encode(const vitrin_shim_seat_evt_scroll_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_SCROLL_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->axis);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->value120);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_scroll_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_scroll_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_SCROLL_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_SCROLL_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_SCROLL_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t axis_raw;
    vitrin_decode_status_t st_axis = vitrin_raw_read_u32(in, in_len, &pos, &axis_raw);
    if (st_axis != VITRIN_DECODE_OK) { return st_axis; }
    if (!vitrin_actuator_pointer_axis_is_valid(axis_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->axis = (vitrin_actuator_pointer_axis_t)axis_raw;
    uint32_t value120_raw;
    vitrin_decode_status_t st_value120 = vitrin_raw_read_u32(in, in_len, &pos, &value120_raw);
    if (st_value120 != VITRIN_DECODE_OK) { return st_value120; }
    out->value120 = (int32_t)value120_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `key` (opcode 3) on `vitrin_shim_seat`.
 *
 * key press or release (keysym)
 */
typedef struct {
    /* xkbcommon keysym, modifier-resolved */
    uint32_t keysym;
    /* pressed or released */
    vitrin_shim_seat_key_state_t state;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_key_t;

#define VITRIN_SHIM_SEAT_EVT_KEY_OPCODE ((uint8_t)3)
#define VITRIN_SHIM_SEAT_EVT_KEY_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_KEY_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_key_encode(const vitrin_shim_seat_evt_key_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_KEY_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_KEY_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, msg->keysym);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_key_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_key_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_KEY_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_KEY_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_KEY_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_keysym = vitrin_raw_read_u32(in, in_len, &pos, &out->keysym);
    if (st_keysym != VITRIN_DECODE_OK) { return st_keysym; }
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_shim_seat_key_state_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_shim_seat_key_state_t)state_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `text` (opcode 4) on `vitrin_shim_seat`.
 *
 * deliver a Unicode string
 */
typedef struct {
    /* UTF-8 text to deliver (max 4096 bytes) */
    vitrin_string_t text;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_text_t;

#define VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE ((uint8_t)4)
#define VITRIN_SHIM_SEAT_EVT_TEXT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_TEXT_SINCE 1u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_text_encode(const vitrin_shim_seat_evt_text_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->text.len > 4096u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->text.len) + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_TEXT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->text);
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_text_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_text_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_TEXT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_TEXT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_TEXT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_text = vitrin_raw_read_string(in, in_len, &pos, 4096u, &out->text);
    if (st_text != VITRIN_DECODE_OK) { return st_text; }
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `relative_motion` (opcode 5) on `vitrin_shim_seat`.
 *
 * pointer moved, as a delta
 */
typedef struct {
    /* accelerated delta x, realm-view pixels */
    vitrin_fixed_t dx;
    /* accelerated delta y, realm-view pixels */
    vitrin_fixed_t dy;
    /* unaccelerated delta x, realm-view pixels */
    vitrin_fixed_t dx_unaccel;
    /* unaccelerated delta y, realm-view pixels */
    vitrin_fixed_t dy_unaccel;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_relative_motion_t;

#define VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE ((uint8_t)5)
#define VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_relative_motion_encode(const vitrin_shim_seat_evt_relative_motion_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dx);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dy);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dx_unaccel);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dy_unaccel);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_relative_motion_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_relative_motion_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_RELATIVE_MOTION_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t dx_raw;
    vitrin_decode_status_t st_dx = vitrin_raw_read_u32(in, in_len, &pos, &dx_raw);
    if (st_dx != VITRIN_DECODE_OK) { return st_dx; }
    out->dx = (vitrin_fixed_t)dx_raw;
    uint32_t dy_raw;
    vitrin_decode_status_t st_dy = vitrin_raw_read_u32(in, in_len, &pos, &dy_raw);
    if (st_dy != VITRIN_DECODE_OK) { return st_dy; }
    out->dy = (vitrin_fixed_t)dy_raw;
    uint32_t dx_unaccel_raw;
    vitrin_decode_status_t st_dx_unaccel = vitrin_raw_read_u32(in, in_len, &pos, &dx_unaccel_raw);
    if (st_dx_unaccel != VITRIN_DECODE_OK) { return st_dx_unaccel; }
    out->dx_unaccel = (vitrin_fixed_t)dx_unaccel_raw;
    uint32_t dy_unaccel_raw;
    vitrin_decode_status_t st_dy_unaccel = vitrin_raw_read_u32(in, in_len, &pos, &dy_unaccel_raw);
    if (st_dy_unaccel != VITRIN_DECODE_OK) { return st_dy_unaccel; }
    out->dy_unaccel = (vitrin_fixed_t)dy_unaccel_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `gesture_begin` (opcode 6) on `vitrin_shim_seat`.
 *
 * a multi-finger gesture began
 */
typedef struct {
    /* which gesture began */
    vitrin_shim_seat_gesture_kind_t kind;
    /* finger count, fixed for this gesture's life */
    uint32_t fingers;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_gesture_begin_t;

#define VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE ((uint8_t)6)
#define VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_gesture_begin_encode(const vitrin_shim_seat_evt_gesture_begin_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->kind);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, msg->fingers);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_gesture_begin_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_gesture_begin_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_BEGIN_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t kind_raw;
    vitrin_decode_status_t st_kind = vitrin_raw_read_u32(in, in_len, &pos, &kind_raw);
    if (st_kind != VITRIN_DECODE_OK) { return st_kind; }
    if (!vitrin_shim_seat_gesture_kind_is_valid(kind_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->kind = (vitrin_shim_seat_gesture_kind_t)kind_raw;
    vitrin_decode_status_t st_fingers = vitrin_raw_read_u32(in, in_len, &pos, &out->fingers);
    if (st_fingers != VITRIN_DECODE_OK) { return st_fingers; }
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `gesture_swipe_update` (opcode 7) on `vitrin_shim_seat`.
 *
 * an in-flight swipe moved
 */
typedef struct {
    /* delta x since this gesture's previous event, realm-view pixels */
    vitrin_fixed_t dx;
    /* delta y since this gesture's previous event, realm-view pixels */
    vitrin_fixed_t dy;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_gesture_swipe_update_t;

#define VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE ((uint8_t)7)
#define VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_gesture_swipe_update_encode(const vitrin_shim_seat_evt_gesture_swipe_update_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dx);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dy);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_gesture_swipe_update_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_gesture_swipe_update_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_SWIPE_UPDATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t dx_raw;
    vitrin_decode_status_t st_dx = vitrin_raw_read_u32(in, in_len, &pos, &dx_raw);
    if (st_dx != VITRIN_DECODE_OK) { return st_dx; }
    out->dx = (vitrin_fixed_t)dx_raw;
    uint32_t dy_raw;
    vitrin_decode_status_t st_dy = vitrin_raw_read_u32(in, in_len, &pos, &dy_raw);
    if (st_dy != VITRIN_DECODE_OK) { return st_dy; }
    out->dy = (vitrin_fixed_t)dy_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `gesture_pinch_update` (opcode 8) on `vitrin_shim_seat`.
 *
 * an in-flight pinch moved, scaled or rotated
 */
typedef struct {
    /* centre delta x since this gesture's previous event, realm-view pixels */
    vitrin_fixed_t dx;
    /* centre delta y since this gesture's previous event, realm-view pixels */
    vitrin_fixed_t dy;
    /* scale relative to this gesture's begin, 1.0 at the begin */
    vitrin_fixed_t scale;
    /* degrees turned since this gesture's previous event, positive clockwise */
    vitrin_fixed_t rotation;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_gesture_pinch_update_t;

#define VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE ((uint8_t)8)
#define VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_gesture_pinch_update_encode(const vitrin_shim_seat_evt_gesture_pinch_update_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dx);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->dy);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->scale);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->rotation);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_gesture_pinch_update_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_gesture_pinch_update_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_PINCH_UPDATE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t dx_raw;
    vitrin_decode_status_t st_dx = vitrin_raw_read_u32(in, in_len, &pos, &dx_raw);
    if (st_dx != VITRIN_DECODE_OK) { return st_dx; }
    out->dx = (vitrin_fixed_t)dx_raw;
    uint32_t dy_raw;
    vitrin_decode_status_t st_dy = vitrin_raw_read_u32(in, in_len, &pos, &dy_raw);
    if (st_dy != VITRIN_DECODE_OK) { return st_dy; }
    out->dy = (vitrin_fixed_t)dy_raw;
    uint32_t scale_raw;
    vitrin_decode_status_t st_scale = vitrin_raw_read_u32(in, in_len, &pos, &scale_raw);
    if (st_scale != VITRIN_DECODE_OK) { return st_scale; }
    out->scale = (vitrin_fixed_t)scale_raw;
    uint32_t rotation_raw;
    vitrin_decode_status_t st_rotation = vitrin_raw_read_u32(in, in_len, &pos, &rotation_raw);
    if (st_rotation != VITRIN_DECODE_OK) { return st_rotation; }
    out->rotation = (vitrin_fixed_t)rotation_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `gesture_end` (opcode 9) on `vitrin_shim_seat`.
 *
 * a multi-finger gesture ended
 */
typedef struct {
    /* which gesture ended; repeats the in-flight kind */
    vitrin_shim_seat_gesture_kind_t kind;
    /* whether the human completed the gesture */
    vitrin_shim_seat_gesture_state_t state;
    /* who caused this event */
    vitrin_shim_seat_origin_t origin;
} vitrin_shim_seat_evt_gesture_end_t;

#define VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE ((uint8_t)9)
#define VITRIN_SHIM_SEAT_EVT_GESTURE_END_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_SHIM_SEAT_EVT_GESTURE_END_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_shim_seat_evt_gesture_end_encode(const vitrin_shim_seat_evt_gesture_end_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_END_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->kind);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->state);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->origin);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_shim_seat_evt_gesture_end_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_shim_seat_evt_gesture_end_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_SHIM_SEAT_EVT_GESTURE_END_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_SHIM_SEAT_EVT_GESTURE_END_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_SHIM_SEAT_EVT_GESTURE_END_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t kind_raw;
    vitrin_decode_status_t st_kind = vitrin_raw_read_u32(in, in_len, &pos, &kind_raw);
    if (st_kind != VITRIN_DECODE_OK) { return st_kind; }
    if (!vitrin_shim_seat_gesture_kind_is_valid(kind_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->kind = (vitrin_shim_seat_gesture_kind_t)kind_raw;
    uint32_t state_raw;
    vitrin_decode_status_t st_state = vitrin_raw_read_u32(in, in_len, &pos, &state_raw);
    if (st_state != VITRIN_DECODE_OK) { return st_state; }
    if (!vitrin_shim_seat_gesture_state_is_valid(state_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->state = (vitrin_shim_seat_gesture_state_t)state_raw;
    uint32_t origin_raw;
    vitrin_decode_status_t st_origin = vitrin_raw_read_u32(in, in_len, &pos, &origin_raw);
    if (st_origin != VITRIN_DECODE_OK) { return st_origin; }
    if (!vitrin_shim_seat_origin_is_valid(origin_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->origin = (vitrin_shim_seat_origin_t)origin_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_launcher messages ==== */

/* Request `launch` (opcode 0) on `vitrin_launcher`.
 *
 * launch the granted template into a new realm
 */
typedef struct {
    /* no arguments -- a truly empty struct is not portable standard C */
    char reserved;
} vitrin_launcher_req_launch_t;

#define VITRIN_LAUNCHER_REQ_LAUNCH_OPCODE ((uint8_t)0)
#define VITRIN_LAUNCHER_REQ_LAUNCH_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_LAUNCHER_REQ_LAUNCH_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_launcher_req_launch_encode(const vitrin_launcher_req_launch_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_LAUNCHER_REQ_LAUNCH_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_LAUNCHER_REQ_LAUNCH_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    (void)msg;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_launcher_req_launch_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_launcher_req_launch_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_LAUNCHER_REQ_LAUNCH_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_LAUNCHER_REQ_LAUNCH_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_LAUNCHER_REQ_LAUNCH_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->reserved = 0;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `launched` (opcode 0) on `vitrin_launcher`.
 *
 * the realm the launch created
 */
typedef struct {
    /* id of the newly created realm instance, usable as get_realm's name (max 64 bytes) */
    vitrin_string_t realm;
} vitrin_launcher_evt_launched_t;

#define VITRIN_LAUNCHER_EVT_LAUNCHED_OPCODE ((uint8_t)0)
#define VITRIN_LAUNCHER_EVT_LAUNCHED_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_LAUNCHER_EVT_LAUNCHED_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_launcher_evt_launched_encode(const vitrin_launcher_evt_launched_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->realm.len > 64u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->realm.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_LAUNCHER_EVT_LAUNCHED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_LAUNCHER_EVT_LAUNCHED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->realm);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_launcher_evt_launched_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_launcher_evt_launched_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_LAUNCHER_EVT_LAUNCHED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_LAUNCHER_EVT_LAUNCHED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_LAUNCHER_EVT_LAUNCHED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_realm = vitrin_raw_read_string(in, in_len, &pos, 64u, &out->realm);
    if (st_realm != VITRIN_DECODE_OK) { return st_realm; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_layout_focus messages ==== */

/* Request `focus` (opcode 0) on `vitrin_layout_focus`.
 *
 * bind the output to the granted realm and direct input there
 */
typedef struct {
    /* no arguments -- a truly empty struct is not portable standard C */
    char reserved;
} vitrin_layout_focus_req_focus_t;

#define VITRIN_LAYOUT_FOCUS_REQ_FOCUS_OPCODE ((uint8_t)0)
#define VITRIN_LAYOUT_FOCUS_REQ_FOCUS_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_LAYOUT_FOCUS_REQ_FOCUS_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_layout_focus_req_focus_encode(const vitrin_layout_focus_req_focus_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_LAYOUT_FOCUS_REQ_FOCUS_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_LAYOUT_FOCUS_REQ_FOCUS_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    (void)msg;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_layout_focus_req_focus_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_layout_focus_req_focus_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_LAYOUT_FOCUS_REQ_FOCUS_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_LAYOUT_FOCUS_REQ_FOCUS_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_LAYOUT_FOCUS_REQ_FOCUS_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->reserved = 0;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_layout_arrange messages ==== */

/* Request `set_fullscreen` (opcode 0) on `vitrin_layout_arrange`.
 *
 * fill the output, or compose at the app's own size
 */
typedef struct {
    /* fullscreen or windowed */
    vitrin_layout_arrange_mode_t mode;
} vitrin_layout_arrange_req_set_fullscreen_t;

#define VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_OPCODE ((uint8_t)0)
#define VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_layout_arrange_req_set_fullscreen_encode(const vitrin_layout_arrange_req_set_fullscreen_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->mode);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_layout_arrange_req_set_fullscreen_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_layout_arrange_req_set_fullscreen_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_LAYOUT_ARRANGE_REQ_SET_FULLSCREEN_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t mode_raw;
    vitrin_decode_status_t st_mode = vitrin_raw_read_u32(in, in_len, &pos, &mode_raw);
    if (st_mode != VITRIN_DECODE_OK) { return st_mode; }
    if (!vitrin_layout_arrange_mode_is_valid(mode_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->mode = (vitrin_layout_arrange_mode_t)mode_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_powerbox messages ==== */

/* Request `request_file` (opcode 0) on `vitrin_powerbox`.
 *
 * ask the human to designate one file
 */
typedef struct {
    /* the access this ask is for; the human may narrow it, and designated.mode carries what was actually approved */
    vitrin_powerbox_mode_t mode;
} vitrin_powerbox_req_request_file_t;

#define VITRIN_POWERBOX_REQ_REQUEST_FILE_OPCODE ((uint8_t)0)
#define VITRIN_POWERBOX_REQ_REQUEST_FILE_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_POWERBOX_REQ_REQUEST_FILE_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_powerbox_req_request_file_encode(const vitrin_powerbox_req_request_file_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_POWERBOX_REQ_REQUEST_FILE_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_POWERBOX_REQ_REQUEST_FILE_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->mode);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_powerbox_req_request_file_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_powerbox_req_request_file_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_POWERBOX_REQ_REQUEST_FILE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_POWERBOX_REQ_REQUEST_FILE_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_POWERBOX_REQ_REQUEST_FILE_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t mode_raw;
    vitrin_decode_status_t st_mode = vitrin_raw_read_u32(in, in_len, &pos, &mode_raw);
    if (st_mode != VITRIN_DECODE_OK) { return st_mode; }
    if (!vitrin_powerbox_mode_is_valid(mode_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->mode = (vitrin_powerbox_mode_t)mode_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Request `request_dir` (opcode 1) on `vitrin_powerbox`.
 *
 * ask the human to designate one directory subtree
 */
typedef struct {
    /* no arguments -- a truly empty struct is not portable standard C */
    char reserved;
} vitrin_powerbox_req_request_dir_t;

#define VITRIN_POWERBOX_REQ_REQUEST_DIR_OPCODE ((uint8_t)1)
#define VITRIN_POWERBOX_REQ_REQUEST_DIR_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_POWERBOX_REQ_REQUEST_DIR_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_powerbox_req_request_dir_encode(const vitrin_powerbox_req_request_dir_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_POWERBOX_REQ_REQUEST_DIR_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_POWERBOX_REQ_REQUEST_DIR_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    (void)msg;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_powerbox_req_request_dir_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_powerbox_req_request_dir_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_POWERBOX_REQ_REQUEST_DIR_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_POWERBOX_REQ_REQUEST_DIR_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_POWERBOX_REQ_REQUEST_DIR_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->reserved = 0;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `designated` (opcode 0) on `vitrin_powerbox`.
 *
 * the descriptor the human designated
 */
typedef struct {
    /* the designated file or directory descriptor; ownership transfers to the receiver, which MUST close it (not present in the byte buffer; carried out-of-band via SCM_RIGHTS) */
    int fd;
    /* the core's opaque id for this designation, matching the journal record and the realm's designation event */
    uint32_t designation_id;
    /* whether the descriptor is a file or a directory subtree */
    vitrin_powerbox_kind_t kind;
    /* the EFFECTIVE access the human approved, which may be narrower than the ask */
    vitrin_powerbox_mode_t mode;
    /* basename of what the human chose, for display only - never a path (max 255 bytes) */
    vitrin_string_t name;
} vitrin_powerbox_evt_designated_t;

#define VITRIN_POWERBOX_EVT_DESIGNATED_OPCODE ((uint8_t)0)
#define VITRIN_POWERBOX_EVT_DESIGNATED_HAS_FD 1
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_POWERBOX_EVT_DESIGNATED_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_powerbox_evt_designated_encode(const vitrin_powerbox_evt_designated_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->name.len > 255u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4 + 4 + 4 + vitrin_raw_string_wire_len(msg->name.len);
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_POWERBOX_EVT_DESIGNATED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_POWERBOX_EVT_DESIGNATED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    /* fd: fd argument, never written to the byte buffer */
    vitrin_raw_write_u32(out + pos, msg->designation_id);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->kind);
    pos += 4u;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->mode);
    pos += 4u;
    pos += vitrin_raw_write_string(out + pos, msg->name);
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_powerbox_evt_designated_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_powerbox_evt_designated_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_POWERBOX_EVT_DESIGNATED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_POWERBOX_EVT_DESIGNATED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_POWERBOX_EVT_DESIGNATED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->fd = fd;
    vitrin_decode_status_t st_designation_id = vitrin_raw_read_u32(in, in_len, &pos, &out->designation_id);
    if (st_designation_id != VITRIN_DECODE_OK) { return st_designation_id; }
    uint32_t kind_raw;
    vitrin_decode_status_t st_kind = vitrin_raw_read_u32(in, in_len, &pos, &kind_raw);
    if (st_kind != VITRIN_DECODE_OK) { return st_kind; }
    if (!vitrin_powerbox_kind_is_valid(kind_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->kind = (vitrin_powerbox_kind_t)kind_raw;
    uint32_t mode_raw;
    vitrin_decode_status_t st_mode = vitrin_raw_read_u32(in, in_len, &pos, &mode_raw);
    if (st_mode != VITRIN_DECODE_OK) { return st_mode; }
    if (!vitrin_powerbox_mode_is_valid(mode_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->mode = (vitrin_powerbox_mode_t)mode_raw;
    vitrin_decode_status_t st_name = vitrin_raw_read_string(in, in_len, &pos, 255u, &out->name);
    if (st_name != VITRIN_DECODE_OK) { return st_name; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `refused` (opcode 1) on `vitrin_powerbox`.
 *
 * the picker was raised and produced no descriptor
 */
typedef struct {
    /* why the ask produced no descriptor */
    vitrin_powerbox_refusal_t code;
} vitrin_powerbox_evt_refused_t;

#define VITRIN_POWERBOX_EVT_REFUSED_OPCODE ((uint8_t)1)
#define VITRIN_POWERBOX_EVT_REFUSED_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_POWERBOX_EVT_REFUSED_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_powerbox_evt_refused_encode(const vitrin_powerbox_evt_refused_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_POWERBOX_EVT_REFUSED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_POWERBOX_EVT_REFUSED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->code);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_powerbox_evt_refused_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_powerbox_evt_refused_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_POWERBOX_EVT_REFUSED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_POWERBOX_EVT_REFUSED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_POWERBOX_EVT_REFUSED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t code_raw;
    vitrin_decode_status_t st_code = vitrin_raw_read_u32(in, in_len, &pos, &code_raw);
    if (st_code != VITRIN_DECODE_OK) { return st_code; }
    if (!vitrin_powerbox_refusal_is_valid(code_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->code = (vitrin_powerbox_refusal_t)code_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* ==== vitrin_egress messages ==== */

/* Request `request_connect` (opcode 0) on `vitrin_egress`.
 *
 * open one outbound connection to a granted endpoint
 */
typedef struct {
    /* the host half of the grant's net: selector, byte-exact, IPv6 literals WITHOUT brackets (max 253 bytes) */
    vitrin_string_t host;
    /* the port half of the grant's net: selector; outside 1-65535 is fatal invalid_argument */
    uint32_t port;
} vitrin_egress_req_request_connect_t;

#define VITRIN_EGRESS_REQ_REQUEST_CONNECT_OPCODE ((uint8_t)0)
#define VITRIN_EGRESS_REQ_REQUEST_CONNECT_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_EGRESS_REQ_REQUEST_CONNECT_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_egress_req_request_connect_encode(const vitrin_egress_req_request_connect_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->host.len > 253u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->host.len) + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_EGRESS_REQ_REQUEST_CONNECT_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_EGRESS_REQ_REQUEST_CONNECT_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    pos += vitrin_raw_write_string(out + pos, msg->host);
    vitrin_raw_write_u32(out + pos, msg->port);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_egress_req_request_connect_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_egress_req_request_connect_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_EGRESS_REQ_REQUEST_CONNECT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_EGRESS_REQ_REQUEST_CONNECT_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_EGRESS_REQ_REQUEST_CONNECT_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_decode_status_t st_host = vitrin_raw_read_string(in, in_len, &pos, 253u, &out->host);
    if (st_host != VITRIN_DECODE_OK) { return st_host; }
    vitrin_decode_status_t st_port = vitrin_raw_read_u32(in, in_len, &pos, &out->port);
    if (st_port != VITRIN_DECODE_OK) { return st_port; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `connected` (opcode 0) on `vitrin_egress`.
 *
 * the connected socket for one admitted request_connect
 */
typedef struct {
    /* the connected stream socket, owned by the receiving principal (not present in the byte buffer; carried out-of-band via SCM_RIGHTS) */
    int fd;
    /* echo of the host this socket is connected to, byte-identical to the request's (max 253 bytes) */
    vitrin_string_t host;
    /* echo of the port this socket is connected to */
    uint32_t port;
} vitrin_egress_evt_connected_t;

#define VITRIN_EGRESS_EVT_CONNECTED_OPCODE ((uint8_t)0)
#define VITRIN_EGRESS_EVT_CONNECTED_HAS_FD 1
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_EGRESS_EVT_CONNECTED_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_egress_evt_connected_encode(const vitrin_egress_evt_connected_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    if (msg->host.len > 253u) {
        return VITRIN_ENCODE_ERR_STRING_TOO_LONG;
    }
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + vitrin_raw_string_wire_len(msg->host.len) + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_EGRESS_EVT_CONNECTED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_EGRESS_EVT_CONNECTED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    /* fd: fd argument, never written to the byte buffer */
    pos += vitrin_raw_write_string(out + pos, msg->host);
    vitrin_raw_write_u32(out + pos, msg->port);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_egress_evt_connected_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_egress_evt_connected_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_EGRESS_EVT_CONNECTED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_EGRESS_EVT_CONNECTED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_EGRESS_EVT_CONNECTED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    out->fd = fd;
    vitrin_decode_status_t st_host = vitrin_raw_read_string(in, in_len, &pos, 253u, &out->host);
    if (st_host != VITRIN_DECODE_OK) { return st_host; }
    vitrin_decode_status_t st_port = vitrin_raw_read_u32(in, in_len, &pos, &out->port);
    if (st_port != VITRIN_DECODE_OK) { return st_port; }
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

/* Event `connect_failed` (opcode 1) on `vitrin_egress`.
 *
 * an admitted request_connect that the far end did not answer
 */
typedef struct {
    /* what the far end did instead of answering */
    vitrin_egress_failure_t reason;
} vitrin_egress_evt_connect_failed_t;

#define VITRIN_EGRESS_EVT_CONNECT_FAILED_OPCODE ((uint8_t)1)
#define VITRIN_EGRESS_EVT_CONNECT_FAILED_HAS_FD 0
/* First protocol version at which this message is defined (`message/@since`); */
/* this opcode is not defined on a connection whose negotiated version is    */
/* lower, where using it is fatal `invalid_opcode`.                          */
#define VITRIN_EGRESS_EVT_CONNECT_FAILED_SINCE 2u

/* Encodes into a complete frame (header + argument payload). Returns the
   number of bytes written (fits in an int32_t: the wire format's own u16
   size field caps a frame at 65535 bytes), VITRIN_ENCODE_ERR_OVERFLOW if
   out_capacity is too small or the frame would exceed 65535 bytes, or
   VITRIN_ENCODE_ERR_STRING_TOO_LONG if a string argument exceeds its own
   documented `(max N bytes)` bound. Nothing is written to `out` on either
   error. Any fd argument is never written here -- send it out-of-band via
   SCM_RIGHTS alongside these bytes. */
static inline int32_t vitrin_egress_evt_connect_failed_encode(const vitrin_egress_evt_connect_failed_t *msg, uint32_t object_id, uint8_t *out, size_t out_capacity) {
    uint64_t size = (uint64_t)VITRIN_HEADER_LEN + 4;
    if (size > 0xffffu || size > (uint64_t)out_capacity) {
        return VITRIN_ENCODE_ERR_OVERFLOW;
    }
    vitrin_frame_header_t hdr;
    hdr.object_id = object_id;
    hdr.size = (uint16_t)size;
    hdr.opcode = VITRIN_EGRESS_EVT_CONNECT_FAILED_OPCODE;
    hdr.fd_count = (uint8_t)VITRIN_EGRESS_EVT_CONNECT_FAILED_HAS_FD;
    vitrin_frame_header_encode(&hdr, out);
    size_t pos = VITRIN_HEADER_LEN;
    vitrin_raw_write_u32(out + pos, (uint32_t)msg->reason);
    pos += 4u;
    return (int32_t)size;
}

/* Decodes one complete frame's bytes (in/in_len -- exactly one frame, e.g.
   already delimited by a transport layer using the header's own size field,
   out of scope here) plus, iff HAS_FD below, the fd received alongside it
   out-of-band (fd = -1 if none). On success writes the frame's object_id to
   *out_object_id and the decoded message to *out and returns
   VITRIN_DECODE_OK; otherwise returns a negative vitrin_decode_status_t and
   leaves *out_object_id and *out unspecified.

   docs/protocol/00-conventions.md 2.4/5.2 define fd_violation as two
   independent disjuncts, both checked here: the header's own fd_count byte
   disagreeing with this message's signature, and the out-of-band fd
   parameter disagreeing with it. A hostile or buggy peer can make either
   one lie without the other, so neither check substitutes for the other.

   The header's opcode and size fields are validated in the same
   defense-in-depth spirit: the dispatcher already selected this message by
   opcode and delimited the frame by size, but a dispatcher bug (or a
   header whose size field lies about the delivered byte count, fatal
   `oversized` per conventions 2.1) must surface as an error here, not as a
   silently mis-decoded message. */
static inline vitrin_decode_status_t vitrin_egress_evt_connect_failed_decode(
    const uint8_t *in, size_t in_len, int fd,
    uint32_t *out_object_id, vitrin_egress_evt_connect_failed_t *out) {
    int fd_present = (fd >= 0) ? 1 : 0;
    if (fd_present != VITRIN_EGRESS_EVT_CONNECT_FAILED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    vitrin_frame_header_t hdr;
    vitrin_decode_status_t hdr_st = vitrin_frame_header_decode(in, in_len, &hdr);
    if (hdr_st != VITRIN_DECODE_OK) {
        return hdr_st;
    }
    if (hdr.opcode != VITRIN_EGRESS_EVT_CONNECT_FAILED_OPCODE) {
        return VITRIN_DECODE_ERR_OPCODE_MISMATCH;
    }
    if ((size_t)hdr.size != in_len) {
        return VITRIN_DECODE_ERR_SIZE_MISMATCH;
    }
    if (hdr.fd_count != (uint8_t)VITRIN_EGRESS_EVT_CONNECT_FAILED_HAS_FD) {
        return VITRIN_DECODE_ERR_FD_MISMATCH;
    }
    size_t pos = VITRIN_HEADER_LEN;
    uint32_t reason_raw;
    vitrin_decode_status_t st_reason = vitrin_raw_read_u32(in, in_len, &pos, &reason_raw);
    if (st_reason != VITRIN_DECODE_OK) { return st_reason; }
    if (!vitrin_egress_failure_is_valid(reason_raw)) { return VITRIN_DECODE_ERR_INVALID_ENUM; }
    out->reason = (vitrin_egress_failure_t)reason_raw;
    if (pos != in_len) {
        return VITRIN_DECODE_ERR_TRAILING_BYTES;
    }
    *out_object_id = hdr.object_id;
    return VITRIN_DECODE_OK;
}

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* VITRIN_PROTOCOL_H */
