# SPDX-License-Identifier: Apache-2.0
"""P1.8.4/P1.8.7 (#43/#110) acceptance: the demo agent's HEADLESS flow, against
the real chain end to end.

**This is the named M1.5 acceptance gate** (issue #110, P1.8.7), as
``tests/integration/README.md``'s table records. It has no mock on any seam:
the shipped ``vitrind`` execs the real C shim (``vitrin-shim``), which
fork/execs a real, trivial Wayland client (``weston-terminal``) inside its own
confined Wayland socket — the same rung ``tests/integration/test_real_app.py``
uses. The demo's ``run`` entry point is imported and called (not run as a
subprocess), so a failure surfaces as a Python traceback with the demo's own
frame dump rather than an opaque non-zero exit. It rides ``run.sh``'s
``unittest discover`` with no CI-yaml edit — the entry-point contract
(``tests/integration/README.md``) is exactly that a new ``test_*.py`` is the
whole change.

(The paragraph that used to stand here called this module a component test
running against ``vitrin-mock-shim --seat --animate`` and said #110 "is not
yet green". #110 merged; the docstring was the stale half.)

Two rounds of work got this gate here, and the second matters as much as the
first:

* **#110 retired the mock shim.** Before it, the realm's ``command`` (and the
  core's ``--shim``) were both ``vitrin-mock-shim``: an animated buffer that
  stands in for nothing real and animated *regardless of actuation*, so a
  byte-diff between two captures proved a timer ran, not that the agent's
  click and typed text reached anything.
* **A real app repaints on its own too.** With the real chain in place the
  gate still asked only for 24 changed pixels in a 640x480 frame, and
  weston-terminal's own asynchronous first paint clears that unaided —
  measured at 569 changed pixels spanning 81 px across 19 scanlines, a diff
  indistinguishable in shape from typed text. So the gate passed whether or
  not the actuation landed. It now takes a **settled control capture** first
  (``run_demo._settle``, so the app's startup paint is inside the "before"
  frame rather than straddling it), watches the settled app idle for at least
  the window it later polls (``run_demo._idle_probe``, so an app that repaints
  steadily on its own fails the run instead of forging its evidence), and then
  requires a change carrying the shape a typed line makes
  (``run_demo.actuation_landed``: enough pixels AND a densely inked *run* of
  them along one scanline). ``HeadlessGateThresholdsStayDiscriminating`` below
  pins the numbers' ordering, and ``ChangeProfileShapeMetrics`` pins what the
  shape metric actually accepts and rejects, so neither can silently relax
  again.

  The run metric was itself a defect once, worth naming here because it is the
  kind that survives review: it measured the *bounding span* of a scanline's
  changed pixels while every sentence around it said "run". Three unrelated
  one-cell repaints at x=0, x=300 and x=600 -- a cursor, a mode indicator, a
  scrollbar -- have a 608 px bounding span and satisfied the gate.
  ``ChangeProfileShapeMetrics.test_scattered_one_cell_repaints_are_rejected``
  is that exact frame pair, asserted to fail.

Register, deliberately: this gate is mock-free, and it now *discriminates
actuation from incidental repaint* — but that second property dates from the
change that added ``_settle``, not from #110. A green run before that proved
the demo completed against a real app; it did not prove the agent's input
reached one.

What only a live, real chain can show — and what the mock-based SDK tests
cannot — is that the real enforcement chokepoint records the demo's
actuations, AND that the pixels changed because the click and typed text
actually reached a real app: an allowed ``move`` at the clicked coordinate
and an allowed ``type`` whose ``chars`` equals the typed text's length, in
the order the recorder is meant to reconstruct, backed by a pixel change the
app could not have drawn on its own.

The nested venue (real Firefox) is the workstation half of ``cargo xtask
demo``; it has no display or browser on a CI runner and is deliberately not
exercised here (plan risk R6/R1) — see ``shim/docs/firefox.md`` for its manual
walkthrough.

# Skip-or-fail policy (matches the real-app ladder's discipline)

- ``VITRIN_SKIP_REAL_APP=1`` -> skip. The shared real-app-ladder local opt-out
  (same variable ``test_real_app.py``/``test_real_actuation.py`` use).
- ``VITRIN_C_SHIM_BIN`` unset -> skip. A developer without a built C shim.
- ``VITRIN_C_SHIM_BIN`` **set** but the shim or ``weston-terminal`` is
  missing -> **fail**. CI sets the variable (``tests/integration/run.sh``'s
  callers already build the C shim for the real-app ladder), so CI cannot
  reach the skip — a gate that only ever reports SKIP on the machine that
  gates merges is a gate nobody is holding.

# The hold-Esc revocation half (issue #109/#110, PR #126 addendum)

Issue #110's acceptance criteria named one piece this module could not close
on its own: "hold-Esc revocation (#109) demonstrably failing the agent's
next actuation" against the demo's own real chain. That depended on #109 /
PR #126, which added the `dead-man-injector` cargo feature (a `SIGUSR1`
handler over the exact same `Runtime::apply_dead_man` entry point a
completed physical hold reaches) and `test_real_deadman.py`, the pattern
`DemoHeadlessHoldEsc` below mirrors -- against the demo's `weston-terminal`
chain and `run_demo`'s own grant shape/locator instead of `click-target`, so
this is the demo path specifically, not a second copy of the dead-man gate.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import signal
import sys
import tempfile
import time
import unittest

from harness import (
    IntegrationTest,
    capture_when_ready,
    children_of,
    comm_of,
    descendant_named,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

# The demo is a shipped example, not a package: put its directory on the path
# and import its entry point, so a failure is a traceback in *our* process.
_DEMO_DIR = pathlib.Path(__file__).resolve().parents[2] / "examples" / "agent-demo"
sys.path.insert(0, str(_DEMO_DIR))

import run_demo  # noqa: E402

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

#: The app the gate boots behind the real shim — never `vitrin-mock-shim`
#: (issue #110). Same rung `tests/integration/test_real_app.py` uses.
APP_NAME = "weston-terminal"

#: The headless / pure-software render selectors the real C shim's wlroots
#: backend needs (CI has no GPU). They reach the shim only through the
#: realm's `env_allow` — the one route a realm's environment may grow by —
#: seeded into the core's own environment for the allowlist to copy from.
#: Identical to `test_real_app.py`'s `WLR_ENV` and `crates/xtask`'s demo
#: launcher, so the three can never disagree about what the shim needs.
WLR_ENV = {
    "WLR_BACKENDS": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE": "1",
    "WLR_LIBINPUT_NO_DEVICES": "1",
}


def _use_decisions(entries: list[dict]) -> list[dict]:
    return [e for e in entries if e["kind"] == "use_decision"]


def _allowed(entries: list[dict], action: str) -> list[dict]:
    return [
        e
        for e in _use_decisions(entries)
        if e.get("decision") == "allowed" and (e.get("input") or {}).get("action") == action
    ]


class DemoHeadless(IntegrationTest):
    """The headless demo drives the REAL chain end to end and reconstructs.

    `vitrind -> vitrin-shim -> weston-terminal`, no `vitrin-mock-shim`
    anywhere on the path; the demo's `run` entry point does the rest
    (connect, the one MVP grant, consent, capture/click/type/capture).
    """

    def setUp(self) -> None:
        super().setUp()
        if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
            self.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")

        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real demo chain "
                "against. Build it (meson setup shim/build shim && meson compile -C "
                "shim/build) and point the variable at shim/build/vitrin-shim. CI sets it, "
                "so CI cannot reach this skip -- there a missing shim is a failure, below."
            )
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, "
                "so a real run was requested; refusing to skip a requested gate (CI misconfig)."
            )
        app = shutil.which(APP_NAME) or (
            f"/usr/bin/{APP_NAME}" if os.access(f"/usr/bin/{APP_NAME}", os.X_OK) else None
        )
        if app is None:
            self.fail(
                f"{APP_NAME} is not installed, but VITRIN_C_SHIM_BIN is set so a real run "
                "was requested. Install weston (shim/ci/install-deps.sh does), or set "
                "VITRIN_SKIP_REAL_APP=1 to opt out. A requested gate must not skip silently."
            )
        # Absolute: the core's spawn audit refuses a relative `command`
        # (crates/vitrin-core/src/spawn.rs).
        self.app_bin = str(pathlib.Path(app).resolve())

    def real_core(self):
        """A core booting the real chain: C shim + `weston-terminal` realm."""
        return self.core(
            size="640x480",
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=[],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
        )

    def _spine(self, core):
        """Wait out the real spawn spine and return `(shim_pid, app_pid)`.

        Same ancestry proof `test_real_app.py` makes: the core's one direct
        child is the real C shim (never `vitrin-mock-shim`), and the app is a
        grandchild of the core, parented by the shim -- proving the core
        never execs the app directly.
        """
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(
            shim_pid, f"the core forked no shim; children were {children_of(core.pid)}"
        )
        self.assertTrue(
            comm_of(shim_pid).startswith("vitrin-shim"),
            f"the core's child must be the real C shim, not {comm_of(shim_pid)!r} -- "
            "vitrin-mock-shim must appear in no demo venue (issue #110)",
        )
        app_pid = descendant_named(core.pid, APP_NAME, timeout=15.0)
        self.assertIsNotNone(
            app_pid,
            f"the C shim never fork/exec'd {APP_NAME}; core={core.pid} shim={shim_pid}",
        )
        return shim_pid, app_pid

    def test_demo_runs_and_the_recorder_reconstructs_the_session(self):
        core = self.real_core()
        shim_pid, app_pid = self._spine(core)
        self.assertEqual(
            {comm_of(core.pid), comm_of(shim_pid), comm_of(app_pid)},
            {"vitrind", "vitrin-shim", APP_NAME},
            "the demo's process spine must be exactly vitrind -> vitrin-shim -> "
            f"{APP_NAME}, with vitrin-mock-shim nowhere on it (issue #110)",
        )

        out_dir = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-demo-test-"))

        result = run_demo.run(
            str(core.socket),
            headless=True,
            consent="auto-approve",
            out_dir=out_dir,
            recorder=str(core.recorder),
        )

        # 1) The demo succeeded, and the change between its two captures
        #    carries the shape only the typed line makes -- the SAME predicate
        #    the demo's own `_assert_page_changed` already enforced above, so a
        #    passing `result.ok` already implies this; re-asserted here with the
        #    raw profile for a diagnosable test failure message.
        #
        #    `result.before` is the demo's *settled control* capture, not its
        #    first frame (`run_demo._settle`): weston-terminal's own startup
        #    paint has been measured at 569 changed pixels spanning 81 px, which
        #    is indistinguishable in shape from typed text, so the control frame
        #    -- not the thresholds -- is what makes this assertion mean
        #    "the agent's actuation landed".
        self.assertTrue(result.ok)
        profile = run_demo.change_profile(result.before, result.after)
        self.assertTrue(
            run_demo.actuation_landed(profile),
            f"{profile} between the settled control capture and the after-frame; need "
            f">= {run_demo.MIN_HEADLESS_CHANGED_PIXELS} px AND a dense run >= "
            f"{run_demo.MIN_HEADLESS_CHANGED_RUN} px wide (the bounding span is not the "
            f"criterion). Frames dumped under {out_dir}",
        )

        conn_closed_url = result.url
        core.terminate()
        entries = core.entries()
        kinds = [e["kind"] for e in entries]

        def _first(kind: str) -> int:
            self.assertIn(kind, kinds, f"recorder must contain {kind}; saw {kinds}")
            return kinds.index(kind)

        # 2) The lifecycle spine, in order: bind -> petition -> resolution.
        bind = _first("handshake_bound")
        petition = _first("petition_requested")
        resolved = _first("petition_resolved")
        self.assertLess(bind, petition, "bind must precede the petition")
        self.assertLess(petition, resolved, "the petition must precede its resolution")

        # 3) At least one admitted capture carrying a frame digest.
        captures = [
            e
            for e in _use_decisions(entries)
            if e.get("decision") == "allowed" and e.get("input") is None
        ]
        self.assertTrue(captures, "the demo's observe()s must be recorded as allowed captures")
        for cap in captures:
            frame = cap.get("frame")
            self.assertIsNotNone(frame, "every admitted capture carries a frame object (B1)")
            self.assertTrue(frame.get("digest"), "the frame object must carry a digest (B1)")
            self.assertEqual(frame.get("digest_alg"), "blake3")

        # 4) The clicked coordinate reached the chokepoint verbatim. The demo's
        #    headless locator is deterministic given the frame size, so
        #    recompute the exact point rather than hard-coding it.
        url_x, url_y = run_demo.locate_url_bar(result.before, headless=True)
        moves = _allowed(entries, "move")
        self.assertTrue(
            any(m["input"]["x"] == url_x and m["input"]["y"] == url_y for m in moves),
            f"an allowed move at the clicked ({url_x}, {url_y}) must be recorded; saw {moves}",
        )

        # 5) The typed string's shape reached the chokepoint: chars == len(text) + 1
        #    (the trailing "\n" that presses Enter). The recorder never holds
        #    the bytes (keylogger avoidance), only the count.
        expected_chars = len(conn_closed_url) + 1
        types = _allowed(entries, "type")
        self.assertTrue(
            any(t["input"]["chars"] == expected_chars for t in types),
            f"an allowed type with chars == {expected_chars} must be recorded; saw {types}",
        )

        # 6) Ordering: the capture(s), the move, and the type all follow the
        #    grant's resolution, and the move precedes the type (click, then
        #    type). Use recorder file order, which is `seq` order.
        move_idx = min(
            i for i, e in enumerate(entries)
            if e in moves  # identity match within this run's entries
        ) if moves else None
        type_idx = min(
            i for i, e in enumerate(entries) if e in types
        ) if types else None
        cap_idx = min(i for i, e in enumerate(entries) if e in captures)
        self.assertGreater(cap_idx, resolved, "captures must follow the resolution")
        self.assertIsNotNone(move_idx)
        self.assertIsNotNone(type_idx)
        self.assertLess(move_idx, type_idx, "the click (move) must precede the typed text")


class DemoHeadlessHoldEsc(IntegrationTest):
    """Issue #110's remaining acceptance criterion: hold-Esc revocation,
    demonstrated against the demo's own real chain (`vitrind` -> real
    `vitrin-shim` -> real `weston-terminal`), not `test_real_deadman.py`'s
    `click-target` chain.

    Headless has no physical Escape key to hold (`crate::deadman`'s module
    docs), so this drives the identical CI stand-in `test_real_deadman.py`
    established: a `SIGUSR1` to the core, meaningful only on a
    `dead-man-injector`-feature `vitrind` (`run.sh` builds one), synthesizes
    the completed chord through the exact same `Runtime::apply_dead_man`
    entry point a real held Escape reaches over the nested backend. What
    this test adds beyond that one is that the actuation the chord cuts off
    is the demo's own: the same grant shape (`harness.whole_realm_grant`,
    matching `run_demo.run`'s `observe + actuate.pointer + actuate.text`)
    and the same pixel locator (`run_demo.locate_url_bar`), reused rather
    than reimplemented so the two can never quietly disagree about what the
    demo does.
    """

    def setUp(self) -> None:
        super().setUp()
        if os.environ.get("VITRIN_SKIP_REAL_APP") == "1":
            self.skipTest("VITRIN_SKIP_REAL_APP=1 (shared real-app-ladder opt-out)")

        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim to run the real demo chain "
                "against. Build it (meson setup shim/build shim && meson compile -C "
                "shim/build) and point the variable at shim/build/vitrin-shim. CI sets it, "
                "so CI cannot reach this skip -- there a missing shim is a failure, below."
            )
        self.shim_bin = pathlib.Path(shim)
        if not (self.shim_bin.is_file() and os.access(self.shim_bin, os.X_OK)):
            self.fail(
                f"VITRIN_C_SHIM_BIN={shim} does not name an executable C shim. It is set, "
                "so a real run was requested; refusing to skip a requested gate (CI misconfig)."
            )
        app = shutil.which(APP_NAME) or (
            f"/usr/bin/{APP_NAME}" if os.access(f"/usr/bin/{APP_NAME}", os.X_OK) else None
        )
        if app is None:
            self.fail(
                f"{APP_NAME} is not installed, but VITRIN_C_SHIM_BIN is set so a real run "
                "was requested. Install weston (shim/ci/install-deps.sh does), or set "
                "VITRIN_SKIP_REAL_APP=1 to opt out. A requested gate must not skip silently."
            )
        self.app_bin = str(pathlib.Path(app).resolve())

    def real_core(self):
        """Identical to `DemoHeadless.real_core`: the demo's own real chain."""
        return self.core(
            size="640x480",
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=[],
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
        )

    def _spine(self, core) -> None:
        """Same ancestry proof `DemoHeadless._spine` makes."""
        deadline = time.monotonic() + 15.0
        shim_pid = None
        while time.monotonic() < deadline:
            kids = children_of(core.pid)
            if kids:
                shim_pid = kids[0]
                break
            time.sleep(0.05)
        self.assertIsNotNone(
            shim_pid, f"the core forked no shim; children were {children_of(core.pid)}"
        )
        self.assertTrue(
            comm_of(shim_pid).startswith("vitrin-shim"),
            f"the core's child must be the real C shim, not {comm_of(shim_pid)!r}",
        )
        app_pid = descendant_named(core.pid, APP_NAME, timeout=15.0)
        self.assertIsNotNone(
            app_pid, f"the C shim never fork/exec'd {APP_NAME}; core={core.pid} shim={shim_pid}"
        )

    def _send_dead_man_signal(self, core) -> None:
        """`SIGUSR1` to the core -- the test-gated stand-in for a completed
        hold-Esc (module docs above; identical to
        `test_real_deadman.py::RealDeadManRevocation._send_dead_man_signal`).
        Tells "the core revoked" apart from "the core died" immediately,
        rather than letting a feature-less build surface as a bare
        `ConnectionClosed` several assertions later with no clue why.
        """
        os.kill(core.pid, signal.SIGUSR1)
        # A feature-less `vitrind` has no SIGUSR1 handler installed at all
        # (the handler only exists under `dead-man-injector`), so the signal
        # takes its default disposition -- terminate -- and the process
        # dies within this window rather than revoking anything.
        time.sleep(0.5)
        if core.proc.poll() is not None:
            self.fail(
                f"the core exited (code {core.proc.returncode}) instead of revoking after "
                "SIGUSR1. This is what a `vitrind` built WITHOUT the `dead-man-injector` cargo "
                "feature does -- SIGUSR1's default disposition is terminate -- not what a "
                "completed dead-man chord does. Rebuild with `cargo build --workspace "
                "--features vitrin-core/dead-man-injector` (tests/integration/run.sh does this "
                f"automatically).\ncore output so far:\n{core.output()}"
            )

    def test_hold_esc_dead_man_revokes_the_demos_next_actuation_and_capture(self):
        core = self.real_core()
        self._spine(core)

        conn = core.connect()
        grant = whole_realm_grant(conn)

        # 1. Before the chord: the demo's own actuation channel is live
        #    against the real weston-terminal -- click the same point
        #    `run_demo.locate_url_bar` would (headless has no URL bar, so
        #    any in-bounds coordinate serves), type the demo's default
        #    input, and confirm the real app's pixels changed -- exactly
        #    `DemoHeadless`'s own read of "the actuation reached the app",
        #    reused rather than reimplemented.
        #
        #    Every step below is the demo's own helper, not a local copy:
        #    `_settle` for the control capture (which now ends in
        #    `_idle_probe`, so this precondition also inherits the "the app
        #    cannot forge the signature unaided" check) and
        #    `_capture_after_change` for the bounded poll, so this
        #    precondition can never drift from what `DemoHeadless` accepts
        #    -- including its shape metric. They are underscore-private
        #    by convention only; reaching for them deliberately is the same
        #    choice this class already makes for `locate_url_bar`.
        before = capture_when_ready(grant)
        before = run_demo._settle(grant, before)
        url_x, url_y = run_demo.locate_url_bar(before, headless=True)
        grant.pointer.click(url_x, url_y)
        grant.text.type(run_demo.DEFAULT_HEADLESS_INPUT + "\n")

        # Poll to a bounded deadline rather than sleeping a fixed 0.4 s and
        # taking a single observe(): a real app's response time is a
        # function of the runner's load, and one unlucky sample on a slow
        # CI machine used to fail this gate for reasons that had nothing to
        # do with revocation. Flaky gates get muted, and a muted gate is
        # worse than none. The assertion itself is NOT weakened -- it is the
        # same `actuation_landed` predicate `DemoHeadless` applies, polled
        # for instead of guessed at.
        mid = run_demo._capture_after_change(grant, before, headless=True)
        profile = run_demo.change_profile(before, mid)
        self.assertTrue(
            run_demo.actuation_landed(profile),
            f"{profile} before the chord fired (need >= "
            f"{run_demo.MIN_HEADLESS_CHANGED_PIXELS} px AND a dense run >= "
            f"{run_demo.MIN_HEADLESS_CHANGED_RUN} px wide, polled for up to "
            f"{run_demo.ACTUATION_POLL_BUDGET:.1f} s after a settled control capture "
            "the app was watched idle through); the demo's own actuation must have "
            "already reached the real app for a subsequent refusal to mean anything",
        )

        # 2. The chord fires: SIGUSR1 stands in for a completed hold-Esc
        #    (module docs), applied through the real `Runtime::apply_dead_man`
        #    -- the same entry point a real held Escape reaches nested.
        self._send_dead_man_signal(core)

        # 3. After the chord: the agent's very NEXT actuation AND capture --
        #    issue #110's named criterion -- both refuse `Revoked`, with no
        #    sleep/wait between the signal and these checks:
        #    `apply_dead_man` runs synchronously inside the signal handler's
        #    dispatch turn, before `_send_dead_man_signal`'s `kill()` call
        #    even returned.
        from vitrin_os.protocol import Verb

        with self.assertRaises(errors.Revoked) as observe_ctx:
            grant.observe()
        self.assertEqual(observe_ctx.exception.verb, Verb.OBSERVE)

        with self.assertRaises(errors.Revoked):
            grant.pointer.click(url_x, url_y)

        conn.close()
        core.terminate()
        entries = core.entries()

        # 4. The journal says why, in the documented write order
        #    (`deadman::apply`): the cause, then the revocations it explains.
        kinds = [e["kind"] for e in entries]
        self.assertIn(
            "dead_man_triggered", kinds, "the flight recorder must journal the completed chord"
        )
        triggered_at = kinds.index("dead_man_triggered")
        revoked_at = [i for i, k in enumerate(kinds) if k == "grant_revoked"]
        self.assertTrue(revoked_at, "the chord must have revoked at least the demo's own grant")
        self.assertTrue(
            all(i > triggered_at for i in revoked_at),
            "every grant_revoked entry must come AFTER the dead_man_triggered cause it explains",
        )

        print(
            "\n[demo-hold-esc] SIGUSR1 fired the dead-man switch mid-demo; the agent's next "
            "observe() and pointer.click() against the real weston-terminal chain both "
            f"refused Revoked; journal recorded {len(revoked_at)} grant_revoked "
            "entr" + ("y" if len(revoked_at) == 1 else "ies") + " after dead_man_triggered"
        )


class HeadlessGateThresholdsStayDiscriminating(unittest.TestCase):
    """The headless gate's numbers must keep their ordering, or it goes vacuous.

    Binary-free, so it runs (and can fail) in a from-scratch checkout. Each
    assertion pins one way the gate could quietly stop discriminating:

    - a settle tolerance at or above the change threshold would put the two
      numbers in an order nothing else in the design expects (note what this
      does *not* buy, below);
    - a run threshold below one character cell (~20 px at any plausible
      terminal font) would be cleared by a blinking cursor;
    - a gap tolerance that is not far below the run threshold would let two
      unrelated repaints chain into one "run", which is the bounding-span
      defect wearing the run's name;
    - an idle probe shorter than the post-actuation poll budget would vouch
      for less time than the gate then spends collecting evidence;
    - the pre-2026-07-25 value of 24 changed pixels is called out by name,
      because that is the number weston-terminal's own repaint cleared and
      the number a future "let's relax this, it's flaky" edit would drift
      back toward.
    """

    def test_settle_tolerance_stays_below_the_change_threshold(self):
        # What this pins: the two numbers keep the order the design reads in.
        # What it does NOT establish -- and an earlier version of this
        # docstring wrongly claimed it did -- is that the settle tolerance
        # "cannot swallow a change the gate demands". The tolerance is
        # per-poll-interval, the threshold is cumulative over the whole
        # post-actuation window, so an app repainting 150 px every 150 ms
        # settles happily and still accumulates past 400 px. What actually
        # rules that out is `run_demo._idle_probe` (see the two assertions
        # below it) -- not this ordering.
        self.assertLess(
            run_demo.SETTLE_MAX_CHANGED_PIXELS,
            run_demo.MIN_HEADLESS_CHANGED_PIXELS,
            "the settle tolerance must stay strictly under the change threshold; a "
            "tolerance at or above it would mean a single 'quiet' interval already "
            "carries a passing diff",
        )

    def test_run_threshold_cannot_be_cleared_by_one_character_cell(self):
        self.assertGreaterEqual(
            run_demo.MIN_HEADLESS_CHANGED_RUN,
            48,
            "the run threshold is the signature only a line of typed characters "
            "makes; below ~48 px a single blinking cursor cell clears it",
        )

    def test_gap_tolerance_cannot_chain_two_unrelated_repaints_into_a_run(self):
        # With a gap tolerance G, reaching a run of length R needs ink at
        # least every G px, i.e. at least R/G clusters. Pinning G <= R/2
        # keeps that at three or more, so no *pair* of unrelated repaints can
        # chain into a passing run however far apart the gate's other numbers
        # drift. (The ink-ratio rule does the rest of the work; see
        # `ChangeProfileShapeMetrics`.)
        self.assertLessEqual(
            run_demo.RUN_GAP_TOLERANCE * 2,
            run_demo.MIN_HEADLESS_CHANGED_RUN,
            "the gap tolerance must stay at or under half the run threshold, or two "
            "isolated repaints could chain into one 'run' -- the exact bounding-span "
            "defect this metric replaced",
        )

    def test_idle_probe_covers_the_whole_post_actuation_poll_budget(self):
        self.assertGreaterEqual(
            run_demo.IDLE_PROBE_SECONDS,
            run_demo.ACTUATION_POLL_BUDGET,
            "the idle probe vouches that the app draws nothing actuation-shaped on "
            "its own over the window the gate later polls; a probe shorter than that "
            "window vouches for less time than the gate spends collecting evidence",
        )

    def test_change_threshold_is_well_above_the_vacuous_pre_fix_value(self):
        self.assertGreaterEqual(
            run_demo.MIN_HEADLESS_CHANGED_PIXELS,
            256,
            "24 changed pixels in a 640x480 frame is what weston-terminal's own async "
            "first paint and shell prompt clear unaided; a threshold anywhere near it "
            "makes the M1.5 gate pass whether or not the agent's actuation landed",
        )


class SettleRequiresProofOfPaint(unittest.TestCase):
    """`_settle` must not accept a frame the app has never drawn into.

    The defect this class exists for, found on 2026-07-26 by the first CI run
    ever to execute `_idle_probe`: quiescence alone cannot tell "the app
    finished painting" from "the app has not started". A blank frame differs
    from its predecessor by 0 px every interval, indefinitely, so the settle
    loop would bless it as the control capture -- and weston-terminal's own
    startup paint (569 px, dense run 81) would then land inside the window the
    gate measures. That paint already satisfies `actuation_landed` unaided, so
    the consequence is not only the false FAILURE CI hit (paint inside the
    idle probe) but a false PASS (paint inside the post-actuation poll), in
    which the app's own drawing is credited as proof the agent's click and
    typed text arrived.

    Binary-free: frames are assembled from raw bytes, no display or app.
    """

    WIDTH, HEIGHT = 64, 8
    STRIDE = WIDTH * 4

    def _uniform(self, colour: bytes) -> "vitrin_os.Frame":
        buf = bytearray()
        for _ in range(self.WIDTH * self.HEIGHT):
            buf += colour + b"\x00"
        return vitrin_os.Frame(
            bytes(buf), format=vitrin_os.Format.XRGB8888,
            width=self.WIDTH, height=self.HEIGHT, stride=self.STRIDE,
        )

    def _with_ink(self) -> "vitrin_os.Frame":
        buf = bytearray(self.STRIDE * self.HEIGHT)
        buf[0:3] = b"\xff\xff\xff"
        return vitrin_os.Frame(
            bytes(buf), format=vitrin_os.Format.XRGB8888,
            width=self.WIDTH, height=self.HEIGHT, stride=self.STRIDE,
        )

    def test_an_all_black_frame_is_not_content(self):
        self.assertFalse(
            run_demo.frame_has_content(self._uniform(b"\x00\x00\x00")),
            "an all-black frame is the pre-paint state, not a drawn app",
        )

    def test_the_shims_opaque_background_is_not_content(self):
        self.assertFalse(
            run_demo.frame_has_content(self._uniform(b"\x20\x20\x20")),
            "a uniform non-black fill is the shim's opaque background or a "
            "toolkit's pre-chrome fill; treating it as a drawn app is exactly "
            "how a blank control capture got blessed",
        )

    def test_one_inked_pixel_is_content(self):
        self.assertTrue(
            run_demo.frame_has_content(self._with_ink()),
            "non-uniform and not all black is the app having drawn something",
        )

    def test_settle_refuses_a_never_painted_app_instead_of_blessing_it(self):
        """The regression: a permanently blank app must fail, not settle."""

        class BlankGrant:
            """Perfectly quiet, and perfectly blank -- the pathological case."""

            def __init__(self, frame):
                self.frame = frame
                self.observations = 0

            def observe(self):
                self.observations += 1
                return self.frame

        blank = self._uniform(b"\x20\x20\x20")
        grant = BlankGrant(blank)
        with self.assertRaises(run_demo.DemoAssertionError) as caught:
            run_demo._settle(grant, blank, timeout=1.5, poll=0.01, floor=0.0)
        self.assertIn("never drew anything", str(caught.exception))
        self.assertGreater(
            grant.observations, 3,
            "the loop must have polled well past `quiet_rounds`; if it gave up "
            "early it is not the paint precondition doing the work",
        )


class ChangeProfileShapeMetrics(unittest.TestCase):
    """What the gate's shape metric accepts and rejects, on frames built here.

    The thresholds above pin numbers against each other; this pins the
    *predicate* against pixels. Binary-free and deterministic: frames are
    assembled in-process from raw bytes, so no image codec enters any
    dependency graph (plan risk R7) and no display, shim or app is needed.

    Every case is a shape a real terminal actually draws, and the first one is
    the defect that made this class necessary: the gate used to measure the
    first-to-last *bounding span* of a scanline's changed pixels while calling
    it a "run", so three unrelated one-cell repaints -- a cursor at the left
    margin, a mode indicator mid-line, a scrollbar at the right edge, none of
    them anything the agent typed -- satisfied it.
    """

    WIDTH, HEIGHT = 640, 32
    STRIDE = WIDTH * 4
    #: One character cell, at the ~8x17 a terminal draws in a 640x480 view.
    CELL_W, CELL_H = 8, 17

    def _frame(self, pixels) -> "vitrin_os.Frame":
        """A black frame with white ink at the given ``(x, y)`` pixels."""
        buf = bytearray(self.STRIDE * self.HEIGHT)
        for x, y in pixels:
            off = y * self.STRIDE + x * 4
            buf[off : off + 3] = b"\xff\xff\xff"
        return vitrin_os.Frame(
            bytes(buf),
            format=vitrin_os.Format.XRGB8888,
            width=self.WIDTH,
            height=self.HEIGHT,
            stride=self.STRIDE,
        )

    def _cell(self, x0, y0=4, ink_columns=None):
        """One filled (or partly inked) character cell at ``(x0, y0)``."""
        columns = range(self.CELL_W) if ink_columns is None else ink_columns
        return [
            (x0 + dx, y0 + dy) for dy in range(self.CELL_H) for dx in columns
        ]

    def _profile(self, pixels):
        return run_demo.change_profile(self._frame([]), self._frame(pixels))

    def test_scattered_one_cell_repaints_are_rejected(self):
        # A cursor, a mode indicator and a scrollbar repainting on the same
        # scanlines, hundreds of pixels apart, drawing nothing anyone typed.
        pixels = self._cell(0) + self._cell(300) + self._cell(632)
        profile = self._profile(pixels)

        # It clears BOTH conditions the pre-2026-07-25 gate applied ...
        self.assertGreaterEqual(profile.count, run_demo.MIN_HEADLESS_CHANGED_PIXELS)
        self.assertGreaterEqual(
            profile.widest_row_span,
            run_demo.MIN_HEADLESS_CHANGED_RUN,
            "this fixture only tests anything if its bounding span clears the "
            "threshold the old, span-based gate applied",
        )
        # ... and must still be rejected, because no run in it is wider than
        # the single cell that drew it.
        self.assertLess(profile.widest_dense_run, run_demo.MIN_HEADLESS_CHANGED_RUN)
        self.assertFalse(
            run_demo.actuation_landed(profile),
            f"{profile}: three isolated one-cell repaints are not a typed line",
        )

    def test_a_typed_line_of_cells_is_accepted(self):
        # 16 monospace cells at an 8 px pitch, inked the way glyphs ink --
        # broken, never a solid bar. The real measurement this models: a real
        # weston-terminal echoing `echo vitrin-demo` produced a 143 px dense
        # run inked 96 px, and a *contiguous* run of only 9 px, which is why
        # the metric chains across intra-line gaps at all.
        pixels = []
        for cell in range(16):
            pixels += self._cell(cell * self.CELL_W, ink_columns=(1, 2, 4, 6))
        profile = self._profile(pixels)
        self.assertTrue(
            run_demo.actuation_landed(profile),
            f"{profile}: a 16-cell typed line must clear the gate, or the gate "
            "demands something the demo's own input cannot draw",
        )

    def test_one_blinking_cursor_cell_is_rejected(self):
        profile = self._profile(self._cell(40))
        self.assertFalse(
            run_demo.actuation_landed(profile),
            f"{profile}: one repainted cell is a cursor blink, not typed text",
        )

    def test_a_dotted_artefact_is_not_a_run(self):
        # Ink exactly at the gap tolerance, right across the frame: chained
        # by the gap rule alone, but nothing is drawn there. The ink-ratio
        # half of the metric is what rejects it.
        pixels = [
            (x, y)
            for y in range(self.HEIGHT)
            for x in range(0, self.WIDTH, run_demo.RUN_GAP_TOLERANCE)
        ]
        profile = self._profile(pixels)
        self.assertGreaterEqual(profile.count, run_demo.MIN_HEADLESS_CHANGED_PIXELS)
        self.assertFalse(
            run_demo.actuation_landed(profile),
            f"{profile}: a dotted artefact chained only by the gap tolerance is not "
            "a drawn line",
        )

    def test_identical_frames_profile_as_no_change(self):
        profile = self._profile([])
        self.assertEqual(
            (profile.count, profile.widest_dense_run, profile.rows), (0, 0, 0)
        )
        self.assertFalse(run_demo.actuation_landed(profile))


class DemoUsesNoMockShim(unittest.TestCase):
    """Grep-provable: `vitrin-mock-shim` is not the demo app in any venue.

    Issue #110's acceptance criterion, checked directly against the source
    rather than trusted: the mock shim binary name (quoted, as it would have
    to be to be passed as a `--shim`/`command` argument or bound to a Rust
    `PathBuf`/Python identifier) must not appear at all in the two files that
    actually launch a demo venue. Does not require a built workspace, so it
    runs (and can fail) even when `require_binaries()` above would otherwise
    skip the rest of this module in a from-scratch checkout.

    Deliberately excludes *this* file: `test_demo.py` is the file that states
    the forbidden literal in the first place (immediately below, as the very
    string this test searches for), so scanning it for its own search term
    would trivially self-match. The launcher (`crates/xtask`) and the shipped
    example (`run_demo.py`) are the two files issue #110 names as needing the
    rewire; this test's own prose is free to keep discussing the retired mock
    shim by name, as the module docstring above does.
    """

    #: Quoted, because that is the only form that could ever construct a
    #: path to the binary or pass it as an argument -- a bare, unquoted
    #: mention could only be prose (a comment or docstring), which is exactly
    #: what this test must NOT flag.
    _FORBIDDEN = ('"vitrin-mock-shim"', "'vitrin-mock-shim'")

    #: The two files issue #110 names as needing the rewire.
    _CHECKED = (
        pathlib.Path(__file__).resolve().parents[2] / "crates/xtask/src/main.rs",
        pathlib.Path(__file__).resolve().parents[2] / "examples/agent-demo/run_demo.py",
    )

    def test_mock_shim_is_not_constructed_by_either_demo_launcher(self):
        for path in self._CHECKED:
            text = path.read_text()
            for needle in self._FORBIDDEN:
                self.assertNotIn(
                    needle,
                    text,
                    f"{path.relative_to(path.parents[2])} still constructs a path/argument "
                    f"from the literal {needle} -- issue #110 requires vitrin-mock-shim "
                    "appear in no demo venue",
                )


if __name__ == "__main__":
    unittest.main()
