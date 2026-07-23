"""P1.8.4/P1.8.7 (#43/#110) acceptance: the demo agent's HEADLESS flow, against
the real chain end to end.

**Component test, not the M1.5 milestone gate (plan §5 D12, issue #111).**
This module drives the real ``vitrind`` binary but against
``vitrin-mock-shim --seat --animate`` (``harness.py``'s ``Core()`` default),
which is exactly the kind of mock seam D12 forbids as milestone evidence.
The *named* M1.5 gate is issue #110 (P1.8.7): it is not yet green, and until
it lands, ``cargo xtask demo --headless`` also still runs the mock shim as
the demo's app (see ``crates/xtask/src/main.rs``), not a real one. What this
module *does* prove — legitimately, as a component test — is that the demo
entry point drives the real enforcement chokepoint and that the flight
recorder reconstructs its actuations correctly; it rides ``run.sh``'s
``unittest discover`` with no CI-yaml edit — the entry-point contract
(``tests/integration/README.md``) is exactly that a new ``test_*.py`` is the
whole change.

Issue #110 retired the mock shim from this gate. Before, the realm's
``command`` (and the core's ``--shim``) were both ``vitrin-mock-shim``: an
animated buffer that stands in for nothing real, and that animated
*regardless of actuation* — so a byte-diff between two captures proved a
timer ran, not that the agent's click and typed text reached anything. This
module now drives the real chain instead: the shipped ``vitrind`` execs the
real C shim (``vitrin-shim``), which fork/execs a real, trivial Wayland
client (``weston-terminal``) inside its own confined Wayland socket — the same
rung ``tests/integration/test_real_app.py`` uses. The demo's ``run`` entry
point is imported and called (not run as a subprocess), so a failure surfaces
as a Python traceback with the demo's own frame dump rather than an opaque
non-zero exit.

What only a live, real chain can show — and what the mock-based SDK tests
cannot — is that the real enforcement chokepoint records the demo's
actuations, AND that the pixels changed because the click and typed text
actually reached a real app, not because a mock animated on its own clock:
an allowed ``move`` at the clicked coordinate and an allowed ``type`` whose
``chars`` equals the typed text's length, in the order the recorder is meant
to reconstruct, backed by a genuine pixel change too small to be a stray
blinking cursor and too real to be a mock's synthetic frame.

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

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)

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

        # 1) The demo succeeded, and enough real pixels changed that a stray
        #    blinking cursor could not explain it -- the SAME threshold the
        #    demo's own `_assert_page_changed` already enforced above, so a
        #    passing `result.ok` already implies this; re-asserted here with
        #    the raw count for a diagnosable test failure message.
        self.assertTrue(result.ok)
        changed = run_demo.count_changed_pixels(result.before, result.after)
        self.assertGreaterEqual(
            changed,
            run_demo.MIN_HEADLESS_CHANGED_PIXELS,
            f"only {changed} real pixel(s) changed between captures; frames dumped "
            f"under {out_dir}",
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
        before = capture_when_ready(grant)
        url_x, url_y = run_demo.locate_url_bar(before, headless=True)
        grant.pointer.click(url_x, url_y)
        grant.text.type(run_demo.DEFAULT_HEADLESS_INPUT + "\n")

        time.sleep(0.4)
        mid = grant.observe()
        changed = run_demo.count_changed_pixels(before, mid)
        self.assertGreaterEqual(
            changed,
            run_demo.MIN_HEADLESS_CHANGED_PIXELS,
            f"only {changed} pixel(s) changed before the chord fired; the demo's own "
            "actuation must have already reached the real app for a subsequent refusal "
            "to mean anything",
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
