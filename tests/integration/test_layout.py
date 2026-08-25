# SPDX-License-Identifier: Apache-2.0
"""WS-E.1.4 (#210): **a client holding `layout.focus` moves the output, and a
capture still returns the granted realm's pixels** — against two real apps.

This is the mock-free half of issue #210. The in-crate tests (`session.rs`'s
`no_arrangement_at_the_maximum_verb_set_can_touch_the_consent_card`,
`the_core_not_the_client_decides_which_surface_input_reaches`,
`no_arrangement_puts_an_agent_cursor_into_any_realms_capture`,
`the_humans_input_follows_the_realm_a_focus_holder_bound`) pin D-018(2)'s
ordering invariants against a real socket in-process; this drives the shipped
`vitrind`, two real `vitrin-shim` processes and two real Wayland clients, and
asks the agents what they actually received after a real `focus`.

## The property, and the one it must not quietly weaken

WS-E.1.3 made a capture return **the granted realm's** pixels rather than the
output's, and `test_two_realms.py` proves that with a binding nobody chose.
Serving `layout_focus` gives somebody the power to *move* that binding, which
is exactly the circumstance under which "a capture reads the output" would
come back as a regression and nobody would notice: with one realm bound
forever, a leak and a correct implementation agree on the bound realm.

So this test moves the output and then re-checks both agents:

1. the previously bound realm's capture still returns **its own** pixels,
   after it stopped being the realm on screen;
2. the newly bound realm's capture matches its own `--capture-dump`, which is
   the core-internal readback of what the output now composites;
3. the two frames still differ, so neither agent is being served one shared
   frame.

Assertion (1) is the one a regression fails. It is stated in terms of the
*previously* bound realm deliberately: a core that served "whatever is on the
output" would satisfy (2) and (3) and fail only here.

## What this test deliberately does not assert

That a **human sees** the focused realm change in the nested host window.
Runners have no display and headless is the only backend CI runs (D-019(4)),
so that half is a manual runbook step under `shim/docs/`, not a CI criterion —
issue #210 says so in as many words.

Same C-shim env contract as the rest of the real-app ladder
(`VITRIN_C_SHIM_BIN`, `VITRIN_SKIP_REAL_APP`).
"""

from __future__ import annotations

import os
import pathlib
import shutil
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))), "sdk", "python", "src"))

from harness import (  # noqa: E402
    IntegrationTest,
    capture_dump_path,
    children_of,
    comm_of,
    dominant_colour,
    golden_cmp,
    packed_xrgb,
    shims_of,
)

import vitrin_os  # noqa: E402
from vitrin_os import errors  # noqa: E402

#: Matches `test_two_realms.py`, so the dump comparison below runs at the
#: geometry the M1.3 comparator is already exercised at.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

#: Channels in multiples of 0x11 so the 4-bit dominant-colour histogram reads
#: them back exactly, and maximally far apart so a frame carrying the wrong
#: realm's pixels cannot be mistaken for noise.
FIRST_COLOUR = "0000ff"
SECOND_COLOUR = "ff0000"

#: `realm-0` sorts first, so it is the realm the output binds to at startup
#: and `second` is the one a `focus` has to move it to.
SECOND = "second"

RUN_MS = "40000"

#: The M1.3 fidelity gate's own comparator policy, so this gate scores frames
#: exactly as that one does rather than inventing a second threshold.
TOLERANCE = "tol:1,0.001"

WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}

#: The widest verb set this core serves. A layout property proved at the
#: maximum verb set is a property of *no grant*, which is what D-018(2)'s
#: "purchasable by no grant, at any verb set, ever" has to mean.
MAX_VERBS = (
    "observe",
    "actuate.pointer",
    "actuate.text",
    "layout.arrange",
    "layout.focus",
)


class RealLayoutFocus(IntegrationTest):
    """A `layout.focus` holder moves the output; captures stay per-realm."""

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

        app = os.environ.get("VITRIN_SOLID_APP")
        if not app:
            sibling = self.shim_bin.resolve().parent / "solid-client"
            if sibling.is_file() and os.access(sibling, os.X_OK):
                app = str(sibling)
        if not app:
            self.fail(
                f"no solid-client beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim (shim/meson.build); "
                "its absence is a build misconfiguration."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-layout-"))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)

    def _two_live_apps(self, core) -> None:
        """Both realms really are running an app before anything is captured."""
        deadline = time.monotonic() + 25.0
        shims: list[int] = []
        while time.monotonic() < deadline:
            shims = shims_of(core.pid)
            if len(shims) >= 2:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(shims), 2, f"two realms must be two C shims; core's children: {shims}"
        )
        seen: list[str] = []
        while time.monotonic() < deadline:
            seen = [
                comm_of(kid)
                for shim in shims
                for kid in children_of(shim)
                if comm_of(kid).startswith("solid-client")
            ]
            if len(seen) >= 2:
                break
            time.sleep(0.05)
        self.assertEqual(
            len(seen), 2, f"each shim must have fork/exec'd its own app; got {seen}"
        )

    def _observe_until_painted(self, grant, timeout=25.0):
        """`observe()` until the app has actually painted; return the frame."""
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
            colour, share = dominant_colour(frame)
            last = (colour, share)
            if share >= 90 and colour in (FIRST_COLOUR, SECOND_COLOUR):
                return frame
            time.sleep(0.1)
        self.fail(f"no realm ever painted a solid frame; last dominant colour was {last}")

    def _await_dump(self, path, timeout=20.0) -> bytes:
        expected = REALM_WH[0] * REALM_WH[1] * 4
        deadline = time.monotonic() + timeout
        p = pathlib.Path(path)
        while time.monotonic() < deadline:
            if p.is_file() and p.stat().st_size == expected:
                return p.read_bytes()
            time.sleep(0.05)
        size = p.stat().st_size if p.is_file() else "absent"
        self.fail(
            f"the core-internal capture at {path} never reached {expected} bytes (size: "
            f"{size}); `--capture-dump` did not write this realm's composited readback"
        )
        raise AssertionError("unreachable")  # pragma: no cover

    def test_focus_moves_the_output_and_captures_stay_per_realm(self):
        dump_base = str(self.work / "internal.rgba")
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS, "--colour", FIRST_COLOUR],
            realms=(SECOND,),
            realm_args={
                "realm-0": ["--run-ms", RUN_MS, "--colour", FIRST_COLOUR],
                SECOND: ["--run-ms", RUN_MS, "--colour", SECOND_COLOUR],
            },
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            capture_dump=dump_base,
            log_file=str(self.work / "core.log"),
        )
        self._two_live_apps(core)

        conn = core.connect()
        # The maximum served verb set, over each realm. Both grants are held
        # by one principal, which is legal: the single-holder rule is about
        # `layout.arrange` and there is one holder here.
        first = conn.request_grant(
            realm="realm-0", verbs=MAX_VERBS, persistence=vitrin_os.Persistence.WHILE_RUNNING
        ).await_consent()
        second = conn.request_grant(
            realm=SECOND,
            verbs=("observe", "layout.focus"),
            persistence=vitrin_os.Persistence.WHILE_RUNNING,
        ).await_consent()

        self.assertIn(
            "layout.focus",
            _verb_names(first.effective_verbs),
            "the core must actually grant layout.focus; if this is a refusal the rest of "
            "this test proves nothing about arrangement",
        )

        # Before the move: each agent sees its own realm.
        first_before = self._observe_until_painted(first)
        second_before = self._observe_until_painted(second)
        self.assertEqual(dominant_colour(first_before)[0], FIRST_COLOUR)
        self.assertEqual(dominant_colour(second_before)[0], SECOND_COLOUR)

        # The move, over the wire. Fire-and-forget, so the sync barrier is
        # what bounds discovery of a refusal — and a refusal here would be
        # raised at the next reply-bearing call rather than silently dropped.
        second.focus()
        conn.sync()

        # (2) The newly bound realm is what the output composites. The dump
        # is written per realm off the redraw path, so the bound realm's dump
        # is the ground truth for "what the output now shows".
        second_after = self._observe_until_painted(second)
        second_dump = self._await_dump(capture_dump_path(dump_base, SECOND))
        second_agent = self.work / "agent-second.xrgb"
        second_agent.write_bytes(packed_xrgb(second_after))
        second_own = self.work / "dump-second.rgba"
        second_own.write_bytes(second_dump)
        match = golden_cmp(
            str(second_agent),
            str(second_own),
            REALM_WH,
            TOLERANCE,
            artifacts=str(self.work / "cmp-second"),
        )
        self.assertEqual(
            match.returncode,
            0,
            "the newly focused realm's observe() must agree with its own "
            f"--capture-dump:\n{match.stdout.strip()}\n{match.stderr.strip()}",
        )

        # (1) THE ASSERTION A REGRESSION FAILS. The realm that *lost* the
        # output still serves its own pixels, not the output's.
        first_after = self._observe_until_painted(first)
        self.assertEqual(
            dominant_colour(first_after)[0],
            FIRST_COLOUR,
            "the realm that lost the output must still capture its own pixels; serving "
            "the output's frame here is the cross-realm leak WS-E.1.3 closed, "
            "re-opened by the first thing that can move the binding",
        )
        first_dump = self._await_dump(capture_dump_path(dump_base, "realm-0"))
        first_agent = self.work / "agent-first.xrgb"
        first_agent.write_bytes(packed_xrgb(first_after))
        first_own = self.work / "dump-realm-0.rgba"
        first_own.write_bytes(first_dump)
        match = golden_cmp(
            str(first_agent),
            str(first_own),
            REALM_WH,
            TOLERANCE,
            artifacts=str(self.work / "cmp-first"),
        )
        self.assertEqual(
            match.returncode,
            0,
            "the unfocused realm's observe() must agree with its OWN --capture-dump:\n"
            f"{match.stdout.strip()}\n{match.stderr.strip()}",
        )
        # ...and the cross check, which is the one a leak fails: the realm
        # that lost the output must NOT match the realm that gained it.
        cross = golden_cmp(str(first_agent), str(second_own), REALM_WH, TOLERANCE)
        self.assertNotEqual(
            cross.returncode,
            0,
            "the unfocused realm's frame also matched the FOCUSED realm's dump: the "
            "capture followed the output binding a client moved, which is the "
            "cross-realm leak WS-E.1.3 closed",
        )

        # (3) ...and the two agents are still not being served one frame.
        self.assertNotEqual(
            packed_xrgb(first_after),
            packed_xrgb(second_after),
            "two realms running two differently-coloured apps must not deliver one frame "
            "to both agents after a focus change",
        )

    def test_set_fullscreen_is_accepted_and_changes_no_pixels_at_equal_sizes(self):
        """`set_fullscreen` is served, and honest about doing nothing here.

        The two modes differ only in whether the realm's view size *tracks*
        the output's, so while the output's **usable view** and the realm are
        the same size — which is exactly a realm spawned into an output that
        has not resized, i.e. every headless CI run — switching between them
        changes nothing observable. The IDL says so in as many words.

        *Usable view, not output.* Issue #304 inset the realm view: a shim is
        configured at `ViewGeometry::usable()`, the output minus the rows the
        core reserves for the trusted band and, when `--status` is on, the
        status strip. So "the output and the realm are the same size" — what
        this docstring said until the inset shipped, and what the IDL clause it
        quotes said — is a condition that no longer occurs at all. The
        equality that makes the two modes indistinguishable is against the
        usable view, and it still holds on every headless run, which is why
        this test asserts exactly what it did before.

        What this asserts is therefore what CI *can* assert: the request is
        served rather than refused, and it does not corrupt the realm's view.
        The observable half needs an output resize, which the headless
        backend's virtual output does not do; that is a nested, manual step.
        """
        core = self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=["--run-ms", RUN_MS, "--colour", FIRST_COLOUR],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            log_file=str(self.work / "core-fs.log"),
        )
        conn = core.connect()
        grant = conn.request_grant(
            realm="realm-0", verbs=MAX_VERBS, persistence=vitrin_os.Persistence.WHILE_RUNNING
        ).await_consent()
        before = self._observe_until_painted(grant)

        grant.set_fullscreen(False)
        conn.sync()
        grant.set_fullscreen(True)
        conn.sync()

        after = self._observe_until_painted(grant)
        self.assertEqual(
            dominant_colour(after)[0],
            FIRST_COLOUR,
            "a served set_fullscreen must leave the realm painting its own colour",
        )
        self.assertEqual(
            len(packed_xrgb(after)),
            len(packed_xrgb(before)),
            "the realm view size must not change while the output has not resized",
        )


def _verb_names(bits) -> set[str]:
    """The dotted names in an effective verb bitfield."""
    from vitrin_os import protocol

    return {name for name, bit in protocol.VERB_BY_DOTTED_NAME.items() if int(bits) & int(bit)}
