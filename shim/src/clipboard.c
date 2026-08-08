/* clipboard.c -- the shim half of the cross-realm clipboard (WS-E.2.1,
 * issue #213). Read clipboard.h first: it carries the design and the reason
 * this file never speaks first.
 *
 * SPDX-License-Identifier: MPL-2.0
 */

/* `pipe2` and `O_CLOEXEC` are GNU/Linux extensions past POSIX.1-2008, and
 * this shim is Linux-only (evdev codes, memfd, wlroots). */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <wlr/types/wlr_data_device.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/util/log.h>

#include "clipboard.h"
#include "server.h"
#include "upstream.h"
#include "vitrin-protocol.h"
#include "wire.h"

/* The shim's cap and the wire's must be the same number; the generated
 * encoder refuses anything longer, and a shim that believed a larger one
 * would build frames it could not send. */
_Static_assert(VITRIN_CLIPBOARD_MAX_BYTES == 61440u,
	"VITRIN_CLIPBOARD_MAX_BYTES must equal the IDL's (max N bytes) bound on "
	"vitrin_shim_session.selection.data");

/* The whole `selection` frame at the cap, plus room for its header, the two
 * scalars and the MIME string. Static, not malloc'd: the encode is a plain
 * memcpy into it and the frame path in this shim does not allocate. */
#define VITRIN_CLIPBOARD_FRAME_BUF (VITRIN_CLIPBOARD_MAX_BYTES + 256u)

/* ---- what may cross ------------------------------------------------------ */

/* Whether `bytes` is well-formed UTF-8 with no embedded NUL.
 *
 * Hand-written rather than borrowed, because the generated header does not
 * validate on the *encode* side (it is a borrowed-view API by design) and the
 * shim must not build a frame the core will kill it over. The rules are the
 * wire's: RFC 3629's four forms, shortest-encoding enforced by the per-form
 * lower bound, surrogates excluded, U+10FFFF the ceiling, and NUL forbidden
 * outright because the string type forbids it.
 *
 * An app's selection is arbitrary bytes it chose. Getting this wrong in the
 * permissive direction costs this shim its connection -- fatal
 * `invalid_argument` at the core's decoder -- so it errs the other way: a
 * sequence this does not recognise is answered `wrong_type`, never sent. */
static bool clipboard_utf8_valid(const uint8_t *bytes, size_t len) {
	size_t i = 0;
	while (i < len) {
		uint8_t b = bytes[i];
		if (b == 0x00u) {
			return false; /* the wire's string type forbids an embedded NUL */
		}
		size_t extra;
		uint32_t cp;
		if (b < 0x80u) {
			i++;
			continue;
		} else if ((b & 0xE0u) == 0xC0u) {
			extra = 1;
			cp = b & 0x1Fu;
		} else if ((b & 0xF0u) == 0xE0u) {
			extra = 2;
			cp = b & 0x0Fu;
		} else if ((b & 0xF8u) == 0xF0u) {
			extra = 3;
			cp = b & 0x07u;
		} else {
			return false; /* a continuation byte or an invalid lead */
		}
		/* `len - i` is at least 1 here, so this cannot wrap: the sequence
		 * needs `extra` more bytes than the lead, and there are `len - i - 1`
		 * left after it. */
		if (extra >= len - i) {
			return false; /* truncated sequence */
		}
		for (size_t k = 1; k <= extra; k++) {
			uint8_t c = bytes[i + k];
			if ((c & 0xC0u) != 0x80u) {
				return false;
			}
			cp = (cp << 6) | (uint32_t)(c & 0x3Fu);
		}
		static const uint32_t lowest[4] = {0u, 0x80u, 0x800u, 0x10000u};
		if (cp < lowest[extra]) {
			return false; /* overlong */
		}
		if (cp > 0x10FFFFu || (cp >= 0xD800u && cp <= 0xDFFFu)) {
			return false; /* out of range, or a surrogate */
		}
		i += extra + 1u;
	}
	return true;
}

/* ---- answering ---------------------------------------------------------- */

/* Send the one `selection` this shim owes the core.
 *
 * EVERY `request_selection` gets exactly one of these, including a refusing
 * one: silence is indistinguishable from a hung shim and would leave the core
 * waiting on an untrusted peer. `mime` and `data` are empty for every status
 * but `ok`, as the IDL requires.
 *
 * The bytes are never logged. Length only -- the same contract the core's
 * recorder keeps, applied on this side so a shim log cannot become the leak
 * the journal refuses to be. */
static void answer(struct vitrin_shim *s, uint32_t serial,
		vitrin_shim_session_selection_status_t status,
		const uint8_t *data, size_t len) {
	static uint8_t frame[VITRIN_CLIPBOARD_FRAME_BUF];
	bool ok = status == VITRIN_SHIM_SESSION_SELECTION_STATUS_OK;
	vitrin_shim_session_req_selection_t msg = {
		.serial = serial,
		.status = status,
		.mime = {.len = ok ? (uint32_t)strlen(VITRIN_CLIPBOARD_MIME) : 0u,
			.data = ok ? (const uint8_t *)VITRIN_CLIPBOARD_MIME : NULL},
		.data = {.len = ok ? (uint32_t)len : 0u, .data = ok ? data : NULL},
	};
	int32_t n = vitrin_shim_session_req_selection_encode(&msg, VITRIN_SESSION_ID,
		frame, sizeof(frame));
	if (n < 0) {
		wlr_log(WLR_ERROR, "clipboard: could not encode a selection answer (%d)", n);
		return;
	}
	if (!vitrin_wire_send(&s->up.wire, frame, (size_t)n, -1)) {
		wlr_log(WLR_ERROR, "clipboard: could not send the selection answer");
		return;
	}
	wlr_log(WLR_INFO, "clipboard: answered request_selection serial=%u status=%d bytes=%zu",
		serial, (int)status, ok ? len : (size_t)0);
}

/* ---- reading the app's selection ---------------------------------------- */

static void read_teardown(struct vitrin_clipboard *c) {
	if (c->read.readable != NULL) {
		wl_event_source_remove(c->read.readable);
		c->read.readable = NULL;
	}
	if (c->read.deadline != NULL) {
		wl_event_source_remove(c->read.deadline);
		c->read.deadline = NULL;
	}
	if (c->read.fd >= 0) {
		close(c->read.fd);
		c->read.fd = -1;
	}
	/* The buffer is wiped rather than merely forgotten: it held an
	 * application's selection, which may be a password, and the shim is a
	 * long-lived process whose core dump or later heap reuse should not
	 * carry it. Length only ever leaves this file. */
	memset(c->read.buf, 0, sizeof(c->read.buf));
	c->read.len = 0;
	c->read.active = false;
}

/* Finish an in-flight read and answer the core. */
static void read_finish(struct vitrin_shim *s) {
	struct vitrin_clipboard *c = &s->clipboard;
	uint32_t serial = c->read.serial;
	size_t len = c->read.len;

	if (len > VITRIN_CLIPBOARD_MAX_BYTES) {
		read_teardown(c);
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_TOO_LARGE, NULL, 0);
		return;
	}
	if (len == 0) {
		read_teardown(c);
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY, NULL, 0);
		return;
	}
	/* The app said `text/plain;charset=utf-8`; that is a claim about bytes it
	 * chose. Well-formedness is checked here rather than left to the core,
	 * because ill-formed UTF-8 or an embedded NUL is fatal `invalid_argument`
	 * at the decoder and would kill this shim -- the app's own connection --
	 * over its own bad clipboard. */
	if (!clipboard_utf8_valid(c->read.buf, len)) {
		read_teardown(c);
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_WRONG_TYPE, NULL, 0);
		return;
	}
	answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_OK, c->read.buf, len);
	read_teardown(c);
}

static int read_deadline(void *data) {
	struct vitrin_shim *s = data;
	wlr_log(WLR_INFO, "clipboard: the app did not write its selection in time");
	uint32_t serial = s->clipboard.read.serial;
	read_teardown(&s->clipboard);
	answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY, NULL, 0);
	return 0;
}

static int read_readable(int fd, uint32_t mask, void *data) {
	struct vitrin_shim *s = data;
	struct vitrin_clipboard *c = &s->clipboard;
	if (!c->read.active) {
		return 0;
	}
	for (;;) {
		if (c->read.len >= sizeof(c->read.buf)) {
			/* One byte past the cap already read: the app is offering more
			 * than may cross. Stop reading -- draining the rest would only
			 * copy bytes that cannot be sent -- and answer `too_large`. */
			read_finish(s);
			return 0;
		}
		ssize_t n = read(fd, c->read.buf + c->read.len, sizeof(c->read.buf) - c->read.len);
		if (n > 0) {
			c->read.len += (size_t)n;
			continue;
		}
		if (n == 0) {
			read_finish(s);
			return 0;
		}
		if (errno == EINTR) {
			continue;
		}
		if (errno == EAGAIN || errno == EWOULDBLOCK) {
			/* More may come; the deadline still bounds the wait. A hangup
			 * without readability is EOF and is handled on the next turn. */
			if ((mask & WL_EVENT_HANGUP) != 0) {
				read_finish(s);
			}
			return 0;
		}
		wlr_log(WLR_ERROR, "clipboard: reading the app's selection failed");
		read_finish(s);
		return 0;
	}
}

/* Does this source offer the one type that may cross? */
static bool offers_our_mime(struct wlr_data_source *source) {
	char **type;
	wl_array_for_each (type, &source->mime_types) {
		if (*type != NULL && strcmp(*type, VITRIN_CLIPBOARD_MIME) == 0) {
			return true;
		}
	}
	return false;
}

void vitrin_clipboard_handle_request(struct vitrin_shim *s, uint32_t serial) {
	struct vitrin_clipboard *c = &s->clipboard;

	/* A second question supersedes the first: the human pressed again, so the
	 * earlier answer is stale by their own account. The core's own serial
	 * matching would discard it anyway; abandoning it here keeps exactly one
	 * pipe open at a time. */
	if (c->read.active) {
		read_teardown(c);
	}

	struct wlr_data_source *source = s->seat != NULL ? s->seat->selection_source : NULL;
	if (source == NULL) {
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY, NULL, 0);
		return;
	}
	if (!offers_our_mime(source)) {
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_WRONG_TYPE, NULL, 0);
		return;
	}

	int fds[2];
	if (pipe2(fds, O_CLOEXEC | O_NONBLOCK) != 0) {
		wlr_log(WLR_ERROR, "clipboard: pipe2 failed");
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY, NULL, 0);
		return;
	}

	c->read.active = true;
	c->read.serial = serial;
	c->read.fd = fds[0];
	c->read.len = 0;
	c->read.readable = wl_event_loop_add_fd(s->loop, fds[0],
		WL_EVENT_READABLE, read_readable, s);
	c->read.deadline = wl_event_loop_add_timer(s->loop, read_deadline, s);
	if (c->read.readable == NULL || c->read.deadline == NULL) {
		wlr_log(WLR_ERROR, "clipboard: could not watch the selection pipe");
		close(fds[1]);
		read_teardown(c);
		answer(s, serial, VITRIN_SHIM_SESSION_SELECTION_STATUS_EMPTY, NULL, 0);
		return;
	}
	wl_event_source_timer_update(c->read.deadline, VITRIN_CLIPBOARD_READ_TIMEOUT_MS);

	/* wlroots hands the write end to the client and closes it here. */
	wlr_data_source_send(source, VITRIN_CLIPBOARD_MIME, fds[1]);
}

/* ---- serving the core's slot to the app --------------------------------- */

/* One outgoing transfer: our bytes, the client's read end, and its deadline.
 *
 * Each transfer owns its own copy. A refcounted buffer would be smaller and
 * would tie every in-flight transfer's lifetime to the source's, which is
 * exactly the coupling that makes selection code leak: a copy per transfer is
 * at most VITRIN_CLIPBOARD_MAX_BYTES times a small cap, on a path a human
 * drives by hand. */
struct clip_transfer {
	struct vitrin_shim *shim;
	int fd;
	struct wl_event_source *writable;
	struct wl_event_source *deadline;
	size_t off;
	size_t len;
	uint8_t *bytes;
};

static void transfer_free(struct clip_transfer *t) {
	if (t->writable != NULL) {
		wl_event_source_remove(t->writable);
	}
	if (t->deadline != NULL) {
		wl_event_source_remove(t->deadline);
	}
	if (t->fd >= 0) {
		close(t->fd);
	}
	if (t->bytes != NULL) {
		/* Wiped, not merely freed: see `read_teardown`. */
		memset(t->bytes, 0, t->len);
		free(t->bytes);
	}
	if (t->shim != NULL && t->shim->clipboard.transfers > 0) {
		t->shim->clipboard.transfers--;
	}
	free(t);
}

static int transfer_deadline(void *data) {
	struct clip_transfer *t = data;
	wlr_log(WLR_INFO, "clipboard: a selection transfer was not drained in time");
	transfer_free(t);
	return 0;
}

static int transfer_writable(int fd, uint32_t mask, void *data) {
	struct clip_transfer *t = data;
	if ((mask & (WL_EVENT_ERROR | WL_EVENT_HANGUP)) != 0) {
		transfer_free(t);
		return 0;
	}
	while (t->off < t->len) {
		ssize_t n = write(fd, t->bytes + t->off, t->len - t->off);
		if (n > 0) {
			t->off += (size_t)n;
			continue;
		}
		if (n < 0 && errno == EINTR) {
			continue;
		}
		if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
			return 0; /* the deadline still bounds the wait */
		}
		/* EPIPE and friends: the client stopped reading. Its problem. */
		transfer_free(t);
		return 0;
	}
	transfer_free(t);
	return 0;
}

/* The shim-owned data source installed by `offer_selection`. */
struct clip_source {
	struct wlr_data_source base;
	struct vitrin_shim *shim;
	size_t len;
	uint8_t *bytes;
};

static void clip_source_send(struct wlr_data_source *base, const char *mime_type, int32_t fd) {
	struct clip_source *src = (struct clip_source *)base;
	if (mime_type == NULL || strcmp(mime_type, VITRIN_CLIPBOARD_MIME) != 0) {
		close(fd);
		return;
	}
	struct vitrin_clipboard *c = &src->shim->clipboard;
	if (c->transfers >= VITRIN_CLIPBOARD_MAX_TRANSFERS) {
		wlr_log(WLR_INFO, "clipboard: transfer cap reached; closing a request unserved");
		close(fd);
		return;
	}
	struct clip_transfer *t = calloc(1, sizeof(*t));
	uint8_t *copy = src->len > 0 ? malloc(src->len) : NULL;
	if (t == NULL || (src->len > 0 && copy == NULL)) {
		free(t);
		free(copy);
		close(fd);
		return;
	}
	if (src->len > 0) {
		memcpy(copy, src->bytes, src->len);
	}
	t->shim = src->shim;
	t->fd = fd;
	t->bytes = copy;
	t->len = src->len;
	/* The client chose this descriptor's flags; the shim must not block on
	 * it whatever they were. */
	int flags = fcntl(fd, F_GETFL, 0);
	if (flags >= 0) {
		(void)fcntl(fd, F_SETFL, flags | O_NONBLOCK);
	}
	t->writable = wl_event_loop_add_fd(src->shim->loop, fd, WL_EVENT_WRITABLE,
		transfer_writable, t);
	t->deadline = wl_event_loop_add_timer(src->shim->loop, transfer_deadline, t);
	if (t->writable == NULL || t->deadline == NULL) {
		transfer_free(t);
		return;
	}
	c->transfers++;
	wl_event_source_timer_update(t->deadline, VITRIN_CLIPBOARD_WRITE_TIMEOUT_MS);
}

static void clip_source_destroy(struct wlr_data_source *base) {
	struct clip_source *src = (struct clip_source *)base;
	if (src->shim != NULL && src->shim->clipboard.offered == base) {
		src->shim->clipboard.offered = NULL;
	}
	if (src->bytes != NULL) {
		memset(src->bytes, 0, src->len);
		free(src->bytes);
	}
	free(src);
}

static const struct wlr_data_source_impl clip_source_impl = {
	.send = clip_source_send,
	.destroy = clip_source_destroy,
};

void vitrin_clipboard_handle_offer(struct vitrin_shim *s, const char *mime,
		const uint8_t *data, size_t len) {
	if (mime == NULL || strcmp(mime, VITRIN_CLIPBOARD_MIME) != 0) {
		/* The core never sends another type, so a mismatch means the two ends
		 * disagree about the allow-list. Installing it anyway would be
		 * trusting the wrong half. */
		wlr_log(WLR_ERROR, "clipboard: offer_selection carried an unexpected type; ignored");
		return;
	}
	if (s->seat == NULL || len == 0 || len > VITRIN_CLIPBOARD_MAX_BYTES) {
		return;
	}
	struct clip_source *src = calloc(1, sizeof(*src));
	uint8_t *copy = malloc(len);
	if (src == NULL || copy == NULL) {
		free(src);
		free(copy);
		wlr_log(WLR_ERROR, "clipboard: out of memory installing an offered selection");
		return;
	}
	memcpy(copy, data, len);
	src->shim = s;
	src->bytes = copy;
	src->len = len;
	wlr_data_source_init(&src->base, &clip_source_impl);

	char **slot = wl_array_add(&src->base.mime_types, sizeof(char *));
	if (slot == NULL) {
		wlr_data_source_destroy(&src->base);
		return;
	}
	*slot = strdup(VITRIN_CLIPBOARD_MIME);
	if (*slot == NULL) {
		wlr_data_source_destroy(&src->base);
		return;
	}

	s->clipboard.offered = &src->base;
	/* Replaces whatever the app had selected, which is the point: the human
	 * asked for the core's slot to become this realm's clipboard. The app
	 * still has to be told to paste, by the human, with its own key. */
	wlr_seat_set_selection(s->seat, &src->base, wl_display_next_serial(s->display));
	wlr_log(WLR_INFO, "clipboard: offered %zu bytes to the app as its selection", len);
}

/* ---- the app's own copies stay the app's own ---------------------------- */

/* wlr_seat.events.request_set_selection.
 *
 * The app asked to own the selection. The shim honours it -- without this the
 * `wl_data_device_manager` global would advertise a clipboard that never
 * worked even *within* the app -- and it sends NOTHING upstream. That absence
 * is the design: see clipboard.h. */
static void on_request_set_selection(struct wl_listener *listener, void *data) {
	struct vitrin_clipboard *c = wl_container_of(listener, c, request_set_selection);
	struct wlr_seat_request_set_selection_event *event = data;
	if (c->offered != NULL && event->source != c->offered) {
		/* The app is taking the selection back from the slot the core
		 * offered. Ordinary, and nothing to report: the core's slot is
		 * unaffected, and it is the core's alone to clear. */
		c->offered = NULL;
	}
	wlr_seat_set_selection(c->shim->seat, event->source, event->serial);
}

bool vitrin_clipboard_wire(struct vitrin_shim *s) {
	struct vitrin_clipboard *c = &s->clipboard;
	c->shim = s;
	c->read.fd = -1;
	if (s->seat == NULL) {
		wlr_log(WLR_ERROR, "clipboard: no seat to attach to");
		return false;
	}
	c->request_set_selection.notify = on_request_set_selection;
	wl_signal_add(&s->seat->events.request_set_selection, &c->request_set_selection);
	c->wired = true;
	return true;
}

void vitrin_clipboard_finish(struct vitrin_shim *s) {
	struct vitrin_clipboard *c = &s->clipboard;
	if (c->wired) {
		wl_list_remove(&c->request_set_selection.link);
		c->wired = false;
	}
	read_teardown(c);
	/* The installed source is wlroots' to destroy with the seat; clearing the
	 * pointer keeps `clip_source_destroy` from writing through a shim that is
	 * on its way out. */
	c->offered = NULL;
	c->shim = NULL;
}
