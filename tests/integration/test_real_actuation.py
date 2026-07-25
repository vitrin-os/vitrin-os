# SPDX-License-Identifier: Apache-2.0
"""P1.8.6 (#108): agent pointer + text actuation LANDS on a real app surface
through the real core, with an effect the agent confirms by its own observe().

`test_real_capture_fidelity.py` (P1.8.5) proved the agent *observes* a real app
through the real chokepoint. This is the actuation half: every seat event
originates from `grant.pointer`/`grant.text` through the real enforcement
chokepoint (`session.rs` `route_seat`), the input router, and the shim's real
`wl_seat` — no `mock_core --input`, no mock shim — and the real app's response
is asserted through the agent's *own* capture, not an out-of-band core report.
It closes the gap `test_actuation.py` leaves open: that suite asserts only the
recorder's `use_decision` parameters against the mock; the D10 click-on-surface
case was deferred to the (broken) #43 demo.

Two rungs, both headless / GPU-free in CI:

- **D10 — a click lands on the observed feature.** The app is `click-target`, a
  wl_shm client with a green TARGET square on a black field that repaints its
  whole surface RED when a click lands inside the target. The agent locates the
  target *in its own captured frame*, clicks its centroid (a realm-view pixel
  coordinate the shim maps to surface-local, D10), and observes the dominant
  colour flip to red — a flip that can only happen if the click landed on the
  feature the agent saw. The surface-local coordinate the app reports is
  cross-checked against the view coordinate the agent clicked (the steady-state
  identity mapping), and the chokepoint's record of the move is asserted too.

- **D7 — a Unicode string arrives intact in a real text field.** The app is
  `gtk-entry-probe` (the P1.6.3 R5 gate's GTK entry). The agent types
  `héllo→世界` via `actuate.text`; the observe() frame changes where the glyphs
  render (the observable effect), the probe reports the exact bytes it received
  (intact), and the chokepoint records the typed string's shape.

The nested Firefox actuation variant (pointer scroll repaints the page; a typed
URL + Enter navigates it) is documented in `shim/docs/firefox.md` §8 —
workstation-only, CI-never-runs by construction.

# Skip-or-fail policy (matches the real-app ladder)

- `VITRIN_SKIP_REAL_APP=1` → skip. The shared real-app-ladder local opt-out.
- `VITRIN_C_SHIM_BIN` unset → skip. A developer without a built C shim.
- `VITRIN_C_SHIM_BIN` **set** but the shim or `click-target` is missing → fail
  (both co-built by the shim's Meson build). A missing `gtk-entry-probe` is a
  loud **skip**, not a fail: its GTK dependency makes its absence a legitimate
  machine state (as `test_real_gtk.py` treats it), and CI installs GTK so CI
  holds it.
"""

from __future__ import annotations

import os
import pathlib
import signal
import tempfile
import time
import unittest

from harness import (
    IntegrationTest,
    children_of,
    comm_of,
    count_changed_pixels,
    descendant_named,
    dominant_colour,
    has_real_content,
    locate_colour,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

#: The realm view size, hence captured-frame size. Same as the rest of the
#: real-app ladder.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

#: The headless/software-render selectors the shim's wlroots backend needs.
WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


def _resolve_sibling(shim_bin: pathlib.Path, name: str, env_override: str) -> str | None:
    """A tool built beside the C shim, or an explicit override, or None."""
    explicit = os.environ.get(env_override)
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / name
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


def _require_shim(test: IntegrationTest) -> pathlib.Path:
    """The shared shim resolution + skip-or-fail preamble for both rungs."""
    if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
        test.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")
    shim = os.environ.get("VITRIN_C_SHIM_BIN")
    if not shim:
        test.skipTest(
            "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real chain against. "
            "Build it (meson setup shim/build shim && meson compile -C shim/build) and point "
            "the variable at shim/build/vitrin-shim. CI sets it."
        )
    shim_bin = pathlib.Path(shim)
    if not (shim_bin.is_file() and os.access(shim_bin, os.X_OK)):
        test.fail(
            f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, so a real "
            "run was requested; refusing to skip a requested gate (CI misconfig)."
        )
    return shim_bin


def _allowed_inputs(entries, action: str):
    """The `input` payloads of allowed `use_decision`s with the given action.

    Keyed on `input.action` (`"move"`, `"type"`), not the grant `verb`, matching
    `test_actuation.py`'s proven read: a capture's `use_decision` carries
    `input: null` (JSON null → None), so coerce before indexing.
    """
    return [
        (e.get("input") or {})
        for e in entries
        if e["kind"] == "use_decision"
        and e.get("decision") == "allowed"
        and (e.get("input") or {}).get("action") == action
    ]


class RealActuationPointer(IntegrationTest):
    """D10: an agent's click lands on the target it observed, and flips it."""

    #: The colours `click-target` paints (channels ×0x11 so the histogram is
    #: exact). Green target on a black field; a landed click repaints red.
    TARGET = "00ff00"
    HIT = "ff0000"

    #: How much green must be on screen before the agent trusts it located the
    #: target — the 160×160 square is ~25 600 px, so this rejects a stray pixel
    #: while clearing with wide margin.
    MIN_TARGET_PIXELS = 5000

    def setUp(self) -> None:
        super().setUp()
        self.shim_bin = _require_shim(self)
        app = _resolve_sibling(self.shim_bin, "click-target", "VITRIN_CLICK_TARGET_APP")
        if app is None:
            # click-target is co-built with the shim unconditionally (a bare
            # wl_shm client, no optional dep), so its absence beside a built
            # shim is a build misconfiguration to FAIL on, not a machine state.
            self.fail(
                f"no click-target beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "rebuild the shim, or set VITRIN_CLICK_TARGET_APP."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-click-"))
        self.addCleanup(_rmtree, self.work)

    def real_core(self):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", "40000"],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            log_file=str(self.work / "core.log"),
        )

    def _spine(self, core):
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        self.assertTrue(
            comm_of(shim_pid).startswith("vitrin-shim"),
            f"the core's child must be the real C shim, not {comm_of(shim_pid)!r}",
        )
        app_pid = descendant_named(core.pid, "click-target", timeout=15.0)
        self.assertIsNotNone(app_pid, "the C shim never fork/exec'd click-target")
        return shim_pid, app_pid

    def _locate_target(self, grant, timeout=20.0):
        """Observe until the green target is on screen; return `(frame, cx, cy)`."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            cx, cy, count = locate_colour(frame, self.TARGET)
            if count >= self.MIN_TARGET_PIXELS:
                return frame, cx, cy
            time.sleep(0.1)
        self.fail(
            f"the green target never reached the agent within {timeout:.0f}s: click-target's "
            "frame did not arrive, so there is nothing to click"
        )

    def _observe_until_flipped(self, grant, timeout=15.0):
        """Observe until the dominant colour is the HIT colour, or fail."""
        deadline = time.monotonic() + timeout
        seen: list[str] = []
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            colour, pct = dominant_colour(frame)
            if not seen or seen[-1] != colour:
                seen.append(colour)
            if colour == self.HIT and pct > 90:
                return pct
            time.sleep(0.1)
        self.fail(
            f"the surface never flipped to #{self.HIT} within {timeout:.0f}s after the click: "
            f"the click did not land on the target. Dominant-colour sequence: "
            f"{' -> '.join('#' + c for c in seen)}"
        )

    def test_agent_click_lands_on_the_observed_target_and_flips_it(self):
        core = self.real_core()
        _shim_pid, _app_pid = self._spine(core)

        conn = core.connect()
        grant = whole_realm_grant(conn)

        # The agent locates the target IN ITS OWN CAPTURED FRAME (D10: it
        # addresses realm-view pixels), then confirms the pre-click scene is the
        # black field, not already the hit colour.
        frame, cx, cy = self._locate_target(grant)
        self.assertEqual((frame.width, frame.height), REALM_WH)
        pre, _pre_pct = dominant_colour(frame)
        self.assertNotEqual(
            pre, self.HIT, "the surface was already the hit colour before any click"
        )

        # Click the observed centroid. `click()` moves (so the pointer enters
        # the surface) then presses+releases, all through the real chokepoint.
        grant.pointer.click(cx, cy)

        # The whole-surface flip to red can only happen if the click landed
        # inside the target the agent saw — asserted by the agent's own capture.
        hit_pct = self._observe_until_flipped(grant)

        conn.close()
        core.terminate()

        out = core.output()

        # The app's own report of the surface-local coordinate it received. In
        # the single-maximized steady state the shim's view→surface mapping is
        # the identity, so it must match the view coordinate the agent clicked
        # within rounding (wl_fixed + integer truncation) — this is the D10
        # coordinate-mapping claim, checked number for number.
        hit_line = next((ln for ln in out.splitlines() if ln.startswith("HIT ")), None)
        self.assertIsNotNone(
            hit_line, f"click-target never reported a HIT; the click did not reach it.\n{out}"
        )
        sx = int(hit_line.split("sx=")[1].split()[0])
        sy = int(hit_line.split("sy=")[1].split()[0])
        self.assertLessEqual(
            abs(sx - cx),
            2,
            f"the surface-local x the app received ({sx}) does not match the view x the agent "
            f"clicked ({cx}); the view→surface mapping is not the identity it must be here",
        )
        self.assertLessEqual(abs(sy - cy), 2, f"surface-local y {sy} != clicked view y {cy}")

        # Both halves (criterion 5): the chokepoint RECORDED the allowed move at
        # the clicked coordinate, and the app demonstrably received it (above).
        moves = _allowed_inputs(core.entries(), "move")
        self.assertTrue(
            any(m["x"] == cx and m["y"] == cy for m in moves),
            f"the chokepoint must record the move to the clicked ({cx}, {cy}); saw {moves}",
        )

        print(
            f"\n[real-actuation] agent located the target at view ({cx}, {cy}); clicked; the app "
            f"received surface ({sx}, {sy}); the surface flipped to #{self.HIT} at {hit_pct}%"
        )


class RealActuationText(IntegrationTest):
    """D7: a Unicode string typed by the agent arrives intact in a GTK entry."""

    #: The D7 string from the issue: a Latin dead-key composition, an arrow, and
    #: CJK — the mix that separates keymap correctness from IME correctness.
    SECRET = "héllo→世界"

    #: The GTK app the gate boots. Same env as test_real_gtk's rung.
    APP_ENV = {
        **WLR_ENV,
        "GDK_BACKEND": "wayland",
        "GTK_A11Y": "none",
        "NO_AT_BRIDGE": "1",
    }

    #: Rendered glyphs move far more pixels than a focus caret's ~30; this
    #: threshold clears the text and rejects caret-blink / incidental noise.
    MIN_CHANGED_PIXELS = 200

    def setUp(self) -> None:
        super().setUp()
        self.shim_bin = _require_shim(self)
        app = _resolve_sibling(self.shim_bin, "gtk-entry-probe", "VITRIN_GTK_APP")
        if app is None:
            # A C shim without a GTK probe is a legitimate machine state (no GTK
            # dev headers at Meson configure time), so this is a loud SKIP, not
            # a fail — exactly as test_real_gtk.py treats it. CI installs GTK.
            self.skipTest(
                "no gtk-entry-probe beside the C shim (GTK dev headers absent at `meson setup`, "
                "or point VITRIN_GTK_APP at it). shim/ci/install-deps.sh installs libgtk-3-dev, "
                "so CI builds it and does not reach this skip."
            )
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-type-"))
        self.addCleanup(_rmtree, self.work)

    #: The probe's `--run-ms` timer is a FALLBACK only, set far past the whole
    #: test so it never fires during it: the dump is triggered ON DEMAND with
    #: SIGUSR1 the instant the test has confirmed the typed text landed, which
    #: removes the timer-vs-cold-start-render race entirely (a fixed deadline
    #: from probe launch would have to outlast GTK's software first paint — up
    #: to many seconds under CI load, per test_real_gtk.py's 25 s allowance —
    #: plus connect + type, or it dumps an empty entry).
    PROBE_RUN_MS = 60000

    def real_core(self):
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", str(self.PROBE_RUN_MS)],
            env_allow=tuple(self.APP_ENV),
            extra_env=self.APP_ENV,
            log_file=str(self.work / "core.log"),
        )

    def _await_entry_hex(self, timeout=15.0) -> str:
        """Wait for the probe's ENTRY_HEX line in the live core log, and return it.

        The probe writes it to stdout (inherited down to the core's redirected
        log file) when it is asked to dump (SIGUSR1, sent by the test once the
        text has landed), so it appears while the core is still running — read
        the file directly rather than via `core.output()`, which only reads
        after the core exits.
        """
        log = self.work / "core.log"
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            text = log.read_text(errors="replace") if log.exists() else ""
            for line in text.splitlines():
                if "ENTRY_HEX " in line:
                    return line.split("ENTRY_HEX ", 1)[1].strip()
            time.sleep(0.1)
        self.fail(
            f"gtk-entry-probe never reported ENTRY_HEX within {timeout:.0f}s: it did not answer "
            f"the SIGUSR1 dump, or the text never landed. Log tail:\n"
            + "\n".join((log.read_text(errors="replace").splitlines() if log.exists() else [])[-15:])
        )

    def _spine(self, core):
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(shim_pid, "the core forked no shim")
        app_pid = descendant_named(core.pid, "gtk-entry-probe", timeout=20.0)
        self.assertIsNotNone(app_pid, "the C shim never fork/exec'd gtk-entry-probe")
        return shim_pid, app_pid

    def _capture_content(self, grant, timeout=25.0):
        """Observe until the GTK window has painted real (non-uniform) content."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            if has_real_content(frame):
                return frame
            time.sleep(0.1)
        self.fail(f"the GTK window never painted content within {timeout:.0f}s")

    def _capture_after_typing(self, grant, before, timeout=15.0):
        """Observe until the frame differs from `before` by more than the caret."""
        deadline = time.monotonic() + timeout
        best = 0
        while time.monotonic() < deadline:
            try:
                frame = grant.observe()
            except errors.NoSurface:
                time.sleep(0.05)
                continue
            except errors.RateLimited as rl:
                time.sleep(max(rl.retry_after_ms / 1000.0, 0.05))
                continue
            changed = count_changed_pixels(before, frame)
            best = max(best, changed)
            if changed >= self.MIN_CHANGED_PIXELS:
                return frame, changed
            time.sleep(0.1)
        self.fail(
            f"no observed frame changed by >= {self.MIN_CHANGED_PIXELS} px within {timeout:.0f}s "
            f"of typing (best {best}): the typed text produced no visible change in the entry"
        )

    def test_typed_unicode_lands_in_the_gtk_entry_intact(self):
        core = self.real_core()
        _shim_pid, app_pid = self._spine(core)

        conn = core.connect()
        grant = whole_realm_grant(conn)

        # The empty-entry baseline, then type the Unicode string through the
        # real chokepoint, then observe the change the glyphs make.
        before = self._capture_content(grant)
        grant.text.type(self.SECRET)
        _after, changed = self._capture_after_typing(grant, before)

        # Intact: the probe reports the exact UTF-8 bytes its entry holds. Ask
        # for the dump NOW, only after the observed pixel change has confirmed
        # the text landed — so the report reflects the typed string, with no
        # race against a fixed timer. This is the "the real app demonstrably
        # received them" half (criterion 5) and the D7 "arrives intact" proof:
        # a rendered field cannot be read back pixel-exact, so the app's own
        # byte report is what proves intactness.
        os.kill(app_pid, signal.SIGUSR1)
        expected_hex = self.SECRET.encode("utf-8").hex()
        entry_hex = self._await_entry_hex()
        self.assertEqual(
            entry_hex,
            expected_hex,
            f"the GTK entry received {entry_hex!r}, not the typed {expected_hex!r} "
            f"({self.SECRET!r}): the Unicode string did not arrive intact",
        )

        entries = core.entries()
        conn.close()
        core.terminate()

        # Both halves (criterion 5): the chokepoint RECORDED the allowed type
        # with the string's shape (the recorder never stores the bytes — the
        # keylogger-avoidance rule — so it round-trips as its char count).
        types = _allowed_inputs(entries, "type")
        self.assertTrue(
            any(t["chars"] == len(self.SECRET) for t in types),
            f"the chokepoint must record the typed string's shape ({len(self.SECRET)} chars); "
            f"saw {types}",
        )

        print(
            f"\n[real-actuation] agent typed {self.SECRET!r} through the real chokepoint; the GTK "
            f"entry received it intact ({entry_hex}); the observe() frame changed by {changed} px"
        )


def _rmtree(path: pathlib.Path) -> None:
    import shutil

    shutil.rmtree(path, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
