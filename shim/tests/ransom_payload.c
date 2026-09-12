/* ransom_payload.c -- the repo-authored ransomware payload for the P2.6.9
 * containment gate (issue #193): a realm app that enumerates every write it
 * attempts against the host, receives designations over the per-realm
 * `designation.sock`, writes through the descriptors it was handed, and -- on
 * request -- paints a counterfeit picker into its own surface.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * THIS CODE IS REPO-AUTHORED, AND THAT REDUCES THE GATE'S INDEPENDENCE.
 * No third-party program will cooperatively enumerate its own write attempts
 * and report the errno each got, so the property the gate measures -- a set
 * equality over ATTEMPTED writes -- requires the payload to report what it
 * tried. This is the `click-target`/`form-target`/`solid-client` precedent
 * (M1.4/M1.5, P2.6.2): the app under a mock-free gate is ours, and the
 * mitigation is stated here rather than left to be noticed (R2.7). What keeps
 * it honest is the TWO independent witnesses in the same run, not this code:
 *   - this file's own `--out` report of every write it attempted, and
 *   - the core's designation journal (`designation_settled`, keyed by
 *     `(st_dev, st_ino)`), which a payload that lied about a designated fd
 *     would have to contradict.
 * What this does NOT rule out is a payload BUG that under-reports its own
 * attempts in a way the journal also misses (the journal only sees
 * designations, never the writes this payload aims at undesignated paths);
 * `tests/integration/test_real_ransomware.py` says so in as many words, and
 * pins the write-set claim per rung against `--print-isolation`'s tier.
 *
 * WHY WRITES HERE ARE SAFE. A "ransomware" payload that actually encrypted or
 * overwrote a file would, under the `--isolation=off` positive control, be
 * pointed at the operator's REAL home directory (that is the whole point of
 * the control: the host home IS reachable there). So this payload never
 * overwrites or truncates anything it did not create: every write attempt is
 * `open(dir/<nonce>, O_WRONLY|O_CREAT|O_EXCL, 0600)` of a uniquely-named
 * canary, followed by a marker `pwrite` and an `unlink` of exactly that
 * canary. `O_EXCL` guarantees it never lands on a pre-existing file; the
 * `unlink` cleans up what it made; and the gate cleans up again by nonce
 * prefix. The reported fact is the errno the kernel gave the CREATE -- which
 * is all the set-equality needs.
 *
 * WHY `--out` AND NEVER STDOUT. Under a real confined core this process's
 * stdout is redirected into the realm's `realm.log`, which is the core's
 * evidence, not this process's (`designation_client.c` and `gesture_probe.c`
 * record why a line written there is spliced into another process's log). The
 * file named by `--out` is this process's alone and every line is fsync'd, so
 * the gate reads it live and sequences "the payload has the descriptor"
 * against the core's own journal without waiting for anyone to exit. Relative
 * `--out` resolves against `$XDG_RUNTIME_DIR` (the realm runtime dir), so one
 * argv serves both isolation settings (`solid_client.c`'s `--probe-out`
 * convention).
 *
 * WHY IT LINGERS. The shim terminates the realm when its app exits, releasing
 * every descriptor. An instrument that exited when done would take the fd
 * census with it, so after its work it blocks in `pause()` until the shim's
 * teardown SIGTERMs it (`designation_client.c`'s reasoning).
 *
 * THE MODES, each a phase the gate drives in isolation by re-booting the core:
 *
 *   --write-target LABEL=DIR   attempt a self-cleaning canary write into DIR
 *                       and report the CREATE's errno with (dev,ino). Repeat
 *                       for each undesignated target the gate names: the host
 *                       home, `/`, `/tmp`, the core runtime tree, the
 *                       core.sock's directory, the realm's own private
 *                       storage. Ends with `WRITE-SET-END`.
 *   --expect N          receive N designations over designation.sock (the
 *                       app-side, mock-free receipt P2.6.6/P2.6.7 left owed to
 *                       this gate). Each is reported `RX ...` with (dev,ino).
 *   --designated-write  after each RX, write through the delivered fd: a
 *                       marker `pwrite` for a file (EBADF on a read-only fd is
 *                       the kernel's answer, the read_write control succeeds),
 *                       and for a directory fd an `openat(fd, <nonce>, O_CREAT
 *                       |O_WRONLY|O_EXCL)` then `unlink`. Reported `DESIGNATED
 *                       ...`. This is "encrypt the fds the user designated",
 *                       made safe the same way the write-set canaries are.
 *   --replica-picker    bring up a Wayland surface and paint a counterfeit of
 *                       the core's picker over the whole of it -- the
 *                       strongest spoof an app that cannot observe the trusted
 *                       colour can mount. Installs no input grab (a plain
 *                       xdg_toplevel client holds none). The gate, on a
 *                       backend that stacks the consent grab (DRM/nested),
 *                       drives a real key and shows it reaches the GENUINE
 *                       picker while the band witness shows the real band
 *                       still overdraws this replica.
 *   --ready FILE        create FILE once connected, so the agent's designation
 *                       (driven by the gate) cannot precede the connection and
 *                       land `no_client` by a timing accident.
 *   --run-ms MS         how long `--replica-picker` services its surface.
 *
 * Every line is one record, `KEY value=... value=...`, on one line, fsync'd.
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
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#include <wayland-client.h>

#include "vitrin-protocol.h"
#include "xdg-shell-client-protocol.h"

/* A `designation` frame is at most 280 bytes (8-byte header, three u32s, a
 * <=255-byte string padded to 4). The iov is larger so an oversized message
 * reads back as oversized (MSG_TRUNC), not as a decode error on a frame whose
 * tail never arrived -- `designation_client.c`'s reasoning. */
#define RX_IOV_BYTES 512
/* What a write through a delivered descriptor puts at offset 0. Distinct from
 * `designation_client.c`'s marker so a stray reader cannot confuse the two. */
#define DESIGNATED_MARKER "RANSOM-WROTE-HERE"
/* What a canary write puts in the file it created before unlinking it. */
#define CANARY_MARKER "RANSOM-CANARY"
/* Every canary the payload creates carries this prefix, so the gate can sweep
 * any that a crash left behind by name. */
#define CANARY_PREFIX ".vitrin-ransom-canary-"

static int g_out = -1;
static volatile sig_atomic_t g_stop = 0;

static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

/* One record, one line, one write, one fsync: the gate tails this file while
 * this process still runs, and a line in the page cache but not the file's
 * readable length is a race the gate would lose (`designation_client.c`). */
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

/* The errno spellings the shim's ledger and `designation_client.c` use, so a
 * gate comparing this file against the journal compares like with like; glibc's
 * name lookup is the fallback, then a bare number. */
static const char *errno_name(int err) {
	switch (err) {
	case 0: return "0";
	case EACCES: return "EACCES";
	case EPERM: return "EPERM";
	case ENOENT: return "ENOENT";
	case EBADF: return "EBADF";
	case EEXIST: return "EEXIST";
	case EISDIR: return "EISDIR";
	case EROFS: return "EROFS";
	case EINVAL: return "EINVAL";
	case ENOTDIR: return "ENOTDIR";
	case ELOOP: return "ELOOP";
	case ENAMETOOLONG: return "ENAMETOOLONG";
	case EXDEV: return "EXDEV";
	case EPIPE: return "EPIPE";
	case ECONNRESET: return "ECONNRESET";
	case ECONNREFUSED: return "ECONNREFUSED";
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

/* Entries in /proc/self/fd, minus the one the listing itself holds. Negative
 * means /proc is unreadable, which the gate treats as "cannot measure", not
 * zero (`designation_client.c`). */
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

static void linger(void) __attribute__((noreturn));
static void linger(void) {
	for (;;) {
		pause();
	}
}

/* A per-process, per-call nonce for canary names: pid plus a monotonic
 * counter, so two canaries in the same directory never collide and `O_EXCL`
 * never trips on our own earlier file. No randomness -- determinism keeps the
 * gate's cleanup-by-prefix exhaustive. */
static unsigned long g_nonce_seq = 0;
static void canary_name(char *out, size_t cap) {
	snprintf(out, cap, "%s%d-%lu", CANARY_PREFIX, (int)getpid(), g_nonce_seq++);
}

/* ---- write-set enumeration ------------------------------------------- */

/* Attempt one self-cleaning canary write into `dir`, reporting the CREATE's
 * errno and, on success, the (dev,ino) of the directory it landed in (fstat of
 * the created fd's parent is not available, so we fstat the created file and
 * report its own inode -- enough for the gate to tell "landed in the realm's
 * private tmpfs" from "landed on the host"). A directory that does not exist
 * inside the realm answers ENOENT on the CREATE; one the ruleset denies
 * answers EACCES; one inside the granted set succeeds. All three are the
 * measurement.
 *
 * The gate names each target with a label so the report is read by intent, not
 * by matching absolute paths that differ between the two isolation settings. */
static void write_target(const char *label, const char *dir) {
	char nonce[64];
	canary_name(nonce, sizeof(nonce));
	char path[PATH_MAX];
	int need = snprintf(path, sizeof(path), "%s/%s", dir, nonce);
	if (need < 0 || (size_t)need >= sizeof(path)) {
		say("WRITE target=%s dir=%s op=create rc=-1 errno=ENAMETOOLONG dev=0 ino=0", label,
			dir);
		return;
	}

	int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
	if (fd < 0) {
		say("WRITE target=%s dir=%s op=create rc=-1 errno=%s dev=0 ino=0", label, dir,
			errno_name(errno));
		return;
	}
	struct stat st;
	unsigned long long dev = 0, ino = 0;
	if (fstat(fd, &st) == 0) {
		dev = (unsigned long long)st.st_dev;
		ino = (unsigned long long)st.st_ino;
	}
	say("WRITE target=%s dir=%s op=create rc=0 errno=0 dev=%llu ino=%llu", label, dir, dev,
		ino);

	ssize_t w = pwrite(fd, CANARY_MARKER, sizeof(CANARY_MARKER) - 1, 0);
	if (w < 0) {
		say("WRITE target=%s op=pwrite rc=-1 errno=%s", label, errno_name(errno));
	} else {
		say("WRITE target=%s op=pwrite rc=0 errno=0 bytes=%zd", label, w);
	}
	close(fd);

	if (unlink(path) != 0) {
		say("WRITE target=%s op=unlink rc=-1 errno=%s", label, errno_name(errno));
	} else {
		say("WRITE target=%s op=unlink rc=0 errno=0", label);
	}
}

/* ---- designation receipt --------------------------------------------- */

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

/* One designation off the socket: the exact receive `designation_client.c`
 * uses, decoded with the generated header. Returns the delivered fd (>= 0), -1
 * on close/error (already reported), -2 on a truncated/undecodable message. */
static int receive_one(int sock, int index, uint8_t *buf,
		vitrin_shim_session_evt_designation_t *ev, uint32_t *object_id, ssize_t *frame_len) {
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
		say("TRUNCATED n=%d msg_trunc=%d cmsg_trunc=%d bytes=%zd fd=%d", index,
			(msg.msg_flags & MSG_TRUNC) != 0, (msg.msg_flags & MSG_CTRUNC) != 0, n, fd);
		if (fd >= 0) {
			close(fd);
		}
		return -2;
	}
	vitrin_decode_status_t st =
		vitrin_shim_session_evt_designation_decode(buf, (size_t)n, fd, object_id, ev);
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

/* Write through a delivered descriptor. A file: a marker `pwrite` at offset 0
 * (a read_write fd succeeds; a read-only fd is the kernel's EBADF; and where
 * `LANDLOCK_ACCESS_FS_TRUNCATE` is absent, below the rung the tier reports, an
 * `O_TRUNC` on a read-granted PATH is the ransomware-relevant loss -- but a
 * `pwrite` on a read-only DESCRIPTOR is EBADF at every rung, so this reports
 * the descriptor write, not a vacuous ftruncate). A directory: `openat(fd,
 * <nonce>, O_CREAT|O_WRONLY|O_EXCL)` then `unlink`, so a subtree grant's write
 * reach is measured through the fd the kernel enforces containment on. */
static void designated_write(int fd, int index, bool is_dir) {
	if (is_dir) {
		char nonce[64];
		canary_name(nonce, sizeof(nonce));
		int w = openat(fd, nonce, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
		if (w < 0) {
			say("DESIGNATED n=%d op=openat rc=-1 errno=%s", index, errno_name(errno));
			return;
		}
		ssize_t b = pwrite(w, DESIGNATED_MARKER, sizeof(DESIGNATED_MARKER) - 1, 0);
		say("DESIGNATED n=%d op=openat rc=0 errno=0 bytes=%zd", index, b);
		close(w);
		if (unlinkat(fd, nonce, 0) != 0) {
			say("DESIGNATED n=%d op=unlinkat rc=-1 errno=%s", index, errno_name(errno));
		} else {
			say("DESIGNATED n=%d op=unlinkat rc=0 errno=0", index);
		}
		return;
	}
	ssize_t w = pwrite(fd, DESIGNATED_MARKER, sizeof(DESIGNATED_MARKER) - 1, 0);
	if (w < 0) {
		say("DESIGNATED n=%d op=pwrite rc=-1 errno=%s", index, errno_name(errno));
	} else {
		say("DESIGNATED n=%d op=pwrite rc=0 errno=0 bytes=%zd", index, w);
	}
}

/* ---- replica picker (Wayland surface) -------------------------------- */

#define REPLICA_BG 0x1b1b24u    /* a plausible dark card matte */
#define REPLICA_CARD 0xf0f0f5u  /* the counterfeit card face */
#define REPLICA_ACCENT 0x3b82f6u /* a fake "trusted" ring the app cannot make real */

struct replica {
	struct wl_display *display;
	struct wl_registry *registry;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;
	int width, height;
	bool configured;
	bool closed;
	void *pool_base;
	size_t pool_size;
	struct wl_buffer *buffer;
	uint32_t *pixels;
};

static inline uint32_t pack(uint32_t rgb) {
	return 0xff000000u | rgb;
}

/* Paint a counterfeit picker over the whole surface: a matte, a centred card,
 * and -- the strongest lie available -- a border in an accent colour meant to
 * pass for the core's trusted ring. It cannot match the real indicator (the
 * core never discloses that colour), which is exactly what the gate's band
 * witness proves the human still sees over this. */
static void replica_paint(struct replica *r) {
	size_t count = (size_t)r->width * (size_t)r->height;
	for (size_t i = 0; i < count; i++) {
		r->pixels[i] = pack(REPLICA_BG);
	}
	int cw = r->width * 3 / 4, ch = r->height * 3 / 4;
	int cx = (r->width - cw) / 2, cy = (r->height - ch) / 2;
	for (int y = cy; y < cy + ch && y < r->height; y++) {
		uint32_t *row = r->pixels + (size_t)y * (size_t)r->width;
		for (int x = cx; x < cx + cw && x < r->width; x++) {
			bool edge = (y < cy + 4) || (y >= cy + ch - 4) || (x < cx + 4) ||
				(x >= cx + cw - 4);
			row[x] = pack(edge ? REPLICA_ACCENT : REPLICA_CARD);
		}
	}
}

static void wm_base_ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(base, serial);
}
static const struct xdg_wm_base_listener wm_base_listener = {.ping = wm_base_ping};

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
		const char *iface, uint32_t version) {
	struct replica *r = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		uint32_t want = version > 4 ? 4 : version;
		r->compositor = wl_registry_bind(reg, name, &wl_compositor_interface, want);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		r->shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		r->wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
	}
	/* Deliberately binds NO wl_seat, no input-inhibit, no keyboard-shortcuts-
	 * inhibit and no layer-shell: a replica that grabbed input would be a
	 * different threat, and the point of this rung is that it CANNOT. */
}
static void registry_global_remove(void *data, struct wl_registry *reg, uint32_t name) {
	(void)data;
	(void)reg;
	(void)name;
}
static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

static void toplevel_configure(void *data, struct xdg_toplevel *tl, int32_t width,
		int32_t height, struct wl_array *states) {
	(void)tl;
	(void)states;
	struct replica *r = data;
	if (width > 0 && height > 0) {
		r->width = width;
		r->height = height;
	}
}
static void toplevel_close(void *data, struct xdg_toplevel *tl) {
	(void)tl;
	((struct replica *)data)->closed = true;
}
static void toplevel_configure_bounds(void *data, struct xdg_toplevel *tl, int32_t w,
		int32_t h) {
	(void)data;
	(void)tl;
	(void)w;
	(void)h;
}
static void toplevel_wm_capabilities(void *data, struct xdg_toplevel *tl,
		struct wl_array *caps) {
	(void)data;
	(void)tl;
	(void)caps;
}
static const struct xdg_toplevel_listener toplevel_listener = {
	.configure = toplevel_configure,
	.close = toplevel_close,
	.configure_bounds = toplevel_configure_bounds,
	.wm_capabilities = toplevel_wm_capabilities,
};

static void xdg_surface_configure(void *data, struct xdg_surface *xs, uint32_t serial) {
	struct replica *r = data;
	xdg_surface_ack_configure(xs, serial);
	r->configured = true;
}
static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

static bool replica_buffer(struct replica *r) {
	size_t stride = (size_t)r->width * 4;
	r->pool_size = stride * (size_t)r->height;
	int fd = memfd_create("ransom-replica", MFD_CLOEXEC);
	if (fd < 0) {
		say("REPLICA_FAIL memfd errno=%s", errno_name(errno));
		return false;
	}
	if (ftruncate(fd, (off_t)r->pool_size) != 0) {
		say("REPLICA_FAIL ftruncate errno=%s", errno_name(errno));
		close(fd);
		return false;
	}
	r->pool_base = mmap(NULL, r->pool_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (r->pool_base == MAP_FAILED) {
		say("REPLICA_FAIL mmap errno=%s", errno_name(errno));
		close(fd);
		return false;
	}
	r->pixels = r->pool_base;
	struct wl_shm_pool *pool = wl_shm_create_pool(r->shm, fd, (int32_t)r->pool_size);
	r->buffer = wl_shm_pool_create_buffer(pool, 0, r->width, r->height, (int32_t)stride,
		WL_SHM_FORMAT_XRGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);
	return r->buffer != NULL;
}

static int run_replica_picker(int run_ms) {
	struct replica r = {.width = 0, .height = 0};
	r.display = wl_display_connect(NULL);
	if (r.display == NULL) {
		say("REPLICA_FAIL no-wayland-display");
		return 1;
	}
	r.registry = wl_display_get_registry(r.display);
	wl_registry_add_listener(r.registry, &registry_listener, &r);
	wl_display_roundtrip(r.display);
	if (r.compositor == NULL || r.shm == NULL || r.wm_base == NULL) {
		say("REPLICA_FAIL missing-globals compositor=%d shm=%d wm_base=%d",
			r.compositor != NULL, r.shm != NULL, r.wm_base != NULL);
		return 1;
	}
	xdg_wm_base_add_listener(r.wm_base, &wm_base_listener, &r);
	r.surface = wl_compositor_create_surface(r.compositor);
	r.xdg_surface = xdg_wm_base_get_xdg_surface(r.wm_base, r.surface);
	xdg_surface_add_listener(r.xdg_surface, &xdg_surface_listener, &r);
	r.toplevel = xdg_surface_get_toplevel(r.xdg_surface);
	xdg_toplevel_add_listener(r.toplevel, &toplevel_listener, &r);
	xdg_toplevel_set_title(r.toplevel, "Grant file access");
	xdg_toplevel_set_app_id(r.toplevel, "org.vitrin.ransom-payload");
	wl_surface_commit(r.surface);

	while (!r.configured && !g_stop && wl_display_dispatch(r.display) != -1) {
	}
	if (r.width <= 0 || r.height <= 0) {
		say("REPLICA_FAIL no-configured-size");
		return 1;
	}
	if (!replica_buffer(&r)) {
		return 1;
	}
	replica_paint(&r);
	wl_surface_attach(r.surface, r.buffer, 0, 0);
	wl_surface_damage_buffer(r.surface, 0, 0, r.width, r.height);
	wl_surface_commit(r.surface);
	say("REPLICA size=%dx%d grab=none", r.width, r.height);

	struct timespec start;
	clock_gettime(CLOCK_MONOTONIC, &start);
	while (!g_stop && !r.closed) {
		if (wl_display_dispatch_pending(r.display) == -1) {
			break;
		}
		wl_display_flush(r.display);
		struct timespec now;
		clock_gettime(CLOCK_MONOTONIC, &now);
		long elapsed = (long)(now.tv_sec - start.tv_sec) * 1000 +
			(now.tv_nsec - start.tv_nsec) / 1000000;
		if (elapsed >= run_ms) {
			break;
		}
		struct pollfd pfd = {.fd = wl_display_get_fd(r.display), .events = POLLIN};
		poll(&pfd, 1, 50);
		if (pfd.revents & POLLIN) {
			wl_display_dispatch(r.display);
		}
	}
	say("REPLICA-END");
	wl_display_disconnect(r.display);
	return 0;
}

/* ---- argv ------------------------------------------------------------- */

#define MAX_TARGETS 16

struct opts {
	const char *out;
	const char *ready;
	long expect;
	bool designated_write;
	bool replica_picker;
	int run_ms;
	int n_targets;
	const char *target_label[MAX_TARGETS];
	const char *target_dir[MAX_TARGETS];
};

static void usage(void) {
	fprintf(stderr,
		"usage: ransom-payload --out FILE [--ready FILE] [--expect N]\n"
		"                      [--write-target LABEL=DIR ...] [--designated-write]\n"
		"                      [--replica-picker] [--run-ms MS]\n");
	exit(2);
}

int main(int argc, char **argv) {
	struct opts o = {.run_ms = 4000};
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--out") == 0 && i + 1 < argc) {
			o.out = argv[++i];
		} else if (strcmp(argv[i], "--ready") == 0 && i + 1 < argc) {
			o.ready = argv[++i];
		} else if (strcmp(argv[i], "--expect") == 0 && i + 1 < argc) {
			o.expect = strtol(argv[++i], NULL, 10);
		} else if (strcmp(argv[i], "--designated-write") == 0) {
			o.designated_write = true;
		} else if (strcmp(argv[i], "--replica-picker") == 0) {
			o.replica_picker = true;
		} else if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			o.run_ms = atoi(argv[++i]);
		} else if (strcmp(argv[i], "--write-target") == 0 && i + 1 < argc) {
			if (o.n_targets >= MAX_TARGETS) {
				fprintf(stderr, "ransom-payload: too many --write-target\n");
				return 2;
			}
			char *spec = argv[++i];
			char *eq = strchr(spec, '=');
			if (eq == NULL) {
				usage();
			}
			*eq = '\0';
			o.target_label[o.n_targets] = spec;
			o.target_dir[o.n_targets] = eq + 1;
			o.n_targets++;
		} else {
			usage();
		}
	}
	if (o.out == NULL) {
		usage();
	}

	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);
	/* A write on a dropped socket must be EPIPE we can report, not a signal
	 * that ends the process before it writes down what it saw. */
	signal(SIGPIPE, SIG_IGN);

	/* BEFORE OPENING ANYTHING but the evidence file: fd 3 is the shim's core
	 * link, and a copy of it here would be the confinement gone. */
	int fl = fcntl(3, F_GETFD);
	bool fd3_closed = fl < 0 && errno == EBADF;
	int open_at_start = count_open_fds();

	/* Relative --out resolves against the realm runtime dir, so one argv
	 * serves both isolation settings (`solid_client.c`'s convention). */
	const char *runtime_dir = getenv("XDG_RUNTIME_DIR");
	char out_path[PATH_MAX];
	if (o.out[0] == '/' || runtime_dir == NULL || runtime_dir[0] == '\0') {
		snprintf(out_path, sizeof(out_path), "%s", o.out);
	} else {
		snprintf(out_path, sizeof(out_path), "%s/%s", runtime_dir, o.out);
	}
	g_out = open(out_path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (g_out < 0) {
		fprintf(stderr, "ransom-payload: cannot open %s: %s\n", out_path, strerror(errno));
		return 2;
	}

	const char *home = getenv("HOME");
	say("START pid=%d fd3=%s open_fds=%d xdg_runtime=%s home=%s", (int)getpid(),
		fd3_closed ? "closed" : "open", open_at_start, runtime_dir ? runtime_dir : "(unset)",
		home ? home : "(unset)");

	/* PHASE 1 -- the write set against every undesignated target the gate
	 * named. Needs no socket and no surface, so it runs first and always
	 * finishes, whatever the isolation setting refuses. */
	for (int i = 0; i < o.n_targets; i++) {
		write_target(o.target_label[i], o.target_dir[i]);
	}
	if (o.n_targets > 0) {
		say("WRITE-SET-END n=%d", o.n_targets);
	}

	/* PHASE 2 -- the app-side designation receipt (P2.6.6/P2.6.7 owed this
	 * gate the mock-free half). Connect BEFORE --ready so the agent's ask,
	 * driven by the gate after --ready appears, cannot land `no_client`. */
	int received = 0;
	if (o.expect > 0) {
		if (runtime_dir == NULL || runtime_dir[0] == '\0') {
			say("FAIL no XDG_RUNTIME_DIR for designation.sock");
			linger();
		}
		char sock_path[PATH_MAX];
		snprintf(sock_path, sizeof(sock_path), "%s/designation.sock", runtime_dir);
		int sock = connect_designation_socket(sock_path);
		if (sock < 0) {
			say("CONNECT_FAILED path=%s errno=%s", sock_path, errno_name(errno));
			linger();
		}
		say("CONNECTED path=%s", sock_path);

		if (o.ready != NULL) {
			char ready_path[PATH_MAX];
			if (o.ready[0] == '/' || runtime_dir == NULL) {
				snprintf(ready_path, sizeof(ready_path), "%s", o.ready);
			} else {
				snprintf(ready_path, sizeof(ready_path), "%s/%s", runtime_dir, o.ready);
			}
			int rfd = open(ready_path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
			if (rfd < 0) {
				say("READY_FAILED path=%s errno=%s", ready_path, errno_name(errno));
				linger();
			}
			close(rfd);
			say("READY path=%s", ready_path);
		}

		uint8_t buf[RX_IOV_BYTES];
		for (;;) {
			vitrin_shim_session_evt_designation_t ev;
			uint32_t object_id = 0;
			ssize_t frame_len = 0;
			int fd = receive_one(sock, received + 1, buf, &ev, &object_id, &frame_len);
			if (fd == -1) {
				break;
			}
			if (fd == -2) {
				continue;
			}
			received++;
			bool is_dir = ev.kind == VITRIN_POWERBOX_KIND_DIRECTORY;
			struct stat st;
			unsigned long long st_dev = 0, st_ino = 0;
			if (fstat(fd, &st) == 0) {
				st_dev = (unsigned long long)st.st_dev;
				st_ino = (unsigned long long)st.st_ino;
			}
			say("RX n=%d designation_id=%u kind=%s mode=%s name=%.*s st_dev=%llu "
			    "st_ino=%llu frame_bytes=%zd object_id=%u",
				received, ev.designation_id, is_dir ? "directory" : "file",
				ev.mode == VITRIN_POWERBOX_MODE_READ_WRITE ? "read_write" : "read",
				(int)ev.name.len, (const char *)ev.name.data, st_dev, st_ino, frame_len,
				object_id);
			if (o.designated_write) {
				designated_write(fd, received, is_dir);
			}
			close(fd);
			if (o.expect > 0 && received >= o.expect) {
				break;
			}
		}
		say("RX-END n=%d open_fds=%d", received, count_open_fds());
	}

	/* PHASE 3 -- the counterfeit picker, if asked. Only this mode touches
	 * Wayland, so the write-set and designation phases never need a display. */
	if (o.replica_picker) {
		run_replica_picker(o.run_ms);
	}

	say("DONE received=%d targets=%d open_fds=%d", received, o.n_targets, count_open_fds());
	linger();
}
