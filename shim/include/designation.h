/* designation.h -- the shim half of file designation: the per-realm
 * `designation.sock` relay (P2.6.7, issue #191).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * ====================================================================
 * WHAT THIS IS
 * ====================================================================
 *
 * The LAST hop of a designation. An agent holding `designate_file` asked, the
 * human chose in the core-drawn picker (P2.6.6), the core resolved the choice
 * once and sent the resulting descriptor twice: to the agent as
 * `vitrin_powerbox.designated`, and to this realm as
 * `vitrin_shim_session.designation` over fd 3 (P2.6.5, wire.c). The app is
 * what that descriptor is FOR, and the app has no connection to the core --
 * deliberately: it is the least-trusted process in the system (PRD Doc 2 §15's
 * first actor row), and a socket from it into the TCB would add attacker-facing
 * surface to the core for no gain. So this file serves one AF_UNIX socket
 * inside the realm's own runtime directory and hands each designation down it
 * over SCM_RIGHTS, and the app finds that socket the same way it finds the
 * Wayland one: at a fixed name under `$XDG_RUNTIME_DIR`.
 *
 * The contract the app is written against is documented in
 * `docs/protocol/09-vitrin_shim_session.md` (CC-BY-4.0) and decodable with the
 * Apache-2.0 generated header alone; this file is the implementation and
 * nothing a third party needs to read.
 *
 * ====================================================================
 * WHERE THE SOCKET IS, AND WHO ANNOUNCES IT: NOBODY
 * ====================================================================
 *
 * `designation.sock` is a SIBLING of the Wayland socket. The shim learns its
 * Wayland socket name from `--socket` or, failing that, from the
 * `WAYLAND_DISPLAY` it inherited (main.c's parse_args); the core passes no
 * flag and sets `WAYLAND_DISPLAY=/run/vitrin/wayland-0`. With an absolute
 * name, whichever way it arrived, the relay's path is
 * `dirname(socket)/designation.sock`; with a relative one (what the
 * acceptance scripts pass, and what libwayland resolves under
 * `$XDG_RUNTIME_DIR`) it is `$XDG_RUNTIME_DIR/designation.sock`. Neither the
 * core, the shim's argv, nor any environment variable names it -- there is no
 * flag and no `VITRIN_DESIGNATION_SOCKET`, and the app finds it by the same
 * convention that finds `wayland-0`. "Announced by nothing" is the claim, and
 * it is a deliberately weaker claim than "no path outside the realm names it":
 * at `--isolation=default` the host spelling of the realm's runtime directory
 * names the same inode, and pretending otherwise would be a confidentiality
 * claim this file cannot honour.
 *
 * Both ends of that convention are pinned: `crates/vitrin-realm-init`'s
 * `IN_REALM_DESIGNATION_SOCKET` is the in-realm spelling, and the Landlock
 * grant on `/run/vitrin` (MAKE_SOCK, REMOVE_FILE, RESOLVE_UNIX) is what lets
 * this bind and lets the app connect.
 *
 * ENAMETOOLONG. `sun_path` is 108 bytes, 107 of them usable. At
 * `--isolation=default` the path is the 28-byte constant and cannot overflow.
 * At `--isolation=off` it is
 * `$XDG_RUNTIME_DIR/vitrin-0/<realm_id>/designation.sock`, and with a 64-byte
 * realm id any `$XDG_RUNTIME_DIR` longer than 16 bytes overflows it:
 * `/run/user/1000000/...` (a seven-digit uid) is 108 bytes, one too many,
 * while a six-digit uid fits by exactly one byte. That is fatal at bring-up with a
 * message naming the remedy -- `--isolation=default`, where the path is the
 * constant -- rather than silently served from a shorter name, because a
 * fallback name is exactly the kind of "announced by nothing" the app could
 * not find. Recorded here, not in `docs/book/src/limits.md`: it is a property
 * of the debugging tier, not of the product.
 *
 * ====================================================================
 * THE SOCKET: SEQPACKET, 0700, STRICTER THAN `wayland-0`
 * ====================================================================
 *
 * SOCK_SEQPACKET, not SOCK_STREAM, so that "one message = one designation frame
 * + one fd" is enforced by the KERNEL rather than by a shim habit: a message
 * lands whole or not at all, and the app's `recvmsg` never reassembles a
 * stream or splits an fd batch from its frame. It is the same invariant
 * conventions §2.4 imposes on the core wire, restated one hop down where the
 * kernel can hold it for us. The consequence to know about: a connected app
 * that does not read pins designations -- each with its descriptor -- in the
 * kernel until THIS SOCKET'S SEND BUFFER is full (`net.core.wmem_default`,
 * 212992 bytes on a default-tuned kernel; a few hundred messages at the frame
 * sizes involved, 278 measured), then the next send fails EAGAIN and the
 * connection is dropped (below). It is NOT `net.unix.max_dgram_qlen`: the
 * kernel skips that per-message check for a peer that is connected back to
 * the sender (`unix_dgram_sendmsg`, `unix_peer(other) != sk`), which a
 * SEQPACKET connection is, so the byte bound is the one that fires. That
 * bound is the kernel's and the app's, never this process's.
 *
 * The node is mode 0700, set by `fchmod` on the socket BEFORE `bind` (there is
 * no umask dance: umask is process-wide and Mesa's worker threads exist by the
 * time this runs) and VERIFIED after `bind` with an `fstatat` that must show a
 * socket at exactly 0700, or bring-up fails. That makes the node STRICTER than
 * `wayland-0`, whose node mode is umask-derived and whose protection is the
 * 0700 directory around it. This file does not claim parity with the Wayland
 * socket; it claims 0700 on the node and checks it.
 *
 * THE STALE NODE. A crashed shim leaves its node behind, and a second shim on
 * the same name must not steal a LIVE one -- two `--no-upstream` shims in a
 * shared host runtime directory would otherwise silently take the fixed name
 * from each other. So before binding, a throwaway socket `connect`s to the
 * path: ENOENT means nothing is there; a connect that SUCCEEDS or reports
 * EAGAIN means another shim is serving this path, which fails bring-up naming
 * the path; ECONNREFUSED means NOTHING LIVE ANSWERS AT THIS NAME -- the kernel
 * says it both for a stale socket node (the previous shim's, left by a crash;
 * teardown never unlinks) and for ANY non-socket inode planted there (a
 * regular file, a FIFO, a directory, a symlink to one of those:
 * `unix_find_bsd` refuses every `!S_ISSOCK` inode with that errno). Only the
 * first of those is ours to remove, so on ECONNREFUSED an `lstat` decides:
 * a socket node is unlinked and bound over; anything else fails bring-up
 * naming what was found. One window is inherent to a probe and stated
 * rather than closed: a second hand-run shim caught between its own `bind`
 * and `listen` answers ECONNREFUSED too and would have its node taken; the
 * core never produces that case (it purges and `flock`s the per-realm
 * directory before the fork), so it is a property of two shims sharing a
 * developer's runtime dir, not of a realm. The throwaway socket is closed
 * immediately either way.
 *
 * THE PATH IS RESOLVED EXACTLY FOUR TIMES, ALL BEFORE THE APP IS FORKED: the
 * probe connect, the lstat that qualifies an ECONNREFUSED, the bind, the
 * post-bind fstatat. TEARDOWN NEVER TOUCHES THE
 * PATH. Once the app is alive it can unlink the node and plant anything at
 * that name, and an `open()` (or a `stat` that follows a symlink into a FIFO)
 * of a planted object would block the single-threaded shim at exit. Teardown
 * closes descriptors only; the stale node it leaves behind is what the probe
 * above exists to clean up next time.
 *
 * ====================================================================
 * ONE CONNECTION AT A TIME; FIRST HOLDS UNTIL IT CLOSES
 * ====================================================================
 *
 * Every process in the realm is one trust domain, so no rule about WHICH
 * connection receives designations changes any authority -- what a rule can
 * change is whether a losing connector finds out. First-holds is chosen
 * because the loser sees an immediate EOF (its connection is accepted and
 * closed, and the ledger records `refused_occupied`) rather than silently
 * displacing a holder that then waits forever for designations it will never
 * get. The failure shape this leaves, published in limits.md: a holder that
 * never reads fills the kernel queue, then the next send fails EAGAIN and
 * frees the slot.
 *
 * ====================================================================
 * NO SHIM-SIDE QUEUE
 * ====================================================================
 *
 * A designation arriving while no connection is held is CLOSED -- by the
 * transport, because this file never claims it -- and recorded
 * `outcome=no_client`. The IDL says close "immediately, on any path that does
 * not relay it", and it is right: any shim-side hold would pin a file open for
 * a time the app controls (until it connects, which may be never), which is
 * exactly the residue the core cannot recall. The ONLY queue is the kernel's,
 * owned by the held connection, bounded as above and released when the app
 * reads or the connection dies.
 *
 * ====================================================================
 * THE FD'S LIFETIME IN THIS PROCESS IS THE HANDLER'S STACK FRAME
 * ====================================================================
 *
 * `vitrin_designation_relay` NEVER claims the descriptor (never writes -1 back
 * through wire.c's `int *fd`), and designation.c contains no `close()` of a
 * designated fd at all. `sendmsg` with SCM_RIGHTS takes its own references to
 * the open file before queueing, and on every failure path keeps none -- so
 * the transport's unconditional close after the handler returns is correct on
 * success (the kernel holds the file for the app now) and on failure (nothing
 * was queued) alike. A relay that closed on success and forgot on failure is
 * the bug this shape makes unwritable; acceptance arm (G) in
 * `designation_relay.sh` is the test that would have caught it anyway.
 *
 * The descriptors this file DOES own -- the listener and its event-loop dup,
 * the held connection and its dup, and the retry timer -- are constant while a
 * connection is held, which is the state the acceptance census is taken in.
 *
 * ====================================================================
 * THE SOCKET ACCEPTS NOTHING
 * ====================================================================
 *
 * It is a delivery endpoint, not a request channel: designation originates at
 * the human's gesture, and an app cannot ask for a file over it. Anything the
 * app writes is discarded and the connection closed, recorded
 * `wrote_and_closed bytes=N` -- N being the whole message's length, which is
 * why the drain is `recv(2)` with MSG_TRUNC and not `read(2)`: a message
 * longer than the drain buffer still reports its full size. It is never a
 * `recvmsg` with a control buffer -- see the comment above it in
 * designation.c for the fd pressure an app could otherwise exert. A
 * zero-length message is a write too (SEQPACKET allows one), and `recv`
 * reports it exactly as it reports EOF, so the two are told apart with a
 * zero-timeout `poll` for POLLRDHUP: the peer's write side shut is `gone`,
 * anything else is `wrote_and_closed bytes=0`.
 */
#ifndef VITRIN_DESIGNATION_H
#define VITRIN_DESIGNATION_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h> /* pid_t (the connected peer, from SO_PEERCRED) */
#include <sys/un.h>    /* sizeof(((struct sockaddr_un *)0)->sun_path) */

#include <wayland-server-core.h>

#include "ledger.h"
#include "vitrin-protocol.h"

struct vitrin_shim;

/* The fixed basename. Both the core's realm-init crate
 * (`IN_REALM_DESIGNATION_SOCKET`) and the app-facing prose spell it; a change
 * here is a change to the app-facing contract. */
#define VITRIN_DESIGNATION_SOCKET_NAME "designation.sock"

/* How many connections the kernel will hold in the accept queue. One is
 * enough (one is all this file ever holds), but a burst of connectors that
 * all get refused is more useful to the app author as N recorded EOFs than as
 * EAGAIN from `connect` (or a hang, for a blocking connector) at the kernel,
 * which looks like a stuck shim rather than a refused one -- and is the same
 * answer the stale-node probe above reads as "live shim, full backlog". */
#define VITRIN_DESIGNATION_BACKLOG 8

/* How long the listener stays disarmed after an EMFILE/ENFILE `accept4`
 * before it is re-armed. The descriptor table being full is a condition
 * `accept4` cannot resolve and level-triggered epoll would re-report every
 * loop iteration; parking for this long is what keeps that from being a
 * 100 % spin. */
#define VITRIN_DESIGNATION_RETRY_MS 100

/* Longest path `bind` accepts: the `sun_path` array, NUL included. */
#define VITRIN_DESIGNATION_PATH_MAX (sizeof(((struct sockaddr_un *)0)->sun_path))

struct vitrin_designation {
	/* `vitrin_designation_init` ran to completion. Guards `finish` so the err
	 * path in main() (and the WLCS module, which never calls init) can call
	 * it after a partial bring-up: with `struct vitrin_shim s = {0}` every
	 * descriptor below starts at 0, a live stdin, and only this flag says
	 * whether they were ever ours. */
	bool active;

	/* The bound path, kept for the ledger and the log only. It is never
	 * re-resolved after init (see the header: teardown never touches it). */
	char path[VITRIN_DESIGNATION_PATH_MAX];
	/* `tier=in-realm` iff the socket's directory is exactly `/run/vitrin`,
	 * the constant the realm's mount namespace presents at
	 * `--isolation=default`; `host-path` otherwise. Recorded so a ledger
	 * reader knows whether "any process of this uid can connect first"
	 * (limits.md) applied to the run. */
	bool in_realm;

	/* The listening socket and its event source. The source holds the event
	 * loop's own dup of the descriptor, so teardown is remove-source-then-
	 * close-original, in that order (the pattern clipboard.c set). */
	int listen_fd;
	struct wl_event_source *listener;
	/* Created ONCE at init, so the shim's descriptor census does not change
	 * the first time the table fills; armed only from the EMFILE/ENFILE
	 * path, and disarmed by having fired. */
	struct wl_event_source *retry;
	/* The listener's mask is currently 0 (parked by the EMFILE/ENFILE path)
	 * and the retry timer is armed to restore it. */
	bool parked;
	/* The one ERROR line for a full descriptor table has been printed. Once
	 * per run, not per episode: ENFILE is system-wide, so any process on the
	 * host could otherwise make this shim repeat itself. */
	bool table_full_logged;

	/* The held connection, or -1 (with `client == NULL`) when the slot is
	 * free. Same remove-then-close discipline as the listener. */
	int conn_fd;
	struct wl_event_source *client;
	/* SO_PEERCRED of the held connection, for the ledger. `peer_is_app` is
	 * `peer_pid == s->app_pid` at accept time -- informational: every process
	 * in the realm is one trust domain, so a helper process connecting
	 * instead of the app's main pid is recorded, not refused. */
	pid_t peer_pid;
	bool peer_is_app;
};

/* Derive the path, probe the stale node, bind at 0700, verify, listen, and
 * register the listener (plus the retry timer) with the event loop.
 *
 * Runs right after `vitrin_ledger_init` and BEFORE `wl_display_add_socket` and
 * `vitrin_upstream_start`, in that order for two reasons: the ledger record
 * this emits needs the ledger, and the frames the core batched behind
 * `configure` are dispatched inside `vitrin_wire_arm` -- before the event loop
 * runs -- so a `designation` in that batch must land on an initialised relay
 * (as `no_client`, since no app can have connected yet), never on an
 * uninitialised one.
 *
 * Every failure is FATAL to bring-up, the same posture as the Wayland socket
 * bind failing: a realm whose app cannot receive designations is not the realm
 * the core asked for. The message names the path and the reason. */
bool vitrin_designation_init(struct vitrin_shim *s);

/* Relay one designation to the held connection, or record why not.
 *
 * `frame`/`len` are the byte-identical `vitrin_shim_session.designation` frame
 * the core sent (header + payload, verbatim), which is what crosses the socket
 * as one SEQPACKET message with `fd` as its one SCM_RIGHTS descriptor. `ev` is
 * that frame already decoded by the caller (upstream.c decodes it once for its
 * own log line; decoding twice would be two places to disagree) and supplies
 * the ledger record's id/kind/mode/name-length.
 *
 * OWNERSHIP. This never claims `fd`: the caller keeps it, and the transport
 * closes it the moment the handler returns, on every outcome. There is no
 * `int *fd` here on purpose, so the function cannot be given the means to
 * claim one.
 *
 * Returns the outcome (the grammar's `relayed | no_client | send_failed`), and
 * for `send_failed` stores the errno in `*err` (0 otherwise) so the caller's
 * log line can name it. `relayed` means the kernel accepted the message for
 * the held connection; receipt by the app is unobservable from here, by
 * design -- the socket has no reply path. */
enum vitrin_ledger_designation_outcome vitrin_designation_relay(struct vitrin_shim *s,
	const uint8_t *frame, size_t len, int fd,
	const vitrin_shim_session_evt_designation_t *ev, int *err);

/* Remove every event source, then close every original descriptor: the held
 * connection, the listener, the retry timer. Never unlinks the path (see the
 * header). Idempotent, and a no-op on a shim whose init never ran. Called
 * from `vitrin_shim_finish` BEFORE `wl_display_destroy`, because the sources
 * belong to the display's event loop. */
void vitrin_designation_finish(struct vitrin_shim *s);

#endif /* VITRIN_DESIGNATION_H */
