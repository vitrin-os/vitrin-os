/* solid_client.c -- a deterministic Wayland client that paints one known
 * solid colour over its whole surface, for the P1.8.5 real-app capture gate
 * (issue #107).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY THIS EXISTS, next to damage_client.c and gtk_entry_probe.c. The M1.3
 * exit gate asks two things of a REAL app captured through the real chain:
 *
 *   - its `observe()` frame's DOMINANT COLOUR equals the colour the app
 *     rendered (criterion 2), and
 *   - that frame agrees, by SSIM, with the core-internal capture (criterion
 *     3) -- the "grant path adds no distortion" proof against a real app.
 *
 * weston-terminal and gtk-entry-probe both render CHROME -- glyphs, borders,
 * a cursor -- so "the dominant colour is #RRGGBB over enough of the view" is
 * theme- and font-dependent, and a terminal at rest is a poor static scene
 * for an SSIM that wants two captures of ONE unchanging frame. This client is
 * the opposite: it fills the entire realm view with a single colour whose
 * channels are multiples of 0x11 (so they survive the capture's 4-bit
 * dominant-colour histogram exactly), and it never animates, so every
 * composited frame is byte-identical. That makes it the clean rung for both
 * criteria and a static scene the SSIM reads back as ~1.0.
 *
 * It is a bare wl_shm + xdg-shell client -- no toolkit -- so the "known
 * colour" is exactly the bytes it wrote, with no client-side decoration or
 * antialiasing to erode the dominant fraction or add off-palette edges. It
 * follows damage_client.c's structure (two buffers with real release
 * tracking, configure-driven geometry) so the two clients cannot drift in how
 * they speak to the shim; the only thing new here is "paint one colour".
 *
 *   CLIENT colour=RRGGBB size=WxH   printed on exit
 *
 * THE CONFINEMENT PROBE (--probe / --probe-out), added by P2.6.2 (issue #186).
 * Opt-in and inert without the flags, so every gate that already runs this app
 * is unchanged. With them, the client `open()`s each named path BEFORE it
 * touches Wayland and writes what the kernel said into a report file.
 *
 * Why it lives here rather than in a fourth Wayland client: the *Wayland*
 * behaviour a confinement gate needs is exactly this file's -- attach, paint
 * one colour, stay alive -- and the repo already carries three copies of that
 * boilerplate whose only stated justification is that they speak to the shim
 * DIFFERENTLY. A fourth copy differing in nothing would be the drift those
 * files' headers warn about. What is new is twelve lines of `open` + `fstat`.
 *
 * Why it reports (st_dev, st_ino) and not just success: "the canary is
 * absent" and "a same-named empty stub is present" are different facts, and
 * the mount table *creates* stubs -- `vitrin-realm-init` binds the app's own
 * directory at its host path, which `mkdir -p`s every ancestor onto the
 * realm's root tmpfs, so `$HOME` itself exists inside the realm as an empty
 * directory. Only inode identity separates "reachable" from "same name".
 * `crates/vitrin-core/src/spawn.rs`'s parent-side canary check learned this
 * the hard way and says so in its own comment.
 *
 * Why BEFORE the Wayland connection: so a report exists even when the
 * compositor never comes up, which is what stops "the probe never ran" from
 * being indistinguishable from "the probe ran and found nothing".
 *
 *   PROBE-VERSION 4
 *   PROBE-GROUPS n=<count> gids=<g,g,...>
 *   PROBE-USERNS created=<yes|no|error> errno=<n>
 *   PROBE-NET-IFACES names=<a,b,...|error>
 *   PROBE-NET-LO up=<yes|no|error> running=<yes|no|error> errno=<n>
 *   PROBE-NET-LOOPBACK rc=<0|-1> errno=<n>
 *   PROBE-NET-CONNECT target=<host:port> rc=<0|-1> errno=<n>    (--net-connect)
 *   PROBE-NET-ABSTRACT name=<nonce> rc=<0|-1> errno=<n>         (--net-abstract)
 *   PROBE path=<abs> open=<ok|fail> errno=<n> dev=<u> ino=<u>   (one per --probe)
 *   PROBE-SYSCALL name=<key> rc=<n> errno=<n>          (one per row, --syscall-probe)
 *   PROBE-END
 *
 * PROBE-USERNS arrived with version 2 (P2.6.3): a realm refuses to create user
 * namespaces inside itself (`vitrin-realm-init`'s K9b), and that is a property
 * about a SYSCALL rather than about a path, so no number of `--probe` entries
 * could have measured it. It is reported unconditionally alongside
 * PROBE-GROUPS because it costs one fork and because a report that carried it
 * only on request would let the interesting run be the one nobody asked for.
 *
 * The PROBE-NET-* block arrived with version 4 (P2.7.1, issue #195), and is
 * about the realm's NETWORK NAMESPACE -- again a property no `--probe` path
 * could reach. The first three are unconditional on PROBE-USERNS's reasoning;
 * the last two take a target because only the harness knows what it bound.
 * See the block above `probe_net_ifaces` for why `lo` is up, and for why the
 * "was it reachable at all" half deliberately is not measured here.
 *
 * THE SYSCALL PROBE (--syscall-probe), added by P2.6.4 (issue #188).
 *
 * THIS CODE IS REPO-AUTHORED. It is not a third party's conformance suite and
 * it is not an independent check: the same repository writes the seccomp table
 * (`crates/vitrin-realm-init/src/seccomp.rs`) and writes the probe that
 * measures it, so a filter and a probe could agree with each other and both be
 * wrong about the world. That reduction in independence is stated here rather
 * than left for a reader to find (R2.7), on the precedent of `click-target`
 * and `form-target` in `tests/integration/`. What keeps it honest is the
 * POSITIVE CONTROL, not the code: `tests/integration/test_real_seccomp.py`
 * runs this same binary OUTSIDE a realm, and a row whose syscall fails there
 * too is reported NOT DEMONSTRATED rather than counted as confinement.
 *
 * One line per row, and the key is the table's own `name` field, so the gate
 * can iterate `vitrind --print-seccomp` and fail on any row this probe did not
 * attempt. `rc` is the syscall's return value and `errno` is what it set;
 * errno is part of the filter's contract (EPERM and ENOSYS are behaviourally
 * different to a library that probes and falls back), so the gate asserts the
 * exact value rather than "not zero".
 *
 * Every probe with a side effect on the calling process runs in a FORKED
 * CHILD, for the reason `probe_userns` gives above: `ptrace(PTRACE_TRACEME)`
 * makes the caller traced, `personality` changes its persona for life, and
 * `seccomp(SECCOMP_SET_MODE_FILTER)` installs a filter that cannot be removed
 * -- so a probe run in-process would destroy the run it is reporting on. The
 * keyring probes deliberately use KEY_SPEC_PROCESS_KEYRING and not the session
 * keyring: a process keyring dies with the process, so the control run outside
 * a realm leaves nothing on the operator's own keyring. The finding that
 * motivated those rows was measured against the SESSION keyring; reproducing
 * it here would mean writing to the operator's keyring on every CI run, and
 * the row is demonstrated either way because the syscall is what the filter
 * denies.
 */
#define _GNU_SOURCE

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <inttypes.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <stddef.h>
#include <linux/audit.h>
#include <linux/bpf.h>
#include <linux/filter.h>
#include <linux/perf_event.h>
#include <linux/seccomp.h>
#include <sched.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/ptrace.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

#define BUFFER_COUNT 2

/* Enough for every path a gate has reason to probe in one run; a request for
 * more is refused rather than silently truncated, because a probe list that
 * quietly lost an entry would read as an absence that was never tested. */
#define MAX_PROBES 8

/* Default colour: pure blue. Each channel is a multiple of 0x11 (00, 00, ff),
 * so the capture's top-nibble dominant-colour histogram reads it back exactly
 * -- the same discipline the Firefox solid page uses (#0000ff). */
#define DEFAULT_COLOUR 0x0000ffu

static volatile sig_atomic_t g_stop = 0;
static void on_signal(int sig) {
	(void)sig;
	g_stop = 1;
}

struct buffer {
	struct wl_buffer *wl;
	uint32_t *pixels;
	size_t size;
	bool busy;
};

struct client {
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

	/* The 0x00RRGGBB the client was asked to paint, and the packed XRGB8888
	 * pixel it fills with (opaque). */
	uint32_t rgb;
	uint32_t pixel;

	struct buffer buffers[BUFFER_COUNT];
	bool buffers_ready;

	uint64_t commits;
};

/* ---- registry -------------------------------------------------------- */

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
		const char *iface, uint32_t version) {
	struct client *c = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		uint32_t want = version < 4 ? 4 : version;
		c->compositor = wl_registry_bind(reg, name, &wl_compositor_interface, want > 6 ? 6 : want);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		c->shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		c->wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
	}
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

/* ---- xdg-shell ------------------------------------------------------- */

static void wm_base_ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {.ping = wm_base_ping};

static void toplevel_configure(void *data, struct xdg_toplevel *tl, int32_t width,
		int32_t height, struct wl_array *states) {
	(void)tl;
	(void)states;
	struct client *c = data;
	if (width > 0 && height > 0) {
		c->width = width;
		c->height = height;
	}
}

static void toplevel_close(void *data, struct xdg_toplevel *tl) {
	(void)tl;
	((struct client *)data)->closed = true;
}

static void toplevel_configure_bounds(void *data, struct xdg_toplevel *tl,
		int32_t width, int32_t height) {
	(void)data;
	(void)tl;
	(void)width;
	(void)height;
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
	struct client *c = data;
	xdg_surface_ack_configure(xs, serial);
	c->configured = true;
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

/* ---- buffers --------------------------------------------------------- */

static void buffer_release(void *data, struct wl_buffer *wl) {
	(void)wl;
	((struct buffer *)data)->busy = false;
}

static const struct wl_buffer_listener buffer_listener = {.release = buffer_release};

static bool buffers_create(struct client *c) {
	size_t stride = (size_t)c->width * 4;
	size_t size = stride * (size_t)c->height;
	size_t total = size * BUFFER_COUNT;

	int fd = memfd_create("solid-client", MFD_CLOEXEC);
	if (fd < 0 || ftruncate(fd, (off_t)total) != 0) {
		perror("memfd");
		return false;
	}
	void *base = mmap(NULL, total, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (base == MAP_FAILED) {
		perror("mmap");
		close(fd);
		return false;
	}
	struct wl_shm_pool *pool = wl_shm_create_pool(c->shm, fd, (int32_t)total);
	for (int i = 0; i < BUFFER_COUNT; i++) {
		c->buffers[i].wl = wl_shm_pool_create_buffer(pool, (int32_t)(size * (size_t)i),
			c->width, c->height, (int32_t)stride, WL_SHM_FORMAT_XRGB8888);
		c->buffers[i].pixels = (uint32_t *)((char *)base + size * (size_t)i);
		c->buffers[i].size = size;
		c->buffers[i].busy = false;
		wl_buffer_add_listener(c->buffers[i].wl, &buffer_listener, &c->buffers[i]);
		/* Both buffers hold the SAME solid colour, so which one the compositor
		 * hands back is irrelevant and every composited frame is identical --
		 * exactly the static scene the SSIM proof wants. */
		for (size_t p = 0; p < size / 4; p++) {
			c->buffers[i].pixels[p] = c->pixel;
		}
	}
	wl_shm_pool_destroy(pool);
	close(fd);
	c->buffers_ready = true;
	return true;
}

static struct buffer *buffer_take(struct client *c) {
	for (int i = 0; i < BUFFER_COUNT; i++) {
		if (!c->buffers[i].busy) {
			return &c->buffers[i];
		}
	}
	return NULL;
}

/* ---- the draw loop --------------------------------------------------- */

static void draw(struct client *c);

static void frame_done(void *data, struct wl_callback *cb, uint32_t time) {
	(void)time;
	wl_callback_destroy(cb);
	/* Re-commit the same solid frame on the cadence the compositor offers.
	 * The content never changes, so this keeps the surface live (and answers
	 * the frame clock) without ever altering a pixel. */
	draw(data);
}

static const struct wl_callback_listener frame_listener = {.done = frame_done};

static void draw(struct client *c) {
	if (g_stop || c->closed) {
		return;
	}
	struct buffer *b = buffer_take(c);
	if (b == NULL) {
		/* Both buffers still held; ask for another callback rather than
		 * dropping out of the frame loop. */
		struct wl_callback *cb = wl_surface_frame(c->surface);
		wl_callback_add_listener(cb, &frame_listener, c);
		wl_surface_commit(c->surface);
		return;
	}
	wl_surface_attach(c->surface, b->wl, 0, 0);
	wl_surface_damage_buffer(c->surface, 0, 0, c->width, c->height);
	b->busy = true;
	c->commits++;

	struct wl_callback *cb = wl_surface_frame(c->surface);
	wl_callback_add_listener(cb, &frame_listener, c);
	wl_surface_commit(c->surface);
}

/* Parse a six-hex-digit RRGGBB colour into 0x00RRGGBB. Returns false on any
 * malformed input -- a client asked for a colour it cannot paint must fail
 * loudly, not silently paint black. */
static bool parse_colour(const char *s, uint32_t *out) {
	if (strlen(s) != 6) {
		return false;
	}
	char *end = NULL;
	unsigned long v = strtoul(s, &end, 16);
	if (end == NULL || *end != '\0') {
		return false;
	}
	*out = (uint32_t)v & 0x00ffffffu;
	return true;
}

/* One line of the confinement report for `path`, appended to `out`.
 *
 * `O_RDONLY` on a directory is legal and is what makes one probe verb serve
 * both a canary FILE in $HOME and a canary DIRECTORY like /dev/input. The
 * `fstat` is on the descriptor the `open` returned, never a second lookup by
 * name -- so what is reported is the identity of the object this process
 * actually opened. `O_NOCTTY` because one of the interesting probes is a tty
 * device and a client that acquired a controlling terminal by probing for one
 * would be reporting on a state it created. */
static void probe_one(FILE *out, const char *path) {
	int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOCTTY);
	if (fd < 0) {
		fprintf(out, "PROBE path=%s open=fail errno=%d dev=0 ino=0\n", path, errno);
		return;
	}
	struct stat st;
	if (fstat(fd, &st) != 0) {
		int e = errno;
		close(fd);
		/* Opened but not stattable: reported as its own outcome rather than
		 * folded into either side, because a reader must not take it for
		 * "unreachable". */
		fprintf(out, "PROBE path=%s open=nostat errno=%d dev=0 ino=0\n", path, e);
		return;
	}
	close(fd);
	fprintf(out, "PROBE path=%s open=ok errno=0 dev=%" PRIu64 " ino=%" PRIu64 "\n", path,
			(uint64_t)st.st_dev, (uint64_t)st.st_ino);
}

/* Can this process create a NESTED user namespace?
 *
 * The one thing on this page that is a fact about a syscall rather than about a
 * path, and it has to be here because `vitrin-realm-init`'s K9b writes 0 to the
 * realm's own `/proc/sys/user/max_user_namespaces` -- so a realm's app is
 * refused at `unshare(CLONE_NEWUSER)` instead of much later, at the first
 * `mount(2)` its nested sandbox attempts.
 *
 * IN A FORKED CHILD, and that is not tidiness. A successful
 * `unshare(CLONE_NEWUSER)` replaces the caller's credentials with an unmapped
 * uid: this process would come out of it as `overflowuid`, unable to write its
 * own report or to open the Wayland socket, so the control run -- the one that
 * must SUCCEED -- would destroy the evidence it exists to produce. The child
 * reports through a pipe rather than through its exit status, because an exit
 * status is 8 bits and an errno deserves the whole int.
 *
 * `created=error` is its own outcome, distinct from `no`: a `fork` or `pipe`
 * that failed says nothing about user namespaces, and a reader must not take it
 * for a refusal. */
static void probe_userns(FILE *out) {
	int pipefd[2];
	if (pipe2(pipefd, O_CLOEXEC) != 0) {
		fprintf(out, "PROBE-USERNS created=error errno=%d\n", errno);
		return;
	}
	pid_t pid = fork();
	if (pid < 0) {
		int e = errno;
		close(pipefd[0]);
		close(pipefd[1]);
		fprintf(out, "PROBE-USERNS created=error errno=%d\n", e);
		return;
	}
	if (pid == 0) {
		close(pipefd[0]);
		int result = (unshare(CLONE_NEWUSER) == 0) ? 0 : errno;
		ssize_t ignored = write(pipefd[1], &result, sizeof(result));
		(void)ignored;
		_exit(0);
	}
	close(pipefd[1]);
	int result = 0;
	ssize_t got = read(pipefd[0], &result, sizeof(result));
	close(pipefd[0]);
	int status = 0;
	while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
		/* retry */
	}
	if (got != (ssize_t)sizeof(result)) {
		/* The child died before it could say anything: not a refusal. */
		fprintf(out, "PROBE-USERNS created=error errno=0\n");
		return;
	}
	fprintf(out, "PROBE-USERNS created=%s errno=%d\n", result == 0 ? "yes" : "no", result);
}

/* ---------------------------------------------------------------------------
 * The network-namespace probe (P2.7.1, issue #195)
 * ---------------------------------------------------------------------------
 *
 * The realm's network confinement is one clone flag and one ioctl:
 * `CLONE_NEWNET` in `vitrin-realm-init`'s single `unshare`, and a
 * `SIOCSIFFLAGS` bringing `lo` up. That is a CONFIGURATION rather than a
 * mechanism -- PRD Doc 2 §12 -- and these five lines are what measures it
 * from inside, because every part of it is invisible from the host.
 *
 * WHY `lo` IS UP AND WHY THAT IS NOT A HOLE. Firefox, dbus-daemon and the
 * accessibility stack all bind or connect on 127.0.0.1 for their own
 * plumbing, and a realm whose loopback is down fails in ways that look like
 * Vitrin bugs. It is safe because nothing of the HOST's is listening on it:
 * "`ssh localhost` reaches the realm's own empty loopback" is a claim about
 * what is BOUND, not about what is routable. So this probe reports both --
 * that loopback works (or the realm is broken) and that the host's is
 * unreachable (or the confinement is).
 *
 * WHY THE HOST HALF IS NOT HERE. `--net-connect` and `--net-abstract` report
 * an attempt and its errno and nothing else. Whether the target was
 * reachable AT ALL is the harness's to establish, from outside, in the same
 * run -- `tests/integration/test_real_confinement.py`. A refusal against a
 * port nothing was listening on proves nothing, and this file cannot tell
 * the two apart: it is inside the realm, which is the whole point. */

/* PROBE-NET-IFACES names=<comma-separated>: every interface in this
 * process's network namespace, from `/proc/self/net/dev`.
 *
 * Reported as a SET for the gate to compare exactly, never as a
 * "does it contain eth0" question: a host that happened to name an interface
 * something else would walk straight through a containment check, and a
 * realm is supposed to have exactly one. `names=` with nothing after it is a
 * namespace with no interfaces, which is distinguishable from
 * `names=error`. */
static void probe_net_ifaces(FILE *out) {
	FILE *dev = fopen("/proc/self/net/dev", "re");
	if (dev == NULL) {
		fprintf(out, "PROBE-NET-IFACES names=error errno=%d\n", errno);
		return;
	}
	char line[512];
	int lineno = 0;
	int written = 0;
	fprintf(out, "PROBE-NET-IFACES names=");
	while (fgets(line, sizeof(line), dev) != NULL) {
		/* Two header lines, then one row per interface: leading spaces, the
		 * name, a colon, then counters. */
		if (++lineno <= 2) {
			continue;
		}
		char *start = line;
		while (*start == ' ' || *start == '\t') {
			start++;
		}
		char *colon = strchr(start, ':');
		if (colon == NULL) {
			continue;
		}
		*colon = '\0';
		fprintf(out, "%s%s", written++ ? "," : "", start);
	}
	fclose(dev);
	fprintf(out, "\n");
}

/* PROBE-NET-LO up=<yes|no> running=<yes|no> errno=<n>: `lo`'s flags, read
 * back rather than assumed from the helper having tried.
 *
 * UP and RUNNING are reported separately because they are different facts:
 * UP is the administrative flag the helper sets, RUNNING is the kernel
 * saying the interface is actually operational. A gate that only checked UP
 * would pass on an interface configured but not carrying. */
static void probe_net_lo(FILE *out) {
	int sock = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
	if (sock < 0) {
		fprintf(out, "PROBE-NET-LO up=error running=error errno=%d\n", errno);
		return;
	}
	struct ifreq req;
	memset(&req, 0, sizeof(req));
	strncpy(req.ifr_name, "lo", IFNAMSIZ - 1);
	if (ioctl(sock, SIOCGIFFLAGS, &req) < 0) {
		int e = errno;
		close(sock);
		fprintf(out, "PROBE-NET-LO up=error running=error errno=%d\n", e);
		return;
	}
	close(sock);
	fprintf(out, "PROBE-NET-LO up=%s running=%s errno=0\n",
			(req.ifr_flags & IFF_UP) ? "yes" : "no",
			(req.ifr_flags & IFF_RUNNING) ? "yes" : "no");
}

/* Connect `fd` to `addr` with a bounded wait, so a probe can never hang the
 * run it is reporting on. Returns 0, or the errno.
 *
 * Non-blocking plus `poll` rather than a bare `connect`: inside an empty
 * netns every interesting answer (ECONNREFUSED, ENETUNREACH) arrives at
 * once, but this file also runs OUTSIDE a realm as its own control, where a
 * blocking connect to something unreachable is a stall that reads as a
 * crashed app. */
static int connect_bounded(int fd, const struct sockaddr *addr, socklen_t len) {
	int flags = fcntl(fd, F_GETFL, 0);
	if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {
		return errno;
	}
	if (connect(fd, addr, len) == 0) {
		return 0;
	}
	if (errno != EINPROGRESS) {
		return errno;
	}
	struct pollfd pfd = {.fd = fd, .events = POLLOUT, .revents = 0};
	int ready;
	do {
		ready = poll(&pfd, 1, 2000);
	} while (ready < 0 && errno == EINTR);
	if (ready == 0) {
		return ETIMEDOUT;
	}
	if (ready < 0) {
		return errno;
	}
	int err = 0;
	socklen_t errlen = sizeof(err);
	if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &errlen) < 0) {
		return errno;
	}
	if (err == 0) {
		/* Put it back. The caller may write on this socket, and a write that
		 * returned EAGAIN because the probe left it non-blocking would be a
		 * failure invented by the measurement. */
		(void)fcntl(fd, F_SETFL, flags);
	}
	return err;
}

/* PROBE-NET-LOOPBACK rc=<0|-1> errno=<n>: a byte round-trip between two
 * processes inside THIS namespace, over 127.0.0.1.
 *
 * This is what separates "there is a network namespace" from "there is a
 * network namespace with a working loopback". Both halves are needed: the
 * negatives below are equally satisfied by a realm whose networking is
 * simply broken, and a gate that only asserted the negatives would call that
 * confinement. */
static void probe_net_loopback(FILE *out) {
	int listener = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (listener < 0) {
		fprintf(out, "PROBE-NET-LOOPBACK rc=-1 errno=%d\n", errno);
		return;
	}
	struct sockaddr_in addr;
	memset(&addr, 0, sizeof(addr));
	addr.sin_family = AF_INET;
	addr.sin_port = 0; /* any free port */
	addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	socklen_t addrlen = sizeof(addr);
	if (bind(listener, (struct sockaddr *)&addr, addrlen) < 0 || listen(listener, 1) < 0 ||
			getsockname(listener, (struct sockaddr *)&addr, &addrlen) < 0) {
		int e = errno;
		close(listener);
		fprintf(out, "PROBE-NET-LOOPBACK rc=-1 errno=%d\n", e);
		return;
	}
	pid_t pid = fork();
	if (pid < 0) {
		int e = errno;
		close(listener);
		fprintf(out, "PROBE-NET-LOOPBACK rc=-1 errno=%d\n", e);
		return;
	}
	if (pid == 0) {
		int peer = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
		if (peer < 0) {
			_exit(1);
		}
		if (connect_bounded(peer, (struct sockaddr *)&addr, addrlen) != 0) {
			_exit(1);
		}
		ssize_t wrote = write(peer, "v", 1);
		close(peer);
		_exit(wrote == 1 ? 0 : 1);
	}
	/* BOUNDED, and the bound is not defensive tidiness: with `lo` DOWN the
	 * child's connect fails and the child exits, so a bare blocking `accept`
	 * waits for a peer that will never arrive and the whole run hangs until
	 * something kills it. That is not a hypothetical -- it is what this
	 * function did when it was first run inside a real `unshare -rn`, where
	 * "loopback is down" is precisely the state being measured. A probe must
	 * report the broken case, never become it. */
	struct pollfd waiting = {.fd = listener, .events = POLLIN, .revents = 0};
	int ready;
	do {
		ready = poll(&waiting, 1, 2000);
	} while (ready < 0 && errno == EINTR);
	int result = -1;
	int err = ETIMEDOUT;
	int served = -1;
	if (ready > 0) {
		served = accept(listener, NULL, NULL);
		err = errno;
	} else if (ready < 0) {
		err = errno;
	}
	if (served >= 0) {
		/* Bounded for the same reason the accept above is: the peer could
		 * connect and then die before writing, and an unbounded read would
		 * turn that into a hang instead of a reported failure. */
		struct timeval limit = {.tv_sec = 2, .tv_usec = 0};
		(void)setsockopt(served, SOL_SOCKET, SO_RCVTIMEO, &limit, sizeof(limit));
		char byte = 0;
		if (read(served, &byte, 1) == 1 && byte == 'v') {
			result = 0;
			err = 0;
		} else {
			err = errno;
		}
		close(served);
	}
	close(listener);
	int status = 0;
	while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
		/* retry */
	}
	fprintf(out, "PROBE-NET-LOOPBACK rc=%d errno=%d\n", result, err);
}

/* PROBE-NET-CONNECT target=<host:port> rc=<0|-1> errno=<n>: one TCP connect
 * to an address the HARNESS chose, reported with the exact errno.
 *
 * The errno is part of the claim and not colour: ECONNREFUSED means a stack
 * answered and nothing was listening, ENETUNREACH means there was no route
 * to try. Both are the confinement; a gate that accepted "not zero" would
 * also accept ETIMEDOUT from a firewall on the host, which is a different
 * and much weaker fact. */
static void probe_net_connect(FILE *out, const char *target) {
	char host[64];
	const char *sep = strrchr(target, ':');
	if (sep == NULL || (size_t)(sep - target) >= sizeof(host)) {
		fprintf(out, "PROBE-NET-CONNECT target=%s rc=-1 errno=%d\n", target, EINVAL);
		return;
	}
	memcpy(host, target, (size_t)(sep - target));
	host[sep - target] = '\0';
	int port = atoi(sep + 1);
	struct sockaddr_in addr;
	memset(&addr, 0, sizeof(addr));
	addr.sin_family = AF_INET;
	addr.sin_port = htons((uint16_t)port);
	if (inet_pton(AF_INET, host, &addr.sin_addr) != 1) {
		fprintf(out, "PROBE-NET-CONNECT target=%s rc=-1 errno=%d\n", target, EINVAL);
		return;
	}
	int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0) {
		fprintf(out, "PROBE-NET-CONNECT target=%s rc=-1 errno=%d\n", target, errno);
		return;
	}
	int err = connect_bounded(fd, (struct sockaddr *)&addr, sizeof(addr));
	close(fd);
	fprintf(out, "PROBE-NET-CONNECT target=%s rc=%d errno=%d\n", target, err == 0 ? 0 : -1, err);
}

/* PROBE-NET-ABSTRACT name=<nonce> rc=<0|-1> errno=<n>: one connect to an
 * abstract-namespace UNIX socket the harness bound on the host.
 *
 * The third of PRD Doc 2 §12's three socket closures, and the one that
 * belongs to the NETWORK namespace rather than the mount namespace: an
 * abstract socket has no filesystem path for a mount table to remove, and is
 * scoped to a netns instead. So this is the only one of the three that would
 * survive P2.6.2's mount work and still need this task. */
static void probe_net_abstract(FILE *out, const char *name) {
	struct sockaddr_un addr;
	memset(&addr, 0, sizeof(addr));
	addr.sun_family = AF_UNIX;
	size_t len = strlen(name);
	if (len + 1 > sizeof(addr.sun_path)) {
		fprintf(out, "PROBE-NET-ABSTRACT name=%s rc=-1 errno=%d\n", name, EINVAL);
		return;
	}
	/* Abstract: a leading NUL, then the name, and the address length stops at
	 * the end of the name -- it is NOT NUL-terminated the way a path is. */
	addr.sun_path[0] = '\0';
	memcpy(addr.sun_path + 1, name, len);
	socklen_t addrlen = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + 1 + len);
	int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0) {
		fprintf(out, "PROBE-NET-ABSTRACT name=%s rc=-1 errno=%d\n", name, errno);
		return;
	}
	int err = connect_bounded(fd, (struct sockaddr *)&addr, addrlen);
	close(fd);
	fprintf(out, "PROBE-NET-ABSTRACT name=%s rc=%d errno=%d\n", name, err == 0 ? 0 : -1, err);
}

/* ---------------------------------------------------------------------------
 * The syscall probe (P2.6.4, issue #188)
 * ------------------------------------------------------------------------- */

#ifndef AF_VSOCK
#define AF_VSOCK 40
#endif
#ifndef AF_ALG
#define AF_ALG 38
#endif
#ifndef KEY_SPEC_PROCESS_KEYRING
#define KEY_SPEC_PROCESS_KEYRING (-2)
#endif
#ifndef KEYCTL_GET_KEYRING_ID
#define KEYCTL_GET_KEYRING_ID 0
#endif
#ifndef ADDR_NO_RANDOMIZE
#define ADDR_NO_RANDOMIZE 0x0040000
#endif

/* One row's outcome. `rc` is the raw syscall return; `err` is errno when rc is
 * negative and 0 otherwise. */
struct syscall_result {
	long rc;
	int err;
};

/* Attempt one syscall IN THIS PROCESS. Only used for probes with no lasting
 * effect on the caller: every descriptor is closed again, and the keyring
 * probes land on the PROCESS keyring, which dies with the process. */
typedef struct syscall_result (*probe_fn)(void);

static struct syscall_result done(long rc) {
	struct syscall_result r;
	r.rc = rc;
	r.err = rc < 0 ? errno : 0;
	return r;
}

static struct syscall_result probe_keyctl(void) {
	/* create=1, so this SUCCEEDS unfiltered rather than answering ENOKEY --
	 * the positive control has to be a success, not merely a different error. */
	return done(syscall(SYS_keyctl, KEYCTL_GET_KEYRING_ID, KEY_SPEC_PROCESS_KEYRING, 1));
}

static struct syscall_result probe_add_key(void) {
	return done(syscall(SYS_add_key, "user", "vitrin188-probe", "x", (size_t)1,
			KEY_SPEC_PROCESS_KEYRING));
}

static struct syscall_result probe_request_key(void) {
	/* The key this looks for is the one `probe_add_key` just planted on the
	 * process keyring, so unfiltered this RETURNS A SERIAL rather than ENOKEY.
	 * Without that the row would only ever demonstrate an errno CHANGE, and a
	 * gate cannot tell a changed error from a denied call. */
	return done(syscall(SYS_request_key, "user", "vitrin188-probe", NULL, 0));
}

static struct syscall_result probe_io_uring_setup(void) {
	/* `struct io_uring_params` is 120 bytes and every field must be zero for a
	 * plain setup. Spelled as a byte array so this file does not need
	 * <linux/io_uring.h>, which is absent on older build hosts. */
	unsigned char params[120];
	memset(params, 0, sizeof(params));
	struct syscall_result r = done(syscall(SYS_io_uring_setup, 1, params));
	if (r.rc >= 0) {
		close((int)r.rc);
	}
	return r;
}

static struct syscall_result probe_socket_family(int family, int type) {
	struct syscall_result r = done(socket(family, type, 0));
	if (r.rc >= 0) {
		close((int)r.rc);
	}
	return r;
}

static struct syscall_result probe_socket_vsock(void) {
	return probe_socket_family(AF_VSOCK, SOCK_STREAM);
}
static struct syscall_result probe_socket_alg(void) {
	return probe_socket_family(AF_ALG, SOCK_SEQPACKET);
}
static struct syscall_result probe_socket_unix(void) {
	return probe_socket_family(AF_UNIX, SOCK_STREAM);
}
static struct syscall_result probe_socket_inet(void) {
	return probe_socket_family(AF_INET, SOCK_STREAM);
}
static struct syscall_result probe_socket_inet6(void) {
	return probe_socket_family(AF_INET6, SOCK_STREAM);
}
static struct syscall_result probe_socket_netlink(void) {
	return probe_socket_family(AF_NETLINK, SOCK_RAW);
}
static struct syscall_result probe_socket_packet(void) {
	/* NOT a demonstration of this filter: AF_PACKET needs CAP_NET_RAW, which
	 * the realm dropped at K10, so it already fails without any filter. It is
	 * reported so the gate can SAY that rather than leave it unmeasured. */
	return probe_socket_family(AF_PACKET, SOCK_RAW);
}

static struct syscall_result probe_ptrace(void) {
	/* PTRACE_TRACEME on the caller itself. In a realm the interesting target
	 * would be a sibling, but the honest claim this row makes is about the
	 * SYSCALL, and PTRACE_TRACEME is the one form that needs no second
	 * process and no permission of its own -- so a denial here is the filter
	 * and never yama or the pid namespace. Forked, because it makes the
	 * caller traced for life. */
	return done(ptrace(PTRACE_TRACEME, 0, NULL, NULL));
}

static struct syscall_result probe_perf_event_open(void) {
	/* A real `struct perf_event_attr`, not a hand-rolled one: a size mismatch
	 * produces a misleading errno, and an earlier probe that got this wrong
	 * reported EACCES for a call that in fact succeeds. A disabled software
	 * CPU-clock counter on the calling process is what an unprivileged caller
	 * is allowed at the default perf_event_paranoid. */
	struct perf_event_attr pe;
	memset(&pe, 0, sizeof(pe));
	pe.type = PERF_TYPE_SOFTWARE;
	pe.size = sizeof(pe);
	pe.config = PERF_COUNT_SW_CPU_CLOCK;
	pe.disabled = 1;
	pe.exclude_kernel = 1;
	pe.exclude_hv = 1;
	struct syscall_result r = done(syscall(SYS_perf_event_open, &pe, 0, -1, -1, 0));
	if (r.rc >= 0) {
		close((int)r.rc);
	}
	return r;
}

static struct syscall_result probe_bpf(void) {
	union bpf_attr attr;
	memset(&attr, 0, sizeof(attr));
	attr.map_type = BPF_MAP_TYPE_ARRAY;
	attr.key_size = 4;
	attr.value_size = 4;
	attr.max_entries = 1;
	struct syscall_result r = done(syscall(SYS_bpf, BPF_MAP_CREATE, &attr, sizeof(attr)));
	if (r.rc >= 0) {
		close((int)r.rc);
	}
	return r;
}

static struct syscall_result probe_userfaultfd(void) {
	struct syscall_result r = done(syscall(SYS_userfaultfd, O_CLOEXEC));
	if (r.rc >= 0) {
		close((int)r.rc);
	}
	return r;
}

static struct syscall_result probe_personality(void) {
	/* ADDR_NO_RANDOMIZE: the value the row exists for. Forked, because it
	 * changes the persona of the caller and everything it execs. */
	return done(syscall(SYS_personality, (unsigned long)ADDR_NO_RANDOMIZE));
}

static struct syscall_result probe_personality_query(void) {
	/* 0xffffffff is the READ form glibc and setarch issue. The predicate keeps
	 * it, so this is a standing negative control: if it ever fails, the
	 * argument predicate has become a blanket denial. */
	return done(syscall(SYS_personality, 0xffffffffUL));
}

static struct syscall_result probe_mbind(void) {
	void *mem = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (mem == MAP_FAILED) {
		struct syscall_result r = {-1, errno};
		return r;
	}
	/* MPOL_DEFAULT with a NULL nodemask: legal, and a no-op on the policy. */
	struct syscall_result r =
			done(syscall(SYS_mbind, mem, (unsigned long)4096, 0, NULL, 0UL, 0U));
	munmap(mem, 4096);
	return r;
}

static struct syscall_result probe_set_mempolicy(void) {
	return done(syscall(SYS_set_mempolicy, 0 /* MPOL_DEFAULT */, NULL, 0UL));
}

static struct syscall_result probe_move_pages(void) {
	/* nr_pages = 0: the kernel walks an empty list and returns 0, so this
	 * touches no memory and still exercises the syscall entry point. */
	return done(syscall(SYS_move_pages, 0, 0UL, NULL, NULL, NULL, 0));
}

static struct syscall_result probe_get_mempolicy(void) {
	/* A standing NEGATIVE control. Firefox's own filter Allow()s this under
	 * `Required by libnuma for FFmpeg` while returning ENOSYS for the setters
	 * beside it, so a filter that denied it would break the acceptance app. */
	int mode = 0;
	return done(syscall(SYS_get_mempolicy, &mode, NULL, 0UL, NULL, 0UL));
}

static struct syscall_result probe_seccomp(void) {
	/* A standing NEGATIVE control, and the most consequential one: Firefox and
	 * Chromium install their own filters for content processes. Forked,
	 * because a filter cannot be removed. NO_NEW_PRIVS first, because an
	 * unprivileged seccomp(2) without it is EACCES for a reason that has
	 * nothing to do with vitrin -- inside a realm K10 has already set it, and
	 * outside one this is what makes the control run comparable. */
	struct sock_filter allow[] = {BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)};
	struct sock_fprog prog;
	prog.len = 1;
	prog.filter = allow;
	if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0) {
		return done(-1);
	}
	return done(syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &prog));
}

static struct syscall_result probe_fork(void) {
	/* A standing NEGATIVE control. This build denies neither clone nor clone3,
	 * and glibc >= 2.34 issues clone3 for fork(3) -- so if this ever fails,
	 * the table has grown a row that breaks every process a realm creates. */
	pid_t pid = fork();
	if (pid == 0) {
		_exit(0);
	}
	if (pid < 0) {
		return done(-1);
	}
	int status = 0;
	while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
		/* retry */
	}
	return done(0);
}

/* Run `f` in a forked child and report its result through a pipe.
 *
 * For the probes that change the caller: `ptrace(PTRACE_TRACEME)`,
 * `personality`, `seccomp`. `rc = -1, errno = 0` is the distinct "the child
 * died before it could say anything" outcome, which a reader must not take for
 * a refusal -- exactly the distinction `probe_userns` draws. */
static struct syscall_result in_a_child(probe_fn f) {
	int pipefd[2];
	struct syscall_result dead = {-1, 0};
	if (pipe2(pipefd, O_CLOEXEC) != 0) {
		return dead;
	}
	pid_t pid = fork();
	if (pid < 0) {
		close(pipefd[0]);
		close(pipefd[1]);
		return dead;
	}
	if (pid == 0) {
		close(pipefd[0]);
		struct syscall_result r = f();
		ssize_t ignored = write(pipefd[1], &r, sizeof(r));
		(void)ignored;
		_exit(0);
	}
	close(pipefd[1]);
	struct syscall_result r = dead;
	ssize_t got = read(pipefd[0], &r, sizeof(r));
	close(pipefd[0]);
	int status = 0;
	while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
		/* retry */
	}
	if (got != (ssize_t)sizeof(r)) {
		return dead;
	}
	return r;
}

/* The probe table. `name` is the seccomp table's own `name` field for a denied
 * row, or a `never-denied` key, or one of the two rows reported so the gate can
 * SAY they are not demonstrations. `forked` marks the probes that change the
 * caller. */
static const struct {
	const char *name;
	probe_fn fn;
	bool forked;
} SYSCALL_PROBES[] = {
		/* Denied rows. */
		{"keyctl", probe_keyctl, false},
		{"add_key", probe_add_key, false},
		{"request_key", probe_request_key, false},
		{"io_uring_setup", probe_io_uring_setup, false},
		{"socket_family", probe_socket_vsock, false},
		{"ptrace", probe_ptrace, true},
		{"perf_event_open", probe_perf_event_open, false},
		{"bpf", probe_bpf, false},
		{"userfaultfd", probe_userfaultfd, false},
		{"personality", probe_personality, true},
		{"mbind", probe_mbind, false},
		{"set_mempolicy", probe_set_mempolicy, false},
		{"move_pages", probe_move_pages, false},
		/* The socket row's second demonstration, and the one it must NOT be
		 * demonstrated by. */
		{"socket_alg", probe_socket_alg, false},
		{"socket_packet", probe_socket_packet, false},
		/* Standing negative controls: these must SUCCEED inside a realm. */
		{"socket_unix", probe_socket_unix, false},
		{"socket_inet", probe_socket_inet, false},
		{"socket_inet6", probe_socket_inet6, false},
		{"socket_netlink", probe_socket_netlink, false},
		{"personality_query", probe_personality_query, true},
		{"get_mempolicy", probe_get_mempolicy, false},
		{"seccomp", probe_seccomp, true},
		{"fork", probe_fork, false},
};

static void probe_syscalls(FILE *out) {
	size_t n = sizeof(SYSCALL_PROBES) / sizeof(SYSCALL_PROBES[0]);
	for (size_t i = 0; i < n; i++) {
		struct syscall_result r = SYSCALL_PROBES[i].forked ? in_a_child(SYSCALL_PROBES[i].fn)
														   : SYSCALL_PROBES[i].fn();
		fprintf(out, "PROBE-SYSCALL name=%s rc=%ld errno=%d\n", SYSCALL_PROBES[i].name, r.rc,
				r.err);
	}
}

/* Write the whole report, and return false if anything about writing it
 * failed. A probe whose report never landed must not be mistaken for a probe
 * that ran, so the caller exits non-zero. */
static bool write_probe_report(const char *out_name, char *const *paths, int count, bool syscalls,
		const char *net_connect, const char *net_abstract) {
	char resolved[4096];
	if (out_name[0] == '/') {
		if ((size_t)snprintf(resolved, sizeof(resolved), "%s", out_name) >= sizeof(resolved)) {
			fprintf(stderr, "--probe-out path too long\n");
			return false;
		}
	} else {
		/* Relative names resolve against XDG_RUNTIME_DIR, which is the realm's
		 * own directory at both isolation settings and is bound read-WRITE
		 * into a confined realm. That is what lets one argv, byte for byte,
		 * serve a `--isolation=default` run and an `--isolation=off` run: the
		 * variable's VALUE differs (`/run/vitrin` vs the host path) and the
		 * host file it lands in is the same one either way. */
		const char *dir = getenv("XDG_RUNTIME_DIR");
		if (dir == NULL || dir[0] == '\0') {
			fprintf(stderr, "--probe-out is relative and XDG_RUNTIME_DIR is unset\n");
			return false;
		}
		if ((size_t)snprintf(resolved, sizeof(resolved), "%s/%s", dir, out_name) >=
				sizeof(resolved)) {
			fprintf(stderr, "--probe-out path too long\n");
			return false;
		}
	}
	int fd = open(resolved, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (fd < 0) {
		fprintf(stderr, "cannot write the probe report %s: %s\n", resolved, strerror(errno));
		return false;
	}
	FILE *out = fdopen(fd, "w");
	if (out == NULL) {
		close(fd);
		return false;
	}
	fprintf(out, "PROBE-VERSION 4\n");
	/* The supplementary groups this process still holds. D-037(5): they cannot
	 * be dropped -- `setgroups=deny` blocks the CALL and drops nothing -- so
	 * the realm keeps `video`, `render`, `input` and the rest as kgids, and
	 * the mount table is the only thing between it and the devices they open.
	 * Reported from inside so that residual is measured at the app rather than
	 * described in a document. */
	gid_t gids[NGROUPS_MAX];
	int n = getgroups(NGROUPS_MAX, gids);
	fprintf(out, "PROBE-GROUPS n=%d gids=", n);
	for (int i = 0; i < n; i++) {
		fprintf(out, "%s%u", i ? "," : "", (unsigned)gids[i]);
	}
	fprintf(out, "\n");
	probe_userns(out);
	/* Unconditional, on `PROBE-USERNS`'s own reasoning: these are facts about
	 * the namespace this process is IN, they cost two reads and one fork
	 * between them, and a report that carried them only on request would let
	 * the interesting run be the one nobody asked for. The two that take an
	 * argument cannot be unconditional -- only the harness knows what it
	 * bound. */
	probe_net_ifaces(out);
	probe_net_lo(out);
	probe_net_loopback(out);
	if (net_connect != NULL) {
		probe_net_connect(out, net_connect);
	}
	if (net_abstract != NULL) {
		probe_net_abstract(out, net_abstract);
	}
	for (int i = 0; i < count; i++) {
		probe_one(out, paths[i]);
	}
	/* Opt-in, unlike PROBE-GROUPS and PROBE-USERNS: this one forks four times
	 * and installs a seccomp filter in one of those children, which is more
	 * than every existing caller of `--probe` should pay for a fact it does
	 * not read. */
	if (syscalls) {
		probe_syscalls(out);
	}
	/* Last, so a truncated report is detectable rather than readable. */
	fprintf(out, "PROBE-END\n");
	if (fflush(out) != 0 || fsync(fileno(out)) != 0) {
		fclose(out);
		return false;
	}
	return fclose(out) == 0;
}

int main(int argc, char **argv) {
	int run_ms = 3000;
	uint32_t rgb = DEFAULT_COLOUR;
	char *probes[MAX_PROBES];
	int probe_count = 0;
	const char *probe_out = NULL;
	bool syscall_probe = false;
	const char *net_connect = NULL;
	const char *net_abstract = NULL;
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			run_ms = atoi(argv[++i]);
		} else if ((strcmp(argv[i], "--colour") == 0 || strcmp(argv[i], "--color") == 0) &&
				i + 1 < argc) {
			if (!parse_colour(argv[++i], &rgb)) {
				fprintf(stderr, "bad colour '%s' (expected six hex digits, e.g. 0000ff)\n", argv[i]);
				return 2;
			}
		} else if (strcmp(argv[i], "--probe") == 0 && i + 1 < argc) {
			if (probe_count == MAX_PROBES) {
				fprintf(stderr, "at most %d --probe paths\n", MAX_PROBES);
				return 2;
			}
			probes[probe_count++] = argv[++i];
		} else if (strcmp(argv[i], "--probe-out") == 0 && i + 1 < argc) {
			probe_out = argv[++i];
		} else if (strcmp(argv[i], "--syscall-probe") == 0) {
			syscall_probe = true;
		} else if (strcmp(argv[i], "--net-connect") == 0 && i + 1 < argc) {
			net_connect = argv[++i];
		} else if (strcmp(argv[i], "--net-abstract") == 0 && i + 1 < argc) {
			net_abstract = argv[++i];
		} else {
			fprintf(stderr,
					"usage: %s [--run-ms MS] [--colour RRGGBB] "
					"[--probe PATH]... [--syscall-probe] "
					"[--net-connect HOST:PORT] [--net-abstract NAME] [--probe-out FILE]\n",
					argv[0]);
			return 2;
		}
	}
	if (syscall_probe && probe_out == NULL) {
		/* Same rule as `--probe`: a probe with nowhere to report is a gate that
		 * reads an absent file as an absent finding. */
		fprintf(stderr, "--syscall-probe needs --probe-out\n");
		return 2;
	}
	if ((net_connect != NULL || net_abstract != NULL) && probe_out == NULL) {
		/* And the same rule again, for the same reason. These two are the
		 * only probes here whose target the harness chose, so a run that
		 * attempted them and reported nowhere would leave the harness holding
		 * a listener nobody ever tried to reach -- which reads exactly like a
		 * confinement. */
		fprintf(stderr, "--net-connect and --net-abstract need --probe-out\n");
		return 2;
	}
	if ((syscall_probe || net_connect != NULL || net_abstract != NULL) && probe_count == 0) {
		/* `--syscall-probe` alone is legal and is how the seccomp gate runs:
		 * it wants the PROBE-SYSCALL lines and no path probes. A bare
		 * `--net-connect`/`--net-abstract` is legal for the same reason. The
		 * pairing rule below is relaxed for exactly those cases and no other
		 * -- in particular a lone `--probe-out` is still refused, as it was
		 * before this task: something must have been ASKED for. */
		if (!write_probe_report(probe_out, NULL, 0, syscall_probe, net_connect, net_abstract)) {
			return 3;
		}
	} else if ((probe_count > 0) != (probe_out != NULL)) {
		/* Refused rather than defaulted: a `--probe` with nowhere to report is
		 * a gate that would read an absent file as an absent canary, and a
		 * `--probe-out` with nothing to probe writes a report whose emptiness
		 * means nothing. */
		fprintf(stderr, "--probe and --probe-out are used together or not at all\n");
		return 2;
	} else if (probe_count > 0 &&
			!write_probe_report(
					probe_out, probes, probe_count, syscall_probe, net_connect, net_abstract)) {
		return 3;
	}
	signal(SIGINT, on_signal);
	signal(SIGTERM, on_signal);

	struct client c = {
		.width = 0,
		.height = 0,
		.rgb = rgb,
		/* XRGB8888, opaque: little-endian memory bytes become B,G,R,X. */
		.pixel = 0xff000000u | rgb,
	};
	c.display = wl_display_connect(NULL);
	if (c.display == NULL) {
		fprintf(stderr, "cannot connect to the Wayland display\n");
		return 1;
	}
	c.registry = wl_display_get_registry(c.display);
	wl_registry_add_listener(c.registry, &registry_listener, &c);
	wl_display_roundtrip(c.display);

	if (c.compositor == NULL || c.shm == NULL || c.wm_base == NULL) {
		fprintf(stderr, "the compositor is missing wl_compositor, wl_shm or xdg_wm_base\n");
		return 1;
	}
	xdg_wm_base_add_listener(c.wm_base, &wm_base_listener, &c);

	c.surface = wl_compositor_create_surface(c.compositor);
	c.xdg_surface = xdg_wm_base_get_xdg_surface(c.wm_base, c.surface);
	xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c);
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
	xdg_toplevel_set_title(c.toplevel, "vitrin-solid-client");
	xdg_toplevel_set_app_id(c.toplevel, "org.vitrin.solid-client");
	wl_surface_commit(c.surface);

	/* Block until configured: the size the compositor hands back is the realm
	 * view, so the client's geometry becomes the core's without either end
	 * being told twice (damage_client.c's contract). */
	while (!c.configured && wl_display_dispatch(c.display) != -1) {
		if (g_stop) {
			return 0;
		}
	}
	if (c.width <= 0 || c.height <= 0) {
		fprintf(stderr, "the compositor configured no size\n");
		return 1;
	}
	if (!buffers_create(&c)) {
		return 1;
	}

	draw(&c);

	struct timespec start;
	clock_gettime(CLOCK_MONOTONIC, &start);
	while (!g_stop && !c.closed) {
		if (wl_display_dispatch(c.display) == -1) {
			break;
		}
		struct timespec now;
		clock_gettime(CLOCK_MONOTONIC, &now);
		long elapsed = (long)(now.tv_sec - start.tv_sec) * 1000 +
			(now.tv_nsec - start.tv_nsec) / 1000000;
		if (elapsed >= run_ms) {
			break;
		}
	}

	printf("CLIENT colour=%06x size=%dx%d\n", c.rgb, c.width, c.height);
	fflush(stdout);
	wl_display_disconnect(c.display);
	return 0;
}
