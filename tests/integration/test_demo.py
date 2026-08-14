# SPDX-License-Identifier: Apache-2.0
"""P1.8.4/P1.8.7 (#43/#110) acceptance: the demo agent's HEADLESS flow, against
the real chain end to end.

**This is the named M1.5 acceptance gate** (issue #110, P1.8.7), as
``tests/integration/README.md``'s table records. It has no mock on any seam:
the shipped ``vitrind`` execs the real C shim (``vitrin-shim``), which
fork/execs a real Wayland client inside its own confined Wayland socket -- the
same rung ``tests/integration/test_real_app.py`` uses, with a different app
behind the shim.

# What the gate now demands, and why it changed

The demo is **goal-directed**: the agent is handed a task record it did not
author and must fill it into a real form, submit it, and prove from pixels
alone that the confirmation reflects the values it was told to enter. So the
acceptance criterion moved from a **diff** to a **positive content check**:

* it used to be "pixels moved" -- ``>= 400`` changed pixels and a ``>= 64`` px
  dense run between two captures. That is satisfiable by incidental repaint,
  and the whole history of this gate is the history of that being true:
  the threshold was 24 px until 2026-07-25 (``weston-terminal``'s own async
  first paint clears it unaided: measured 569 px, dense run 81), and the first
  replacement measured a *bounding span* while its prose derived a *run*, so
  three unrelated one-cell repaints at x=0, x=300 and x=600 cleared it with
  nothing typed.
* it is now "the frame contains three specific full-width colours that only
  **this task's** checksum produces, in order" -- an equality against a value
  computed from the SUPPLIED task at runtime, so it cannot be a hardcoded
  constant that would pass anyway. ``test_the_wrong_tasks_receipt_does_not_match``
  asserts the complement on the same real frame.

Every headless pixel threshold from the old gate was **deleted, not reworded**.
Those numbers were derived against ``weston-terminal`` glyph cells (measured:
1703 changed px, 9 px contiguous run, 143 px dense run); swapping the app
invalidates the derivation, and a reworded constant would be unjustified magic
-- which is the exact failure the repo's own gate-integrity pass was fought
over. The constants that replace them are derived in
``examples/agent-demo/run_demo.py`` against ``form-target``'s own geometry, and
this module's ``ChangeProfileShapeMetrics`` pins the predicate against pixels.

Beside the pixels there is an **out-of-band, byte-exact ground truth**:
``form-target`` prints ``SUBMIT ... canon=<hex>`` on stdout, the role
``gtk-entry-probe``'s ``ENTRY_HEX`` plays in ``test_real_actuation.py``. A
pixel receipt and an app's own byte report both agreeing is a materially
stronger claim than either alone.

# Disclosure: the M1.5 gate's app is now REPO-AUTHORED

State it here, in the gate, because it will be raised and it should be.

The headless app used to be ``weston-terminal``, third-party software. It is
now ``form-target`` (``shim/tests/form_target.c``). ``form-target`` is a real
Wayland client -- it binds real globals, commits real ``wl_shm`` buffers and
resolves real keys through the shim's real dynamically generated keymap -- and
it is **neither ``vitrin-mock-shim`` nor ``shim/tests/mock_core.c``**, so D12
(``docs/plan/01-phase-1-mvp.md`` §5) holds literally: no mock sits on any seam
this milestone claims.

But "the app is written by the same repo that asserts on it" is a fair
criticism, and a real reduction in independence. The mitigations, in the same
breath:

* **Precedent, not novelty.** The M1.4 actuation gate (#108) has used a
  repo-authored app, ``click-target``, since it landed, for the same reason:
  no third-party app gives a GPU-free, unambiguous, whole-frame response to a
  specific input that an ``observe()`` can assert without a human eyeballing
  it.
* **The third-party rungs stay green and stay in CI**:
  ``test_real_app.py`` (weston-terminal), ``test_real_gtk.py`` (a GTK app) and
  ``test_real_firefox.py`` (real Firefox) exercise the same shim, transport and
  actuation chokepoint against software nobody here wrote.
* **The pixel claim has an independent witness** in the same run: the app's
  ``SUBMIT`` line, compared against a value the *agent* computed.

What this does not rule out: a ``form-target`` bug that makes both the bands
and the ``SUBMIT`` line agree with a record the agent never delivered. Only a
third-party app could close that, and none offers this response shape. Read the
gate as what it is.

# Honesty about what "the agent" is

There is no language model anywhere in this. The agent is deterministic: it
scans its own captured frame for a known marker colour and clicks that
region's centre. And the receipt is a **checksum, not glyph recognition** --
the agent reads back a 36-bit function of the record the app received, never
the characters. "The agent read back what it typed" would be false, and no
assertion message in this file says it.

The nested venue (real Firefox) is the workstation half of ``cargo xtask
demo``; it has no display or browser on a CI runner and is deliberately not
exercised here (plan risk R6/R1) -- see ``shim/docs/firefox.md`` for its manual
walkthrough.

# Skip-or-fail policy (matches the real-app ladder's discipline)

- ``VITRIN_SKIP_REAL_APP=1`` -> skip. The shared real-app-ladder local opt-out
  (same variable ``test_real_app.py``/``test_real_actuation.py`` use).
- ``VITRIN_C_SHIM_BIN`` unset -> skip. A developer without a built C shim.
- ``VITRIN_C_SHIM_BIN`` **set** but the shim or ``form-target`` is missing ->
  **fail**. CI sets the variable, so CI cannot reach the skip -- and
  ``form-target`` is co-built with the shim unconditionally, so its absence
  beside a built shim is a build misconfiguration, not a machine state.

# The hold-Esc revocation half (issue #109/#110, PR #126 addendum)

Issue #110's acceptance criteria named one piece this module could not close
on its own: "hold-Esc revocation (#109) demonstrably failing the agent's
next actuation" against the demo's own real chain. ``DemoHeadlessHoldEsc``
below is that, rebuilt on the new chain: it clicks and types into
``form-target``'s first field using the demo's own helpers, confirms the ink
landed, fires the chord, and asserts the agent's very next ``observe()`` and
``pointer.click()`` both refuse ``Revoked``.
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

from harness import (
    IntegrationTest,
    await_shims,
    capture_when_ready,
    children_of,
    comm_of,
    descendant_named,
    exe_identity,
    file_identity,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

# The demo is a shipped example, not a package: put its directory on the path
# and import its entry point, so a failure is a traceback in *our* process.
_REPO = pathlib.Path(__file__).resolve().parents[2]
_DEMO_DIR = _REPO / "examples" / "agent-demo"
sys.path.insert(0, str(_DEMO_DIR))

import run_demo  # noqa: E402

import vitrin_os  # noqa: E402  (needs PYTHONPATH, which run.sh sets)
from vitrin_os import errors  # noqa: E402

#: The app the gate boots behind the real shim -- never `vitrin-mock-shim`
#: (issue #110). Repo-authored; see the module docstring's disclosure.
APP_NAME = "form-target"

#: The realm view, hence captured-frame size. Same as the rest of the real-app
#: ladder (`test_real_app.py`'s `REALM_SIZE`) and the size `form_target.c`'s
#: layout is authored at -- enlarging it would break parity with both.
REALM_SIZE = "640x480"
REALM_WH = (640, 480)

#: How long the app stays up: it must outlive the whole agent flow. Matches
#: `crates/xtask`'s `HEADLESS_APP_RUN_MS`.
APP_RUN_MS = "120000"

#: The task this gate hands the agent. **Deliberately NOT
#: `run_demo.TASK_DEFAULT`**: the point of the scenario is that the assertion
#: is computed from the supplied task at runtime, and a gate that only ever
#: exercised the shipped default could not tell that apart from a hardcoded
#: constant. `test_the_gates_task_is_not_the_shipped_default` asserts the two
#: produce different receipts, so this choice is provably load-bearing rather
#: than decorative.
TASK = (("name", "Grace Hopper"), ("email", "grace@example.net"))

#: A record the agent was never given, used for the negative half.
WRONG_TASK = (("name", "Grace Hopper"), ("email", "grace@example.org"))

#: The headless / pure-software render selectors the real C shim's wlroots
#: backend needs (CI has no GPU). They reach the shim only through the
#: realm's `env_allow` -- the one route a realm's environment may grow by --
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


def _app_args(task) -> list[str]:
    """The realm `args` for `form-target`: how long to live, and the field NAMES.

    The VALUES are deliberately absent. The only route they can reach the app
    by is the agent typing them through the real chokepoint -- which is the
    entire claim this gate makes.
    """
    args = ["--run-ms", APP_RUN_MS]
    for key, _value in task:
        args += ["--field", key]
    return args


def _resolve_app(shim_bin: pathlib.Path) -> str | None:
    """`form-target` built beside the C shim, or an explicit override."""
    explicit = os.environ.get("VITRIN_FORM_TARGET_APP")
    if explicit:
        return explicit
    sibling = shim_bin.resolve().parent / APP_NAME
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return str(sibling)
    return None


class _RealChainTest(IntegrationTest):
    """The shared skip-or-fail preamble and realm shape for both real rungs."""

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
        app = _resolve_app(self.shim_bin)
        if app is None:
            # form-target is co-built with the shim unconditionally
            # (shim/meson.build), so its absence beside a built shim is a
            # build misconfiguration to FAIL on, not a machine state.
            self.fail(
                f"no {APP_NAME} beside the C shim ({self.shim_bin.resolve().parent}), and "
                "VITRIN_C_SHIM_BIN is set. It is co-built with the shim "
                "(shim/meson.build); rebuild the shim, or set VITRIN_FORM_TARGET_APP."
            )
        # Absolute: the core's spawn audit refuses a relative `command`
        # (crates/vitrin-core/src/spawn.rs).
        self.app_bin = str(pathlib.Path(app).resolve())
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-demo-test-"))
        self.addCleanup(shutil.rmtree, self.work, True)

    def real_core(self, task=TASK):
        """A core booting the real chain: C shim + `form-target` realm."""
        return self.core(
            size=REALM_SIZE,
            shim=str(self.shim_bin),
            command=self.app_bin,
            args=_app_args(task),
            env_allow=tuple(WLR_ENV),
            extra_env=WLR_ENV,
            # A file, not a pipe: the shim under RUST_LOG=info is chatty and
            # this harness reads the child's output only after it exits, so a
            # run that filled the ~64 KiB pipe would wedge the writer.
            log_file=str(self.work / "core.log"),
        )

    def _spine(self, core):
        """Wait out the real spawn spine and return `(shim_pid, app_pid)`.

        Same ancestry proof `test_real_app.py` makes: the core's one direct
        child is the real C shim (never `vitrin-mock-shim`), and the app is a
        grandchild of the core, parented by the shim -- proving the core
        never execs the app directly.
        """
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
        app_pid = descendant_named(core.pid, APP_NAME, timeout=15.0)
        self.assertIsNotNone(
            app_pid,
            f"the C shim never fork/exec'd {APP_NAME}; core={core.pid} shim={shim_pid}",
        )
        return shim_pid, app_pid


class DemoHeadless(_RealChainTest):
    """The headless demo fills a task it was handed, submits it, and proves it.

    `vitrind -> vitrin-shim -> form-target`, no `vitrin-mock-shim` anywhere on
    the path; the demo's `run` entry point does the rest (connect, the one MVP
    grant, consent, then locate/click/type per field, submit, decode).
    """

    def test_the_gates_task_is_not_the_shipped_default(self):
        """The supplied task must actually change the expected answer.

        Binary-free. Without this, a green run above could not distinguish
        "the assertion is computed from the supplied task" from "the assertion
        is a constant that happens to match the shipped default".
        """
        self.assertNotEqual(TASK, run_demo.TASK_DEFAULT)
        self.assertNotEqual(
            run_demo.receipt_bands(TASK),
            run_demo.receipt_bands(run_demo.TASK_DEFAULT),
            "the gate's task must produce a different receipt from the shipped default, "
            "or a passing run cannot tell a computed assertion from a constant",
        )
        self.assertNotEqual(
            run_demo.receipt_bands(TASK),
            run_demo.receipt_bands(WRONG_TASK),
            "the negative half's task must produce a different receipt, or "
            "test_the_wrong_tasks_receipt_does_not_match asserts nothing",
        )

    def test_demo_fills_the_task_submits_it_and_the_receipt_matches(self):
        core = self.real_core()
        shim_pid, app_pid = self._spine(core)
        self.assertEqual(
            (comm_of(core.pid), exe_identity(shim_pid), comm_of(app_pid)),
            ("vitrind", file_identity(self.shim_bin), APP_NAME),
            "the demo's process spine must be exactly vitrind -> the real C shim -> "
            f"{APP_NAME}, with vitrin-mock-shim nowhere on it (issue #110). The shim is "
            "matched by the executing file's inode: a confined shim runs from the bind "
            "target /vitrin/shim and answers the same comm as the mock (P2.6.2, #186)",
        )

        out_dir = self.work / "frames"

        result = run_demo.run(
            str(core.socket),
            headless=True,
            consent="auto-approve",
            task=TASK,
            out_dir=out_dir,
            recorder=str(core.recorder),
        )
        self.assertTrue(result.ok)
        self.assertEqual((result.after.width, result.after.height), REALM_WH)

        # --- 1) POSITIVE CONTENT CHECK ---------------------------------------
        # Recompute the expected bands here, from the task this test supplied,
        # rather than trusting `result.bands`: the claim is "the frame carries
        # the checksum of the record the agent was told to enter".
        expected = run_demo.receipt_bands(TASK)
        self.assertEqual(result.bands, expected)
        runs = run_demo.solid_row_runs(result.after, expected)
        self.assertTrue(
            run_demo.match_bands(runs, expected),
            f"the confirmation frame must carry {len(expected)} full-width bands of >= "
            f"{run_demo.MIN_BAND_ROWS} rows, in order: "
            + ", ".join("#" + run_demo.rgb_hex(b) for b in expected)
            + f". Solid runs of those colours actually found: "
            + (", ".join(str(r) for r in runs) or "none")
            + f". Frames dumped under {out_dir}",
        )

        # --- 2) NEGATIVE: a record the agent was never given ----------------
        wrong = run_demo.receipt_bands(WRONG_TASK)
        self.assertFalse(
            run_demo.match_bands(run_demo.solid_row_runs(result.after, wrong), wrong),
            f"the same frame must NOT carry the receipt of {WRONG_TASK!r} "
            + "(#" + ", #".join(run_demo.rgb_hex(b) for b in wrong) + "). If it does, "
            "the band check is not discriminating between records at all",
        )

        # --- 2b) THE FOCUS-RING TRAP IS REAL IN THIS RUN --------------------
        # `ChangeProfileShapeMetrics` pins the mitigation on frames built
        # in-process; this asserts the real app actually SPRINGS the trap, so
        # the mitigation is exercised rather than merely asserted about. A
        # venue where the click alone changed nothing inside the field would
        # be a venue where the baseline ordering was never tested.
        self.assertEqual(len(result.focus_changes), len(TASK))
        for index, changed in enumerate(result.focus_changes):
            self.assertGreaterEqual(
                changed, run_demo.MIN_FIELD_INK_PIXELS,
                f"the click on field {index} changed only {changed} px inside the "
                f"field, under the {run_demo.MIN_FIELD_INK_PIXELS} px ink threshold. "
                f"{APP_NAME} draws a {run_demo.FIELD_RECT_INSET // 2} px focus ring "
                "inside the field rectangle deliberately, precisely so this gate "
                "exercises the baseline-after-click mitigation; if the ring is gone, "
                "the trap is no longer sprung and the mitigation is untested here",
            )

        # --- 3) OUT-OF-BAND, BYTE-EXACT GROUND TRUTH ------------------------
        # The app's own report of what it received, independent of pixels.
        core.terminate()
        # A confined realm writes to its own log rather than inheriting the
        # core's descriptors (P2.6.2, #186), so the app's SUBMIT line is no
        # longer in `core.output()`.
        out = core.app_output()
        submit_line = next(
            (ln for ln in out.splitlines() if ln.startswith("SUBMIT ")), None
        )
        self.assertIsNotNone(
            submit_line,
            f"{APP_NAME} never printed a SUBMIT line: the click on the located button "
            f"did not reach it.\n{out[-4000:]}",
        )
        fields = dict(
            part.split("=", 1) for part in submit_line.split()[1:] if "=" in part
        )
        self.assertEqual(
            fields.get("canon"),
            run_demo.canonical_task(TASK).encode("utf-8").hex(),
            f"{APP_NAME} received {fields.get('canon')!r}, not the hex of "
            f"{run_demo.canonical_task(TASK)!r}. The agent's typed values did not "
            "arrive intact -- and this is the byte-exact half of the proof, which no "
            "pixel check can make",
        )
        for index, (_key, value) in enumerate(TASK):
            self.assertEqual(
                fields.get(f"f{index}"),
                value.encode("utf-8").hex(),
                f"field {index} received {fields.get(f'f{index}')!r}, not "
                f"{value!r} hex-encoded",
            )
        for index, band in enumerate(expected):
            self.assertEqual(
                fields.get(f"band{index}"),
                run_demo.rgb_hex(band),
                "the app's own band derivation must agree with the Python reference "
                "(the encoding is normative in examples/agent-demo/README.md)",
            )

        # --- 4) THE RECORDER RECONSTRUCTS THE SESSION -----------------------
        entries = core.entries()
        kinds = [e["kind"] for e in entries]

        def _first(kind: str) -> int:
            self.assertIn(kind, kinds, f"recorder must contain {kind}; saw {kinds}")
            return kinds.index(kind)

        bind = _first("handshake_bound")
        petition = _first("petition_requested")
        resolved = _first("petition_resolved")
        self.assertLess(bind, petition, "bind must precede the petition")
        self.assertLess(petition, resolved, "the petition must precede its resolution")

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

        # Every point the agent clicked reached the chokepoint verbatim: one
        # per field, then the submit button. Read back from the result rather
        # than recomputed -- the agent located them in its own capture, so
        # there is no second derivation that could disagree.
        self.assertEqual(
            len(result.clicks), len(TASK) + 1,
            f"the agent must have clicked each of {len(TASK)} fields and the submit "
            f"button; it recorded {result.clicks}",
        )
        moves = _allowed(entries, "move")
        for point in result.clicks:
            self.assertTrue(
                any(m["input"]["x"] == point[0] and m["input"]["y"] == point[1] for m in moves),
                f"an allowed move at the clicked {point} must be recorded; saw "
                f"{[(m['input']['x'], m['input']['y']) for m in moves]}",
            )

        # Each field's typed value reached the chokepoint with its own shape.
        # `chars`, not bytes: the recorder never holds the text (keylogger
        # avoidance), only the count.
        types = _allowed(entries, "type")
        typed_chars = [t["input"]["chars"] for t in types]
        for _key, value in TASK:
            self.assertIn(
                len(value), typed_chars,
                f"an allowed type with chars == {len(value)} (for {value!r}) must be "
                f"recorded; saw {typed_chars}",
            )
        self.assertEqual(
            len(types), len(TASK),
            "headless types exactly once per field -- no URL bar, and NO trailing "
            f"newline (submission is a click on the located button); saw {typed_chars}",
        )

        # Ordering: the first click precedes the first typed value.
        move_idx = min(i for i, e in enumerate(entries) if e in moves)
        type_idx = min(i for i, e in enumerate(entries) if e in types)
        cap_idx = min(i for i, e in enumerate(entries) if e in captures)
        self.assertGreater(cap_idx, resolved, "captures must follow the resolution")
        self.assertLess(move_idx, type_idx, "the click (move) must precede the typed text")

        print(
            f"\n[demo] agent filled a task it was handed ({run_demo.canonical_task(TASK)!r}), "
            f"clicked {result.clicks}, and the confirmation carried this record's 36-bit "
            "receipt "
            + " ".join("#" + run_demo.rgb_hex(b) for b in expected)
            + f"; {APP_NAME} independently reported the same bytes"
        )


class DemoHeadlessHoldEsc(_RealChainTest):
    """Issue #110's remaining acceptance criterion: hold-Esc revocation,
    demonstrated against the demo's own real chain (`vitrind` -> real
    `vitrin-shim` -> real `form-target`), not `test_real_deadman.py`'s
    `click-target` chain.

    Headless has no physical Escape key to hold (`crate::deadman`'s module
    docs), so this drives the identical CI stand-in `test_real_deadman.py`
    established: a `SIGUSR1` to the core, meaningful only on a
    `dead-man-injector`-feature `vitrind` (`run.sh` builds one), synthesizes
    the completed chord through the exact same `Runtime::apply_dead_man`
    entry point a real held Escape reaches over the nested backend. What
    this test adds beyond that one is that the actuation the chord cuts off
    is the demo's own: the same grant shape (`harness.whole_realm_grant`,
    matching `run_demo.run`'s `observe + actuate.pointer + actuate.text`) and
    the demo's own locator, baseline and ink helpers, reused rather than
    reimplemented so the two can never quietly disagree about what the demo
    does.
    """

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
        #    against the real form -- locate field 0 by its marker colour the
        #    way `run_demo.run` does, click its centre, baseline AFTER the
        #    click (so the app's focus ring is inside the baseline, not the
        #    measured diff), type, and confirm the ink landed inside the
        #    field. Every step is the demo's own helper, not a local copy, so
        #    this precondition can never drift from what `DemoHeadless`
        #    accepts. They are underscore-private by convention only.
        capture_when_ready(grant)
        _frame, marker = run_demo._locate_with_poll(
            grant, run_demo.FIELD_MARKERS[0], what="field 0"
        )
        click_x, click_y = marker.click_point
        grant.pointer.click(click_x, click_y)
        baseline = run_demo._baseline_after_click(grant, marker.rect)
        value = TASK[0][1]
        grant.text.type(value)
        measured = marker.rect.inset(run_demo.FIELD_RECT_INSET)
        _after, ink = run_demo._await_ink(grant, baseline, measured, what="field 0")
        self.assertGreaterEqual(
            ink, run_demo.MIN_FIELD_INK_PIXELS,
            "the demo's own actuation must have already reached the real app for a "
            "subsequent refusal to mean anything",
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
            grant.pointer.click(click_x, click_y)

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

        # 5. The app never saw the revoked submit: it printed no SUBMIT line,
        #    because the click that would have produced one was refused.
        # A confined realm writes to its own log rather than inheriting the
        # core's descriptors (P2.6.2, #186), so the app's SUBMIT line is no
        # longer in `core.output()`.
        out = core.app_output()
        self.assertNotIn(
            "SUBMIT ", out,
            f"{APP_NAME} reported a submission after the chord fired; the refused click "
            "must never have reached its wl_seat",
        )

        print(
            "\n[demo-hold-esc] SIGUSR1 fired the dead-man switch mid-demo; the agent's next "
            f"observe() and pointer.click() against the real {APP_NAME} chain both refused "
            f"Revoked; journal recorded {len(revoked_at)} grant_revoked "
            "entr" + ("y" if len(revoked_at) == 1 else "ies") + " after dead_man_triggered"
        )


class ReceiptEncodingIsPinned(unittest.TestCase):
    """The normative receipt encoding, and its three implementations.

    `examples/agent-demo/README.md` defines it; the Python in `run_demo.py` is
    the reference; `form.html` (JS) and `shim/tests/form_target.c` (C) restate
    it. Three copies of a hash function is exactly the shape that drifts
    silently, so each is pinned against the reference here on the shipped
    default task.
    """

    #: The published FNV-1a-32 test vectors. Pinning these means a "harmless"
    #: edit to the six-line hash (a wrong prime, a missing mask, XOR/multiply
    #: swapped into FNV-1) fails here rather than by making every receipt
    #: mismatch look like a delivery failure.
    FNV_VECTORS = (
        (b"", 0x811C9DC5),
        (b"a", 0xE40C292C),
        (b"foobar", 0xBF9CF968),
    )

    #: The literals `examples/agent-demo/README.md` publishes for the shipped
    #: default task. If the encoding changes, this fails and the README has to
    #: be edited in the same commit.
    DEFAULT_CANON = "name=Ada Lovelace\nemail=ada@example.org"
    DEFAULT_BANDS = ("993300", "aacc33", "cc5566")

    def test_fnv1a32_matches_the_published_vectors(self):
        for data, expected in self.FNV_VECTORS:
            self.assertEqual(run_demo.fnv1a32(data), expected, f"fnv1a32({data!r})")

    def test_the_canonical_string_is_order_sensitive(self):
        forward = (("a", "1"), ("b", "2"))
        reversed_ = (("b", "2"), ("a", "1"))
        self.assertEqual(run_demo.canonical_task(forward), "a=1\nb=2")
        self.assertNotEqual(
            run_demo.canonical_task(forward), run_demo.canonical_task(reversed_)
        )
        self.assertNotEqual(
            run_demo.receipt_bands(forward), run_demo.receipt_bands(reversed_),
            "reordering the same pairs must change the receipt: order is part of the "
            "record, which is why the task is a tuple of pairs and never a dict",
        )

    def test_the_readme_literals_still_describe_the_code(self):
        self.assertEqual(run_demo.canonical_task(run_demo.TASK_DEFAULT), self.DEFAULT_CANON)
        self.assertEqual(
            tuple(run_demo.rgb_hex(b) for b in run_demo.receipt_bands(run_demo.TASK_DEFAULT)),
            self.DEFAULT_BANDS,
            "examples/agent-demo/README.md publishes these three colours for the shipped "
            "default task; the README and the code must be edited together",
        )

    def test_every_band_channel_is_a_multiple_of_0x11(self):
        # Why this matters: 0x11 multiples survive the capture path AND the
        # 4-bit-per-channel histogram exactly, with no tolerance, which is what
        # lets the band check be an equality rather than a distance.
        for task in (run_demo.TASK_DEFAULT, TASK, WRONG_TASK, (("x", "y"), ("z", "w"))):
            for band in run_demo.receipt_bands(task):
                for channel in band:
                    self.assertEqual(channel % 0x11, 0, f"{band} in {task}")

    def test_the_c_implementation_agrees_with_the_python_reference(self):
        """`form-target --bands CANON` computes the same three colours.

        `--bands` touches no Wayland at all -- it calls the same `band_rgb` the
        paint path calls and exits -- which is what makes this runnable on a
        machine with no compositor.
        """
        shim = os.environ.get("VITRIN_C_SHIM_BIN")
        if not shim:
            self.skipTest(
                "VITRIN_C_SHIM_BIN is unset: no built C shim, so no co-built "
                f"{APP_NAME} to pin the C restatement against. CI sets it."
            )
        app = _resolve_app(pathlib.Path(shim))
        self.assertIsNotNone(
            app,
            f"no {APP_NAME} beside the C shim, and VITRIN_C_SHIM_BIN is set; it is "
            "co-built with the shim (shim/meson.build)",
        )
        canon = run_demo.canonical_task(run_demo.TASK_DEFAULT)
        proc = subprocess.run(
            [app, "--bands", canon], capture_output=True, text=True, timeout=30
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(
            proc.stdout.split(),
            ["BANDS", *self.DEFAULT_BANDS],
            f"{APP_NAME}'s C restatement of the receipt encoding disagrees with the "
            "Python reference. The README and the Python win; the C is the bug.",
        )

    def test_the_js_implementation_agrees_with_the_python_reference(self):
        """The `FNV-BEGIN`/`FNV-END` block in `form.html`, run under node.

        The JS half only ever runs in the nested venue, which CI never
        exercises, so a runner without a JS engine skips rather than fails --
        and says which engine it looked for, so the skip is diagnosable
        instead of mysterious.
        """
        node = shutil.which("node") or shutil.which("nodejs")
        if node is None:
            self.skipTest(
                "no `node`/`nodejs` on PATH to evaluate form.html's JS restatement of "
                "the receipt encoding. The JS half runs only in the nested venue, which "
                "CI does not exercise, so this is a skip rather than a failure."
            )
        html = (_DEMO_DIR / "form.html").read_text()
        block = re.search(r"/\* FNV-BEGIN.*?\*/(.*?)/\* FNV-END \*/", html, re.S)
        self.assertIsNotNone(
            block,
            "form.html must keep the /* FNV-BEGIN */ ... /* FNV-END */ markers around "
            "its receipt-encoding functions; this test extracts them by name",
        )
        script = block.group(1) + (
            "\nvar pairs = %s;\n"
            "var canon = canonical(pairs);\n"
            "console.log(JSON.stringify([canon, bandHex(canon,0), bandHex(canon,1), "
            "bandHex(canon,2)]));\n"
        ) % json.dumps([[k, v] for k, v in run_demo.TASK_DEFAULT])
        proc = subprocess.run(
            [node, "-e", script], capture_output=True, text=True, timeout=60
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        canon, *bands = json.loads(proc.stdout)
        self.assertEqual(canon, self.DEFAULT_CANON)
        self.assertEqual(
            bands,
            ["#" + hex6 for hex6 in self.DEFAULT_BANDS],
            "form.html's JS restatement of the receipt encoding disagrees with the "
            "Python reference. The README and the Python win; the JS is the bug.",
        )


class DefaultTaskAgreesAcrossLaunchers(unittest.TestCase):
    """`crates/xtask`'s `DEFAULT_TASK` keys must equal `run_demo.TASK_DEFAULT`'s.

    Two definitions exist because the launcher passes the field NAMES to the
    app (`--field NAME`, so it can build the same canonical string) while the
    agent types the VALUES. A silent disagreement would make the receipt
    unmatchable for a reason that looks exactly like a delivery failure -- the
    hardest kind of failure to read. Source-level and binary-free, so it runs
    in a from-scratch checkout.
    """

    def test_the_launcher_and_the_agent_default_to_the_same_record(self):
        text = (_REPO / "crates/xtask/src/main.rs").read_text()
        match = re.search(r"const DEFAULT_TASK:[^=]*=\s*\[(.*?)\];", text, re.S)
        self.assertIsNotNone(
            match, "crates/xtask/src/main.rs must declare `const DEFAULT_TASK`"
        )
        pairs = re.findall(r'\(\s*"([^"]*)"\s*,\s*"([^"]*)"\s*\)', match.group(1))
        self.assertEqual(
            [tuple(p) for p in pairs],
            list(run_demo.TASK_DEFAULT),
            "crates/xtask's DEFAULT_TASK and run_demo.TASK_DEFAULT must name the same "
            "record in the same order",
        )


class TaskInputIsValidated(unittest.TestCase):
    """`--task K=V` parsing: order preserved, illegal payloads refused early.

    Binary-free. The refusals matter because the IDL makes the whole Cc
    category except newline/tab a **fatal** `invalid_argument` on
    `vitrin_actuator_text.type` -- an unchecked value would kill the
    connection rather than fail an assertion, and the run would report a
    transport error instead of a bad task.
    """

    def test_pairs_keep_their_order(self):
        task = run_demo.parse_task(["b=2", "a=1"])
        self.assertEqual(task, (("b", "2"), ("a", "1")))

    def test_a_value_may_contain_equals_signs(self):
        task = run_demo.parse_task(["k=a=b", "j=c"])
        self.assertEqual(task, (("k", "a=b"), ("j", "c")))

    def test_no_task_yields_the_shipped_default(self):
        self.assertEqual(run_demo.parse_task(None), run_demo.TASK_DEFAULT)
        self.assertEqual(run_demo.parse_task([]), run_demo.TASK_DEFAULT)

    def test_control_characters_are_refused(self):
        for bad in ("\x00", "\x1b", "\x7f", "\x9f", "\n", "\t"):
            with self.assertRaises(run_demo.TaskError, msg=repr(bad)):
                run_demo.parse_task([f"k=x{bad}y", "j=ok"])

    def test_an_oversized_value_is_refused(self):
        big = "a" * (run_demo.MAX_TEXT_BYTES + 1)
        with self.assertRaises(run_demo.TaskError):
            run_demo.parse_task([f"k={big}", "j=ok"])
        # Multi-byte characters are measured in BYTES, not characters, because
        # that is what the IDL caps.
        just_over = "é" * (run_demo.MAX_TEXT_BYTES // 2 + 1)
        with self.assertRaises(run_demo.TaskError):
            run_demo.parse_task([f"k={just_over}", "j=ok"])

    def test_the_wrong_number_of_fields_is_refused(self):
        for specs in (["k=v"], ["a=1", "b=2", "c=3"]):
            with self.assertRaises(run_demo.TaskError, msg=repr(specs)):
                run_demo.parse_task(specs)

    def test_a_malformed_spec_is_refused(self):
        for bad in ("novalue", "=v"):
            with self.assertRaises(run_demo.TaskError, msg=bad):
                run_demo.parse_task([bad, "j=ok"])


class ChangeProfileShapeMetrics(unittest.TestCase):
    """What the per-field ink check accepts and rejects, on frames built here.

    **This class exists for one failure mode, and it is the one this repo has
    already been burned by twice** (see `docs/plan/01-phase-1-mvp.md`'s D12
    seam table): a metric whose prose says one thing and whose pixels say
    another.

    Here the trap is the **focus ring**. A real app draws a focus indicator
    when a field is clicked, and that indicator is a change *inside* the
    field's bounding box that no typing produced -- often *larger* than the
    typed text (a 2 px ring around a 560x44 field is 2400 px; the shipped
    task's value inks ~576 px). A naive "did anything change in the field?"
    check passes on it with nothing typed at all.

    `form-target` draws its ring 2 px inside the field rectangle deliberately,
    so the real gate springs the trap; `form.html` uses a 3 px inset
    box-shadow. Two mitigations answer it, and the tests below pin both:
    the ink baseline is taken AFTER the click, and the measured rectangle is
    inset past the ring (`run_demo.FIELD_RECT_INSET`).

    Binary-free and deterministic: frames are assembled in-process from raw
    bytes, so no image codec enters any dependency graph (plan risk R7) and no
    display, shim or app is needed.
    """

    WIDTH, HEIGHT = 640, 480
    STRIDE = WIDTH * 4

    #: `form_target.c`'s field 0, verbatim.
    FIELD = run_demo.Rect(40, 96, 600, 140)
    FIELD_RGB = (0x00, 0xFF, 0x00)
    RING_W = 2

    def _blank(self) -> bytearray:
        buf = bytearray(b"\x00\x00\x00\xff" * (self.WIDTH * self.HEIGHT))
        self._rect(buf, run_demo.Rect(0, 0, self.WIDTH, self.HEIGHT), (0xFF, 0xFF, 0xFF))
        return buf

    def _rect(self, buf: bytearray, rect: run_demo.Rect, rgb) -> None:
        pixel = bytes((rgb[2], rgb[1], rgb[0], 0xFF))
        for y in range(rect.y0, rect.y1):
            base = (y * self.WIDTH + rect.x0) * 4
            buf[base : base + rect.width * 4] = pixel * rect.width

    def _frame(self, buf: bytearray):
        return vitrin_os.Frame(
            bytes(buf),
            format=vitrin_os.Format.XRGB8888,
            width=self.WIDTH,
            height=self.HEIGHT,
            stride=self.STRIDE,
        )

    def _form(self) -> bytearray:
        buf = self._blank()
        self._rect(buf, self.FIELD, self.FIELD_RGB)
        return buf

    def _with_ring(self) -> bytearray:
        """The form, plus a focus ring drawn just INSIDE the field rectangle."""
        buf = self._form()
        f, w = self.FIELD, self.RING_W
        self._rect(buf, run_demo.Rect(f.x0, f.y0, f.x1, f.y0 + w), (0, 0, 0))
        self._rect(buf, run_demo.Rect(f.x0, f.y1 - w, f.x1, f.y1), (0, 0, 0))
        self._rect(buf, run_demo.Rect(f.x0, f.y0, f.x0 + w, f.y1), (0, 0, 0))
        self._rect(buf, run_demo.Rect(f.x1 - w, f.y0, f.x1, f.y1), (0, 0, 0))
        return buf

    def _with_ink(self, byte_count: int) -> bytearray:
        """The ringed form, plus `form-target`'s ink: a 4x12 cell per byte."""
        buf = self._with_ring()
        x = self.FIELD.x0 + 8
        y = self.FIELD.y0 + 16
        for _ in range(byte_count):
            self._rect(buf, run_demo.Rect(x, y, x + 4, y + 12), (0, 0, 0))
            x += 6
        return buf

    # -- the trap ---------------------------------------------------------

    def test_a_focus_ring_alone_is_rejected_by_the_inset_rectangle(self):
        before, after = self._frame(self._form()), self._frame(self._with_ring())
        measured = self.FIELD.inset(run_demo.FIELD_RECT_INSET)
        ink = run_demo.changed_in_rect(before, after, measured)
        self.assertLess(
            ink, run_demo.MIN_FIELD_INK_PIXELS,
            f"{ink} px: a focus ring drawn inside the field must not read as typed "
            "text. This is the exact defect class the gate has been burned by twice",
        )

    def test_the_same_ring_would_be_accepted_without_the_inset(self):
        """The inset is what does the work -- proven, not asserted.

        Without this, `test_a_focus_ring_alone_is_rejected...` could be passing
        because the fixture's ring is too small to matter, and the mitigation
        it claims to pin would be untested.
        """
        before, after = self._frame(self._form()), self._frame(self._with_ring())
        ink = run_demo.changed_in_rect(before, after, self.FIELD)
        self.assertGreaterEqual(
            ink, run_demo.MIN_FIELD_INK_PIXELS,
            f"{ink} px: this fixture only tests anything if the ring WOULD clear the "
            "ink threshold when the rectangle is not inset",
        )

    def test_a_baseline_taken_after_the_click_sees_only_the_typing(self):
        """The other mitigation: baseline ordering, measured on the full rect.

        Baselining after the click puts the ring in the baseline, so even
        without the inset the diff is the ink alone.
        """
        baseline = self._frame(self._with_ring())
        after = self._frame(self._with_ink(12))
        self.assertEqual(
            run_demo.changed_in_rect(baseline, after, self.FIELD),
            12 * 4 * 12,
            "12 bytes of form-target ink is exactly 12 filled 4x12 cells; anything else "
            "means the baseline is not the post-click frame",
        )

    # -- what a typed value looks like ------------------------------------

    def test_the_shipped_tasks_ink_clears_the_threshold_with_margin(self):
        baseline = self._frame(self._with_ring())
        for _key, value in run_demo.TASK_DEFAULT:
            byte_count = len(value.encode("utf-8"))
            after = self._frame(self._with_ink(byte_count))
            ink = run_demo.changed_in_rect(
                baseline, after, self.FIELD.inset(run_demo.FIELD_RECT_INSET)
            )
            self.assertGreaterEqual(
                ink, run_demo.MIN_FIELD_INK_PIXELS,
                f"{value!r} inks {ink} px, under the {run_demo.MIN_FIELD_INK_PIXELS} px "
                "the gate demands: the gate would be asking for something the demo's "
                "own task cannot draw",
            )

    def test_a_three_byte_value_is_the_documented_floor(self):
        """`MIN_FIELD_INK_PIXELS`'s derivation, checked rather than trusted."""
        baseline = self._frame(self._with_ring())
        measured = self.FIELD.inset(run_demo.FIELD_RECT_INSET)
        self.assertGreaterEqual(
            run_demo.changed_in_rect(baseline, self._frame(self._with_ink(3)), measured),
            run_demo.MIN_FIELD_INK_PIXELS,
            "run_demo derives the threshold as 'any value of 3 bytes or more'; 3 bytes "
            "of ink must therefore clear it",
        )
        self.assertLess(
            run_demo.changed_in_rect(baseline, self._frame(self._with_ink(2)), measured),
            run_demo.MIN_FIELD_INK_PIXELS,
            "and 2 bytes must not -- otherwise the documented floor is wrong in the "
            "other direction and the prose is unjustified",
        )

    def test_ink_outside_the_field_does_not_count(self):
        buf = self._with_ring()
        # A big change in the OTHER field's area: a repaint the agent did not
        # cause in the field the agent clicked.
        self._rect(buf, run_demo.Rect(40, 176, 600, 220), (0, 0, 0))
        after = self._frame(buf)
        self.assertEqual(
            run_demo.changed_in_rect(
                self._frame(self._with_ring()),
                after,
                self.FIELD.inset(run_demo.FIELD_RECT_INSET),
            ),
            0,
            "a change elsewhere in the frame must not be credited to this field",
        )

    def test_identical_frames_have_no_ink(self):
        frame = self._frame(self._form())
        self.assertEqual(run_demo.changed_in_rect(frame, frame, self.FIELD), 0)

    # -- the locator ------------------------------------------------------

    def test_the_locator_finds_the_field_rectangle_and_its_centre(self):
        marker = run_demo.locate_marker(self._frame(self._form()), "00ff00")
        self.assertIsNotNone(marker)
        self.assertEqual(marker.rect, self.FIELD)
        self.assertEqual(marker.count, self.FIELD.width * self.FIELD.height)
        self.assertEqual(marker.click_point, (320, 118))
        self.assertGreaterEqual(marker.count, run_demo.MIN_MARKER_PIXELS)

    def test_an_absent_marker_locates_as_none(self):
        self.assertIsNone(run_demo.locate_marker(self._frame(self._blank()), "00ff00"))

    def test_a_stray_speck_of_the_marker_colour_is_below_the_locator_floor(self):
        buf = self._blank()
        self._rect(buf, run_demo.Rect(10, 10, 30, 30), self.FIELD_RGB)
        marker = run_demo.locate_marker(self._frame(buf), "00ff00")
        self.assertIsNotNone(marker)
        self.assertLess(
            marker.count, run_demo.MIN_MARKER_PIXELS,
            "a 20x20 speck must not be believed to be a 560x44 field",
        )


class ReceiptDecodingIsDiscriminating(unittest.TestCase):
    """What the band decoder accepts and rejects, on frames built here.

    Binary-free. The positive check replaced a pixel *diff* as the gate's
    acceptance criterion, so its own discriminating power has to be pinned in
    process rather than only exercised against the real app.
    """

    WIDTH, HEIGHT = 640, 480
    STRIDE = WIDTH * 4
    BAND_TOP = run_demo.BAND_TOP

    def _paint(self, bands, *, band_top=None) -> "vitrin_os.Frame":
        """A white frame with `bands` filling everything below `band_top`."""
        top = self.BAND_TOP if band_top is None else band_top
        buf = bytearray(b"\xff\xff\xff\xff" * (self.WIDTH * self.HEIGHT))
        span = self.HEIGHT - top
        height = span // len(bands)
        for index, rgb in enumerate(bands):
            y0 = top + index * height
            y1 = self.HEIGHT if index == len(bands) - 1 else y0 + height
            pixel = bytes((rgb[2], rgb[1], rgb[0], 0xFF))
            for y in range(y0, y1):
                base = y * self.STRIDE
                buf[base : base + self.WIDTH * 4] = pixel * self.WIDTH
        return vitrin_os.Frame(
            bytes(buf), format=vitrin_os.Format.XRGB8888,
            width=self.WIDTH, height=self.HEIGHT, stride=self.STRIDE,
        )

    def test_the_right_record_matches(self):
        bands = run_demo.receipt_bands(TASK)
        frame = self._paint(bands)
        self.assertTrue(run_demo.match_bands(run_demo.solid_row_runs(frame, bands), bands))

    def test_a_wrong_record_does_not_match(self):
        right = run_demo.receipt_bands(TASK)
        wrong = run_demo.receipt_bands(WRONG_TASK)
        self.assertNotEqual(right, wrong, "the fixture only tests anything if they differ")
        frame = self._paint(right)
        self.assertFalse(
            run_demo.match_bands(run_demo.solid_row_runs(frame, wrong), wrong),
            "a record the agent was never given must not match the frame the right "
            "record painted",
        )

    def test_a_reordered_record_does_not_match(self):
        forward = TASK
        backwards = tuple(reversed(TASK))
        frame = self._paint(run_demo.receipt_bands(forward))
        wrong = run_demo.receipt_bands(backwards)
        self.assertFalse(
            run_demo.match_bands(run_demo.solid_row_runs(frame, wrong), wrong),
            "order is part of the record; the same pairs reversed must not match",
        )

    def test_the_bands_must_appear_in_order(self):
        bands = run_demo.receipt_bands(TASK)
        frame = self._paint(tuple(reversed(bands)))
        self.assertFalse(
            run_demo.match_bands(run_demo.solid_row_runs(frame, bands), bands),
            "the three bands carry 36 bits only because their ORDER is part of the "
            "claim; an unordered set match would throw that away",
        )

    def test_a_blank_frame_matches_nothing(self):
        bands = run_demo.receipt_bands(TASK)
        frame = self._paint(((0xFF, 0xFF, 0xFF),))
        self.assertFalse(run_demo.match_bands(run_demo.solid_row_runs(frame, bands), bands))

    def test_bands_thinner_than_the_floor_are_rejected(self):
        # Squeeze all three bands into fewer rows than MIN_BAND_ROWS each: the
        # colours are all present, in order, and must still be refused.
        bands = run_demo.receipt_bands(TASK)
        frame = self._paint(bands, band_top=self.HEIGHT - 3 * (run_demo.MIN_BAND_ROWS - 1))
        self.assertFalse(
            run_demo.match_bands(run_demo.solid_row_runs(frame, bands), bands),
            f"a band under {run_demo.MIN_BAND_ROWS} rows is a strip, not a band",
        )

    def test_adjacent_identical_bands_still_match(self):
        # A hash collision between two adjacent bands paints ONE taller run.
        # Refusing that would fail a legitimate record ~1-in-4096 of the time.
        colour = (0x33, 0x66, 0x99)
        other = (0xCC, 0x00, 0x11)
        frame = self._paint((colour, colour, other))
        self.assertTrue(
            run_demo.match_bands(
                run_demo.solid_row_runs(frame, (colour, colour, other)),
                (colour, colour, other),
            )
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
        _REPO / "crates/xtask/src/main.rs",
        _REPO / "examples/agent-demo/run_demo.py",
    )

    def test_mock_shim_is_not_constructed_by_either_demo_launcher(self):
        for path in self._CHECKED:
            text = path.read_text()
            for needle in self._FORBIDDEN:
                self.assertNotIn(
                    needle,
                    text,
                    f"{path.relative_to(_REPO)} still constructs a path/argument "
                    f"from the literal {needle} -- issue #110 requires vitrin-mock-shim "
                    "appear in no demo venue",
                )


if __name__ == "__main__":
    unittest.main()
