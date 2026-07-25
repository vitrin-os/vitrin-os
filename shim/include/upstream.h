/* upstream.h -- the shim's half of the shim-facing protocol (P1.6.2):
 * `vitrin_shim_session` + `vitrin_shim_surface` over the core socketpair.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * wire.c moves bytes; this layer decides which bytes. It is the C
 * counterpart of `crates/vitrin-mock-shim` (the compliant reference shim
 * the core's tests are written against) and the peer of
 * `crates/vitrin-core/src/shim.rs` (the server side, whose module docs pin
 * every rule enforced against us).
 *
 * WHAT HAPPENS, IN WHAT ORDER (flow G of docs/protocol/09, flow g of 10):
 *
 *   1. adopt fd 3 and re-arm FD_CLOEXEC on it                 [wire.c]
 *   2. ONE synchronous read: the core's `configure`, its guaranteed-first
 *      message, carrying the realm identity and the realm-view geometry
 *   3. `create_surface(2)` -- the buffer path
 *   4. `get_seat(3)`       -- the input path (see the note below)
 *   5. ... only now does the shim bind its private Wayland socket ...
 *   6. per composed frame: `attach` -> `damage`* -> `commit`
 *   7. per core answer: `buffer_done` recycles a buffer, `frame_done` is
 *      relayed to the app as its `wl_surface.frame` callbacks
 *
 * Steps 2-4 are synchronous and complete before the app can connect, which
 * is exactly why the protocol needs no deferred-configure state machine
 * (conventions 7.2).
 *
 * WHY THE SEAT IS MINTED HERE, BEFORE THE CODE THAT USES IT EXISTS.
 * Input replay is P1.6.3, so every seat event this version receives is
 * logged and dropped. Minting the object anyway is deliberate:
 *
 *   - The IDL asks for it: "a well-behaved shim calls get_seat during
 *     startup, immediately after receiving configure", because input the
 *     core routes before the seat exists "has no destination and is
 *     dropped". Minting late converts every early keystroke into a silent
 *     loss.
 *   - Object ids are strictly increasing and never reused (conventions
 *     3.1). Minting the seat now freezes the id layout at surface=2,
 *     seat=3; minting it in P1.6.3 would renumber it behind whatever else
 *     had been allocated by then, and every log, trace and test expectation
 *     recorded against the id would move with it.
 *
 * The cost is one object and one dropped-event log line per input event
 * until P1.6.3 fills in the replay.
 *
 * TWO DECISIONS THIS TASK SETTLES (issue #34 names them as open):
 *
 *   - DAMAGE GRANULARITY: per rectangle, verbatim from the compositor's own
 *     damage region, never pre-coalesced to the full surface. The IDL asks
 *     for exactly this ("damage travels per-rectangle on the wire ... so
 *     damage-only updates are verifiable in the core log"), and it is what
 *     makes "a small repaint did not forward the full surface" a checkable
 *     fact rather than a claim. Rectangle count is capped at
 *     VITRIN_MAX_DAMAGE_RECTS and the overflow folds into a bounding box --
 *     the same over-approximation the core itself applies past its own cap,
 *     so a pathological region costs O(1) wire traffic instead of O(rects).
 *
 *   - MEMFD REUSE, from a small fixed pool, never fresh-per-frame. A fresh
 *     memfd per frame means memfd_create + ftruncate + a fault on every
 *     page of a multi-megabyte buffer, 60 times a second, for bytes the
 *     core copies out and discards immediately. Reuse turns that into a
 *     memcpy into an already-faulted mapping. The invariant that makes it
 *     safe is the protocol's own: a buffer is written only while the shim
 *     owns it, and ownership returns exactly once per attach, at
 *     `buffer_done` -- so a slot re-enters the pool only when the core has
 *     said it is done with it (see vitrin_buffer below).
 */
#ifndef VITRIN_UPSTREAM_H
#define VITRIN_UPSTREAM_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <pixman.h>

#include "vitrin-protocol.h"
#include "wire.h"

struct vitrin_shim;
struct wlr_buffer;

/* Object ids this shim allocates. Client-allocated ids live in
 * [2, 0xfeffffff] and must be strictly increasing and never reused
 * (conventions 3.1); id 1 is the implicit `vitrin_shim_session` bootstrap
 * object, which no message creates. Version 1 needs exactly two, both
 * minted at startup, so the whole "allocator" is these two constants. */
#define VITRIN_SESSION_ID 1u
#define VITRIN_SURFACE_ID 2u
#define VITRIN_SEAT_ID 3u

/* Buffers in the recycle pool.
 *
 * Two, not one: under version-1 shm copy-in `buffer_done` arrives promptly
 * (the core copies at commit), so one slot would almost always be free --
 * but `buffer_done` and `frame_done` are deliberately distinct facts, and
 * under the dmabuf passthrough this pool is designed to survive into
 * (P1.3.5) `released` is deferred until the NEXT commit replaces the
 * retained buffer. With one slot that is a deadlock: the shim cannot build
 * the replacement it is being told to send. Two slots make the replacement
 * always constructible, and bound the shim's buffer memory at 2 frames
 * regardless of how slow the core is.
 *
 * Two, not more: nothing is gained. The forwarder admits one frame at a
 * time (see vitrin_upstream_can_forward), so a third slot could never be
 * in flight -- it would only let the shim run further ahead of a core that
 * is already behind, which is precisely the unbounded buffering the
 * acceptance criteria forbid. */
#define VITRIN_BUFFER_POOL_SLOTS 2

/* Cap on damage rectangles forwarded per commit; the remainder folds into a
 * bounding box. Chosen below the core's own MAX_DAMAGE_RECTS (64) so a
 * legitimate frame is never the thing that trips the core's folding path. */
#define VITRIN_MAX_DAMAGE_RECTS 32

/* `vitrin_shim_session.configure`'s realm argument bound (conventions 2.3). */
#define VITRIN_REALM_MAX_BYTES 64

/* One pooled shm buffer: a memfd, its mapping, and the cookie naming it
 * upstream.
 *
 * OWNERSHIP, which is the whole safety argument for reuse: `buffer_id`
 * doubles as the slot's state. Zero means the shim owns the buffer and may
 * write it. Non-zero means it is in flight -- the core holds an SCM_RIGHTS
 * duplicate of the fd and may still be reading it -- and the slot is
 * untouchable until `buffer_done` echoes that exact cookie back. Nothing
 * else may clear it, so "wrote into a buffer the core was reading" is
 * unrepresentable rather than merely avoided. */
struct vitrin_buffer {
	int fd;       /* memfd, -1 when the slot has no allocation yet */
	void *data;   /* MAP_SHARED mapping of the whole memfd, or NULL */
	size_t size;  /* bytes: stride * height */
	uint32_t width, height, stride;
	vitrin_view_format_t format;
	uint32_t buffer_id; /* 0 = free/owned by us; else in flight */
};

struct vitrin_upstream {
	struct vitrin_wire wire;
	/* False in standalone mode (--no-upstream): there is no core, so
	 * nothing is forwarded and the shim paces its app locally. Every entry
	 * point below is a no-op in that mode. */
	bool active;

	/* From `configure`. `realm` is informational: the shim can never assert
	 * its identity (there is no wire path to), so this exists to label logs
	 * -- exactly what the IDL says it is for. */
	char realm[VITRIN_REALM_MAX_BYTES + 1];
	uint32_t view_width, view_height;

	uint32_t next_buffer_id;
	struct vitrin_buffer pool[VITRIN_BUFFER_POOL_SLOTS];

	/* A commit is awaiting its `frame_done`. This single bit is the frame
	 * path's admission control: while it is set the shim composites
	 * nothing, so an app that renders faster than the core presents is
	 * throttled at the source rather than queued. */
	bool frame_inflight;
	/* An output frame tick was skipped because the core was busy. The
	 * scene's damage ring still holds every pixel involved, so resuming is
	 * just asking the output for another tick. */
	bool composite_deferred;

	/* Instrumentation. The acceptance harness asserts on the core's view of
	 * the wire, not on these, but they make a live shim's log readable and
	 * they are how a stall is diagnosed. */
	uint64_t frames_forwarded;
	uint64_t frames_deferred;
	uint64_t damage_rects_sent;
	uint64_t frame_dones;
	uint64_t buffer_dones;
	uint64_t unpaced_frame_dones;

	/* Composed frames that never reached the core -- see the retry path in
	 * output.c. `frames_lost` is the lifetime total (0 on a healthy run);
	 * `consecutive_frames_lost` is the retry counter, cleared by every frame
	 * that does make it, and is what separates a one-off from a standing
	 * fault the realm should die of rather than spin on. */
	uint64_t frames_lost;
	uint32_t consecutive_frames_lost;
};

/* Put the upstream state in its "this process owns nothing" shape: no
 * connection, no allocated buffer, every descriptor -1.
 *
 * Called unconditionally at startup, BEFORE the standalone/upstream branch,
 * because teardown is unconditional too: `struct vitrin_shim s = {0}` leaves
 * every pool descriptor at 0, and 0 is stdin -- so a teardown that never ran
 * vitrin_upstream_open would close a descriptor this process never opened.
 * Initialising here rather than in vitrin_upstream_open is what makes the
 * invariant independent of which startup branch was taken. */
void vitrin_upstream_init(struct vitrin_shim *s);

/* Phase A0 -- claim fd 3, read `configure`, mint the surface and the seat.
 * Runs before ANY wlroots bring-up, because `configure` carries the
 * realm-view geometry the headless output and the app's window are sized
 * from. Returns false if there is no usable core connection. */
bool vitrin_upstream_open(struct vitrin_shim *s);

/* Phase A1 -- hand the connection to the event loop. Runs after the loop
 * exists and before the private Wayland socket is bound, so no app request
 * can be served ahead of the core's bring-up. */
bool vitrin_upstream_start(struct vitrin_shim *s);

/* May a composed frame be forwarded right now? False while a commit awaits
 * its `frame_done` or while every pool slot is in flight. The output frame
 * handler asks BEFORE compositing: work that cannot be forwarded is work
 * not worth doing, and skipping it loses nothing (the damage stays in the
 * scene's ring). */
bool vitrin_upstream_can_forward(const struct vitrin_shim *s);

/* Forward one composed frame: copy `buf`'s pixels into a pooled memfd, then
 * `attach` -> `damage` per rectangle of `damage` (full surface when NULL)
 * -> `commit`.
 *
 * Returns false when the frame did NOT reach the core, and the caller is
 * then obliged to recover it -- see output.c. This is a return value rather
 * than a void with a log line because every failure here happens AFTER the
 * composite: the scene's damage has been spent on a frame that no longer
 * exists anywhere, and the app is waiting on a `wl_surface.frame` callback
 * that only a `frame_done` can produce. Silently dropping such a frame is
 * not one lost frame, it is a permanently frozen realm.
 *
 * True means either "forwarded" or "there was nothing to forward" (the
 * standalone mode, which composites for nobody); both leave nothing to
 * recover. */
bool vitrin_upstream_forward(struct vitrin_shim *s, struct wlr_buffer *buf,
	const pixman_region32_t *damage);

/* Fire the app's `wl_surface.frame` callbacks for a tick that produced no
 * new content upstream. See the call site in output.c for why this exists
 * and why it cannot reintroduce free-running. */
void vitrin_upstream_pace_locally(struct vitrin_shim *s);

/* Release the connection, the pool's memfds, and their mappings. */
void vitrin_upstream_finish(struct vitrin_shim *s);

#endif /* VITRIN_UPSTREAM_H */
