/* test_wire_designation.c -- the shim transport's RECEIVE side for
 * `vitrin_shim_session.designation`, the first and so far only core -> shim
 * event that carries a file descriptor (P2.6.5, issue #189).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS IS A UNIT TEST AND NOT ONLY AN ACCEPTANCE SCRIPT. Four of the seven
 * facts below are reachable only from a core that is buggy or hostile -- an fd
 * on a frame that declares none, a frame that declares one with nothing
 * attached, two fds on a one-fd frame, a descriptor left pending when the
 * connection ends. `mock_core.c` could be made to produce each, but one
 * process per violation, each proved by grepping a log, is a slower and
 * blunter instrument than calling the transport directly over a socketpair.
 * The acceptance script beside this (tests/acceptance/designation_receive.sh)
 * covers the fact this cannot: that the SHIPPED shim binary survives a real
 * designation and closes the descriptor.
 *
 * THE LEAK ASSERTIONS ARE REAL MEASUREMENTS, not `close()` call counts. Every
 * scenario designates one unlinked temp file and then counts, through
 * /proc/self/fd, how many descriptors in this process still refer to that
 * inode. The test's own is always one of them, so "the transport is holding
 * nothing" is `refs == 1` -- a claim about the process's actual descriptor
 * table rather than about the code path a reader believes ran. It is why
 * `open_refs()` failing is a hard error here: a leak check that cannot see
 * /proc would pass every scenario while measuring nothing.
 *
 * This links src/wire.c directly. It never calls `vitrin_wire_adopt`, which
 * insists on fd 3; the wire's descriptor is set by hand from a socketpair,
 * which is the only difference from how the shim itself uses this transport.
 */
#define _POSIX_C_SOURCE 200809L

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

#include <wayland-server-core.h>

#include "vitrin-protocol.h"
#include "wire.h"

/* ---- reporting -------------------------------------------------------- */

static int g_failures;
static const char *g_scenario = "(none)";

static void fail(const char *fmt, ...) {
	va_list ap;
	fprintf(stderr, "FAIL [%s]: ", g_scenario);
	va_start(ap, fmt);
	vfprintf(stderr, fmt, ap);
	va_end(ap);
	fputc('\n', stderr);
	g_failures++;
}

#define CHECK(cond, ...) \
	do { \
		if (!(cond)) { \
			fail(__VA_ARGS__); \
		} \
	} while (0)

/* ---- how many descriptors in this process name one file ---------------- */

/* Counts descriptors in THIS process referring to the same open file as `want`
 * describes. The scenarios below hold exactly one themselves, so `1` means the
 * transport is holding nothing and `2` means it (or the handler) kept one.
 *
 * Returns -1 if it could not look, which every caller treats as a hard failure
 * rather than as zero: a leak check that silently inspects nothing is the
 * shape of defect this repository names most often. */
static int open_refs(const struct stat *want) {
	DIR *d = opendir("/proc/self/fd");
	if (d == NULL) {
		return -1;
	}
	int skip = dirfd(d);
	int n = 0;
	struct dirent *e;
	while ((e = readdir(d)) != NULL) {
		char *end = NULL;
		long v = strtol(e->d_name, &end, 10);
		if (end == e->d_name || *end != '\0') {
			continue;
		}
		if ((int)v == skip) {
			continue;
		}
		struct stat st;
		if (fstat((int)v, &st) != 0) {
			continue;
		}
		if (st.st_dev == want->st_dev && st.st_ino == want->st_ino) {
			n++;
		}
	}
	closedir(d);
	return n;
}

/* The file every designation in this test hands over: created, stat'd and
 * immediately unlinked, so the inode `open_refs` counts against exists only as
 * long as some descriptor names it. */
static int g_subject_fd = -1;
static struct stat g_subject;

static bool subject_open(void) {
	char path[] = "/tmp/vitrin-designation-XXXXXX";
	const char *dir = getenv("TMPDIR");
	char buf[512];
	if (dir != NULL && dir[0] != '\0') {
		snprintf(buf, sizeof(buf), "%s/vitrin-designation-XXXXXX", dir);
	} else {
		snprintf(buf, sizeof(buf), "%s", path);
	}
	g_subject_fd = mkstemp(buf);
	if (g_subject_fd < 0) {
		fprintf(stderr, "FAIL: cannot create a temp file to designate: %s\n",
			strerror(errno));
		return false;
	}
	unlink(buf);
	if (write(g_subject_fd, "vitrin", 6) != 6) {
		fprintf(stderr, "FAIL: cannot write the temp file\n");
		return false;
	}
	if (fstat(g_subject_fd, &g_subject) != 0) {
		fprintf(stderr, "FAIL: cannot fstat the temp file\n");
		return false;
	}
	return true;
}

/* ---- the fixture ------------------------------------------------------- */

struct seen {
	int calls;
	/* The descriptor number the handler was handed on the LAST call, or -1. */
	int fd_seen;
	/* Whether the handler claims what it is handed (scenario-controlled). */
	bool claim;
	int claimed_fd;
	/* Whether the handler declares the connection dead (scenario-controlled). */
	bool refuse;

	bool decoded;
	vitrin_decode_status_t decode_st;
	uint32_t designation_id;
	vitrin_powerbox_kind_t kind;
	vitrin_powerbox_mode_t mode;
	char name[256];
	uint32_t name_len;

	int closed_calls;
};

static bool test_handler(void *data, const uint8_t *frame, size_t len, int *fd) {
	struct seen *s = data;
	s->calls++;
	s->fd_seen = *fd;

	vitrin_frame_header_t hdr;
	if (vitrin_frame_header_decode(frame, len, &hdr) == VITRIN_DECODE_OK &&
			hdr.opcode == VITRIN_SHIM_SESSION_EVT_DESIGNATION_OPCODE &&
			hdr.object_id == 1u) {
		uint32_t object_id = 0;
		vitrin_shim_session_evt_designation_t ev;
		s->decode_st = vitrin_shim_session_evt_designation_decode(
			frame, len, *fd, &object_id, &ev);
		if (s->decode_st == VITRIN_DECODE_OK) {
			s->decoded = true;
			s->designation_id = ev.designation_id;
			s->kind = ev.kind;
			s->mode = ev.mode;
			s->name_len = ev.name.len;
			memcpy(s->name, ev.name.data, ev.name.len);
			s->name[ev.name.len] = '\0';
		}
	}

	if (s->claim && *fd >= 0) {
		s->claimed_fd = *fd;
		*fd = -1;
	}
	return !s->refuse;
}

static void test_closed(void *data) {
	struct seen *s = data;
	s->closed_calls++;
}

struct fixture {
	int core; /* the core's end of the socketpair */
	struct wl_event_loop *loop;
	struct vitrin_wire wire;
	struct seen seen;
};

/* Builds the connection WITHOUT arming it, so a scenario can drive
 * `vitrin_wire_recv_sync` first -- which is the order the real shim uses. */
static struct fixture *fx_new(bool claim) {
	struct fixture *f = calloc(1, sizeof(*f));
	if (f == NULL) {
		fprintf(stderr, "FAIL: out of memory\n");
		exit(1);
	}
	int sv[2];
	if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) != 0) {
		fprintf(stderr, "FAIL: socketpair: %s\n", strerror(errno));
		exit(1);
	}
	f->core = sv[0];
	f->wire.fd = sv[1];
	int fl = fcntl(sv[1], F_GETFL);
	if (fl < 0 || fcntl(sv[1], F_SETFL, fl | O_NONBLOCK) != 0) {
		fprintf(stderr, "FAIL: O_NONBLOCK: %s\n", strerror(errno));
		exit(1);
	}
	f->loop = wl_event_loop_create();
	f->seen.fd_seen = -1;
	f->seen.claimed_fd = -1;
	f->seen.claim = claim;
	return f;
}

/* Returns what `vitrin_wire_arm` returned. That is not always true: arming
 * dispatches whatever the synchronous read left buffered, so a violation
 * already sitting in that buffer is declared here rather than on a later
 * wakeup. Scenarios with an empty buffer use `fx_arm_ok`; scenario (8) expects
 * the refusal and checks for it. */
static bool fx_arm(struct fixture *f) {
	return vitrin_wire_arm(&f->wire, f->loop, test_handler, NULL, test_closed, &f->seen);
}

static void fx_arm_ok(struct fixture *f) {
	if (!fx_arm(f)) {
		fail("vitrin_wire_arm refused a connection with nothing buffered");
	}
}

static void fx_pump(struct fixture *f) {
	wl_event_loop_dispatch(f->loop, 200);
}

static void fx_free(struct fixture *f) {
	vitrin_wire_finish(&f->wire);
	wl_event_loop_destroy(f->loop);
	close(f->core);
	free(f);
}

/* ---- the core side ----------------------------------------------------- */

/* One sendmsg carrying `len` bytes and `nfds` descriptors, exactly as the real
 * core's `Connection::send_message` does it. Blocking: the socketpair's buffer
 * dwarfs every frame here. */
static void core_send(struct fixture *f, const uint8_t *bytes, size_t len,
		const int *fds, size_t nfds) {
	union {
		struct cmsghdr align;
		char buf[CMSG_SPACE(sizeof(int) * 4)];
	} control;
	memset(&control, 0, sizeof(control));
	struct iovec iov = {.iov_base = (void *)(uintptr_t)bytes, .iov_len = len};
	struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1};
	if (nfds > 0) {
		msg.msg_control = control.buf;
		msg.msg_controllen = CMSG_SPACE(sizeof(int) * nfds);
		struct cmsghdr *cm = CMSG_FIRSTHDR(&msg);
		cm->cmsg_level = SOL_SOCKET;
		cm->cmsg_type = SCM_RIGHTS;
		cm->cmsg_len = CMSG_LEN(sizeof(int) * nfds);
		memcpy(CMSG_DATA(cm), fds, sizeof(int) * nfds);
	}
	ssize_t n;
	do {
		n = sendmsg(f->core, &msg, MSG_NOSIGNAL);
	} while (n < 0 && errno == EINTR);
	if (n != (ssize_t)len) {
		fail("core sendmsg wrote %zd of %zu bytes: %s", n, len, strerror(errno));
	}
}

#define DESIGNATION_ID 4242u
static const char DESIGNATION_NAME[] = "quarterly.csv";

static size_t encode_designation(uint8_t *out, size_t cap) {
	vitrin_shim_session_evt_designation_t ev = {
		/* Never written to the byte buffer; it rides SCM_RIGHTS. Set to -1 so
		 * nothing here can accidentally depend on the encoder reading it. */
		.fd = -1,
		.designation_id = DESIGNATION_ID,
		.kind = VITRIN_POWERBOX_KIND_FILE,
		.mode = VITRIN_POWERBOX_MODE_READ,
		.name = {
			.len = (uint32_t)strlen(DESIGNATION_NAME),
			.data = (const uint8_t *)DESIGNATION_NAME,
		},
	};
	int32_t n = vitrin_shim_session_evt_designation_encode(&ev, 1u, out, cap);
	if (n < 0) {
		fail("cannot encode a designation frame (%d)", n);
		return 0;
	}
	return (size_t)n;
}

static size_t encode_frame_done(uint8_t *out, size_t cap) {
	vitrin_shim_surface_evt_frame_done_t ev = {.time_ms = 7u};
	int32_t n = vitrin_shim_surface_evt_frame_done_encode(&ev, 2u, out, cap);
	if (n < 0) {
		fail("cannot encode a frame_done frame (%d)", n);
		return 0;
	}
	return (size_t)n;
}

static size_t encode_configure(uint8_t *out, size_t cap) {
	static const char realm[] = "realm-test";
	vitrin_shim_session_evt_configure_t ev = {
		.realm = {.len = (uint32_t)strlen(realm), .data = (const uint8_t *)realm},
		.width = 640u,
		.height = 480u,
	};
	int32_t n = vitrin_shim_session_evt_configure_encode(&ev, 1u, out, cap);
	if (n < 0) {
		fail("cannot encode a configure frame (%d)", n);
		return 0;
	}
	return (size_t)n;
}

/* Every scenario asserts against this: the transport is holding nothing of the
 * designated file, and the leak check actually looked. */
static void check_refs(int want, const char *what) {
	int refs = open_refs(&g_subject);
	if (refs < 0) {
		fail("cannot read /proc/self/fd, so the leak check inspected nothing (%s)", what);
		return;
	}
	CHECK(refs == want, "%s: expected %d descriptor(s) on the designated file, found %d",
		what, want, refs);
}

/* ---- the scenarios ----------------------------------------------------- */

/* (1) The whole point: a designation arrives, its arguments decode, and the
 * descriptor the handler claims names the file the core sent. */
static void scenario_delivered(void) {
	g_scenario = "delivered and claimed";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);
	check_refs(1, "before");

	uint8_t frame[512];
	size_t len = encode_designation(frame, sizeof(frame));
	int fd = g_subject_fd;
	core_send(f, frame, len, &fd, 1);
	fx_pump(f);

	CHECK(f->seen.calls == 1, "handler called %d times, expected 1", f->seen.calls);
	CHECK(f->seen.decoded, "the designation did not decode: %s",
		vitrin_decode_status_string(f->seen.decode_st));
	CHECK(f->seen.designation_id == DESIGNATION_ID, "designation_id %u, expected %u",
		f->seen.designation_id, DESIGNATION_ID);
	CHECK(f->seen.kind == VITRIN_POWERBOX_KIND_FILE, "kind %d", (int)f->seen.kind);
	CHECK(f->seen.mode == VITRIN_POWERBOX_MODE_READ, "mode %d", (int)f->seen.mode);
	CHECK(strcmp(f->seen.name, DESIGNATION_NAME) == 0, "name \"%s\", expected \"%s\"",
		f->seen.name, DESIGNATION_NAME);
	CHECK(f->seen.claimed_fd >= 0, "the handler was handed no descriptor");

	if (f->seen.claimed_fd >= 0) {
		struct stat got;
		CHECK(fstat(f->seen.claimed_fd, &got) == 0, "the claimed descriptor is not open");
		CHECK(got.st_dev == g_subject.st_dev && got.st_ino == g_subject.st_ino,
			"the claimed descriptor names a different file than the core sent");
		check_refs(2, "while the handler holds it");
		close(f->seen.claimed_fd);
	}
	CHECK(vitrin_wire_alive(&f->wire), "the connection died on a legitimate designation");
	CHECK(f->seen.closed_calls == 0, "the connection reported itself closed");
	check_refs(1, "after the handler closed its own");
	fx_free(f);
}

/* (2) The IDL's "a shim that cannot relay a descriptor closes it", made
 * structural: a handler that does not claim leaks nothing, because the
 * transport closes what it left. */
static void scenario_unclaimed_is_closed(void) {
	g_scenario = "unclaimed descriptor is closed by the transport";
	struct fixture *f = fx_new(false);
	fx_arm_ok(f);

	uint8_t frame[512];
	size_t len = encode_designation(frame, sizeof(frame));
	int fd = g_subject_fd;
	core_send(f, frame, len, &fd, 1);
	fx_pump(f);

	CHECK(f->seen.calls == 1, "handler called %d times, expected 1", f->seen.calls);
	CHECK(f->seen.fd_seen >= 0, "the handler was handed no descriptor");
	if (f->seen.fd_seen >= 0) {
		CHECK(fcntl(f->seen.fd_seen, F_GETFD) < 0 && errno == EBADF,
			"descriptor %d is still open after the handler declined it",
			f->seen.fd_seen);
	}
	check_refs(1, "after an unclaimed designation");
	CHECK(vitrin_wire_alive(&f->wire), "declining a descriptor killed the connection");
	fx_free(f);
}

/* (3) A frame that declares a descriptor with nothing attached: the header and
 * the ancillary data disagree, which conventions 2.4 makes fatal. The handler
 * must never see the frame -- a `designation` with no fd is not a designation
 * with a missing file, it is a desynchronized stream. */
static void scenario_declared_but_absent(void) {
	g_scenario = "declared fd, none delivered";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);

	uint8_t frame[512];
	size_t len = encode_designation(frame, sizeof(frame));
	core_send(f, frame, len, NULL, 0);
	fx_pump(f);

	CHECK(f->seen.calls == 0, "the handler saw a frame whose descriptor never arrived");
	CHECK(!vitrin_wire_alive(&f->wire), "the connection survived an fd_violation");
	CHECK(f->seen.closed_calls == 1, "the connection did not report itself closed (%d)",
		f->seen.closed_calls);
	fx_free(f);
}

/* (4) A descriptor smuggled onto a frame that declares none. Fatal, and the
 * descriptor must not survive the fatal -- this is the path on which a leak
 * would be invisible, because nothing downstream ever hears about the frame. */
static void scenario_unsolicited(void) {
	g_scenario = "fd on a frame that declares none";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);

	uint8_t frame[64];
	size_t len = encode_frame_done(frame, sizeof(frame));
	int fd = g_subject_fd;
	core_send(f, frame, len, &fd, 1);
	fx_pump(f);

	CHECK(f->seen.calls == 0, "the handler was given a frame carrying a smuggled fd");
	CHECK(!vitrin_wire_alive(&f->wire), "the connection survived an unsolicited fd");
	check_refs(1, "after an unsolicited fd");
	fx_free(f);
}

/* (5) Two descriptors on a one-fd frame. The one-fd-per-message rule is a
 * FRAMING invariant (conventions 2.4), so the second is a violation even
 * though the frame legitimately declares one -- and neither may survive. */
static void scenario_second_fd(void) {
	g_scenario = "two fds on a one-fd frame";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);

	uint8_t frame[512];
	size_t len = encode_designation(frame, sizeof(frame));
	int fds[2] = {g_subject_fd, g_subject_fd};
	core_send(f, frame, len, fds, 2);
	fx_pump(f);

	CHECK(f->seen.calls == 0, "the handler was given a frame carrying a second fd");
	CHECK(!vitrin_wire_alive(&f->wire), "the connection survived a second fd");
	check_refs(1, "after a doubled fd");
	fx_free(f);
}

/* (6) A descriptor whose frame is still arriving when the connection ends.
 * Nothing is wrong here -- the fd is legitimately pending -- so the fatal
 * path's cleanup does not run and teardown is the only thing that can close
 * it. A realm exiting must not leave the file open. */
static void scenario_pending_at_teardown(void) {
	g_scenario = "pending fd at teardown";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);

	uint8_t frame[512];
	(void)encode_designation(frame, sizeof(frame));
	int fd = g_subject_fd;
	core_send(f, frame, 4, &fd, 1); /* half a header: no frame can complete */
	fx_pump(f);

	CHECK(f->seen.calls == 0, "a frame completed out of four bytes");
	CHECK(vitrin_wire_alive(&f->wire), "a partial frame killed the connection");
	check_refs(2, "while the frame is still arriving");

	vitrin_wire_finish(&f->wire);
	check_refs(1, "after teardown");
	fx_free(f);
}

/* (7) The same descriptor, delivered by an earlier recvmsg than the one that
 * completes its frame. This is why pending descriptors carry the byte-stream
 * SPAN of their delivering recvmsg rather than an attach offset: the kernel
 * hands over ancillary data per sendmsg, and a frame split across two of them
 * still owns the fd that arrived with its first byte. */
static void scenario_split_frame(void) {
	g_scenario = "fd delivered with a partial frame";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);

	uint8_t frame[512];
	size_t len = encode_designation(frame, sizeof(frame));
	int fd = g_subject_fd;
	core_send(f, frame, 5, &fd, 1);
	fx_pump(f); /* forces a recvmsg that sees the fd and no whole frame */
	CHECK(f->seen.calls == 0, "a frame completed out of five bytes");

	core_send(f, frame + 5, len - 5, NULL, 0);
	fx_pump(f);

	CHECK(f->seen.calls == 1, "handler called %d times, expected 1", f->seen.calls);
	CHECK(f->seen.decoded, "the split designation did not decode: %s",
		vitrin_decode_status_string(f->seen.decode_st));
	CHECK(f->seen.claimed_fd >= 0, "the descriptor delivered with the first half was lost");
	CHECK(vitrin_wire_alive(&f->wire), "a split designation killed the connection");
	if (f->seen.claimed_fd >= 0) {
		close(f->seen.claimed_fd);
	}
	check_refs(1, "after a split designation");
	fx_free(f);
}

/* (8) An fd smuggled onto the frame BEHIND `configure`, in the same sendmsg.
 *
 * This is the one scenario that needs `vitrin_wire_recv_sync` -- the shim's
 * single synchronous read -- and it is here because that function removes
 * bytes from the reassembly buffer outside the dispatch loop. Pending
 * descriptors are matched against byte-stream offsets, so a synchronous read
 * that advanced the buffer without advancing the stream offset would leave the
 * two counters disagreeing by exactly `configure`'s size, and the smuggled fd
 * below would be judged to belong to some later frame instead of being caught
 * here. It would then be handed to a handler as a legitimate designation. */
static void scenario_smuggled_behind_configure(void) {
	g_scenario = "fd smuggled behind the synchronous configure read";
	struct fixture *f = fx_new(true);

	uint8_t batch[512];
	size_t c_len = encode_configure(batch, sizeof(batch));
	size_t d_len = encode_frame_done(batch + c_len, sizeof(batch) - c_len);
	int fd = g_subject_fd;
	core_send(f, batch, c_len + d_len, &fd, 1);

	uint8_t out[512];
	size_t out_len = 0;
	bool got = vitrin_wire_recv_sync(&f->wire, 1000, out, sizeof(out), &out_len);
	CHECK(got, "the synchronous read did not return configure");
	CHECK(out_len == c_len, "the synchronous read returned %zu bytes, expected %zu",
		out_len, c_len);

	/* The violation is already in the reassembly buffer by here, so it is
	 * ARMING -- which drains what the synchronous read left behind -- that
	 * declares it, not a later wakeup. A `true` here would mean the frame was
	 * stranded in the buffer with nothing left in the socket to wake for it. */
	bool armed = fx_arm(f);
	fx_pump(f);

	CHECK(!armed, "arming accepted a connection whose buffered frame carries a "
		"smuggled descriptor");
	CHECK(f->seen.calls == 0, "the handler was given the frame carrying the smuggled fd");
	CHECK(!vitrin_wire_alive(&f->wire),
		"the connection survived an fd attached to a frame that declares none");
	check_refs(1, "after a smuggled fd behind configure");
	fx_free(f);
}

/* (10) A handler that declares the connection dead WHILE holding a descriptor
 * it did not claim. wire.h promises the transport closes an unclaimed fd "on
 * every path including a `false` return", and that path is not hypothetical:
 * it is exactly what `upstream.c` does with `fd_violation`'s first disjunct --
 * a descriptor on a frame whose SIGNATURE declares none, which the transport
 * cannot see because the header and the ancillary data agree. So the refusal
 * and the close have to happen in that order, and only a test that refuses
 * while an fd is in flight can tell the two orderings apart.
 *
 * Both halves are asserted: the descriptor is gone, and the connection is
 * dead rather than left alive to re-dispatch the same bytes forever. */
static void scenario_refused_with_fd(void) {
	g_scenario = "handler refuses while holding a descriptor";
	struct fixture *f = fx_new(false);
	f->seen.refuse = true;
	fx_arm_ok(f);

	uint8_t frame[512];
	size_t len = encode_designation(frame, sizeof(frame));
	int fd = g_subject_fd;
	core_send(f, frame, len, &fd, 1);
	fx_pump(f);

	CHECK(f->seen.calls == 1, "handler called %d times, expected 1", f->seen.calls);
	CHECK(f->seen.fd_seen >= 0, "the handler was handed no descriptor");
	check_refs(1, "after a handler refused while holding a descriptor");
	CHECK(!vitrin_wire_alive(&f->wire),
		"the connection survived a handler that declared it dead");
	CHECK(f->seen.closed_calls == 1, "the connection did not report itself closed (%d)",
		f->seen.closed_calls);
	fx_free(f);
}

/* (9) The ordinary path, unchanged: an fd-less event still reaches the handler
 * with no descriptor. A regression guard on everything above -- the machinery
 * must not have made every event look fd-bearing. */
static void scenario_fdless_still_works(void) {
	g_scenario = "an fd-less event still arrives";
	struct fixture *f = fx_new(true);
	fx_arm_ok(f);

	uint8_t frame[64];
	size_t len = encode_frame_done(frame, sizeof(frame));
	core_send(f, frame, len, NULL, 0);
	fx_pump(f);

	CHECK(f->seen.calls == 1, "handler called %d times, expected 1", f->seen.calls);
	CHECK(f->seen.fd_seen == -1, "an fd-less event was handed descriptor %d",
		f->seen.fd_seen);
	CHECK(vitrin_wire_alive(&f->wire), "an ordinary event killed the connection");
	fx_free(f);
}

int main(void) {
	/* Loud, because a leak reported by wire.c's own error path is the evidence
	 * for half the scenarios below. */
	if (!subject_open()) {
		return 1;
	}
	int refs = open_refs(&g_subject);
	if (refs != 1) {
		fprintf(stderr,
			"FAIL: the descriptor census reports %d references to a file this "
			"process has opened exactly once, so every leak assertion below "
			"would be measuring the wrong thing\n",
			refs);
		return 1;
	}

	scenario_delivered();
	scenario_unclaimed_is_closed();
	scenario_declared_but_absent();
	scenario_unsolicited();
	scenario_second_fd();
	scenario_pending_at_teardown();
	scenario_split_frame();
	scenario_smuggled_behind_configure();
	scenario_fdless_still_works();
	scenario_refused_with_fd();

	if (g_failures > 0) {
		fprintf(stderr, "FAIL: %d assertion(s) failed\n", g_failures);
		return 1;
	}
	printf("PASS: shim/tests/test_wire_designation.c (10 scenarios)\n");
	close(g_subject_fd);
	return 0;
}
