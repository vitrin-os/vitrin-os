# SPDX-License-Identifier: Apache-2.0
"""The GTK rung of the real bring-up ladder, under the real core (P1.6.6).

The M1.2 ladder is weston-terminal -> GTK app -> Firefox. `test_real_app.py`
(issue #105) proves the bottom rung end to end: real `vitrind` execs the real
C shim which fork/execs `weston-terminal`, and a real frame flows to the agent
through the real capture path, with no mock on any seam. This module climbs to
the middle rung -- a real GTK client -- reusing that harness's real-app mode
and the shared captured-frame content check, so the only thing new here is the
app.

Why GTK specifically, above a terminal: a terminal is a bare Wayland client,
but the apps this shim exists to confine are toolkit apps, and GTK exercises a
different slice of `xdg-shell`/`wl_seat` -- `GtkWindow` bring-up, GTK's own
buffer management, its client-side decorations. Proving a GTK window maps and
paints through the real chain is the step between "a Wayland client works" and
"Firefox works".

The app is `gtk-entry-probe` (shim/tests/gtk_entry_probe.c), the same real
`GtkEntry` client the shim's P1.6.3 R5 gate uses -- built by the shim's Meson
build wherever GTK dev headers are present (they are in `shim/ci/install-deps.sh`,
so both the shim job and this integration job build it). It renders a
deterministic window (a grey toplevel with a white text field) headlessly, so
its committed frame carries real, non-uniform content the way weston's chrome
does. This gate asserts render, not input -- typing into the entry is the
P1.6.3 shim-unit concern and, for the agent-driven path, issue #108.

# Skip-or-fail policy (matches the #105 real-app gate)

- `VITRIN_SKIP_REAL_APP=1` -> skip. The shared local-dev opt-out for the whole
  real-app ladder (this rung and the weston rung read the same knob).
- `VITRIN_C_SHIM_BIN` unset -> skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim or the GTK probe is missing -> the
  shim being absent is a **fail** (CI misconfiguration, exactly as #105); the
  GTK probe being absent is a **skip** with a loud reason, because a machine
  can have a C shim and no GTK toolkit, and the probe is an optional Meson
  target (shim/meson.build) that simply is not built there. CI builds it, so
  CI does not reach that skip.
"""

from __future__ import annotations

import os
import pathlib
import time
import unittest

from harness import (
    IntegrationTest,
    await_shims,
    children_of,
    colour_bytes,
    comm_of,
    descendant_named,
    exe_identity,
    file_identity,
    has_real_content,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

#: The GTK client the gate boots. `comm` is kernel-truncated to 15 bytes and
#: "gtk-entry-probe" is exactly 15, so the descendant match (a prefix test) is
#: exact.
APP_NAME = "gtk-entry-probe"

#: The realm view the core configures, hence the captured frame size. Same as
#: the weston rung: room for the toolkit to lay out a window with decorations.
REALM_SIZE = "640x480"

#: How long the probe stays up. It exits `--run-ms` after mapping (dumping its
#: entry), so this must outlast the capture loop below; 40 s leaves generous
#: margin under the harness's 90 s SIGALRM budget.
PROBE_RUN_MS = "40000"

#: The environment the GTK app needs, forwarded the one legal way a realm's
#: environment grows -- the realm's `env_allow`, copied from the core's own
#: environment (which the harness seeds via `extra_env`). WLR_* is the shim's
#: headless/software-render backend selection (CI has no GPU), identical to the
#: weston rung. The GTK/GDK names force the Wayland backend (DISPLAY is scrubbed,
#: so an X11 attempt would only waste time) and silence the accessibility bus,
#: which is absent under the confined, session-bus-less environment and would
#: otherwise stall GTK startup reaching for it.
APP_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
    "GDK_BACKEND": "wayland",
    "GTK_A11Y": "none",
    "NO_AT_BRIDGE": "1",
}


def _resolve_gtk_app() -> str | None:
    """The GTK probe binary, or None if this machine has no built one.

    `VITRIN_GTK_APP` names it explicitly (an operator pointing at a system GTK
    app, or a build tree the sibling heuristic misses). Otherwise it is a
    sibling of the C shim -- the shim's Meson build drops `vitrin-shim` and
    `gtk-entry-probe` in the same build directory, so
    `<build>/vitrin-shim` implies `<build>/gtk-entry-probe` when GTK was
    present at configure time.
    """
    explicit = os.environ.get("VITRIN_GTK_APP")
    if explicit:
        return explicit
    shim = os.environ.get("VITRIN_C_SHIM_BIN")
    if shim:
        sibling = pathlib.Path(shim).resolve().parent / APP_NAME
        if sibling.is_file() and os.access(sibling, os.X_OK):
            return str(sibling)
    return None


class RealGtk(IntegrationTest):
    """A real GTK window mapping and painting through the real chain."""

    def setUp(self) -> None:
        super().setUp()
        if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
            self.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")

        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
                "Build it (meson setup shim/build shim && meson compile -C shim/build) and "
                "point the variable at shim/build/vitrin-shim. CI sets it."
            )
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so a "
                "real run was requested; refusing to skip a requested gate (CI misconfig)."
            )

        app = _resolve_gtk_app()
        if app is None:
            # A C shim without a GTK probe is a real, legitimate machine state
            # (no GTK dev headers at Meson configure time), so this is a loud
            # SKIP, not a fail -- unlike a missing shim, which is a misconfig.
            self.skipTest(
                f"no {APP_NAME} found (build it: GTK dev headers present at `meson setup`, "
                "or point VITRIN_GTK_APP at a GTK client). shim/ci/install-deps.sh installs "
                "libgtk-3-dev, so CI builds it and does not reach this skip."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

    def real_core(self):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", PROBE_RUN_MS],
            env_allow=tuple(APP_ENV),
            extra_env=APP_ENV,
        )

    def _spine(self, core):
        """Wait out `vitrind -> vitrin-shim -> gtk-entry-probe` and return pids."""
        # `shims_of`, not `children_of`: at --isolation=default (the
        # default since P2.6.2, #186) the core's direct child is the
        # `vitrin-realm-init` supervisor and the shim is ITS child, so a
        # direct-children walk finds no shim at all.
        found = await_shims(core.pid, timeout=15.0)
        shim_pid = found[0] if found else None
        self.assertIsNotNone(
            shim_pid, f"the core forked no shim; children were {children_of(core.pid)}"
        )
        # The mock-freeness check, by INODE rather than by name. A confined
        # shim is bound at `/vitrin/shim`, so its `comm` is `shim` whichever
        # binary it is (P2.6.2, #186) and a name test stopped telling the real
        # shim from `vitrin-mock-shim`. The running image's inode does, and
        # more sharply: a name says what a program is called, an inode says
        # which file is executing.
        self.assertEqual(
            exe_identity(shim_pid),
            file_identity(self.shim_bin),
            f"the realm's shim (pid {shim_pid}, comm {comm_of(shim_pid)!r}) is not "
            f"the C shim this gate named ({self.shim_bin}) -- vitrin-mock-shim must "
            "appear nowhere in this path",
        )
        app_pid = descendant_named(core.pid, APP_NAME, timeout=20.0)
        self.assertIsNotNone(
            app_pid,
            f"the C shim never fork/exec'd {APP_NAME}; core={core.pid} shim={shim_pid} "
            f"shim children were {children_of(shim_pid)}",
        )
        ppid = int(pathlib.Path(f"/proc/{app_pid}/stat").read_text().split()[3])
        self.assertEqual(
            ppid,
            shim_pid,
            f"{APP_NAME} (pid {app_pid}) parent is {ppid}, not the shim {shim_pid}",
        )
        return shim_pid, app_pid

    def _capture_real_content(self, grant, timeout=25.0):
        """Observe until the GTK window's content reaches the agent.

        Like the weston rung: the app's first commit is frequently the shim's
        opaque background or GTK's pre-chrome fill, both uniform. This loops
        past them to a frame whose colour channels carry real, non-uniform
        content (`has_real_content`), which is what makes "a real GTK frame
        reached the agent" a genuine claim and not a pass on the empty
        background.
        """
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            last = frame
            if has_real_content(frame):
                return frame
            time.sleep(0.1)
        distinct = "no frame captured" if last is None else len(set(colour_bytes(last)))
        self.fail(
            f"no content-bearing {APP_NAME} frame reached the agent within {timeout}s "
            f"(last frame distinct colour values: {distinct}). The GTK window must map and "
            "paint, or the real chain is not delivering its content."
        )

    def test_a_real_gtk_window_paints_through_the_real_spine_to_the_agent(self):
        core = self.real_core()
        shim_pid, app_pid = self._spine(core)
        self.assertEqual(
            (comm_of(core.pid), exe_identity(shim_pid), comm_of(app_pid)),
            ("vitrind", file_identity(self.shim_bin), APP_NAME),
            f"the spine must be exactly vitrind -> the real C shim -> {APP_NAME}; the "
            "middle link is matched by inode because a confined shim's comm is `shim` "
            "whichever binary it is (P2.6.2, #186)",
        )

        conn = core.connect()
        grant = whole_realm_grant(conn)
        frame = self._capture_real_content(grant, timeout=25.0)

        self.assertEqual((frame.width, frame.height), (640, 480))
        self.assertEqual(len(frame.raw), frame.stride * frame.height)

        colour = colour_bytes(frame)
        self.assertTrue(
            any(colour),
            "the captured frame's colour channels are all zero: only the shim's opaque "
            f"background reached the agent, no real {APP_NAME} content",
        )
        distinct = len(set(colour))
        self.assertGreater(
            distinct,
            1,
            "the captured frame is a single uniform colour: the GTK toolkit paints a window "
            "with a text field, so a uniform buffer means the agent received the shim's empty "
            "fill, not the live composite of the real app's frame",
        )
        # A visible record of how much real content arrived: a GTK window with a
        # text field yields many distinct colour values (window chrome, the
        # entry's border and field), well above the >1 the assertion needs.
        print(f"\n[real-gtk] captured a {frame.width}x{frame.height} frame with "
              f"{distinct} distinct colour values through the real capture path")

        conn.close()
        core.terminate()

        allowed = {
            e.get("verb")
            for e in core.entries()
            if e["kind"] == "use_decision" and e.get("decision") == "allowed"
        }
        self.assertIn(
            "observe",
            allowed,
            f"the real GTK capture must be admitted at the enforcement chokepoint; saw {allowed}",
        )


if __name__ == "__main__":
    unittest.main()
