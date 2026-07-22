"""P1.8.4 (#43) acceptance: the demo agent's HEADLESS flow, against a live core.

This is the M1.5 acceptance gate, wired into the integration suite so it rides
``run.sh``'s ``unittest discover`` with no CI-yaml edit — the entry-point
contract (``tests/integration/README.md``) is exactly that a new
``test_*.py`` is the whole change.

The demo's *entry point* is imported and called (not run as a subprocess), so
a failure surfaces as a Python traceback with the demo's own frame dump rather
than an opaque non-zero exit. What only a live core can show — and what the
mock-based SDK tests cannot — is that the real enforcement chokepoint records
the demo's actuations: an allowed ``move`` at the clicked coordinate and an
allowed ``type`` whose ``chars`` equals the URL's length, in the order the
recorder is meant to reconstruct.

The nested venue (real Firefox) is the workstation half of ``cargo xtask
demo``; it has no display or browser on a CI runner and is deliberately not
exercised here (plan risk R6/R1).
"""

from __future__ import annotations

import pathlib
import sys
import tempfile
import unittest

from harness import IntegrationTest, require_binaries

require_binaries()

# The demo is a shipped example, not a package: put its directory on the path
# and import its entry point, so a failure is a traceback in *our* process.
_DEMO_DIR = pathlib.Path(__file__).resolve().parents[2] / "examples" / "agent-demo"
sys.path.insert(0, str(_DEMO_DIR))

import run_demo  # noqa: E402


def _use_decisions(entries: list[dict]) -> list[dict]:
    return [e for e in entries if e["kind"] == "use_decision"]


def _allowed(entries: list[dict], action: str) -> list[dict]:
    return [
        e
        for e in _use_decisions(entries)
        if e.get("decision") == "allowed" and (e.get("input") or {}).get("action") == action
    ]


class DemoHeadless(IntegrationTest):
    """The headless demo drives the shipped core end to end and reconstructs.

    Uses a mock shim started with ``--seat --animate`` so seat events deliver
    and the two captures differ; the demo's ``run`` entry point does the rest
    (connect, the one MVP grant, consent, capture/click/type/capture).
    """

    def test_demo_runs_and_the_recorder_reconstructs_the_session(self):
        # `--seat` so routed seat events have somewhere to land; the harness's
        # default animate budget outlives the one capture-diff this needs.
        core = self.core(seat=True)
        out_dir = pathlib.Path(tempfile.mkdtemp(prefix="vitrin-demo-test-"))

        result = run_demo.run(
            str(core.socket),
            headless=True,
            consent="auto-approve",
            out_dir=out_dir,
            recorder=str(core.recorder),
        )

        # 1) The demo succeeded, and its two captures genuinely differ.
        self.assertTrue(result.ok)
        self.assertNotEqual(
            result.before.raw,
            result.after.raw,
            f"the animation must advance between captures; frames dumped under {out_dir}",
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

        # 5) The typed string's shape reached the chokepoint: chars == len(url) + 1
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
        self.assertLess(move_idx, type_idx, "the click (move) must precede the typed URL")


if __name__ == "__main__":
    unittest.main()
