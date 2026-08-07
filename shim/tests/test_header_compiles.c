/* test_header_compiles.c -- proves shim/include/vitrin-protocol.h compiles
 * standalone under -Wall -Werror (P1.1.2 acceptance criterion 4).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * HAND-WRITTEN, not generated: unlike vitrin-protocol.h itself, this file
 * carries no "DO NOT EDIT" banner and is free to be edited directly.
 *
 * This is a compile-time proof, not a runtime test: main() below is never
 * meant to be executed (it does nothing meaningful, deliberately touches
 * uninitialized-looking dummy buffers, and would not run inside any real
 * test harness). Its only job is to force every inline function the header
 * declares to be fully type-checked by the compiler:
 *
 *   - every message's encode and decode function pointer is assigned into a
 *     precisely-typed local (via the CHECK_MESSAGE macro below), which only
 *     compiles if the function's real signature matches exactly what this
 *     file expects;
 *   - every enum's validity predicate likewise (CHECK_IS_VALID);
 *   - the frame-header and raw little-endian helpers are checked directly;
 *   - a handful of representative messages (a plain zero-argument request,
 *     a string-and-scalar-heavy request, a bitfield-and-enum-heavy event,
 *     and both of the two fd-bearing messages) are actually *called* with
 *     dummy buffers, end to end (encode then decode), to prove the API is
 *     genuinely callable from an external C translation unit, not merely
 *     syntactically present.
 *
 * "Every message" is asserted, not trusted: VITRIN_EVERY_MESSAGE below is
 * counted at compile time and _Static_assert'd against the generated
 * VITRIN_MESSAGE_COUNT, so a message appended to protocol/vitrin-v0.xml
 * fails this build until it is listed here. Without that gate the list is
 * a hand-maintained copy that silently falls behind the IDL -- which is
 * exactly what happened when version 2 added get_launcher, launch and
 * launched: the file kept compiling while type-checking 29 of 32 messages.
 * (It fired as designed when version 2 later added the two layout mints and
 * their two facet requests -- 32 to 36.)
 * It mirrors crates/vitrin-protocol/tests/roundtrip.rs's
 * `every_idl_message_is_in_the_roundtrip_table`, which gates the Rust
 * round-trip table on the same generated constant.
 *
 * Build/check exactly as CI does:
 *
 *   cc -std=c11 -Wall -Werror -I shim/include -c shim/tests/test_header_compiles.c -o /dev/null
 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "vitrin-protocol.h"

/* Accumulates trivial, compiler-visible "uses" of every checked function
 * pointer and call result, so nothing above is dead code a compiler could
 * complain about (or a reader could dismiss as untouched) and so main()
 * has a well-defined, if meaningless, return value. */
static int sink = 0;

/* Every message the IDL defines, in header (document) order: interfaces in
 * document order, and within an interface requests then events, each in
 * document order. One entry per message, naming its marshal-function *base*
 * -- the generator spells the struct <base>_t and the two functions
 * <base>_encode / <base>_decode, so one token drives the type-check, and
 * the same list is what the count below is taken from. */
#define VITRIN_EVERY_MESSAGE(X)                                              \
    X(vitrin_handshake_req_hello)                                            \
    X(vitrin_handshake_req_sync)                                             \
    X(vitrin_handshake_evt_error)                                            \
    X(vitrin_handshake_evt_done)                                             \
    X(vitrin_principal_req_get_realm)                                        \
    X(vitrin_principal_evt_bound)                                            \
    X(vitrin_realm_req_request_grant)                                        \
    X(vitrin_grant_req_get_launcher)                                         \
    X(vitrin_grant_req_get_layout_focus)                                     \
    X(vitrin_grant_req_get_layout_arrange)                                   \
    X(vitrin_grant_evt_resolved)                                             \
    X(vitrin_grant_evt_refused)                                              \
    X(vitrin_consent_evt_state)                                              \
    X(vitrin_view_req_capture_frame)                                         \
    X(vitrin_view_evt_frame_ready)                                           \
    X(vitrin_actuator_pointer_req_move)                                      \
    X(vitrin_actuator_pointer_req_button)                                    \
    X(vitrin_actuator_pointer_req_scroll)                                    \
    X(vitrin_actuator_text_req_type)                                         \
    X(vitrin_shim_session_req_create_surface)                                \
    X(vitrin_shim_session_req_get_seat)                                      \
    X(vitrin_shim_session_evt_configure)                                     \
    X(vitrin_shim_surface_req_attach)                                        \
    X(vitrin_shim_surface_req_damage)                                        \
    X(vitrin_shim_surface_req_commit)                                        \
    X(vitrin_shim_surface_evt_frame_done)                                    \
    X(vitrin_shim_surface_evt_buffer_done)                                   \
    X(vitrin_shim_seat_evt_motion)                                           \
    X(vitrin_shim_seat_evt_button)                                           \
    X(vitrin_shim_seat_evt_scroll)                                           \
    X(vitrin_shim_seat_evt_key)                                              \
    X(vitrin_shim_seat_evt_text)                                             \
    X(vitrin_launcher_req_launch)                                            \
    X(vitrin_launcher_evt_launched)                                          \
    X(vitrin_layout_focus_req_focus)                                         \
    X(vitrin_layout_arrange_req_set_fullscreen)

/* The exhaustiveness gate. VITRIN_MESSAGE_COUNT is generated from the IDL,
 * so an appended message makes this a compile error here rather than a
 * silent hole in the coverage the file's banner comment claims. */
#define VITRIN_COUNT_ONE(base) +1
enum { vitrin_messages_listed = 0 VITRIN_EVERY_MESSAGE(VITRIN_COUNT_ONE) };
_Static_assert((int)vitrin_messages_listed == (int)VITRIN_MESSAGE_COUNT,
               "protocol/vitrin-v0.xml defines a message missing from (or "
               "extra in) VITRIN_EVERY_MESSAGE, so this file no longer "
               "type-checks every generated marshal function");

/* One macro per function *shape* (every *_encode shares the same parameter
 * list modulo the struct type; likewise every *_decode and every
 * *_is_valid), so each message is one list entry rather than two full
 * function-pointer declarations. The trailing semicolon lives inside
 * CHECK_MESSAGE because VITRIN_EVERY_MESSAGE expands to a bare sequence of
 * invocations with no separators. */
#define CHECK_MESSAGE(base)                                                  \
    do {                                                                     \
        int32_t (*enc)(const base##_t *, uint32_t, uint8_t *, size_t) =      \
            base##_encode;                                                   \
        vitrin_decode_status_t (*dec)(const uint8_t *, size_t, int,          \
                                       uint32_t *, base##_t *) =             \
            base##_decode;                                                   \
        sink += (enc != NULL) ? 1 : 0;                                       \
        sink += (dec != NULL) ? 1 : 0;                                       \
    } while (0);

#define CHECK_IS_VALID(fn_name)                                              \
    do {                                                                     \
        bool (*p)(uint32_t) = fn_name;                                       \
        sink += (p != NULL) ? 1 : 0;                                         \
    } while (0)

/* Takes the address of every marshal function the header declares, forcing
 * each one's exact signature to be type-checked against this file's own
 * expectation of that signature. */
static void check_every_symbol_type_checks(void) {
    /* ---- generic support ---- */
    {
        void (*hdr_enc)(const vitrin_frame_header_t *, uint8_t *) =
            vitrin_frame_header_encode;
        vitrin_decode_status_t (*hdr_dec)(const uint8_t *, size_t,
                                           vitrin_frame_header_t *) =
            vitrin_frame_header_decode;
        void (*raw_w32)(uint8_t *, uint32_t) = vitrin_raw_write_u32;
        vitrin_decode_status_t (*raw_r32)(const uint8_t *, size_t, size_t *,
                                           uint32_t *) = vitrin_raw_read_u32;
        size_t (*pad_len)(uint32_t) = vitrin_raw_pad_len;
        size_t (*str_wire_len)(uint32_t) = vitrin_raw_string_wire_len;
        size_t (*raw_wstr)(uint8_t *, vitrin_string_t) =
            vitrin_raw_write_string;
        vitrin_decode_status_t (*raw_rstr)(const uint8_t *, size_t, size_t *,
                                            uint32_t, vitrin_string_t *) =
            vitrin_raw_read_string;
        double (*fixed_to_dbl)(vitrin_fixed_t) = vitrin_fixed_to_double;
        vitrin_fixed_t (*fixed_from_dbl)(double) = vitrin_fixed_from_double;
        const char *(*status_str)(vitrin_decode_status_t) =
            vitrin_decode_status_string;

        sink += (hdr_enc != NULL) ? 1 : 0;
        sink += (hdr_dec != NULL) ? 1 : 0;
        sink += (raw_w32 != NULL) ? 1 : 0;
        sink += (raw_r32 != NULL) ? 1 : 0;
        sink += (pad_len != NULL) ? 1 : 0;
        sink += (str_wire_len != NULL) ? 1 : 0;
        sink += (raw_wstr != NULL) ? 1 : 0;
        sink += (raw_rstr != NULL) ? 1 : 0;
        sink += (fixed_to_dbl != NULL) ? 1 : 0;
        sink += (fixed_from_dbl != NULL) ? 1 : 0;
        sink += (status_str != NULL) ? 1 : 0;
    }

    /* ---- enum/bitfield validity predicates, one per enum in the IDL ---- */
    CHECK_IS_VALID(vitrin_handshake_error_is_valid);
    CHECK_IS_VALID(vitrin_grant_verb_is_valid);
    CHECK_IS_VALID(vitrin_grant_persistence_is_valid);
    CHECK_IS_VALID(vitrin_grant_outcome_is_valid);
    CHECK_IS_VALID(vitrin_grant_refusal_is_valid);
    CHECK_IS_VALID(vitrin_consent_consent_state_is_valid);
    CHECK_IS_VALID(vitrin_view_format_is_valid);
    CHECK_IS_VALID(vitrin_view_frame_flags_is_valid);
    CHECK_IS_VALID(vitrin_actuator_pointer_button_state_is_valid);
    CHECK_IS_VALID(vitrin_actuator_pointer_axis_is_valid);
    CHECK_IS_VALID(vitrin_shim_surface_kind_is_valid);
    CHECK_IS_VALID(vitrin_shim_surface_buffer_status_is_valid);
    CHECK_IS_VALID(vitrin_shim_seat_key_state_is_valid);
    CHECK_IS_VALID(vitrin_shim_seat_origin_is_valid);

    /* ---- every message's encode + decode, in header (document) order ---- */
    VITRIN_EVERY_MESSAGE(CHECK_MESSAGE)
}

/* Encodes then decodes a zero-argument request (vitrin_view.capture_frame),
 * the simplest possible message shape. */
static void call_zero_arg_message(void) {
    vitrin_view_req_capture_frame_t msg;
    uint8_t buf[64];
    int32_t written;
    uint32_t object_id_out;
    vitrin_view_req_capture_frame_t decoded;
    vitrin_decode_status_t st;

    memset(&msg, 0, sizeof(msg));
    memset(buf, 0, sizeof(buf));

    written = vitrin_view_req_capture_frame_encode(&msg, 42, buf, sizeof(buf));
    sink += (written == (int32_t)VITRIN_HEADER_LEN) ? 1 : 0;

    st = vitrin_view_req_capture_frame_decode(buf, (size_t)written, -1,
                                               &object_id_out, &decoded);
    sink += (st == VITRIN_DECODE_OK) ? 1 : 0;
    sink += (object_id_out == 42u) ? 1 : 0;
}

/* Encodes then decodes a message with several scalar arguments plus three
 * string arguments of different bounds (vitrin_handshake.hello), the
 * heaviest string user in the IDL. */
static void call_string_and_scalar_message(void) {
    static const uint8_t identity_bytes[] = "vitrin://local/agent/demo";
    static const uint8_t cred_type_bytes[] = "static-token";
    static const uint8_t cred_bytes[] = "opaque-credential-bytes";
    vitrin_handshake_req_hello_t msg;
    uint8_t buf[256];
    int32_t written;
    uint32_t object_id_out;
    vitrin_handshake_req_hello_t decoded;
    vitrin_decode_status_t st;

    msg.version = VITRIN_PROTOCOL_VERSION;
    msg.principal = 2;
    msg.identity.len = (uint32_t)(sizeof(identity_bytes) - 1);
    msg.identity.data = identity_bytes;
    msg.credential_type.len = (uint32_t)(sizeof(cred_type_bytes) - 1);
    msg.credential_type.data = cred_type_bytes;
    msg.credential.len = (uint32_t)(sizeof(cred_bytes) - 1);
    msg.credential.data = cred_bytes;

    written = vitrin_handshake_req_hello_encode(&msg, 1, buf, sizeof(buf));
    sink += (written > 0) ? 1 : 0;

    st = vitrin_handshake_req_hello_decode(buf, (size_t)written, -1,
                                            &object_id_out, &decoded);
    sink += (st == VITRIN_DECODE_OK) ? 1 : 0;
    sink += (decoded.identity.len == msg.identity.len) ? 1 : 0;
    sink += (decoded.principal == msg.principal) ? 1 : 0;

    /* Also exercises encode's overflow path: a capacity too small to hold
     * even the frame header must be rejected, not overrun the buffer. */
    written = vitrin_handshake_req_hello_encode(&msg, 1, buf, 2);
    sink += (written == VITRIN_ENCODE_ERR_OVERFLOW) ? 1 : 0;
}

/* Encodes then decodes a message combining a plain enum, a bitfield, and a
 * cross-interface enum reference in one place (vitrin_grant.resolved: outcome
 * is vitrin_grant's own enum, verbs is vitrin_grant's own bitfield,
 * persistence is vitrin_grant's own enum). */
static void call_enum_and_bitfield_message(void) {
    vitrin_grant_evt_resolved_t msg;
    uint8_t buf[64];
    int32_t written;
    uint32_t object_id_out;
    vitrin_grant_evt_resolved_t decoded;
    vitrin_decode_status_t st;

    msg.outcome = VITRIN_GRANT_OUTCOME_GRANTED;
    msg.verbs = VITRIN_GRANT_VERB_OBSERVE | VITRIN_GRANT_VERB_ACTUATE_TEXT;
    msg.persistence = VITRIN_GRANT_PERSISTENCE_WHILE_RUNNING;
    msg.expiry_ms = 0;

    sink += vitrin_grant_verb_is_valid(msg.verbs) ? 1 : 0;

    written = vitrin_grant_evt_resolved_encode(&msg, 7, buf, sizeof(buf));
    sink += (written > 0) ? 1 : 0;

    st = vitrin_grant_evt_resolved_decode(buf, (size_t)written, -1,
                                           &object_id_out, &decoded);
    sink += (st == VITRIN_DECODE_OK) ? 1 : 0;
    sink += (decoded.verbs == msg.verbs) ? 1 : 0;
    sink += (decoded.outcome == msg.outcome) ? 1 : 0;
}

/* Encodes then decodes vitrin_view.frame_ready: one of the two fd-bearing
 * messages, exercising the plain `int fd` field, a cross-interface plain
 * enum (vitrin_view_format_t, defined on this same interface here but
 * referenced from vitrin_shim_surface.attach too -- see
 * call_other_fd_bearing_message below), and a bitfield in the same struct. */
static void call_fd_bearing_message(void) {
    vitrin_view_evt_frame_ready_t msg;
    uint8_t buf[64];
    int32_t written;
    uint32_t object_id_out;
    vitrin_view_evt_frame_ready_t decoded;
    vitrin_decode_status_t st;
    int dummy_fd = 3; /* never actually a valid descriptor; main() never runs */

    msg.fd = dummy_fd;
    msg.format = VITRIN_VIEW_FORMAT_XRGB8888;
    msg.width = 1920;
    msg.height = 1080;
    msg.stride = msg.width * 4u;
    msg.flags = 0;

    written = vitrin_view_evt_frame_ready_encode(&msg, 5, buf, sizeof(buf));
    sink += (written > 0) ? 1 : 0;

    /* HAS_FD is true for this message: fd < 0 (absent) must be rejected. */
    st = vitrin_view_evt_frame_ready_decode(buf, (size_t)written, -1,
                                             &object_id_out, &decoded);
    sink += (st == VITRIN_DECODE_ERR_FD_MISMATCH) ? 1 : 0;

    st = vitrin_view_evt_frame_ready_decode(buf, (size_t)written, dummy_fd,
                                             &object_id_out, &decoded);
    sink += (st == VITRIN_DECODE_OK) ? 1 : 0;
    sink += (decoded.fd == dummy_fd) ? 1 : 0;
    sink += (decoded.format == msg.format) ? 1 : 0;
}

/* Encodes then decodes vitrin_shim_surface.attach: the other fd-bearing
 * message, and the one that references vitrin_view.format from a *different*
 * interface (vitrin_shim_surface is defined after vitrin_view in
 * protocol/vitrin-v0.xml, so this is a backward reference; request_grant's
 * forward reference to vitrin_grant's enums is exercised structurally by
 * CHECK_ENCODE/CHECK_DECODE(vitrin_realm_req_request_grant_t, ...) above,
 * since that struct could not even be declared if the two-phase ordering in
 * c_gen.rs were wrong). */
static void call_other_fd_bearing_message(void) {
    vitrin_shim_surface_req_attach_t msg;
    uint8_t buf[64];
    int32_t written;
    uint32_t object_id_out;
    vitrin_shim_surface_req_attach_t decoded;
    vitrin_decode_status_t st;
    int dummy_fd = 4;

    msg.buffer_id = 1;
    msg.fd = dummy_fd;
    msg.kind = VITRIN_SHIM_SURFACE_KIND_SHM;
    msg.format = VITRIN_VIEW_FORMAT_ARGB8888;
    msg.width = 640;
    msg.height = 480;
    msg.stride = msg.width * 4u;

    written = vitrin_shim_surface_req_attach_encode(&msg, 9, buf, sizeof(buf));
    sink += (written > 0) ? 1 : 0;

    st = vitrin_shim_surface_req_attach_decode(buf, (size_t)written, dummy_fd,
                                                &object_id_out, &decoded);
    sink += (st == VITRIN_DECODE_OK) ? 1 : 0;
    sink += (decoded.format == msg.format) ? 1 : 0;
    sink += (decoded.kind == msg.kind) ? 1 : 0;
}

int main(void) {
    check_every_symbol_type_checks();
    call_zero_arg_message();
    call_string_and_scalar_message();
    call_enum_and_bitfield_message();
    call_fd_bearing_message();
    call_other_fd_bearing_message();
    return sink >= 0 ? 0 : 1;
}
