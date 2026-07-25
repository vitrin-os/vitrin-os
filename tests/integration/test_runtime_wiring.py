# SPDX-License-Identifier: Apache-2.0
"""Issue #77's acceptance criteria, against the shipped binary.

Each test here corresponds to a criterion that `cargo test --workspace`
structurally cannot discharge, because the in-process tests build the
runtime by calling `start_realm_in` and never execute `run_session` — the
startup ordering the shipped binary actually uses.

Every test runs under a hard deadline and reaps its cores; see
:class:`harness.IntegrationTest` for why both are load-bearing here.
"""

from __future__ import annotations

import hashlib
import time
import unittest

from harness import (
    IntegrationTest,
    capture_when_ready,
    children_of,
    comm_of,
    require_binaries,
    whole_realm_grant,
)

require_binaries()

from vitrin_os import errors  # noqa: E402  (needs PYTHONPATH, which run.sh sets)


class EndToEnd(IntegrationTest):
    """**AC1** — an agent drives the shipped binary, start to finish.

    This is also the regression test for issue #77's trap T1, the wedge: if
    a change ever registered the shim socketpair's event source *after* the
    fork, the shim would block on `configure` forever, no frame would ever
    compose, and `observe()` here would never return. Nothing else in the
    repo would notice.
    """

    def test_agent_binds_petitions_captures_and_actuates(self):
        core = self.core()
        conn = core.connect()
        grant = whole_realm_grant(conn)

        first = capture_when_ready(grant)
        self.assertEqual((first.width, first.height), (320, 200))
        self.assertEqual(len(first.raw), first.stride * first.height)

        grant.pointer.click(160, 100)
        grant.text.type("hello from an agent")

        # The mock shim animates, so a later capture must differ. This is
        # what separates "capture returned bytes" from "capture returned
        # *this realm's current* bytes" — a stale cache or a scrubbed
        # buffer would pass the assertions above and fail here.
        time.sleep(0.6)
        second = grant.observe()
        self.assertNotEqual(
            hashlib.sha256(first.raw).hexdigest(),
            hashlib.sha256(second.raw).hexdigest(),
            "two captures of an animating realm must differ; identical bytes mean "
            "capture is serving something other than the live composite",
        )

        conn.close()
        core.terminate()

        kinds = core.kinds()
        for expected in ("run_started", "realm_spawned", "handshake_bound",
                         "petition_requested", "petition_resolved", "run_ended"):
            self.assertIn(expected, kinds, f"flight recorder must record {expected}")

        verbs = {
            e.get("verb") for e in core.entries()
            if e["kind"] == "use_decision" and e.get("decision") == "allowed"
        }
        self.assertLessEqual(
            {"observe", "actuate_pointer", "actuate_text"}, verbs,
            "every capture and actuation must pass the enforcement chokepoint and be "
            f"recorded there; saw {verbs}",
        )


class ProcessTree(IntegrationTest):
    """**AC2** — the process tree shows core → shim.

    Discharges issue #31's first acceptance criterion, which until #77 was
    met by tests only: a running `vitrind` forked nothing at all.
    """

    def test_the_realm_is_a_real_forked_child(self):
        core = self.core()
        names = {pid: comm_of(pid) for pid in children_of(core.pid)}
        self.assertTrue(
            any("mock-shi" in name or "shim" in name for name in names.values()),
            f"the core must fork a shim; children were {names}",
        )


class Enforcement(IntegrationTest):
    """The capability claim: authority is the core's to refuse, not the
    agent's to respect.

    An agent holding an observe-only grant that actuates anyway must be
    refused **by the core**, over the wire — not stopped client-side by the
    SDK, which would make the enforcement story a matter of the agent's
    good manners.
    """

    def test_a_verb_the_grant_never_carried_is_refused(self):
        core = self.core()
        conn = core.connect()
        grant = whole_realm_grant(conn, verbs=("observe",))

        capture_when_ready(grant)  # granted, so this must work

        with self.assertRaises(errors.GrantRefused) as caught:
            grant.pointer.click(100, 100)
        self.assertIsInstance(caught.exception, errors.NotGranted)

        conn.close()
        core.terminate()

        refusals = [
            e for e in core.entries()
            if e["kind"] == "use_decision" and e.get("decision") == "refused"
        ]
        self.assertTrue(
            any(e.get("verb") == "actuate_pointer" for e in refusals),
            f"the refusal must be recorded at the chokepoint; refusals were {refusals}",
        )


class Teardown(IntegrationTest):
    """**AC4** and the shutdown path that used to skip it.

    Grants die with their connection. The core-stops-first case was for a
    while the one close path that dropped the connection table silently,
    leaving a log whose last word on a live grant was `petition_resolved` —
    which cannot distinguish a released grant from one the core lost track
    of.
    """

    def test_closing_a_connection_removes_its_grant_rows(self):
        core = self.core()
        conn = core.connect()
        whole_realm_grant(conn)
        conn.close()
        time.sleep(0.5)
        core.terminate()

        kinds = core.kinds()
        self.assertIn("grant_removed", kinds)
        self.assertIn("connection_teardown", kinds)

    def test_shutdown_tears_down_a_connection_still_holding_a_grant(self):
        core = self.core()
        conn = core.connect()
        whole_realm_grant(conn)
        # The agent never disconnects: the core stops underneath it, which
        # is what SIGTERM on a live session does.
        core.terminate()

        kinds = core.kinds()
        self.assertIn(
            "grant_removed", kinds,
            "a grant live at shutdown must have its end recorded; without it the log "
            f"cannot say whether the core released it or lost it. entries: {kinds}",
        )
        self.assertIn("connection_teardown", kinds)
        self.assertLess(
            kinds.index("connection_teardown"), kinds.index("run_ended"),
            "teardown must land inside the run it belongs to, before the footer",
        )


class SingleCorePerRuntimeTree(IntegrationTest):
    """**AC5** — two cores on one tree: the second refuses.

    The refusal is only half the claim, and the half a lazy test stops at.
    The other half is that the first core is *undamaged* — because the
    failure guarded against is not "two cores ran" but "core B unlinked core
    A's socket and purged the realm directory A's live shim is bound to,
    then bound `core.sock` itself, so new agents reached B while the
    authority they think they hold lived in A".
    """

    def test_the_second_core_refuses_and_leaves_the_first_serving(self):
        first = self.core()
        tree_before = sorted(p.name for p in (first.runtime / "vitrin-0").iterdir())

        second = self.core(runtime_dir=first.runtime, wait=False)
        self.assertNotEqual(
            second.await_exit(timeout=30), 0, "the second core must refuse to start"
        )
        self.assertIn(
            "core.sock.lock", second.output(),
            f"the refusal must name the lock it lost. output: {second.output()}",
        )

        self.assertIsNone(
            first.proc.poll(), "the first core must survive the second's attempt"
        )
        self.assertEqual(
            tree_before,
            sorted(p.name for p in (first.runtime / "vitrin-0").iterdir()),
            "the refused core must leave the running core's runtime tree untouched",
        )

        # The strongest form of "undamaged": it is still serving.
        conn = first.connect()
        capture_when_ready(whole_realm_grant(conn))
        conn.close()


class BootRefusals(IntegrationTest):
    """The R6 guard, which fails closed on a registry it cannot audit.

    Worth an integration test rather than trusting the unit tests: this
    check runs before anything else in `run_session`, so it is the one
    refusal a startup-ordering regression would silently skip.
    """

    def test_auto_approve_refuses_a_world_readable_registry(self):
        staged = self.core(wait=False)
        staged.terminate()
        staged.principals.chmod(0o644)  # the mode the guard exists to catch

        relaxed = self.core(
            runtime_dir=staged.runtime, wait=False, write_config=False
        )
        self.assertNotEqual(
            relaxed.await_exit(timeout=30), 0,
            "auto-approve must refuse a registry it cannot audit",
        )
        self.assertIn("chmod 600", relaxed.output())


if __name__ == "__main__":
    unittest.main()
