/* upstream.c -- session bring-up, the frame forwarder, and the core's
 * answers. See upstream.h for the flow, the seat-minting decision, and the
 * two settled decisions (damage granularity, memfd reuse).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * The three things that happen here, in the order a frame meets them:
 *
 *   forward   composed pixels -> a pooled memfd -> attach/damage/commit
 *   answer    buffer_done recycles the slot; frame_done paces the app
 *   resume    a tick skipped under backpressure is asked for again
 */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <drm_fourcc.h>

#include <wayland-server-core.h>

#include <wlr/render/wlr_renderer.h>
#include <wlr/render/wlr_texture.h>
#include <wlr/types/wlr_buffer.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/util/log.h>

#include "seat.h"
#include "server.h"
#include "upstream.h"
#include "vitrin-protocol.h"
#include "wire.h"

/* The protocol's format enum is the DRM fourcc namespace itself -- the same
 * vocabulary the agent-facing capture path uses, so the shim's supply side
 * and the agent's observation side can never diverge on pixel encoding
 * (docs/protocol/10, "format (shared, defined elsewhere)"). Asserted rather
 * than assumed: this file maps wlroots' fourccs onto it by value. */
_Static_assert((uint32_t)VITRIN_VIEW_FORMAT_XRGB8888 == DRM_FORMAT_XRGB8888,
	"vitrin_view.format.xrgb8888 must be the DRM fourcc");
_Static_assert((uint32_t)VITRIN_VIEW_FORMAT_ARGB8888 == DRM_FORMAT_ARGB8888,
	"vitrin_view.format.argb8888 must be the DRM fourcc");

/* Bytes per pixel of both version-1 formats. */
#define VITRIN_BPP 4u

/* The core's own renderer limits (crates/vitrin-core/src/shim.rs:
 * MAX_SURFACE_DIM / MAX_SURFACE_BYTES). A buffer past them is answered
 * `buffer_done(too_large)` -- recoverable, not a kill -- but a shim that
 * knows the limits should not spend a multi-megabyte copy to be told so.
 * Mirrored here so the refusal happens before the work, not after. */
#define VITRIN_MAX_SURFACE_DIM 16384u
#define VITRIN_MAX_SURFACE_BYTES (128u * 1024u * 1024u)

/* Deadline for the one synchronous read of `configure`. This is NOT protocol
 * policy -- the protocol guarantees the message arrives first, unprompted --
 * it is a liveness guard: a shim blocked forever on a core that never spoke
 * would hang its realm invisibly, whereas exiting turns the failure into a
 * SIGCHLD the core's lifecycle layer (P1.5.3) already knows how to read. */
#define VITRIN_CONFIGURE_TIMEOUT_MS 10000

/* Encode buffer for one outgoing frame. Every shim -> core request is at
 * most 32 bytes (wire.h); 64 is the same headroom the wire's parked-frame
 * slot uses. */
#define VITRIN_ENCODE_BUF 64

/* Terminate the shim after an upstream failure. The core connection IS the
 * shim's reason to exist: with it gone there is nobody to show the app to,
 * so the honest response is a clean exit -- which the core's lifecycle
 * layer observes as ordinary shim death -- not carrying on rendering into
 * nothing. */
static void upstream_die(struct vitrin_shim *s, const char *reason) {
	wlr_log(WLR_ERROR, "upstream link lost (%s); shutting down", reason);
	if (s->display != NULL) {
		wl_display_terminate(s->display);
	}
}

/* ---- the buffer pool ------------------------------------------------ */

static void buffer_release(struct vitrin_buffer *b) {
	if (b->data != NULL) {
		munmap(b->data, b->size);
		b->data = NULL;
	}
	if (b->fd >= 0) {
		close(b->fd);
		b->fd = -1;
	}
	b->size = 0;
	b->width = b->height = b->stride = 0;
}

/* (Re)allocate a slot for `width` x `height` at a tight stride.
 *
 * The memfd is sealed F_SEAL_SHRINK | F_SEAL_GROW immediately after sizing.
 * That is a promise to the core, kept structurally: the core validates at
 * `attach` that the fd is at least stride * height bytes and then `pread`s
 * exactly that at `commit`, and a shim that shrank the memfd in between
 * would earn `invalid_buffer` -- log-and-close, with nothing on the wire to
 * explain it. Sealing makes that race unrepresentable rather than merely
 * avoided. It costs nothing: the core requires no seals (its `F_GET_SEALS`
 * probe only checks the fd is seal-CAPABLE, i.e. really shmem-backed), and
 * F_SEAL_WRITE is deliberately NOT taken -- writing is exactly what reuse
 * needs. A geometry change discards the slot and allocates afresh, which is
 * why the seals may fix the size for the slot's whole life. */
static bool buffer_alloc(struct vitrin_buffer *b, uint32_t width, uint32_t height) {
	buffer_release(b);

	size_t stride = (size_t)width * VITRIN_BPP;
	size_t size = stride * (size_t)height;

	b->fd = memfd_create("vitrin-shim-frame", MFD_CLOEXEC | MFD_ALLOW_SEALING);
	if (b->fd < 0) {
		wlr_log(WLR_ERROR, "memfd_create failed: %s", strerror(errno));
		return false;
	}
	if (ftruncate(b->fd, (off_t)size) != 0) {
		wlr_log(WLR_ERROR, "ftruncate(%zu) failed: %s", size, strerror(errno));
		buffer_release(b);
		return false;
	}
	if (fcntl(b->fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW) != 0) {
		wlr_log(WLR_ERROR, "sealing the frame memfd failed: %s", strerror(errno));
		buffer_release(b);
		return false;
	}
	b->data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, b->fd, 0);
	if (b->data == MAP_FAILED) {
		b->data = NULL;
		wlr_log(WLR_ERROR, "mapping the frame memfd failed: %s", strerror(errno));
		buffer_release(b);
		return false;
	}

	b->size = size;
	b->width = width;
	b->height = height;
	b->stride = (uint32_t)stride;
	return true;
}

/* A slot the shim owns and that fits this geometry, reallocating on a size
 * change. NULL when every slot is still in flight -- which the caller has
 * already ruled out via vitrin_upstream_can_forward, so it means the pool
 * accounting and the admission bit disagree. */
static struct vitrin_buffer *pool_acquire(struct vitrin_upstream *up,
		uint32_t width, uint32_t height) {
	struct vitrin_buffer *free_slot = NULL;
	for (int i = 0; i < VITRIN_BUFFER_POOL_SLOTS; i++) {
		struct vitrin_buffer *b = &up->pool[i];
		if (b->buffer_id != 0) {
			continue; /* in flight: the core may still be reading it */
		}
		if (b->data != NULL && b->width == width && b->height == height) {
			return b; /* the reuse fast path: no syscall, no fault */
		}
		if (free_slot == NULL) {
			free_slot = b;
		}
	}
	if (free_slot == NULL) {
		return NULL;
	}
	return buffer_alloc(free_slot, width, height) ? free_slot : NULL;
}

/* ---- pixel copy-out -------------------------------------------------- */

static bool format_from_drm(uint32_t drm, vitrin_view_format_t *out) {
	switch (drm) {
	case DRM_FORMAT_XRGB8888:
		*out = VITRIN_VIEW_FORMAT_XRGB8888;
		return true;
	case DRM_FORMAT_ARGB8888:
		*out = VITRIN_VIEW_FORMAT_ARGB8888;
		return true;
	default:
		return false;
	}
}

/* Copy the composed frame out of `buf` into `slot`.
 *
 * Two paths, in preference order:
 *
 *   1. A direct pointer to the buffer's storage. This is the CI and
 *      version-0 path: with the pixman software renderer the output's
 *      buffers are shm-backed, so the copy is row memcpys at the buffer's
 *      own stride into our tight one -- no renderer involvement at all.
 *   2. A renderer read-back, for a buffer with no CPU mapping (a GPU
 *      allocator's dmabuf) or an exotic format. Costs a texture import per
 *      frame, which is precisely why it is the fallback and why D3 keeps
 *      the mandatory path on shm.
 *
 * Both produce tightly packed rows in one of the two version-1 formats,
 * which is what the core's copy-in converter accepts. */
static bool copy_frame(struct vitrin_shim *s, struct wlr_buffer *buf,
		struct vitrin_buffer *slot) {
	void *src = NULL;
	uint32_t src_format = 0;
	size_t src_stride = 0;

	if (wlr_buffer_begin_data_ptr_access(buf, WLR_BUFFER_DATA_PTR_ACCESS_READ,
			&src, &src_format, &src_stride)) {
		vitrin_view_format_t mapped;
		if (format_from_drm(src_format, &mapped)) {
			const uint8_t *in = src;
			uint8_t *out = slot->data;
			for (uint32_t y = 0; y < slot->height; y++) {
				memcpy(out + (size_t)y * slot->stride,
					in + (size_t)y * src_stride,
					slot->stride);
			}
			slot->format = mapped;
			wlr_buffer_end_data_ptr_access(buf);
			return true;
		}
		/* Mapped, but in a format version 1 has no name for. Fall through
		 * to the renderer, which will convert. */
		wlr_buffer_end_data_ptr_access(buf);
		wlr_log(WLR_DEBUG,
			"composed buffer format 0x%08x is outside the v0 set; converting via the renderer",
			src_format);
	}

	struct wlr_texture *tex = wlr_texture_from_buffer(s->renderer, buf);
	if (tex == NULL) {
		wlr_log(WLR_ERROR, "cannot read back the composed frame: no texture");
		return false;
	}
	struct wlr_texture_read_pixels_options opts = {
		.data = slot->data,
		.format = DRM_FORMAT_XRGB8888,
		.stride = slot->stride,
		.dst_x = 0,
		.dst_y = 0,
		.src_box = {0},
	};
	bool ok = wlr_texture_read_pixels(tex, &opts);
	wlr_texture_destroy(tex);
	if (!ok) {
		wlr_log(WLR_ERROR, "wlr_texture_read_pixels failed");
		return false;
	}
	slot->format = VITRIN_VIEW_FORMAT_XRGB8888;
	return true;
}

/* ---- sending ---------------------------------------------------------- */

static bool send_frame(struct vitrin_shim *s, const uint8_t *bytes, int32_t len, int fd) {
	if (len < 0) {
		upstream_die(s, "encode failed");
		return false;
	}
	if (!vitrin_wire_send(&s->up.wire, bytes, (size_t)len, fd)) {
		upstream_die(s, "send failed");
		return false;
	}
	return true;
}

/* One `damage` request per rectangle, verbatim in buffer coordinates.
 *
 * The region arrives output-buffer-local from the compositor and the buffer
 * we just attached IS that output buffer, so the two coordinate spaces are
 * the same one and no transform is needed.
 *
 * Past VITRIN_MAX_DAMAGE_RECTS the tail is folded into its bounding box:
 * damage is a hint the core MAY over-approximate, so a coarser rectangle is
 * always legal, and the alternative -- unbounded wire traffic driven by
 * however finely the app happened to damage -- is not. */
static void send_damage(struct vitrin_shim *s, const pixman_region32_t *damage,
		uint32_t width, uint32_t height) {
	struct vitrin_upstream *up = &s->up;
	uint8_t buf[VITRIN_ENCODE_BUF];
	int nrects = 0;
	const pixman_box32_t *rects = NULL;

	if (damage != NULL && pixman_region32_not_empty(damage)) {
		rects = pixman_region32_rectangles((pixman_region32_t *)damage, &nrects);
	}
	if (rects == NULL || nrects <= 0) {
		/* No region: the whole surface changed (or the compositor did not
		 * tell us what did). Correct, if coarse -- and the honest thing to
		 * send, since claiming narrower damage than we can prove would let
		 * a later damage-respecting core drop real pixels. */
		vitrin_shim_surface_req_damage_t d = {
			.x = 0, .y = 0, .width = (int32_t)width, .height = (int32_t)height,
		};
		int32_t n = vitrin_shim_surface_req_damage_encode(&d, VITRIN_SURFACE_ID,
			buf, sizeof(buf));
		if (send_frame(s, buf, n, -1)) {
			up->damage_rects_sent++;
		}
		return;
	}

	int emit = nrects < VITRIN_MAX_DAMAGE_RECTS ? nrects : VITRIN_MAX_DAMAGE_RECTS;
	for (int i = 0; i < emit; i++) {
		pixman_box32_t box = rects[i];
		if (i == VITRIN_MAX_DAMAGE_RECTS - 1 && nrects > VITRIN_MAX_DAMAGE_RECTS) {
			for (int j = i + 1; j < nrects; j++) {
				if (rects[j].x1 < box.x1) { box.x1 = rects[j].x1; }
				if (rects[j].y1 < box.y1) { box.y1 = rects[j].y1; }
				if (rects[j].x2 > box.x2) { box.x2 = rects[j].x2; }
				if (rects[j].y2 > box.y2) { box.y2 = rects[j].y2; }
			}
		}
		vitrin_shim_surface_req_damage_t d = {
			.x = box.x1,
			.y = box.y1,
			.width = box.x2 - box.x1,
			.height = box.y2 - box.y1,
		};
		int32_t n = vitrin_shim_surface_req_damage_encode(&d, VITRIN_SURFACE_ID,
			buf, sizeof(buf));
		if (!send_frame(s, buf, n, -1)) {
			return;
		}
		up->damage_rects_sent++;
	}
}

bool vitrin_upstream_can_forward(const struct vitrin_shim *s) {
	const struct vitrin_upstream *up = &s->up;
	if (!up->active || !vitrin_wire_alive(&up->wire)) {
		return false;
	}
	if (up->frame_inflight) {
		return false;
	}
	for (int i = 0; i < VITRIN_BUFFER_POOL_SLOTS; i++) {
		if (up->pool[i].buffer_id == 0) {
			return true;
		}
	}
	return false;
}

/* This composed frame is not going upstream. Say why, once, and report it as
 * a loss so the caller runs its recovery instead of dropping the frame on the
 * floor. Every caller of this is post-composite by construction. */
static bool forward_lost(const char *why) {
	wlr_log(WLR_ERROR, "composed frame not forwarded: %s", why);
	return false;
}

bool vitrin_upstream_forward(struct vitrin_shim *s, struct wlr_buffer *buf,
		const pixman_region32_t *damage) {
	struct vitrin_upstream *up = &s->up;
	if (!up->active) {
		/* Standalone (--no-upstream): there is no core, so this frame was
		 * never destined anywhere and nothing was lost by not sending it.
		 * output.c paces the app itself in that mode. */
		return true;
	}
	if (buf == NULL || !vitrin_upstream_can_forward(s)) {
		/* Both are the caller's preconditions -- it checks can_forward before
		 * it composites, precisely so an unforwardable frame is never
		 * rendered. Reaching here means the two disagree, and the composite
		 * has already been spent, so it takes the recovery path. */
		return forward_lost("the frame path was not ready");
	}
	if (buf->width <= 0 || buf->height <= 0) {
		return forward_lost("the composed buffer has a zero dimension");
	}
	uint32_t width = (uint32_t)buf->width;
	uint32_t height = (uint32_t)buf->height;
	/* The core's limits, checked before the copy rather than after (see the
	 * constants). Both are far above any realm view version 1 composites. */
	if (width > VITRIN_MAX_SURFACE_DIM || height > VITRIN_MAX_SURFACE_DIM ||
			(uint64_t)width * height * VITRIN_BPP > VITRIN_MAX_SURFACE_BYTES) {
		return forward_lost("the composed buffer exceeds the core's limits");
	}

	struct vitrin_buffer *slot = pool_acquire(up, width, height);
	if (slot == NULL) {
		return forward_lost("no buffer slot could be allocated");
	}

	/* Hold the buffer across the copy: the compositor could otherwise
	 * recycle it into the swapchain underneath us. */
	wlr_buffer_lock(buf);
	bool copied = copy_frame(s, buf, slot);
	wlr_buffer_unlock(buf);
	if (!copied) {
		return forward_lost("the composed pixels could not be copied out");
	}

	/* Cookies are monotonic, skipping 0 (which the pool uses to mean "the
	 * shim owns this slot"). The IDL only requires that a cookie not name a
	 * buffer whose ownership has not returned; monotonic satisfies that
	 * trivially and makes a wire trace readable in the bargain. */
	uint32_t cookie = up->next_buffer_id++;
	if (up->next_buffer_id == 0) {
		up->next_buffer_id = 1;
	}
	slot->buffer_id = cookie;

	uint8_t frame[VITRIN_ENCODE_BUF];
	vitrin_shim_surface_req_attach_t attach = {
		.buffer_id = cookie,
		.fd = slot->fd, /* not encoded: it rides SCM_RIGHTS, see the header */
		.kind = VITRIN_SHIM_SURFACE_KIND_SHM,
		.format = slot->format,
		.width = slot->width,
		.height = slot->height,
		.stride = slot->stride,
	};
	int32_t n = vitrin_shim_surface_req_attach_encode(&attach, VITRIN_SURFACE_ID,
		frame, sizeof(frame));
	/* The shim keeps its memfd -- that is the whole point of the pool. What
	 * crosses is an SCM_RIGHTS duplicate, and the core closes that one, per
	 * the IDL's fd-ownership rule.
	 *
	 * A send failure from here on is not a lost frame but a lost LINK:
	 * send_frame has already called upstream_die, so the display is
	 * terminating and there is nothing to recover the frame for. It still
	 * reports false, because the frame did not reach the core and the caller
	 * must not treat a half-sent commit as a presented one. */
	if (!send_frame(s, frame, n, slot->fd)) {
		return false;
	}

	send_damage(s, damage, slot->width, slot->height);

	vitrin_shim_surface_req_commit_t commit = {.reserved = 0};
	n = vitrin_shim_surface_req_commit_encode(&commit, VITRIN_SURFACE_ID,
		frame, sizeof(frame));
	if (!send_frame(s, frame, n, -1)) {
		return false;
	}

	up->frame_inflight = true;
	up->frames_forwarded++;
	wlr_log(WLR_DEBUG, "forwarded frame: buffer_id=%u %ux%u stride=%u",
		cookie, slot->width, slot->height, slot->stride);
	return true;
}

void vitrin_upstream_pace_locally(struct vitrin_shim *s) {
	if (s->scene_output == NULL) {
		return;
	}
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	wlr_scene_output_send_frame_done(s->scene_output, &now);
	if (s->up.active) {
		s->up.unpaced_frame_dones++;
	}
}

/* ---- the core's answers ---------------------------------------------- */

/* A tick was skipped under backpressure and the core has since freed us:
 * ask the output for another frame. Without this the headless output's
 * timer, which is re-armed by a commit, would never fire again -- the shim
 * would go quiet exactly when it became able to work. */
static void upstream_resume(struct vitrin_shim *s) {
	if (s->up.composite_deferred && vitrin_upstream_can_forward(s) && s->output != NULL) {
		s->up.composite_deferred = false;
		wlr_output_schedule_frame(s->output);
	}
}

static void handle_frame_done(struct vitrin_shim *s, const uint8_t *frame, size_t len) {
	uint32_t object_id = 0;
	vitrin_shim_surface_evt_frame_done_t ev;
	vitrin_decode_status_t st =
		vitrin_shim_surface_evt_frame_done_decode(frame, len, -1, &object_id, &ev);
	if (st != VITRIN_DECODE_OK) {
		wlr_log(WLR_ERROR, "malformed frame_done: %s", vitrin_decode_status_string(st));
		return;
	}

	s->up.frame_inflight = false;
	s->up.frame_dones++;

	/* THE relay (PRD Doc 2 section 4.4). The app's `wl_surface.frame`
	 * callbacks fire here and nowhere else while the upstream link is
	 * active, so the app's redraw clock is the core's presentation clock
	 * rather than this shim's headless timer. The core's own `time_ms` is
	 * passed through rather than a local reading: relaying the cadence but
	 * substituting our clock would hand the app a timebase that drifts
	 * against the frames it is actually being shown. */
	if (s->scene_output != NULL) {
		struct timespec when = {
			.tv_sec = (time_t)(ev.time_ms / 1000u),
			.tv_nsec = (long)(ev.time_ms % 1000u) * 1000000L,
		};
		wlr_scene_output_send_frame_done(s->scene_output, &when);
	}
	upstream_resume(s);
}

static void handle_buffer_done(struct vitrin_shim *s, const uint8_t *frame, size_t len) {
	uint32_t object_id = 0;
	vitrin_shim_surface_evt_buffer_done_t ev;
	vitrin_decode_status_t st =
		vitrin_shim_surface_evt_buffer_done_decode(frame, len, -1, &object_id, &ev);
	if (st != VITRIN_DECODE_OK) {
		wlr_log(WLR_ERROR, "malformed buffer_done: %s", vitrin_decode_status_string(st));
		return;
	}
	s->up.buffer_dones++;

	if (ev.status != VITRIN_SHIM_SURFACE_BUFFER_STATUS_RELEASED) {
		/* The recoverable arm: the buffer was NOT used and the previously
		 * committed content stays on screen. Under version-1 shm the only
		 * reachable status is `too_large`, which the size guard in
		 * vitrin_upstream_forward already prevents -- so this line firing
		 * means the core's limits and ours have drifted apart. The dmabuf
		 * fallback statuses belong to the v1 passthrough path, which this
		 * version does not take. */
		wlr_log(WLR_ERROR,
			"core rejected buffer %u (status %u): the frame was not presented",
			ev.buffer_id, (unsigned)ev.status);
	}

	for (int i = 0; i < VITRIN_BUFFER_POOL_SLOTS; i++) {
		if (s->up.pool[i].buffer_id == ev.buffer_id) {
			/* Ownership is back: the slot is writable again. This is the
			 * single place that clears the in-flight marker, which is what
			 * makes "never write a buffer the core is reading" structural. */
			s->up.pool[i].buffer_id = 0;
			upstream_resume(s);
			return;
		}
	}
	wlr_log(WLR_ERROR, "buffer_done for unknown cookie %u", ev.buffer_id);
}

static void handle_configure(struct vitrin_shim *s, const uint8_t *frame, size_t len) {
	uint32_t object_id = 0;
	vitrin_shim_session_evt_configure_t ev;
	vitrin_decode_status_t st =
		vitrin_shim_session_evt_configure_decode(frame, len, -1, &object_id, &ev);
	if (st != VITRIN_DECODE_OK) {
		wlr_log(WLR_ERROR, "malformed configure: %s", vitrin_decode_status_string(st));
		return;
	}
	/* `configure` MAY be re-sent when the realm view resizes, and a
	 * version-1 core is explicitly permitted to letterbox a fixed-size
	 * buffer instead -- which is what the core in this tree does (it sends
	 * `configure` exactly once, at connection setup). Live resize is
	 * therefore reported and not acted on: acting on it means re-moding the
	 * headless output and re-configuring the toplevel mid-stream, which is
	 * its own task, and silently ignoring it would hide the day the core
	 * starts sending it. */
	if (ev.width != s->up.view_width || ev.height != s->up.view_height) {
		wlr_log(WLR_INFO,
			"core re-configured the realm view to %ux%u; version 1 letterboxes "
			"(live resize is a later task)",
			ev.width, ev.height);
	}
}

/* One complete frame from the core. Routing is by object id, exactly as the
 * core routes ours: id 1 is the session, id 2 the surface we minted, id 3
 * the seat. */
static bool upstream_message(void *data, const uint8_t *frame, size_t len) {
	struct vitrin_shim *s = data;
	vitrin_frame_header_t hdr;
	if (vitrin_frame_header_decode(frame, len, &hdr) != VITRIN_DECODE_OK) {
		return false;
	}

	switch (hdr.object_id) {
	case VITRIN_SESSION_ID:
		if (hdr.opcode == VITRIN_SHIM_SESSION_EVT_CONFIGURE_OPCODE) {
			handle_configure(s, frame, len);
		} else {
			wlr_log(WLR_ERROR, "unknown session event opcode %u", hdr.opcode);
		}
		return true;
	case VITRIN_SURFACE_ID:
		if (hdr.opcode == VITRIN_SHIM_SURFACE_EVT_FRAME_DONE_OPCODE) {
			handle_frame_done(s, frame, len);
		} else if (hdr.opcode == VITRIN_SHIM_SURFACE_EVT_BUFFER_DONE_OPCODE) {
			handle_buffer_done(s, frame, len);
		} else {
			wlr_log(WLR_ERROR, "unknown surface event opcode %u", hdr.opcode);
		}
		return true;
	case VITRIN_SEAT_ID:
		/* Input replay (P1.6.3). Everything past the routing decision --
		 * decoding, the origin tag, the dynamic keymap, the view ->
		 * surface-local mapping -- lives in seat.c; see seat.h for why this
		 * is the only call into it. */
		vitrin_seat_handle_event(s, frame, len);
		return true;
	default:
		/* Version 1 has no other objects. The core is the TCB, so this is
		 * version skew rather than an attack; discard and keep serving (the
		 * conventions' tolerate-unknown-events posture) instead of killing
		 * an app over a message we merely do not recognize. */
		wlr_log(WLR_ERROR, "event for unknown object %u (opcode %u); discarded",
			hdr.object_id, hdr.opcode);
		return true;
	}
}

/* ---- bring-up --------------------------------------------------------- */

void vitrin_upstream_init(struct vitrin_shim *s) {
	struct vitrin_upstream *up = &s->up;
	up->active = false;
	up->next_buffer_id = 1;
	/* -1, not 0, on every descriptor this struct will ever own. `struct
	 * vitrin_shim s = {0}` would leave them at 0 -- a perfectly valid
	 * descriptor, the process's stdin -- and vitrin_upstream_finish releases
	 * the pool unconditionally, on every exit path including --no-upstream's
	 * (which never opens a connection and so never allocates a slot). See the
	 * header: this runs before the standalone/upstream branch precisely so
	 * the invariant does not depend on which branch was taken. */
	up->wire.fd = -1;
	for (int i = 0; i < VITRIN_BUFFER_POOL_SLOTS; i++) {
		up->pool[i].fd = -1;
	}
}

bool vitrin_upstream_open(struct vitrin_shim *s) {
	struct vitrin_upstream *up = &s->up;

	if (!vitrin_wire_adopt(&up->wire)) {
		return false;
	}

	/* The one synchronous read (conventions 7.2). It happens here, before a
	 * single wlroots object exists, because `configure` carries the geometry
	 * the output and the app's window are built at -- and because doing it
	 * before the private socket is bound is what lets the rest of the shim
	 * assume a configured session with no state machine. */
	uint8_t frame[VITRIN_WIRE_SLOT_BYTES + VITRIN_REALM_MAX_BYTES + 16];
	size_t len = 0;
	if (!vitrin_wire_recv_sync(&up->wire, VITRIN_CONFIGURE_TIMEOUT_MS,
			frame, sizeof(frame), &len)) {
		return false;
	}

	uint32_t object_id = 0;
	vitrin_shim_session_evt_configure_t cfg;
	vitrin_decode_status_t st =
		vitrin_shim_session_evt_configure_decode(frame, len, -1, &object_id, &cfg);
	if (st != VITRIN_DECODE_OK) {
		wlr_log(WLR_ERROR, "the core's first message is not a valid configure: %s",
			vitrin_decode_status_string(st));
		return false;
	}
	if (object_id != VITRIN_SESSION_ID) {
		wlr_log(WLR_ERROR, "configure addressed object %u, not the session object %u",
			object_id, VITRIN_SESSION_ID);
		return false;
	}
	if (cfg.width == 0 || cfg.height == 0) {
		wlr_log(WLR_ERROR, "core configured a zero-sized realm view (%ux%u)",
			cfg.width, cfg.height);
		return false;
	}

	/* The realm string is a borrowed, length-delimited view into `frame`
	 * (never NUL-terminated on the wire); copy it out before `frame` dies. */
	size_t realm_len = cfg.realm.len < VITRIN_REALM_MAX_BYTES
		? cfg.realm.len : VITRIN_REALM_MAX_BYTES;
	memcpy(up->realm, cfg.realm.data, realm_len);
	up->realm[realm_len] = '\0';
	up->view_width = cfg.width;
	up->view_height = cfg.height;

	/* The core's geometry is authoritative: it is the realm view an agent
	 * will capture and the size the layout must fill. --width/--height stay
	 * meaningful only in standalone mode. */
	s->cfg.width = (int)cfg.width;
	s->cfg.height = (int)cfg.height;

	uint8_t out[VITRIN_ENCODE_BUF];
	vitrin_shim_session_req_create_surface_t create = {.surface = VITRIN_SURFACE_ID};
	int32_t n = vitrin_shim_session_req_create_surface_encode(&create,
		VITRIN_SESSION_ID, out, sizeof(out));
	if (n < 0 || !vitrin_wire_send(&up->wire, out, (size_t)n, -1)) {
		wlr_log(WLR_ERROR, "failed to mint the shim surface");
		return false;
	}

	vitrin_shim_session_req_get_seat_t seat = {.seat = VITRIN_SEAT_ID};
	n = vitrin_shim_session_req_get_seat_encode(&seat, VITRIN_SESSION_ID,
		out, sizeof(out));
	if (n < 0 || !vitrin_wire_send(&up->wire, out, (size_t)n, -1)) {
		wlr_log(WLR_ERROR, "failed to mint the shim seat");
		return false;
	}

	up->active = true;
	wlr_log(WLR_INFO, "upstream link up: realm=\"%s\" view=%ux%u surface=%u seat=%u",
		up->realm, up->view_width, up->view_height,
		VITRIN_SURFACE_ID, VITRIN_SEAT_ID);
	return true;
}

/* One wakeup's worth of core traffic has been dispatched. The only thing
 * that cares is input replay: the pointer events this batch carried belong
 * to one group, and `wl_pointer.frame` is how the app is told so (seat.h,
 * "pointer batching"). */
static void upstream_drained(void *data) {
	vitrin_seat_frame_boundary((struct vitrin_shim *)data);
}

/* The core is gone. Socketpair EOF is how the core says so -- the first rung
 * of its orderly shutdown ladder (P1.5.3), before it escalates to SIGTERM --
 * and a shim that only died on the signal would make every clean shutdown
 * take the timeout path. Exiting here also means a crashed core takes its
 * realm down with it, which is the intended coupling: there is nowhere for
 * the app's frames to go. */
static void upstream_closed(void *data) {
	struct vitrin_shim *s = data;
	if (s->up.wire.failed) {
		upstream_die(s, s->up.wire.fail_reason ? s->up.wire.fail_reason : "protocol failure");
		return;
	}
	wlr_log(WLR_INFO, "the core closed the connection; shutting down");
	if (s->display != NULL) {
		wl_display_terminate(s->display);
	}
}

bool vitrin_upstream_start(struct vitrin_shim *s) {
	if (!s->up.active) {
		return true;
	}
	return vitrin_wire_arm(&s->up.wire, s->loop, upstream_message, upstream_drained,
		upstream_closed, s);
}

void vitrin_upstream_finish(struct vitrin_shim *s) {
	for (int i = 0; i < VITRIN_BUFFER_POOL_SLOTS; i++) {
		buffer_release(&s->up.pool[i]);
	}
	vitrin_wire_finish(&s->up.wire);
	s->up.active = false;
}
