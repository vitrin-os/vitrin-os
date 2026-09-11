/* designation.c -- the per-realm `designation.sock` relay (P2.6.7, issue #191).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * See designation.h for what this is and the decisions it implements. This
 * file is the mechanism, in four parts:
 *
 *   1. bring-up: derive the path, probe the stale node, bind at 0700, verify,
 *      listen, register with the event loop (`vitrin_designation_init`);
 *   2. accept: one held connection at a time, first holds (`on_listener`);
 *   3. relay: one SEQPACKET message per designation, the core's frame
 *      verbatim plus one SCM_RIGHTS descriptor (`vitrin_designation_relay`);
 *   4. drain: whatever the app writes ends its connection (`on_client`).
 *
 * THE ONE RULE EVERY FUNCTION HERE OBEYS: no `close()` of a designated
 * descriptor, anywhere in this file. The descriptor's lifetime in this process
 * is the handler's stack frame (wire.c closes it the instant the handler
 * returns), and `sendmsg` either took its own reference to the file or took
 * nothing. The descriptors this file closes are its own: the listener, the
 * held connection, and the throwaway probe socket.
 *
 * Every event source here is torn down remove-source-then-close-original, the
 * pattern clipboard.c set: `wl_event_loop_add_fd` dups the descriptor and
 * `wl_event_source_remove` closes the dup, so closing the original first
 * leaves a registered dup on a file the shim no longer names -- and
 * `wl_event_source_remove` is safe from inside the source's own callback: it
 * closes the dup and sets the source's fd to -1 at once, the dispatch loop
 * skips such a source for the rest of that dispatch, and the free is
 * deferred to its end. That is where most of the removals below happen.
 */
/* _GNU_SOURCE rather than _POSIX_C_SOURCE: `accept4`, `SOCK_CLOEXEC`,
 * `SOCK_NONBLOCK` and `struct ucred` are Linux extensions. */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <wlr/util/log.h>

#include "designation.h"
#include "ledger.h"
#include "server.h"

/* The directory the realm's mount namespace presents its runtime dir at
 * (`crates/vitrin-realm-init`'s IN_REALM_RUNTIME_DIR). `tier=in-realm` in the
 * ledger means the socket's directory is EXACTLY this string -- no
 * normalisation of trailing slashes or `..`, because the core passes the
 * constant and anything else is, by definition, not the constant. */
#define VITRIN_DESIGNATION_IN_REALM_DIR "/run/vitrin"

/* ---- teardown primitives --------------------------------------------- */

/* Free the held-connection slot: remove the source, then close the original.
 * Idempotent, and the only place the connection is closed, so every path that
 * ends a connection -- EOF, a write, a read error, a failed send, teardown --
 * is the same four lines. */
static void drop_client(struct vitrin_designation *d) {
	if (d->client != NULL) {
		wl_event_source_remove(d->client);
		d->client = NULL;
	}
	if (d->conn_fd >= 0) {
		close(d->conn_fd);
		d->conn_fd = -1;
	}
	d->peer_pid = -1;
	d->peer_is_app = false;
}

/* ---- the drain: the socket accepts nothing ------------------------------ */

/* The held connection woke the loop, on ANY mask. The app is not supposed to
 * send anything, so readability means it did, and a hangup means it left.
 *
 * WHY A RECEIVE WITH NO CONTROL BUFFER, AND NEVER A `recvmsg` WITH ONE. The
 * app can attach SCM_RIGHTS descriptors to whatever it writes here. A
 * `recvmsg` whose `msg_control` is set would INSTALL those descriptors in the
 * shim's table -- app-controlled fd pressure on the process that confines it,
 * and a leak, because nothing in this file would ever close them (this file
 * closes no received descriptor, by the rule in the header). With
 * `msg_control == NULL`, which is what `recv` is, the kernel drops the
 * attached descriptors without installing them (`scm_recv_common` ->
 * `scm_destroy`, reported to a caller that could see it as MSG_CTRUNC). So
 * `recv` is not the lazy choice; it is the only receive call on this socket
 * that cannot be made to hand the shim a descriptor. Acceptance arm (E2)
 * sends exactly that message and checks the shim's table afterwards.
 *
 * `recv` WITH MSG_TRUNC rather than `read`, for one reason: the ledger's
 * `bytes=N` is the length of what the app wrote, and on SEQPACKET a message
 * longer than the buffer is cut to the buffer by `read` -- the excess is
 * discarded either way (one message, one receive), but MSG_TRUNC makes the
 * return value the message's real length instead of the buffer's.
 *
 * ONE receive per wakeup. After a non-zero receive the connection is closed,
 * so there is nothing to loop for; after EAGAIN the next wakeup retries. */
static int on_client(int fd, uint32_t mask, void *data) {
	struct vitrin_shim *s = data;
	struct vitrin_designation *d = &s->designation;

	uint8_t buf[256];
	ssize_t n = recv(fd, buf, sizeof(buf), MSG_TRUNC);
	if (n > 0) {
		/* The app WROTE. Discarded unread -- `buf` is never looked at -- and
		 * the connection ends: this is a delivery endpoint, not a request
		 * channel, and an app that treats it as one has misunderstood the
		 * contract in a way an EOF states more clearly than silence would.
		 * Only the byte count is recorded; the bytes are the app's. */
		vitrin_ledger_designation_client(s, VITRIN_LEDGER_DESIGNATION_WROTE_AND_CLOSED,
			-1, false, (size_t)n, 0);
		drop_client(d);
		return 0;
	}
	if (n == 0) {
		/* EOF -- or a zero-length message. SEQPACKET permits `send(fd, "", 0)`,
		 * with or without descriptors attached, and `recv` reports it exactly
		 * as it reports the peer's write side being shut; the event mask cannot
		 * tell them apart either (a half-close arrives as a plain readable with
		 * no HANGUP, like an empty message, and libwayland never maps
		 * EPOLLRDHUP). The kernel can: POLLRDHUP is set only when the peer has
		 * shut its write side. A zero-timeout poll never blocks; the one race --
		 * the app closing between the recv and the poll -- resolves to `gone`,
		 * which is then true. Both outcomes end the connection identically; the
		 * poll exists so the ledger attributes the end to the right party. */
		struct pollfd p = { .fd = fd, .events = POLLRDHUP };
		bool eof = poll(&p, 1, 0) > 0 && (p.revents & (POLLRDHUP | POLLHUP)) != 0;
		vitrin_ledger_designation_client(s,
			eof ? VITRIN_LEDGER_DESIGNATION_GONE : VITRIN_LEDGER_DESIGNATION_WROTE_AND_CLOSED,
			-1, false, 0, 0);
		drop_client(d);
		return 0;
	}
	int err = errno;
	if (err == EINTR) {
		return 0; /* the next wakeup retries */
	}
	if ((err == EAGAIN || err == EWOULDBLOCK) &&
			(mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR)) == 0) {
		/* Woken without data and without a hangup -- a spurious readable.
		 * Returning is safe ONLY because the mask carries neither HANGUP nor
		 * ERROR: epoll is level-triggered and delivers those two regardless
		 * of what was asked for, so a source left armed after either would
		 * re-fire every iteration and spin the shim at 100 %. */
		return 0;
	}
	/* Anything else: ECONNRESET (the app died with unread designations still
	 * queued -- measured: that is what the kernel reports, with
	 * EPOLLIN|EPOLLHUP|EPOLLERR), or EAGAIN alongside a HANGUP/ERROR that
	 * carried no bytes. The connection is over either way; the errno says
	 * how, and the queued designations died with the socket, which is the
	 * kernel releasing what it held and never this process holding more. */
	vitrin_ledger_designation_client(s, VITRIN_LEDGER_DESIGNATION_GONE,
		-1, false, 0, err);
	drop_client(d);
	return 0;
}

/* ---- accept: one connection, first holds --------------------------------- */

/* Re-arm the listener after an EMFILE/ENFILE park. */
static int on_retry(void *data) {
	struct vitrin_shim *s = data;
	struct vitrin_designation *d = &s->designation;
	if (d->listener != NULL) {
		wl_event_source_fd_update(d->listener, WL_EVENT_READABLE);
	}
	d->parked = false;
	return 0;
}

/* `accept4` failed with something that will not clear by itself. Drop the
 * listener's mask to 0 and arm the one-shot timer that restores it.
 *
 * Why not just return: epoll is level-triggered and the pending connection
 * stays pending, so a plain return would re-enter here on the very next loop
 * iteration, forever, at 100 % of a core -- with the app's own Wayland
 * connection starved behind it. Why not close the listener: the table being
 * full is a transient the shim may well survive (the app closing one file),
 * and a realm that permanently lost its designation socket over it would be
 * broken in a way nothing reports. Parking for VITRIN_DESIGNATION_RETRY_MS
 * costs the connector a delayed accept and nothing else.
 *
 * The timer was created at init and is only ARMED here, so the shim's
 * descriptor census (the acceptance script's leak predicate) is the same
 * before and after the first time this fires. */
static void park_listener(struct vitrin_shim *s, int err) {
	struct vitrin_designation *d = &s->designation;
	if (!d->table_full_logged) {
		/* Once per run, not per episode: ENFILE is system-wide, so any
		 * process on the host could otherwise make this shim repeat itself. */
		wlr_log(WLR_ERROR,
			"designation: accept4 on %s failed: %s; the listener is parked for "
			"%d ms and re-armed (this line is printed once per run)",
			d->path, strerror(err), VITRIN_DESIGNATION_RETRY_MS);
		d->table_full_logged = true;
	}
	if (d->parked) {
		return;
	}
	wl_event_source_fd_update(d->listener, 0);
	wl_event_source_timer_update(d->retry, VITRIN_DESIGNATION_RETRY_MS);
	d->parked = true;
}

static int on_listener(int fd, uint32_t mask, void *data) {
	(void)mask;
	struct vitrin_shim *s = data;
	struct vitrin_designation *d = &s->designation;

	int conn = accept4(fd, NULL, NULL, SOCK_CLOEXEC | SOCK_NONBLOCK);
	if (conn < 0) {
		switch (errno) {
		case EAGAIN:
#if EWOULDBLOCK != EAGAIN
		case EWOULDBLOCK:
#endif
		case ECONNABORTED:
		case EINTR:
			/* Nothing to accept after all (a spurious readable, a connector
			 * that gave up before we got to it, a signal). The next wakeup
			 * is a real one. */
			return 0;
		case EMFILE:
		case ENFILE:
		default:
			/* EMFILE/ENFILE are the named cases -- the table is full and
			 * `accept4` cannot make room. Every other errno goes the same way
			 * for the same reason: whatever it is, the pending connection is
			 * still pending, so returning would spin (see park_listener), and
			 * parking is the only answer that neither spins nor gives the
			 * socket up. */
			park_listener(s, errno);
			return 0;
		}
	}

	if (d->conn_fd >= 0) {
		/* FIRST HOLDS. A second connector is accepted and closed at once, so
		 * it sees EOF -- a fact it can act on -- rather than a hang, and rather
		 * than silently displacing a holder that would then wait forever for
		 * designations it will never get. Every process in the realm is one
		 * trust domain, so this rule changes nothing about authority; it only
		 * changes who finds out. Recorded, because "my app connected and got
		 * nothing" is a question this line answers. */
		close(conn);
		vitrin_ledger_designation_client(s, VITRIN_LEDGER_DESIGNATION_REFUSED_OCCUPIED,
			-1, false, 0, 0);
		return 0;
	}

	/* Who connected, for the ledger. Informational, never a gate: the app's
	 * helper process connecting instead of its main pid is still the app. */
	struct ucred cred = { .pid = -1 };
	socklen_t cred_len = sizeof(cred);
	pid_t peer = -1;
	if (getsockopt(conn, SOL_SOCKET, SO_PEERCRED, &cred, &cred_len) == 0) {
		peer = cred.pid;
	}

	d->client = wl_event_loop_add_fd(s->loop, conn, WL_EVENT_READABLE, on_client, s);
	if (d->client == NULL) {
		/* Out of memory in the event loop. The connector sees EOF, which is
		 * the same thing the occupied case gives it; the slot stays free for
		 * its retry. Logged rather than recorded: this is the shim failing,
		 * not the app doing anything. */
		wlr_log(WLR_ERROR, "designation: cannot watch an accepted connection; closed it");
		close(conn);
		return 0;
	}
	d->conn_fd = conn;
	d->peer_pid = peer;
	d->peer_is_app = peer > 0 && peer == s->app_pid;
	vitrin_ledger_designation_client(s, VITRIN_LEDGER_DESIGNATION_CONNECTED,
		peer, d->peer_is_app, 0, 0);
	return 0;
}

/* ---- the relay ------------------------------------------------------------ */

enum vitrin_ledger_designation_outcome vitrin_designation_relay(struct vitrin_shim *s,
		const uint8_t *frame, size_t len, int fd,
		const vitrin_shim_session_evt_designation_t *ev, int *err) {
	struct vitrin_designation *d = &s->designation;
	bool directory = ev->kind == VITRIN_POWERBOX_KIND_DIRECTORY;
	bool read_write = ev->mode == VITRIN_POWERBOX_MODE_READ_WRITE;
	*err = 0;

	if (!d->active || d->conn_fd < 0) {
		/* NO SHIM-SIDE QUEUE. Nothing is connected, so the descriptor is not
		 * relayed and not held: the caller leaves it for the transport, which
		 * closes it as this handler returns. Holding it "until the app
		 * connects" would pin a file for a time the app controls, which is the
		 * residue the core can never take back. */
		vitrin_ledger_designation_relay(s, ev->designation_id, directory, read_write,
			ev->name.len, VITRIN_LEDGER_DESIGNATION_NO_CLIENT, 0);
		return VITRIN_LEDGER_DESIGNATION_NO_CLIENT;
	}

	/* ONE message: the core's frame, byte for byte, as the only iov, and the
	 * descriptor as the only SCM_RIGHTS entry. Verbatim because the app-side
	 * decoder is the generated one, which takes the whole frame (header
	 * included) and validates `fd_count == 1` against the descriptor it was
	 * handed; re-encoding would be a second encoder for the same event with
	 * nothing to check it against. On SEQPACKET the kernel delivers this whole
	 * or not at all, which is what makes "one recvmsg = one designation" true
	 * for the app without any framing of its own. */
	struct iovec iov = {
		.iov_base = (void *)(uintptr_t)frame,
		.iov_len = len,
	};
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
	memcpy(CMSG_DATA(cm), &fd, sizeof(int));

	/* MSG_NOSIGNAL: an app that closed its end must not SIGPIPE the shim --
	 * EPIPE is the answer we want, and it is handled three lines down.
	 * MSG_DONTWAIT: the socket is non-blocking already; this states it at the
	 * one call where blocking would let the app stall the shim's event loop,
	 * and so the Wayland connection the same loop serves. */
	ssize_t n = sendmsg(d->conn_fd, &msg, MSG_NOSIGNAL | MSG_DONTWAIT);
	if (n < 0 && errno == EINTR) {
		n = sendmsg(d->conn_fd, &msg, MSG_NOSIGNAL | MSG_DONTWAIT);
	}
	if (n == (ssize_t)len) {
		/* The kernel took the message and its own reference to the file.
		 * From here the file stays open in the kernel's queue until the app's
		 * recvmsg installs it or the connection dies -- and the transport's
		 * close of `fd`, a moment from now, releases only this process's
		 * name for it. That is the whole of "relayed": accepted for the held
		 * connection, receipt unobservable, by design. */
		vitrin_ledger_designation_relay(s, ev->designation_id, directory, read_write,
			ev->name.len, VITRIN_LEDGER_DESIGNATION_RELAYED, 0);
		return VITRIN_LEDGER_DESIGNATION_RELAYED;
	}

	int e;
	if (n < 0) {
		/* EAGAIN: the app stopped reading and this socket's send buffer
		 * (`net.core.wmem_default` bytes of queued messages; designation.h
		 * says why `max_dgram_qlen` is not the bound) is full. EPIPE / ECONNRESET:
		 * the app is gone. ETOOMANYREFS: this uid's in-flight-descriptor
		 * budget is exhausted, possibly by another process of the same uid --
		 * the core, the agent, anything -- which is why it is a DoS surface
		 * of the whole SCM_RIGHTS plane and not of this relay. Whatever the
		 * errno, NOTHING WAS QUEUED and the kernel kept NO reference: the
		 * transport's close is the only one owed. */
		e = errno;
	} else {
		/* A partial send is impossible on SEQPACKET -- a message is atomic --
		 * so this branch is defensive code for a kernel this file does not
		 * expect to meet, at ERROR so that meeting one is not silent. */
		e = EIO;
		wlr_log(WLR_ERROR,
			"designation: sendmsg on a SEQPACKET socket returned %zd of %zu bytes, "
			"which cannot happen; treating it as EIO",
			n, len);
	}
	/* The connection is dropped on every send failure, including EAGAIN: a
	 * holder that has stopped reading is indistinguishable from one that
	 * never will, and freeing the slot lets a working process take it. The
	 * app that was dropped sees EOF -- and whatever was still in its kernel
	 * queue is released with the socket. */
	drop_client(d);
	vitrin_ledger_designation_relay(s, ev->designation_id, directory, read_write,
		ev->name.len, VITRIN_LEDGER_DESIGNATION_SEND_FAILED, e);
	*err = e;
	return VITRIN_LEDGER_DESIGNATION_SEND_FAILED;
}

/* ---- bring-up -------------------------------------------------------------- */

/* Where the socket goes: a sibling of the Wayland socket (designation.h).
 * Writes the path into `out` and reports whether its directory is the in-realm
 * constant. False, with the reason logged, when there is no directory to put
 * it in or the path does not fit `sun_path`. */
static bool derive_path(const struct vitrin_shim *s, char *out, size_t cap, bool *in_realm) {
	const char *sock = s->cfg.socket_name;
	char dir[VITRIN_DESIGNATION_PATH_MAX];

	if (sock != NULL && sock[0] == '/') {
		/* An absolute socket name (the core's `WAYLAND_DISPLAY`, or an
		 * absolute `--socket`; parse_args folds both into cfg): dirname of it. The
		 * corner where the socket lives in `/` itself gives dir "/", which
		 * the join below handles without producing "//". */
		const char *slash = strrchr(sock, '/');
		size_t dirlen = (size_t)(slash - sock);
		if (dirlen == 0) {
			dirlen = 1; /* "/" */
		}
		if (dirlen >= sizeof(dir)) {
			wlr_log(WLR_ERROR,
				"designation: the Wayland socket's directory is longer than an "
				"AF_UNIX path allows, so %s cannot live beside it",
				VITRIN_DESIGNATION_SOCKET_NAME);
			return false;
		}
		memcpy(dir, sock, dirlen);
		dir[dirlen] = '\0';
	} else {
		/* Relative `--socket` (what the acceptance scripts pass, and what
		 * libwayland resolves under $XDG_RUNTIME_DIR): the same directory.
		 * Neither available means there is nowhere to bind; libwayland is
		 * about to fail the Wayland bind for the same reason, so failing here
		 * first, with the reason named, costs nothing. */
		const char *runtime = getenv("XDG_RUNTIME_DIR");
		if (runtime == NULL || runtime[0] == '\0') {
			wlr_log(WLR_ERROR,
				"designation: the Wayland socket name '%s' is relative and "
				"XDG_RUNTIME_DIR is unset, so there is no directory to bind %s in "
				"(libwayland would refuse the Wayland socket for the same reason)",
				sock != NULL ? sock : "(null)", VITRIN_DESIGNATION_SOCKET_NAME);
			return false;
		}
		if (snprintf(dir, sizeof(dir), "%s", runtime) >= (int)sizeof(dir)) {
			wlr_log(WLR_ERROR,
				"designation: XDG_RUNTIME_DIR is longer than an AF_UNIX path allows");
			return false;
		}
	}

	int n = snprintf(out, cap, "%s%s%s", dir, strcmp(dir, "/") == 0 ? "" : "/",
		VITRIN_DESIGNATION_SOCKET_NAME);
	if (n < 0 || (size_t)n >= cap) {
		/* ENAMETOOLONG. Reachable at `--isolation=off`, where the runtime dir
		 * is `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>` and a 64-byte realm id
		 * under a seven-digit uid (`/run/user/1000000/...`) is one byte past
		 * what the 108-byte `sun_path` holds. Fatal with
		 * the remedy named, never served from a shorter name the app would
		 * not look for. */
		wlr_log(WLR_ERROR,
			"designation: %s/%s does not fit an AF_UNIX path (%zu bytes max). "
			"This happens at --isolation=off with a long realm id under a long "
			"runtime dir; the remedy is --isolation=default, where the path is "
			"the constant %s/%s",
			dir, VITRIN_DESIGNATION_SOCKET_NAME, cap - 1,
			VITRIN_DESIGNATION_IN_REALM_DIR, VITRIN_DESIGNATION_SOCKET_NAME);
		return false;
	}
	*in_realm = strcmp(dir, VITRIN_DESIGNATION_IN_REALM_DIR) == 0;
	return true;
}

/* The stale-node probe. Returns true with `*unlink_first` set when binding
 * may proceed; false, with the reason logged, when another shim is serving
 * the path or the probe could not tell. Never binds, never unlinks: it only
 * decides. */
static bool probe_node(const char *path, const struct sockaddr_un *addr, socklen_t addr_len,
		bool *unlink_first) {
	int probe = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
	if (probe < 0) {
		wlr_log(WLR_ERROR, "designation: cannot create the probe socket: %s", strerror(errno));
		return false;
	}
	int rc = connect(probe, (const struct sockaddr *)addr, addr_len);
	int err = rc == 0 ? 0 : errno;
	/* Closed before any decision is acted on: the probe exists to ask one
	 * question and must not be a second descriptor in the census. */
	close(probe);

	if (rc == 0 || err == EAGAIN) {
		/* Something ACCEPTED (or has a full backlog, which is the same
		 * thing): a live shim is serving this path. Stealing the name by
		 * unlinking it would leave that shim's app connecting to a node that
		 * no longer exists, silently. Two `--no-upstream` shims in one shared
		 * host runtime dir is exactly how this arises, and loud is right. */
		wlr_log(WLR_ERROR,
			"designation: another process is already serving %s; refusing to "
			"take its name (is a second shim running in this runtime dir?)",
			path);
		return false;
	}
	switch (err) {
	case ENOENT:
		*unlink_first = false;
		return true;
	case ECONNREFUSED:
		/* Nothing live answers at this name. The kernel says so both for a
		 * stale socket node -- the previous shim's, left by a crash; teardown
		 * never unlinks (designation.h says why) -- and for ANY non-socket
		 * inode at the path (`unix_find_bsd` answers ECONNREFUSED to every
		 * `!S_ISSOCK` inode: a regular file, a FIFO, a directory, a symlink to
		 * one). Only the first is ours to remove; the caller's lstat decides
		 * which this is before anything is unlinked. */
		*unlink_first = true;
		return true;
	default:
		/* EPROTOTYPE (a STREAM or DGRAM server on our name), EACCES, ...
		 * Whatever it is, this process does not understand the node well
		 * enough to remove it, and fail-closed here is one message away from
		 * a fix. */
		wlr_log(WLR_ERROR,
			"designation: cannot tell whether %s is stale (connect: %s); refusing "
			"to bind over it",
			path, strerror(err));
		return false;
	}
}

bool vitrin_designation_init(struct vitrin_shim *s) {
	struct vitrin_designation *d = &s->designation;
	if (d->active) {
		return true;
	}
	if (s->loop == NULL) {
		wlr_log(WLR_ERROR, "designation: no event loop to register with (bring-up order)");
		return false;
	}

	d->listen_fd = -1;
	d->conn_fd = -1;
	d->peer_pid = -1;
	d->listener = NULL;
	d->retry = NULL;
	d->client = NULL;
	d->parked = false;
	d->table_full_logged = false;

	if (!derive_path(s, d->path, sizeof(d->path), &d->in_realm)) {
		return false;
	}

	struct sockaddr_un addr = { .sun_family = AF_UNIX };
	/* `derive_path` already bounded the path to sizeof(sun_path)-1, so this
	 * copy cannot truncate; the +1 carries the NUL, which bind wants. */
	memcpy(addr.sun_path, d->path, strlen(d->path) + 1);
	socklen_t addr_len = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(d->path) + 1);

	/* RESOLUTION ONE OF FOUR: the probe. */
	bool unlink_first = false;
	if (!probe_node(d->path, &addr, addr_len, &unlink_first)) {
		return false;
	}
	if (unlink_first) {
		/* RESOLUTION TWO OF FOUR: what is actually there. ECONNREFUSED covers
		 * a stale socket AND any planted non-socket inode (probe_node); only
		 * a socket node is this process's to remove. `AT_SYMLINK_NOFOLLOW`
		 * so a symlink is seen as a symlink and refused, never followed. A
		 * node that vanished between probe and lstat is simply gone. */
		struct stat stale;
		if (fstatat(AT_FDCWD, d->path, &stale, AT_SYMLINK_NOFOLLOW) != 0) {
			if (errno != ENOENT) {
				wlr_log(WLR_ERROR, "designation: cannot stat %s before removing it: %s",
					d->path, strerror(errno));
				return false;
			}
			unlink_first = false;
		} else if (!S_ISSOCK(stale.st_mode)) {
			wlr_log(WLR_ERROR,
				"designation: %s exists and is not a socket (mode %07o); refusing to "
				"remove something this shim did not create",
				d->path, (unsigned)stale.st_mode);
			return false;
		}
	}
	if (unlink_first && unlink(d->path) != 0 && errno != ENOENT) {
		wlr_log(WLR_ERROR, "designation: cannot remove the stale node %s: %s",
			d->path, strerror(errno));
		return false;
	}

	/* SEQPACKET (designation.h: the kernel enforces one-message-one-
	 * designation), close-on-exec from birth (the app must not inherit the
	 * listener; main.c asserts this flag before forking), non-blocking (the
	 * single event-loop thread must never block in accept). */
	int fd = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
	if (fd < 0) {
		wlr_log(WLR_ERROR, "designation: socket(AF_UNIX, SOCK_SEQPACKET): %s", strerror(errno));
		return false;
	}
	/* The node's mode, set on the socket BEFORE bind so that bind creates
	 * the node at (0700 & ~umask). Not a umask dance: umask is process-wide,
	 * and by this point wlroots' renderer has threads whose own file
	 * creation a temporary umask would race. The post-bind fstatat below is
	 * what turns "asked for 0700" into "is 0700". */
	if (fchmod(fd, 0700) != 0) {
		wlr_log(WLR_ERROR, "designation: fchmod(0700) on the socket: %s", strerror(errno));
		close(fd);
		return false;
	}
	/* RESOLUTION THREE OF FOUR: the bind. Any failure is fatal to bring-up,
	 * the same posture as the Wayland socket's bind failing: a realm whose
	 * app cannot be handed a designation is not the realm the core asked
	 * for. On a planted symlink bind fails EADDRINUSE without following it,
	 * which is the right answer. */
	if (bind(fd, (const struct sockaddr *)&addr, addr_len) != 0) {
		wlr_log(WLR_ERROR, "designation: cannot bind %s: %s", d->path, strerror(errno));
		close(fd);
		return false;
	}
	/* RESOLUTION FOUR OF FOUR: verify what bind created, without following
	 * a symlink. A node that is not a socket, or a socket whose mode is not
	 * exactly 0700 (a umask that masked owner bits, a filesystem that ignores
	 * modes), fails bring-up rather than serving from a node this file
	 * cannot describe honestly in its ledger record. */
	struct stat st;
	if (fstatat(AT_FDCWD, d->path, &st, AT_SYMLINK_NOFOLLOW) != 0) {
		wlr_log(WLR_ERROR, "designation: cannot stat %s after bind: %s", d->path, strerror(errno));
		close(fd);
		return false;
	}
	if (!S_ISSOCK(st.st_mode) || (st.st_mode & 07777) != 0700) {
		wlr_log(WLR_ERROR,
			"designation: %s is %s with mode %04o after bind; a socket at exactly "
			"0700 is required",
			d->path, S_ISSOCK(st.st_mode) ? "a socket" : "not a socket",
			(unsigned)(st.st_mode & 07777));
		close(fd);
		return false;
	}
	if (listen(fd, VITRIN_DESIGNATION_BACKLOG) != 0) {
		wlr_log(WLR_ERROR, "designation: listen on %s: %s", d->path, strerror(errno));
		close(fd);
		return false;
	}

	/* Registered at init, before the Wayland socket exists and before the
	 * core link is armed, so a `designation` the core batched behind
	 * `configure` -- dispatched inside `vitrin_wire_arm`, before the loop
	 * runs -- meets an initialised relay and is recorded `no_client`. The
	 * retry timer is created here too, and only ever armed later, so the
	 * descriptor census is the same before and after its first use. */
	d->listener = wl_event_loop_add_fd(s->loop, fd, WL_EVENT_READABLE, on_listener, s);
	d->retry = wl_event_loop_add_timer(s->loop, on_retry, s);
	if (d->listener == NULL || d->retry == NULL) {
		wlr_log(WLR_ERROR, "designation: cannot register %s with the event loop", d->path);
		if (d->listener != NULL) {
			wl_event_source_remove(d->listener);
			d->listener = NULL;
		}
		if (d->retry != NULL) {
			wl_event_source_remove(d->retry);
			d->retry = NULL;
		}
		close(fd);
		return false;
	}
	d->listen_fd = fd;
	d->active = true;

	/* Stated as fact, after the fstatat above made it one. */
	vitrin_ledger_designation_sock(s, d->path, d->in_realm);
	return true;
}

void vitrin_designation_finish(struct vitrin_shim *s) {
	struct vitrin_designation *d = &s->designation;
	if (!d->active) {
		return;
	}
	/* Descriptors only. The path is NOT unlinked: the app may have replaced
	 * the node with anything by now, and resolving that name again is the
	 * one thing teardown must not do (designation.h). The stale node this
	 * leaves is the next shim's probe's problem, and it knows how to tell. */
	drop_client(d);
	if (d->listener != NULL) {
		wl_event_source_remove(d->listener);
		d->listener = NULL;
	}
	if (d->retry != NULL) {
		wl_event_source_remove(d->retry);
		d->retry = NULL;
	}
	if (d->listen_fd >= 0) {
		close(d->listen_fd);
		d->listen_fd = -1;
	}
	d->parked = false;
	d->active = false;
}
