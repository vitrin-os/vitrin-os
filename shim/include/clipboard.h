/* clipboard.h -- the shim half of the cross-realm clipboard (WS-E.2.1,
 * issue #213).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * ====================================================================
 * WHAT THIS IS, AND WHAT IT IS NOT
 * ====================================================================
 *
 * `wl_data_device_manager` (globals.c) mediates selection BETWEEN CLIENTS OF
 * THIS SEAT, of which there is one. That is app-internal clipboard and it
 * grants nothing across the realm boundary. This file is the other thing:
 * the endpoint of a channel the CORE mediates, driven by a physical human
 * gesture and by nothing else.
 *
 * Three messages, and the direction of each is the design (D-024):
 *
 *   request_selection(serial)      core -> shim   "the human pressed promote
 *                                                  while looking at you; what
 *                                                  is your selection?"
 *   selection(serial, status, ...) shim -> core   the one answer, always sent
 *   offer_selection(mime, data)    core -> shim   "the human pressed offer
 *                                                  while looking at you; make
 *                                                  this your app's selection"
 *
 * THE SHIM NEVER SPEAKS FIRST. There is deliberately no message by which a
 * shim announces a copy, and this file must never grow one: a shim that
 * pushed every app-side copy upstream would make every ordinary Ctrl-C a
 * cross-realm event, and the core would learn every copy a human made inside
 * every realm. The core asks; this answers. That asymmetry is the whole of
 * why the Qubes phrase the design quotes -- "cannot be triggered/forced by
 * any" realm -- is literally true here.
 *
 * `offer_selection` does NOT paste. It makes the payload the app's ordinary
 * selection; the human then presses their own app's paste key. That is the
 * Qubes model exactly, and it is what keeps the gesture count at two.
 *
 * ====================================================================
 * WHY BOTH HALVES ARE ASYNCHRONOUS, AND WHY EACH HAS A DEADLINE
 * ====================================================================
 *
 * Wayland's selection transfer is a pipe between two clients. Reading the
 * app's selection means handing it a write end and waiting; writing ours
 * means taking its read end and waiting. Both peers are the confined app,
 * so neither wait may be unbounded: an app that accepts a descriptor and
 * never touches it would otherwise wedge the shim's event loop against a
 * core that is waiting for an answer.
 *
 * So each half carries a timer, and the read half ALWAYS answers -- with
 * `empty` if the deadline passes. Silence is indistinguishable from a hung
 * shim, and the core must never be left waiting on an untrusted peer.
 *
 * ====================================================================
 * WHAT MAY CROSS
 * ====================================================================
 *
 * `text/plain;charset=utf-8` only, at most VITRIN_CLIPBOARD_MAX_BYTES, and
 * the shim judges both before the wire does. That is not defence in depth
 * for its own sake: a `data` string past its IDL bound is fatal
 * `invalid_argument`, which on a shim connection means log-and-close, so a
 * human copying a large file would kill their own app's shim. Answering
 * `too_large` is the difference between a clipboard that declines and a
 * session that dies.
 *
 * The core re-judges everything this file claims. A shim is untrusted and
 * `ok` is a claim, not a credential.
 */

#ifndef VITRIN_SHIM_CLIPBOARD_H
#define VITRIN_SHIM_CLIPBOARD_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <wayland-server-core.h>

struct vitrin_shim;
struct wlr_data_source;

/* The one MIME type that crosses. Must equal `clipboard::CLIPBOARD_MIME` in
 * the core; both are checked against the other end by
 * tests/integration/test_real_clipboard.py. */
#define VITRIN_CLIPBOARD_MIME "text/plain;charset=utf-8"

/* The byte cap, equal to the IDL's `(max N bytes)` bound on
 * `vitrin_shim_session.selection.data`. Static-asserted against the
 * generated encoder's own bound in clipboard.c, so the two cannot drift. */
#define VITRIN_CLIPBOARD_MAX_BYTES 61440u

/* How long the app has to write its selection into the pipe before the read
 * is abandoned and the core is answered `empty`.
 *
 * Two seconds: long enough that a toolkit's ordinary "serve the selection on
 * the next main-loop turn" cannot lose the race even on a loaded machine,
 * short enough that a human who pressed the chord and got nothing has not
 * been left wondering. The failure it bounds is an app that accepts the
 * descriptor and never writes, which no honest client does and any hostile
 * one can. */
#define VITRIN_CLIPBOARD_READ_TIMEOUT_MS 2000

/* How long a client has to drain a selection the shim is serving it before
 * that transfer is abandoned. Same reasoning, other direction. */
#define VITRIN_CLIPBOARD_WRITE_TIMEOUT_MS 2000

/* The most concurrent outgoing transfers the shim will serve. A paste is a
 * human act and a toolkit asks once; more than a handful in flight means a
 * client is opening transfers it does not read, so the cap is a
 * denial-of-service bound rather than a capacity estimate. Beyond it a
 * transfer is closed immediately, which the client sees as an empty
 * selection. */
#define VITRIN_CLIPBOARD_MAX_TRANSFERS 8u

/* One in-flight read of the app's selection: the pipe the app writes into,
 * its readiness source, its deadline, and the bytes so far. */
struct vitrin_clipboard_read {
	bool active;
	uint32_t serial;
	int fd;
	struct wl_event_source *readable;
	struct wl_event_source *deadline;
	size_t len;
	/* One byte past the cap, so "the app offered more than may cross" is
	 * observable rather than silently truncated to exactly the cap. */
	uint8_t buf[VITRIN_CLIPBOARD_MAX_BYTES + 1u];
};

/* The shim's clipboard state. Lives in `struct vitrin_shim`. */
struct vitrin_clipboard {
	/* Back-pointer, because the wlroots listeners and the event-loop
	 * callbacks here need the shim and none of them is reachable by
	 * `wl_container_of` from a source. */
	struct vitrin_shim *shim;
	/* Whether `wire()` attached the listener below; teardown must not
	 * `wl_list_remove` a link that was never inserted. */
	bool wired;
	/* wlr_seat.events.request_set_selection: the app asked to own the
	 * selection. The shim honours it -- that is the app-internal clipboard
	 * `wl_data_device_manager` exists for -- and sends NOTHING upstream. */
	struct wl_listener request_set_selection;

	struct vitrin_clipboard_read read;

	/* Live outgoing transfers, for the cap above. */
	size_t transfers;

	/* The shim-owned source currently installed as the seat's selection, or
	 * NULL. Owned by wlroots once installed; this pointer is cleared by its
	 * destroy path. */
	struct wlr_data_source *offered;
};

/* Attach the selection listener. Called from `vitrin_create_globals` after
 * the seat exists. Safe to call once; `false` means the shim must not
 * continue, on the same terms as every other bring-up step. */
bool vitrin_clipboard_wire(struct vitrin_shim *s);

/* Release every clipboard resource: the listener, an in-flight read, and the
 * installed source. Idempotent, and safe on a shim that never wired. */
void vitrin_clipboard_finish(struct vitrin_shim *s);

/* `vitrin_shim_session.request_selection` arrived. Always results in exactly
 * one `selection` request, now or when the read finishes or times out. */
void vitrin_clipboard_handle_request(struct vitrin_shim *s, uint32_t serial);

/* `vitrin_shim_session.offer_selection` arrived: install `data` as the app's
 * ordinary selection. `mime` is checked against VITRIN_CLIPBOARD_MIME and a
 * mismatch is logged and ignored -- the core does not send one, so a mismatch
 * means the two ends disagree about the allow-list and installing it anyway
 * would be the wrong half to trust. */
void vitrin_clipboard_handle_offer(struct vitrin_shim *s, const char *mime,
	const uint8_t *data, size_t len);

#endif /* VITRIN_SHIM_CLIPBOARD_H */
