/* mock_core.c -- the CORE side of the shim wire, for SHIM-IN-ISOLATION unit
 * testing the C shim's upstream link (P1.6.2).
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * THIS IS A MOCK, NOT THE REAL CORE (emphasis added by P1.6.6, issue #106).
 * It is a strict hand-written stand-in that lets the shim be exercised on its
 * own, and every acceptance script that drives it (shim_globals_and_client.sh,
 * upstream_frame_path.sh, seat_input_replay.sh, firefox_bringup.sh) is a
 * shim-only unit check, NOT a milestone integration proof. The milestone
 * proofs run the shipped Rust `vitrind` against this same shim with no mock on
 * any seam -- tests/integration/test_real_app.py (weston, M1.2 exit gate),
 * test_real_gtk.py (GTK), and test_real_firefox.py (Firefox render + globals
 * ledger). When a claim needs the REAL core, it lives there; this file is for
 * exercising the shim standalone, quickly, without a Rust toolchain.
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
 * EACH COMMIT IS ALSO MEASURED, not just validated: the copy-in pass reports
 * an FNV-1a digest (did the pixels change?) and the DOMINANT COLOUR with its
 * coverage (what is on screen?). The second one is what the M1.2 verification
 * asks for -- "Firefox smoke: local page rendering a known solid color,
 * assert dominant color" -- and it is deliberately computed here, in the
 * core's own copy-in, rather than by a separate screenshot tool: what gets
 * asserted is then exactly the bytes that crossed the wire.
 *
 * Options exist to make the core behave badly on purpose, which is what the
 * backpressure criterion needs:
 *
 *   --frame-delay MS   sit on `frame_done` for MS -- a slow presenting core
 *   --defer-release    hold `buffer_done` until the NEXT commit, which is
 *                      the dmabuf passthrough timing regime and the case a
 *                      one-slot buffer pool would deadlock on
 *
 * INPUT INJECTION (P1.6.3). `--input FILE` plays a script of
 * `vitrin_shim_seat` events at the shim, which is what turns "the app
 * receives exactly the scripted synthetic events" into an assertion rather
 * than a hope. The script is deliberately written by the test rather than
 * generated here, so the expectation and the stimulus are the same text.
 *
 *   delay MS
 *   motion X Y             physical|emulated
 *   button EVDEV_CODE      pressed|released  physical|emulated
 *   scroll vertical|horizontal V120          physical|emulated
 *   key KEYSYM             pressed|released  physical|emulated
 *   text physical|emulated UTF-8 REST OF LINE  (\n, \t, \\, \xNN escapes)
 *
 * WIRE BATCHING. Consecutive script events with no `delay` between them are
 * written to the socket in ONE `send`, and a `delay` is a flush. That is
 * what makes the shim's pointer-frame grouping testable: `vitrin_shim_seat`
 * runs over SOCK_STREAM, which has no message boundaries, so "arrived in one
 * batch" means "arrived in one recvmsg" and nothing else. The real core
 * issues one sendmsg per event and relies on the kernel to coalesce the ones
 * it emits back-to-back -- true in practice, but a race, and a test that
 * depended on winning it would be flaky. Writing them together produces the
 * identical byte stream the shim sees when the race is won, deterministically.
 *
 * Every event is traced on the way out as `EV seat_send ... origin=...`, so
 * the harness can correlate the core's record of what it sent against the
 * shim's record of what it delivered and the client's record of what
 * arrived -- three independent witnesses, which is the only way the origin
 * tag's survival (B2) is checkable without a shim -> core message that does
 * not exist.
 *
 * Playback starts once the shim has forwarded `--input-after-commits`
 * frames, because a commit is the first observable proof that the app has
 * mapped a window: firing input at a realm with nothing in it would test
 * the drop path, not the delivery path.
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
#define MAX_INPUT_EVENTS 128
/* `vitrin_shim_seat.text` is capped at 4096 bytes by the IDL; the encoder
 * refuses more, so the script parser refuses more too. */
#define MAX_INPUT_TEXT 4096

enum input_kind {
	INPUT_DELAY = 0,
	INPUT_MOTION,
	INPUT_BUTTON,
	INPUT_SCROLL,
	INPUT_KEY,
	INPUT_TEXT,
};

/* One line of an `--input` script, parsed up front so a malformed script is
 * a startup failure rather than a mid-run surprise. */
struct input_event {
	enum input_kind kind;
	uint32_t origin;
	double x, y;      /* motion */
	uint32_t code;    /* button code / keysym */
	uint32_t state;   /* pressed / released */
	uint32_t axis;    /* scroll */
	int32_t value120; /* scroll */
	int delay_ms;     /* delay */
	char text[MAX_INPUT_TEXT];
	size_t text_len;
};

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
	/* Idle inhibition (WS-E.4.4, issue #306): what the shim has told this
	 * peer, and how many edges it has sent. The real core keeps exactly this
	 * one bit per realm and no count; the edge counter is this peer's own, so
	 * a shim relaying levels instead of edges is observable. */
	bool idle_held;
	unsigned long long idle_edges;

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

	/* scripted seat input */
	struct input_event input[MAX_INPUT_EVENTS];
	int input_len;
	int input_next;
	uint64_t input_after_commits;
	int64_t input_due_ms; /* -1 until playback has been armed */
	uint64_t seat_sends;
	/* Events encoded but not yet written: everything the script asked for
	 * since the last `delay`. Big enough for two maximal `text` payloads, so
	 * a burst of ordinary pointer/key events is never split by capacity;
	 * anything larger flushes early, which costs a batch boundary and
	 * nothing else. */
	uint8_t seat_batch[2 * (MAX_INPUT_TEXT + 64)];
	size_t seat_batch_len;

	/* --dump-frame: write the Nth committed frame out as a binary PPM, for
	 * looking at with human eyes. The acceptance criteria are all asserted
	 * on bytes (dominant colour, digest, damage), which is what makes them
	 * mechanical -- but "Firefox renders in the realm" is a claim somebody
	 * will eventually want to SEE rather than read a hex triple of, and
	 * nothing else in the tree can produce a picture: vitrind never spawns a
	 * shim yet, so there is no nested window to screenshot.
	 *
	 * PPM (P6) rather than PNG deliberately: a header plus raw RGB bytes
	 * needs no codec, and this binary links nothing but libc on purpose --
	 * the shim CI job must not pull a Rust toolchain, and it should not grow
	 * an image library either. Converting to PNG is the viewer's problem. */
	const char *dump_path;
	uint64_t dump_at;

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

/* One core, at file scope rather than on main's stack: the parsed input
 * script lives inside it and a script of 128 maximal `text` payloads is
 * half a megabyte, which is not a stack frame. Parsing straight into it
 * also means the script is loaded before the fork, with no copy. */
static struct core g_core;

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

/* ---- scripted seat input (core -> shim) ------------------------------- */

static const char *origin_name(uint32_t o) {
	return o == VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL ? "physical" : "emulated";
}

static bool parse_origin(const char *tok, uint32_t *out) {
	if (tok == NULL) {
		return false;
	}
	if (strcmp(tok, "physical") == 0) {
		*out = VITRIN_SHIM_SEAT_ORIGIN_PHYSICAL;
		return true;
	}
	if (strcmp(tok, "emulated") == 0) {
		*out = VITRIN_SHIM_SEAT_ORIGIN_EMULATED;
		return true;
	}
	return false;
}

static bool parse_state(const char *tok, uint32_t *out) {
	if (tok == NULL) {
		return false;
	}
	if (strcmp(tok, "pressed") == 0) {
		*out = 1;
		return true;
	}
	if (strcmp(tok, "released") == 0) {
		*out = 0;
		return true;
	}
	return false;
}

static int hexval(char ch) {
	if (ch >= '0' && ch <= '9') { return ch - '0'; }
	if (ch >= 'a' && ch <= 'f') { return ch - 'a' + 10; }
	if (ch >= 'A' && ch <= 'F') { return ch - 'A' + 10; }
	return -1;
}

/* Copy a script line's text payload, expanding the escapes the script format
 * defines. `\n` and `\t` matter specifically: the IDL requires the shim to
 * render them as Return and Tab, so a script has to be able to contain them
 * without containing a literal newline. `\xNN` matters for the opposite
 * reason: the shim must NOT render any other control character as a
 * keystroke, and a test of that has to be able to put a raw 0x08 or 0x1b in
 * a payload while the script file itself stays readable. */
static size_t unescape(const char *in, char *out, size_t cap) {
	size_t n = 0;
	for (const char *p = in; *p != '\0' && n + 1 < cap; p++) {
		if (*p != '\\' || p[1] == '\0') {
			out[n++] = *p;
			continue;
		}
		p++;
		switch (*p) {
		case 'n': out[n++] = '\n'; break;
		case 't': out[n++] = '\t'; break;
		case '\\': out[n++] = '\\'; break;
		case 'x': {
			/* Exactly two hex digits. `hi >= 0` proves p[1] is not the
			 * terminator, so reading p[2] stays inside the string. */
			int hi = hexval(p[1]);
			int lo = hi >= 0 ? hexval(p[2]) : -1;
			if (lo < 0) {
				goto literal;
			}
			out[n++] = (char)(unsigned char)(hi * 16 + lo);
			p += 2;
			break;
		}
		default:
		literal:
			out[n++] = '\\';
			if (n + 1 < cap) {
				out[n++] = *p;
			}
			break;
		}
	}
	out[n] = '\0';
	return n;
}

/* Parse the whole script before anything is spawned. A bad script must fail
 * the run loudly, not silently deliver fewer events than the assertions
 * expect. */
static bool load_input_script(struct core *c, const char *path) {
	FILE *f = fopen(path, "r");
	if (f == NULL) {
		fprintf(stderr, "cannot open input script %s: %s\n", path, strerror(errno));
		return false;
	}
	char line[MAX_INPUT_TEXT + 256];
	int lineno = 0;
	bool ok = true;
	while (fgets(line, sizeof(line), f) != NULL) {
		lineno++;
		char *nl = strchr(line, '\n');
		if (nl != NULL) {
			*nl = '\0';
		}
		char *p = line;
		while (*p == ' ' || *p == '\t') {
			p++;
		}
		if (*p == '\0' || *p == '#') {
			continue;
		}
		if (c->input_len >= MAX_INPUT_EVENTS) {
			fprintf(stderr, "input script %s:%d: more than %d events\n",
				path, lineno, MAX_INPUT_EVENTS);
			ok = false;
			break;
		}
		struct input_event *ev = &c->input[c->input_len];
		memset(ev, 0, sizeof(*ev));

		char *save = NULL;
		char *verb = strtok_r(p, " \t", &save);
		if (verb == NULL) {
			continue;
		}
		if (strcmp(verb, "delay") == 0) {
			char *ms = strtok_r(NULL, " \t", &save);
			if (ms == NULL) { ok = false; }
			ev->kind = INPUT_DELAY;
			ev->delay_ms = ms != NULL ? atoi(ms) : 0;
		} else if (strcmp(verb, "motion") == 0) {
			char *xs = strtok_r(NULL, " \t", &save);
			char *ys = strtok_r(NULL, " \t", &save);
			char *os = strtok_r(NULL, " \t", &save);
			ok = ok && xs != NULL && ys != NULL && parse_origin(os, &ev->origin);
			ev->kind = INPUT_MOTION;
			ev->x = xs != NULL ? atof(xs) : 0;
			ev->y = ys != NULL ? atof(ys) : 0;
		} else if (strcmp(verb, "button") == 0) {
			char *cs = strtok_r(NULL, " \t", &save);
			char *ss = strtok_r(NULL, " \t", &save);
			char *os = strtok_r(NULL, " \t", &save);
			ok = ok && cs != NULL && parse_state(ss, &ev->state) &&
				parse_origin(os, &ev->origin);
			ev->kind = INPUT_BUTTON;
			ev->code = cs != NULL ? (uint32_t)strtoul(cs, NULL, 0) : 0;
		} else if (strcmp(verb, "scroll") == 0) {
			char *as = strtok_r(NULL, " \t", &save);
			char *vs = strtok_r(NULL, " \t", &save);
			char *os = strtok_r(NULL, " \t", &save);
			ok = ok && as != NULL && vs != NULL && parse_origin(os, &ev->origin);
			ev->kind = INPUT_SCROLL;
			ev->axis = (as != NULL && strcmp(as, "horizontal") == 0)
				? VITRIN_ACTUATOR_POINTER_AXIS_HORIZONTAL
				: VITRIN_ACTUATOR_POINTER_AXIS_VERTICAL;
			ev->value120 = vs != NULL ? (int32_t)strtol(vs, NULL, 0) : 0;
		} else if (strcmp(verb, "key") == 0) {
			char *ks = strtok_r(NULL, " \t", &save);
			char *ss = strtok_r(NULL, " \t", &save);
			char *os = strtok_r(NULL, " \t", &save);
			ok = ok && ks != NULL && parse_state(ss, &ev->state) &&
				parse_origin(os, &ev->origin);
			ev->kind = INPUT_KEY;
			ev->code = ks != NULL ? (uint32_t)strtoul(ks, NULL, 0) : 0;
		} else if (strcmp(verb, "text") == 0) {
			char *os = strtok_r(NULL, " \t", &save);
			ok = ok && parse_origin(os, &ev->origin);
			ev->kind = INPUT_TEXT;
			/* The rest of the line, verbatim -- spaces included. */
			ev->text_len = unescape(save != NULL ? save : "", ev->text, sizeof(ev->text));
		} else {
			fprintf(stderr, "input script %s:%d: unknown verb '%s'\n", path, lineno, verb);
			ok = false;
		}
		if (!ok) {
			fprintf(stderr, "input script %s:%d: malformed\n", path, lineno);
			break;
		}
		c->input_len++;
	}
	fclose(f);
	return ok;
}

/* Write everything the script has produced since the last flush as ONE
 * socket write, so the shim receives it in one recvmsg and the frame-grouping
 * rule it applies per batch is exercised deterministically. */
static void flush_seat_batch(struct core *c) {
	if (c->seat_batch_len == 0) {
		return;
	}
	size_t len = c->seat_batch_len;
	c->seat_batch_len = 0;
	if (!core_send(c, c->seat_batch, len)) {
		fail(c, "cannot send a %zu-byte seat event batch", len);
	}
}

static void send_seat_event(struct core *c, const struct input_event *ev) {
	uint8_t buf[MAX_INPUT_TEXT + 64];
	int32_t n = -1;

	switch (ev->kind) {
	case INPUT_MOTION: {
		vitrin_shim_seat_evt_motion_t m = {
			.x = vitrin_fixed_from_double(ev->x),
			.y = vitrin_fixed_from_double(ev->y),
			.origin = (vitrin_shim_seat_origin_t)ev->origin,
		};
		n = vitrin_shim_seat_evt_motion_encode(&m, c->seat_id, buf, sizeof(buf));
		if (n > 0) {
			trace("EV seat_send n=%llu event=motion origin=%s x=%.3f y=%.3f",
				(unsigned long long)(c->seat_sends + 1), origin_name(ev->origin),
				ev->x, ev->y);
		}
		break;
	}
	case INPUT_BUTTON: {
		vitrin_shim_seat_evt_button_t m = {
			.button = ev->code,
			.state = (vitrin_actuator_pointer_button_state_t)ev->state,
			.origin = (vitrin_shim_seat_origin_t)ev->origin,
		};
		n = vitrin_shim_seat_evt_button_encode(&m, c->seat_id, buf, sizeof(buf));
		if (n > 0) {
			trace("EV seat_send n=%llu event=button origin=%s button=%u state=%s",
				(unsigned long long)(c->seat_sends + 1), origin_name(ev->origin),
				ev->code, ev->state ? "pressed" : "released");
		}
		break;
	}
	case INPUT_SCROLL: {
		vitrin_shim_seat_evt_scroll_t m = {
			.axis = (vitrin_actuator_pointer_axis_t)ev->axis,
			.value120 = ev->value120,
			.origin = (vitrin_shim_seat_origin_t)ev->origin,
		};
		n = vitrin_shim_seat_evt_scroll_encode(&m, c->seat_id, buf, sizeof(buf));
		if (n > 0) {
			trace("EV seat_send n=%llu event=scroll origin=%s axis=%s value120=%d",
				(unsigned long long)(c->seat_sends + 1), origin_name(ev->origin),
				ev->axis == VITRIN_ACTUATOR_POINTER_AXIS_HORIZONTAL
					? "horizontal" : "vertical",
				ev->value120);
		}
		break;
	}
	case INPUT_KEY: {
		vitrin_shim_seat_evt_key_t m = {
			.keysym = ev->code,
			.state = (vitrin_shim_seat_key_state_t)ev->state,
			.origin = (vitrin_shim_seat_origin_t)ev->origin,
		};
		n = vitrin_shim_seat_evt_key_encode(&m, c->seat_id, buf, sizeof(buf));
		if (n > 0) {
			trace("EV seat_send n=%llu event=key origin=%s keysym=0x%08x state=%s",
				(unsigned long long)(c->seat_sends + 1), origin_name(ev->origin),
				ev->code, ev->state ? "pressed" : "released");
		}
		break;
	}
	case INPUT_TEXT: {
		vitrin_shim_seat_evt_text_t m = {
			.text = {.len = (uint32_t)ev->text_len, .data = (const uint8_t *)ev->text},
			.origin = (vitrin_shim_seat_origin_t)ev->origin,
		};
		n = vitrin_shim_seat_evt_text_encode(&m, c->seat_id, buf, sizeof(buf));
		if (n > 0) {
			trace("EV seat_send n=%llu event=text origin=%s bytes=%zu",
				(unsigned long long)(c->seat_sends + 1), origin_name(ev->origin),
				ev->text_len);
		}
		break;
	}
	case INPUT_DELAY:
	default:
		return;
	}

	if (n < 0) {
		fail(c, "cannot encode seat event kind %d", (int)ev->kind);
		return;
	}
	/* Append to the pending batch rather than writing now -- see the wire
	 * batching note at the top of this file. Flushing when the frame does
	 * not fit keeps the buffer bounded without ever splitting a frame. */
	if (c->seat_batch_len + (size_t)n > sizeof(c->seat_batch)) {
		flush_seat_batch(c);
	}
	memcpy(c->seat_batch + c->seat_batch_len, buf, (size_t)n);
	c->seat_batch_len += (size_t)n;
	c->seat_sends++;
}

/* Advance the script. Called once per poll iteration; playback is armed by
 * the shim's Nth commit and paced by the script's own `delay` entries. */
static void pump_input(struct core *c) {
	if (c->input_len == 0 || c->input_next >= c->input_len || !c->seat_created) {
		return;
	}
	if (c->input_due_ms < 0) {
		if (c->commits < c->input_after_commits) {
			return;
		}
		c->input_due_ms = now_ms();
		trace("EV input_armed after_commits=%llu events=%d",
			(unsigned long long)c->commits, c->input_len);
	}
	while (c->input_next < c->input_len && now_ms() >= c->input_due_ms) {
		const struct input_event *ev = &c->input[c->input_next++];
		if (ev->kind == INPUT_DELAY) {
			/* A `delay` is the batch boundary: everything before it goes out
			 * now, so the shim sees the pause the script asked for. */
			flush_seat_batch(c);
			c->input_due_ms = now_ms() + ev->delay_ms;
			continue;
		}
		send_seat_event(c, ev);
	}
	flush_seat_batch(c);
	if (c->input_next >= c->input_len) {
		trace("EV input_done sends=%llu", (unsigned long long)c->seat_sends);
	}
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

/* `vitrin_shim_session.idle_inhibit` (WS-E.4.4, issue #306).
 *
 * TWO OF THE CHECKS BELOW MATCH THE REAL CORE AND THREE ARE DELIBERATELY
 * STRICTER THAN IT. Which is which is written down rather than left to a reader,
 * because a mock that claims parity it does not have turns "this shim is
 * conformant" into "this shim satisfies this file".
 *
 * MATCHING: `crates/vitrin-core/src/shim.rs` answers a surface id this
 * connection never minted with fatal `invalid_object`, and the generated decoder
 * answers an out-of-range `state` with fatal `invalid_argument`. Both present as
 * a named FAIL line here and as log-and-close there.
 *
 * STRICTER, ON PURPOSE, AND IT FOLLOWS THAT A CONFORMANT SHIM CAN PASS `vitrind`
 * AND FAIL THIS FILE:
 *
 *  - A `held` carrying a NULL surface. The IDL's MUST forbids it, but the IDL
 *    also now specifies what a peer does with it — record it, hold nothing
 *    extra, raise NO error — and the real core does exactly that
 *    (`crates/vitrin-core/src/backend/blank.rs`, `held: BTreeMap<RealmId,
 *    Option<u32>>`). This file fails it instead, because a shim under test that
 *    forgets to name its surface has a bug the real core is specified to swallow
 *    and no other instrument would ever see.
 *  - A `held` while already holding, or a `released` with nothing held. The wire
 *    promises idempotence in both directions, so the real core folds these in
 *    silently and by design. They still mean the shim is relaying LEVELS instead
 *    of EDGES, which is a real defect, and only a peer that counts can see it. */
static void handle_idle_inhibit(struct core *c, const uint8_t *f, size_t len) {
	uint32_t oid = 0;
	vitrin_shim_session_req_idle_inhibit_t req;
	vitrin_decode_status_t st =
		vitrin_shim_session_req_idle_inhibit_decode(f, len, -1, &oid, &req);
	if (st != VITRIN_DECODE_OK) {
		fail(c, "idle_inhibit decode: %s", vitrin_decode_status_string(st));
		return;
	}
	if (req.state == VITRIN_SHIM_SESSION_IDLE_INHIBIT_STATE_HELD) {
		if (req.surface == 0u) {
			fail(c, "idle_inhibit: held with a null surface");
			return;
		}
		if (!c->surface_created || req.surface != c->surface_id) {
			fail(c, "invalid_object: idle_inhibit names surface %u, which this "
				"connection never minted", req.surface);
			return;
		}
		if (c->idle_held) {
			fail(c, "idle_inhibit: a second `held` with no `released` between -- the "
				"shim must relay EDGES of its inhibitor count, not levels");
			return;
		}
		c->idle_held = true;
	} else {
		if (req.surface != 0u) {
			fail(c, "idle_inhibit: released must carry a null surface, got %u",
				req.surface);
			return;
		}
		if (!c->idle_held) {
			fail(c, "idle_inhibit: a `released` with nothing held");
			return;
		}
		c->idle_held = false;
	}
	c->idle_edges++;
	trace("EV idle_inhibit state=%s surface=%u",
		c->idle_held ? "held" : "released", req.surface);
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

/* Dominant-colour histogram: 4 bits per channel, so 4096 buckets.
 *
 * The M1.2 verification is "Firefox smoke (local page rendering a known solid
 * color, assert dominant color)" (plan section 5), and this is what makes
 * that assertion mechanical without an image codec in a test binary that
 * links only libc. Four bits per channel rather than eight for two reasons:
 * 8-bit buckets are a 16 MiB array, and a browser does not paint a "solid"
 * colour solid -- subpixel antialiasing, colour management and compositing
 * all smear a flat #0000ff across neighbouring values. Quantising to the top
 * nibble collapses that smear into one bucket while still separating any two
 * colours a test would sensibly pick.
 *
 * The bucket is reported EXPANDED (nibble n -> byte n*0x11), so a page
 * painted in a colour whose channels are multiples of 0x11 -- which is what
 * the test pages use -- round-trips to exactly the value the CSS asked for,
 * and the assertion in the shell script is a plain string compare against
 * the colour in the HTML. */
#define HIST_BUCKETS 4096

/* The copy-in: pread rows bounded by the validated geometry, never a
 * mapping, so an fd that lied since attach is a clean short read rather
 * than SIGBUS. FNV-1a over the copied bytes gives the shell script a
 * content fingerprint without an image codec anywhere, and the histogram
 * gives it the dominant colour on the same pass over the same pixels. */
static uint64_t copy_in_digest(struct core *c, uint64_t *out_bytes,
		uint32_t *out_dominant, unsigned *out_dominant_pct) {
	uint64_t hash = 1469598103934665603ull;
	size_t row_bytes = (size_t)c->pending.width * 4;
	uint8_t *row = malloc(row_bytes);
	uint32_t *hist = calloc(HIST_BUCKETS, sizeof(*hist));
	if (row == NULL || hist == NULL) {
		fail(c, "out of memory in copy-in");
		free(row);
		free(hist);
		return 0;
	}
	/* Opened before the row loop so the dump costs one pass, not two: the
	 * rows are already being read here under validated geometry, and reading
	 * the fd a second time would be reading a buffer the shim may by then
	 * have been told it can reuse. NULL means "not dumping this frame",
	 * which is every frame but at most one. */
	FILE *dump = NULL;
	if (c->dump_path != NULL && c->commits == c->dump_at) {
		dump = fopen(c->dump_path, "wb");
		if (dump == NULL) {
			trace("EV dump_failed path=%s errno=%d", c->dump_path, errno);
		} else {
			fprintf(dump, "P6\n%u %u\n255\n", c->pending.width, c->pending.height);
		}
	}

	uint64_t total = 0;
	uint64_t pixels = 0;
	for (uint32_t y = 0; y < c->pending.height; y++) {
		size_t got = 0;
		while (got < row_bytes) {
			ssize_t n = pread(c->pending.fd, row + got, row_bytes - got,
				(off_t)((uint64_t)y * c->pending.stride + got));
			if (n <= 0) {
				fail(c, "invalid_buffer: copy-in short read at row %u", y);
				free(row);
				free(hist);
				return 0;
			}
			got += (size_t)n;
		}
		for (size_t i = 0; i < row_bytes; i++) {
			hash ^= row[i];
			hash *= 1099511628211ull;
		}
		/* Both version-1 formats are 32 bits little-endian with the same
		 * channel order in memory -- byte 0 blue, byte 1 green, byte 2 red --
		 * so XRGB8888 and ARGB8888 are read identically here and only the
		 * ignored fourth byte differs. */
		for (size_t i = 0; i + 3 < row_bytes; i += 4) {
			unsigned b = row[i] >> 4, g = row[i + 1] >> 4, r = row[i + 2] >> 4;
			hist[(r << 8) | (g << 4) | b]++;
			pixels++;
		}
		/* Same memory-order note as the histogram above: byte 0 is blue,
		 * byte 2 is red, and PPM wants R,G,B -- so this is a swap, not a
		 * copy. The fourth byte is ignored under both v1 formats. */
		if (dump != NULL) {
			for (size_t i = 0; i + 3 < row_bytes; i += 4) {
				fputc(row[i + 2], dump);
				fputc(row[i + 1], dump);
				fputc(row[i], dump);
			}
		}
		total += row_bytes;
	}
	if (dump != NULL) {
		bool ok = ferror(dump) == 0;
		if (fclose(dump) != 0 || !ok) {
			trace("EV dump_failed path=%s (write error)", c->dump_path);
		} else {
			trace("EV dump_written path=%s commit=%llu %ux%u",
				c->dump_path, (unsigned long long)c->commits,
				c->pending.width, c->pending.height);
		}
	}
	free(row);

	uint32_t best = 0, best_count = 0;
	for (uint32_t i = 0; i < HIST_BUCKETS; i++) {
		if (hist[i] > best_count) {
			best_count = hist[i];
			best = i;
		}
	}
	free(hist);
	*out_dominant = (uint32_t)(((best >> 8) & 0xf) * 0x11) << 16 |
		(uint32_t)(((best >> 4) & 0xf) * 0x11) << 8 |
		(uint32_t)((best & 0xf) * 0x11);
	*out_dominant_pct = pixels > 0 ? (unsigned)((uint64_t)best_count * 100 / pixels) : 0;
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
	uint32_t dominant = 0;
	unsigned dominant_pct = 0;
	if (c->pending.present) {
		digest = copy_in_digest(c, &bytes, &dominant, &dominant_pct);
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
		"bytes=%llu digest=%016llx dominant=%06x dominant_pct=%u inflight=%d",
		(unsigned long long)c->commits, committed_id, c->damage_len,
		(unsigned long long)area, (unsigned long long)surface_area,
		(unsigned long long)bytes, (unsigned long long)digest,
		dominant, dominant_pct, c->inflight);

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
		} else if (hdr.opcode == VITRIN_SHIM_SESSION_REQ_IDLE_INHIBIT_OPCODE) {
			handle_idle_inhibit(c, f, len);
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
		"                 [--frame-delay MS] [--defer-release]\n"
		"                 [--input SCRIPT] [--input-after-commits N]\n"
		"                 [--dump-frame N:PATH] -- SHIM_ARGV...\n");
	exit(2);
}

int main(int argc, char **argv) {
	const char *realm = "realm-0";
	uint32_t width = 800, height = 600;
	uint64_t want_frames = 0;
	int run_ms = 10000;
	int frame_delay = 0;
	bool defer_release = false;
	const char *input_script = NULL;
	uint64_t input_after_commits = 3;
	const char *dump_path = NULL;
	uint64_t dump_at = 0;
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
		} else if (strcmp(argv[i], "--input") == 0 && i + 1 < argc) {
			input_script = argv[++i];
		} else if (strcmp(argv[i], "--input-after-commits") == 0 && i + 1 < argc) {
			input_after_commits = strtoull(argv[++i], NULL, 10);
		} else if (strcmp(argv[i], "--dump-frame") == 0 && i + 1 < argc) {
			/* N:PATH -- the commit index to capture, then where to put it.
			 * Which frame matters: frame 0 of a browser is usually blank,
			 * so a caller wants to name one after the paint it cares about. */
			const char *spec = argv[++i];
			const char *colon = strchr(spec, ':');
			if (colon == NULL || colon == spec || colon[1] == '\0') {
				fprintf(stderr, "mock-core: --dump-frame wants N:PATH\n");
				usage();
			}
			dump_at = strtoull(spec, NULL, 10);
			dump_path = colon + 1;
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

	/* Parse the script BEFORE the socketpair and the fork: a malformed
	 * script must fail the run without ever having spawned a shim, or the
	 * test would report a partial injection as a shim fault. */
	if (input_script != NULL && !load_input_script(&g_core, input_script)) {
		return 2;
	}

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

	struct core *c = &g_core;
	c->fd = sv[0];
	c->watermark = 1;
	c->frame_delay_ms = frame_delay;
	c->defer_release = defer_release;
	c->retained_fd = -1;
	c->pending.fd = -1;
	c->input_after_commits = input_after_commits;
	c->dump_path = dump_path;
	c->dump_at = dump_at;
	c->input_due_ms = -1; /* armed by the Nth commit, not by the clock */

	trace("EV spawned shim_pid=%d", (int)shim);

	/* The core's first message, before anything is read. */
	send_configure(c, realm, width, height);
	trace("EV configure realm=%s width=%u height=%u", realm, width, height);

	int64_t deadline = now_ms() + run_ms;
	int exit_code = 0;
	while (!g_stop) {
		int64_t remaining = deadline - now_ms();
		if (remaining <= 0) {
			break;
		}
		/* An unfinished input script keeps the run alive: stopping on the
		 * frame count while events are still queued would silently deliver
		 * fewer than the test scripted, which is the one failure mode a
		 * stimulus generator must not have. */
		bool input_pending = c->input_next < c->input_len;
		if (want_frames > 0 && c->commits >= want_frames && !c->frame_due && !input_pending) {
			break;
		}

		int timeout = remaining > 50 ? 50 : (int)remaining;
		struct pollfd pfd = {.fd = c->fd, .events = POLLIN};
		int pr = poll(&pfd, 1, timeout);
		if (pr < 0 && errno != EINTR) {
			perror("poll");
			exit_code = 1;
			break;
		}
		if (pr > 0) {
			int r = core_fill(c);
			if (r == 0) {
				trace("EV shim_eof");
				break;
			}
			if (r < 0) {
				exit_code = 1;
				break;
			}
		}
		if (c->frame_due) {
			struct timespec now;
			clock_gettime(CLOCK_MONOTONIC, &now);
			if (now.tv_sec > c->frame_at.tv_sec ||
					(now.tv_sec == c->frame_at.tv_sec && now.tv_nsec >= c->frame_at.tv_nsec)) {
				c->frame_due = false;
				send_frame_done(c);
			}
		}
		pump_input(c);
	}

	trace("SUMMARY commits=%llu frame_dones=%llu buffer_dones=%llu max_inflight=%d "
		"last_damage_area=%llu last_damage_rects=%d seat_sends=%llu "
		"idle_edges=%llu idle_held=%d failures=%d",
		(unsigned long long)c->commits, (unsigned long long)c->frame_dones,
		(unsigned long long)c->buffer_dones, c->max_inflight,
		(unsigned long long)c->last_damage_area, c->last_damage_rects,
		(unsigned long long)c->seat_sends, c->idle_edges, c->idle_held ? 1 : 0,
		c->failures);

	/* Hang up: socketpair EOF is how the core tells a shim to go away, and
	 * the first rung of the P1.5.3 shutdown ladder. */
	close(c->fd);
	if (c->pending.present) {
		close(c->pending.fd);
	}
	if (c->retained_fd >= 0) {
		close(c->retained_fd);
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

	if (c->failures > 0) {
		exit_code = 1;
	}
	return exit_code;
}
