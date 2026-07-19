/* wire.h -- the framed transport between the shim and the trusted core
 * (P1.6.2).
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
 * THE ASYMMETRY THAT SIMPLIFIES THE RECEIVE PATH. Every fd on a shim
 * connection travels shim -> core (the `attach` buffer). No version-1
 * core -> shim event carries one: `configure`, `frame_done`, `buffer_done`
 * and all five `vitrin_shim_seat` events are fd-less. So this transport
 * needs SCM_RIGHTS on the send side only; on the receive side an arriving fd
 * is a violation, closed immediately and then fatal. That is why there is no
 * pending-fd queue and no positional-matching machinery here, unlike
 * vitrin-ipc's `Connection`, which must serve both directions.
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

/* The largest frame the SHIM ever sends. Every shim -> core request is
 * fixed-size: attach is 8 + 6*4 = 32 bytes (its fd rides out of band),
 * damage 24, commit 8, create_surface and get_seat 12. 64 rounds that up
 * with room for a future appended argument, and it is what lets a parked
 * frame live inline in the queue below instead of in a heap allocation --
 * the frame path must not call the allocator. A frame that does not fit is
 * caught as local misuse before it reaches the kernel. */
#define VITRIN_WIRE_SLOT_BYTES 64u

/* Depth of the parked-frame queue. Backpressure toward the core is really
 * bounded one layer up -- the frame forwarder admits one frame at a time and
 * its pixels travel out of band in a memfd, so what flows through this queue
 * is a handful of ~30-byte control frames per presented frame. This queue is
 * therefore a last-resort guard, not the mechanism; overflowing it means the
 * core stopped reading entirely, which is fatal (the same conclusion
 * vitrin-ipc reaches for its own send-queue cap). */
#define VITRIN_WIRE_QUEUE_SLOTS 64u

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

/* Called with one complete, framed message. Return false to declare the
 * connection dead (the transport then refuses every later operation). */
typedef bool (*vitrin_wire_handler_t)(void *data, const uint8_t *frame, size_t len);

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

	/* Parked frames, oldest first (FIFO preserves wire order). */
	struct vitrin_wire_slot queue[VITRIN_WIRE_QUEUE_SLOTS];
	size_t queue_head;
	size_t queue_len;

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
 * timeout, EOF, or a fatal condition. */
bool vitrin_wire_recv_sync(struct vitrin_wire *w, int timeout_ms,
	uint8_t *out, size_t out_cap, size_t *out_len);

/* Hand the connection to the Wayland event loop: from here the core link is
 * pumped by the same loop that serves the app, so a message from the core
 * and a request from the app are dispatched by one thread with no locking
 * and no ordering questions between them. */
bool vitrin_wire_arm(struct vitrin_wire *w, struct wl_event_loop *loop,
	vitrin_wire_handler_t handler, vitrin_wire_drained_t on_drained,
	vitrin_wire_closed_t on_closed, void *data);

/* Remove the event source and close the descriptor (idempotent). */
void vitrin_wire_finish(struct vitrin_wire *w);

#endif /* VITRIN_WIRE_H */
