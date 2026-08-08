/* wire.c -- framed send/receive over the core socketpair. See wire.h for
 * the design and for why the receive path needs no fd machinery.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * What happens, in what order:
 *
 *   adopt   -- claim fd 3, re-arm FD_CLOEXEC, go non-blocking
 *   send    -- flush anything parked, sendmsg (fd as SCM_RIGHTS on the first
 *              write), park the tail the kernel refused
 *   recv    -- recvmsg into a reassembly buffer, split on the header's own
 *              size field, hand whole frames to the protocol layer
 *   arm     -- put the descriptor on the Wayland event loop, watching for
 *              write readiness only while something is parked
 */
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include <wayland-server-core.h>

#include <wlr/util/log.h>

#include "vitrin-protocol.h"
#include "wire.h"

/* Declare the fatal condition once. Sticky: after this the stream may be
 * torn (a partial write leaves a frame prefix in the kernel buffer that no
 * later send may append past), so every operation refuses from here on. */
static bool wire_fail(struct vitrin_wire *w, const char *reason) {
	if (!w->failed) {
		w->failed = true;
		w->fail_reason = reason;
		wlr_log(WLR_ERROR, "core connection: %s", reason);
	}
	return false;
}

bool vitrin_wire_alive(const struct vitrin_wire *w) {
	return w->fd >= 0 && !w->failed && !w->eof;
}

bool vitrin_wire_adopt(struct vitrin_wire *w) {
	w->fd = -1;

	/* Prove the descriptor is what the spawn contract says before claiming
	 * it. fstat is enough and cannot block: fd 3 is either the core's
	 * socketpair end or -- when this binary is run by hand -- whatever the
	 * shell happened to leave there, usually nothing. */
	struct stat st;
	if (fstat(VITRIN_CORE_FD, &st) != 0) {
		wlr_log(WLR_ERROR,
			"fd %d is not open: this program is spawned by the trusted core, "
			"which places the shim's end of the identity socketpair there at "
			"fork. Pass --no-upstream to run it standalone.",
			VITRIN_CORE_FD);
		return false;
	}
	if (!S_ISSOCK(st.st_mode)) {
		wlr_log(WLR_ERROR, "fd %d is open but is not a socket", VITRIN_CORE_FD);
		return false;
	}

	/* THE one non-negotiable line of the spawn contract. fd 3 arrived with
	 * FD_CLOEXEC cleared -- that is how it survived execve -- and every
	 * process this shim spawns from here on must not inherit the core
	 * connection. Without it the confined app holds a live channel to the
	 * TCB, which is the whole confinement gone. */
	int flags = fcntl(VITRIN_CORE_FD, F_GETFD);
	if (flags < 0 || fcntl(VITRIN_CORE_FD, F_SETFD, flags | FD_CLOEXEC) != 0) {
		wlr_log(WLR_ERROR, "cannot re-arm FD_CLOEXEC on fd %d: %s",
			VITRIN_CORE_FD, strerror(errno));
		return false;
	}

	/* Non-blocking for the connection's whole life: the core link is pumped
	 * by the app's event loop, and a core that stops reading must never
	 * wedge the loop that serves the app. */
	int fl = fcntl(VITRIN_CORE_FD, F_GETFL);
	if (fl < 0 || fcntl(VITRIN_CORE_FD, F_SETFL, fl | O_NONBLOCK) != 0) {
		wlr_log(WLR_ERROR, "cannot set O_NONBLOCK on fd %d: %s",
			VITRIN_CORE_FD, strerror(errno));
		return false;
	}

	w->fd = VITRIN_CORE_FD;
	return true;
}

/* ---- send ---------------------------------------------------------- */

/* One write attempt: `fd` (if any) rides the first sendmsg as ancillary
 * data, then plain send()s carry the rest until done or EAGAIN. Returns
 * bytes written (0 means nothing went out, so the fd did NOT), or -1 fatal.
 */
static ssize_t wire_write(struct vitrin_wire *w, const uint8_t *bytes, size_t len, int fd) {
	union {
		struct cmsghdr align;
		char buf[CMSG_SPACE(sizeof(int))];
	} control;
	struct iovec iov = {.iov_base = (void *)(uintptr_t)bytes, .iov_len = len};
	struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1};

	if (fd >= 0) {
		memset(&control, 0, sizeof(control));
		msg.msg_control = control.buf;
		msg.msg_controllen = sizeof(control.buf);
		struct cmsghdr *cm = CMSG_FIRSTHDR(&msg);
		cm->cmsg_level = SOL_SOCKET;
		cm->cmsg_type = SCM_RIGHTS;
		cm->cmsg_len = CMSG_LEN(sizeof(int));
		memcpy(CMSG_DATA(cm), &fd, sizeof(int));
	}

	ssize_t n;
	do {
		/* MSG_NOSIGNAL: a core that closed its end surfaces as EPIPE here,
		 * never as SIGPIPE killing the shim mid-frame. */
		n = sendmsg(w->fd, &msg, MSG_NOSIGNAL);
	} while (n < 0 && errno == EINTR);

	if (n < 0) {
		if (errno == EAGAIN || errno == EWOULDBLOCK) {
			return 0;
		}
		if (errno == EPIPE || errno == ECONNRESET) {
			/* The core is gone. Not a protocol failure: treat it as EOF so
			 * teardown takes the ordinary shim-death path. */
			w->eof = true;
			return -1;
		}
		wire_fail(w, "sendmsg failed");
		return -1;
	}

	size_t sent = (size_t)n;
	while (sent < len) {
		ssize_t m;
		do {
			m = send(w->fd, bytes + sent, len - sent, MSG_NOSIGNAL);
		} while (m < 0 && errno == EINTR);
		if (m < 0) {
			if (errno == EAGAIN || errno == EWOULDBLOCK) {
				break;
			}
			if (errno == EPIPE || errno == ECONNRESET) {
				w->eof = true;
				return -1;
			}
			/* An error AFTER a partial write left a torn frame prefix in
			 * the kernel buffer; nothing may ever be appended past it. */
			wire_fail(w, "send failed mid-frame");
			return -1;
		}
		if (m == 0) {
			wire_fail(w, "send returned 0 mid-frame");
			return -1;
		}
		sent += (size_t)m;
	}
	return (ssize_t)sent;
}

/* Park an unsent frame tail. `fd` is duplicated only when the kernel has not
 * already taken it (i.e. when zero bytes of the frame went out). */
static bool wire_park(struct vitrin_wire *w, const uint8_t *bytes, size_t len, int fd) {
	if (len > VITRIN_WIRE_SLOT_BYTES) {
		/* The oversized slot (WS-E.2.1). Accepted only when nothing else is
		 * parked -- the caller has just flushed -- so "drain big, then the
		 * queue" preserves wire order. A second while one is pending means
		 * the core stopped reading, which is the conclusion a full queue
		 * reaches too. */
		if (w->big_len > w->big_offset || w->queue_len > 0) {
			return wire_fail(w, "a second oversized frame while one is unsent: "
			                    "the core has stopped reading");
		}
		memcpy(w->big, bytes, len);
		w->big_len = len;
		w->big_offset = 0;
		return true;
	}
	if (w->queue_len >= VITRIN_WIRE_QUEUE_SLOTS) {
		return wire_fail(w, "outgoing queue full: the core has stopped reading");
	}
	size_t idx = (w->queue_head + w->queue_len) % VITRIN_WIRE_QUEUE_SLOTS;
	struct vitrin_wire_slot *slot = &w->queue[idx];
	memcpy(slot->bytes, bytes, len);
	slot->len = len;
	slot->offset = 0;
	slot->fd = -1;
	if (fd >= 0) {
		/* The caller's fd may be closed the moment this returns, so the
		 * queue needs its own. Close-on-exec, like every other fd here. */
		slot->fd = fcntl(fd, F_DUPFD_CLOEXEC, 0);
		if (slot->fd < 0) {
			return wire_fail(w, "cannot duplicate a buffer fd for the send queue");
		}
	}
	w->queue_len++;
	return true;
}

/* Write as much of the parked queue as the kernel will take. EAGAIN is
 * success ("wait for write readiness"), so returning true does not mean the
 * queue is now empty. */
static bool wire_flush(struct vitrin_wire *w) {
	/* The oversized slot first, always: it was accepted only when the queue
	 * was empty, so anything in the queue was handed over after it and must
	 * not overtake it (WS-E.2.1). */
	while (w->big_len > w->big_offset) {
		ssize_t n = wire_write(w, w->big + w->big_offset, w->big_len - w->big_offset, -1);
		if (n < 0) {
			return false;
		}
		w->big_offset += (size_t)n;
		if (n == 0) {
			return true; /* kernel is full; the rest waits for writability */
		}
	}
	w->big_len = 0;
	w->big_offset = 0;
	while (w->queue_len > 0) {
		struct vitrin_wire_slot *slot = &w->queue[w->queue_head];
		ssize_t n = wire_write(w, slot->bytes + slot->offset, slot->len - slot->offset, slot->fd);
		if (n < 0) {
			return false;
		}
		if (n > 0 && slot->fd >= 0) {
			/* Delivered with that first successful sendmsg; drop our dup so
			 * a resumed write cannot send it twice. */
			close(slot->fd);
			slot->fd = -1;
		}
		slot->offset += (size_t)n;
		if (slot->offset < slot->len) {
			return true; /* kernel is full; the rest waits for writability */
		}
		w->queue_head = (w->queue_head + 1) % VITRIN_WIRE_QUEUE_SLOTS;
		w->queue_len--;
	}
	return true;
}

/* Watch for write readiness only while something is parked; otherwise a
 * permanently writable socket would spin the loop. */
static void wire_update_mask(struct vitrin_wire *w) {
	if (w->source == NULL) {
		return;
	}
	uint32_t mask = WL_EVENT_READABLE;
	if (w->queue_len > 0 || w->big_len > w->big_offset) {
		mask |= WL_EVENT_WRITABLE;
	}
	wl_event_source_fd_update(w->source, mask);
}

bool vitrin_wire_send(struct vitrin_wire *w, const uint8_t *frame, size_t len, int fd) {
	if (!vitrin_wire_alive(w)) {
		return false;
	}
	/* Local misuse, caught before any byte hits the wire -- the core answers
	 * a malformed frame by closing without a word, so a bug here must be
	 * loud on this side instead (vitrin-ipc's `validate_outgoing`). */
	if (len < VITRIN_HEADER_LEN || len > VITRIN_WIRE_MAX_SEND) {
		return wire_fail(w, "outgoing frame outside the shim's size envelope");
	}
	/* The oversized path carries no ancillary data, by construction: every
	 * message in v0 that can exceed a queue slot has `fd_count` 0. Checked
	 * rather than assumed, because the parking slot below has nowhere to put
	 * an fd and a silent drop would be an fd leak plus a positional-matching
	 * violation at the core. */
	if (len > VITRIN_WIRE_SLOT_BYTES && fd >= 0) {
		return wire_fail(w, "an oversized outgoing frame may not carry a descriptor");
	}
	vitrin_frame_header_t hdr;
	if (vitrin_frame_header_decode(frame, len, &hdr) != VITRIN_DECODE_OK) {
		return wire_fail(w, "outgoing frame has no decodable header");
	}
	if ((size_t)hdr.size != len) {
		return wire_fail(w, "outgoing frame size field disagrees with its length");
	}
	if ((hdr.fd_count == 1) != (fd >= 0)) {
		return wire_fail(w, "outgoing fd_count disagrees with the attached fd");
	}

	/* Anything already parked must go first: the stream is ordered, and a
	 * frame that jumped the queue would interleave with a torn predecessor. */
	if (w->big_len > w->big_offset || w->queue_len > 0) {
		if (!wire_flush(w)) {
			return false;
		}
		if (w->big_len > w->big_offset || w->queue_len > 0) {
			bool ok = wire_park(w, frame, len, fd);
			wire_update_mask(w);
			return ok;
		}
	}

	ssize_t sent = wire_write(w, frame, len, fd);
	if (sent < 0) {
		return false;
	}
	if ((size_t)sent == len) {
		return true;
	}
	/* Partial write. Any successful sendmsg carried the whole ancillary
	 * payload, so the fd is still owed only if literally nothing went out. */
	bool ok = wire_park(w, frame + sent, len - (size_t)sent, sent == 0 ? fd : -1);
	wire_update_mask(w);
	return ok;
}

/* ---- receive -------------------------------------------------------- */

/* One recvmsg round. Returns false on a fatal condition; a would-block is
 * ordinary success with nothing appended, and EOF sets `w->eof`. */
static bool wire_fill(struct vitrin_wire *w) {
	if (w->rx_len == sizeof(w->rx)) {
		/* Unreachable while frames are drained after every fill: a full
		 * buffer means a frame larger than the format's own ceiling. */
		return wire_fail(w, "reassembly buffer full");
	}
	union {
		struct cmsghdr align;
		char buf[CMSG_SPACE(sizeof(int) * 4)];
	} control;
	memset(&control, 0, sizeof(control));
	struct iovec iov = {
		.iov_base = w->rx + w->rx_len,
		.iov_len = sizeof(w->rx) - w->rx_len,
	};
	struct msghdr msg = {
		.msg_iov = &iov,
		.msg_iovlen = 1,
		.msg_control = control.buf,
		.msg_controllen = sizeof(control.buf),
	};

	ssize_t n;
	do {
		/* MSG_CMSG_CLOEXEC so an fd we are about to reject is never
		 * inheritable, not even for the instant before we close it. */
		n = recvmsg(w->fd, &msg, MSG_CMSG_CLOEXEC);
	} while (n < 0 && errno == EINTR);

	if (n < 0) {
		if (errno == EAGAIN || errno == EWOULDBLOCK) {
			return true;
		}
		if (errno == ECONNRESET) {
			w->eof = true;
			return true;
		}
		return wire_fail(w, "recvmsg failed");
	}

	/* No version-1 core -> shim event carries a descriptor (wire.h). One
	 * arriving means the core is not the core we compiled against; close it
	 * before dying so the rejection leaks nothing. */
	bool unsolicited = false;
	for (struct cmsghdr *cm = CMSG_FIRSTHDR(&msg); cm != NULL; cm = CMSG_NXTHDR(&msg, cm)) {
		if (cm->cmsg_level != SOL_SOCKET || cm->cmsg_type != SCM_RIGHTS) {
			continue;
		}
		size_t payload = cm->cmsg_len - CMSG_LEN(0);
		for (size_t off = 0; off + sizeof(int) <= payload; off += sizeof(int)) {
			int got = -1;
			memcpy(&got, CMSG_DATA(cm) + off, sizeof(int));
			if (got >= 0) {
				close(got);
			}
			unsolicited = true;
		}
	}
	if (unsolicited) {
		return wire_fail(w, "core -> shim event carried a file descriptor");
	}
	if ((msg.msg_flags & MSG_CTRUNC) != 0) {
		return wire_fail(w, "ancillary data truncated");
	}

	if (n == 0) {
		w->eof = true;
		return true;
	}
	w->rx_len += (size_t)n;
	return true;
}

/* Split the reassembly buffer on the header's own size field and hand every
 * complete frame to the protocol layer. */
static bool wire_dispatch(struct vitrin_wire *w) {
	size_t pos = 0;
	while (w->rx_len - pos >= VITRIN_HEADER_LEN) {
		vitrin_frame_header_t hdr;
		if (vitrin_frame_header_decode(w->rx + pos, w->rx_len - pos, &hdr) != VITRIN_DECODE_OK) {
			return wire_fail(w, "undecodable frame header");
		}
		if (hdr.size < VITRIN_HEADER_LEN) {
			/* conventions 5.2 `oversized`: a size below the header minimum
			 * cannot be advanced past, so the stream is unrecoverable. */
			return wire_fail(w, "frame size below the 8-byte header minimum");
		}
		if (hdr.fd_count != 0) {
			return wire_fail(w, "core -> shim event declared a file descriptor");
		}
		if (w->rx_len - pos < hdr.size) {
			break; /* frame still arriving */
		}
		bool ok = w->handler(w->handler_data, w->rx + pos, hdr.size);
		pos += hdr.size;
		if (!ok) {
			return false;
		}
	}
	memmove(w->rx, w->rx + pos, w->rx_len - pos);
	w->rx_len -= pos;
	return true;
}

bool vitrin_wire_recv_sync(struct vitrin_wire *w, int timeout_ms,
		uint8_t *out, size_t out_cap, size_t *out_len) {
	if (!vitrin_wire_alive(w)) {
		return false;
	}
	struct timespec start;
	clock_gettime(CLOCK_MONOTONIC, &start);

	for (;;) {
		/* Extract a frame if one is already buffered. */
		if (w->rx_len >= VITRIN_HEADER_LEN) {
			vitrin_frame_header_t hdr;
			if (vitrin_frame_header_decode(w->rx, w->rx_len, &hdr) != VITRIN_DECODE_OK) {
				return wire_fail(w, "undecodable frame header");
			}
			if (hdr.size < VITRIN_HEADER_LEN) {
				return wire_fail(w, "frame size below the 8-byte header minimum");
			}
			if (hdr.fd_count != 0) {
				return wire_fail(w, "core -> shim event declared a file descriptor");
			}
			if (w->rx_len >= hdr.size) {
				if (hdr.size > out_cap) {
					return wire_fail(w, "frame larger than the caller's buffer");
				}
				memcpy(out, w->rx, hdr.size);
				*out_len = hdr.size;
				memmove(w->rx, w->rx + hdr.size, w->rx_len - hdr.size);
				w->rx_len -= hdr.size;
				return true;
			}
		}

		struct timespec now;
		clock_gettime(CLOCK_MONOTONIC, &now);
		long elapsed_ms = (now.tv_sec - start.tv_sec) * 1000 +
			(now.tv_nsec - start.tv_nsec) / 1000000;
		long remaining = (long)timeout_ms - elapsed_ms;
		if (remaining <= 0) {
			wlr_log(WLR_ERROR,
				"timed out after %dms waiting for the core's first message",
				timeout_ms);
			return false;
		}

		struct pollfd pfd = {.fd = w->fd, .events = POLLIN};
		int pr = poll(&pfd, 1, (int)remaining);
		if (pr < 0) {
			if (errno == EINTR) {
				continue;
			}
			return wire_fail(w, "poll failed");
		}
		if (pr == 0) {
			continue; /* re-checks the deadline above */
		}
		if (!wire_fill(w)) {
			return false;
		}
		if (w->eof) {
			wlr_log(WLR_ERROR, "the core closed the connection during bring-up");
			return false;
		}
	}
}

/* ---- event-loop integration ----------------------------------------- */

/* The connection is over. Drop the event source FIRST: a hangup stays
 * readable forever, so a source left armed would re-fire every loop
 * iteration and spin the shim at 100% instead of shutting it down. Then tell
 * the layer above, exactly once. */
static void wire_finished(struct vitrin_wire *w) {
	if (w->source != NULL) {
		wl_event_source_remove(w->source);
		w->source = NULL;
	}
	if (w->on_closed != NULL) {
		vitrin_wire_closed_t cb = w->on_closed;
		w->on_closed = NULL;
		cb(w->handler_data);
	}
}

static int wire_ready(int fd, uint32_t mask, void *data) {
	(void)fd;
	struct vitrin_wire *w = data;

	if ((mask & WL_EVENT_WRITABLE) != 0) {
		if (wire_flush(w)) {
			wire_update_mask(w);
		}
	}
	/* A hangup is handled by reading, not by believing the mask: it can
	 * arrive alongside the core's last bytes, and those bytes may be the
	 * `frame_done` the app is waiting on. `wire_fill` reports the real EOF
	 * when the read returns 0. */
	if (vitrin_wire_alive(w) &&
			(mask & (WL_EVENT_READABLE | WL_EVENT_HANGUP | WL_EVENT_ERROR)) != 0) {
		if (wire_fill(w) && wire_dispatch(w)) {
			/* Everything this wakeup delivered has now been handed on, so
			 * this is the batch boundary (see vitrin_wire_drained_t). It runs
			 * before the mask update because a drain handler may itself
			 * queue sends. */
			if (w->on_drained != NULL) {
				w->on_drained(w->handler_data);
			}
			/* A handler may have queued sends. */
			wire_update_mask(w);
		}
	}
	if (!vitrin_wire_alive(w)) {
		wire_finished(w);
	}
	return 0;
}

bool vitrin_wire_arm(struct vitrin_wire *w, struct wl_event_loop *loop,
		vitrin_wire_handler_t handler, vitrin_wire_drained_t on_drained,
		vitrin_wire_closed_t on_closed, void *data) {
	w->handler = handler;
	w->on_drained = on_drained;
	w->on_closed = on_closed;
	w->handler_data = data;
	w->source = wl_event_loop_add_fd(loop, w->fd, WL_EVENT_READABLE, wire_ready, w);
	if (w->source == NULL) {
		return wire_fail(w, "cannot add the core connection to the event loop");
	}
	wire_update_mask(w);
	return true;
}

void vitrin_wire_finish(struct vitrin_wire *w) {
	if (w->source != NULL) {
		wl_event_source_remove(w->source);
		w->source = NULL;
	}
	while (w->queue_len > 0) {
		struct vitrin_wire_slot *slot = &w->queue[w->queue_head];
		if (slot->fd >= 0) {
			close(slot->fd);
			slot->fd = -1;
		}
		w->queue_head = (w->queue_head + 1) % VITRIN_WIRE_QUEUE_SLOTS;
		w->queue_len--;
	}
	if (w->fd >= 0) {
		close(w->fd);
		w->fd = -1;
	}
}
