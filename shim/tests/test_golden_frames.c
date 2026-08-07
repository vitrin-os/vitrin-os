/* Golden-bytes (known-answer) test for the generated C header: encodes the
 * SAME six frames as crates/vitrin-protocol/tests/golden.rs and compares
 * them byte-for-byte against the same hardcoded expectations, then decodes
 * them back. Because both sides must match these bytes -- not each other --
 * the Rust and C encoders cannot silently drift apart, and a symmetric
 * encode/decode bug (which the round-trip property cannot see) fails here.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * Also exercises the encode-side guards the compile-only test cannot:
 * per-string bound rejection, and the decode-side size/opcode/padding
 * checks.
 *
 * Built and RUN by CI:
 *   cc -std=c11 -Wall -Werror -I shim/include shim/tests/test_golden_frames.c -o <tmp> && <tmp>
 */

#include <stdio.h>
#include <string.h>

#include "vitrin-protocol.h"

static int failures = 0;

static void expect_bytes(const char *what, const uint8_t *got, int32_t got_len,
                         const uint8_t *want, size_t want_len) {
    if (got_len < 0 || (size_t)got_len != want_len ||
        memcmp(got, want, want_len) != 0) {
        fprintf(stderr, "FAIL: %s: encoded bytes differ from golden frame\n", what);
        failures++;
    }
}

static void expect(const char *what, int ok) {
    if (!ok) {
        fprintf(stderr, "FAIL: %s\n", what);
        failures++;
    }
}

int main(void) {
    uint8_t buf[128];

    /* -- sync: header + uint ------------------------------------------- */
    {
        static const uint8_t want[] = {1, 0, 0, 0, 12, 0, 1, 0, 42, 0, 0, 0};
        vitrin_handshake_req_sync_t msg = {42};
        int32_t n = vitrin_handshake_req_sync_encode(&msg, 1, buf, sizeof buf);
        expect_bytes("sync", buf, n, want, sizeof want);

        uint32_t object_id = 0;
        vitrin_handshake_req_sync_t back;
        expect("sync decode ok",
               vitrin_handshake_req_sync_decode(want, sizeof want, -1, &object_id,
                                                &back) == VITRIN_DECODE_OK);
        expect("sync decode fields", object_id == 1u && back.cookie == 42u);
    }

    /* -- attention: a bare header, no payload --------------------------- */
    {
        /* vitrin_principal.attention (WS-E.1.7) carries no arguments, forever.
         * The vector pins the empty payload AND the opcode: it is event 1,
         * appended after bound, and a reorder would decode as a truncated
         * bound. */
        static const uint8_t want[] = {2, 0, 0, 0, 8, 0, 1, 0};
        vitrin_principal_evt_attention_t msg = {0};
        int32_t n = vitrin_principal_evt_attention_encode(&msg, 2, buf, sizeof buf);
        expect_bytes("attention", buf, n, want, sizeof want);

        uint32_t object_id = 0;
        vitrin_principal_evt_attention_t back;
        expect("attention decode ok",
               vitrin_principal_evt_attention_decode(want, sizeof want, -1, &object_id,
                                                     &back) == VITRIN_DECODE_OK);
        expect("attention decode fields", object_id == 2u);
    }

    /* -- get_realm: new_id + string with padding ------------------------ */
    {
        static const uint8_t want[] = {7, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0,
                                       3, 0, 0, 0, 'a', 'b', 'c', 0};
        vitrin_principal_req_get_realm_t msg;
        msg.realm = 2;
        msg.name.len = 3;
        msg.name.data = (const uint8_t *)"abc";
        int32_t n = vitrin_principal_req_get_realm_encode(&msg, 7, buf, sizeof buf);
        expect_bytes("get_realm", buf, n, want, sizeof want);

        uint32_t object_id = 0;
        vitrin_principal_req_get_realm_t back;
        expect("get_realm decode ok",
               vitrin_principal_req_get_realm_decode(want, sizeof want, -1, &object_id,
                                                     &back) == VITRIN_DECODE_OK);
        expect("get_realm decode fields",
               object_id == 7u && back.realm == 2u && back.name.len == 3u &&
                   memcmp(back.name.data, "abc", 3) == 0);
    }

    /* -- pointer move: negative int (signedness + endianness) ----------- */
    {
        static const uint8_t want[] = {3, 0, 0, 0, 16, 0, 0, 0,
                                       0xff, 0xff, 0xff, 0xff, 2, 0, 0, 0};
        vitrin_actuator_pointer_req_move_t msg = {-1, 2};
        int32_t n = vitrin_actuator_pointer_req_move_encode(&msg, 3, buf, sizeof buf);
        expect_bytes("pointer_move", buf, n, want, sizeof want);
    }

    /* -- seat motion: 24.8 fixed-point ---------------------------------- */
    {
        static const uint8_t want[] = {9, 0, 0, 0, 20, 0, 0, 0, 0x80, 1, 0, 0,
                                       0, 0xff, 0xff, 0xff, 0, 0, 0, 0};
        vitrin_shim_seat_evt_motion_t msg;
        msg.x = vitrin_fixed_from_double(1.5); /* 384 */
        msg.y = (vitrin_fixed_t)-256;
        msg.origin = VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL;
        int32_t n = vitrin_shim_seat_evt_motion_encode(&msg, 9, buf, sizeof buf);
        expect_bytes("seat_motion", buf, n, want, sizeof want);
    }

    /* -- frame_ready: fd_count header byte + fourcc enum ---------------- */
    {
        static const uint8_t want[] = {5, 0, 0, 0, 28, 0, 0, 1,
                                       0x58, 0x52, 0x32, 0x34,
                                       1, 0, 0, 0, 2, 0, 0, 0,
                                       4, 0, 0, 0, 0, 0, 0, 0};
        vitrin_view_evt_frame_ready_t msg;
        msg.fd = 42; /* never enters the byte buffer */
        msg.format = VITRIN_VIEW_FORMAT_XRGB8888;
        msg.width = 1;
        msg.height = 2;
        msg.stride = 4;
        msg.flags = (vitrin_view_frame_flags_t)0;
        int32_t n = vitrin_view_evt_frame_ready_encode(&msg, 5, buf, sizeof buf);
        expect_bytes("frame_ready", buf, n, want, sizeof want);

        uint32_t object_id = 0;
        vitrin_view_evt_frame_ready_t back;
        expect("frame_ready decode ok",
               vitrin_view_evt_frame_ready_decode(want, sizeof want, 42, &object_id,
                                                  &back) == VITRIN_DECODE_OK);
        expect("frame_ready decode fields",
               object_id == 5u && back.fd == 42 &&
                   back.format == VITRIN_VIEW_FORMAT_XRGB8888 && back.stride == 4u);
    }

    /* -- negative paths the compile-only test cannot see ---------------- */
    {
        /* per-string encode bound: get_realm's name is (max 64 bytes) */
        static uint8_t big[65]; /* zero-initialized content is irrelevant */
        vitrin_principal_req_get_realm_t msg;
        msg.realm = 2;
        msg.name.len = (uint32_t)sizeof big;
        msg.name.data = big;
        expect("encode rejects an over-bound string",
               vitrin_principal_req_get_realm_encode(&msg, 7, buf, sizeof buf) ==
                   VITRIN_ENCODE_ERR_STRING_TOO_LONG);
    }
    {
        /* forged size field */
        uint8_t frame[] = {1, 0, 0, 0, 8, 0, 1, 0, 42, 0, 0, 0};
        uint32_t object_id;
        vitrin_handshake_req_sync_t back;
        expect("decode rejects a lying size field",
               vitrin_handshake_req_sync_decode(frame, sizeof frame, -1, &object_id,
                                                &back) == VITRIN_DECODE_ERR_SIZE_MISMATCH);
    }
    {
        /* forged opcode byte */
        uint8_t frame[] = {1, 0, 0, 0, 12, 0, 0, 0, 42, 0, 0, 0};
        uint32_t object_id;
        vitrin_handshake_req_sync_t back;
        expect("decode rejects a lying opcode byte",
               vitrin_handshake_req_sync_decode(frame, sizeof frame, -1, &object_id,
                                                &back) == VITRIN_DECODE_ERR_OPCODE_MISMATCH);
    }
    {
        /* nonzero string padding byte */
        uint8_t frame[] = {7, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0,
                           1, 0, 0, 0, 'a', 0xff, 0, 0};
        uint32_t object_id;
        vitrin_principal_req_get_realm_t back;
        expect("decode rejects nonzero string padding",
               vitrin_principal_req_get_realm_decode(frame, sizeof frame, -1, &object_id,
                                                     &back) ==
                   VITRIN_DECODE_ERR_MALFORMED_PADDING);
    }

    if (failures) {
        fprintf(stderr, "%d golden-frame check(s) failed\n", failures);
        return 1;
    }
    puts("all golden-frame checks passed");
    return 0;
}
