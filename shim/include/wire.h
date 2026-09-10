/* wire.h -- the framed transport between the shim and the trusted core
 * (P1.6.2).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * This is the C counterpart of `crates/vitrin-ipc` and, like it, it does
 * FRAMING ONLY: it neither decodes argument payloads nor knows what an
 * object id means. The generated header (include/vitrin-protocol.h) encodes
 * and decodes message BODIES and performs no I/O; this file performs the
 * I/O and nothing else. Together they cover the whole wire.
 *
 * WHAT IS ON THE OTHER END. Exactly one thing, always: the trusted core,
 * over the socketpair it created before forking us, whose shim-side end it
 * placed at VITRIN_CORE_FD. Holding that descriptor *is* being this realm's
 * shim -- there is no handshake and no credential to present
 * (crates/vitrin-core/src/spawn.rs; conventions 1.2). Two consequences
 * shape this file:
 *
 *   - The peer is the TCB, so this layer does not defend against it the way
 *     the core defends against us. It still validates the framing it parses,
 *     because a core bug must surface as a named failure here rather than as
 *     a silently mis-decoded message.
 *   - Our own outgoing framing must be exactly right, because the core's
 *     answer to a malformed frame is to log and close with NO error event on
 *     the wire (conventions 5.2). A framing bug here looks like an
 *     unexplained disconnect, so every send is checked before it reaches the
 *     kernel.
 *
 * THE ASYMMETRY THAT SIMPLIFIED THE RECEIVE PATH, AND THE MESSAGE THAT ENDED
 * IT. For this transport's first two phases every fd it carried travelled
 * shim -> core (the `attach` buffer). No version-1 core -> shim event carries
 * one: `configure`, `frame_done`, `buffer_done` and all five
 * `vitrin_shim_seat` events are fd-less. So the receive side needed no
 * pending-fd queue and no positional-matching machinery, unlike vitrin-ipc's
 * `Connection`, which must serve both directions.
 *
 * P2.6.5 (issue #189) PUT THE FIRST FD-BEARING core -> shim EVENT ON THE
 * WIRE: `vitrin_shim_session.designation`, which delivers one designated file
 * or directory descriptor into the realm. This transport now RECEIVES it, and
 * the machinery below is vitrin-ipc's transcribed rather than a second design:
 * fds are harvested off the ancillary queue tagged with the byte-stream SPAN
 * of the recvmsg that delivered them, claimed by the frame whose header
 * declares `fd_count == 1`, and every disagreement between the two is fatal.
 * See `struct vitrin_wire_pending_fd` for why a span and not an offset.
 *
 * OWNERSHIP, MADE STRUCTURAL BY THE CLAIM PROTOCOL. `designation`'s IDL says
 * ownership transfers to the receiver, which MUST close the descriptor: a
 * shim that cannot relay one closes it, and a leaked one pins a file open for
 * the life of the realm. So the handler is handed `int *fd` and CLAIMS the
 * descriptor by writing -1 back; whatever it leaves, this transport closes as
 * soon as the handler returns. A handler that forgets the argument entirely
 * therefore cannot leak, and neither can the fatal paths, which close the
 * pending queue as they poison the connection. Relaying the descriptor onward
 * to the app, over the realm's own designation socket, is P2.6.7's (issue
 * #191); until that lands the shim logs the arrival and lets this transport
 * close the fd, so the app never sees it.
 *
 * BLOCKING MODE. The descriptor is non-blocking from the moment it is
 * adopted, and stays that way. The one synchronous read the protocol
 * mandates (the `configure` that precedes everything -- conventions 7.2) is
 * a poll()-bounded loop over the same non-blocking primitives, so there is a
 * single send path and a single receive path in this file rather than a
 * blocking and a non-blocking variant of each.
 */
#ifndef VITRIN_WIRE_H
#define VITRIN_WIRE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <wayland-server-core.h>

/* The descriptor the core places the shim's end of the identity socketpair
 * on. A compile-time constant on both sides, announced by nothing -- no
 * environment variable, no argv element (see spawn.rs for why both were
 * rejected). Keep in sync with `spawn::SHIM_CORE_FD`. */
#define VITRIN_CORE_FD 3

/* The largest frame the wire format can express: the header's `size` is a
 * u16, so this bound cannot be exceeded by any peer, well-behaved or not
 * (conventions 2.1). Sizing the reassembly buffer at this bound is what
 * makes the receive path structurally incapable of overflowing, with no
 * dynamic allocation in a path that runs per frame. */
#define VITRIN_WIRE_MAX_FRAME 65535u

/* The largest frame the SHIM ever sends **on the per-frame path**. Every
 * shim -> core request on that path is fixed-size: attach is 8 + 6*4 = 32
 * bytes (its fd rides out of band), damage 24, commit 8, create_surface and
 * get_seat 12. 64 rounds that up with room for a future appended argument,
 * and it is what lets a parked frame live inline in the queue below instead
 * of in a heap allocation -- the frame path must not call the allocator. A
 * frame that does not fit is caught as local misuse before it reaches the
 * kernel.
 *
 * `vitrin_shim_session.selection` (WS-E.2.1) is the one exception and is why
 * this constant is no longer the send-side ceiling: its `data` argument is
 * bounded at 61440 bytes by the IDL, so the whole frame can approach the
 * format's own 65535. It is NOT on the frame path -- it is answered once per
 * human keypress -- so it gets the single dedicated slot below rather than
 * 64 queue slots sized for it, which would cost 4 MiB of the shim's memory
 * to carry a clipboard. */
#define VITRIN_WIRE_SLOT_BYTES 64u

/* Depth of the parked-frame queue. Backpressure toward the core is really
 * bounded one layer up -- the frame forwarder admits one frame at a time and
 * its pixels travel out of band in a memfd, so what flows through this queue
 * is a handful of ~30-byte control frames per presented frame. This queue is
 * therefore a last-resort guard, not the mechanism; overflowing it means the
 * core stopped reading entirely, which is fatal (the same conclusion
 * vitrin-ipc reaches for its own send-queue cap). */
#define VITRIN_WIRE_QUEUE_SLOTS 64u

/* The most received-but-unclaimed descriptors this transport holds at once.
 *
 * One is the protocol's own bound: `designation` is the only fd-bearing
 * core -> shim event and the core sends one per completed designation. But the
 * kernel delivers ancillary data per sendmsg, not per frame, so a second fd
 * can legitimately be in flight while the frame that declares the first is
 * still being reassembled. Four is that slack. Exceeding it means descriptors
 * arrived that no frame declared, which is a violation to die on rather than a
 * queue to grow -- the same conclusion vitrin-ipc's MAX_UNCLAIMED_FDS reaches.
 *
 * The recvmsg ancillary buffer is sized for exactly this many, so MORE fds in
 * ONE sendmsg surface as MSG_CTRUNC -- a named fatal -- instead of as a silent
 * drop of descriptors the kernel already installed somewhere. */
#define VITRIN_WIRE_MAX_UNCLAIMED_FDS 4u

/* One received SCM_RIGHTS descriptor awaiting the frame that declares it.
 *
 * WHY A SPAN AND NOT AN OFFSET. The kernel delivers an fd batch with the
 * recvmsg that first consumes bytes of the sendmsg that carried it, and never
 * delivers two batches in one recvmsg -- but it DOES glue preceding fd-less
 * bytes from the same sender onto the head of that recvmsg
 * (`unix_stream_read_generic` merges consecutive skbs whose credentials match,
 * and the fd array is not part of that comparison). So a receiver can observe
 * the byte-stream span of the delivering recvmsg and cannot observe the offset
 * the sender attached the fd at. Positional matching is therefore enforced
 * against the span, which is as tight as the kernel's delivery semantics
 * allow. Same reasoning and same fields as vitrin-ipc's `PendingFd`; the two
 * ends of this wire must agree about this or one of them mis-matches. */
struct vitrin_wire_pending_fd {
	/* Stream offset of the first byte of the recvmsg that delivered this
	 * fd. The true attach offset is >= this. */
	uint64_t span_start;
	/* One past the last byte of that recvmsg. Because a recvmsg ends at, or
	 * inside, the fd-bearing segment, the true attach offset is < this. */
	uint64_t span_end;
	/* Owned here from the moment `recvmsg` returned, close-on-exec from
	 * birth (MSG_CMSG_CLOEXEC). -1 once claimed. */
	int fd;
};

/* One outgoing frame the kernel would not take yet. */
struct vitrin_wire_slot {
	uint8_t bytes[VITRIN_WIRE_SLOT_BYTES];
	size_t len;
	/* Bytes of `bytes` already written by an earlier partial flush. */
	size_t offset;
	/* The fd that must ride this frame's FIRST sendmsg (positional
	 * matching, conventions 2.2). Owned here (a dup of the caller's, whose
	 * lifetime cannot be assumed to outlive the call) and -1 once the
	 * kernel has taken it -- any successful sendmsg carries the whole
	 * ancillary payload, so a resumed write must never re-send it. */
	int fd;
};

/* Called with one complete, framed message, and with `*fd` set to the
 * descriptor that rode alongside it iff its header declared one -- -1
 * otherwise, which is every message on this wire but `designation`.
 *
 * OWNERSHIP. The handler CLAIMS the descriptor by writing -1 back through
 * `fd`; anything it leaves behind the transport closes the instant this
 * returns, on every path including a `false` return. That is the IDL's "a
 * shim that cannot relay a descriptor closes it" made structural rather than
 * conventional: a handler that ignores the argument entirely is correct, not
 * leaky, and no future handler can leak by forgetting.
 *
 * Return false to declare the connection dead (the transport then refuses
 * every later operation). */
typedef bool (*vitrin_wire_handler_t)(void *data, const uint8_t *frame, size_t len, int *fd);

/* Called once per event-loop wakeup, after every complete frame that wakeup
 * delivered has been handed to the handler above.
 *
 * This exists because a batch boundary is real protocol information, not a
 * scheduling artefact: the core writes all the events it routed from one
 * cause in one go, so frames that arrive together are frames that belong
 * together. Input replay needs exactly that to place `wl_pointer.frame`
 * correctly (seat.h, "pointer batching"). It fires whether or not the batch
 * contained anything the consumer cares about -- deciding that is the
 * consumer's job, and it keeps this layer ignorant of message meaning, which
 * is the whole point of the split with the generated header. */
typedef void (*vitrin_wire_drained_t)(void *data);

/* Called once, from the event loop, when the connection is finished for any
 * reason -- the core hung up (the first rung of its shutdown ladder), a
 * transport error, or a framing violation. The event source is already gone
 * by then, so the shim cannot spin on a hangup it is not going to act on. */
typedef void (*vitrin_wire_closed_t)(void *data);

struct vitrin_wire {
	int fd; /* VITRIN_CORE_FD, or -1 when no upstream link exists */
	/* Sticky: set on the first fatal transport or framing failure and never
	 * cleared, so a torn stream can never be appended to and every later
	 * call fails identically (vitrin-ipc's `poisoned`/`send_poisoned`). */
	bool failed;
	const char *fail_reason;
	/* The core hung up. Not a failure: P1.5.3's orderly shutdown ladder
	 * begins with the core closing its end. */
	bool eof;

	/* Reassembly: bytes received but not yet handed on as whole frames. */
	uint8_t rx[VITRIN_WIRE_MAX_FRAME];
	size_t rx_len;

	/* Total bytes already taken out of `rx` as completed frames -- i.e. the
	 * byte-stream offset of `rx[0]`. This is the coordinate every span in
	 * `pending` below is expressed in, and the only reason it is tracked. */
	uint64_t consumed;

	/* Received fds not yet claimed by a completed frame, oldest first, which
	 * is frame order (positional matching, conventions 2.2). Closed both by
	 * the fatal path and by `vitrin_wire_finish`, so no way this connection
	 * can end leaves one open. */
	struct vitrin_wire_pending_fd pending[VITRIN_WIRE_MAX_UNCLAIMED_FDS];
	size_t pending_head;
	size_t pending_len;

	/* Parked frames, oldest first (FIFO preserves wire order). */
	struct vitrin_wire_slot queue[VITRIN_WIRE_QUEUE_SLOTS];
	size_t queue_head;
	size_t queue_len;

	/* The single OVERSIZED parked frame (WS-E.2.1): a `selection` answer the
	 * kernel would not take whole. Drained BEFORE `queue`, and only ever
	 * accepted when `queue` is empty, so "big first, then the queue" is the
	 * wire order rather than a reordering -- anything parked afterwards was
	 * handed to `vitrin_wire_send` afterwards.
	 *
	 * Exactly one, and a second while this is pending is fatal, for the same
	 * reason a full `queue` is: it means the core stopped reading. Reachable
	 * only from the clipboard path, which the core drives one question at a
	 * time. No fd field: every oversized message in v0 has `fd_count` 0, and
	 * `vitrin_wire_send` refuses the combination rather than trusting that. */
	uint8_t big[VITRIN_WIRE_MAX_FRAME];
	size_t big_len;
	size_t big_offset;

	struct wl_event_source *source;
	vitrin_wire_handler_t handler;
	vitrin_wire_drained_t on_drained;
	vitrin_wire_closed_t on_closed;
	void *handler_data;
};

/* Adopt VITRIN_CORE_FD as this shim's core connection.
 *
 * Verifies the descriptor is an open AF_UNIX stream socket, re-arms
 * FD_CLOEXEC on it (it arrived with the flag CLEARED -- that is how it
 * survived execve -- so without this line every process the shim later
 * spawns inherits a live core connection and the confinement is gone), and
 * switches it to non-blocking. Returns false if the descriptor is absent or
 * is not a socket, which means this process was not spawned by the core. */
bool vitrin_wire_adopt(struct vitrin_wire *w);

/* True once a connection exists and has not failed or hung up. */
bool vitrin_wire_alive(const struct vitrin_wire *w);

/* The largest frame `vitrin_wire_send` accepts: the wire format's own
 * ceiling, since `vitrin_shim_session.selection` can approach it. */
#define VITRIN_WIRE_MAX_SEND VITRIN_WIRE_MAX_FRAME

/* Send one complete frame, with `fd` (or -1) riding its first sendmsg as
 * SCM_RIGHTS ancillary data, so the core's positional fd matching holds by
 * construction. `frame` must be exactly one encoder-produced frame; the
 * caller keeps ownership of `fd` and may close it as soon as this returns.
 * Whatever the kernel will not take is parked and flushed on write
 * readiness. Returns false only on a fatal condition. */
bool vitrin_wire_send(struct vitrin_wire *w, const uint8_t *frame, size_t len, int fd);

/* Read exactly one frame, blocking up to `timeout_ms`, and copy it into
 * `out`. This exists for the ONE synchronous read the protocol mandates:
 * `configure`, the core's guaranteed-first message, read before the shim
 * serves its private Wayland socket (conventions 7.2). Returns false on
 * timeout, EOF, or a fatal condition.
 *
 * There is no fd out-parameter, and a first frame that declares one is fatal:
 * the only frame this ever reads is `configure`, which is fd-less, and the
 * shim has nowhere to put a descriptor before it is armed. Descriptors
 * belonging to LATER frames in the same batch are unaffected -- they stay in
 * the pending queue and are claimed once `vitrin_wire_arm` starts dispatching. */
bool vitrin_wire_recv_sync(struct vitrin_wire *w, int timeout_ms,
	uint8_t *out, size_t out_cap, size_t *out_len);

/* Hand the connection to the Wayland event loop: from here the core link is
 * pumped by the same loop that serves the app, so a message from the core
 * and a request from the app are dispatched by one thread with no locking
 * and no ordering questions between them.
 *
 * Frames the core batched behind `configure` are dispatched HERE, before this
 * returns, so `handler` can run before the caller regains control. They have to
 * be: `vitrin_wire_recv_sync` leaves them in the reassembly buffer while the
 * socket -- which is what the loop watches -- is empty, so nothing else would
 * wake for them until the core's next message. */
bool vitrin_wire_arm(struct vitrin_wire *w, struct wl_event_loop *loop,
	vitrin_wire_handler_t handler, vitrin_wire_drained_t on_drained,
	vitrin_wire_closed_t on_closed, void *data);

/* Remove the event source and close the descriptor (idempotent). */
void vitrin_wire_finish(struct vitrin_wire *w);

#endif /* VITRIN_WIRE_H */
