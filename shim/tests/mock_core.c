/* mock_core.c -- the CORE side of the shim wire, for acceptance testing the
 * C shim's upstream link (P1.6.2).
 *
 * This is to `crates/vitrin-core/src/shim.rs` what `crates/vitrin-mock-shim`
 * is to the C shim: a small, deliberately strict peer that lets one side be
 * exercised end to end without the other's whole stack. It exists in C, not
 * Rust, because the shim's CI job must not pull a Rust toolchain onto PATH
 * (shim/ci/install-deps.sh states that as an invariant), so the acceptance
 * proof has to stand on its own.
 *
 * WHAT IT DOES, in the order the protocol does it:
 *
 *   1. socketpair, fork, place the shim's end at fd 3 with FD_CLOEXEC
 *      CLEARED -- byte for byte the spawn contract of P1.5.2
 *      (crates/vitrin-core/src/spawn.rs), including the detail that the shim
 *      is the one obliged to re-arm the flag.
 *   2. send `vitrin_shim_session.configure` as the first message on the
 *      connection, before reading anything.
 *   3. read the shim's requests, VALIDATING EACH ONE THE WAY THE REAL CORE
 *      DOES (see below), and answer `buffer_done` + `frame_done`.
 *   4. print a machine-readable trace of every wire event, so a shell script
 *      can assert on the frame path mechanically instead of eyeballing logs.
 *
 * THE VALIDATION IS THE POINT. Every check below is transcribed from the
 * real core's `handle_attach` / `handle_damage` / `handle_commit`, because a
 * shim that passes a lenient mock and fails the real core has been tested
 * for nothing. In particular the real core answers a violation with
 * log-and-close and NO wire event, so a shim bug there presents as an
 * unexplained disconnect; here it presents as a named FAIL line.
 *
 *   attach   nonzero dimensions; stride >= width * 4; F_GET_SEALS succeeds
 *            (the fd must really be shmem-backed); fd size >= stride*height
 *   damage   only after an attach has ever been seen (bad_order)
 *   commit   likewise; latches pending state; copies the pixels out with
 *            pread, exactly as the core's copy-in does -- never mmap
 *
 * Options exist to make the core behave badly on purpose, which is what the
 * backpressure criterion needs:
 *
 *   --frame-delay MS   sit on `frame_done` for MS -- a slow presenting core
 *   --defer-release    hold `buffer_done` until the NEXT commit, which is
 *                      the dmabuf passthrough timing regime and the case a
 *                      one-slot buffer pool would deadlock on
 */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "vitrin-protocol.h"

#define CORE_FD 3
#define RX_CAP 65536
#define MAX_PENDING_DAMAGE 128

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

struct damage_rect {
	int32_t x, y, w, h;
};

struct pending_buffer {
	bool present;
	uint32_t buffer_id;
	int fd;
	uint32_t kind, format, width, height, stride;
};

struct core {
	int fd;
	uint8_t rx[RX_CAP];
	size_t rx_len;

	/* session state */
	bool surface_created;
	uint32_t surface_id;
	bool seat_created;
	uint32_t seat_id;
	uint32_t watermark;

	/* surface pending state (Wayland's double-buffered model) */
	bool ever_attached;
	struct pending_buffer pending;
	struct damage_rect damage[MAX_PENDING_DAMAGE];
	int damage_len;

	/* deferred dispositions */
	bool defer_release;
	uint32_t retained_id;
	int retained_fd;

	/* pacing */
	int frame_delay_ms;
	bool frame_due;
	struct timespec frame_at;

	/* accounting the acceptance script asserts on */
	uint64_t commits;
	uint64_t frame_dones;
	uint64_t buffer_dones;
	int inflight;
	int max_inflight;
	uint64_t last_damage_area;
	int last_damage_rects;

	int failures;
};

static void trace(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void trace(const char *fmt, ...) {
	va_list ap;
	va_start(ap, fmt);
	vprintf(fmt, ap);
	va_end(ap);
	putchar('\n');
	fflush(stdout);
}

static void fail(struct core *c, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
static void fail(struct core *c, const char *fmt, ...) {
	va_list ap;
	printf("FAIL ");
	va_start(ap, fmt);
	vprintf(fmt, ap);
	va_end(ap);
	putchar('\n');
	fflush(stdout);
	c->failures++;
}

static int64_t now_ms(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

/* ---- sending (core -> shim; never carries an fd in version 1) -------- */

static bool core_send(struct core *c, const uint8_t *frame, size_t len) {
	size_t sent = 0;
	while (sent < len) {
		ssize_t n = send(c->fd, frame + sent, len - sent, MSG_NOSIGNAL);
		if (n < 0) {
			if (errno == EINTR) {
				continue;
			}
			return false;
		}
		sent += (size_t)n;
	}
	return true;
}

static void send_configure(struct core *c, const char *realm, uint32_t w, uint32_t h) {
	uint8_t buf[256];
	vitrin_shim_session_evt_configure_t ev = {
		.realm = {.len = (uint32_t)strlen(realm), .data = (const uint8_t *)realm},
		.width = w,
		.height = h,
	};
	int32_t n = vitrin_shim_session_evt_configure_encode(&ev, 1, buf, sizeof(buf));
	if (n < 0 || !core_send(c, buf, (size_t)n)) {
		fail(c, "cannot send configure");
	}
}

static void send_buffer_done(struct core *c, uint32_t buffer_id, uint32_t status) {
	uint8_t buf[64];
	vitrin_shim_surface_evt_buffer_done_t ev = {
		.buffer_id = buffer_id,
		.status = (vitrin_shim_surface_buffer_status_t)status,
	};
	int32_t n = vitrin_shim_surface_evt_buffer_done_encode(&ev, c->surface_id, buf, sizeof(buf));
	if (n < 0 || !core_send(c, buf, (size_t)n)) {
		fail(c, "cannot send buffer_done");
		return;
	}
	c->buffer_dones++;
	trace("EV buffer_done buffer_id=%u status=%u", buffer_id, status);
}

static void send_frame_done(struct core *c) {
	uint8_t buf[64];
	vitrin_shim_surface_evt_frame_done_t ev = {.time_ms = (uint32_t)now_ms()};
	int32_t n = vitrin_shim_surface_evt_frame_done_encode(&ev, c->surface_id, buf, sizeof(buf));
	if (n < 0 || !core_send(c, buf, (size_t)n)) {
		fail(c, "cannot send frame_done");
		return;
	}
	c->frame_dones++;
	c->inflight--;
	trace("EV frame_done time_ms=%u inflight=%d", ev.time_ms, c->inflight);
}

/* ---- request handling ------------------------------------------------ */

/* conventions 3.1: client ids are strictly increasing, never reused, and
 * inside [2, 0xfeffffff]. */
static void allocate_id(struct core *c, uint32_t id, const char *what) {
	if (id <= c->watermark || id > 0xfeffffffu) {
		fail(c, "invalid_object: %s id %u violates the watermark rule (watermark %u)",
			what, id, c->watermark);
		return;
	}
	c->watermark = id;
}

static void handle_create_surface(struct core *c, const uint8_t *f, size_t len) {
	uint32_t oid = 0;
	vitrin_shim_session_req_create_surface_t req;
	vitrin_decode_status_t st =
		vitrin_shim_session_req_create_surface_decode(f, len, -1, &oid, &req);
	if (st != VITRIN_DECODE_OK) {
		fail(c, "create_surface decode: %s", vitrin_decode_status_string(st));
		return;
	}
	allocate_id(c, req.surface, "create_surface");
	c->surface_created = true;
	c->surface_id = req.surface;
	trace("EV create_surface id=%u", req.surface);
}

static void handle_get_seat(struct core *c, const uint8_t *f, size_t len) {
	uint32_t oid = 0;
	vitrin_shim_session_req_get_seat_t req;
	vitrin_decode_status_t st =
		vitrin_shim_session_req_get_seat_decode(f, len, -1, &oid, &req);
	if (st != VITRIN_DECODE_OK) {
		fail(c, "get_seat decode: %s", vitrin_decode_status_string(st));
		return;
	}
	if (c->seat_created) {
		fail(c, "already_initialized: a second get_seat on the session");
		return;
	}
	allocate_id(c, req.seat, "get_seat");
	c->seat_created = true;
	c->seat_id = req.seat;
	trace("EV get_seat id=%u", req.seat);
}

static void handle_attach(struct core *c, const uint8_t *f, size_t len, int fd) {
	uint32_t oid = 0;
	vitrin_shim_surface_req_attach_t req;
	vitrin_decode_status_t st =
		vitrin_shim_surface_req_attach_decode(f, len, fd, &oid, &req);
	if (st != VITRIN_DECODE_OK) {
		fail(c, "attach decode: %s", vitrin_decode_status_string(st));
		if (fd >= 0) {
			close(fd);
		}
		return;
	}

	/* Geometry first, exactly as the core orders it. */
	if (req.width == 0 || req.height == 0) {
		fail(c, "invalid_buffer: zero dimension %ux%u", req.width, req.height);
	}
	if ((uint64_t)req.stride < (uint64_t)req.width * 4) {
		fail(c, "invalid_buffer: stride %u below width*4 = %llu",
			req.stride, (unsigned long long)req.width * 4);
	}
	/* The fd must be shmem-backed. F_GET_SEALS is implemented by the kernel
	 * only for memfd/tmpfs/hugetlb, entirely in-kernel, so it cannot be
	 * faked and (unlike fstat) cannot block on hostile filesystem I/O. */
	int seals = fcntl(fd, F_GET_SEALS);
	if (seals < 0) {
		fail(c, "invalid_buffer: kind=shm fd is not memfd/shmem-backed (F_GET_SEALS: %s)",
			strerror(errno));
		seals = 0;
	}
	struct stat sb;
	off_t fd_size = 0;
	if (fstat(fd, &sb) == 0) {
		fd_size = sb.st_size;
	}
	uint64_t required = (uint64_t)req.stride * req.height;
	if ((uint64_t)fd_size < required) {
		fail(c, "invalid_buffer: fd size %lld below stride*height = %llu",
			(long long)fd_size, (unsigned long long)required);
	}
	if (req.kind != VITRIN_SHIM_SURFACE_KIND_SHM) {
		fail(c, "version-0 attach must be kind=shm (D3), got %u", (unsigned)req.kind);
	}

	/* Supersede: a replaced pending attach is released promptly, unused. */
	if (c->pending.present) {
		close(c->pending.fd);
		send_buffer_done(c, c->pending.buffer_id, VITRIN_SHIM_SURFACE_BUFFER_STATUS_RELEASED);
		c->pending.present = false;
	}

	c->ever_attached = true;
	c->pending.present = true;
	c->pending.buffer_id = req.buffer_id;
	c->pending.fd = fd;
	c->pending.kind = (uint32_t)req.kind;
	c->pending.format = (uint32_t)req.format;
	c->pending.width = req.width;
	c->pending.height = req.height;
	c->pending.stride = req.stride;

	trace("EV attach buffer_id=%u kind=%u format=0x%08x w=%u h=%u stride=%u fd_size=%lld seals=0x%x",
		req.buffer_id, (unsigned)req.kind, (unsigned)req.format, req.width,
		req.height, req.stride, (long long)fd_size, (unsigned)seals);
}

static void handle_damage(struct core *c, const uint8_t *f, size_t len) {
	uint32_t oid = 0;
	vitrin_shim_surface_req_damage_t req;
	vitrin_decode_status_t st = vitrin_shim_surface_req_damage_decode(f, len, -1, &oid, &req);
	if (st != VITRIN_DECODE_OK) {
		fail(c, "damage decode: %s", vitrin_decode_status_string(st));
		return;
	}
	if (!c->ever_attached) {
		fail(c, "bad_order: damage against a never-attached surface");
		return;
	}
	if (c->damage_len < MAX_PENDING_DAMAGE) {
		c->damage[c->damage_len].x = req.x;
		c->damage[c->damage_len].y = req.y;
		c->damage[c->damage_len].w = req.width;
		c->damage[c->damage_len].h = req.height;
		c->damage_len++;
	}
	trace("EV damage x=%d y=%d w=%d h=%d", req.x, req.y, req.width, req.height);
}

/* The copy-in: pread rows bounded by the validated geometry, never a
 * mapping, so an fd that lied since attach is a clean short read rather
 * than SIGBUS. FNV-1a over the copied bytes gives the shell script a
 * content fingerprint without an image codec anywhere. */
static uint64_t copy_in_digest(struct core *c, uint64_t *out_bytes) {
	uint64_t hash = 1469598103934665603ull;
	size_t row_bytes = (size_t)c->pending.width * 4;
	uint8_t *row = malloc(row_bytes);
	if (row == NULL) {
		fail(c, "out of memory in copy-in");
		return 0;
	}
	uint64_t total = 0;
	for (uint32_t y = 0; y < c->pending.height; y++) {
		size_t got = 0;
		while (got < row_bytes) {
			ssize_t n = pread(c->pending.fd, row + got, row_bytes - got,
				(off_t)((uint64_t)y * c->pending.stride + got));
			if (n <= 0) {
				fail(c, "invalid_buffer: copy-in short read at row %u", y);
				free(row);
				return 0;
			}
			got += (size_t)n;
		}
		for (size_t i = 0; i < row_bytes; i++) {
			hash ^= row[i];
			hash *= 1099511628211ull;
		}
		total += row_bytes;
	}
	free(row);
	*out_bytes = total;
	return hash;
}

static void handle_commit(struct core *c, const uint8_t *f, size_t len) {
	uint32_t oid = 0;
	vitrin_shim_surface_req_commit_t req;
	vitrin_decode_status_t st = vitrin_shim_surface_req_commit_decode(f, len, -1, &oid, &req);
	if (st != VITRIN_DECODE_OK) {
		fail(c, "commit decode: %s", vitrin_decode_status_string(st));
		return;
	}
	if (!c->ever_attached) {
		fail(c, "bad_order: commit against a never-attached surface");
		return;
	}

	uint64_t area = 0;
	for (int i = 0; i < c->damage_len; i++) {
		if (c->damage[i].w > 0 && c->damage[i].h > 0) {
			area += (uint64_t)c->damage[i].w * (uint64_t)c->damage[i].h;
		}
	}
	c->last_damage_area = area;
	c->last_damage_rects = c->damage_len;

	uint64_t digest = 0, bytes = 0;
	uint32_t committed_id = 0;
	uint64_t surface_area = 0;
	if (c->pending.present) {
		digest = copy_in_digest(c, &bytes);
		committed_id = c->pending.buffer_id;
		surface_area = (uint64_t)c->pending.width * c->pending.height;

		/* Release the previous retained buffer first, preserving attach
		 * order along the release chain (the core's own rule). */
		if (c->defer_release && c->retained_id != 0) {
			close(c->retained_fd);
			send_buffer_done(c, c->retained_id,
				VITRIN_SHIM_SURFACE_BUFFER_STATUS_RELEASED);
			c->retained_id = 0;
			c->retained_fd = -1;
		}
		if (c->defer_release) {
			c->retained_id = c->pending.buffer_id;
			c->retained_fd = c->pending.fd;
		} else {
			close(c->pending.fd);
			send_buffer_done(c, c->pending.buffer_id,
				VITRIN_SHIM_SURFACE_BUFFER_STATUS_RELEASED);
		}
		c->pending.present = false;
	}

	c->commits++;
	c->inflight++;
	if (c->inflight > c->max_inflight) {
		c->max_inflight = c->inflight;
	}
	trace("EV commit n=%llu buffer_id=%u rects=%d damage_area=%llu surface_area=%llu "
		"bytes=%llu digest=%016llx inflight=%d",
		(unsigned long long)c->commits, committed_id, c->damage_len,
		(unsigned long long)area, (unsigned long long)surface_area,
		(unsigned long long)bytes, (unsigned long long)digest, c->inflight);

	c->damage_len = 0;

	/* Exactly one frame_done owed per commit, sent after `frame_delay_ms`
	 * so a deliberately slow core is expressible. */
	if (c->frame_delay_ms <= 0) {
		send_frame_done(c);
	} else {
		c->frame_due = true;
		clock_gettime(CLOCK_MONOTONIC, &c->frame_at);
		c->frame_at.tv_sec += c->frame_delay_ms / 1000;
		c->frame_at.tv_nsec += (long)(c->frame_delay_ms % 1000) * 1000000L;
		if (c->frame_at.tv_nsec >= 1000000000L) {
			c->frame_at.tv_sec++;
			c->frame_at.tv_nsec -= 1000000000L;
		}
	}
}

static void dispatch(struct core *c, const uint8_t *f, size_t len, int fd) {
	vitrin_frame_header_t hdr;
	if (vitrin_frame_header_decode(f, len, &hdr) != VITRIN_DECODE_OK) {
		fail(c, "undecodable header");
		return;
	}
	if (hdr.object_id == 1) {
		if (hdr.opcode == VITRIN_SHIM_SESSION_REQ_CREATE_SURFACE_OPCODE) {
			handle_create_surface(c, f, len);
		} else if (hdr.opcode == VITRIN_SHIM_SESSION_REQ_GET_SEAT_OPCODE) {
			handle_get_seat(c, f, len);
		} else {
			fail(c, "unknown session opcode %u", hdr.opcode);
		}
		if (fd >= 0) {
			close(fd);
			fail(c, "fd_violation: session request carried an fd");
		}
		return;
	}
	if (c->surface_created && hdr.object_id == c->surface_id) {
		if (hdr.opcode == VITRIN_SHIM_SURFACE_REQ_ATTACH_OPCODE) {
			handle_attach(c, f, len, fd);
			return; /* attach owns the fd */
		}
		if (fd >= 0) {
			close(fd);
			fail(c, "fd_violation: non-attach surface request carried an fd");
			return;
		}
		if (hdr.opcode == VITRIN_SHIM_SURFACE_REQ_DAMAGE_OPCODE) {
			handle_damage(c, f, len);
		} else if (hdr.opcode == VITRIN_SHIM_SURFACE_REQ_COMMIT_OPCODE) {
			handle_commit(c, f, len);
		} else {
			fail(c, "unknown surface opcode %u", hdr.opcode);
		}
		return;
	}
	if (fd >= 0) {
		close(fd);
	}
	fail(c, "invalid_object: request for unknown object %u", hdr.object_id);
}

/* ---- receive --------------------------------------------------------- */

/* Returns 1 on progress, 0 on EOF, -1 on error. Positional fd matching: the
 * fd delivered by a recvmsg belongs to the first frame in that batch that
 * declares one, which is exactly what the real transport enforces. */
static int core_fill(struct core *c) {
	union {
		struct cmsghdr align;
		char buf[CMSG_SPACE(sizeof(int) * 4)];
	} control;
	memset(&control, 0, sizeof(control));
	struct iovec iov = {
		.iov_base = c->rx + c->rx_len,
		.iov_len = sizeof(c->rx) - c->rx_len,
	};
	struct msghdr msg = {
		.msg_iov = &iov,
		.msg_iovlen = 1,
		.msg_control = control.buf,
		.msg_controllen = sizeof(control.buf),
	};
	ssize_t n;
	do {
		n = recvmsg(c->fd, &msg, MSG_CMSG_CLOEXEC);
	} while (n < 0 && errno == EINTR);
	if (n < 0) {
		return (errno == EAGAIN || errno == EWOULDBLOCK) ? 1 : -1;
	}
	if (n == 0) {
		return 0;
	}

	int fds[8];
	int nfds = 0;
	for (struct cmsghdr *cm = CMSG_FIRSTHDR(&msg); cm != NULL; cm = CMSG_NXTHDR(&msg, cm)) {
		if (cm->cmsg_level != SOL_SOCKET || cm->cmsg_type != SCM_RIGHTS) {
			continue;
		}
		size_t payload = cm->cmsg_len - CMSG_LEN(0);
		for (size_t off = 0; off + sizeof(int) <= payload && nfds < 8; off += sizeof(int)) {
			memcpy(&fds[nfds++], (char *)CMSG_DATA(cm) + off, sizeof(int));
		}
	}
	c->rx_len += (size_t)n;

	size_t pos = 0;
	int fd_next = 0;
	while (c->rx_len - pos >= VITRIN_HEADER_LEN) {
		vitrin_frame_header_t hdr;
		if (vitrin_frame_header_decode(c->rx + pos, c->rx_len - pos, &hdr) != VITRIN_DECODE_OK) {
			fail(c, "undecodable header");
			return -1;
		}
		if (hdr.size < VITRIN_HEADER_LEN) {
			fail(c, "oversized: frame size %u below the 8-byte header minimum", hdr.size);
			return -1;
		}
		if (c->rx_len - pos < hdr.size) {
			break;
		}
		int fd = -1;
		if (hdr.fd_count == 1) {
			if (fd_next < nfds) {
				fd = fds[fd_next++];
			} else {
				fail(c, "fd_violation: frame declares an fd but none was delivered");
			}
		}
		dispatch(c, c->rx + pos, hdr.size, fd);
		pos += hdr.size;
	}
	if (fd_next < nfds) {
		fail(c, "fd_violation: %d fd(s) delivered that no frame claimed", nfds - fd_next);
		while (fd_next < nfds) {
			close(fds[fd_next++]);
		}
	}
	memmove(c->rx, c->rx + pos, c->rx_len - pos);
	c->rx_len -= pos;
	return 1;
}

/* ---- spawn ----------------------------------------------------------- */

/* The P1.5.2 spawn contract, reproduced exactly: the shim's socketpair end
 * lands on fd 3 with FD_CLOEXEC CLEARED (dup2 clears it on the new
 * descriptor), announced by nothing -- no environment variable, no argv. */
static pid_t spawn_shim(int shim_end, char **argv) {
	pid_t pid = fork();
	if (pid != 0) {
		return pid;
	}
	if (shim_end != CORE_FD) {
		if (dup2(shim_end, CORE_FD) < 0) {
			_exit(127);
		}
		close(shim_end);
	} else {
		int fl = fcntl(CORE_FD, F_GETFD);
		if (fl < 0 || fcntl(CORE_FD, F_SETFD, fl & ~FD_CLOEXEC) < 0) {
			_exit(127);
		}
	}
	execv(argv[0], argv);
	_exit(127);
}

static void usage(void) {
	fprintf(stderr,
		"usage: mock-core [--realm NAME] [--size WxH] [--frames N] [--run-ms MS]\n"
		"                 [--frame-delay MS] [--defer-release] -- SHIM_ARGV...\n");
	exit(2);
}

int main(int argc, char **argv) {
	const char *realm = "realm-0";
	uint32_t width = 800, height = 600;
	uint64_t want_frames = 0;
	int run_ms = 10000;
	int frame_delay = 0;
	bool defer_release = false;
	int shim_argi = 0;

	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--realm") == 0 && i + 1 < argc) {
			realm = argv[++i];
		} else if (strcmp(argv[i], "--size") == 0 && i + 1 < argc) {
			if (sscanf(argv[++i], "%ux%u", &width, &height) != 2) {
				usage();
			}
		} else if (strcmp(argv[i], "--frames") == 0 && i + 1 < argc) {
			want_frames = strtoull(argv[++i], NULL, 10);
		} else if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--frame-delay") == 0 && i + 1 < argc) {
			frame_delay = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--defer-release") == 0) {
			defer_release = true;
		} else if (strcmp(argv[i], "--") == 0 && i + 1 < argc) {
			shim_argi = i + 1;
			break;
		} else {
			usage();
		}
	}
	if (shim_argi == 0) {
		usage();
	}

	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);
	signal(SIGPIPE, SIG_IGN);

	int sv[2];
	if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) != 0) {
		perror("socketpair");
		return 1;
	}

	pid_t shim = spawn_shim(sv[1], &argv[shim_argi]);
	if (shim < 0) {
		perror("fork");
		return 1;
	}
	close(sv[1]);

	struct core c = {0};
	c.fd = sv[0];
	c.watermark = 1;
	c.frame_delay_ms = frame_delay;
	c.defer_release = defer_release;
	c.retained_fd = -1;
	c.pending.fd = -1;

	trace("EV spawned shim_pid=%d", (int)shim);

	/* The core's first message, before anything is read. */
	send_configure(&c, realm, width, height);
	trace("EV configure realm=%s width=%u height=%u", realm, width, height);

	int64_t deadline = now_ms() + run_ms;
	int exit_code = 0;
	while (!g_stop) {
		int64_t remaining = deadline - now_ms();
		if (remaining <= 0) {
			break;
		}
		if (want_frames > 0 && c.commits >= want_frames && !c.frame_due) {
			break;
		}

		int timeout = remaining > 50 ? 50 : (int)remaining;
		struct pollfd pfd = {.fd = c.fd, .events = POLLIN};
		int pr = poll(&pfd, 1, timeout);
		if (pr < 0 && errno != EINTR) {
			perror("poll");
			exit_code = 1;
			break;
		}
		if (pr > 0) {
			int r = core_fill(&c);
			if (r == 0) {
				trace("EV shim_eof");
				break;
			}
			if (r < 0) {
				exit_code = 1;
				break;
			}
		}
		if (c.frame_due) {
			struct timespec now;
			clock_gettime(CLOCK_MONOTONIC, &now);
			if (now.tv_sec > c.frame_at.tv_sec ||
					(now.tv_sec == c.frame_at.tv_sec && now.tv_nsec >= c.frame_at.tv_nsec)) {
				c.frame_due = false;
				send_frame_done(&c);
			}
		}
	}

	trace("SUMMARY commits=%llu frame_dones=%llu buffer_dones=%llu max_inflight=%d "
		"last_damage_area=%llu last_damage_rects=%d failures=%d",
		(unsigned long long)c.commits, (unsigned long long)c.frame_dones,
		(unsigned long long)c.buffer_dones, c.max_inflight,
		(unsigned long long)c.last_damage_area, c.last_damage_rects, c.failures);

	/* Hang up: socketpair EOF is how the core tells a shim to go away, and
	 * the first rung of the P1.5.3 shutdown ladder. */
	close(c.fd);
	if (c.pending.present) {
		close(c.pending.fd);
	}
	if (c.retained_fd >= 0) {
		close(c.retained_fd);
	}

	int status = 0;
	for (int i = 0; i < 50; i++) {
		pid_t r = waitpid(shim, &status, WNOHANG);
		if (r == shim) {
			break;
		}
		struct timespec ts = {.tv_sec = 0, .tv_nsec = 20000000L};
		nanosleep(&ts, NULL);
		if (i == 25) {
			kill(shim, SIGTERM);
		}
	}
	if (waitpid(shim, &status, WNOHANG) == 0) {
		kill(shim, SIGKILL);
		waitpid(shim, &status, 0);
	}

	if (c.failures > 0) {
		exit_code = 1;
	}
	return exit_code;
}
