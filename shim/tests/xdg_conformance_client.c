/* xdg_conformance_client.c -- asserts, from the app's side of the socket, the
 * three xdg-shell facts about `src/xdg.c` that prose keeps getting backwards.
 *
 * SPDX-License-Identifier: MPL-2.0
 *
 * WHY A CLIENT AND NOT A UNIT TEST. All three facts are about *when* an event
 * crosses the wire relative to another event, and only the receiving end can
 * settle that -- the same argument input_echo_client.c makes for the keymap.
 *
 * FACT 1 -- THE INITIAL CONFIGURE IS NOT SENT BEFORE THE INITIAL COMMIT.
 * xdg-shell-stable, `xdg_surface`: "the client must perform an initial commit
 * without any buffer attached. The compositor will reply with initial
 * wl_surface state [...] followed by an xdg_surface.configure event. The
 * client must acknowledge it and is then allowed to attach a buffer." So a
 * compositor that configures eagerly at `get_toplevel`, before the client has
 * committed, is the non-conformant one. This client asks for a toplevel,
 * round-trips twice WITHOUT committing, and asserts NOTHING arrived; then it
 * performs the buffer-less initial commit and asserts the configure does.
 *
 * That ordering was misread as a bug once (issue #128, filed against the
 * retracted annotation in shim/wlcs/README.md, proposing the exact inverse),
 * so it is worth a test that fails loudly if anyone "fixes" it.
 *
 * FACT 2 -- `wm_capabilities` CARRIES WHAT THE SHIM ACTUALLY IMPLEMENTS.
 * xdg-shell-stable, `xdg_toplevel.wm_capabilities`: "Compositors must send
 * this event once before the first xdg_surface.configure event", and a client
 * "should hide or disable the UI elements" for anything absent. wlroots seeds
 * all four capabilities by default; `xdg.c` narrows that to the two the shim
 * has a handler for (maximize, fullscreen). This client asserts the event
 * arrives, arrives BEFORE the first configure, and carries exactly
 * {maximize, fullscreen} -- both directions, so an over-broad set fails too.
 *
 * FACT 3 -- A POPUP IS CONFIGURED TOO, SO OPENING A MENU DOES NOT KILL THE APP.
 * `xdg_surface`'s initial-commit/configure/ack contract is a property of the
 * *role object*, not of toplevels: a popup that attaches a buffer without
 * having been configured is killed with `xdg_surface` error 3
 * (`unconfigured_buffer`, "xdg_surface has never been configured"). wlroots
 * does not configure popups on the compositor's behalf -- its own reference
 * compositor schedules that configure by hand -- so a shim with no
 * `xdg_shell.new_popup` listener disconnects every app that opens a menu,
 * tooltip or combo box. This client brings a real toplevel up and MAPS it,
 * then does the full popup sequence (`get_popup`, buffer-less initial commit,
 * ack, attach, commit) and asserts the popup was configured and the connection
 * survived. It was measured failing on the tree before that listener landed.
 *
 * Prints one line per check and exits non-zero on the first failure.
 */
#define _GNU_SOURCE /* memfd_create, for the buffer that maps the toplevel */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client.h>

#include "xdg-shell-client-protocol.h"

/* The shim advertises xdg_wm_base v6; wm_capabilities needs v5. Binding at
 * exactly the advertised version (capped) is what a real toolkit does, and it
 * is also what makes "the event never arrived" mean the compositor, not a
 * version negotiated too low to carry it. */
#define WANT_WM_BASE_VERSION 6

/* The popup's own geometry. Small and anchored inside the toplevel, so the
 * positioner is unambiguously valid whatever view size the shim was given --
 * wlroots rejects a positioner with a zero size or a zero anchor rect. */
#define POPUP_W 40
#define POPUP_H 30

struct probe {
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *wm_base;
	uint32_t wm_base_version;

	int configure_count;
	int wm_capabilities_count;
	/* Snapshot of wm_capabilities_count taken when the FIRST configure
	 * arrives: the ordering assertion, not a second counter. */
	int wm_capabilities_at_first_configure;

	bool caps[5]; /* indexed by the wm_capabilities enum value, 1..4 */
	size_t caps_len;
	bool caps_out_of_range;

	/* FACT 3. Counted on the popup's OWN xdg_surface, so a toplevel
	 * configure can never be mistaken for a popup one. */
	int popup_xdg_surface_configures;
	int popup_configures; /* the xdg_popup.configure geometry event */
	bool popup_done;
};

static int failures = 0;

static void check(bool ok, const char *what) {
	printf("%s %s\n", ok ? "ok  " : "FAIL", what);
	if (!ok) {
		failures++;
	}
}

static void registry_global(void *data, struct wl_registry *registry,
		uint32_t name, const char *iface, uint32_t version) {
	struct probe *p = data;
	if (strcmp(iface, wl_compositor_interface.name) == 0) {
		uint32_t v = version > 4 ? 4 : version;
		p->compositor = wl_registry_bind(registry, name,
			&wl_compositor_interface, v);
	} else if (strcmp(iface, wl_shm_interface.name) == 0) {
		p->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
	} else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
		p->wm_base_version = version;
		uint32_t v = version > WANT_WM_BASE_VERSION
			? WANT_WM_BASE_VERSION : version;
		p->wm_base = wl_registry_bind(registry, name,
			&xdg_wm_base_interface, v);
	}
}

static void registry_global_remove(void *data, struct wl_registry *registry,
		uint32_t name) {
	(void)data; (void)registry; (void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

static void wm_base_ping(void *data, struct xdg_wm_base *wm_base,
		uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(wm_base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {
	.ping = wm_base_ping,
};

static void xdg_surface_configure(void *data, struct xdg_surface *xdg_surface,
		uint32_t serial) {
	struct probe *p = data;
	if (p->configure_count == 0) {
		p->wm_capabilities_at_first_configure = p->wm_capabilities_count;
	}
	p->configure_count++;
	xdg_surface_ack_configure(xdg_surface, serial);
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

static void toplevel_configure(void *data, struct xdg_toplevel *toplevel,
		int32_t width, int32_t height, struct wl_array *states) {
	(void)data; (void)toplevel; (void)width; (void)height; (void)states;
}

static void toplevel_close(void *data, struct xdg_toplevel *toplevel) {
	(void)data; (void)toplevel;
}

static void toplevel_configure_bounds(void *data,
		struct xdg_toplevel *toplevel, int32_t width, int32_t height) {
	(void)data; (void)toplevel; (void)width; (void)height;
}

static void toplevel_wm_capabilities(void *data, struct xdg_toplevel *toplevel,
		struct wl_array *capabilities) {
	(void)toplevel;
	struct probe *p = data;
	p->wm_capabilities_count++;
	p->caps_len = capabilities->size / sizeof(uint32_t);
	memset(p->caps, 0, sizeof(p->caps));
	p->caps_out_of_range = false;
	uint32_t *cap;
	wl_array_for_each(cap, capabilities) {
		if (*cap >= 1 && *cap <= 4) {
			p->caps[*cap] = true;
		} else {
			p->caps_out_of_range = true;
		}
	}
}

static const struct xdg_toplevel_listener toplevel_listener = {
	.configure = toplevel_configure,
	.close = toplevel_close,
	.configure_bounds = toplevel_configure_bounds,
	.wm_capabilities = toplevel_wm_capabilities,
};

/* FACT 3 needs the parent toplevel actually MAPPED, which needs a real buffer:
 * a menu is opened from a window that is on screen, and that is the case the
 * shim has to survive. Contents are irrelevant -- nothing here reads a pixel
 * back (that is what damage_client.c and solid_client.c are for), so this is
 * the smallest correct wl_shm buffer and no more. */
static struct wl_buffer *make_buffer(struct wl_shm *shm, int w, int h) {
	size_t stride = (size_t)w * 4;
	size_t size = stride * (size_t)h;
	int fd = memfd_create("vitrin-xdgconf", MFD_CLOEXEC);
	if (fd < 0) {
		return NULL;
	}
	if (ftruncate(fd, (off_t)size) < 0) {
		close(fd);
		return NULL;
	}
	void *px = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (px == MAP_FAILED) {
		close(fd);
		return NULL;
	}
	memset(px, 0xff, size); /* opaque white; never inspected */
	munmap(px, size);
	struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)size);
	struct wl_buffer *buf = wl_shm_pool_create_buffer(pool, 0, w, h,
		(int32_t)stride, WL_SHM_FORMAT_XRGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);
	return buf;
}

static void popup_xdg_surface_configure(void *data,
		struct xdg_surface *xdg_surface, uint32_t serial) {
	struct probe *p = data;
	p->popup_xdg_surface_configures++;
	xdg_surface_ack_configure(xdg_surface, serial);
}

static const struct xdg_surface_listener popup_xdg_surface_listener = {
	.configure = popup_xdg_surface_configure,
};

static void popup_configure(void *data, struct xdg_popup *popup,
		int32_t x, int32_t y, int32_t width, int32_t height) {
	(void)popup; (void)x; (void)y; (void)width; (void)height;
	struct probe *p = data;
	p->popup_configures++;
}

static void popup_popup_done(void *data, struct xdg_popup *popup) {
	(void)popup;
	struct probe *p = data;
	p->popup_done = true;
}

static void popup_repositioned(void *data, struct xdg_popup *popup,
		uint32_t token) {
	(void)data; (void)popup; (void)token;
}

static const struct xdg_popup_listener popup_listener = {
	.configure = popup_configure,
	.popup_done = popup_popup_done,
	.repositioned = popup_repositioned,
};

int main(void) {
	struct probe p = {0};

	struct wl_display *display = wl_display_connect(NULL);
	if (display == NULL) {
		fprintf(stderr, "FAIL cannot connect to WAYLAND_DISPLAY\n");
		return 2;
	}

	struct wl_registry *registry = wl_display_get_registry(display);
	wl_registry_add_listener(registry, &registry_listener, &p);
	if (wl_display_roundtrip(display) < 0) {
		fprintf(stderr, "FAIL registry roundtrip\n");
		return 2;
	}
	if (p.compositor == NULL || p.wm_base == NULL || p.shm == NULL) {
		fprintf(stderr, "FAIL missing wl_compositor, wl_shm or xdg_wm_base\n");
		return 2;
	}
	printf("info xdg_wm_base advertised version=%u\n", p.wm_base_version);
	check(p.wm_base_version >= 5,
		"xdg_wm_base is advertised at v5+ (wm_capabilities is reachable)");
	xdg_wm_base_add_listener(p.wm_base, &wm_base_listener, &p);

	struct wl_surface *surface = wl_compositor_create_surface(p.compositor);
	struct xdg_surface *xdg_surface =
		xdg_wm_base_get_xdg_surface(p.wm_base, surface);
	xdg_surface_add_listener(xdg_surface, &xdg_surface_listener, &p);
	struct xdg_toplevel *toplevel = xdg_surface_get_toplevel(xdg_surface);
	xdg_toplevel_add_listener(toplevel, &toplevel_listener, &p);
	xdg_toplevel_set_title(toplevel, "vitrin-xdg-conformance");
	xdg_toplevel_set_app_id(toplevel, "org.vitrin.xdg-conformance");

	/* FACT 1, first half: the role exists and the client has said everything
	 * it means to say about it -- and still has not committed. Two round
	 * trips, because a configure scheduled on the compositor's idle loop
	 * would land on the second one, not the first. */
	if (wl_display_roundtrip(display) < 0 || wl_display_roundtrip(display) < 0) {
		fprintf(stderr, "FAIL roundtrip after get_toplevel\n");
		return 2;
	}
	check(p.configure_count == 0,
		"no xdg_surface.configure before the initial commit");

	/* The initial commit: no buffer attached, as xdg-shell requires. */
	wl_surface_commit(surface);
	if (wl_display_roundtrip(display) < 0 || wl_display_roundtrip(display) < 0) {
		fprintf(stderr, "FAIL roundtrip after the initial commit\n");
		return 2;
	}
	check(p.configure_count >= 1,
		"xdg_surface.configure arrives on the initial commit");

	/* FACT 2. */
	check(p.wm_capabilities_count == 1,
		"xdg_toplevel.wm_capabilities sent exactly once");
	check(p.wm_capabilities_at_first_configure == 1,
		"wm_capabilities precedes the first xdg_surface.configure");
	check(!p.caps_out_of_range,
		"every advertised capability is a defined enum value");
	check(p.caps[XDG_TOPLEVEL_WM_CAPABILITIES_MAXIMIZE],
		"wm_capabilities offers maximize (request_maximize is handled)");
	check(p.caps[XDG_TOPLEVEL_WM_CAPABILITIES_FULLSCREEN],
		"wm_capabilities offers fullscreen (request_fullscreen is handled)");
	check(!p.caps[XDG_TOPLEVEL_WM_CAPABILITIES_MINIMIZE],
		"wm_capabilities withholds minimize (nowhere to minimize to)");
	check(!p.caps[XDG_TOPLEVEL_WM_CAPABILITIES_WINDOW_MENU],
		"wm_capabilities withholds window_menu (no decorations, no WM)");
	check(p.caps_len == 2, "wm_capabilities carries exactly two entries");

	/* FACT 3. Map the parent first -- the configure above is already acked by
	 * the xdg_surface listener, so attaching is legal now and only now. */
	struct wl_buffer *parent_buffer = make_buffer(p.shm, 200, 150);
	if (parent_buffer == NULL) {
		fprintf(stderr, "FAIL cannot allocate a wl_shm buffer\n");
		return 2;
	}
	wl_surface_attach(surface, parent_buffer, 0, 0);
	wl_surface_damage_buffer(surface, 0, 0, 200, 150);
	wl_surface_commit(surface);
	if (wl_display_roundtrip(display) < 0) {
		fprintf(stderr, "FAIL roundtrip after mapping the parent; "
			"wl_display error %d\n", wl_display_get_error(display));
		return 2;
	}
	check(wl_display_get_error(display) == 0,
		"the parent toplevel maps without a protocol error");

	struct wl_surface *popup_surface = wl_compositor_create_surface(p.compositor);
	struct xdg_surface *popup_xdg_surface =
		xdg_wm_base_get_xdg_surface(p.wm_base, popup_surface);
	xdg_surface_add_listener(popup_xdg_surface, &popup_xdg_surface_listener, &p);
	struct xdg_positioner *positioner = xdg_wm_base_create_positioner(p.wm_base);
	xdg_positioner_set_size(positioner, POPUP_W, POPUP_H);
	xdg_positioner_set_anchor_rect(positioner, 0, 0, 10, 10);
	xdg_positioner_set_anchor(positioner, XDG_POSITIONER_ANCHOR_BOTTOM_LEFT);
	xdg_positioner_set_gravity(positioner, XDG_POSITIONER_GRAVITY_BOTTOM_RIGHT);
	struct xdg_popup *popup =
		xdg_surface_get_popup(popup_xdg_surface, xdg_surface, positioner);
	xdg_popup_add_listener(popup, &popup_listener, &p);
	xdg_positioner_destroy(positioner);

	/* The popup's own buffer-less initial commit. Two round trips, for the
	 * same reason FACT 1 takes two: a configure scheduled on the compositor's
	 * idle loop lands on the second. */
	wl_surface_commit(popup_surface);
	if (wl_display_roundtrip(display) < 0 || wl_display_roundtrip(display) < 0) {
		fprintf(stderr, "FAIL roundtrip after the popup's initial commit; "
			"wl_display error %d\n", wl_display_get_error(display));
		return 2;
	}
	check(p.popup_xdg_surface_configures >= 1,
		"xdg_surface.configure arrives for the POPUP on its initial commit");
	check(p.popup_configures >= 1,
		"xdg_popup.configure carries the popup's geometry");
	check(!p.popup_done, "the popup is not dismissed before it is mapped");

	/* The whole point: a client that follows the sequence gets to map. Without
	 * a popup configure this attach is `unconfigured_buffer` and the app is
	 * disconnected -- which is a dead app, not a missing menu. */
	struct wl_buffer *popup_buffer = make_buffer(p.shm, POPUP_W, POPUP_H);
	if (popup_buffer == NULL) {
		fprintf(stderr, "FAIL cannot allocate the popup's wl_shm buffer\n");
		return 2;
	}
	wl_surface_attach(popup_surface, popup_buffer, 0, 0);
	wl_surface_damage_buffer(popup_surface, 0, 0, POPUP_W, POPUP_H);
	wl_surface_commit(popup_surface);
	wl_display_roundtrip(display);
	int err = wl_display_get_error(display);
	if (err != 0) {
		/* wl_display_get_protocol_error RETURNS the error code and writes the
		 * offending interface and object id through its out-parameters -- in
		 * that order, which is easy to transpose and prints a plausible lie
		 * when you do. */
		uint32_t id = 0;
		const struct wl_interface *iface = NULL;
		uint32_t code = wl_display_get_protocol_error(display, &iface, &id);
		fprintf(stderr, "info disconnected: errno=%d protocol iface=%s id=%u code=%u\n",
			err, iface != NULL ? iface->name : "(none)", id, code);
	}
	check(err == 0, "the popup maps without the client being disconnected");

	if (err == 0) {
		xdg_popup_destroy(popup);
		xdg_surface_destroy(popup_xdg_surface);
		wl_surface_destroy(popup_surface);
		wl_buffer_destroy(popup_buffer);
		xdg_toplevel_destroy(toplevel);
		xdg_surface_destroy(xdg_surface);
		wl_surface_destroy(surface);
		wl_buffer_destroy(parent_buffer);
		wl_display_roundtrip(display);
	}
	wl_display_disconnect(display);

	printf("%s (%d failed)\n", failures == 0 ? "PASS" : "FAIL", failures);
	return failures == 0 ? 0 : 1;
}
