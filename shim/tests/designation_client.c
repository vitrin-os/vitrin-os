/* designation_client.c -- the APP side of the per-realm `designation.sock`
 * relay (P2.6.7, issue #191): a process the shim spawns as its app, which
 * connects to the socket, receives designations and writes down exactly what
 * it got.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * THIS IS A TEST INSTRUMENT, NOT A REFERENCE APP. The contract an application
 * is written against -- where the socket is, what one message on it is, how to
 * receive it -- is documented in `docs/protocol/09-vitrin_shim_session.md`
 * (CC-BY-4.0) and is decodable with the Apache-2.0 generated header
 * (`shim/include/vitrin-protocol.h`) alone; a third party copies the ~30-line
 * receive snippet in that prose page, not this file. Everything here beyond
 * that snippet exists to make the relay's failure modes OBSERVABLE from a
 * shell script: the fd census the shim is measured against needs an app that
 * is still alive and still connected at the moment of measurement, and the
 * drain, refusal and backpressure paths need an app that misbehaves on
 * request. Hence the flags, and hence the MPL-2.0 header: this is shim test
 * fixture, per NOTICE, and nothing an app author needs to read.
 *
 * WHY THE SHIM SPAWNS THIS, RATHER THAN THE SCRIPT RUNNING IT BESIDE THE SHIM.
 * The relay's claims are about the app's REAL situation: the environment the
 * shim composed for it (`XDG_RUNTIME_DIR`, which is where the socket is found,
 * by convention and by nothing else), and the descriptor table it inherited
 * across execve. Only a process that came through `vitrin_spawn_app` has that
 * situation, so this one is passed to the shim after `--` and the first thing
 * it does, before opening anything, is check that fd 3 -- the core link, the
 * one descriptor whose leak into the app would be the confinement gone -- is
 * CLOSED, and count what is open. A script-launched copy would inherit the
 * script's table and prove nothing about the shim's.
 *
 * WHY IT LINGERS. The shim's SIGCHLD handler terminates the realm when its app
 * exits (main.c, `handle_sigchld`), and every descriptor the shim might have
 * leaked is released by that exit. So an instrument that exited when done
 * would take the evidence with it: `--expect N` prints `DONE` after N
 * designations and then blocks in `pause()` until the shim's teardown SIGTERMs
 * it, which is what keeps `/proc/<shim>/fd` readable for the census the
 * acceptance script takes after the last `RX` line.
 *
 * WHY `--out` AND NEVER STDOUT. This process's stdout is the shim's, which is
 * the mock core's, which the script redirected to the core's log; a line
 * written there is a line spliced into another process's evidence
 * (gesture_probe.c records the observed splice at length). The file named by
 * `--out` is this process's alone, and every line is fsync'd so the script can
 * read it LIVE and sequence "the app has the descriptor" against the shim's
 * own ledger without waiting for anyone to exit.
 *
 * THE MISBEHAVIOUR FLAGS, and the relay rule each one exists to measure:
 *
 *   --send-garbage      write bytes on the socket after the first designation.
 *                       The socket accepts nothing; the shim must read once,
 *                       record the byte count and close (D8).
 *   --send-garbage-fd   the same, but the one byte carries an SCM_RIGHTS
 *                       descriptor on a file this process opened (path
 *                       printed). The shim drains with `read(2)`, never a
 *                       control-buffer `recvmsg`, so the kernel drops that
 *                       descriptor without installing it -- the script then
 *                       checks the shim's table for the file (D8).
 *   --second-connect    open a second connection while holding the first; it
 *                       must see EOF, not a hang, and the first must still
 *                       receive (D4: one connection, first holds).
 *   --second-connect-loop N   N of those in a row, so the ledger's per-kind
 *                       cap on `refused_occupied` records can be watched
 *                       overflowing into `suppressed_refused` (D9).
 *   --stall             connect, create --ready, then never read. The kernel
 *                       queue behind this connection fills, the shim's next
 *                       send fails and the shim drops the connection; this
 *                       process observes the hangup with poll() and lingers
 *                       (D6 -- the arm that catches a relay that frees the
 *                       slot on success and forgets to on failure).
 *   --hold              keep every received descriptor open. The default
 *                       closes each after use so this process's own table is
 *                       flat across a run, which is a second, independent
 *                       leak measurement.
 *
 * Every line is one record on one line, `KEY value=... value=...`, and the
 * basename arrives as the wire carried it; it is printed because the script
 * matches it against the mock core's `EV designation` line, and this file is
 * evidence, not a log a human is shown.
 */
#define _GNU_SOURCE

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
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
#include <sys/un.h>
#include <unistd.h>

#include "vitrin-protocol.h"

/* Every message on the socket is one `designation` frame: 8-byte header, three
 * u32s, a string of at most 255 bytes padded to 4, so at most 280 bytes. The
 * iov is deliberately larger than that: a message longer than the iov would
 * be TRUNCATED by SEQPACKET (MSG_TRUNC, the tail gone), and an instrument
 * should see an oversized message as an oversized message, not as a decode
 * error on a frame whose end it never received. */
#define RX_IOV_BYTES 512
/* What --write-probe writes at offset 0 on a read_write descriptor; the script
 * reads the subject back from disk and expects to find it there. */
#define WRITE_PROBE_MARKER "WRITTEN-BY-APP"

struct opts {
	const char *out;
	const char *ready;
	long expect;
	bool pread;
	bool write_probe;
	bool send_garbage;
	bool send_garbage_fd;
	int second_connect; /* 0 = none; --second-connect is 1; --second-connect-loop N is N */
	bool stall;
	bool hold;
};

static int g_out = -1;

/* One record, one line, one write, one fsync: the script tails this file while
 * this process is still running, and a line that reaches the page cache but
 * not the file's readable length is a race the script would lose. */
static void say(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *fmt, ...) {
	char line[1024];
	va_list ap;
	va_start(ap, fmt);
	int n = vsnprintf(line, sizeof(line) - 1, fmt, ap);
	va_end(ap);
	if (n < 0) {
		return;
	}
	if ((size_t)n >= sizeof(line) - 1) {
		n = (int)sizeof(line) - 2;
	}
	line[n++] = '\n';
	const char *p = line;
	size_t left = (size_t)n;
	while (left > 0) {
		ssize_t w = write(g_out, p, left);
		if (w < 0) {
			if (errno == EINTR) {
				continue;
			}
			return;
		}
		p += w;
		left -= (size_t)w;
	}
	fsync(g_out);
}

/* The errno spellings the shim's ledger uses (ledger.c's table), so a script
 * comparing this file against the ledger compares like with like; glibc's
 * name lookup is the fallback, then a bare number. */
static const char *errno_name(int err) {
	switch (err) {
	case 0: return "0";
	case EAGAIN: return "EAGAIN";
	case EPIPE: return "EPIPE";
	case ECONNRESET: return "ECONNRESET";
	case ECONNREFUSED: return "ECONNREFUSED";
	case ENOENT: return "ENOENT";
	case EBADF: return "EBADF";
	case EINVAL: return "EINVAL";
	case ENOTCONN: return "ENOTCONN";
	case ETOOMANYREFS: return "ETOOMANYREFS";
	case EACCES: return "EACCES";
	case EISDIR: return "EISDIR";
	case EPERM: return "EPERM";
	default:
		break;
	}
#if defined(__GLIBC__) && (__GLIBC__ > 2 || (__GLIBC__ == 2 && __GLIBC_MINOR__ >= 32))
	const char *np = strerrorname_np(err);
	if (np != NULL) {
		return np;
	}
#endif
	static char fallback[16];
	snprintf(fallback, sizeof(fallback), "E%d", err);
	return fallback;
}

/* Entries in /proc/self/fd, minus the one the listing itself holds open. A
 * negative result means /proc is not readable here, which the script treats
 * as "the instrument cannot measure", not as zero. */
static int count_open_fds(void) {
	DIR *d = opendir("/proc/self/fd");
	if (d == NULL) {
		return -1;
	}
	int n = 0;
	struct dirent *e;
	while ((e = readdir(d)) != NULL) {
		if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
			continue;
		}
		n++;
	}
	closedir(d);
	return n - 1;
}

static void hex_of(const uint8_t *buf, size_t n, char *out, size_t cap) {
	static const char digits[] = "0123456789abcdef";
	size_t o = 0;
	for (size_t i = 0; i < n && o + 2 < cap; i++) {
		out[o++] = digits[buf[i] >> 4];
		out[o++] = digits[buf[i] & 0xf];
	}
	out[o] = '\0';
}

/* Block until the shim's teardown SIGTERMs this process (default action:
 * terminate). Never returns; see the header for why exiting would destroy the
 * evidence the census is taken over. */
static void linger(void) __attribute__((noreturn));
static void linger(void) {
	for (;;) {
		pause();
	}
}

static int connect_designation_socket(const char *path) {
	int fd = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0);
	if (fd < 0) {
		return -1;
	}
	struct sockaddr_un addr = {.sun_family = AF_UNIX};
	if (strlen(path) >= sizeof(addr.sun_path)) {
		close(fd);
		errno = ENAMETOOLONG;
		return -1;
	}
	strcpy(addr.sun_path, path);
	if (connect(fd, (const struct sockaddr *)&addr, sizeof(addr)) != 0) {
		int err = errno;
		close(fd);
		errno = err;
		return -1;
	}
	return fd;
}

/* What a socket does after this process misbehaved on it: one blocking recv,
 * reported as the shim's answer. EOF is the contract (D8); anything else is
 * still written down rather than judged here. */
static const char *observe_socket_after(int sock, uint8_t *buf, size_t cap, char *desc, size_t desc_cap) {
	ssize_t r;
	do {
		r = recv(sock, buf, cap, 0);
	} while (r < 0 && errno == EINTR);
	if (r == 0) {
		snprintf(desc, desc_cap, "eof");
	} else if (r < 0) {
		snprintf(desc, desc_cap, "%s", errno_name(errno));
	} else {
		snprintf(desc, desc_cap, "bytes=%zd", r);
	}
	return desc;
}

/* --second-connect / --second-connect-loop: a second connection while the
 * first is held. The expected answer is an immediate EOF (D4: accepted and
 * closed at once, so the loser learns rather than hangs); what is observed is
 * written down either way. */
static void second_connects(const char *path, int count) {
	uint8_t buf[RX_IOV_BYTES];
	char desc[64];
	for (int i = 1; i <= count; i++) {
		int s2 = connect_designation_socket(path);
		if (s2 < 0) {
			say("SECOND_CONNECT i=%d result=connect_failed errno=%s", i, errno_name(errno));
			continue;
		}
		observe_socket_after(s2, buf, sizeof(buf), desc, sizeof(desc));
		say("SECOND_CONNECT i=%d result=%s", i, desc);
		close(s2);
	}
}

/* --send-garbage: bytes the socket must not accept. */
static void send_garbage(int sock) {
	static const char garbage[] = "not-a-request";
	uint8_t buf[RX_IOV_BYTES];
	char desc[64];
	ssize_t w;
	do {
		w = send(sock, garbage, sizeof(garbage) - 1, MSG_NOSIGNAL);
	} while (w < 0 && errno == EINTR);
	if (w < 0) {
		say("GARBAGE wrote=-1 errno=%s", errno_name(errno));
		return;
	}
	observe_socket_after(sock, buf, sizeof(buf), desc, sizeof(desc));
	say("GARBAGE wrote=%zd then=%s", w, desc);
}

/* --send-garbage-fd: one byte with a descriptor attached. The file is this
 * process's own, created under the runtime dir so the script can look for its
 * path in the shim's descriptor table and find it absent. Closed here after
 * the send; the kernel holds its own reference in flight until the shim's
 * `read(2)` discards it. */
static void send_garbage_fd(int sock, const char *runtime_dir) {
	char path[PATH_MAX];
	snprintf(path, sizeof(path), "%s/attached-by-app.txt", runtime_dir);
	int file = open(path, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
	if (file < 0) {
		say("GARBAGE_FD path=%s result=open_failed errno=%s", path, errno_name(errno));
		return;
	}
	uint8_t one = 'x';
	struct iovec iov = {.iov_base = &one, .iov_len = 1};
	union {
		char buf[CMSG_SPACE(sizeof(int))];
		struct cmsghdr align;
	} control;
	memset(&control, 0, sizeof(control));
	struct msghdr msg = {
		.msg_iov = &iov,
		.msg_iovlen = 1,
		.msg_control = control.buf,
		.msg_controllen = sizeof(control.buf),
	};
	struct cmsghdr *cm = CMSG_FIRSTHDR(&msg);
	cm->cmsg_level = SOL_SOCKET;
	cm->cmsg_type = SCM_RIGHTS;
	cm->cmsg_len = CMSG_LEN(sizeof(int));
	memcpy(CMSG_DATA(cm), &file, sizeof(int));
	ssize_t w;
	do {
		w = sendmsg(sock, &msg, MSG_NOSIGNAL);
	} while (w < 0 && errno == EINTR);
	close(file);
	if (w < 0) {
		say("GARBAGE_FD path=%s wrote=-1 errno=%s", path, errno_name(errno));
		return;
	}
	uint8_t buf[RX_IOV_BYTES];
	char desc[64];
	observe_socket_after(sock, buf, sizeof(buf), desc, sizeof(desc));
	say("GARBAGE_FD path=%s wrote=%zd then=%s", path, w, desc);
}

/* --stall: never read. The shim closing the connection is visible without a
 * read as POLLHUP (a unix peer's release sets our shutdown mask), which is
 * what a process that promised never to read can still honestly report. */
static void stall(int sock) __attribute__((noreturn));
static void stall(int sock) {
	struct pollfd pfd = {.fd = sock, .events = 0};
	for (;;) {
		int r = poll(&pfd, 1, -1);
		if (r < 0 && errno == EINTR) {
			continue;
		}
		if (r > 0 && (pfd.revents & (POLLHUP | POLLERR)) != 0) {
			say("STALL closed=1 revents=0x%x", (unsigned)pfd.revents);
			linger();
		}
		if (r < 0) {
			say("STALL poll_failed errno=%s", errno_name(errno));
			linger();
		}
	}
}

/* The receive contract, as the prose page states it: one recvmsg, an iov of
 * at least 280 bytes, control space for one int, MSG_CMSG_CLOEXEC, and the
 * generated decoder on the whole frame plus the descriptor. Returns the
 * received descriptor (>= 0) with `ev` filled in, -1 on EOF, -2 on anything
 * that ends the receive loop (already written down). */
static int receive_one(int sock, int index, uint8_t *buf, vitrin_shim_session_evt_designation_t *ev,
		uint32_t *object_id, ssize_t *frame_len) {
	union {
		char buf[CMSG_SPACE(sizeof(int))];
		struct cmsghdr align;
	} control;
	memset(&control, 0, sizeof(control));
	struct iovec iov = {.iov_base = buf, .iov_len = RX_IOV_BYTES};
	struct msghdr msg = {
		.msg_iov = &iov,
		.msg_iovlen = 1,
		.msg_control = control.buf,
		.msg_controllen = sizeof(control.buf),
	};
	ssize_t n;
	do {
		n = recvmsg(sock, &msg, MSG_CMSG_CLOEXEC);
	} while (n < 0 && errno == EINTR);
	if (n == 0) {
		say("CLOSED how=eof at=%d", index);
		return -1;
	}
	if (n < 0) {
		say("CLOSED how=%s at=%d", errno_name(errno), index);
		return -1;
	}
	int fd = -1;
	for (struct cmsghdr *cm = CMSG_FIRSTHDR(&msg); cm != NULL; cm = CMSG_NXTHDR(&msg, cm)) {
		if (cm->cmsg_level == SOL_SOCKET && cm->cmsg_type == SCM_RIGHTS &&
				cm->cmsg_len >= CMSG_LEN(sizeof(int))) {
			memcpy(&fd, CMSG_DATA(cm), sizeof(int));
		}
	}
	if ((msg.msg_flags & MSG_TRUNC) != 0 || (msg.msg_flags & MSG_CTRUNC) != 0) {
		/* A truncated message is a contract violation either way: the frame
		 * is incomplete, or the kernel had to drop a descriptor it could not
		 * hand over (closed, not leaked -- but this process never saw it). */
		say("TRUNCATED n=%d msg_trunc=%d cmsg_trunc=%d bytes=%zd fd=%d", index,
			(msg.msg_flags & MSG_TRUNC) != 0, (msg.msg_flags & MSG_CTRUNC) != 0, n, fd);
		if (fd >= 0) {
			close(fd);
		}
		return -2;
	}
	vitrin_decode_status_t st = vitrin_shim_session_evt_designation_decode(buf, (size_t)n, fd,
		object_id, ev);
	if (st != VITRIN_DECODE_OK) {
		say("DECODE_ERROR n=%d status=%s bytes=%zd fd=%d", index,
			vitrin_decode_status_string(st), n, fd);
		if (fd >= 0) {
			close(fd);
		}
		return -2;
	}
	*frame_len = n;
	return fd;
}

static void usage(void) {
	fprintf(stderr,
		"usage: designation-client --out FILE [--ready FILE] [--expect N] [--pread]\n"
		"                          [--write-probe] [--send-garbage] [--send-garbage-fd]\n"
		"                          [--second-connect] [--second-connect-loop N]\n"
		"                          [--stall] [--hold]\n");
	exit(2);
}

int main(int argc, char **argv) {
	struct opts o = {0};
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--out") == 0 && i + 1 < argc) {
			o.out = argv[++i];
		} else if (strcmp(argv[i], "--ready") == 0 && i + 1 < argc) {
			o.ready = argv[++i];
		} else if (strcmp(argv[i], "--expect") == 0 && i + 1 < argc) {
			o.expect = strtol(argv[++i], NULL, 10);
		} else if (strcmp(argv[i], "--pread") == 0) {
			o.pread = true;
		} else if (strcmp(argv[i], "--write-probe") == 0) {
			o.write_probe = true;
		} else if (strcmp(argv[i], "--send-garbage") == 0) {
			o.send_garbage = true;
		} else if (strcmp(argv[i], "--send-garbage-fd") == 0) {
			o.send_garbage_fd = true;
		} else if (strcmp(argv[i], "--second-connect") == 0) {
			o.second_connect = 1;
		} else if (strcmp(argv[i], "--second-connect-loop") == 0 && i + 1 < argc) {
			o.second_connect = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--stall") == 0) {
			o.stall = true;
		} else if (strcmp(argv[i], "--hold") == 0) {
			o.hold = true;
		} else {
			usage();
		}
	}
	if (o.out == NULL) {
		usage();
	}

	/* BEFORE OPENING ANYTHING -- including the evidence file, because the
	 * point is the table as execve left it. fd 3 must be closed (EBADF): it is
	 * the shim's core link, and a copy of it here is the confinement gone. */
	int fl = fcntl(3, F_GETFD);
	bool fd3_closed = fl < 0 && errno == EBADF;
	int open_at_start = count_open_fds();

	/* A write on the socket after the shim dropped us must be EPIPE, not a
	 * signal that ends this process before it writes down what it saw. */
	signal(SIGPIPE, SIG_IGN);

	g_out = open(o.out, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (g_out < 0) {
		fprintf(stderr, "designation-client: cannot open %s: %s\n", o.out, strerror(errno));
		return 2;
	}
	say("START pid=%d fd3=%s open_fds=%d", (int)getpid(), fd3_closed ? "closed" : "open",
		open_at_start);

	/* Found the way `wayland-0` is found: a fixed name under the runtime dir
	 * the shim composed, announced by no flag and no variable (D1). */
	const char *runtime_dir = getenv("XDG_RUNTIME_DIR");
	if (runtime_dir == NULL || runtime_dir[0] == '\0') {
		say("FAIL no XDG_RUNTIME_DIR in the app's environment");
		return 2;
	}
	char path[PATH_MAX];
	snprintf(path, sizeof(path), "%s/designation.sock", runtime_dir);

	int sock = connect_designation_socket(path);
	if (sock < 0) {
		/* No retry: the shim binds the socket BEFORE it forks the app (D3),
		 * so a connect failure here is a finding about the shim, not a race
		 * this instrument should paper over. Lingering keeps the realm up so
		 * the script can read the shim's log. */
		say("CONNECT_FAILED path=%s errno=%s", path, errno_name(errno));
		linger();
	}
	say("CONNECTED path=%s", path);

	/* Before --ready, so the refusal is observed while nothing is in flight
	 * and the designations that follow prove the FIRST connection still
	 * receives after the second was turned away. */
	if (o.second_connect > 0) {
		second_connects(path, o.second_connect);
	}

	/* The script's trigger: created only once this process is connected, so
	 * a designation the mock core sends on it cannot precede the connection
	 * and land as `no_client` by accident of timing. O_EXCL: a trigger that
	 * already exists is a script bug, and is reported rather than reused. */
	if (o.ready != NULL) {
		int r = open(o.ready, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
		if (r < 0) {
			say("READY_FAILED path=%s errno=%s", o.ready, errno_name(errno));
			linger();
		}
		close(r);
		say("READY path=%s", o.ready);
	}

	if (o.stall) {
		stall(sock);
	}

	uint8_t buf[RX_IOV_BYTES];
	int received = 0;
	for (;;) {
		vitrin_shim_session_evt_designation_t ev;
		uint32_t object_id = 0;
		ssize_t frame_len = 0;
		int fd = receive_one(sock, received + 1, buf, &ev, &object_id, &frame_len);
		if (fd == -1) {
			break; /* CLOSED already written */
		}
		if (fd == -2) {
			continue;
		}
		received++;

		struct stat st;
		unsigned long long st_dev = 0, st_ino = 0;
		if (fstat(fd, &st) == 0) {
			st_dev = (unsigned long long)st.st_dev;
			st_ino = (unsigned long long)st.st_ino;
		}
		say("RX n=%d designation_id=%u kind=%s mode=%s name=%.*s st_dev=%llu st_ino=%llu "
		    "frame_bytes=%zd object_id=%u",
			received, ev.designation_id,
			ev.kind == VITRIN_POWERBOX_KIND_DIRECTORY ? "directory" : "file",
			ev.mode == VITRIN_POWERBOX_MODE_READ_WRITE ? "read_write" : "read",
			(int)ev.name.len, (const char *)ev.name.data, st_dev, st_ino, frame_len, object_id);

		if (o.write_probe) {
			/* pwrite at offset 0, so a read_write descriptor's success is
			 * visible on disk at a known place and --pread below reads it
			 * back through the same descriptor. EBADF is the expected answer
			 * on a read-only descriptor: the kernel's, not the shim's. */
			ssize_t w = pwrite(fd, WRITE_PROBE_MARKER, sizeof(WRITE_PROBE_MARKER) - 1, 0);
			if (w < 0) {
				say("WRITE_PROBE n=%d result=%s", received, errno_name(errno));
			} else {
				say("WRITE_PROBE n=%d result=ok bytes=%zd", received, w);
			}
		}
		if (o.pread) {
			uint8_t rb[256];
			ssize_t r = pread(fd, rb, sizeof(rb), 0);
			if (r < 0) {
				say("PREAD n=%d result=%s", received, errno_name(errno));
			} else {
				char hex[2 * sizeof(rb) + 1];
				hex_of(rb, (size_t)r, hex, sizeof(hex));
				say("PREAD n=%d bytes=%zd hex=%s", received, r, hex);
			}
		}
		if (!o.hold) {
			close(fd);
		}

		if (received == 1 && o.send_garbage) {
			send_garbage(sock);
		}
		if (received == 1 && o.send_garbage_fd) {
			send_garbage_fd(sock, runtime_dir);
		}

		if (o.expect > 0 && received >= o.expect) {
			break;
		}
	}

	/* `open_fds` here against the START line is this process's own leak
	 * measurement: the evidence file and the socket are the two descriptors
	 * opened since, so with the default (no --hold) the difference is exactly
	 * two however many designations arrived. */
	say("DONE n=%d open_fds=%d", received, count_open_fds());
	linger();
}
